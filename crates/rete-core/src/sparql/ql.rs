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
use std::collections::{HashMap, HashSet};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const SUBPROPERTY_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const RDFS_DOMAIN: &str = "<http://www.w3.org/2000/01/rdf-schema#domain>";
const RDFS_RANGE: &str = "<http://www.w3.org/2000/01/rdf-schema#range>";
const OWL_INVERSE_OF: &str = "<http://www.w3.org/2002/07/owl#inverseOf>";
const OWL_ON_PROPERTY: &str = "<http://www.w3.org/2002/07/owl#onProperty>";
const OWL_SOME_VALUES_FROM: &str = "<http://www.w3.org/2002/07/owl#someValuesFrom>";

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
    /// Property → its inverses (`P owl:inverseOf Q`, both directions), so a role
    /// atom `?x P ?y` is also answered by `?y Q ?x`.
    inverses: HashMap<String, Vec<String>>,
    /// The graph declares at least one existential restriction (`owl:someValuesFrom`).
    /// Enables the existential branch: `?x P ?y` with `?y` purely existential is
    /// entailed when `?x` is (transitively) an `A` with `A ⊑ ∃P`.
    has_restrictions: bool,
}

impl QlTbox {
    fn read(rete: &Rete) -> Self {
        QlTbox {
            superclasses: objects_of(rete, SUBCLASS_OF),
            superprops: objects_of(rete, SUBPROPERTY_OF),
            has_domain: has_predicate(rete, RDFS_DOMAIN),
            has_range: has_predicate(rete, RDFS_RANGE),
            inverses: inverse_pairs(rete),
            has_restrictions: has_predicate(rete, OWL_SOME_VALUES_FROM),
        }
    }
    fn is_empty(&self) -> bool {
        self.superclasses.is_empty()
            && self.superprops.is_empty()
            && !self.has_domain
            && !self.has_range
            && self.inverses.is_empty()
            && !self.has_restrictions
    }
    /// Does a concrete class atom `?x a C` need rewriting? (has subclasses, or
    /// there are domain/range axioms that could infer the type from a property.)
    fn class_rewritable(&self, c: &str) -> bool {
        self.superclasses.contains(c) || self.has_domain || self.has_range
    }
    /// Does a role atom `?x P ?y` need rewriting? (P has subproperties or inverses.)
    fn role_rewritable(&self, p: &str) -> bool {
        self.superprops.contains(p) || self.inverses.contains_key(p)
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

/// The `owl:inverseOf` relation as a SYMMETRIC map (P ↦ its inverses), so a role
/// atom on P can also match its inverse in either declared direction. Bounded —
/// inverse declarations are a handful of TBox triples.
fn inverse_pairs(rete: &Rete) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    let q = format!("SELECT ?s ?o WHERE {{ ?s {OWL_INVERSE_OF} ?o }}");
    if let Ok((_, sols)) = super::eval_sparql(rete, &q) {
        for b in sols {
            if let (Some(s), Some(o)) = (b.get("s"), b.get("o")) {
                m.entry(s.clone()).or_default().push(o.clone());
                m.entry(o.clone()).or_default().push(s.clone());
            }
        }
    }
    for v in m.values_mut() {
        v.sort();
        v.dedup();
    }
    m
}

/// Count every occurrence of each variable across the whole plan — triple
/// patterns, path endpoints, filter/bind expressions, VALUES, sub-queries — so
/// the existential rewrite can tell a **purely existential** object variable
/// (occurs exactly once, nowhere else) from a shared/returned one. Overcounting
/// is safe (fewer existential rewrites); undercounting would be UNSOUND, so every
/// position that references a variable is counted.
fn plan_var_occurrences(plan: &Plan, c: &mut HashMap<String, usize>) {
    let mut term = |t: &PatternTerm, c: &mut HashMap<String, usize>| {
        if let PatternTerm::Var(v) = t {
            *c.entry(v.clone()).or_insert(0) += 1;
        }
    };
    match plan {
        Plan::Bgp(pats) => {
            for tp in pats {
                term(&tp.s, c);
                term(&tp.p, c);
                term(&tp.o, c);
            }
        }
        Plan::Path(s, _, o) => {
            term(s, c);
            term(o, c);
        }
        Plan::Join(a, b) | Plan::Union(a, b) | Plan::Minus(a, b) => {
            plan_var_occurrences(a, c);
            plan_var_occurrences(b, c);
        }
        Plan::LeftJoin(a, b, cond) => {
            plan_var_occurrences(a, c);
            plan_var_occurrences(b, c);
            if let Some(e) = cond {
                fexpr_var_occurrences(e, c);
            }
        }
        Plan::Filter(e, p) => {
            fexpr_var_occurrences(e, c);
            plan_var_occurrences(p, c);
        }
        Plan::Extend(v, e, p) => {
            *c.entry(v.clone()).or_insert(0) += 1;
            fexpr_var_occurrences(e, c);
            plan_var_occurrences(p, c);
        }
        Plan::Graph(target, p) => {
            if let GraphTarget::Var(v) = target {
                *c.entry(v.clone()).or_insert(0) += 1;
            }
            plan_var_occurrences(p, c);
        }
        Plan::Values(vars, _) => {
            for v in vars {
                *c.entry(v.clone()).or_insert(0) += 1;
            }
        }
        Plan::Subquery(sel) => {
            // Its projected vars are visible outside — count them as uses.
            for v in &sel.project {
                *c.entry(v.clone()).or_insert(0) += 1;
            }
        }
        Plan::Service { vars, .. } => {
            for v in vars {
                *c.entry(v.clone()).or_insert(0) += 1;
            }
        }
    }
}

/// Count variable occurrences inside a filter/bind expression (incl. `EXISTS`
/// sub-plans, whose variables reference the surrounding pattern).
fn fexpr_var_occurrences(e: &FExpr, c: &mut HashMap<String, usize>) {
    match e {
        FExpr::Var(v) | FExpr::Bound(v) => {
            *c.entry(v.clone()).or_insert(0) += 1;
        }
        FExpr::Const(_) => {}
        FExpr::Arith(_, a, b)
        | FExpr::SameTerm(a, b)
        | FExpr::Compare(_, a, b)
        | FExpr::And(a, b)
        | FExpr::Or(a, b) => {
            fexpr_var_occurrences(a, c);
            fexpr_var_occurrences(b, c);
        }
        FExpr::Not(a) => fexpr_var_occurrences(a, c),
        FExpr::Func(_, xs) | FExpr::Coalesce(xs) => {
            xs.iter().for_each(|x| fexpr_var_occurrences(x, c))
        }
        FExpr::If(a, b, d) => {
            fexpr_var_occurrences(a, c);
            fexpr_var_occurrences(b, c);
            fexpr_var_occurrences(d, c);
        }
        FExpr::In(a, xs) => {
            fexpr_var_occurrences(a, c);
            xs.iter().for_each(|x| fexpr_var_occurrences(x, c));
        }
        FExpr::Exists(p) => plan_var_occurrences(p, c),
    }
}

/// Rewrite a lowered plan for OWL 2 QL entailment. `projected` are the query's
/// distinguished (SELECT) variables — never treated as existential.
pub(crate) fn reason_rewrite(plan: Plan, rete: &Rete, projected: &[String]) -> Plan {
    let tbox = QlTbox::read(rete);
    if tbox.is_empty() {
        return plan; // no axioms — reasoning is a no-op
    }
    // Existential objects: variables that occur EXACTLY ONCE in the whole query
    // and are NOT distinguished — only these may be answered via `A ⊑ ∃P` (an
    // anonymous successor cannot be returned or joined). `SELECT *` (empty
    // `projected`) distinguishes every variable, so nothing is existential.
    let exq: HashSet<String> = if tbox.has_restrictions && !projected.is_empty() {
        let mut counts = HashMap::new();
        plan_var_occurrences(&plan, &mut counts);
        let proj: HashSet<&String> = projected.iter().collect();
        counts
            .into_iter()
            .filter(|(v, n)| *n == 1 && !proj.contains(v))
            .map(|(v, _)| v)
            .collect()
    } else {
        HashSet::new()
    };
    let mut counter: usize = 0;
    rewrite_plan(plan, &tbox, &exq, &mut counter)
}

/// Walk the plan tree; transform each BGP, recurse structurally through every
/// other operator so reasoning reaches nested patterns (UNION/OPTIONAL/subquery…).
fn rewrite_plan(plan: Plan, tbox: &QlTbox, exq: &HashSet<String>, counter: &mut usize) -> Plan {
    let recur = |p: Box<Plan>, counter: &mut usize| Box::new(rewrite_plan(*p, tbox, exq, counter));
    match plan {
        Plan::Bgp(patterns) => rewrite_bgp(patterns, tbox, exq, counter),
        Plan::Join(a, b) => Plan::Join(recur(a, counter), recur(b, counter)),
        Plan::Union(a, b) => Plan::Union(recur(a, counter), recur(b, counter)),
        Plan::LeftJoin(a, b, cond) => Plan::LeftJoin(recur(a, counter), recur(b, counter), cond),
        Plan::Filter(f, p) => Plan::Filter(f, recur(p, counter)),
        Plan::Extend(v, e, p) => Plan::Extend(v, e, recur(p, counter)),
        Plan::Minus(a, b) => Plan::Minus(recur(a, counter), recur(b, counter)),
        Plan::Graph(t, p) => Plan::Graph(t, recur(p, counter)),
        Plan::Subquery(mut sel) => {
            sel.plan = rewrite_plan(sel.plan, tbox, exq, counter);
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
    // Domain: a property whose domain is `⊑* C` makes each of its subjects a C.
    if tbox.has_domain {
        plan = union(plan, dr_branch(subject.clone(), true, class, tbox, counter));
    }
    // Range: a property whose range is `⊑* C` makes each of its objects a C.
    if tbox.has_range {
        plan = union(
            plan,
            dr_branch(subject.clone(), false, class, tbox, counter),
        );
    }
    plan
}

/// The domain (`is_domain`) or range branch of a class atom `subject a C`:
/// `?pd rdfs:domain|range ?dc . ?dc subClassOf* C` and `subject` uses a property
/// that is `subPropertyOf* ?pd` (in subject/object position respectively). When
/// the graph has subproperties the used property walks `subPropertyOf*` (so a
/// subproperty of a domain/range-declared property also infers the type); with
/// none it is `?pd` directly.
fn dr_branch(
    subject: PatternTerm,
    is_domain: bool,
    class: &str,
    tbox: &QlTbox,
    counter: &mut usize,
) -> Plan {
    let (pd, dc, anon) = (
        fresh(counter, "drp"),
        fresh(counter, "drc"),
        fresh(counter, "dra"),
    );
    let dr_pred = if is_domain { RDFS_DOMAIN } else { RDFS_RANGE };
    // `?pd <domain|range> ?dc . ?dc subClassOf* C`
    let restr = join(
        single(atom(
            PatternTerm::Var(pd.clone()),
            dr_pred,
            PatternTerm::Var(dc.clone()),
        )),
        star_path(
            PatternTerm::Var(dc),
            SUBCLASS_OF,
            PatternTerm::Const(class.to_string()),
        ),
    );
    // The property `subject` actually uses — `?pd`, or (composing) a subproperty
    // `?q` with `?q subPropertyOf* ?pd`.
    let (used, compose) = if tbox.superprops.is_empty() {
        (pd.clone(), None)
    } else {
        let q = fresh(counter, "drq");
        (
            q.clone(),
            Some(star_path(
                PatternTerm::Var(q),
                SUBPROPERTY_OF,
                PatternTerm::Var(pd.clone()),
            )),
        )
    };
    let use_pat = if is_domain {
        single(TriplePattern {
            s: subject,
            p: PatternTerm::Var(used),
            o: PatternTerm::Var(anon),
        })
    } else {
        single(TriplePattern {
            s: PatternTerm::Var(anon),
            p: PatternTerm::Var(used),
            o: subject,
        })
    };
    let mut branch = join(restr, use_pat);
    if let Some(e) = compose {
        branch = join(branch, e);
    }
    branch
}

/// One directional branch matching `s pred o`, expanded over `subPropertyOf*`
/// when `pred` has subproperties (else the plain atom).
fn role_branch(
    s: PatternTerm,
    pred: &str,
    o: PatternTerm,
    tbox: &QlTbox,
    counter: &mut usize,
) -> Plan {
    if tbox.superprops.contains(pred) {
        let pv = fresh(counter, "p");
        join(
            single(TriplePattern {
                s,
                p: PatternTerm::Var(pv.clone()),
                o,
            }),
            star_path(
                PatternTerm::Var(pv),
                SUBPROPERTY_OF,
                PatternTerm::Const(pred.to_string()),
            ),
        )
    } else {
        single(atom(s, pred, o))
    }
}

/// The existential branch for `subject P ?y` with `?y` purely existential:
/// `subject` is entailed to have a P-successor when it is (transitively) a member
/// of a class `A` with `A ⊑ ∃P` — i.e.
/// `?r owl:onProperty P . ?r owl:someValuesFrom ?b . ?a rdfs:subClassOf ?r .
///  subject a ?ax . ?ax rdfs:subClassOf* ?a`.
/// The object stays unbound (the successor is anonymous), which is why this is
/// only used when `?y` occurs nowhere else and is not returned.
fn existential_branch(subject: PatternTerm, pred: &str, counter: &mut usize) -> Plan {
    let (r, b, a, ax) = (
        fresh(counter, "er"),
        fresh(counter, "eb"),
        fresh(counter, "ea"),
        fresh(counter, "ex"),
    );
    let bgp = Plan::Bgp(vec![
        atom(
            PatternTerm::Var(r.clone()),
            OWL_ON_PROPERTY,
            PatternTerm::Const(pred.to_string()),
        ),
        atom(
            PatternTerm::Var(r.clone()),
            OWL_SOME_VALUES_FROM,
            PatternTerm::Var(b),
        ),
        atom(
            PatternTerm::Var(a.clone()),
            SUBCLASS_OF,
            PatternTerm::Var(r),
        ),
        atom(subject, RDF_TYPE, PatternTerm::Var(ax.clone())),
    ]);
    join(
        bgp,
        star_path(PatternTerm::Var(ax), SUBCLASS_OF, PatternTerm::Var(a)),
    )
}

/// The plan for a role atom `s P o` under QL: the forward branch (P and its
/// subproperties) UNION, for each inverse Q of P, an inverse branch `o Q s`
/// (Q and its subproperties), UNION — when `o` is a purely-existential variable
/// and the graph has `∃P` restrictions — an existential branch.
fn role_atom_plan(
    s: PatternTerm,
    pred: &str,
    o: PatternTerm,
    tbox: &QlTbox,
    exq: &HashSet<String>,
    counter: &mut usize,
) -> Plan {
    let mut plan = role_branch(s.clone(), pred, o.clone(), tbox, counter);
    if let Some(invs) = tbox.inverses.get(pred) {
        for q in invs {
            let branch = role_branch(o.clone(), q, s.clone(), tbox, counter);
            plan = union(plan, branch);
        }
    }
    if tbox.has_restrictions && is_existential(&o, exq) {
        // Forward existential `A ⊑ ∃P`: the OBJECT is existential, so `s` is
        // entailed to have a P-successor when it is (transitively) such an A.
        plan = union(plan, existential_branch(s.clone(), pred, counter));
    }
    if tbox.has_restrictions && is_existential(&s, exq) {
        // Inverse existential `A ⊑ ∃P⁻`: the SUBJECT is existential, so `o` is
        // entailed to be a P-object when it is an A restricted on P's inverse Q
        // (∃Q ≡ ∃P⁻). Uses each NAMED inverse of P.
        if let Some(invs) = tbox.inverses.get(pred) {
            for q in invs {
                plan = union(plan, existential_branch(o.clone(), q, counter));
            }
        }
    }
    plan
}

/// `true` iff `t` is a variable that the query uses purely existentially (occurs
/// once, not returned), so an anonymous `∃` successor is an admissible answer.
fn is_existential(t: &PatternTerm, exq: &HashSet<String>) -> bool {
    matches!(t, PatternTerm::Var(v) if exq.contains(v))
}

/// Rewrite one BGP. Each concrete class atom `?x a C` (needing reasoning) becomes
/// a class-atom UNION plan; each role atom `?x P ?y` whose P has subproperties or
/// inverses becomes a role-atom UNION plan. Every other atom stays in the residual
/// BGP; a BGP with nothing to rewrite is returned unchanged.
fn rewrite_bgp(
    patterns: Vec<TriplePattern>,
    tbox: &QlTbox,
    exq: &HashSet<String>,
    counter: &mut usize,
) -> Plan {
    // A role atom needs rewriting if P has subproperties/inverses, OR (with `∃`
    // restrictions in the graph) its object is existential (forward `∃P`), or its
    // subject is existential and P has an inverse (inverse `∃P⁻`).
    let role_needs = |tp: &TriplePattern| {
        role_atom_pred(tp).is_some_and(|p| {
            tbox.role_rewritable(p)
                || (tbox.has_restrictions
                    && (is_existential(&tp.o, exq)
                        || (is_existential(&tp.s, exq) && tbox.inverses.contains_key(p))))
        })
    };
    let rewritable = |tp: &TriplePattern| {
        type_atom_class(tp).is_some_and(|c| tbox.class_rewritable(c)) || role_needs(tp)
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
        } else if role_needs(&tp) {
            let pred = role_atom_pred(&tp).unwrap().to_string();
            extra.push(role_atom_plan(tp.s, &pred, tp.o, tbox, exq, counter));
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
