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
//! Solutions are slot [`Row`]s of tagged dictionary ids (see `crate::row`):
//! joins hash and compare integers, and terms are resolved to strings only at
//! the engine's projection boundary, never per intermediate row.

use std::collections::{BTreeMap, HashMap};

use crate::file::Rete;
use crate::index::GraphIndex;
use crate::row::{Ctx, Row, Slots, Val};

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
    let order = selectivity_order(ctx, &lowered);
    let mut rows: Vec<Row> = vec![ctx.slots.empty_row()];
    let mut bound: Vec<usize> = Vec::new();
    for (k, &idx) in order.iter().enumerate() {
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
        if k > 0 && shares_bound && !rows.is_empty() && rows.len() <= BGP_PROBE_THRESHOLD {
            let mut next: Vec<Row> = Vec::with_capacity(rows.len());
            for base in std::mem::take(&mut rows) {
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

/// Probe the join tail (instead of full-scanning) once the running BGP result is
/// at most this many rows: each remaining pattern that shares a bound variable is
/// then resolved by `rows × index-lookup` rather than a whole-extent scan. Kept
/// small so the probe count is always cheap in absolute terms — the dominant win
/// is the large-scan-after-tiny-prefix case (a `?x a C` over thousands when the
/// prefix already pinned ?x to a few dozen rows); a *moderately* large prefix
/// keeps the one-pass scan + hash join, which beats thousands of probes.
const BGP_PROBE_THRESHOLD: usize = 512;

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

/// Build a solution row for one scanned triple, enforcing repeated variables
/// *within* the pattern (e.g. `?x p ?x`). `None` = the triple doesn't satisfy
/// a repeated variable.
fn triple_row(
    ctx: &Ctx,
    t: &(SlotTerm, SlotTerm, SlotTerm),
    (s_id, p_id, o_id): (u32, u32, u32),
) -> Option<Row> {
    let dict = ctx.rete.dictionary();
    let s_val = dict.subject_node(s_id) as i64;
    let p_val = ctx.resolver.canon_id(pred_tag(p_id));
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
        index
            .scan_iter((sid, pid, oid))
            .filter_map(move |triple| triple_row(ctx, &t, triple)),
    )
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
    for triple in index.scan_iter((sid, pid, oid)) {
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
    // Measured per-predicate distinct subjects/objects (query_stats block; empty
    // on files built before it existed → fall back to the default selectivities).
    let stats: std::collections::HashMap<u32, &crate::meta::PredStat> = pyr
        .predicate_stats
        .iter()
        .map(|s| (s.predicate, s))
        .collect();
    let total = ctx.rete.header().quad_count.max(1) as f64;
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
                let mut est = base.max(1.0);
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

/// Order pattern indices for a left-deep join: cheapest (smallest estimated
/// cardinality) first, always preferring a pattern that shares a variable with
/// the already-joined set so the join stays connected and intermediate relations
/// stay small. Cardinality comes from [`pattern_estimates`]; with no summary it
/// falls back to a most-constants-first heuristic. Pure reordering — `hash_join`
/// is order-independent, so the result multiset is unchanged.
fn selectivity_order(ctx: &Ctx, lowered: &[(SlotTerm, SlotTerm, SlotTerm)]) -> Vec<usize> {
    selectivity_order_seeded(ctx, lowered, &std::collections::HashSet::new())
}

/// [`selectivity_order`] with a set of slots already bound by an outer seed
/// row: those variables count as already connected (and, in the no-summary
/// fallback, as constants for selectivity).
fn selectivity_order_seeded(
    ctx: &Ctx,
    lowered: &[(SlotTerm, SlotTerm, SlotTerm)],
    seed: &std::collections::HashSet<usize>,
) -> Vec<usize> {
    let estimates = pattern_estimates(ctx, lowered, seed);
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
        type_pid.is_some_and(|tp| matches!(t.1, SlotTerm::Pred(p) if p == tp))
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
        let order = selectivity_order(ctx, &lowered);
        let (&last_i, prefix_is) = order.split_last().unwrap();
        let prefix_pats: Vec<TriplePattern> =
            prefix_is.iter().map(|&i| patterns[i].clone()).collect();
        let prefix = eval_bgp_rows(ctx, index, &prefix_pats);
        if prefix.is_empty() {
            return Self::empty();
        }

        // Stream the last pattern with the lazy index cursor.
        let Some(scan) = scan_rows(ctx, index, lowered[last_i]) else {
            return Self::empty();
        };

        // Hash-join key = slots shared by the prefix and the last pattern (BGP
        // rows bind every slot of their pattern set, so the prefix's bound set
        // can be read off its first row).
        let shared: Vec<usize> = pattern_slots(&lowered[last_i])
            .into_iter()
            .filter(|&s| prefix[0][s].is_some())
            .collect();
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
    Box::new(
        index
            .scan_iter((sid, pid, oid))
            .filter_map(move |(s_id, p_id, o_id)| {
                let s_val = dict.subject_node(s_id) as i64;
                let p_val = ctx.resolver.canon_id(pred_tag(p_id));
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
            }),
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
    pub(crate) fn new(ctx: &Ctx, patterns: &[TriplePattern], seed_mask: &[bool]) -> Option<Self> {
        let lowered = lower(patterns, ctx)?;
        let seed: std::collections::HashSet<usize> = seed_mask
            .iter()
            .enumerate()
            .filter_map(|(i, b)| b.then_some(i))
            .collect();
        let order = selectivity_order_seeded(ctx, &lowered, &seed);
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
        let plan = ProbePlan::new(ctx, patterns, &vec![false; ctx.slots.len()])?;
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
    use crate::file::write_file;
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
}
