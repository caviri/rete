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
const RDFS_DOMAIN: &str = "<http://www.w3.org/2000/01/rdf-schema#domain>";
const RDFS_RANGE: &str = "<http://www.w3.org/2000/01/rdf-schema#range>";

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
    /// The graph declares at least one `rdfs:domain` axiom (enables the domain
    /// branch of a class-atom rewrite; the branch self-filters on the actual class).
    has_domain: bool,
    /// The graph declares at least one `rdfs:range` axiom.
    has_range: bool,
}

impl QlTbox {
    fn read(rete: &Rete) -> Self {
        QlTbox {
            superclasses: objects_of(rete, SUBCLASS_OF),
            superprops: objects_of(rete, SUBPROPERTY_OF),
            has_domain: has_predicate(rete, RDFS_DOMAIN),
            has_range: has_predicate(rete, RDFS_RANGE),
        }
    }
    fn is_empty(&self) -> bool {
        self.superclasses.is_empty()
            && self.superprops.is_empty()
            && !self.has_domain
            && !self.has_range
    }
    /// Does a concrete class atom `?x a C` need rewriting? (has subclasses, or
    /// there are domain/range axioms that could infer the type from a property.)
    fn class_rewritable(&self, c: &str) -> bool {
        self.superclasses.contains(c) || self.has_domain || self.has_range
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

/// Whether the graph has any triple with predicate `pred` (a one-row probe).
fn has_predicate(rete: &Rete, pred: &str) -> bool {
    let q = format!("SELECT ?s WHERE {{ ?s {pred} ?o }} LIMIT 1");
    matches!(super::eval_sparql(rete, &q), Ok((_, s)) if !s.is_empty())
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

/// A fresh, query-unique variable name for a rewrite-introduced binding.
fn fresh(counter: &mut usize, tag: &str) -> String {
    *counter += 1;
    format!("__ql{tag}{}", counter)
}

fn single(tp: TriplePattern) -> Plan {
    Plan::Bgp(vec![tp])
}
fn join(a: Plan, b: Plan) -> Plan {
    Plan::Join(Box::new(a), Box::new(b))
}
fn union(a: Plan, b: Plan) -> Plan {
    Plan::Union(Box::new(a), Box::new(b))
}
fn atom(s: PatternTerm, p: &str, o: PatternTerm) -> TriplePattern {
    TriplePattern {
        s,
        p: PatternTerm::Const(p.to_string()),
        o,
    }
}

/// The plan for a single class atom `subject a C` under QL: a UNION of the
/// **typing** branch (subject typed to C or a subclass) and — when the graph has
/// them — the **domain** and **range** branches (subject inferred to be a C
/// because it is the subject/object of a property whose domain/range is `⊑* C`).
fn class_atom_plan(subject: PatternTerm, class: &str, tbox: &QlTbox, counter: &mut usize) -> Plan {
    // Typing: `subject a ?c . ?c subClassOf* C` (reflexive → direct type too).
    // If C has no subclasses, the plain atom is exactly equivalent and cheaper.
    let typing = if tbox.superclasses.contains(class) {
        let c = fresh(counter, "c");
        join(
            single(atom(subject.clone(), RDF_TYPE, PatternTerm::Var(c.clone()))),
            star_path(
                PatternTerm::Var(c),
                SUBCLASS_OF,
                PatternTerm::Const(class.to_string()),
            ),
        )
    } else {
        single(atom(
            subject.clone(),
            RDF_TYPE,
            PatternTerm::Const(class.to_string()),
        ))
    };
    let mut plan = typing;
    // Domain: `?p rdfs:domain ?dc . ?dc subClassOf* C . subject ?p ?_` — a
    // property whose domain is `⊑* C` makes each of its subjects a C.
    if tbox.has_domain {
        let (p, dc, anon) = (
            fresh(counter, "dp"),
            fresh(counter, "dc"),
            fresh(counter, "da"),
        );
        let branch = join(
            join(
                single(atom(
                    PatternTerm::Var(p.clone()),
                    RDFS_DOMAIN,
                    PatternTerm::Var(dc.clone()),
                )),
                star_path(
                    PatternTerm::Var(dc),
                    SUBCLASS_OF,
                    PatternTerm::Const(class.to_string()),
                ),
            ),
            single(TriplePattern {
                s: subject.clone(),
                p: PatternTerm::Var(p),
                o: PatternTerm::Var(anon),
            }),
        );
        plan = union(plan, branch);
    }
    // Range: `?p rdfs:range ?rc . ?rc subClassOf* C . ?_ ?p subject` — a property
    // whose range is `⊑* C` makes each of its objects a C.
    if tbox.has_range {
        let (p, rc, anon) = (
            fresh(counter, "rp"),
            fresh(counter, "rc"),
            fresh(counter, "ra"),
        );
        let branch = join(
            join(
                single(atom(
                    PatternTerm::Var(p.clone()),
                    RDFS_RANGE,
                    PatternTerm::Var(rc.clone()),
                )),
                star_path(
                    PatternTerm::Var(rc),
                    SUBCLASS_OF,
                    PatternTerm::Const(class.to_string()),
                ),
            ),
            single(TriplePattern {
                s: PatternTerm::Var(anon),
                p: PatternTerm::Var(p),
                o: subject.clone(),
            }),
        );
        plan = union(plan, branch);
    }
    plan
}

/// Rewrite one BGP. Each concrete class atom `?x a C` (needing reasoning) becomes
/// a class-atom UNION plan; each role atom `?x P ?y` whose P has subproperties
/// becomes `?x ?pN ?y` joined with `?pN subPropertyOf* P`. Every other atom stays
/// in the residual BGP; a BGP with nothing to rewrite is returned unchanged.
fn rewrite_bgp(patterns: Vec<TriplePattern>, tbox: &QlTbox, counter: &mut usize) -> Plan {
    let rewritable = |tp: &TriplePattern| {
        type_atom_class(tp).is_some_and(|c| tbox.class_rewritable(c))
            || role_atom_pred(tp).is_some_and(|p| tbox.superprops.contains(p))
    };
    if !patterns.iter().any(rewritable) {
        return Plan::Bgp(patterns);
    }
    let mut residual: Vec<TriplePattern> = Vec::new();
    let mut extra: Vec<Plan> = Vec::new();
    for tp in patterns {
        if type_atom_class(&tp).is_some_and(|c| tbox.class_rewritable(c)) {
            let class = type_atom_class(&tp).unwrap().to_string();
            extra.push(class_atom_plan(tp.s, &class, tbox, counter));
        } else if role_atom_pred(&tp).is_some_and(|p| tbox.superprops.contains(p)) {
            let pred = role_atom_pred(&tp).unwrap().to_string();
            let pvar = fresh(counter, "p");
            residual.push(TriplePattern {
                s: tp.s,
                p: PatternTerm::Var(pvar.clone()),
                o: tp.o,
            });
            extra.push(star_path(
                PatternTerm::Var(pvar),
                SUBPROPERTY_OF,
                PatternTerm::Const(pred),
            ));
        } else {
            residual.push(tp);
        }
    }
    // Conjoin the residual BGP with each extra plan. If every atom was rewritten,
    // start from the first extra plan (avoid an empty-BGP left operand).
    let mut plan;
    let mut iter = extra.into_iter();
    if residual.is_empty() {
        plan = iter.next().expect("rewritable ⇒ at least one extra plan");
    } else {
        plan = Plan::Bgp(residual);
    }
    for p in iter {
        plan = join(plan, p);
    }
    plan
}
