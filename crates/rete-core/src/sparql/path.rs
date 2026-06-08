//! Property-path evaluation (SPEC.md §8): `subject <path> object` as a binary
//! relation over the graph's nodes. Traversal runs on integer node IDs; terms
//! are resolved only for the produced bindings, and a constant endpoint is
//! pushed down so an unbounded path never enumerates the whole graph.

use crate::bgp::{Binding, PatternTerm};
use crate::file::Rete;
use crate::index::GraphIndex;

use super::{reverse, PathAst, Rep};

/// Cached adjacency keyed by `(predicate, reversed)` → start node → successor
/// nodes. Everything is in the unified integer node space — no term strings.
type AdjCache =
    std::collections::HashMap<(String, bool), std::collections::BTreeMap<u32, Vec<u32>>>;

/// Successor nodes of `start` along a single predicate (built and cached on
/// first use, directly from integer node pairs — no term resolution).
fn successors(
    rete: &Rete,
    index: &GraphIndex,
    cache: &mut AdjCache,
    pred: &str,
    rev: bool,
    start: u32,
) -> Vec<u32> {
    let key = (pred.to_string(), rev);
    if !cache.contains_key(&key) {
        let dict = rete.dictionary();
        let pairs: Vec<(u32, u32)> = match dict.predicate_id(pred) {
            Some(pid) => index
                .match_pattern((None, Some(pid), None))
                .into_iter()
                .map(|(s, _p, o)| (dict.subject_node(s), dict.object_node(o)))
                .collect(),
            None => Vec::new(),
        };
        let mut adj: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for (s, o) in pairs {
            let (from, to) = if rev { (o, s) } else { (s, o) };
            adj.entry(from).or_default().push(to);
        }
        cache.insert(key.clone(), adj);
    }
    cache
        .get(&key)
        .and_then(|a| a.get(&start))
        .cloned()
        .unwrap_or_default()
}

/// Nodes reachable from `start` along `ast` — forward from the start node, so a
/// bound endpoint never triggers a global closure. Integer node space.
fn reach_from(
    rete: &Rete,
    index: &GraphIndex,
    ast: &PathAst,
    start: u32,
    cache: &mut AdjCache,
) -> std::collections::BTreeSet<u32> {
    use std::collections::BTreeSet;
    match ast {
        PathAst::Pred(p, rev) => successors(rete, index, cache, p, *rev, start)
            .into_iter()
            .collect(),
        PathAst::Alt(a, b) => {
            let mut r = reach_from(rete, index, a, start, cache);
            r.extend(reach_from(rete, index, b, start, cache));
            r
        }
        PathAst::Seq(a, b) => {
            let mids = reach_from(rete, index, a, start, cache);
            let mut out = BTreeSet::new();
            for m in &mids {
                out.extend(reach_from(rete, index, b, *m, cache));
            }
            out
        }
        PathAst::Rep(inner, rep) => match rep {
            Rep::One => reach_from(rete, index, inner, start, cache),
            Rep::ZeroOrOne => {
                let mut r = reach_from(rete, index, inner, start, cache);
                r.insert(start);
                r
            }
            Rep::OneOrMore | Rep::ZeroOrMore => {
                let mut visited = BTreeSet::new();
                let mut stack: Vec<u32> = reach_from(rete, index, inner, start, cache)
                    .into_iter()
                    .collect();
                while let Some(n) = stack.pop() {
                    if visited.insert(n) {
                        for m in reach_from(rete, index, inner, n, cache) {
                            if !visited.contains(&m) {
                                stack.push(m);
                            }
                        }
                    }
                }
                if *rep == Rep::ZeroOrMore {
                    visited.insert(start); // zero-length path
                }
                visited
            }
        },
    }
}

fn bind_pair(subj: &PatternTerm, obj: &PatternTerm, a: &str, b: &str) -> Option<Binding> {
    let mut binding = Binding::new();
    for (term, val) in [(subj, a), (obj, b)] {
        if let PatternTerm::Var(v) = term {
            match binding.get(v) {
                Some(existing) if existing != val => return None,
                _ => {
                    binding.insert(v.clone(), val.to_string());
                }
            }
        }
    }
    Some(binding)
}

/// Evaluate a property path to solution bindings. Traversal runs on integer node
/// IDs; terms are resolved only for the produced bindings. A constant endpoint is
/// pushed down so unbounded paths don't enumerate the whole graph.
pub(super) fn eval_path(
    rete: &Rete,
    index: &GraphIndex,
    subj: &PatternTerm,
    ast: &PathAst,
    obj: &PatternTerm,
) -> Vec<Binding> {
    let dict = rete.dictionary();
    let mut cache = AdjCache::new();
    let mut out = Vec::new();

    match (subj, obj) {
        // Bound subject: forward search from its node.
        (PatternTerm::Const(s), _) => {
            let Some(sn) = dict.node_of_term(s) else {
                return Vec::new();
            };
            let obj_node = match obj {
                PatternTerm::Const(o) => Some(dict.node_of_term(o)),
                _ => None,
            };
            for e in reach_from(rete, index, ast, sn, &mut cache) {
                if let Some(on) = obj_node {
                    if on != Some(e) {
                        continue;
                    }
                }
                if let Some(et) = dict.node_term(e) {
                    if let Some(b) = bind_pair(subj, obj, s, &et) {
                        out.push(b);
                    }
                }
            }
        }
        // Bound object only: search backward along the reversed path.
        (PatternTerm::Var(_), PatternTerm::Const(o)) => {
            let Some(on) = dict.node_of_term(o) else {
                return Vec::new();
            };
            let rev = reverse(ast.clone());
            for s in reach_from(rete, index, &rev, on, &mut cache) {
                if let Some(st) = dict.node_term(s) {
                    if let Some(b) = bind_pair(subj, obj, &st, o) {
                        out.push(b);
                    }
                }
            }
        }
        // Both unbound: enumerate from every node (inherently expensive).
        (PatternTerm::Var(_), PatternTerm::Var(_)) => {
            for start in 0..dict.node_count() {
                for e in reach_from(rete, index, ast, start, &mut cache) {
                    if let (Some(st), Some(et)) = (dict.node_term(start), dict.node_term(e)) {
                        if let Some(b) = bind_pair(subj, obj, &st, &et) {
                            out.push(b);
                        }
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}
