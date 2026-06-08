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

use std::collections::{BTreeMap, HashMap};

use crate::file::Rete;
use crate::index::GraphIndex;

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

/// A solution: variable name → bound term.
pub type Binding = BTreeMap<String, String>;

// --- integer join core ------------------------------------------------------
//
// Variables bind to a tagged i64: a node ID `n` is stored as `n` (>= 0), a
// predicate ID `p` as `-(p+1)` (< 0). Nodes are the unified subject/object space
// so a variable joins consistently across subject and object positions. (A
// variable ranging over both a predicate *and* a node — rare — won't match
// across those roles; that's the one HDT-split limitation.)

/// A pattern position lowered to integer space.
enum IntTerm {
    Var(String),
    Node(u32),
    Pred(u32),
}

/// An integer binding: variable → tagged i64 (node `>= 0`, predicate `< 0`).
pub(crate) type IntBinding = BTreeMap<String, i64>;

fn pred_tag(p: u32) -> i64 {
    -(p as i64) - 1
}

/// Lower a subject/object position; `None` if a constant term is unknown
/// (making the whole BGP unsatisfiable).
fn lower_node(t: &PatternTerm, d: &crate::Dictionary) -> Option<IntTerm> {
    match t {
        PatternTerm::Var(v) => Some(IntTerm::Var(v.clone())),
        PatternTerm::Const(c) => d.node_of_term(c).map(IntTerm::Node),
    }
}

/// Lower a predicate position; `None` if the constant predicate is unknown.
fn lower_pred(t: &PatternTerm, d: &crate::Dictionary) -> Option<IntTerm> {
    match t {
        PatternTerm::Var(v) => Some(IntTerm::Var(v.clone())),
        PatternTerm::Const(c) => d.predicate_id(c).map(IntTerm::Pred),
    }
}

/// Evaluate a BGP against the file's default graph, returning all solutions.
pub fn eval_bgp(rete: &Rete, patterns: &[TriplePattern]) -> Vec<Binding> {
    let dict = rete.dictionary();
    eval_bgp_int_in(rete, rete.default_index(), patterns)
        .into_iter()
        .map(|ib| {
            ib.into_iter()
                .filter_map(|(var, val)| term_of(dict, val).map(|t| (var, t)))
                .collect()
        })
        .collect()
}

/// Evaluate a BGP to *integer* bindings against a specific graph `index`
/// (joins run on integer IDs; the shared dictionary comes from `rete`).
pub(crate) fn eval_bgp_int_in(
    rete: &Rete,
    index: &GraphIndex,
    patterns: &[TriplePattern],
) -> Vec<IntBinding> {
    let dict = rete.dictionary();

    // Lower all patterns; a missing constant term makes the BGP empty.
    let mut lowered = Vec::with_capacity(patterns.len());
    for p in patterns {
        let (s, pr, o) = match (
            lower_node(&p.s, dict),
            lower_pred(&p.p, dict),
            lower_node(&p.o, dict),
        ) {
            (Some(s), Some(pr), Some(o)) => (s, pr, o),
            _ => return Vec::new(),
        };
        lowered.push((s, pr, o));
    }

    // Join the patterns most-constrained-first (and keeping the join connected),
    // so intermediate relations stay small. Pure reordering: the hash join is
    // order-independent, so the result is unchanged.
    let order = selectivity_order(&lowered);
    let mut bindings: Vec<IntBinding> = vec![IntBinding::new()];
    for &idx in &order {
        let (sp, pp, op) = &lowered[idx];
        // Scan this pattern ONCE, constraining only its constant terms (bound
        // variables are joined in afterwards, not pushed into the scan). A
        // constant in an impossible role / unknown to the dictionary makes the
        // pattern — and thus the whole BGP — unsatisfiable.
        let (sid, pid, oid) = match (
            const_subject(sp, dict),
            const_predicate(pp),
            const_object(op, dict),
        ) {
            (Some(s), Some(p), Some(o)) => (s, p, o),
            _ => return Vec::new(),
        };
        let mut rel: Vec<IntBinding> = Vec::new();
        // Stream the matches with the lazy cursor — the hash join is
        // order-independent, so no canonical re-sort is needed here.
        for (s_id, p_id, o_id) in index.scan_iter((sid, pid, oid)) {
            let s_node = dict.subject_node(s_id) as i64;
            let p_val = pred_tag(p_id);
            let o_node = dict.object_node(o_id) as i64;
            // extend_int from an empty base enforces repeated variables *within*
            // this pattern (e.g. `?x p ?x`).
            if let Some(rb) = extend_int(&IntBinding::new(), sp, pp, op, s_node, p_val, o_node) {
                rel.push(rb);
            }
        }
        bindings = hash_join(bindings, rel);
        if bindings.is_empty() {
            break;
        }
    }
    bindings
}

/// Order pattern indices for a left-deep join: most-constrained (most constant
/// terms) first, and after the seed always preferring a pattern that shares a
/// variable with the already-joined set so the join stays connected and
/// intermediate relations stay small. Pure reordering — `hash_join` is
/// order-independent, so the result multiset is unchanged.
fn selectivity_order(lowered: &[(IntTerm, IntTerm, IntTerm)]) -> Vec<usize> {
    let consts = |t: &(IntTerm, IntTerm, IntTerm)| {
        [&t.0, &t.1, &t.2]
            .into_iter()
            .filter(|x| !matches!(x, IntTerm::Var(_)))
            .count()
    };
    let vars = |t: &(IntTerm, IntTerm, IntTerm)| -> Vec<String> {
        [&t.0, &t.1, &t.2]
            .into_iter()
            .filter_map(|x| match x {
                IntTerm::Var(v) => Some(v.clone()),
                _ => None,
            })
            .collect()
    };
    let n = lowered.len();
    let mut remaining: Vec<usize> = (0..n).collect();
    let mut order: Vec<usize> = Vec::with_capacity(n);
    let mut bound: std::collections::HashSet<String> = std::collections::HashSet::new();
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
pub(crate) fn bgp_exists(rete: &Rete, index: &GraphIndex, patterns: &[TriplePattern]) -> bool {
    let dict = rete.dictionary();
    let mut lowered = Vec::with_capacity(patterns.len());
    for p in patterns {
        match (
            lower_node(&p.s, dict),
            lower_pred(&p.p, dict),
            lower_node(&p.o, dict),
        ) {
            (Some(s), Some(pr), Some(o)) => lowered.push((s, pr, o)),
            // An unknown constant term makes the pattern unsatisfiable.
            _ => return false,
        }
    }
    if let [t] = lowered.as_slice() {
        // The fast path can't enforce a variable repeated across positions
        // (e.g. `?x p ?x`) — that needs `extend_int` — so only take it when the
        // pattern's variables are all distinct.
        let names: Vec<&str> = [&t.0, &t.1, &t.2]
            .into_iter()
            .filter_map(|x| match x {
                IntTerm::Var(v) => Some(v.as_str()),
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
    !eval_bgp_int_in(rete, index, patterns).is_empty()
}

/// A lazy left-deep BGP join that yields term-resolved solution bindings one at a
/// time, so a consumer under `LIMIT`/`OFFSET` (with no ORDER BY/DISTINCT) can
/// stop early. The all-but-last patterns are joined eagerly into `prefix`; the
/// last pattern is probed lazily. Yields the same solution multiset as
/// [`eval_bgp_int_in`] (the join is order-independent), just incrementally.
pub(crate) struct BgpSolutions<'r> {
    rete: &'r Rete,
    prefix: Vec<IntBinding>,
    rel: Vec<IntBinding>,
    buckets: HashMap<Vec<i64>, Vec<usize>>,
    shared: Vec<String>,
    cartesian: bool,
    li: usize,
    cur_left: Option<IntBinding>,
    matches: Vec<usize>,
    mi: usize,
}

impl<'r> BgpSolutions<'r> {
    /// An iterator that yields nothing (an unsatisfiable BGP).
    fn empty(rete: &'r Rete) -> Self {
        BgpSolutions {
            rete,
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

    pub(crate) fn new(rete: &'r Rete, index: &GraphIndex, patterns: &[TriplePattern]) -> Self {
        let dict = rete.dictionary();
        // An empty BGP has exactly one (empty) solution.
        if patterns.is_empty() {
            return BgpSolutions {
                rete,
                prefix: vec![IntBinding::new()],
                rel: vec![IntBinding::new()],
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
        let mut lowered = Vec::with_capacity(patterns.len());
        for p in patterns {
            match (
                lower_node(&p.s, dict),
                lower_pred(&p.p, dict),
                lower_node(&p.o, dict),
            ) {
                (Some(s), Some(pr), Some(o)) => lowered.push((s, pr, o)),
                _ => return Self::empty(rete),
            }
        }
        // Make the *last* (least selective) pattern the lazy probe; join the rest
        // eagerly into the prefix.
        let order = selectivity_order(&lowered);
        let (&last_i, prefix_is) = order.split_last().unwrap();
        let prefix_pats: Vec<TriplePattern> =
            prefix_is.iter().map(|&i| patterns[i].clone()).collect();
        let prefix = eval_bgp_int_in(rete, index, &prefix_pats);

        // Build the last pattern's relation with the lazy index cursor.
        let (sp, pp, op) = &lowered[last_i];
        let rel: Vec<IntBinding> = match (
            const_subject(sp, dict),
            const_predicate(pp),
            const_object(op, dict),
        ) {
            (Some(sid), Some(pid), Some(oid)) => {
                let mut rel = Vec::new();
                for (s_id, p_id, o_id) in index.scan_iter((sid, pid, oid)) {
                    let s_node = dict.subject_node(s_id) as i64;
                    let p_val = pred_tag(p_id);
                    let o_node = dict.object_node(o_id) as i64;
                    if let Some(rb) =
                        extend_int(&IntBinding::new(), sp, pp, op, s_node, p_val, o_node)
                    {
                        rel.push(rb);
                    }
                }
                rel
            }
            _ => Vec::new(),
        };
        if prefix.is_empty() || rel.is_empty() {
            return Self::empty(rete);
        }

        // Hash-join key = variables shared by the prefix and the last pattern
        // (BGP bindings are total, so every shared var is bound on both sides).
        let shared: Vec<String> = prefix[0]
            .keys()
            .filter(|k| rel[0].contains_key(*k))
            .cloned()
            .collect();
        let cartesian = shared.is_empty();
        let mut buckets: HashMap<Vec<i64>, Vec<usize>> = HashMap::new();
        if !cartesian {
            for (i, r) in rel.iter().enumerate() {
                let key: Vec<i64> = shared.iter().map(|k| r[k]).collect();
                buckets.entry(key).or_default().push(i);
            }
        }
        BgpSolutions {
            rete,
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

impl Iterator for BgpSolutions<'_> {
    type Item = Binding;

    fn next(&mut self) -> Option<Binding> {
        let dict = self.rete.dictionary();
        loop {
            // Emit the next match for the current left (prefix) row.
            if self.mi < self.matches.len() {
                let ri = self.matches[self.mi];
                self.mi += 1;
                let mut merged = self.cur_left.clone().unwrap();
                for (k, v) in &self.rel[ri] {
                    merged.insert(k.clone(), *v);
                }
                return Some(
                    merged
                        .into_iter()
                        .filter_map(|(k, v)| term_of(dict, v).map(|t| (k, t)))
                        .collect(),
                );
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
                let key: Vec<i64> = self.shared.iter().map(|k| left[k]).collect();
                self.buckets.get(&key).cloned().unwrap_or_default()
            };
            self.mi = 0;
            self.cur_left = Some(left);
        }
    }
}

/// Hash-join two solution relations on the variables they share.
///
/// Every binding in `left` carries the same variable set (the union of the
/// already-processed patterns' variables), and every binding in `right` carries
/// the current pattern's variable set — so the shared keys are uniform and can
/// be read off the first row of each side.
fn hash_join(left: Vec<IntBinding>, right: Vec<IntBinding>) -> Vec<IntBinding> {
    // Fast path: the seed `[{}]` joins to `right` unchanged (first pattern).
    if left.len() == 1 && left[0].is_empty() {
        return right;
    }
    if left.is_empty() || right.is_empty() {
        return Vec::new();
    }
    let shared: Vec<String> = {
        let rkeys = &right[0];
        left[0]
            .keys()
            .filter(|k| rkeys.contains_key(*k))
            .cloned()
            .collect()
    };
    if shared.is_empty() {
        // No shared variable: Cartesian product. In a connected BGP this only
        // arises for a fully-ground pattern (deduped index ⇒ one matching row),
        // so it acts as an existence filter rather than multiplying rows.
        let mut out = Vec::with_capacity(left.len() * right.len());
        for l in &left {
            for r in &right {
                out.push(merge(l, r));
            }
        }
        return out;
    }
    let key_of = |b: &IntBinding| -> Vec<i64> { shared.iter().map(|k| b[k]).collect() };
    let mut buckets: HashMap<Vec<i64>, Vec<IntBinding>> = HashMap::new();
    for r in right {
        buckets.entry(key_of(&r)).or_default().push(r);
    }
    let mut out = Vec::new();
    for l in &left {
        if let Some(rs) = buckets.get(&key_of(l)) {
            for r in rs {
                out.push(merge(l, r));
            }
        }
    }
    out
}

/// Merge two bindings that already agree on their shared variables.
fn merge(l: &IntBinding, r: &IntBinding) -> IntBinding {
    let mut b = l.clone();
    for (k, v) in r {
        b.insert(k.clone(), *v);
    }
    b
}

/// Resolve a tagged integer binding value to its term.
pub(crate) fn term_of_value(dict: &crate::Dictionary, val: i64) -> Option<String> {
    term_of(dict, val)
}

// Constant-only index constraints. A variable scans as a wildcard (`Some(None)`)
// — it is resolved later by the hash join, not pushed into the scan. The outer
// `None` means "unsatisfiable" (a constant in an impossible role, or unknown to
// the dictionary), which empties the whole BGP. Inner `None` = "wildcard".
fn const_subject(t: &IntTerm, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        IntTerm::Node(n) => d.node_as_subject_id(*n).map(Some),
        IntTerm::Pred(_) => None, // a predicate term can't be a subject
        IntTerm::Var(_) => Some(None),
    }
}

fn const_object(t: &IntTerm, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        IntTerm::Node(n) => d.node_as_object_id(*n).map(Some),
        IntTerm::Pred(_) => None,
        IntTerm::Var(_) => Some(None),
    }
}

fn const_predicate(t: &IntTerm) -> Option<Option<u32>> {
    match t {
        IntTerm::Pred(p) => Some(Some(*p)),
        IntTerm::Node(_) => None, // a node term can't be a predicate
        IntTerm::Var(_) => Some(None),
    }
}

/// Extend an integer binding, rejecting inconsistent variable reuse.
fn extend_int(
    base: &IntBinding,
    sp: &IntTerm,
    pp: &IntTerm,
    op: &IntTerm,
    s: i64,
    p: i64,
    o: i64,
) -> Option<IntBinding> {
    let mut b = base.clone();
    for (term, val) in [(sp, s), (pp, p), (op, o)] {
        if let IntTerm::Var(v) = term {
            match b.get(v) {
                Some(&existing) if existing != val => return None,
                Some(_) => {}
                None => {
                    b.insert(v.clone(), val);
                }
            }
        }
    }
    Some(b)
}

fn term_of(dict: &crate::Dictionary, val: i64) -> Option<String> {
    if val >= 0 {
        dict.node_term(val as u32)
    } else {
        dict.predicate_term((-val - 1) as u32)
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
