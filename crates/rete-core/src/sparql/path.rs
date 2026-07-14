//! Property-path evaluation (SPEC.md §8): `subject <path> object` as a binary
//! relation over the graph's nodes. Traversal runs on integer node IDs and
//! solutions are emitted as integer slot rows — terms are never resolved here
//! (the engine materializes only at projection). A constant endpoint is pushed
//! down so an unbounded path never enumerates the whole graph.

use crate::bgp::PatternTerm;
use crate::index::GraphIndex;
use crate::row::{Ctx, Row, Val};

use super::{reverse, PathAst, Rep};

/// Per-start-node successor cache keyed by `(predicate-or-negated-set, reversed,
/// start node)`. Only the edges a path actually traverses are read and kept, so
/// it never materializes a predicate's **whole** adjacency — a full predicate
/// scan over a planet-scale graph (e.g. every `geo:asWKT`) buries a 32-bit WASM
/// heap and was the cause of an intermittent `RuntimeError: unreachable`.
type AdjCache = std::collections::HashMap<(String, bool, u32), Vec<u32>>;

/// Successor nodes of `start` along a single predicate — a **targeted** routed
/// read of just this node's edges (forward: `start` as subject; reverse: `start`
/// as object), mapped back into the unified node space. Cached per start node.
fn successors(
    ctx: &Ctx,
    index: &GraphIndex,
    cache: &mut AdjCache,
    pred: &str,
    rev: bool,
    start: u32,
) -> Vec<u32> {
    let key = (pred.to_string(), rev, start);
    if let Some(v) = cache.get(&key) {
        return v.clone();
    }
    let dict = ctx.rete.dictionary();
    let succ: Vec<u32> = match dict.predicate_id(pred) {
        // forward: out-edges of `start` (one tile, not the predicate's whole index)
        Some(pid) if !rev => match dict.node_as_subject_id(start) {
            Some(sid) => index
                .match_pattern((Some(sid), Some(pid), None))
                .into_iter()
                .map(|(_s, _p, o)| dict.object_node(o))
                .collect(),
            None => Vec::new(),
        },
        // reverse: in-edges of `start` (it appears as the object)
        Some(pid) => match dict.node_as_object_id(start) {
            Some(oid) => index
                .match_pattern((None, Some(pid), Some(oid)))
                .into_iter()
                .map(|(s, _p, _o)| dict.subject_node(s))
                .collect(),
            None => Vec::new(),
        },
        None => Vec::new(),
    };
    cache.insert(key, succ.clone());
    succ
}

/// Successors of `start` over a **negated property set**: any predicate not in
/// `set`, in direction `rev`. The non-excluded adjacency is built once (over
/// every triple) and cached under a synthetic key.
fn negated_successors(
    ctx: &Ctx,
    index: &GraphIndex,
    cache: &mut AdjCache,
    set: &[String],
    rev: bool,
    start: u32,
) -> Vec<u32> {
    let key = (format!("!\u{1}{}", set.join("\u{1}")), rev, start);
    if let Some(v) = cache.get(&key) {
        return v.clone();
    }
    let dict = ctx.rete.dictionary();
    let excluded: std::collections::HashSet<u32> =
        set.iter().filter_map(|p| dict.predicate_id(p)).collect();
    // Read only `start`'s own edges (any predicate), then drop the excluded ones —
    // never a scan of every triple in the graph.
    let succ: Vec<u32> = if !rev {
        match dict.node_as_subject_id(start) {
            Some(sid) => index
                .match_pattern((Some(sid), None, None))
                .into_iter()
                .filter(|(_s, p, _o)| !excluded.contains(p))
                .map(|(_s, _p, o)| dict.object_node(o))
                .collect(),
            None => Vec::new(),
        }
    } else {
        match dict.node_as_object_id(start) {
            Some(oid) => index
                .match_pattern((None, None, Some(oid)))
                .into_iter()
                .filter(|(_s, p, _o)| !excluded.contains(p))
                .map(|(s, _p, _o)| dict.subject_node(s))
                .collect(),
            None => Vec::new(),
        }
    };
    cache.insert(key, succ.clone());
    succ
}

/// Nodes reachable from `start` along `ast` — forward from the start node, so a
/// bound endpoint never triggers a global closure. Integer node space.
fn reach_from(
    ctx: &Ctx,
    index: &GraphIndex,
    ast: &PathAst,
    start: u32,
    cache: &mut AdjCache,
) -> std::collections::BTreeSet<u32> {
    use std::collections::BTreeSet;
    match ast {
        PathAst::Pred(p, rev) => successors(ctx, index, cache, p, *rev, start)
            .into_iter()
            .collect(),
        PathAst::NegatedSet(set, rev) => negated_successors(ctx, index, cache, set, *rev, start)
            .into_iter()
            .collect(),
        PathAst::Alt(a, b) => {
            let mut r = reach_from(ctx, index, a, start, cache);
            r.extend(reach_from(ctx, index, b, start, cache));
            r
        }
        PathAst::Seq(a, b) => {
            let mids = reach_from(ctx, index, a, start, cache);
            let mut out = BTreeSet::new();
            for m in &mids {
                out.extend(reach_from(ctx, index, b, *m, cache));
            }
            out
        }
        PathAst::Rep(inner, rep) => match rep {
            Rep::One => reach_from(ctx, index, inner, start, cache),
            Rep::ZeroOrOne => {
                let mut r = reach_from(ctx, index, inner, start, cache);
                r.insert(start);
                r
            }
            Rep::OneOrMore | Rep::ZeroOrMore => {
                let mut visited = BTreeSet::new();
                let mut stack: Vec<u32> = reach_from(ctx, index, inner, start, cache)
                    .into_iter()
                    .collect();
                while let Some(n) = stack.pop() {
                    if visited.insert(n) {
                        for m in reach_from(ctx, index, inner, n, cache) {
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

/// Build a solution row binding the subject/object endpoints (consistently for
/// a repeated variable like `?x <path> ?x`).
fn bind_pair(ctx: &Ctx, subj: &PatternTerm, obj: &PatternTerm, a: u32, b: u32) -> Option<Row> {
    let mut row = ctx.slots.empty_row();
    for (term, node) in [(subj, a), (obj, b)] {
        if let PatternTerm::Var(v) = term {
            let slot = ctx.slots.slot(v)?;
            match row[slot] {
                Some(Val::Id(existing)) if existing != node as i64 => return None,
                Some(_) => {}
                None => row[slot] = Some(Val::Id(node as i64)),
            }
        }
    }
    Some(row)
}

/// Can `ast` relate a node to *itself* via a zero-length path (`*`/`?`)? Used so
/// a constant endpoint that isn't even in the graph still yields the identity
/// solution (`:x :p* ?o` ⇒ `?o = :x`, even on an empty dataset).
fn matches_zero_length(ast: &PathAst) -> bool {
    match ast {
        PathAst::Rep(_, Rep::ZeroOrMore | Rep::ZeroOrOne) => true,
        PathAst::Rep(inner, Rep::One) => matches_zero_length(inner),
        PathAst::Rep(_, Rep::OneOrMore) => false,
        PathAst::Seq(a, b) => matches_zero_length(a) && matches_zero_length(b),
        PathAst::Alt(a, b) => matches_zero_length(a) || matches_zero_length(b),
        PathAst::Pred(..) | PathAst::NegatedSet(..) => false,
    }
}

/// The zero-length self-solution for a constant `term` not present in the graph:
/// both endpoints must equal `term`. Binds an endpoint variable to the term;
/// a constant other endpoint must match it.
fn bind_self_const(ctx: &Ctx, subj: &PatternTerm, obj: &PatternTerm, term: &str) -> Option<Row> {
    let v = ctx.resolver.canon_term(term);
    let mut row = ctx.slots.empty_row();
    for pt in [subj, obj] {
        match pt {
            PatternTerm::Var(name) => {
                let slot = ctx.slots.slot(name)?;
                match &row[slot] {
                    Some(existing) if *existing != v => return None,
                    _ => row[slot] = Some(v.clone()),
                }
            }
            PatternTerm::Const(c) => {
                if ctx.resolver.canon_term(c) != v {
                    return None;
                }
            }
        }
    }
    Some(row)
}

/// Evaluate a property path to solution rows. Traversal runs on integer node
/// IDs. A constant endpoint is pushed down so unbounded paths don't enumerate
/// the whole graph.
pub(super) fn eval_path(
    ctx: &Ctx,
    index: &GraphIndex,
    subj: &PatternTerm,
    ast: &PathAst,
    obj: &PatternTerm,
) -> Vec<Row> {
    let dict = ctx.rete.dictionary();
    let mut cache = AdjCache::new();
    let mut out = Vec::new();

    match (subj, obj) {
        // Bound subject: forward search from its node.
        (PatternTerm::Const(s), _) => {
            let Some(sn) = dict.node_of_term(s) else {
                // Absent from the graph: only a zero-length self-match is possible.
                if matches_zero_length(ast) {
                    if let Some(b) = bind_self_const(ctx, subj, obj, s) {
                        out.push(b);
                    }
                }
                return out;
            };
            let obj_node = match obj {
                PatternTerm::Const(o) => Some(dict.node_of_term(o)),
                _ => None,
            };
            for e in reach_from(ctx, index, ast, sn, &mut cache) {
                if let Some(on) = obj_node {
                    if on != Some(e) {
                        continue;
                    }
                }
                if let Some(b) = bind_pair(ctx, subj, obj, sn, e) {
                    out.push(b);
                }
            }
        }
        // Bound object only: search backward along the reversed path.
        (PatternTerm::Var(_), PatternTerm::Const(o)) => {
            let Some(on) = dict.node_of_term(o) else {
                if matches_zero_length(ast) {
                    if let Some(b) = bind_self_const(ctx, subj, obj, o) {
                        out.push(b);
                    }
                }
                return out;
            };
            let rev = reverse(ast.clone());
            for s in reach_from(ctx, index, &rev, on, &mut cache) {
                if let Some(b) = bind_pair(ctx, subj, obj, s, on) {
                    out.push(b);
                }
            }
        }
        // Both unbound: enumerate from every node (inherently expensive).
        (PatternTerm::Var(_), PatternTerm::Var(_)) => {
            for start in 0..dict.node_count() {
                for e in reach_from(ctx, index, ast, start, &mut cache) {
                    if let Some(b) = bind_pair(ctx, subj, obj, start, e) {
                        out.push(b);
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{write_file, Rete};
    use crate::index::GraphIndexBuilder;
    use crate::row::Slots;

    fn fixture() -> Rete {
        let triples = [
            ("<A>", "<p>", "<B>"),
            ("<B>", "<p>", "<C>"),
            ("<C>", "<p>", "<A>"),
            ("<A>", "<q>", "<C>"),
            ("<D>", "<r>", "<A>"),
            ("<A>", "<p>", "<object-only>"),
        ];
        let mut builder = DictionaryBuilder::new();
        for (s, p, o) in triples {
            builder.observe(s, p, o);
        }
        let dict = builder.build();
        let mut index = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            index.push(dict.encode(s, p, o).unwrap());
        }
        Rete::open(&write_file(&dict, &index.build(), false, &[], 0)).unwrap()
    }

    fn context(rete: &Rete) -> Ctx<'_> {
        let mut slots = Slots::new();
        slots.add("x");
        slots.add("y");
        Ctx::new(rete, slots)
    }

    fn pred(name: &str) -> PathAst {
        PathAst::Pred(name.to_string(), false)
    }

    #[test]
    fn reach_handles_predicate_reverse_negation_composition_and_repetition() {
        let rete = fixture();
        let ctx = context(&rete);
        let index = rete.default_index();
        let a = rete.dictionary().node_of_term("<A>").unwrap();
        let b = rete.dictionary().node_of_term("<B>").unwrap();
        let c = rete.dictionary().node_of_term("<C>").unwrap();
        let object_only = rete.dictionary().node_of_term("<object-only>").unwrap();
        let mut cache = AdjCache::new();

        let first = successors(&ctx, index, &mut cache, "<p>", false, a);
        assert!(first.contains(&b) && first.contains(&object_only));
        assert_eq!(successors(&ctx, index, &mut cache, "<p>", false, a), first);
        assert!(successors(&ctx, index, &mut cache, "<missing>", false, a).is_empty());
        assert!(successors(&ctx, index, &mut cache, "<p>", false, object_only).is_empty());
        assert!(successors(&ctx, index, &mut cache, "<p>", true, a).contains(&c));
        assert_eq!(successors(&ctx, index, &mut cache, "<p>", true, b), vec![a]);

        let not_p = negated_successors(&ctx, index, &mut cache, &["<p>".into()], false, a);
        assert_eq!(not_p, vec![c]);
        assert_eq!(
            negated_successors(&ctx, index, &mut cache, &["<p>".into()], false, a),
            not_p
        );
        assert!(
            negated_successors(&ctx, index, &mut cache, &["<p>".into()], true, a)
                .iter()
                .any(|n| *n == rete.dictionary().node_of_term("<D>").unwrap())
        );
        assert!(negated_successors(&ctx, index, &mut cache, &[], false, object_only).is_empty());

        assert!(reach_from(
            &ctx,
            index,
            &PathAst::Pred("<p>".into(), false),
            a,
            &mut cache
        )
        .contains(&b));
        assert!(reach_from(
            &ctx,
            index,
            &PathAst::NegatedSet(vec!["<p>".into()], false),
            a,
            &mut cache
        )
        .contains(&c));
        assert!(reach_from(
            &ctx,
            index,
            &PathAst::Alt(Box::new(pred("<p>")), Box::new(pred("<q>"))),
            a,
            &mut cache
        )
        .contains(&c));
        assert!(reach_from(
            &ctx,
            index,
            &PathAst::Seq(Box::new(pred("<q>")), Box::new(pred("<p>"))),
            a,
            &mut cache
        )
        .contains(&a));
        assert!(reach_from(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::One),
            a,
            &mut cache
        )
        .contains(&b));
        assert!(reach_from(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::ZeroOrOne),
            a,
            &mut cache
        )
        .contains(&a));
        let plus = reach_from(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::OneOrMore),
            a,
            &mut cache,
        );
        assert!(plus.contains(&a) && plus.contains(&b) && plus.contains(&c));
        let star = reach_from(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<missing>")), Rep::ZeroOrMore),
            a,
            &mut cache,
        );
        assert_eq!(star.into_iter().collect::<Vec<_>>(), vec![a]);
    }

    #[test]
    fn binding_and_zero_length_rules_cover_repeated_and_absent_terms() {
        let rete = fixture();
        let ctx = context(&rete);
        let a = rete.dictionary().node_of_term("<A>").unwrap();
        let b = rete.dictionary().node_of_term("<B>").unwrap();
        let x = PatternTerm::Var("x".into());
        let y = PatternTerm::Var("y".into());
        assert!(bind_pair(&ctx, &x, &y, a, b).is_some());
        assert!(bind_pair(&ctx, &x, &x, a, b).is_none());
        assert!(bind_pair(&ctx, &PatternTerm::Var("missing".into()), &y, a, b).is_none());
        assert!(bind_pair(
            &ctx,
            &PatternTerm::Const("<A>".into()),
            &PatternTerm::Const("<B>".into()),
            a,
            b
        )
        .is_some());

        assert!(matches_zero_length(&PathAst::Rep(
            Box::new(pred("<p>")),
            Rep::ZeroOrMore
        )));
        assert!(matches_zero_length(&PathAst::Rep(
            Box::new(pred("<p>")),
            Rep::ZeroOrOne
        )));
        assert!(!matches_zero_length(&PathAst::Rep(
            Box::new(pred("<p>")),
            Rep::OneOrMore
        )));
        assert!(matches_zero_length(&PathAst::Rep(
            Box::new(PathAst::Rep(Box::new(pred("<p>")), Rep::ZeroOrOne)),
            Rep::One
        )));
        assert!(matches_zero_length(&PathAst::Seq(
            Box::new(PathAst::Rep(Box::new(pred("<p>")), Rep::ZeroOrOne)),
            Box::new(PathAst::Rep(Box::new(pred("<q>")), Rep::ZeroOrMore))
        )));
        assert!(matches_zero_length(&PathAst::Alt(
            Box::new(pred("<p>")),
            Box::new(PathAst::Rep(Box::new(pred("<q>")), Rep::ZeroOrMore))
        )));
        assert!(!matches_zero_length(&pred("<p>")));
        assert!(!matches_zero_length(&PathAst::NegatedSet(vec![], false)));

        assert!(
            bind_self_const(&ctx, &PatternTerm::Const("<absent>".into()), &x, "<absent>").is_some()
        );
        assert!(bind_self_const(&ctx, &x, &x, "<absent>").is_some());
        assert!(
            bind_self_const(&ctx, &PatternTerm::Const("<other>".into()), &x, "<absent>").is_none()
        );
        assert!(
            bind_self_const(&ctx, &PatternTerm::Var("missing".into()), &x, "<absent>").is_none()
        );
    }

    #[test]
    fn eval_path_covers_bound_unbound_reverse_and_absent_endpoint_cases() {
        let rete = fixture();
        let ctx = context(&rete);
        let index = rete.default_index();
        let x = PatternTerm::Var("x".into());
        let y = PatternTerm::Var("y".into());
        let p = pred("<p>");

        let forward = eval_path(&ctx, index, &PatternTerm::Const("<A>".into()), &p, &y);
        assert_eq!(forward.len(), 2);
        let exact = eval_path(
            &ctx,
            index,
            &PatternTerm::Const("<A>".into()),
            &p,
            &PatternTerm::Const("<B>".into()),
        );
        assert_eq!(exact.len(), 1);
        let no_exact = eval_path(
            &ctx,
            index,
            &PatternTerm::Const("<A>".into()),
            &p,
            &PatternTerm::Const("<D>".into()),
        );
        assert!(no_exact.is_empty());
        assert_eq!(
            eval_path(&ctx, index, &x, &p, &PatternTerm::Const("<B>".into())).len(),
            1
        );
        assert!(!eval_path(&ctx, index, &x, &p, &y).is_empty());
        assert!(eval_path(&ctx, index, &x, &p, &PatternTerm::Const("<absent>".into())).is_empty());

        let zero = PathAst::Rep(Box::new(p.clone()), Rep::ZeroOrMore);
        assert_eq!(
            eval_path(
                &ctx,
                index,
                &PatternTerm::Const("<absent>".into()),
                &zero,
                &y
            )
            .len(),
            1
        );
        assert_eq!(
            eval_path(
                &ctx,
                index,
                &x,
                &zero,
                &PatternTerm::Const("<absent>".into())
            )
            .len(),
            1
        );
        assert!(eval_path(&ctx, index, &PatternTerm::Const("<absent>".into()), &p, &y).is_empty());
    }
}
