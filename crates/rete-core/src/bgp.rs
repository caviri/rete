//! Basic Graph Pattern (BGP) evaluation — the core of SPARQL (SPEC.md §8,
//! stage 1). A BGP is a set of triple patterns whose variables join on equality;
//! evaluating it yields variable bindings.
//!
//! Evaluation is a left-deep **hash join**: each pattern is scanned from the
//! index *once* (binding only its own constant terms), producing a relation of
//! candidate rows; that relation is then joined against the running solution set
//! on the variables they share. This is O(scan + matches) per pattern rather
//! than the O(bindings × scan) of a per-binding nested-loop probe — the
//! difference between sub-second and minutes once a pattern binds tens of
//! thousands of rows. Correctness does not depend on pattern order.
//!
//! Solutions are slot `Row`s of tagged dictionary ids (see `crate::row`):
//! joins hash and compare integers, and terms are resolved to strings only at
//! the engine's projection boundary, never per intermediate row.

use std::collections::{BTreeMap, HashMap};

use crate::file::Rete;
use crate::index::{GraphIndex, Pattern, ProbeBatch};
use crate::row::{Ctx, Row, Slots, Val};
use crate::triples::Triple;

/// A term in a pattern: a named variable or a constant term token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatternTerm {
    Var(String),
    Const(String),
}

impl PatternTerm {
    /// `?x` → variable `x`; anything else → a constant.
    pub fn parse(token: &str) -> Self {
        if let Some(name) = token.strip_prefix('?') {
            PatternTerm::Var(name.to_string())
        } else {
            PatternTerm::Const(token.to_string())
        }
    }
}

/// A triple pattern `(subject, predicate, object)` of [`PatternTerm`]s.
#[derive(Debug, Clone)]
pub struct TriplePattern {
    pub s: PatternTerm,
    pub p: PatternTerm,
    pub o: PatternTerm,
}

/// A solution: variable name → bound term (the public, resolved form).
pub type Binding = BTreeMap<String, String>;

// --- integer join core ------------------------------------------------------
//
// Variables bind to a tagged i64: a node ID `n` is stored as `n` (>= 0), a
// predicate ID `p` as `-(p+1)` (< 0). Nodes are the unified subject/object space
// so a variable joins consistently across subject and object positions. A
// predicate value whose term is also a node is canonicalized to the node id
// (`Resolver::canon_id`), so cross-role joins match exactly when the term
// strings match.

/// A pattern position lowered to slot/integer space.
#[derive(Clone, Copy)]
enum SlotTerm {
    Var(usize),
    Node(u32),
    Pred(u32),
}

fn pred_tag(p: u32) -> i64 {
    -(p as i64) - 1
}

/// Register every variable in `patterns` with the slot map.
pub(crate) fn collect_pattern_slots(patterns: &[TriplePattern], slots: &mut Slots) {
    for p in patterns {
        for t in [&p.s, &p.p, &p.o] {
            if let PatternTerm::Var(v) = t {
                slots.add(v);
            }
        }
    }
}

/// Lower patterns to slot/integer space; `None` if a constant term is unknown
/// (making the whole BGP unsatisfiable) or a variable has no slot.
fn lower(patterns: &[TriplePattern], ctx: &Ctx) -> Option<Vec<(SlotTerm, SlotTerm, SlotTerm)>> {
    let dict = ctx.rete.dictionary();
    let node = |t: &PatternTerm| -> Option<SlotTerm> {
        match t {
            PatternTerm::Var(v) => ctx.slots.slot(v).map(SlotTerm::Var),
            PatternTerm::Const(c) => dict.node_of_term(c).map(SlotTerm::Node),
        }
    };
    let pred = |t: &PatternTerm| -> Option<SlotTerm> {
        match t {
            PatternTerm::Var(v) => ctx.slots.slot(v).map(SlotTerm::Var),
            PatternTerm::Const(c) => dict.predicate_id(c).map(SlotTerm::Pred),
        }
    };
    let mut lowered = Vec::with_capacity(patterns.len());
    for p in patterns {
        lowered.push((node(&p.s)?, pred(&p.p)?, node(&p.o)?));
    }
    Some(lowered)
}

/// Evaluate a BGP against the file's default graph, returning all solutions
/// resolved to terms (the public convenience API).
pub fn eval_bgp(rete: &Rete, patterns: &[TriplePattern]) -> Vec<Binding> {
    let mut slots = Slots::new();
    collect_pattern_slots(patterns, &mut slots);
    let ctx = Ctx::new(rete, slots);
    eval_bgp_rows(&ctx, rete.default_index(), patterns)
        .into_iter()
        .map(|row| row_to_binding(&ctx, &row))
        .collect()
}

/// Resolve a row to a named-term binding (every bound slot). Uses the uncached
/// decode — this is the output boundary, where terms are typically seen once.
pub(crate) fn row_to_binding(ctx: &Ctx, row: &Row) -> Binding {
    let mut b = Binding::new();
    for (i, v) in row.iter().enumerate() {
        if let Some(val) = v {
            if let Some(t) = ctx.resolver.str_once(val) {
                b.insert(ctx.slots.name(i).to_string(), t);
            }
        }
    }
    b
}

/// The join order of `lowered` and what a full scan of each pattern would
/// produce (its own constants only, from the tile directory): the probe-vs-scan
/// decisions compare the latter with the running result.
///
/// A single pattern has one order and nothing to compare its scan with, so it
/// is not estimated at all. Estimating reads the tile-directory leaves of every
/// permutation that leads with a bound id, one after another: on the 62 GB
/// OpenCitations file `<br> ?p ?o` read two 520 KB leaves, the second of them
/// for a permutation the scan never used, plus a dictionary chunk for
/// `rdf:type`, each a dependent round trip.
fn plan_order(
    ctx: &Ctx,
    index: &GraphIndex,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
) -> (Vec<usize>, Vec<f64>) {
    if lowered.len() == 1 {
        return (vec![0], vec![0.0]);
    }
    let order = selectivity_order(ctx, index, lowered);
    let scan_est = tile_estimates(ctx, index, lowered, &std::collections::HashSet::new());
    (order, scan_est)
}

/// Evaluate a BGP to slot rows against a specific graph `index` (joins run on
/// integer ids; the shared dictionary comes from `ctx`).
pub(crate) fn eval_bgp_rows(ctx: &Ctx, index: &GraphIndex, patterns: &[TriplePattern]) -> Vec<Row> {
    // An empty BGP has exactly one (empty) solution.
    if patterns.is_empty() {
        return vec![ctx.slots.empty_row()];
    }
    // Lower all patterns; a missing constant term makes the BGP empty.
    let Some(lowered) = lower(patterns, ctx) else {
        return Vec::new();
    };

    // Join the patterns most-constrained-first (and keeping the join connected),
    // so intermediate relations stay small. Pure reordering: the hash join is
    // order-independent, so the result is unchanged.
    let (order, scan_est) = plan_order(ctx, index, &lowered);
    let mut rows: Vec<Row> = vec![ctx.slots.empty_row()];
    let mut bound: Vec<usize> = Vec::new();
    let mut merged: [bool; 2] = [false, false];

    // Merge-join seed: if the two cheapest patterns share a same-role variable the
    // 6-permutation index can co-sort them on, join them with a linear two-pointer
    // **merge** (no hash table) and skip both below. Falls back silently to the
    // hash/probe path when the shape doesn't qualify.
    if order.len() >= 2 {
        if let Some((rel, slots)) =
            try_merge_join(ctx, index, &lowered[order[0]], &lowered[order[1]])
        {
            rows = rel;
            bound = slots;
            merged = [true, true];
        }
    }

    for (k, &idx) in order.iter().enumerate() {
        if k < 2 && merged[k] {
            continue; // already consumed by the merge seed
        }
        let t = lowered[idx];
        // Hybrid join: once the running result is small and the next pattern shares
        // an already-bound variable, **probe** it per row through the index instead
        // of scanning its whole extent and hash-joining. A left-deep hash join
        // otherwise materializes every pattern's full scan even when only a handful
        // of rows survive — e.g. a `?x rdf:type C` over thousands of instances when
        // the prefix already pinned ?x to 26 rows. Probing turns that O(scan) into
        // O(rows × lookup). The full scan + hash join stays the path when the prefix
        // is large (one scan beats many probes) or the pattern is a cartesian join
        // (no shared bound variable to constrain the probe).
        let shares_bound = pattern_slots(&t).iter().any(|s| bound.contains(s));
        // Decide probe (per-row index lookup) vs scan + hash join. In memory a
        // probe is a cheap lookup, so we probe up to a moderate prefix. Remotely
        // each probe is a byte-range round-trip: probing a 500-row prefix is 500
        // sequential fetches, while scanning a selectively-bound pattern is a few
        // coalesced reads — so remotely we only probe a broad (whole-predicate)
        // pattern, or a tiny prefix, and never a large one.
        //
        // Whatever the reader, probe when the scan would be far larger than
        // the rows asking about it: 517 `?s a :Service` rows must not scan
        // and materialize all 109 M `publicationYear` triples to keep 517.
        let do_probe = !bound.is_empty()
            && shares_bound
            && !rows.is_empty()
            && (probe_beats_scan(rows.len(), scan_est[idx])
                || if index.is_remote() {
                    rows.len() <= remote_probe_max(index)
                        && (rows.len() <= REMOTE_PROBE_MIN || !pattern_is_selective(&t))
                } else {
                    rows.len() <= BGP_PROBE_THRESHOLD
                });
        if do_probe {
            let taken = std::mem::take(&mut rows);
            // Over a lazy reader each probe is a byte-range round trip, and they run
            // one at a time. Fault the tiles each window of the batch will route to,
            // together — one coalesced parallel read per section instead of N
            // sequential ones — so the probes hit a warm tile cache. A window, not
            // the whole batch: see [`Windowed`]. Passes the rows straight through
            // for a local index (every tile is already resident).
            let mut next: Vec<Row> = Vec::with_capacity(taken.len());
            for base in windowed(index, taken.into_iter(), bgp_probe_pattern(ctx, t)) {
                next.extend(probe_rows(ctx, index, t, base));
            }
            rows = next;
        } else {
            let Some((rel, rel_slots)) = pattern_rows(ctx, index, &t) else {
                return Vec::new();
            };
            rows = hash_join(rows, &bound, rel, &rel_slots);
        }
        for s in pattern_slots(&t) {
            if !bound.contains(&s) {
                bound.push(s);
            }
        }
        if rows.is_empty() {
            break;
        }
    }
    rows
}

/// Merge-join two base patterns that share **exactly one** variable sitting in the
/// **same canonical position** (both subject, or both object) in each. That is the
/// only shape rete's role-aware id spaces let a sort-merge align: the index sorts
/// subject-ids and object-ids in independent spaces, so a cross-role (subject↔
/// object) join's two streams are not co-sorted on the join value. Each side is
/// streamed from the 6-permutation index already sorted on that column
/// ([`GraphIndex::scan_iter_sorted_on`]), so the join is a linear two-pointer
/// merge with no hash table — equal-key runs cross-product. Returns
/// `(rows, bound_slots)`, or `None` when the shape/index can't support a merge and
/// the caller should hash-join instead. The result multiset equals the hash
/// join's (a join is order-independent).
fn try_merge_join(
    ctx: &Ctx,
    index: &GraphIndex,
    ta: &(SlotTerm, SlotTerm, SlotTerm),
    tb: &(SlotTerm, SlotTerm, SlotTerm),
) -> Option<(Vec<Row>, Vec<usize>)> {
    let dict = ctx.rete.dictionary();
    let sa = pattern_slots(ta);
    let sb = pattern_slots(tb);
    let shared: Vec<usize> = sa.iter().copied().filter(|s| sb.contains(s)).collect();
    if shared.len() != 1 {
        return None;
    }
    let v = shared[0];
    let col_of = |t: &(SlotTerm, SlotTerm, SlotTerm)| -> Option<usize> {
        [&t.0, &t.1, &t.2]
            .iter()
            .position(|x| matches!(x, SlotTerm::Var(i) if *i == v))
    };
    let ca = col_of(ta)?;
    let cb = col_of(tb)?;
    if ca != cb {
        return None; // cross-role: the two role id-spaces aren't co-sorted
    }
    // A merge MATERIALIZES both sides in full (`collect` below builds a Vec per
    // side). When one side is a pinpoint (a bound leading key routing to a few
    // KB of tiles) and the other is fat (a broad predicate spanning megabytes),
    // the linear merge costs building — and, on a remote index, FETCHING — the
    // fat extent end-to-end: gigabytes over HTTP, and the 32-bit wasm OOM
    // behind `?x wdt:P279 <C> . ?x rdfs:label ?l` on a 185M-triple remote
    // graph. The hybrid loop handles that shape strictly better (scan the
    // pinpoint side, probe the fat one per surviving row), so leave the merge
    // seed to comparably-sized sides.
    if merge_seed_lopsided(ctx, index, ta, tb) {
        return None;
    }
    let lower = |t: &(SlotTerm, SlotTerm, SlotTerm)| -> Option<Pattern> {
        Some((
            const_subject(&t.0, dict)?,
            const_predicate(&t.1)?,
            const_object(&t.2, dict)?,
        ))
    };
    // Materialize each side as `(join-key, row)`, already ascending in the join
    // column (the index streams it sorted there).
    let collect = |t: &(SlotTerm, SlotTerm, SlotTerm), col: usize| -> Option<Vec<(u32, Row)>> {
        let pat = lower(t)?;
        let mut out = Vec::new();
        for tri in canon_ahead(ctx, t, index.scan_iter_sorted_on(pat, col)?) {
            if let Some(r) = triple_row(ctx, t, tri) {
                out.push(([tri.0, tri.1, tri.2][col], r));
            }
        }
        Some(out)
    };
    let rows_a = collect(ta, ca)?;
    let rows_b = collect(tb, cb)?;

    let mut out: Vec<Row> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < rows_a.len() && j < rows_b.len() {
        match rows_a[i].0.cmp(&rows_b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                let key = rows_a[i].0;
                let (i0, j0) = (i, j);
                while i < rows_a.len() && rows_a[i].0 == key {
                    i += 1;
                }
                while j < rows_b.len() && rows_b[j].0 == key {
                    j += 1;
                }
                for (_, ra) in &rows_a[i0..i] {
                    for (_, rb) in &rows_b[j0..j] {
                        // Combine the two rows; they agree on the shared slot `v`
                        // (same key ⇒ same node value at the same column).
                        let mut row = ra.clone();
                        let mut ok = true;
                        for (slot, val) in rb.iter().enumerate() {
                            if let Some(val) = val {
                                match &row[slot] {
                                    Some(existing) if existing != val => {
                                        ok = false;
                                        break;
                                    }
                                    _ => row[slot] = Some(val.clone()),
                                }
                            }
                        }
                        if ok {
                            out.push(row);
                        }
                    }
                }
            }
        }
    }
    let mut slots = pattern_slots(ta);
    for s in pattern_slots(tb) {
        if !slots.contains(&s) {
            slots.push(s);
        }
    }
    Some((out, slots))
}

/// Probe the join tail (instead of full-scanning) once the running BGP result is
/// at most this many rows: each remaining pattern that shares a bound variable is
/// then resolved by `rows × index-lookup` rather than a whole-extent scan. Kept
/// small so the probe count is always cheap in absolute terms — the dominant win
/// is the large-scan-after-tiny-prefix case (a `?x a C` over thousands when the
/// prefix already pinned ?x to a few dozen rows); a *moderately* large prefix
/// keeps the one-pass scan + hash join, which beats thousands of probes.
const BGP_PROBE_THRESHOLD: usize = 512;

/// A pattern whose estimated scan is below this many triples is never worth
/// planning around: scanning it is cheap whatever the other side holds. Keeps
/// every small graph (the W3C suite, the playground datasets) on the joins it
/// always used.
const PROBE_SCAN_FLOOR: f64 = 65_536.0;

/// Probe instead of scanning once the scan is at least this many times larger
/// than the rows that would probe it. A probe is a tile-directory binary search
/// plus a group lookup in an (often cached) tile; a scan pays per triple, and
/// on the hash-join path also materializes every one of them.
const PROBE_RATIO: f64 = 16.0;

/// Is probing `rows` bound rows cheaper than scanning a pattern estimated at
/// `scan_est` triples?
fn probe_beats_scan(rows: usize, scan_est: f64) -> bool {
    scan_est >= limits().probe_floor && (rows as f64) * PROBE_RATIO <= scan_est
}

/// The most rows that may probe a BGP whose cheapest pattern is estimated at
/// `est` triples while probing still beats evaluating it whole
/// ([`probe_beats_scan`]); `None` below the probe floor, where the BGP is
/// cheap to evaluate either way.
pub(crate) fn probe_row_budget(est: f64) -> Option<usize> {
    (est >= limits().probe_floor).then(|| (est / PROBE_RATIO) as usize)
}

/// The smallest tile-directory estimate ([`GraphIndex::estimate_matches`])
/// among a BGP's patterns — a floor on what evaluating it whole would scan.
/// `None` when a constant is not in the file (the BGP matches nothing).
pub(crate) fn bgp_min_estimate(
    ctx: &Ctx,
    index: &GraphIndex,
    patterns: &[TriplePattern],
) -> Option<f64> {
    let lowered = lower(patterns, ctx)?;
    tile_estimates(ctx, index, &lowered, &std::collections::HashSet::new())
        .into_iter()
        .reduce(f64::min)
}

/// The sum of the tile-directory estimates of a BGP's patterns: what evaluating
/// it whole reads when each pattern is scanned once. [`bgp_min_estimate`] is
/// only the cheapest pattern's share of that, so on a BGP of several mid-sized
/// patterns (a journal's title, identifiers and their scheme: 38 k rows each
/// under the probe floor, 5x that together) it stays under the floor and the
/// BGP is materialized for a handful of left rows. `None` as for
/// [`bgp_min_estimate`].
pub(crate) fn bgp_total_estimate(
    ctx: &Ctx,
    index: &GraphIndex,
    patterns: &[TriplePattern],
) -> Option<f64> {
    let lowered = lower(patterns, ctx)?;
    Some(
        tile_estimates(ctx, index, &lowered, &std::collections::HashSet::new())
            .into_iter()
            .sum(),
    )
}

/// Is `t` the pattern `?x rdf:type ?c` with `?x` among the slots `shared` with
/// the rows probing it, and `?c` not?
fn is_unbound_class(ctx: &Ctx, t: &(SlotTerm, SlotTerm, SlotTerm), shared: &[usize]) -> bool {
    let type_pid = ctx.rete.dictionary().predicate_id(crate::file::RDF_TYPE);
    matches!(
        t,
        (SlotTerm::Var(x), SlotTerm::Pred(p), SlotTerm::Var(c))
            if Some(*p) == type_pid && shared.contains(x) && !shared.contains(c) && x != c
    )
}

/// Is a BGP whose cheapest pattern is estimated at `est` rows fat enough that
/// probing it per outer row beats loading it? [`FAT_SCAN_ROWS`]; tests set the
/// probe floor instead, to drive the path over a graph small enough to check.
fn fat_enough(est: f64) -> bool {
    #[cfg(test)]
    if TEST_LIMITS.with(|c| c.get()).is_some() {
        return est >= limits().probe_floor;
    }
    est >= FAT_SCAN_ROWS
}

/// A BGP probed once per outer row instead of evaluated whole: the shape of a
/// correlated `FILTER (NOT) EXISTS` or `MINUS` over a BGP. Each outer row's
/// bound variables become constants of the probe ([`ProbeJoin`]), so a fat
/// BGP is never loaded; a row costs a few index lookups and stops at its
/// first solution. A BGP below [`FAT_SCAN_ROWS`] is cheap to load once and
/// index, so it keeps that plan: probing it would only fault scattered tiles.
pub(crate) struct CorrelatedProbe {
    /// `None`: a constant is not in the file, so no row has a solution.
    plan: Option<ProbePlan>,
    /// The variable slots the BGP mentions.
    slots: Vec<usize>,
}

impl CorrelatedProbe {
    /// `seed` marks the slots the outer rows certainly bind. `None` = evaluate
    /// whole: the BGP is empty, shares no variable with the outer side, or is
    /// not fat.
    pub(crate) fn new(
        ctx: &Ctx,
        index: &GraphIndex,
        patterns: &[TriplePattern],
        seed: &[bool],
    ) -> Option<Self> {
        if patterns.is_empty() {
            return None;
        }
        let mut slots: Vec<usize> = Vec::new();
        for p in patterns {
            for t in [&p.s, &p.p, &p.o] {
                if let PatternTerm::Var(v) = t {
                    if let Some(i) = ctx.slots.slot(v) {
                        if !slots.contains(&i) {
                            slots.push(i);
                        }
                    }
                }
            }
        }
        if !slots.iter().any(|&i| seed[i]) {
            return None;
        }
        // An unknown constant empties the BGP for every row.
        let Some(est) = bgp_min_estimate(ctx, index, patterns) else {
            return Some(CorrelatedProbe { plan: None, slots });
        };
        if !fat_enough(est) {
            return None;
        }
        Some(CorrelatedProbe {
            plan: ProbePlan::new(ctx, index, patterns, seed),
            slots,
        })
    }

    /// Does `row` bind a variable of the BGP?
    pub(crate) fn shares(&self, row: &Row) -> bool {
        self.slots.iter().any(|&i| row[i].is_some())
    }

    /// Does the BGP, with `row`'s bindings substituted, have a solution?
    /// Callers check [`Self::shares`] first: a row that binds none of the
    /// BGP's variables would re-evaluate all of it.
    pub(crate) fn probe(&self, ctx: &Ctx, index: &GraphIndex, row: &Row) -> bool {
        match &self.plan {
            None => false,
            Some(plan) => ProbeJoin::from_plan(ctx, index, plan, row.clone())
                .next()
                .is_some(),
        }
    }
}

/// Run `f` with the probe floor set to `floor` (tests only), so a join's
/// probe paths can be driven over a graph small enough to check by hand.
#[cfg(test)]
pub(crate) fn with_probe_floor<T>(floor: f64, f: impl FnOnce() -> T) -> T {
    let saved = TEST_LIMITS.with(|c| c.get());
    TEST_LIMITS.with(|c| {
        c.set(Some(PlanLimits {
            probe_floor: floor,
            ..limits()
        }))
    });
    let out = f();
    TEST_LIMITS.with(|c| c.set(saved));
    out
}

/// Run `f` with every planner threshold set (tests only): probe floor, the
/// streaming seed size and the hash-build size. Lets a test elsewhere in the
/// crate drive each join path over a graph small enough to build in a test.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn with_plan_limits<T>(
    probe_floor: f64,
    stream_seed: f64,
    hash_build: f64,
    f: impl FnOnce() -> T,
) -> T {
    let saved = TEST_LIMITS.with(|c| c.get());
    TEST_LIMITS.with(|c| {
        c.set(Some(PlanLimits {
            probe_floor,
            stream_seed,
            hash_build,
        }))
    });
    let out = f();
    TEST_LIMITS.with(|c| c.set(saved));
    out
}

/// The planner's size thresholds, gathered so tests can shrink them and drive
/// every join path over a graph small enough to check by hand.
#[derive(Clone, Copy)]
struct PlanLimits {
    probe_floor: f64,
    stream_seed: f64,
    hash_build: f64,
}

#[cfg(test)]
thread_local! {
    static TEST_LIMITS: std::cell::Cell<Option<PlanLimits>> = const { std::cell::Cell::new(None) };
}

fn limits() -> PlanLimits {
    #[cfg(test)]
    if let Some(l) = TEST_LIMITS.with(|c| c.get()) {
        return l;
    }
    PlanLimits {
        probe_floor: PROBE_SCAN_FLOOR,
        stream_seed: STREAM_SEED_ROWS,
        hash_build: HASH_BUILD_ROWS,
    }
}

/// A BGP whose most selective pattern is estimated above this many matches is
/// **streamed** rather than joined into a materialized prefix
/// ([`BgpSolutions`]): every later pattern is then joined per row (probe) or
/// against a small materialized pattern (hash), so resident memory no longer
/// follows the largest pattern. `?s dcite:doi ?d . ?s dcite:fromSnapshot ?x`
/// materialized 109 M rows before joining and ran a 16 GB container out of
/// memory.
const STREAM_SEED_ROWS: f64 = (1u64 << 21) as f64;

/// On the streaming path, a later pattern estimated at no more than this many
/// matches is scanned once into a hash table; a larger one is probed per row.
const HASH_BUILD_ROWS: f64 = (1u64 << 20) as f64;

/// Tile-directory cardinality estimates ([`GraphIndex::estimate_matches`]) for
/// each lowered pattern. A variable an outer seed row binds is a constant at
/// run time but unknown now: it scales the estimate by the same default
/// selectivities [`pattern_estimates`] falls back to. An unsatisfiable
/// constant estimates 0.
fn tile_estimates(
    ctx: &Ctx,
    index: &GraphIndex,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
    seed: &std::collections::HashSet<usize>,
) -> Vec<f64> {
    const SEL_SUBJECT: f64 = 0.001;
    const SEL_OBJECT: f64 = 0.02;
    const SEL_PREDICATE: f64 = 0.05;
    let dict = ctx.rete.dictionary();
    let total = ctx.rete.index_triples(index);
    let seeded = |x: &SlotTerm| matches!(x, SlotTerm::Var(v) if seed.contains(v));
    // A seeded subject/object of a constant predicate takes that predicate's
    // MEASURED selectivity from `query_stats` (the SCHEMA section, kind 9, or
    // a resident pyramid-meta) instead of the fixed default. The tile
    // directory cannot see a value that only exists at run time, so this is
    // the one estimate here the statistics improve. They are read only when
    // some pattern needs them: one small range, once per session, on a lazy
    // open — never for a query whose patterns have no seeded position.
    let needs_stats = lowered
        .iter()
        .any(|t| matches!(t.1, SlotTerm::Pred(_)) && (seeded(&t.0) || seeded(&t.2)));
    let stats: std::collections::HashMap<u32, &crate::meta::PredStat> = if needs_stats {
        ctx.rete
            .planner_stats(true)
            .iter()
            .map(|s| (s.predicate, s))
            .collect()
    } else {
        std::collections::HashMap::new()
    };
    // Each pattern is estimated on every permutation that leads with a bound id,
    // reading the directory leaves its run starts and ends in: fault them all
    // for all the patterns at once, not one permutation after another.
    let patterns: Vec<Pattern> = lowered
        .iter()
        .filter_map(|t| {
            Some((
                const_subject(&t.0, dict)?,
                const_predicate(&t.1)?,
                const_object(&t.2, dict)?,
            ))
        })
        .collect();
    index.prefault_estimate_leaves(&patterns);
    lowered
        .iter()
        .map(|t| {
            let (Some(s), Some(p), Some(o)) = (
                const_subject(&t.0, dict),
                const_predicate(&t.1),
                const_object(&t.2, dict),
            ) else {
                return 0.0;
            };
            let mut est = index.estimate_matches((s, p, o), total);
            if est == 0.0 {
                return 0.0;
            }
            let st = match t.1 {
                SlotTerm::Pred(pid) => stats.get(&pid).copied(),
                _ => None,
            };
            if seeded(&t.0) {
                est *= match st {
                    Some(st) if st.distinct_subjects > 0 => 1.0 / st.distinct_subjects as f64,
                    _ => SEL_SUBJECT,
                };
            }
            if seeded(&t.1) {
                est *= SEL_PREDICATE;
            }
            if seeded(&t.2) {
                est *= match st {
                    Some(st) if st.distinct_objects > 0 => 1.0 / st.distinct_objects as f64,
                    _ => SEL_OBJECT,
                };
            }
            est.max(1.0)
        })
        .collect()
}

/// Remote (lazy) probe bounds. Over HTTP byte-range reads, a per-row probe is a
/// network round-trip, so we probe far more reluctantly than in memory: only when
/// scanning the next pattern would be a *broad* read (a whole-predicate scan,
/// [`pattern_is_selective`] is false), or the prefix is tiny enough that a handful
/// of probes is trivial either way. A pattern that carries its own bound
/// subject/object routes to a narrow index range — cheaper to scan once and
/// hash-join than to probe per prefix row. Probing a large prefix is always a
/// loss remotely, so it is capped.
const REMOTE_PROBE_MIN: usize = 8;
const REMOTE_PROBE_MAX: usize = 1024;

/// The remote probe budget, widened by the reader's concurrency: a serial
/// sync-XHR reader (a phone without the COI pool) pays one full round trip per
/// probe, so its budget stays at [`REMOTE_PROBE_MAX`]; a reader overlapping
/// 16 range reads (the CLI's thread pool, the asyncified fetch variant, the
/// COI fetch-worker pool) amortizes ~16 probes per round trip and gets a
/// proportionally larger budget before the one-pass scan wins again.
fn remote_probe_max(index: &GraphIndex) -> usize {
    REMOTE_PROBE_MAX * index.read_concurrency().clamp(1, 16)
}

/// The rows a first [`Windowed`] window holds: few enough that a `LIMIT 1` or an
/// `ASK` that stops after one row costs at most a few parallel reads.
const WINDOW_START_ROWS: usize = 256;

/// The most rows one [`Windowed`] window holds. The window doubles from
/// [`WINDOW_START_ROWS`] (times four) up to this; the tile and byte bound of
/// [`ProbeBatch`] cuts it shorter where the rows' tiles are many or large.
const WINDOW_MAX_ROWS: usize = 1 << 16;

/// Rows pulled and routed together while a window fills: their directory
/// leaves are faulted in one read.
const WINDOW_CHUNK_ROWS: usize = 256;

/// How a row becomes the pattern its probe will run.
type ProbePattern<'a> = Box<dyn Fn(&Row) -> Option<Pattern> + 'a>;

/// A stream of rows about to be **probed** against a remote index, handed on
/// unchanged and in order, with each window's tiles faulted in one coalesced,
/// parallel read ([`GraphIndex::probe_batch_fault`]) just before the window is
/// consumed.
///
/// A probe join over a lazy reader otherwise faults one tile per probe, one
/// round trip at a time (the lazy streaming join did exactly that), or — when
/// the whole prefix was prefetched at once — faulted more tiles than the tile
/// cache holds, so the early ones were evicted before their probes ran and
/// came back one request each (G50a: 15,539 requests at 0.12 s for a 221,817
/// row prefix). A window grows until its tiles fill a [`ProbeBatch`] (tiles and
/// stored bytes, within the cache budget), so the reader's fan-out is used (up
/// to `read_concurrency` requests in flight) and memory stays bounded; it
/// starts small and grows fourfold, so a demand-bounded consumer reads only a little
/// past what it uses.
///
/// A local index has nothing to fault: the stream passes through untouched.
pub(crate) struct Windowed<'a> {
    index: &'a GraphIndex,
    upstream: Box<dyn Iterator<Item = Row> + 'a>,
    /// `None`: pass through (a local index).
    pat_of: Option<ProbePattern<'a>>,
    /// Pulled from upstream, not yet in a window.
    pending: std::collections::VecDeque<Row>,
    /// The window whose tiles were requested (or are resident): ready to probe.
    ready: std::collections::VecDeque<Row>,
    /// The next window's row cap.
    size: usize,
}

/// Fault `rows`' probe tiles a window at a time as they are consumed.
pub(crate) fn windowed<'a>(
    index: &'a GraphIndex,
    upstream: impl Iterator<Item = Row> + 'a,
    pat_of: impl Fn(&Row) -> Option<Pattern> + 'a,
) -> Windowed<'a> {
    Windowed {
        index,
        upstream: Box::new(upstream),
        pat_of: index.is_remote().then(|| {
            let f: ProbePattern<'a> = Box::new(pat_of);
            f
        }),
        pending: std::collections::VecDeque::new(),
        ready: std::collections::VecDeque::new(),
        size: WINDOW_START_ROWS,
    }
}

impl Windowed<'_> {
    /// Fill `ready` with the next window and fault its tiles.
    fn refill(&mut self, pat_of: &dyn Fn(&Row) -> Option<Pattern>) {
        let mut batch = ProbeBatch::default();
        'window: while self.ready.len() < self.size {
            let chunk = WINDOW_CHUNK_ROWS.min(self.size - self.ready.len());
            while self.pending.len() < chunk {
                match self.upstream.next() {
                    Some(r) => self.pending.push_back(r),
                    None => break,
                }
            }
            if self.pending.is_empty() {
                break;
            }
            let chunk = chunk.min(self.pending.len());
            // One batched read for the chunk's directory leaves.
            let pats: Vec<Pattern> = self.pending.iter().take(chunk).filter_map(pat_of).collect();
            self.index.prefault_leaves(&pats);
            for _ in 0..chunk {
                if let Some(p) = pat_of(&self.pending[0]) {
                    if !self.index.probe_batch_add(&mut batch, p) {
                        break 'window; // the window is full: this row opens the next
                    }
                }
                let row = self.pending.pop_front().expect("chunk rows are pending");
                self.ready.push_back(row);
            }
        }
        self.index.probe_batch_fault(batch);
        self.size = (self.size * 4).min(WINDOW_MAX_ROWS);
    }
}

impl Iterator for Windowed<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        // Taken out so `refill` can borrow `self` mutably, and put back.
        let Some(pat_of) = self.pat_of.take() else {
            return self.upstream.next();
        };
        if self.ready.is_empty() {
            self.refill(pat_of.as_ref());
        }
        self.pat_of = Some(pat_of);
        self.ready.pop_front()
    }
}

/// The probe pattern a lowered triple pattern runs for `base`: every term the
/// row binds is a constant. `None` when a bound value cannot take its role (the
/// probe is empty), so there is nothing to fault.
fn bgp_probe_pattern<'a>(
    ctx: &'a Ctx<'_>,
    t: (SlotTerm, SlotTerm, SlotTerm),
) -> impl Fn(&Row) -> Option<Pattern> + 'a {
    move |base| match (
        probe_subject(ctx, &t.0, base),
        probe_predicate(ctx, &t.1, base),
        probe_object(ctx, &t.2, base),
    ) {
        (Some(s), Some(p), Some(o)) => Some((s, p, o)),
        _ => None,
    }
}

/// `rows` as the seeds of a [`ProbePlan`], with the tiles its FIRST pattern
/// will route to faulted a window ahead ([`Windowed`]). Only the first pattern
/// is prefetchable (later ones bind from earlier probe results), and only rows
/// whose seed binds that pattern's subject or object are included: a
/// predicate-only pattern routes to the predicate's WHOLE span, which for the
/// fat sides this path serves would prefetch the very extent the probe strategy
/// exists to avoid.
pub(crate) fn plan_windowed<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    plan: &ProbePlan,
    rows: Box<dyn Iterator<Item = Row> + 'q>,
) -> Box<dyn Iterator<Item = Row> + 'q> {
    let Some(&t) = plan.pats.first() else {
        return rows;
    };
    if !index.is_remote() {
        return rows;
    }
    let pat = bgp_probe_pattern(ctx, t);
    let mut win = windowed(index, rows, move |base| {
        pat(base).filter(|(s, _, o)| s.is_some() || o.is_some())
    });
    // A window is pulled whole before its first row comes out. Under a demand
    // bound the first one holds no more than the bound: a sparse left side
    // (a selective filter over a scan) otherwise has to yield 256 rows before
    // a `LIMIT 25` sees the first (G33).
    if let Some(demand) = ctx.limit_hint.get() {
        win.size = win.size.min(demand.max(1));
    }
    Box::new(win)
}

/// Batch-fault the tiles that probing `patterns` from `subject_ids` (each
/// bound onto the subject variable `sv`) will route to — one coalesced read
/// per section instead of one round trip per candidate. Purely a cache
/// warmer: correctness is untouched, and a local index is a no-op. Used by
/// the FILTER-CONTAINS pushdown before it probes its candidate subjects.
pub(crate) fn prefetch_subject_probes(
    ctx: &Ctx,
    index: &GraphIndex,
    patterns: &[TriplePattern],
    sv: &str,
    subject_ids: &[u32],
) {
    if !index.is_remote() {
        return;
    }
    let Some(lowered) = lower(patterns, ctx) else {
        return;
    };
    let Some(slot) = ctx.slots.slot(sv) else {
        return;
    };
    let dict = ctx.rete.dictionary();
    let mut pats: Vec<Pattern> = Vec::new();
    for &sid in subject_ids {
        let mut base = ctx.slots.empty_row();
        base[slot] = Some(Val::Id(dict.subject_node(sid) as i64));
        for t in &lowered {
            if !matches!(t.0, SlotTerm::Var(i) if i == slot) {
                continue;
            }
            if let (Some(s), Some(p), Some(o)) = (
                probe_subject(ctx, &t.0, &base),
                probe_predicate(ctx, &t.1, &base),
                probe_object(ctx, &t.2, &base),
            ) {
                pats.push((s, p, o));
            }
        }
    }
    index.prefetch_probe_tiles(&pats);
}

/// A pattern whose own constant terms (a bound subject or object) already route
/// it to a narrow index range — a few coalesced range reads to scan in full.
fn pattern_is_selective(t: &(SlotTerm, SlotTerm, SlotTerm)) -> bool {
    matches!(t.0, SlotTerm::Node(_)) || matches!(t.2, SlotTerm::Node(_))
}

/// The (deduped) slots a lowered pattern binds.
fn pattern_slots(t: &(SlotTerm, SlotTerm, SlotTerm)) -> Vec<usize> {
    let mut slots: Vec<usize> = Vec::new();
    for term in [&t.0, &t.1, &t.2] {
        if let SlotTerm::Var(i) = term {
            if !slots.contains(i) {
                slots.push(*i);
            }
        }
    }
    slots
}

/// How many scanned triples [`canon_ahead`] pulls before canonicalizing their
/// predicates together.
const CANON_AHEAD: usize = 64;

/// `triples`, with the predicates of each run of [`CANON_AHEAD`] of them
/// canonicalized in one batch before the first row of the run is built.
///
/// A pattern with a variable predicate canonicalizes every new predicate it
/// meets ([`pred_val`]): its term is looked up as a node, up to three chunk
/// reads, each a round trip on a lazy open, one new predicate after another.
/// Looking the run's predicates up together costs the round trips of one
/// ([`crate::row::Resolver::prefetch_canon`]). Anything but a variable
/// predicate over a faulting dictionary passes straight through.
fn canon_ahead<'a, 'c>(
    ctx: &'a Ctx<'c>,
    t: &(SlotTerm, SlotTerm, SlotTerm),
    triples: impl Iterator<Item = Triple> + 'a,
) -> Box<dyn Iterator<Item = Triple> + 'a> {
    if !matches!(t.1, SlotTerm::Var(_)) || !ctx.rete.dictionary().faults_chunks() {
        return Box::new(triples);
    }
    // Which of the subject and object the query will decode as it ends: a
    // projected variable of this pattern (see [`Ctx::decode_ahead`]).
    let decoded = |term: &SlotTerm| match term {
        SlotTerm::Var(i) => ctx.decodes_slot(*i),
        _ => false,
    };
    Box::new(CanonAhead {
        ctx,
        triples,
        run: std::collections::VecDeque::new(),
        decode: (decoded(&t.0), decoded(&t.2)),
        ahead: 0,
    })
}

struct CanonAhead<'a, 'c, I> {
    ctx: &'a Ctx<'c>,
    triples: I,
    run: std::collections::VecDeque<Triple>,
    /// Is the subject, the object, decoded by the query's final projection?
    decode: (bool, bool),
    /// Rows whose terms were already prefetched, across every run: the
    /// decode-ahead is capped by the query's demand once, not once per run.
    ahead: usize,
}

impl<I: Iterator<Item = Triple>> Iterator for CanonAhead<'_, '_, I> {
    type Item = Triple;

    fn next(&mut self) -> Option<Triple> {
        if self.run.is_empty() {
            self.run.extend(self.triples.by_ref().take(CANON_AHEAD));
            let preds: Vec<u32> = self.run.iter().map(|t| t.1).collect();
            // The rows the query will decode are these ones (up to its LIMIT):
            // their terms go out with the canonicalization lookups, one batch
            // for both, where the decode after the last row was two more
            // dependent reads (leaves, then chunks) on a leafed dictionary.
            let dict = self.ctx.rete.dictionary();
            let take = self
                .ctx
                .decode_rows()
                .saturating_sub(self.ahead)
                .min(self.run.len());
            self.ahead += take;
            let mut nodes: Vec<u32> = Vec::new();
            for t in self.run.iter().take(take) {
                if self.decode.0 {
                    nodes.push(dict.subject_node(t.0));
                }
                if self.decode.1 {
                    nodes.push(dict.object_node(t.2));
                }
            }
            self.ctx.resolver.prefetch_canon(&preds, &nodes);
        }
        self.run.pop_front()
    }
}

/// The row value of a matched triple's predicate `p_id`, for a pattern whose
/// predicate slot is `slot`. Only a variable predicate binds it, so only then
/// is it canonicalized (a predicate that is also a node becomes the node id):
/// that is a dictionary lookup of the predicate as a node, memoized per query,
/// but up to three dependent chunk reads on a lazy open for each new predicate
/// — reads a constant predicate never needed. Otherwise the value is unused.
fn pred_val(ctx: &Ctx, slot: &SlotTerm, p_id: u32) -> i64 {
    match slot {
        SlotTerm::Var(_) => ctx.resolver.canon_id(pred_tag(p_id)),
        _ => pred_tag(p_id),
    }
}

/// Build a solution row for one scanned triple, enforcing repeated variables
/// *within* the pattern (e.g. `?x p ?x`). `None` = the triple doesn't satisfy
/// a repeated variable.
fn triple_row(
    ctx: &Ctx,
    t: &(SlotTerm, SlotTerm, SlotTerm),
    (s_id, p_id, o_id): (u32, u32, u32),
) -> Option<Row> {
    // A query under a memory ceiling ([`crate::guard`]) stops between rows.
    crate::guard::check();
    let dict = ctx.rete.dictionary();
    let s_val = dict.subject_node(s_id) as i64;
    let p_val = pred_val(ctx, &t.1, p_id);
    let o_val = dict.object_node(o_id) as i64;
    let mut row = ctx.slots.empty_row();
    for (term, val) in [(&t.0, s_val), (&t.1, p_val), (&t.2, o_val)] {
        if let SlotTerm::Var(i) = term {
            match row[*i] {
                Some(Val::Id(existing)) if existing != val => return None,
                Some(_) => {}
                None => row[*i] = Some(Val::Id(val)),
            }
        }
    }
    Some(row)
}

/// Lazily scan one lowered pattern as a stream of solution rows. The scan
/// constrains only the pattern's constant terms (bound variables are joined in
/// afterwards, not pushed into the scan). `None` = a constant is unsatisfiable
/// (unknown to the dictionary or in an impossible role), which empties the BGP.
fn scan_rows<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    t: (SlotTerm, SlotTerm, SlotTerm),
) -> Option<impl Iterator<Item = Row> + 'q> {
    let dict = ctx.rete.dictionary();
    let (sid, pid, oid) = (
        const_subject(&t.0, dict)?,
        const_predicate(&t.1)?,
        const_object(&t.2, dict)?,
    );
    Some(
        canon_ahead(ctx, &t, index.scan_iter((sid, pid, oid)))
            .filter_map(move |triple| triple_row(ctx, &t, triple)),
    )
}

/// A one-pattern BGP as shard `part` of a grouped query run on several
/// threads ([`crate::row::Shard`]): the rows of [`BgpSolutions::new`] whose
/// triple lies in that slice of the scan's tiles. `None` when this is not a
/// single satisfiable pattern (the caller then evaluates the BGP whole, and
/// the shard is not counted as applied).
pub(crate) fn shard_scan_rows<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    patterns: &[TriplePattern],
    part: crate::row::ShardPart,
) -> Option<Box<dyn Iterator<Item = Row> + 'q>> {
    let [_] = patterns else {
        return None;
    };
    let lowered = lower(patterns, ctx)?;
    let t = lowered[0];
    let dict = ctx.rete.dictionary();
    let (sid, pid, oid) = (
        const_subject(&t.0, dict)?,
        const_predicate(&t.1)?,
        const_object(&t.2, dict)?,
    );
    let cut = |start: usize, end: usize| ctx.shard.applied(part, start, end);
    let triples = index.scan_iter_cut((sid, pid, oid), &cut);
    Some(Box::new(
        canon_ahead(ctx, &t, triples).filter_map(move |triple| triple_row(ctx, &t, triple)),
    ))
}

/// `GRAPH ?g { one pattern }` over a quad-index file: every match of the
/// pattern in any graph as a row that also binds slot `gslot` to the graph,
/// streamed from one pass over the tiles (no view per graph). `None` when this
/// does not apply (not a single pattern, the graph variable also inside the
/// pattern, a file without a quad index): the caller falls back to walking the
/// graphs. A constant the dictionary cannot place gives an empty stream.
pub(crate) fn quad_scan_rows<'q>(
    ctx: &'q Ctx<'q>,
    patterns: &[TriplePattern],
    gslot: usize,
    part: Option<crate::row::ShardPart>,
) -> Option<Box<dyn Iterator<Item = Row> + 'q>> {
    let [p] = patterns else {
        return None;
    };
    if [&p.s, &p.p, &p.o]
        .iter()
        .any(|t| matches!(t, PatternTerm::Var(v) if ctx.slots.slot(v) == Some(gslot)))
    {
        return None;
    }
    let dict = ctx.rete.dictionary();
    let Some(lowered) = lower(patterns, ctx) else {
        return Some(Box::new(std::iter::empty()));
    };
    let t = lowered[0];
    let (Some(sid), Some(pid), Some(oid)) = (
        const_subject(&t.0, dict),
        const_predicate(&t.1),
        const_object(&t.2, dict),
    ) else {
        return Some(Box::new(std::iter::empty()));
    };
    let mut scan = ctx.rete.named_quad_scan((sid, pid, oid))?;
    // A shard of a grouped query run on several threads: its slice of the
    // tiles ([`crate::row::Shard`]).
    if let Some(part) = part {
        let (start, end) = scan.tile_range();
        let (lo, hi) = ctx.shard.applied(part, start, end);
        scan.restrict(lo, hi);
    }
    Some(Box::new(scan.filter_map(move |(g, triple)| {
        let mut row = triple_row(ctx, &t, triple)?;
        row[gslot] = Some(Val::Id(dict.subject_node(g) as i64));
        Some(row)
    })))
}

mod class_probe;
pub(crate) use class_probe::class_probe_rows;
pub use class_probe::set_class_route;

/// A triple pattern as the dictionary ids of its constants (subject, predicate, object).
pub(crate) type ScanPattern = (Option<u32>, Option<u32>, Option<u32>);

/// The pattern of a BGP that best narrows a scan over every graph, as the
/// dictionary ids of its constants: the one with the most constants, a bound
/// subject or object weighing more than a bound predicate. `None` when no
/// pattern has a constant, or one cannot be placed in the dictionary.
pub(crate) fn driver_pattern(ctx: &Ctx, patterns: &[TriplePattern]) -> Option<ScanPattern> {
    let dict = ctx.rete.dictionary();
    let lowered = lower(patterns, ctx)?;
    let mut best: Option<(u32, ScanPattern)> = None;
    for t in &lowered {
        let (sid, pid, oid) = (
            const_subject(&t.0, dict)?,
            const_predicate(&t.1)?,
            const_object(&t.2, dict)?,
        );
        let score =
            3 * u32::from(sid.is_some()) + u32::from(pid.is_some()) + 3 * u32::from(oid.is_some());
        if score > 0 && best.is_none_or(|(b, _)| score > b) {
            best = Some((score, (sid, pid, oid)));
        }
    }
    best.map(|(_, pat)| pat)
}

/// Scan one lowered pattern into a materialized relation, returning the rows
/// and the slots the pattern binds. `None` = a constant is unsatisfiable.
fn pattern_rows(
    ctx: &Ctx,
    index: &GraphIndex,
    t: &(SlotTerm, SlotTerm, SlotTerm),
) -> Option<(Vec<Row>, Vec<usize>)> {
    let slots = pattern_slots(t);
    let mut rel: Vec<Row> = Vec::new();
    let dict = ctx.rete.dictionary();
    let (sid, pid, oid) = (
        const_subject(&t.0, dict)?,
        const_predicate(&t.1)?,
        const_object(&t.2, dict)?,
    );
    // Stream the matches with the lazy cursor — the hash join is
    // order-independent, so no canonical re-sort is needed here.
    for triple in canon_ahead(ctx, t, index.scan_iter((sid, pid, oid))) {
        if let Some(row) = triple_row(ctx, t, triple) {
            rel.push(row);
        }
    }
    Some((rel, slots))
}

/// A per-pattern cardinality estimate, smaller = cheaper to scan. The base is
/// the **exact** per-predicate triple count (sum of the summary quotient graph's
/// super-edge counts) when the predicate is bound, else the whole-graph total.
/// A bound subject/object (a constant, or a variable already bound by `seed`)
/// then scales it down by that predicate's **measured selectivity** from the
/// `query_stats` block — `1 / distinct_subjects` for a bound subject (so
/// `<s> <p> ?o` ≈ the average objects per subject; exactly 1 for a functional
/// predicate), `1 / distinct_objects` for a bound object. Files built before the
/// `query_stats` block fall back to fixed default selectivities. `None` when the
/// file carries no pyramid summary at all — ordering then uses the constant-count
/// heuristic.
fn pattern_estimates(
    ctx: &Ctx,
    index: &GraphIndex,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
    seed: &std::collections::HashSet<usize>,
) -> Option<Vec<f64>> {
    // Only use the summary when it's already in memory — never fault it just to
    // plan (the lazy remote path defers the pyramid by design).
    let pyr = ctx.rete.pyramid_if_loaded()?;
    let mut pred: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for e in &pyr.summary {
        *pred.entry(e.predicate).or_insert(0) += e.count as u64;
    }
    // Measured per-predicate distinct subjects/objects: the SCHEMA section
    // (kind 9) — where a file built since it existed keeps them, its
    // pyramid-meta holding only the community summary — else the pyramid-meta's
    // query-stats block; empty on files built before either → the default
    // selectivities. The pyramid is resident here, so the (small) section is
    // too on a resident open, and costs one range once on a lazy one.
    let stats: std::collections::HashMap<u32, &crate::meta::PredStat> = ctx
        .rete
        .planner_stats(true)
        .iter()
        .map(|s| (s.predicate, s))
        .collect();
    // The active graph's own size: a pattern inside `GRAPH <g>` cannot match more
    // triples than `g` holds, whatever the file-wide predicate count says.
    let graph_total = ctx.rete.index_triples(index).max(1) as f64;
    let total = graph_total;
    let num_preds = pred.len().max(1) as f64;
    // Defaults used only when query_stats has no entry for the predicate.
    const SEL_SUBJECT: f64 = 0.001;
    const SEL_OBJECT: f64 = 0.02;
    let node_bound = |t: &SlotTerm| match t {
        SlotTerm::Node(_) => true,
        SlotTerm::Var(v) => seed.contains(v),
        SlotTerm::Pred(_) => false,
    };
    Some(
        lowered
            .iter()
            .map(|t| {
                // Base from the predicate: exact total for a constant predicate,
                // the average predicate total for a seed-bound predicate variable
                // (the value is known to the probe but not at plan time), else the
                // whole graph. `st` is the measured stats for a constant predicate.
                let (base, st) = match t.1 {
                    SlotTerm::Pred(p) => {
                        (*pred.get(&p).unwrap_or(&0) as f64, stats.get(&p).copied())
                    }
                    SlotTerm::Var(v) if seed.contains(&v) => (total / num_preds, None),
                    _ => (total, None),
                };
                let mut est = base.min(graph_total).max(1.0);
                if node_bound(&t.0) {
                    est *= match st {
                        Some(s) if s.distinct_subjects > 0 => 1.0 / s.distinct_subjects as f64,
                        _ => SEL_SUBJECT,
                    };
                }
                if node_bound(&t.2) {
                    est *= match st {
                        Some(s) if s.distinct_objects > 0 => 1.0 / s.distinct_objects as f64,
                        _ => SEL_OBJECT,
                    };
                }
                est.max(1.0)
            })
            .collect(),
    )
}

/// The minimum, over a BGP's patterns, of [`pattern_scan_bytes`] — the
/// cheapest single-pattern scan the hash-join path would at least perform to
/// materialize this BGP. `None` = a constant term is unknown to the
/// dictionary (the BGP is unsatisfiable; the caller short-circuits anyway).
/// Unlike [`pattern_estimates`], this needs no pyramid summary, so it works on
/// remote-lazy files — exactly where the answer matters most.
pub(crate) fn bgp_min_scan_bytes(
    ctx: &Ctx,
    index: &GraphIndex,
    patterns: &[TriplePattern],
) -> Option<u64> {
    let lowered = lower(patterns, ctx)?;
    lowered
        .iter()
        .map(|t| pattern_scan_bytes(ctx, index, t))
        .min()
}

/// One pattern's cheapest scan extent in BYTES — what `rete cost` previews
/// (the planner's gates use row estimates instead): for each permutation, sum the
/// encoded lengths of the tiles a scan of this pattern would route to (all
/// tiles covering the bound leading key, or the whole section when the leading
/// key is unbound), and take the minimum. Tile *counts* are useless here — a
/// broad predicate can sit inside one giant tile — but the directory's byte
/// lengths expose it. Reads only resident metadata, never tile data.
///
/// A bound term routes by its id **in the role its position has**: a tile's
/// leading range is in subject-, predicate- or object-id space, not in the
/// unified node space a lowered constant carries. Routing the node id instead
/// was off by one for subjects and off by the whole subject-only block for an
/// object-only term (every literal), so `?id :value "10.1234/x"` could cost
/// "0 bytes" and pass the merge-join gate against a fat partner, which the
/// merge then materialized in full. A constant that has no id in the role
/// (never a subject, say) matches nothing: 0 bytes is then exact.
fn pattern_scan_bytes(ctx: &Ctx, index: &GraphIndex, t: &(SlotTerm, SlotTerm, SlotTerm)) -> u64 {
    let dict = ctx.rete.dictionary();
    match (
        const_subject(&t.0, dict),
        const_predicate(&t.1),
        const_object(&t.2, dict),
    ) {
        (Some(s), Some(p), Some(o)) => routed_scan_bytes(index, (s, p, o)),
        _ => 0,
    }
}

/// [`pattern_scan_bytes`] for a pattern already in role-id space.
pub(crate) fn routed_scan_bytes(index: &GraphIndex, pattern: Pattern) -> u64 {
    let sections = index.tile_sections();
    let mut best = u64::MAX;
    // Only the permutations the FILE carries: an absent section is an empty
    // `Vec<Tile>`, and summing its (zero) tile lengths would report a free scan
    // for the one permutation that cannot be scanned at all.
    for perm in index.perms().iter() {
        let tiles = sections[perm.section_index()];
        // A leafed (0x06) directory answers from its root without faulting a
        // leaf: a cost estimate must not cost a round trip.
        best = best.min(match perm.order_pattern(pattern)[0] {
            // A bound subject of a clustered file is one tile of an unknown run.
            Some(_) if index.cluster_runs(perm.section_index()).is_some() => {
                index.clustered_subject_bytes(perm.section_index())
            }
            Some(id) => tiles.routed_bytes(id),
            None => tiles.total_bytes(),
        });
    }
    best
}

/// Estimated matches above which a join side counts as "fat": materializing
/// (or, on a remote index, fetching) a million rows to build a hash/merge side
/// is past the point where probing it per surviving row wins. Shared by the
/// merge-seed asymmetry gate here and the force-probe gates in `sparql::eval`.
///
/// A ROW estimate ([`GraphIndex::estimate_matches`]: routed tiles × rows per
/// tile), not a byte figure. Tiles are cut by their encoded size before
/// compression, so the estimate does not move when only the compression does;
/// the compressed tile bytes this gate used to read did — at zstd level 19,
/// two OpenCitations queries crossed the old 2 MiB line and ran 3–7× slower
/// with up to 6× the memory — and they differed between a resident open
/// (decoded sizes) and a lazy one (compressed sizes) of the same file.
pub(crate) const FAT_SCAN_ROWS: f64 = (1u64 << 20) as f64;

/// Would a merge join of `ta` and `tb` materialize a fat side next to a
/// pinpoint one? (One side above [`FAT_SCAN_ROWS`] and at least four times the
/// other.) Then the hybrid loop — scan the small side, probe the fat one — is
/// strictly better, and the merge seed is declined.
fn merge_seed_lopsided(
    ctx: &Ctx,
    index: &GraphIndex,
    ta: &(SlotTerm, SlotTerm, SlotTerm),
    tb: &(SlotTerm, SlotTerm, SlotTerm),
) -> bool {
    let est = tile_estimates(ctx, index, &[*ta, *tb], &std::collections::HashSet::new());
    let (lo, hi) = (est[0].min(est[1]), est[0].max(est[1]));
    hi >= FAT_SCAN_ROWS && hi / 4.0 >= lo
}

/// Order pattern indices for a left-deep join: cheapest (smallest estimated
/// cardinality) first, always preferring a pattern that shares a variable with
/// the already-joined set so the join stays connected and intermediate relations
/// stay small. Cardinality comes from [`pattern_estimates`]; with no summary it
/// falls back to a most-constants-first heuristic. Pure reordering — `hash_join`
/// is order-independent, so the result multiset is unchanged.
fn selectivity_order(
    ctx: &Ctx,
    index: &GraphIndex,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
) -> Vec<usize> {
    selectivity_order_seeded(ctx, index, lowered, &std::collections::HashSet::new())
}

/// [`selectivity_order`] with a set of slots already bound by an outer seed
/// row: those variables count as already connected (and, in the no-summary
/// fallback, as constants for selectivity).
fn selectivity_order_seeded(
    ctx: &Ctx,
    index: &GraphIndex,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
    seed: &std::collections::HashSet<usize>,
) -> Vec<usize> {
    // Exact per-predicate statistics when the pyramid is resident; otherwise
    // (the lazy path, and files built without a pyramid) the tile-directory
    // estimate, which needs no fetch. That one also counts a class's own
    // instances (`?s a :C` routes to the tiles of `:C`), so the
    // class-enumeration rule below, a stand-in for exactly that missing
    // number, applies only to the pyramid figures, whose `rdf:type`
    // selectivity is an average over classes.
    let (estimates, per_class) = match pattern_estimates(ctx, index, lowered, seed) {
        Some(e) => (Some(e), false),
        None => (Some(tile_estimates(ctx, index, lowered, seed)), true),
    };
    let consts = |t: &(SlotTerm, SlotTerm, SlotTerm)| {
        [&t.0, &t.1, &t.2]
            .into_iter()
            .filter(|x| match x {
                SlotTerm::Var(v) => seed.contains(v),
                _ => true,
            })
            .count()
    };
    let vars = |t: &(SlotTerm, SlotTerm, SlotTerm)| -> Vec<usize> {
        [&t.0, &t.1, &t.2]
            .into_iter()
            .filter_map(|x| match x {
                SlotTerm::Var(v) => Some(*v),
                _ => None,
            })
            .collect()
    };
    // Higher score = picked sooner. With summary stats that's the negated
    // estimate (smaller cardinality wins); without, the constant count (old
    // behaviour, byte-identical ordering for pyramid-less files).
    let score = |i: usize| -> f64 {
        match &estimates {
            Some(e) => -e[i],
            None => consts(&lowered[i]) as f64,
        }
    };
    // A `?s rdf:type <Class>` with an as-yet-unbound subject is a **class
    // enumeration**: it matches every instance of the class, so it is almost never
    // the most selective place to *start* a join (a popular class has thousands of
    // instances), even though it has two bound positions and so looks selective to
    // the constant-count heuristic. Deprioritize it as a seed; once its subject is
    // bound by another pattern it becomes a cheap per-row type *check* (the hybrid
    // join probes it), so pushing it later costs nothing. Robust and stats-free.
    let type_pid = ctx.rete.dictionary().predicate_id(crate::file::RDF_TYPE);
    let is_class_enum = |i: usize, bound: &std::collections::HashSet<usize>| -> bool {
        let t = &lowered[i];
        !per_class
            && type_pid.is_some_and(|tp| matches!(t.1, SlotTerm::Pred(p) if p == tp))
            && matches!(t.2, SlotTerm::Node(_))
            && matches!(t.0, SlotTerm::Var(v) if !bound.contains(&v))
    };
    let n = lowered.len();
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut bound: std::collections::HashSet<usize> = seed.clone();
    while !remaining.is_empty() {
        // Pick the remaining pattern with the best (connected, not-a-class-enum,
        // score) key; ties go to the lowest original index for a stable order.
        let best = *remaining
            .iter()
            .max_by(|&&a, &&b| {
                let connected = |i: usize| vars(&lowered[i]).iter().any(|v| bound.contains(v));
                connected(a)
                    .cmp(&connected(b))
                    .then_with(|| is_class_enum(b, &bound).cmp(&is_class_enum(a, &bound)))
                    .then_with(|| {
                        score(a)
                            .partial_cmp(&score(b))
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .then_with(|| b.cmp(&a))
            })
            .unwrap();
        for v in vars(&lowered[best]) {
            bound.insert(v);
        }
        order.push(best);
        remaining.retain(|&i| i != best);
    }
    order
}

/// Does the BGP have at least one solution against `index`? A single pattern
/// with all-distinct variables streams the index and stops at the first match
/// (no materialization); anything else falls back to the full evaluator and
/// tests non-emptiness (still benefiting from the lazy per-pattern scan). Used
/// by `ASK`.
pub(crate) fn bgp_exists(ctx: &Ctx, index: &GraphIndex, patterns: &[TriplePattern]) -> bool {
    let dict = ctx.rete.dictionary();
    let Some(lowered) = lower(patterns, ctx) else {
        return false;
    };
    if let [t] = lowered.as_slice() {
        // The fast path can't enforce a variable repeated across positions
        // (e.g. `?x p ?x`) — that needs the row builder — so only take it when
        // the pattern's variables are all distinct.
        let names: Vec<usize> = [&t.0, &t.1, &t.2]
            .into_iter()
            .filter_map(|x| match x {
                SlotTerm::Var(v) => Some(*v),
                _ => None,
            })
            .collect();
        let distinct = names
            .iter()
            .enumerate()
            .all(|(i, v)| !names[i + 1..].contains(v));
        if distinct {
            return match (
                const_subject(&t.0, dict),
                const_predicate(&t.1),
                const_object(&t.2, dict),
            ) {
                (Some(s), Some(p), Some(o)) => index.scan_iter((s, p, o)).next().is_some(),
                _ => false,
            };
        }
    }
    !eval_bgp_rows(ctx, index, patterns).is_empty()
}

/// A lazy left-deep BGP join that yields solution rows one at a time, so a
/// consumer under `LIMIT`/`OFFSET` (or `.next()` for ASK) can stop early —
/// stopping also stops the underlying index scan.
///
/// The all-but-last patterns are joined eagerly into a `prefix` **hash table**
/// (they are the most selective, so this is the small side); the *last* (least
/// selective) pattern is then **streamed** from the index cursor, probing the
/// prefix — the big scan is never materialized, and a `LIMIT` above stops it
/// after a handful of triples. Yields the same solution multiset as
/// [`eval_bgp_rows`] (the join is order-independent), just incrementally.
pub(crate) struct BgpSolutions<'q> {
    scan: Option<Box<dyn Iterator<Item = Row> + 'q>>,
    prefix: Vec<Row>,
    /// Prefix row indices keyed by the shared-slot values.
    buckets: HashMap<Vec<Val>, Vec<usize>>,
    shared: Vec<usize>,
    cartesian: bool,
    /// The prefix is the all-unbound seed row (single-pattern BGP): scanned
    /// rows are already complete solutions — pass them through unmerged.
    seed_only: bool,
    cur_scan: Option<Row>,
    /// Reused candidate buffer (avoids a Vec allocation per scanned row).
    matches: Vec<usize>,
    mi: usize,
}

impl<'q> BgpSolutions<'q> {
    /// Yield `rows` as complete solutions (no prefix merge).
    fn passthrough(rows: Box<dyn Iterator<Item = Row> + 'q>) -> Self {
        BgpSolutions {
            scan: Some(rows),
            prefix: Vec::new(),
            buckets: HashMap::new(),
            shared: Vec::new(),
            cartesian: false,
            seed_only: true,
            cur_scan: None,
            matches: Vec::new(),
            mi: 0,
        }
    }

    /// The fully streaming join, for a BGP whose most selective pattern is
    /// itself large ([`STREAM_SEED_ROWS`]). Nothing proportional to it is ever
    /// held: the seed pattern streams from its index cursor (or, when the next
    /// pattern is just as large and shares its subject variable, both stream
    /// through a merge join), and each later pattern joins per row — through
    /// a hash table when the pattern is small ([`HASH_BUILD_ROWS`]), by an
    /// index probe otherwise. Same solution multiset as the materializing
    /// path; only the row order differs.
    fn streaming(
        ctx: &'q Ctx<'q>,
        index: &'q GraphIndex,
        lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
        order: &[usize],
        est: &[f64],
    ) -> Self {
        let (mut stream, mut bound, skip): (Box<dyn Iterator<Item = Row> + 'q>, Vec<usize>, usize) =
            match (est[order[1]] > limits().stream_seed)
                .then(|| merge_stream(ctx, index, lowered[order[0]], lowered[order[1]]))
                .flatten()
            {
                Some((m, slots)) => (m, slots, 2),
                None => {
                    let t = lowered[order[0]];
                    let Some(scan) = scan_rows(ctx, index, t) else {
                        return Self::empty();
                    };
                    (Box::new(scan), pattern_slots(&t), 1)
                }
            };
        for &i in &order[skip..] {
            let t = lowered[i];
            let slots = pattern_slots(&t);
            let shared: Vec<usize> = slots
                .iter()
                .copied()
                .filter(|s| bound.contains(s))
                .collect();
            if !shared.is_empty() && est[i] > limits().hash_build {
                // Each window of streamed rows has its probes' tiles faulted
                // together ([`Windowed`]); one at a time, a probe join over a
                // remote file was bound by round-trip latency.
                stream = Box::new(
                    windowed(index, stream, bgp_probe_pattern(ctx, t))
                        .flat_map(move |base| probe_rows(ctx, index, t, base)),
                );
            } else {
                // Small (or unconnected) pattern: scan it once into a table
                // keyed on the shared slots and join every streamed row to it.
                let Some((rel, _)) = pattern_rows(ctx, index, &t) else {
                    return Self::empty();
                };
                let mut table: HashMap<Vec<Val>, Vec<Row>> = HashMap::new();
                for r in rel {
                    let key: Vec<Val> = shared.iter().map(|&s| r[s].clone().unwrap()).collect();
                    table.entry(key).or_default().push(r);
                }
                let fill = slots.clone();
                stream = Box::new(stream.flat_map(move |base| {
                    let key: Vec<Val> = shared.iter().map(|&s| base[s].clone().unwrap()).collect();
                    let out: Vec<Row> = table
                        .get(&key)
                        .map(|rs| {
                            rs.iter()
                                .map(|r| {
                                    let mut row = base.clone();
                                    for &s in &fill {
                                        row[s] = r[s].clone();
                                    }
                                    row
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    out.into_iter()
                }));
            }
            for s in slots {
                if !bound.contains(&s) {
                    bound.push(s);
                }
            }
        }
        Self::passthrough(stream)
    }

    /// An iterator that yields nothing (an unsatisfiable BGP).
    fn empty() -> Self {
        BgpSolutions {
            scan: None,
            prefix: Vec::new(),
            buckets: HashMap::new(),
            shared: Vec::new(),
            cartesian: false,
            seed_only: false,
            cur_scan: None,
            matches: Vec::new(),
            mi: 0,
        }
    }

    pub(crate) fn new(ctx: &'q Ctx<'q>, index: &'q GraphIndex, patterns: &[TriplePattern]) -> Self {
        // An empty BGP has exactly one (empty) solution.
        if patterns.is_empty() {
            return BgpSolutions {
                scan: Some(Box::new(std::iter::once(ctx.slots.empty_row()))),
                prefix: vec![ctx.slots.empty_row()],
                buckets: HashMap::new(),
                shared: Vec::new(),
                cartesian: true,
                seed_only: true,
                cur_scan: None,
                matches: Vec::new(),
                mi: 0,
            };
        }
        // Lower all patterns; an unknown constant term ⇒ no solutions.
        let Some(lowered) = lower(patterns, ctx) else {
            return Self::empty();
        };
        // Join all but the *last* (least selective) pattern eagerly into the
        // prefix; a single pattern leaves the seed row as the prefix.
        let (order, scan_est) = plan_order(ctx, index, &lowered);
        // Even the most selective pattern is big: materializing it as the
        // prefix would hold every one of its rows. Stream the whole join.
        if order.len() >= 2 && scan_est[order[0]] > limits().stream_seed {
            return Self::streaming(ctx, index, &lowered, &order, &scan_est);
        }
        let (&last_i, prefix_is) = order.split_last().unwrap();
        let prefix_pats: Vec<TriplePattern> =
            prefix_is.iter().map(|&i| patterns[i].clone()).collect();
        let prefix = eval_bgp_rows(ctx, index, &prefix_pats);
        if prefix.is_empty() {
            return Self::empty();
        }

        // Hash-join key = slots shared by the prefix and the last pattern (BGP
        // rows bind every slot of their pattern set, so the prefix's bound set
        // can be read off its first row).
        let shared: Vec<usize> = pattern_slots(&lowered[last_i])
            .into_iter()
            .filter(|&s| prefix[0][s].is_some())
            .collect();

        // A few prefix rows against a big last pattern: look each one up
        // instead of scanning the pattern. `?r :relationSubject <doi> . ?r ?p
        // ?o` has four prefix rows and a last pattern that is the whole graph.
        // The last pattern is an unbound class (`?x rdf:type ?c`) whose subject
        // the prefix binds: each row's types are one SPO lookup, and the prefix
        // is already resident, so probing costs no memory the scan would not.
        // It wins until the rows outnumber the statements the scan would read
        // (the usual sixteenth gives way to one).
        let class_probe = !shared.is_empty()
            && scan_est[last_i] >= limits().probe_floor
            && (prefix.len() as f64) <= scan_est[last_i]
            && is_unbound_class(ctx, &lowered[last_i], &shared);
        if !shared.is_empty() && (probe_beats_scan(prefix.len(), scan_est[last_i]) || class_probe) {
            let t = lowered[last_i];
            let stream = windowed(index, prefix.into_iter(), bgp_probe_pattern(ctx, t))
                .flat_map(move |base| probe_rows(ctx, index, t, base));
            return Self::passthrough(Box::new(stream));
        }

        // Stream the last pattern with the lazy index cursor.
        let Some(scan) = scan_rows(ctx, index, lowered[last_i]) else {
            return Self::empty();
        };
        let cartesian = shared.is_empty();
        // A single-pattern BGP joins against the all-unbound seed: scanned rows
        // are already complete solutions.
        let seed_only = prefix.len() == 1 && prefix[0].iter().all(Option::is_none);
        let mut buckets: HashMap<Vec<Val>, Vec<usize>> = HashMap::new();
        if !cartesian {
            for (i, r) in prefix.iter().enumerate() {
                let key: Vec<Val> = shared.iter().map(|&s| r[s].clone().unwrap()).collect();
                buckets.entry(key).or_default().push(i);
            }
        }
        BgpSolutions {
            scan: Some(Box::new(scan)),
            prefix,
            buckets,
            shared,
            cartesian,
            seed_only,
            cur_scan: None,
            matches: Vec::new(),
            mi: 0,
        }
    }
}

impl Iterator for BgpSolutions<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        // Single-pattern fast path: pass scanned rows through unmerged.
        if self.seed_only {
            return self.scan.as_mut()?.next();
        }
        loop {
            // Emit the next prefix match for the current scanned row.
            if self.mi < self.matches.len() {
                let pi = self.matches[self.mi];
                self.mi += 1;
                let mut merged = self.prefix[pi].clone();
                for (slot, v) in self.cur_scan.as_ref().unwrap().iter().enumerate() {
                    if v.is_some() {
                        merged[slot] = v.clone();
                    }
                }
                return Some(merged);
            }
            // Pull the next row from the index scan and gather its matches
            // (into the reused buffer — no allocation per scanned row).
            let s = self.scan.as_mut()?.next()?;
            self.matches.clear();
            if self.cartesian {
                self.matches.extend(0..self.prefix.len());
            } else {
                let key: Vec<Val> = self.shared.iter().map(|&i| s[i].clone().unwrap()).collect();
                if let Some(c) = self.buckets.get(&key) {
                    self.matches.extend_from_slice(c);
                }
            }
            self.mi = 0;
            self.cur_scan = Some(s);
        }
    }
}

/// A **streaming** merge join of two patterns that share exactly one variable,
/// in the subject position of both: each side streams from a permutation
/// sorted on subject ([`GraphIndex::scan_iter_sorted_on`]) and only the
/// current subject's rows are held. Restricted to the subject column because a
/// subject's rows are its out-edges, a small run; an object run (every
/// instance of a class) can be the whole pattern. `None` when the shape does
/// not qualify; the caller then streams the first pattern alone.
#[allow(clippy::type_complexity)]
fn merge_stream<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    ta: (SlotTerm, SlotTerm, SlotTerm),
    tb: (SlotTerm, SlotTerm, SlotTerm),
) -> Option<(Box<dyn Iterator<Item = Row> + 'q>, Vec<usize>)> {
    let sa = pattern_slots(&ta);
    let sb = pattern_slots(&tb);
    let shared: Vec<usize> = sa.iter().copied().filter(|s| sb.contains(s)).collect();
    let [v] = shared[..] else {
        return None;
    };
    if !matches!(ta.0, SlotTerm::Var(i) if i == v) || !matches!(tb.0, SlotTerm::Var(i) if i == v) {
        return None;
    }
    let dict = ctx.rete.dictionary();
    let lower = |t: &(SlotTerm, SlotTerm, SlotTerm)| -> Option<Pattern> {
        Some((
            const_subject(&t.0, dict)?,
            const_predicate(&t.1)?,
            const_object(&t.2, dict)?,
        ))
    };
    let side = |t: (SlotTerm, SlotTerm, SlotTerm)| -> Option<KeyedRows<'q>> {
        let it = index.scan_iter_sorted_on(lower(&t)?, 0)?;
        let keyed: Box<dyn Iterator<Item = (u32, Row)> + 'q> =
            Box::new(it.filter_map(move |tri| triple_row(ctx, &t, tri).map(|r| (tri.0, r))));
        Some(keyed.peekable())
    };
    let a = side(ta)?;
    let b = side(tb)?;
    let mut slots = sa;
    for s in sb {
        if !slots.contains(&s) {
            slots.push(s);
        }
    }
    Some((
        Box::new(MergeStream {
            a,
            b,
            ra: Vec::new(),
            rb: Vec::new(),
            i: 0,
            j: 0,
        }),
        slots,
    ))
}

type KeyedRows<'q> = std::iter::Peekable<Box<dyn Iterator<Item = (u32, Row)> + 'q>>;

/// The two-pointer merge behind [`merge_stream`]: `ra`/`rb` hold the current
/// equal-key runs and `(i, j)` walks their cross product lazily.
struct MergeStream<'q> {
    a: KeyedRows<'q>,
    b: KeyedRows<'q>,
    ra: Vec<Row>,
    rb: Vec<Row>,
    i: usize,
    j: usize,
}

impl Iterator for MergeStream<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            while self.i < self.ra.len() {
                if self.j < self.rb.len() {
                    let j = self.j;
                    self.j += 1;
                    if let Some(m) = crate::row::merge_rows(&self.ra[self.i], &self.rb[j]) {
                        return Some(m);
                    }
                } else {
                    self.i += 1;
                    self.j = 0;
                }
            }
            let ka = self.a.peek()?.0;
            let kb = self.b.peek()?.0;
            match ka.cmp(&kb) {
                std::cmp::Ordering::Less => {
                    self.a.next();
                }
                std::cmp::Ordering::Greater => {
                    self.b.next();
                }
                std::cmp::Ordering::Equal => {
                    self.ra.clear();
                    self.rb.clear();
                    while let Some((_, r)) = self.a.next_if(|(k, _)| *k == ka) {
                        self.ra.push(r);
                    }
                    while let Some((_, r)) = self.b.next_if(|(k, _)| *k == ka) {
                        self.rb.push(r);
                    }
                    self.i = 0;
                    self.j = 0;
                }
            }
        }
    }
}

// --- index-nested-loop probing ------------------------------------------------
//
// Under a small, known demand (LIMIT/ASK — see `Ctx::limit_hint`), scanning
// every pattern once to hash-join is mostly wasted work: the consumer wants a
// handful of rows. The probe path instead streams the seed/first pattern and,
// per row, *probes* each next pattern through the index with the row's bound
// values substituted as scan constants — so producing k solutions touches
// O(k · patterns) index groups instead of every pattern's full extent. Same
// solution multiset as the hash join (joins are order-independent); only the
// evaluation order differs.

/// Index-scan constant for a subject position given a partially-bound row.
/// Outer `None` = unsatisfiable for this row; inner `None` = wildcard.
fn probe_subject(ctx: &Ctx, t: &SlotTerm, base: &Row) -> Option<Option<u32>> {
    let dict = ctx.rete.dictionary();
    match t {
        SlotTerm::Node(n) => dict.node_as_subject_id(*n).map(Some),
        SlotTerm::Pred(_) => None,
        SlotTerm::Var(i) => match &base[*i] {
            None => Some(None),
            Some(Val::Id(v)) if *v >= 0 => dict.node_as_subject_id(*v as u32).map(Some),
            // A predicate-tagged or computed value can never be a subject.
            Some(_) => None,
        },
    }
}

/// Index-scan constant for an object position (see [`probe_subject`]).
fn probe_object(ctx: &Ctx, t: &SlotTerm, base: &Row) -> Option<Option<u32>> {
    let dict = ctx.rete.dictionary();
    match t {
        SlotTerm::Node(n) => dict.node_as_object_id(*n).map(Some),
        SlotTerm::Pred(_) => None,
        SlotTerm::Var(i) => match &base[*i] {
            None => Some(None),
            Some(Val::Id(v)) if *v >= 0 => dict.node_as_object_id(*v as u32).map(Some),
            Some(_) => None,
        },
    }
}

/// Index-scan constant for a predicate position (see [`probe_subject`]).
fn probe_predicate(ctx: &Ctx, t: &SlotTerm, base: &Row) -> Option<Option<u32>> {
    let dict = ctx.rete.dictionary();
    match t {
        SlotTerm::Pred(p) => Some(Some(*p)),
        SlotTerm::Node(_) => None,
        SlotTerm::Var(i) => match &base[*i] {
            None => Some(None),
            Some(Val::Id(v)) if *v < 0 => Some(Some((-v - 1) as u32)),
            // Canonicalized to a node id — the term may still be a predicate.
            Some(Val::Id(v)) => ctx
                .resolver
                .term(*v)
                .and_then(|t| dict.predicate_id(&t))
                .map(Some),
            Some(Val::Str(_)) => None,
        },
    }
}

/// Probe one pattern with a partially-bound row: every bound variable becomes
/// an index-scan constant, and each matching triple extends a clone of the row
/// (repeated unbound variables stay consistent).
fn probe_rows<'q>(
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    t: (SlotTerm, SlotTerm, SlotTerm),
    base: Row,
) -> Box<dyn Iterator<Item = Row> + 'q> {
    let (Some(sid), Some(pid), Some(oid)) = (
        probe_subject(ctx, &t.0, &base),
        probe_predicate(ctx, &t.1, &base),
        probe_object(ctx, &t.2, &base),
    ) else {
        return Box::new(std::iter::empty());
    };
    let dict = ctx.rete.dictionary();
    // A variable predicate is canonicalized a run at a time ([`canon_ahead`]):
    // one new predicate after another was a dictionary round trip each.
    Box::new(
        canon_ahead(ctx, &t, index.scan_iter((sid, pid, oid))).filter_map(
            move |(s_id, p_id, o_id)| {
                let s_val = dict.subject_node(s_id) as i64;
                let p_val = pred_val(ctx, &t.1, p_id);
                let o_val = dict.object_node(o_id) as i64;
                let mut row = base.clone();
                for (term, val) in [(&t.0, s_val), (&t.1, p_val), (&t.2, o_val)] {
                    if let SlotTerm::Var(i) = term {
                        match row[*i] {
                            Some(Val::Id(existing)) if existing != val => return None,
                            Some(Val::Id(_)) => {}
                            Some(Val::Str(_)) => return None,
                            None => row[*i] = Some(Val::Id(val)),
                        }
                    }
                }
                Some(row)
            },
        ),
    )
}

/// A lowered, probe-ordered BGP, reusable across many seed rows.
pub(crate) struct ProbePlan {
    pats: Vec<(SlotTerm, SlotTerm, SlotTerm)>,
}

impl ProbePlan {
    /// Lower and order `patterns` for probing from rows that bind (at least)
    /// the slots in `seed_mask`. `None` = a constant term is unknown, making
    /// the BGP unsatisfiable for every seed.
    pub(crate) fn new(
        ctx: &Ctx,
        index: &GraphIndex,
        patterns: &[TriplePattern],
        seed_mask: &[bool],
    ) -> Option<Self> {
        let lowered = lower(patterns, ctx)?;
        let seed: std::collections::HashSet<usize> = seed_mask
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.then_some(i))
            .collect();
        let order = selectivity_order_seeded(ctx, index, &lowered, &seed);
        Some(ProbePlan {
            pats: order.into_iter().map(|i| lowered[i]).collect(),
        })
    }
}

/// Depth-first index-nested-loop join over a [`ProbePlan`], starting from a
/// seed row. Fully lazy: pulling k rows probes O(k · patterns) index groups.
pub(crate) struct ProbeJoin<'q> {
    ctx: &'q Ctx<'q>,
    index: &'q GraphIndex,
    pats: Vec<(SlotTerm, SlotTerm, SlotTerm)>,
    stack: Vec<Box<dyn Iterator<Item = Row> + 'q>>,
}

impl<'q> ProbeJoin<'q> {
    /// Probe a whole BGP from scratch (the seed is the all-unbound row).
    /// `None` = unsatisfiable.
    pub(crate) fn new(
        ctx: &'q Ctx<'q>,
        index: &'q GraphIndex,
        patterns: &[TriplePattern],
    ) -> Option<Self> {
        let plan = ProbePlan::new(ctx, index, patterns, &vec![false; ctx.slots.len()])?;
        Some(Self::from_plan(ctx, index, &plan, ctx.slots.empty_row()))
    }

    /// Probe a pre-lowered plan from one seed row.
    pub(crate) fn from_plan(
        ctx: &'q Ctx<'q>,
        index: &'q GraphIndex,
        plan: &ProbePlan,
        seed: Row,
    ) -> Self {
        let pats = plan.pats.clone();
        let first = probe_rows(ctx, index, pats[0], seed);
        ProbeJoin {
            ctx,
            index,
            pats,
            stack: vec![first],
        }
    }
}

impl Iterator for ProbeJoin<'_> {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            let depth = self.stack.len();
            match self.stack.last_mut()?.next() {
                Some(row) => {
                    if depth == self.pats.len() {
                        return Some(row);
                    }
                    let it = probe_rows(self.ctx, self.index, self.pats[depth], row);
                    self.stack.push(it);
                }
                None => {
                    self.stack.pop();
                    if self.stack.is_empty() {
                        return None;
                    }
                }
            }
        }
    }
}

/// Hash-join two BGP relations on the slots they share. Every left row binds
/// exactly the slots accumulated so far and every right row binds the current
/// pattern's slots, so the shared set is uniform across rows.
fn hash_join(
    left: Vec<Row>,
    left_bound: &[usize],
    right: Vec<Row>,
    right_slots: &[usize],
) -> Vec<Row> {
    // Fast path: the all-unbound seed row joins to `right` unchanged.
    if left.len() == 1 && left_bound.is_empty() {
        return right;
    }
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    let shared: Vec<usize> = right_slots
        .iter()
        .copied()
        .filter(|s| left_bound.contains(s))
        .collect();
    let fill = |l: &Row, r: &Row| -> Row {
        let mut out = l.clone();
        for &s in right_slots {
            out[s] = r[s].clone();
        }
        out
    };
    if shared.is_empty() {
        // No shared variable: Cartesian product. In a connected BGP this only
        // arises for a fully-ground pattern (deduped index ⇒ one matching row),
        // so it acts as an existence filter rather than multiplying rows.
        let mut out = Vec::with_capacity(left.len() * right.len());
        for l in &left {
            for r in &right {
                out.push(fill(l, r));
            }
        }
        return out;
    }
    let key_of = |b: &Row| -> Vec<Val> { shared.iter().map(|&s| b[s].clone().unwrap()).collect() };
    // A hash join is symmetric on the shared key, so build the **smaller** side's
    // hash table and probe with the larger — same result set, fewer inserts and a
    // smaller table. (The old code always built `right`; after cardinality-based
    // ordering the accumulating `left` is usually the small selective side, so
    // building it instead is the common win.) Whichever side is built, the output
    // row carries every bound slot of both.
    let mut out = Vec::new();
    if right.len() <= left.len() {
        let mut buckets: HashMap<Vec<Val>, Vec<Row>> = HashMap::new();
        for r in right {
            buckets.entry(key_of(&r)).or_default().push(r);
        }
        for l in &left {
            if let Some(rs) = buckets.get(&key_of(l)) {
                for r in rs {
                    out.push(fill(l, r));
                }
            }
        }
    } else {
        // Build the left side; fill the left's own slots into each probed right.
        let mut buckets: HashMap<Vec<Val>, Vec<Row>> = HashMap::new();
        for l in left {
            buckets.entry(key_of(&l)).or_default().push(l);
        }
        for r in &right {
            if let Some(ls) = buckets.get(&key_of(r)) {
                for l in ls {
                    let mut row = r.clone();
                    for &s in left_bound {
                        if let Some(v) = &l[s] {
                            row[s] = Some(v.clone());
                        }
                    }
                    out.push(row);
                }
            }
        }
    }
    out
}

// Constant-only index constraints. A variable scans as a wildcard (`Some(None)`)
// — it is resolved later by the hash join, not pushed into the scan. The outer
// `None` means "unsatisfiable" (a constant in an impossible role, or unknown to
// the dictionary), which empties the whole BGP. Inner `None` = "wildcard".
fn const_subject(t: &SlotTerm, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        SlotTerm::Node(n) => d.node_as_subject_id(*n).map(Some),
        SlotTerm::Pred(_) => None, // a predicate term can't be a subject
        SlotTerm::Var(_) => Some(None),
    }
}

fn const_object(t: &SlotTerm, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        SlotTerm::Node(n) => d.node_as_object_id(*n).map(Some),
        SlotTerm::Pred(_) => None,
        SlotTerm::Var(_) => Some(None),
    }
}

fn const_predicate(t: &SlotTerm) -> Option<Option<u32>> {
    match t {
        SlotTerm::Pred(p) => Some(Some(*p)),
        SlotTerm::Node(_) => None, // a node term can't be a predicate
        SlotTerm::Var(_) => Some(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::write_file_v05 as write_file;
    use crate::index::GraphIndexBuilder;

    fn rete_from(triples: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        write_file(&dict, &ib.build(), false, &[], 0)
    }

    fn pat(s: &str, p: &str, o: &str) -> TriplePattern {
        TriplePattern {
            s: PatternTerm::parse(s),
            p: PatternTerm::parse(p),
            o: PatternTerm::parse(o),
        }
    }

    /// The scan-bytes figure (`rete cost`'s preview, and through 0.3.7 the
    /// merge-join and fat-side gates) routes a bound term by its id IN ITS
    /// ROLE. It used to route the unified node id,
    /// which for an object-only term (any literal) is shifted past the whole
    /// subject-only block: `?s :value "lit"` found no OPS tile at all, cost
    /// "0 bytes", and so let a merge join materialize its partner in full —
    /// on OpenCitations, every DOI identifier of a 2.9 B-triple file, behind
    /// `?id :hasLiteralValue "10.…" ; :usesIdentifierScheme :doi`.
    #[test]
    fn scan_bytes_route_a_literal_by_its_object_id() {
        let value = "<http://ex/value>";
        let owned: Vec<(String, String)> = (0..400)
            .map(|i| (format!("<http://ex/s{i:03}>"), format!("\"lit {i:03}\"")))
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, o) in &owned {
            db.observe(s, value, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, o) in &owned {
            ib.push(dict.encode(s, value, o).unwrap());
        }
        let rete = Rete::open(&write_file(&dict, &ib.build(), false, &[], 0)).unwrap();
        let index = rete.default_index();
        // The whole predicate, the most a bound object could ever cost.
        let whole = routed_scan_bytes(index, (None, rete.dictionary().predicate_id(value), None));
        for (_, lit) in owned.iter().step_by(57) {
            let q = [pat("?s", value, lit)];
            let mut slots = Slots::new();
            collect_pattern_slots(&q, &mut slots);
            let ctx = Ctx::new(&rete, slots);
            let got = bgp_min_scan_bytes(&ctx, index, &q).unwrap();
            assert!(got > 0, "{lit} routed to no tile");
            assert!(
                got < whole,
                "{lit} routed to the whole predicate ({got} of {whole})"
            );
        }
    }

    /// The planner's size gates read ROW estimates, which do not depend on how
    /// the tiles are compressed. The same file opened resident (tile sizes are
    /// the decoded images) and lazily (tile sizes are the compressed frames)
    /// reports different bytes but must take the same decisions — the byte
    /// gates this replaced flipped plans when only the zstd level changed.
    #[test]
    fn plan_gates_do_not_move_with_the_compression() {
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..3000 {
            let s = format!("<http://ex/s{i}>");
            owned.push((
                s.clone(),
                "<http://ex/kind>".into(),
                format!("<http://ex/K{}>", i % 7),
            ));
            owned.push((s.clone(), "<http://ex/value>".into(), format!("\"v {i}\"")));
            if i % 50 == 0 {
                owned.push((s, "<http://ex/rare>".into(), "<http://ex/R>".into()));
            }
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &owned {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(1024);
        for (s, p, o) in &owned {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let resident = Rete::open(&bytes).unwrap();
        let lazy = Rete::open_ranged_lazy(crate::reader::SliceReader::new(leaked)).unwrap();
        let shapes: Vec<Vec<TriplePattern>> = vec![
            vec![
                pat("?s", "<http://ex/rare>", "<http://ex/R>"),
                pat("?s", "<http://ex/value>", "?v"),
            ],
            vec![
                pat("?s", "<http://ex/kind>", "<http://ex/K3>"),
                pat("?s", "<http://ex/value>", "?v"),
            ],
            vec![
                pat("?s", "<http://ex/kind>", "?k"),
                pat("?s", "<http://ex/value>", "?v"),
            ],
            vec![
                pat("?s", "<http://ex/value>", "\"v 7\""),
                pat("?s", "<http://ex/kind>", "?k"),
            ],
        ];
        let decisions = |rete: &Rete| -> Vec<(Vec<u64>, bool, Option<u64>)> {
            shapes
                .iter()
                .map(|q| {
                    let mut slots = Slots::new();
                    collect_pattern_slots(q, &mut slots);
                    let ctx = Ctx::new(rete, slots);
                    let index = rete.default_index();
                    let lowered = lower(q, &ctx).unwrap();
                    let est = tile_estimates(&ctx, index, &lowered, &Default::default());
                    let lopsided = merge_seed_lopsided(&ctx, index, &lowered[0], &lowered[1]);
                    let min = bgp_min_estimate(&ctx, index, q).map(|e| e as u64);
                    (est.iter().map(|e| *e as u64).collect(), lopsided, min)
                })
                .collect()
        };
        assert_eq!(decisions(&resident), decisions(&lazy));
        // ... while the byte figures the gates used to read do differ.
        #[cfg(feature = "compression")]
        {
            let bytes_of = |rete: &Rete| {
                let q = &shapes[2];
                let mut slots = Slots::new();
                collect_pattern_slots(q, &mut slots);
                let ctx = Ctx::new(rete, slots);
                bgp_min_scan_bytes(&ctx, rete.default_index(), q)
            };
            assert_ne!(bytes_of(&resident), bytes_of(&lazy));
        }
    }

    /// A fat predicate must look fat whichever layout stores the directory.
    /// On format 0x06 the tile directory is two-level and a leaf that is not
    /// resident is estimated from the root alone; it used to count as ONE
    /// tile, so a 2-million-row `rdfs:label` read as ~7 thousand rows on 0x06
    /// and 2 million on 0x05 — and the OPTIONAL joined to a 1000-row left side
    /// was hash-joined (the whole predicate materialized: 14.5 GB on a 8 B
    /// triple file) instead of probed per row. The same file in both layouts
    /// must give estimates of the same order, and the probe decision must be
    /// the same one.
    #[test]
    fn a_fat_predicate_estimates_alike_in_both_layouts() {
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..120_000 {
            let s = format!("<http://ex/s{i:06}>");
            owned.push((s.clone(), "<http://ex/kind>".into(), "<http://ex/K>".into()));
            owned.push((s, "<http://ex/value>".into(), format!("\"v {i}\"")));
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &owned {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in &owned {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let v5 = write_file(&dict, &ib.build(), false, &[], 0);
        let v6: &'static [u8] = Box::leak(
            crate::v06::write::convert_image(&v5, Default::default())
                .unwrap()
                .into_boxed_slice(),
        );
        let leaked5: &'static [u8] = Box::leak(v5.clone().into_boxed_slice());
        let flat = Rete::open_ranged_lazy(crate::reader::SliceReader::new(leaked5)).unwrap();
        let leafed = Rete::open_ranged_lazy(crate::reader::SliceReader::new(v6)).unwrap();
        let q = [pat("?s", "<http://ex/value>", "?v")];
        let figures = |rete: &Rete| {
            let mut slots = Slots::new();
            collect_pattern_slots(&q, &mut slots);
            let ctx = Ctx::new(rete, slots);
            let index = rete.default_index();
            let total = bgp_total_estimate(&ctx, index, &q).unwrap();
            (total, probe_row_budget(total))
        };
        let (est5, budget5) = figures(&flat);
        let (est6, budget6) = figures(&leafed);
        assert!(est5 >= 70_000.0, "flat estimate {est5}");
        assert!(
            est6 >= est5 / 2.0 && est6 <= est5 * 2.0,
            "0x06 estimates {est6} where 0x05 estimates {est5}"
        );
        // The decision: a 1000-row left side probes in both.
        assert!(budget5.is_some_and(|b| b >= 1000), "{budget5:?}");
        assert!(budget6.is_some_and(|b| b >= 1000), "{budget6:?}");
    }

    /// A selective object on a fat leading predicate must estimate small in
    /// both layouts: `?s rdf:type <rare class>` is a few tiles of a predicate
    /// that fills thousands, and the planner seeds the join from it. On 0x06 a
    /// leaf that is not resident used to be estimated without the bound object
    /// (the whole of `rdf:type`) or by the ids' average share of its leaf
    /// (skewed ids put a rare class beside a huge one).
    #[test]
    fn a_rare_object_of_a_fat_predicate_estimates_small_in_both_layouts() {
        let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..120_000 {
            let s = format!("<http://ex/s{i:06}>");
            owned.push((s.clone(), ty.into(), "<http://ex/Common>".into()));
            owned.push((s.clone(), "<http://ex/other>".into(), format!("\"o {i}\"")));
            if i % 3000 == 0 {
                owned.push((s, ty.into(), "<http://ex/Rare>".into()));
            }
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &owned {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in &owned {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let v5 = write_file(&dict, &ib.build(), false, &[], 0);
        let v6: &'static [u8] = Box::leak(
            crate::v06::write::convert_image(&v5, Default::default())
                .unwrap()
                .into_boxed_slice(),
        );
        let leaked5: &'static [u8] = Box::leak(v5.into_boxed_slice());
        let figures = |rete: &Rete| {
            let est = |q: &[TriplePattern]| {
                let mut slots = Slots::new();
                collect_pattern_slots(q, &mut slots);
                let ctx = Ctx::new(rete, slots);
                bgp_min_estimate(&ctx, rete.default_index(), q).unwrap()
            };
            (
                est(&[pat("?s", ty, "<http://ex/Rare>")]),
                est(&[pat("?s", ty, "<http://ex/Common>")]),
                est(&[pat("?s", "<http://ex/other>", "?x")]),
            )
        };
        for (name, bytes) in [("0x05", leaked5), ("0x06", v6)] {
            let rete = Rete::open_ranged_lazy(crate::reader::SliceReader::new(bytes)).unwrap();
            let (rare, common, other) = figures(&rete);
            assert!(rare * 20.0 < other, "{name}: rare {rare} vs other {other}");
            assert!(
                rare * 20.0 < common,
                "{name}: rare {rare} vs common {common}"
            );
        }
    }

    /// A probe prefetch covers a bounded window of its patterns — at most
    /// [`PROBE_WINDOW_TILES`] tiles — and leaves the rest for the next call, so
    /// a long list of probes never faults more tiles than the cache holds (the
    /// whole list at once evicted the early tiles before their probes ran, and
    /// each came back as a request of its own: G50a). What a window covers is
    /// resident: probing it reads nothing more.
    #[test]
    fn a_probe_prefetch_window_is_bounded_and_warm() {
        use crate::reader::{CountingReader, SliceReader};
        let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..20_000 {
            let s = format!("<http://ex/s{i:06}>");
            owned.push((s.clone(), ty.into(), format!("<http://ex/C{}>", i % 7)));
            owned.push((s, "<http://ex/other>".into(), format!("\"o {}\"", i % 5000)));
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &owned {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(128);
        for (s, p, o) in &owned {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let v5 = write_file(&dict, &ib.build(), false, &[], 0);
        let v6 = crate::v06::write::convert_image(&v5, Default::default()).unwrap();
        let leaked: &'static [u8] = Box::leak(v6.into_boxed_slice());
        let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
        let index = rete.default_index();
        // A cache far smaller than the tiles the probes touch: a prefetch of every
        // probe at once would evict its own early tiles.
        index.set_cache_cap(1 << 20);
        // Every third subject, as the probes of a long join.
        let pats: Vec<Pattern> = (0..20_000)
            .step_by(3)
            .map(|i| {
                let (s, _, _) = dict
                    .encode(&format!("<http://ex/s{i:06}>"), ty, "<http://ex/C0>")
                    .unwrap();
                (Some(s), None, None)
            })
            .collect();
        let mut from = 0;
        let mut windows = 0;
        while from < pats.len() {
            let n = index.prefetch_probe_tiles(&pats[from..]);
            assert!(n >= 1, "a window always makes progress");
            assert!(
                n < pats.len() - from || windows > 0,
                "the first window is cut"
            );
            let before = reader.requests();
            for &p in &pats[from..from + n] {
                assert!(index.scan_iter(p).next().is_some());
            }
            assert_eq!(reader.requests(), before, "a covered probe reads nothing");
            from += n;
            windows += 1;
        }
        assert!(windows > 1, "{windows} window(s) for {} probes", pats.len());
    }

    /// The run of tiles holding a leading id is the same in both layouts, for
    /// every id and every permutation, and a leafed lookup does not walk the
    /// leaf tile by tile (a probe join runs it per row: 94,700 probes took 126 s
    /// on a 0x06 file against 1.3 s on its 0x05 copy).
    #[test]
    fn a_tile_span_is_the_same_in_both_layouts() {
        let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..30_000 {
            let s = format!("<http://ex/s{i:06}>");
            owned.push((s.clone(), ty.into(), format!("<http://ex/C{}>", i % 7)));
            owned.push((s, "<http://ex/other>".into(), format!("\"o {}\"", i % 5000)));
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &owned {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in &owned {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let v5 = write_file(&dict, &ib.build(), false, &[], 0);
        let v6 = crate::v06::write::convert_image(&v5, Default::default()).unwrap();
        let (v5, v6): (&'static [u8], &'static [u8]) = (
            Box::leak(v5.into_boxed_slice()),
            Box::leak(v6.into_boxed_slice()),
        );
        let flat = Rete::open_ranged_lazy(crate::reader::SliceReader::new(v5)).unwrap();
        let leafed = Rete::open_ranged_lazy(crate::reader::SliceReader::new(v6)).unwrap();
        let (fi, li) = (flat.default_index(), leafed.default_index());
        let mut checked = 0;
        for perm in fi.perms().iter() {
            let si = perm.section_index();
            for a in (0..dict.term_count() + 2).step_by(97) {
                assert_eq!(
                    fi.tile_span(si, Some(a)),
                    li.tile_span(si, Some(a)),
                    "section {si}, id {a}"
                );
                checked += 1;
            }
            assert_eq!(fi.tile_span(si, None), li.tile_span(si, None));
        }
        assert!(checked > 100);
    }

    /// Every join path the planner can pick — materialized prefix, probed last
    /// pattern, streamed seed with hashed or probed later patterns, streaming
    /// merge join — returns the same solution multiset. The thresholds are
    /// shrunk so a 400-triple graph takes each path.
    #[test]
    fn every_join_path_returns_the_same_solutions() {
        let mut owned: Vec<(String, String, String)> = Vec::new();
        let mut x: u64 = 7;
        let mut next = |m: u64| {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (x >> 33) % m
        };
        for i in 0..60 {
            let s = format!("s{i}");
            owned.push((s.clone(), "a".into(), format!("C{}", next(3))));
            for p in 0..4 {
                for _ in 0..next(3) {
                    owned.push((s.clone(), format!("p{p}"), format!("s{}", next(60))));
                }
            }
        }
        let triples: Vec<(&str, &str, &str)> = owned
            .iter()
            .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
            .collect();
        let bytes = rete_from(&triples);
        let rete = Rete::open(&bytes).unwrap();
        let queries: Vec<Vec<TriplePattern>> = vec![
            vec![pat("?s", "p0", "?a"), pat("?s", "p1", "?b")],
            vec![pat("?s", "a", "C1"), pat("?s", "p0", "?a")],
            vec![pat("?s", "p0", "?a"), pat("?a", "p2", "?b")],
            vec![pat("s5", "p0", "?a"), pat("?a", "?p", "?o")],
            vec![pat("?s", "p0", "?x"), pat("?s", "p1", "?x")],
            vec![
                pat("?s", "p3", "?o"),
                pat("?o", "a", "C0"),
                pat("?s", "p1", "?z"),
            ],
            vec![
                pat("?s", "p0", "?a"),
                pat("?s", "p1", "?b"),
                pat("?b", "p2", "?c"),
            ],
        ];
        let run = |limits: Option<PlanLimits>| -> Vec<Vec<Vec<(String, String)>>> {
            TEST_LIMITS.with(|c| c.set(limits));
            let out = queries
                .iter()
                .map(|q| {
                    let mut slots = crate::row::Slots::new();
                    collect_pattern_slots(q, &mut slots);
                    let ctx = Ctx::new(&rete, slots);
                    let mut rows: Vec<Vec<(String, String)>> =
                        BgpSolutions::new(&ctx, rete.default_index(), q)
                            .map(|r| row_to_binding(&ctx, &r).into_iter().collect())
                            .collect();
                    rows.sort();
                    rows
                })
                .collect();
            TEST_LIMITS.with(|c| c.set(None));
            out
        };
        let reference = run(None);
        assert!(reference.iter().all(|r| !r.is_empty()), "{reference:?}");
        for (name, l) in [
            (
                "probe everything",
                PlanLimits {
                    probe_floor: 0.0,
                    stream_seed: f64::MAX,
                    hash_build: f64::MAX,
                },
            ),
            (
                "stream + hash",
                PlanLimits {
                    probe_floor: f64::MAX,
                    stream_seed: 0.0,
                    hash_build: f64::MAX,
                },
            ),
            (
                "stream + probe",
                PlanLimits {
                    probe_floor: f64::MAX,
                    stream_seed: 0.0,
                    hash_build: 0.0,
                },
            ),
            (
                "stream, all paths",
                PlanLimits {
                    probe_floor: 0.0,
                    stream_seed: 0.0,
                    hash_build: 30.0,
                },
            ),
        ] {
            assert_eq!(run(Some(l)), reference, "{name}");
        }
    }

    #[test]
    fn single_pattern_binds_variable() {
        let bytes = rete_from(&[("Alice", "knows", "Bob"), ("Bob", "knows", "Carol")]);
        let rete = Rete::open(&bytes).unwrap();
        let sols = eval_bgp(&rete, &[pat("Alice", "knows", "?y")]);
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["y"], "Bob");
    }

    #[test]
    fn two_hop_join_on_shared_variable() {
        // Alice -> Bob -> Carol, plus a dead-end branch.
        let bytes = rete_from(&[
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Carol", "knows", "Dave"),
            ("Alice", "knows", "Eve"), // Eve knows no one -> no 2-hop
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let sols = eval_bgp(&rete, &[pat("?x", "knows", "?y"), pat("?y", "knows", "?z")]);
        // Alice-Bob-Carol, Bob-Carol-Dave.
        let mut got: Vec<_> = sols
            .iter()
            .map(|b| (b["x"].clone(), b["y"].clone(), b["z"].clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("Alice".into(), "Bob".into(), "Carol".into()),
                ("Bob".into(), "Carol".into(), "Dave".into()),
            ]
        );
    }

    #[test]
    fn repeated_variable_within_pattern() {
        // A mutual/self relation: only Bob knows himself.
        let bytes = rete_from(&[("Alice", "knows", "Bob"), ("Bob", "knows", "Bob")]);
        let rete = Rete::open(&bytes).unwrap();
        let sols = eval_bgp(&rete, &[pat("?x", "knows", "?x")]);
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["x"], "Bob");
    }

    #[test]
    fn no_solutions_yields_empty() {
        let bytes = rete_from(&[("Alice", "knows", "Bob")]);
        let rete = Rete::open(&bytes).unwrap();
        assert!(eval_bgp(&rete, &[pat("Alice", "likes", "?y")]).is_empty());
    }

    /// A LUBM-Q7-shaped snowflake: a popular class enumeration (`?x a Student`)
    /// plus a selective adjacency seed (`<Prof> teacherOf ?y`). Exercises both the
    /// **class-enum seed deprioritization** (the join must not start from the huge
    /// `a Student` extent) and the **probe-the-tail hybrid** (the type checks are
    /// applied late, per row). The answer must equal a brute-force computation
    /// regardless of how the join is ordered/probed.
    #[test]
    fn snowflake_type_and_adjacency_matches_reference() {
        const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let mut triples: Vec<(String, String, String)> = Vec::new();
        // 60 students, each a Student and taking 2 courses (c{i%5}, c{(i+1)%5}).
        for i in 0..60 {
            let s = format!("S{i}");
            triples.push((s.clone(), TYPE.into(), "Student".into()));
            triples.push((s.clone(), "takesCourse".into(), format!("c{}", i % 5)));
            triples.push((s, "takesCourse".into(), format!("c{}", (i + 1) % 5)));
        }
        // 5 courses; the professor teaches only c0 and c2.
        for j in 0..5 {
            triples.push((format!("c{j}"), TYPE.into(), "Course".into()));
        }
        triples.push(("Prof".into(), "teacherOf".into(), "c0".into()));
        triples.push(("Prof".into(), "teacherOf".into(), "c2".into()));

        let refs: Vec<(&str, &str, &str)> = triples
            .iter()
            .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
            .collect();
        let bytes = rete_from(&refs);
        let rete = Rete::open(&bytes).unwrap();

        let got: std::collections::BTreeSet<(String, String)> = eval_bgp(
            &rete,
            &[
                pat("?x", TYPE, "Student"),
                pat("?y", TYPE, "Course"),
                pat("?x", "takesCourse", "?y"),
                pat("Prof", "teacherOf", "?y"),
            ],
        )
        .into_iter()
        .map(|b| (b["x"].clone(), b["y"].clone()))
        .collect();

        // Brute force: students taking a course the professor teaches (c0 or c2).
        let taught = ["c0", "c2"];
        let mut want = std::collections::BTreeSet::new();
        for i in 0..60 {
            for c in [i % 5, (i + 1) % 5] {
                let course = format!("c{c}");
                if taught.contains(&course.as_str()) {
                    want.insert((format!("S{i}"), course));
                }
            }
        }
        assert_eq!(got, want, "snowflake join must match the brute-force set");
        assert!(!want.is_empty(), "sanity: the reference set is non-empty");
    }

    /// A subject-subject join (`?x a ?av . ?x b ?bv`) is the same-role shape the
    /// merge join handles: both patterns sort on the subject column. With multiple
    /// `a` and `b` values per subject it must emit the full cross product per
    /// subject (equal-key run handling) and drop subjects missing either side.
    #[test]
    fn merge_join_subject_star_cross_product() {
        let bytes = rete_from(&[
            ("x", "a", "a1"),
            ("x", "a", "a2"),
            ("x", "b", "b1"),
            ("x", "b", "b2"),
            ("y", "a", "a3"),
            ("y", "b", "b3"),
            ("z", "a", "a4"), // z has no `b` → contributes no join row
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let mut got: Vec<(String, String, String)> =
            eval_bgp(&rete, &[pat("?x", "a", "?av"), pat("?x", "b", "?bv")])
                .iter()
                .map(|m| (m["x"].clone(), m["av"].clone(), m["bv"].clone()))
                .collect();
        got.sort();
        let mut want: Vec<(String, String, String)> = [
            ("x", "a1", "b1"),
            ("x", "a1", "b2"),
            ("x", "a2", "b1"),
            ("x", "a2", "b2"),
            ("y", "a3", "b3"),
        ]
        .into_iter()
        .map(|(a, b, c)| (a.to_string(), b.to_string(), c.to_string()))
        .collect();
        want.sort();
        assert_eq!(got, want, "same-role merge must equal the brute-force join");
    }
}
