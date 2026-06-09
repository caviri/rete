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
    let order = selectivity_order(&lowered);
    let mut rows: Vec<Row> = vec![ctx.slots.empty_row()];
    let mut bound: Vec<usize> = Vec::new();
    for &idx in &order {
        let Some((rel, rel_slots)) = pattern_rows(ctx, index, &lowered[idx]) else {
            return Vec::new();
        };
        rows = hash_join(rows, &bound, rel, &rel_slots);
        for s in rel_slots {
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

/// Scan one lowered pattern into a relation of rows, returning the rows and the
/// (deduped) slots the pattern binds. `None` = a constant is unsatisfiable.
fn pattern_rows(
    ctx: &Ctx,
    index: &GraphIndex,
    t: &(SlotTerm, SlotTerm, SlotTerm),
) -> Option<(Vec<Row>, Vec<usize>)> {
    let dict = ctx.rete.dictionary();
    let (sp, pp, op) = t;
    // Scan this pattern ONCE, constraining only its constant terms (bound
    // variables are joined in afterwards, not pushed into the scan). A constant
    // in an impossible role / unknown to the dictionary makes the pattern — and
    // thus the whole BGP — unsatisfiable.
    let (sid, pid, oid) = (
        const_subject(sp, dict)?,
        const_predicate(pp)?,
        const_object(op, dict)?,
    );
    let mut slots: Vec<usize> = Vec::new();
    for term in [sp, pp, op] {
        if let SlotTerm::Var(i) = term {
            if !slots.contains(i) {
                slots.push(*i);
            }
        }
    }
    let mut rel: Vec<Row> = Vec::new();
    // Stream the matches with the lazy cursor — the hash join is
    // order-independent, so no canonical re-sort is needed here.
    'scan: for (s_id, p_id, o_id) in index.scan_iter((sid, pid, oid)) {
        let s_val = dict.subject_node(s_id) as i64;
        let p_val = ctx.resolver.canon_id(pred_tag(p_id));
        let o_val = dict.object_node(o_id) as i64;
        let mut row = ctx.slots.empty_row();
        // Setting each slot enforces repeated variables *within* this pattern
        // (e.g. `?x p ?x`).
        for (term, val) in [(sp, s_val), (pp, p_val), (op, o_val)] {
            if let SlotTerm::Var(i) = term {
                match row[*i] {
                    Some(Val::Id(existing)) if existing != val => continue 'scan,
                    Some(_) => {}
                    None => row[*i] = Some(Val::Id(val)),
                }
            }
        }
        rel.push(row);
    }
    Some((rel, slots))
}

/// Order pattern indices for a left-deep join: most-constrained (most constant
/// terms) first, and after the seed always preferring a pattern that shares a
/// variable with the already-joined set so the join stays connected and
/// intermediate relations stay small. Pure reordering — `hash_join` is
/// order-independent, so the result multiset is unchanged.
fn selectivity_order(lowered: &[(SlotTerm, SlotTerm, SlotTerm)]) -> Vec<usize> {
    let consts = |t: &(SlotTerm, SlotTerm, SlotTerm)| {
        [&t.0, &t.1, &t.2]
            .into_iter()
            .filter(|x| !matches!(x, SlotTerm::Var(_)))
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
    let n = lowered.len();
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut bound: std::collections::HashSet<usize> = std::collections::HashSet::new();
    while !remaining.is_empty() {
        // Pick the remaining pattern with the best (connected, consts) key; ties
        // go to the lowest original index for a stable, predictable order.
        let best = *remaining
            .iter()
            .max_by(|&&a, &&b| {
                let key = |i: usize| {
                    let t = &lowered[i];
                    let connected = !order.is_empty() && vars(t).iter().any(|v| bound.contains(v));
                    (connected, consts(t))
                };
                key(a).cmp(&key(b)).then(b.cmp(&a))
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
/// consumer under `LIMIT`/`OFFSET` (with no ORDER BY/DISTINCT) can stop early.
/// The all-but-last patterns are joined eagerly into `prefix`; the last pattern
/// is probed lazily. Yields the same solution multiset as [`eval_bgp_rows`]
/// (the join is order-independent), just incrementally.
pub(crate) struct BgpSolutions {
    prefix: Vec<Row>,
    rel: Vec<Row>,
    buckets: HashMap<Vec<Val>, Vec<usize>>,
    shared: Vec<usize>,
    cartesian: bool,
    li: usize,
    cur_left: Option<Row>,
    matches: Vec<usize>,
    mi: usize,
}

impl BgpSolutions {
    /// An iterator that yields nothing (an unsatisfiable BGP).
    fn empty() -> Self {
        BgpSolutions {
            prefix: Vec::new(),
            rel: Vec::new(),
            buckets: HashMap::new(),
            shared: Vec::new(),
            cartesian: false,
            li: 0,
            cur_left: None,
            matches: Vec::new(),
            mi: 0,
        }
    }

    pub(crate) fn new(ctx: &Ctx, index: &GraphIndex, patterns: &[TriplePattern]) -> Self {
        // An empty BGP has exactly one (empty) solution.
        if patterns.is_empty() {
            return BgpSolutions {
                prefix: vec![ctx.slots.empty_row()],
                rel: vec![ctx.slots.empty_row()],
                buckets: HashMap::new(),
                shared: Vec::new(),
                cartesian: true,
                li: 0,
                cur_left: None,
                matches: Vec::new(),
                mi: 0,
            };
        }
        // Lower all patterns; an unknown constant term ⇒ no solutions.
        let Some(lowered) = lower(patterns, ctx) else {
            return Self::empty();
        };
        // Make the *last* (least selective) pattern the lazy probe; join the rest
        // eagerly into the prefix.
        let order = selectivity_order(&lowered);
        let (&last_i, prefix_is) = order.split_last().unwrap();
        let prefix_pats: Vec<TriplePattern> =
            prefix_is.iter().map(|&i| patterns[i].clone()).collect();
        let prefix = eval_bgp_rows(ctx, index, &prefix_pats);

        // Build the last pattern's relation with the lazy index cursor.
        let Some((rel, rel_slots)) = pattern_rows(ctx, index, &lowered[last_i]) else {
            return Self::empty();
        };
        if prefix.is_empty() || rel.is_empty() {
            return Self::empty();
        }

        // Hash-join key = slots shared by the prefix and the last pattern (BGP
        // rows bind every slot of their pattern set, so the prefix's bound set
        // can be read off its first row).
        let shared: Vec<usize> = rel_slots
            .iter()
            .copied()
            .filter(|&s| prefix[0][s].is_some())
            .collect();
        let cartesian = shared.is_empty();
        let mut buckets: HashMap<Vec<Val>, Vec<usize>> = HashMap::new();
        if !cartesian {
            for (i, r) in rel.iter().enumerate() {
                let key: Vec<Val> = shared.iter().map(|&s| r[s].clone().unwrap()).collect();
                buckets.entry(key).or_default().push(i);
            }
        }
        BgpSolutions {
            prefix,
            rel,
            buckets,
            shared,
            cartesian,
            li: 0,
            cur_left: None,
            matches: Vec::new(),
            mi: 0,
        }
    }
}

impl Iterator for BgpSolutions {
    type Item = Row;

    fn next(&mut self) -> Option<Row> {
        loop {
            // Emit the next match for the current left (prefix) row.
            if self.mi < self.matches.len() {
                let ri = self.matches[self.mi];
                self.mi += 1;
                let mut merged = self.cur_left.clone().unwrap();
                for (slot, v) in self.rel[ri].iter().enumerate() {
                    if v.is_some() {
                        merged[slot] = v.clone();
                    }
                }
                return Some(merged);
            }
            // Advance to the next left row and gather its matches.
            if self.li >= self.prefix.len() {
                return None;
            }
            let left = self.prefix[self.li].clone();
            self.li += 1;
            self.matches = if self.cartesian {
                (0..self.rel.len()).collect()
            } else {
                let key: Vec<Val> = self
                    .shared
                    .iter()
                    .map(|&s| left[s].clone().unwrap())
                    .collect();
                self.buckets.get(&key).cloned().unwrap_or_default()
            };
            self.mi = 0;
            self.cur_left = Some(left);
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
    let mut buckets: HashMap<Vec<Val>, Vec<Row>> = HashMap::new();
    for r in right {
        buckets.entry(key_of(&r)).or_default().push(r);
    }
    let mut out = Vec::new();
    for l in &left {
        if let Some(rs) = buckets.get(&key_of(l)) {
            for r in rs {
                out.push(fill(l, r));
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
}
