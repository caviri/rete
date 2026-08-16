//! Property-path evaluation (SPEC.md §8): `subject <path> object` as a binary
//! relation over the graph's nodes. Traversal runs on integer node IDs and
//! solutions are emitted as integer slot rows — terms are never resolved here
//! (the engine materializes only at projection). A constant endpoint is pushed
//! down so an unbounded path never enumerates the whole graph.

use crate::bgp::PatternTerm;
use crate::dictionary::Dictionary;
use crate::index::{GraphIndex, IndexPermutation};
use crate::row::{Ctx, Row, Val};
use std::collections::{BTreeSet, HashMap, HashSet};

use super::{PathAst, Rep};

/// Per-start-node successor cache keyed by `(predicate-or-negated-set, reversed,
/// start node)`. Only the edges a path actually traverses are read and kept, so
/// it never materializes a predicate's **whole** adjacency — a full predicate
/// scan over a planet-scale graph (e.g. every `geo:asWKT`) buries a 32-bit WASM
/// heap and was the cause of an intermittent `RuntimeError: unreachable`.
type AdjCache = HashMap<(u32, u32), Vec<u32>>;

enum ResolvedPathAst {
    Pred {
        key: u32,
        predicate: Option<u32>,
        reversed: bool,
    },
    NegatedSet {
        key: u32,
        excluded: HashSet<u32>,
        reversed: bool,
    },
    Rep(Box<ResolvedPathAst>, Rep),
    Seq(Box<ResolvedPathAst>, Box<ResolvedPathAst>),
    Alt(Box<ResolvedPathAst>, Box<ResolvedPathAst>),
}

impl ResolvedPathAst {
    /// Push a reverse through the already-resolved tree without repeating any
    /// dictionary lookup. Sequence order reverses under relational inversion.
    fn reversed(&self) -> Self {
        match self {
            Self::Pred {
                key,
                predicate,
                reversed,
            } => Self::Pred {
                key: *key,
                predicate: *predicate,
                reversed: !reversed,
            },
            Self::NegatedSet {
                key,
                excluded,
                reversed,
            } => Self::NegatedSet {
                key: *key,
                excluded: excluded.clone(),
                reversed: !reversed,
            },
            Self::Rep(inner, rep) => Self::Rep(Box::new(inner.reversed()), *rep),
            Self::Seq(a, b) => Self::Seq(Box::new(b.reversed()), Box::new(a.reversed())),
            Self::Alt(a, b) => Self::Alt(Box::new(a.reversed()), Box::new(b.reversed())),
        }
    }
}

struct PathResolver<'a> {
    dict: &'a Dictionary,
    ids: HashMap<String, Option<u32>>,
    next_key: u32,
    #[cfg(test)]
    predicate_resolutions: u64,
}

impl<'a> PathResolver<'a> {
    fn new(dict: &'a Dictionary) -> Self {
        Self {
            dict,
            ids: HashMap::new(),
            next_key: 0,
            #[cfg(test)]
            predicate_resolutions: 0,
        }
    }

    fn key(&mut self) -> u32 {
        let key = self.next_key;
        self.next_key = self.next_key.saturating_add(1);
        key
    }

    fn predicate(&mut self, lexical: &str) -> Option<u32> {
        if let Some(id) = self.ids.get(lexical) {
            return *id;
        }
        crate::read_path_metrics::record_predicate_resolution();
        let id = self.dict.predicate_id(lexical);
        self.ids.insert(lexical.to_owned(), id);
        #[cfg(test)]
        {
            self.predicate_resolutions += 1;
        }
        id
    }

    fn resolve(&mut self, ast: &PathAst) -> ResolvedPathAst {
        match ast {
            PathAst::Pred(predicate, reversed) => ResolvedPathAst::Pred {
                key: self.key(),
                predicate: self.predicate(predicate),
                reversed: *reversed,
            },
            PathAst::NegatedSet(predicates, reversed) => {
                let excluded = predicates
                    .iter()
                    .filter_map(|predicate| self.predicate(predicate))
                    .collect();
                ResolvedPathAst::NegatedSet {
                    key: self.key(),
                    excluded,
                    reversed: *reversed,
                }
            }
            PathAst::Rep(inner, rep) => ResolvedPathAst::Rep(Box::new(self.resolve(inner)), *rep),
            PathAst::Seq(a, b) => {
                ResolvedPathAst::Seq(Box::new(self.resolve(a)), Box::new(self.resolve(b)))
            }
            PathAst::Alt(a, b) => {
                ResolvedPathAst::Alt(Box::new(self.resolve(a)), Box::new(self.resolve(b)))
            }
        }
    }
}

pub(super) struct ResolvedPath {
    ast: ResolvedPathAst,
    zero_length: bool,
    #[cfg(test)]
    predicate_resolutions: u64,
}

impl ResolvedPath {
    pub(super) fn new(dict: &Dictionary, ast: &PathAst) -> Self {
        let mut resolver = PathResolver::new(dict);
        let resolved_ast = resolver.resolve(ast);
        Self {
            ast: resolved_ast,
            zero_length: matches_zero_length(ast),
            #[cfg(test)]
            predicate_resolutions: resolver.predicate_resolutions,
        }
    }

    #[cfg(test)]
    fn predicate_resolutions(&self) -> u64 {
        self.predicate_resolutions
    }
}

/// Successor nodes of `start` along a single predicate — a **targeted** routed
/// read of just this node's edges (forward: `start` as subject; reverse: `start`
/// as object), mapped back into the unified node space. Cached per start node.
fn successors(
    ctx: &Ctx,
    index: &GraphIndex,
    cache: &mut AdjCache,
    key: u32,
    predicate: Option<u32>,
    reversed: bool,
    start: u32,
) -> Vec<u32> {
    if let Some(v) = cache.get(&(key, start)) {
        return v.clone();
    }
    crate::read_path_metrics::record_path_probe();
    let dict = ctx.rete.dictionary();
    let succ: Vec<u32> = match predicate {
        Some(pid) if !reversed => dict
            .node_as_subject_id(start)
            .map(|sid| {
                index
                    .scan_prefix2(IndexPermutation::Spo, sid, pid)
                    .map(|oid| dict.object_node(oid))
                    .collect()
            })
            .unwrap_or_default(),
        Some(pid) => dict
            .node_as_object_id(start)
            .map(|oid| {
                index
                    .scan_prefix2(IndexPermutation::Ops, oid, pid)
                    .map(|sid| dict.subject_node(sid))
                    .collect()
            })
            .unwrap_or_default(),
        None => Vec::new(),
    };
    cache.insert((key, start), succ.clone());
    succ
}

/// Successors of `start` over a **negated property set**: any predicate not in
/// `set`, in direction `rev`. The non-excluded adjacency is built once (over
/// every triple) and cached under a synthetic key.
fn negated_successors(
    ctx: &Ctx,
    index: &GraphIndex,
    cache: &mut AdjCache,
    key: u32,
    excluded: &HashSet<u32>,
    reversed: bool,
    start: u32,
) -> Vec<u32> {
    if let Some(v) = cache.get(&(key, start)) {
        return v.clone();
    }
    crate::read_path_metrics::record_path_probe();
    let dict = ctx.rete.dictionary();
    // Read only `start`'s own edges (any predicate), then drop the excluded ones —
    // never a scan of every triple in the graph.
    let succ: Vec<u32> = if !reversed {
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
    cache.insert((key, start), succ.clone());
    succ
}

/// Nodes reachable from `start` along `ast` — forward from the start node, so a
/// bound endpoint never triggers a global closure. Integer node space.
fn reach_from(
    ctx: &Ctx,
    index: &GraphIndex,
    ast: &ResolvedPathAst,
    start: u32,
    cache: &mut AdjCache,
) -> BTreeSet<u32> {
    match ast {
        ResolvedPathAst::Pred {
            key,
            predicate,
            reversed,
        } => successors(ctx, index, cache, *key, *predicate, *reversed, start)
            .into_iter()
            .collect(),
        ResolvedPathAst::NegatedSet {
            key,
            excluded,
            reversed,
        } => negated_successors(ctx, index, cache, *key, excluded, *reversed, start)
            .into_iter()
            .collect(),
        ResolvedPathAst::Alt(a, b) => {
            let mut r = reach_from(ctx, index, a, start, cache);
            r.extend(reach_from(ctx, index, b, start, cache));
            r
        }
        ResolvedPathAst::Seq(a, b) => {
            let mids = reach_from(ctx, index, a, start, cache);
            let mut out = BTreeSet::new();
            for m in &mids {
                out.extend(reach_from(ctx, index, b, *m, cache));
            }
            out
        }
        ResolvedPathAst::Rep(inner, rep) => match rep {
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
    let resolved = ResolvedPath::new(ctx.rete.dictionary(), ast);
    eval_resolved_path(ctx, index, subj, &resolved, obj)
}

/// Evaluate an already-resolved path for one set of endpoints. Correlated joins
/// reuse the same value for every bound input row, avoiding repeated dictionary
/// resolution and resolved-tree allocation.
pub(super) fn eval_resolved_path(
    ctx: &Ctx,
    index: &GraphIndex,
    subj: &PatternTerm,
    resolved: &ResolvedPath,
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
                if resolved.zero_length {
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
            for e in reach_from(ctx, index, &resolved.ast, sn, &mut cache) {
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
                if resolved.zero_length {
                    if let Some(b) = bind_self_const(ctx, subj, obj, o) {
                        out.push(b);
                    }
                }
                return out;
            };
            let rev = resolved.ast.reversed();
            for s in reach_from(ctx, index, &rev, on, &mut cache) {
                if let Some(b) = bind_pair(ctx, subj, obj, s, on) {
                    out.push(b);
                }
            }
        }
        // Both unbound: enumerate from every node (inherently expensive).
        (PatternTerm::Var(_), PatternTerm::Var(_)) => {
            for start in 0..dict.node_count() {
                for e in reach_from(ctx, index, &resolved.ast, start, &mut cache) {
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

    fn reachable(ctx: &Ctx, index: &GraphIndex, ast: &PathAst, start: u32) -> BTreeSet<u32> {
        let resolved = ResolvedPath::new(ctx.rete.dictionary(), ast);
        reach_from(ctx, index, &resolved.ast, start, &mut AdjCache::new())
    }

    #[test]
    fn resolved_path_resolves_each_distinct_predicate_once() {
        let rete = fixture();
        let ast = PathAst::Alt(
            Box::new(pred("<p>")),
            Box::new(PathAst::Seq(Box::new(pred("<p>")), Box::new(pred("<q>")))),
        );

        let resolved = ResolvedPath::new(rete.dictionary(), &ast);
        assert_eq!(resolved.predicate_resolutions(), 2);
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
        let p = rete.dictionary().predicate_id("<p>");
        let mut cache = AdjCache::new();

        let first = successors(&ctx, index, &mut cache, 0, p, false, a);
        assert!(first.contains(&b) && first.contains(&object_only));
        assert_eq!(successors(&ctx, index, &mut cache, 0, p, false, a), first);
        assert!(successors(&ctx, index, &mut cache, 1, None, false, a).is_empty());
        assert!(successors(&ctx, index, &mut cache, 0, p, false, object_only).is_empty());
        assert!(successors(&ctx, index, &mut cache, 2, p, true, a).contains(&c));
        assert_eq!(successors(&ctx, index, &mut cache, 2, p, true, b), vec![a]);

        let excluded_p = HashSet::from([p.unwrap()]);
        let not_p = negated_successors(&ctx, index, &mut cache, 3, &excluded_p, false, a);
        assert_eq!(not_p, vec![c]);
        assert_eq!(
            negated_successors(&ctx, index, &mut cache, 3, &excluded_p, false, a),
            not_p
        );
        assert!(
            negated_successors(&ctx, index, &mut cache, 4, &excluded_p, true, a)
                .iter()
                .any(|n| *n == rete.dictionary().node_of_term("<D>").unwrap())
        );
        assert!(negated_successors(
            &ctx,
            index,
            &mut cache,
            5,
            &HashSet::new(),
            false,
            object_only
        )
        .is_empty());

        assert!(reachable(&ctx, index, &PathAst::Pred("<p>".into(), false), a).contains(&b));
        assert!(reachable(
            &ctx,
            index,
            &PathAst::NegatedSet(vec!["<p>".into()], false),
            a
        )
        .contains(&c));
        assert!(reachable(
            &ctx,
            index,
            &PathAst::Alt(Box::new(pred("<p>")), Box::new(pred("<q>"))),
            a
        )
        .contains(&c));
        assert!(reachable(
            &ctx,
            index,
            &PathAst::Seq(Box::new(pred("<q>")), Box::new(pred("<p>"))),
            a
        )
        .contains(&a));
        assert!(reachable(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::One),
            a
        )
        .contains(&b));
        assert!(reachable(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::ZeroOrOne),
            a
        )
        .contains(&a));
        let plus = reachable(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<p>")), Rep::OneOrMore),
            a,
        );
        assert!(plus.contains(&a) && plus.contains(&b) && plus.contains(&c));
        let star = reachable(
            &ctx,
            index,
            &PathAst::Rep(Box::new(pred("<missing>")), Rep::ZeroOrMore),
            a,
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
