//! OWL 2 QL — lazy reasoning by query REWRITING (design in `dev/owl-ql.md`).
//!
//! Opt-in. Instead of materializing entailments into the (huge, remote) ABox,
//! rewrite the *query* so that evaluating it over the RAW data yields the certain
//! answers. A remote file becomes ontology-aware with no rebuild, and only the
//! bytes the rewritten query touches are fetched.
//!
//! **Stage 1a — class hierarchy (`rdfs:subClassOf`).** A class atom `?x a C`
//! (C a constant) is entailed by any `?x a D` with `D rdfs:subClassOf* C`.
//! Rewrite it to exactly that: `?x a ?c . ?c rdfs:subClassOf* C`, reusing the
//! goal-directed path engine, which walks the `subClassOf` edges lazily from the
//! bound endpoint `C`. Sound + complete for the `subClassOf` fragment, with no
//! TBox pre-read and no `UNION` blow-up. `subClassOf*` is reflexive, so a direct
//! `?x a C` is still matched.
//!
//! This runs as a post-lowering pass on the `Plan` tree, only when reasoning is
//! requested — the hot BGP matcher and every default query are untouched.

use super::*;
use crate::bgp::{PatternTerm, TriplePattern};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";

/// Rewrite a lowered plan for OWL 2 QL entailment (Stage 1a). Entry point.
pub(crate) fn reason_rewrite(plan: Plan) -> Plan {
    let mut counter: usize = 0;
    rewrite_plan(plan, &mut counter)
}

/// Walk the plan tree; transform each BGP, recurse structurally through every
/// other operator so reasoning reaches nested patterns (UNION/OPTIONAL/subquery…).
fn rewrite_plan(plan: Plan, counter: &mut usize) -> Plan {
    match plan {
        Plan::Bgp(patterns) => rewrite_bgp(patterns, counter),
        Plan::Join(a, b) => Plan::Join(
            Box::new(rewrite_plan(*a, counter)),
            Box::new(rewrite_plan(*b, counter)),
        ),
        Plan::Union(a, b) => Plan::Union(
            Box::new(rewrite_plan(*a, counter)),
            Box::new(rewrite_plan(*b, counter)),
        ),
        Plan::LeftJoin(a, b, cond) => Plan::LeftJoin(
            Box::new(rewrite_plan(*a, counter)),
            Box::new(rewrite_plan(*b, counter)),
            cond,
        ),
        Plan::Filter(f, p) => Plan::Filter(f, Box::new(rewrite_plan(*p, counter))),
        Plan::Extend(v, e, p) => Plan::Extend(v, e, Box::new(rewrite_plan(*p, counter))),
        Plan::Minus(a, b) => Plan::Minus(
            Box::new(rewrite_plan(*a, counter)),
            Box::new(rewrite_plan(*b, counter)),
        ),
        Plan::Graph(t, p) => Plan::Graph(t, Box::new(rewrite_plan(*p, counter))),
        Plan::Subquery(mut sel) => {
            sel.plan = rewrite_plan(sel.plan, counter);
            Plan::Subquery(sel)
        }
        // No BGP inside — a path, inline VALUES, or a SERVICE block (rewriting a
        // remote sub-query against our TBox would be unsound) — left unchanged.
        other => other,
    }
}

/// The constant class token `C` iff `tp` is a `?x rdf:type C` atom (concrete
/// class in object position); otherwise `None`.
fn type_atom_class(tp: &TriplePattern) -> Option<&str> {
    match (&tp.p, &tp.o) {
        (PatternTerm::Const(p), PatternTerm::Const(c)) if p == RDF_TYPE => Some(c.as_str()),
        _ => None,
    }
}

/// Rewrite one BGP: each concrete-class type atom `?x a C` becomes `?x a ?cN`
/// (kept in the residual BGP) joined with a `?cN rdfs:subClassOf* C` path. Every
/// other atom is left in the residual BGP untouched. A BGP with no such atom is
/// returned unchanged (the hot path).
fn rewrite_bgp(patterns: Vec<TriplePattern>, counter: &mut usize) -> Plan {
    if !patterns.iter().any(|tp| type_atom_class(tp).is_some()) {
        return Plan::Bgp(patterns);
    }
    let mut residual: Vec<TriplePattern> = Vec::new();
    let mut paths: Vec<Plan> = Vec::new();
    for tp in patterns {
        match type_atom_class(&tp) {
            Some(class) => {
                let class = class.to_string();
                *counter += 1;
                let cvar = format!("__qlc{}", counter);
                residual.push(TriplePattern {
                    s: tp.s,
                    p: PatternTerm::Const(RDF_TYPE.to_string()),
                    o: PatternTerm::Var(cvar.clone()),
                });
                paths.push(Plan::Path(
                    PatternTerm::Var(cvar),
                    PathAst::Rep(
                        Box::new(PathAst::Pred(SUBCLASS_OF.to_string(), false)),
                        Rep::ZeroOrMore,
                    ),
                    PatternTerm::Const(class),
                ));
            }
            None => residual.push(tp),
        }
    }
    let mut plan = Plan::Bgp(residual);
    for p in paths {
        plan = Plan::Join(Box::new(plan), Box::new(p));
    }
    plan
}
