//! OWL 2 QL — lazy reasoning by query REWRITING (design in `dev/owl-ql.md`).
//!
//! Opt-in. Instead of materializing entailments into the (huge, remote) ABox,
//! rewrite the *query* so that evaluating it over the RAW data yields the certain
//! answers. A remote file becomes ontology-aware with no rebuild, and only the
//! bytes the rewritten query touches are fetched.
//!
//! **Stage 1a — hierarchy (`rdfs:subClassOf`, `rdfs:subPropertyOf`).** Both are
//! exactly what rete's goal-directed property paths already do, so a hierarchy
//! atom is rewritten to walk the hierarchy edge over the RAW data:
//!
//! - class atom `?x a C`      → `?x a ?c . ?c rdfs:subClassOf* C`
//! - role atom  `?x P ?y`     → `?x ?p ?y . ?p rdfs:subPropertyOf* P`
//!
//! `*` is reflexive, so a direct match still counts. No `UNION` enumeration, no
//! blow-up. A tiny TBox read (the *objects* of the two hierarchy predicates)
//! GATES the rewrite: an atom whose class/property has no sub-terms is left
//! untouched, so reasoning is zero-overhead where it can add nothing.
//!
//! This runs as a post-lowering pass on the `Plan` tree, only when reasoning is
//! requested — the hot BGP matcher and every default query are untouched.

use super::*;
use crate::bgp::{PatternTerm, TriplePattern};
use crate::Rete;
use std::collections::HashSet;

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const SUBPROPERTY_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";

/// The slice of the ontology (TBox) that gates the Stage 1a rewrite: which
/// classes/properties actually have sub-terms, so we only rewrite atoms that can
/// gain rows. Read once per reasoned query with two small scans (the hierarchy
/// transitivity itself is handled at eval time by the `*` path — the gate only
/// needs the *direct* super-terms, and a transitive super is always the object
/// of some hierarchy edge, so this set is exact).
struct QlTbox {
    /// Classes that are the object of a `subClassOf` edge (⇒ have ≥1 subclass).
    superclasses: HashSet<String>,
    /// Properties that are the object of a `subPropertyOf` edge (⇒ have ≥1 subproperty).
    superprops: HashSet<String>,
}

impl QlTbox {
    fn read(rete: &Rete) -> Self {
        QlTbox {
            superclasses: objects_of(rete, SUBCLASS_OF),
            superprops: objects_of(rete, SUBPROPERTY_OF),
        }
    }
    fn is_empty(&self) -> bool {
        self.superclasses.is_empty() && self.superprops.is_empty()
    }
}

/// The distinct objects of triples with predicate `pred` (reasoning OFF, so no
/// recursion). Bounded — a hierarchy predicate is a small fraction of any graph.
fn objects_of(rete: &Rete, pred: &str) -> HashSet<String> {
    let q = format!("SELECT ?o WHERE {{ ?s {pred} ?o }}");
    match super::eval_sparql(rete, &q) {
        Ok((_, sols)) => sols.iter().filter_map(|b| b.get("o").cloned()).collect(),
        Err(_) => HashSet::new(),
    }
}

/// Rewrite a lowered plan for OWL 2 QL entailment (Stage 1a). Entry point.
pub(crate) fn reason_rewrite(plan: Plan, rete: &Rete) -> Plan {
    let tbox = QlTbox::read(rete);
    if tbox.is_empty() {
        return plan; // no hierarchy axioms — reasoning is a no-op
    }
    let mut counter: usize = 0;
    rewrite_plan(plan, &tbox, &mut counter)
}

/// Walk the plan tree; transform each BGP, recurse structurally through every
/// other operator so reasoning reaches nested patterns (UNION/OPTIONAL/subquery…).
fn rewrite_plan(plan: Plan, tbox: &QlTbox, counter: &mut usize) -> Plan {
    let recur = |p: Box<Plan>, counter: &mut usize| Box::new(rewrite_plan(*p, tbox, counter));
    match plan {
        Plan::Bgp(patterns) => rewrite_bgp(patterns, tbox, counter),
        Plan::Join(a, b) => Plan::Join(recur(a, counter), recur(b, counter)),
        Plan::Union(a, b) => Plan::Union(recur(a, counter), recur(b, counter)),
        Plan::LeftJoin(a, b, cond) => Plan::LeftJoin(recur(a, counter), recur(b, counter), cond),
        Plan::Filter(f, p) => Plan::Filter(f, recur(p, counter)),
        Plan::Extend(v, e, p) => Plan::Extend(v, e, recur(p, counter)),
        Plan::Minus(a, b) => Plan::Minus(recur(a, counter), recur(b, counter)),
        Plan::Graph(t, p) => Plan::Graph(t, recur(p, counter)),
        Plan::Subquery(mut sel) => {
            sel.plan = rewrite_plan(sel.plan, tbox, counter);
            Plan::Subquery(sel)
        }
        // No BGP inside — a path, inline VALUES, or a SERVICE block (rewriting a
        // remote sub-query against our TBox would be unsound) — left unchanged.
        other => other,
    }
}

/// The constant class token `C` iff `tp` is a `?x rdf:type C` atom.
fn type_atom_class(tp: &TriplePattern) -> Option<&str> {
    match (&tp.p, &tp.o) {
        (PatternTerm::Const(p), PatternTerm::Const(c)) if p == RDF_TYPE => Some(c.as_str()),
        _ => None,
    }
}

/// The constant predicate token `P` iff `tp` is a concrete-predicate role atom
/// (predicate is a constant, and not `rdf:type` — that is the class case).
fn role_atom_pred(tp: &TriplePattern) -> Option<&str> {
    match &tp.p {
        PatternTerm::Const(p) if p != RDF_TYPE => Some(p.as_str()),
        _ => None,
    }
}

/// A `subject <pred>* target` reflexive-transitive path plan.
fn star_path(subject: PatternTerm, pred: &str, target: PatternTerm) -> Plan {
    Plan::Path(
        subject,
        PathAst::Rep(
            Box::new(PathAst::Pred(pred.to_string(), false)),
            Rep::ZeroOrMore,
        ),
        target,
    )
}

/// Rewrite one BGP. A class atom `?x a C` where C has subclasses becomes
/// `?x a ?cN` (residual) joined with `?cN subClassOf* C`; a role atom `?x P ?y`
/// where P has subproperties becomes `?x ?pN ?y` (residual) joined with
/// `?pN subPropertyOf* P`. Every other atom stays in the residual BGP untouched;
/// a BGP with nothing to rewrite is returned unchanged (the hot path).
fn rewrite_bgp(patterns: Vec<TriplePattern>, tbox: &QlTbox, counter: &mut usize) -> Plan {
    let rewritable = |tp: &TriplePattern| {
        type_atom_class(tp).is_some_and(|c| tbox.superclasses.contains(c))
            || role_atom_pred(tp).is_some_and(|p| tbox.superprops.contains(p))
    };
    if !patterns.iter().any(rewritable) {
        return Plan::Bgp(patterns);
    }
    let mut residual: Vec<TriplePattern> = Vec::new();
    let mut paths: Vec<Plan> = Vec::new();
    for tp in patterns {
        if type_atom_class(&tp).is_some_and(|c| tbox.superclasses.contains(c)) {
            let class = type_atom_class(&tp).unwrap().to_string();
            *counter += 1;
            let cvar = format!("__qlc{}", counter);
            residual.push(TriplePattern {
                s: tp.s,
                p: PatternTerm::Const(RDF_TYPE.to_string()),
                o: PatternTerm::Var(cvar.clone()),
            });
            paths.push(star_path(
                PatternTerm::Var(cvar),
                SUBCLASS_OF,
                PatternTerm::Const(class),
            ));
        } else if role_atom_pred(&tp).is_some_and(|p| tbox.superprops.contains(p)) {
            let pred = role_atom_pred(&tp).unwrap().to_string();
            *counter += 1;
            let pvar = format!("__qlp{}", counter);
            residual.push(TriplePattern {
                s: tp.s,
                p: PatternTerm::Var(pvar.clone()),
                o: tp.o,
            });
            paths.push(star_path(
                PatternTerm::Var(pvar),
                SUBPROPERTY_OF,
                PatternTerm::Const(pred),
            ));
        } else {
            residual.push(tp);
        }
    }
    let mut plan = Plan::Bgp(residual);
    for p in paths {
        plan = Plan::Join(Box::new(plan), Box::new(p));
    }
    plan
}
