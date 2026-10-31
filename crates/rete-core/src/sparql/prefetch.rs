//! Resolve a query's constant terms in one batched dictionary read.
//!
//! Every constant a query names — a pattern's IRI or literal, a predicate, a
//! `VALUES` row, a path's predicates, a graph name on a quad-index file — is
//! looked up in the dictionary before anything else can be read: its id routes
//! every tile fetch that follows. On a lazy or ranged open each lookup faults
//! the dictionary chunk it routes to, one dependent read per term and section
//! (a literal object probes three sections). Here the whole query is walked
//! once, every chunk those lookups will read is routed on the resident chunk
//! directories, and the missing ones are fetched in one batch — one round trip
//! remotely — so the lookups that follow are cache hits. It only warms the
//! cache: no answer depends on it, and on a resident open it does nothing.

use super::*;
use crate::bgp::PatternTerm;

/// The most constants one query prefetches. Past it (a long `VALUES` list) the
/// rest are looked up as before; the batch itself is bounded too, by the
/// dictionary ([`crate::dictionary::Dictionary::prefetch_lookups`]).
const MAX_PREFETCH_TERMS: usize = 512;

/// The constants of a query, split by how they are looked up.
#[derive(Default)]
struct Terms<'a> {
    nodes: Vec<&'a str>,
    preds: Vec<&'a str>,
    /// Graph names, looked up as nodes only where a quad index stores them in
    /// the dictionary.
    graphs: Vec<&'a str>,
}

impl<'a> Terms<'a> {
    fn full(&self) -> bool {
        self.nodes.len() + self.preds.len() + self.graphs.len() >= MAX_PREFETCH_TERMS
    }

    fn node(&mut self, t: &'a PatternTerm) {
        if let PatternTerm::Const(c) = t {
            self.nodes.push(c.as_str());
        }
    }

    fn pred(&mut self, t: &'a PatternTerm) {
        if let PatternTerm::Const(c) = t {
            self.preds.push(c.as_str());
        }
    }

    fn select(&mut self, sel: &'a Select) {
        self.graphs.extend(sel.from.iter().map(String::as_str));
        if let Some(named) = &sel.from_named {
            self.graphs.extend(named.iter().map(String::as_str));
        }
        self.plan(&sel.plan);
    }

    fn plan(&mut self, plan: &'a Plan) {
        if self.full() {
            return;
        }
        match plan {
            Plan::Bgp(patterns) => {
                for p in patterns {
                    self.node(&p.s);
                    self.pred(&p.p);
                    self.node(&p.o);
                }
            }
            Plan::DistinctUnion { sources, .. } => {
                for p in sources {
                    self.node(&p.s);
                    self.pred(&p.p);
                    self.node(&p.o);
                }
            }
            Plan::Join(l, r) | Plan::Union(l, r) | Plan::Minus(l, r) => {
                self.plan(l);
                self.plan(r);
            }
            Plan::LeftJoin(l, r, cond) => {
                self.plan(l);
                self.plan(r);
                if let Some(e) = cond {
                    self.expr(e);
                }
            }
            Plan::Filter(e, inner) | Plan::Extend(_, e, inner) => {
                self.expr(e);
                self.plan(inner);
            }
            Plan::Path(s, path, o) => {
                self.node(s);
                self.node(o);
                self.path(path);
            }
            // A VALUES term is canonicalized as a node, else as a predicate.
            Plan::Values(_, rows) => {
                for t in rows.iter().flatten().flatten() {
                    if self.full() {
                        return;
                    }
                    self.nodes.push(t.as_str());
                    self.preds.push(t.as_str());
                }
            }
            Plan::Graph(target, inner) => {
                if let GraphTarget::Named(iri) = target {
                    self.graphs.push(iri.as_str());
                }
                self.plan(inner);
            }
            Plan::Subquery(sub) => self.select(sub),
            Plan::Service { .. } => {}
        }
    }

    fn path(&mut self, path: &'a PathAst) {
        match path {
            PathAst::Pred(p, _) => self.preds.push(p.as_str()),
            PathAst::NegatedSet(ps, _) => self.preds.extend(ps.iter().map(String::as_str)),
            PathAst::Rep(inner, _) => self.path(inner),
            PathAst::Seq(a, b) | PathAst::Alt(a, b) => {
                self.path(a);
                self.path(b);
            }
        }
    }

    /// Only the patterns inside `EXISTS` are looked up; a FILTER's own
    /// constants are compared as strings.
    fn expr(&mut self, e: &'a FExpr) {
        match e {
            FExpr::Exists(plan) => self.plan(plan),
            FExpr::Not(a) => self.expr(a),
            FExpr::And(a, b) | FExpr::Or(a, b) => {
                self.expr(a);
                self.expr(b);
            }
            FExpr::If(c, t, f) => {
                self.expr(c);
                self.expr(t);
                self.expr(f);
            }
            _ => {}
        }
    }
}

/// Fault, in one batch, every dictionary chunk the constant lookups of `sel`
/// will read (see the module docs). A no-op on a resident dictionary.
pub(super) fn prefetch_constants(rete: &Rete, sel: &Select) {
    let dict = rete.dictionary();
    if !dict.faults_chunks() {
        return;
    }
    let mut terms = Terms::default();
    terms.select(sel);
    if rete.header().has_quad_index() {
        terms.nodes.extend(terms.graphs.iter().copied());
    }
    let mut nodes = terms.nodes;
    nodes.sort_unstable();
    nodes.dedup();
    let mut preds = terms.preds;
    // The join planner looks `rdf:type` up for every multi-pattern BGP (to
    // recognise a class enumeration), named in the query or not.
    if !preds.is_empty() {
        preds.push(crate::file::RDF_TYPE);
    }
    preds.sort_unstable();
    preds.dedup();
    dict.prefetch_lookups(nodes, preds);
}
