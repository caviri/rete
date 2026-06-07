//! Basic Graph Pattern (BGP) evaluation — the core of SPARQL (SPEC.md §8,
//! stage 1). A BGP is a set of triple patterns whose variables join on equality;
//! evaluating it yields variable bindings.
//!
//! This is a left-deep nested-loop join over [`Rete::query`]: start with one
//! empty binding, and for each pattern substitute already-bound variables, scan
//! the matching triples, and extend the binding (rejecting inconsistent reuses
//! of a variable). A later pass can add cardinality-based pattern reordering;
//! correctness does not depend on order.

use std::collections::BTreeMap;

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

    let mut bindings: Vec<IntBinding> = vec![IntBinding::new()];
    for (sp, pp, op) in &lowered {
        let mut next = Vec::new();
        for b in &bindings {
            let (sid, pid, oid) = match (
                subject_constraint(sp, b, dict),
                predicate_constraint(pp, b),
                object_constraint(op, b, dict),
            ) {
                (Some(s), Some(p), Some(o)) => (s, p, o),
                _ => continue,
            };
            for (s_id, p_id, o_id) in index.match_pattern((sid, pid, oid)) {
                let s_node = dict.subject_node(s_id) as i64;
                let p_val = pred_tag(p_id);
                let o_node = dict.object_node(o_id) as i64;
                if let Some(nb) = extend_int(b, sp, pp, op, s_node, p_val, o_node) {
                    next.push(nb);
                }
            }
        }
        bindings = next;
        if bindings.is_empty() {
            break;
        }
    }
    bindings
}

/// Resolve a tagged integer binding value to its term.
pub(crate) fn term_of_value(dict: &crate::Dictionary, val: i64) -> Option<String> {
    term_of(dict, val)
}

/// `Some(constraint)` for the index query; outer `None` means "skip this
/// binding" (impossible role). Inner `None` means "unbound wildcard".
fn subject_constraint(t: &IntTerm, b: &IntBinding, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        IntTerm::Node(n) => d.node_as_subject_id(*n).map(Some),
        IntTerm::Pred(_) => None,
        IntTerm::Var(v) => match b.get(v) {
            Some(&val) if val >= 0 => d.node_as_subject_id(val as u32).map(Some),
            Some(_) => None, // bound to a predicate; can't be a subject
            None => Some(None),
        },
    }
}

fn object_constraint(t: &IntTerm, b: &IntBinding, d: &crate::Dictionary) -> Option<Option<u32>> {
    match t {
        IntTerm::Node(n) => d.node_as_object_id(*n).map(Some),
        IntTerm::Pred(_) => None,
        IntTerm::Var(v) => match b.get(v) {
            Some(&val) if val >= 0 => d.node_as_object_id(val as u32).map(Some),
            Some(_) => None,
            None => Some(None),
        },
    }
}

fn predicate_constraint(t: &IntTerm, b: &IntBinding) -> Option<Option<u32>> {
    match t {
        IntTerm::Pred(p) => Some(Some(*p)),
        IntTerm::Node(_) => None,
        IntTerm::Var(v) => match b.get(v) {
            Some(&val) if val < 0 => Some(Some((-val - 1) as u32)),
            Some(_) => None, // bound to a node; can't be a predicate
            None => Some(None),
        },
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
