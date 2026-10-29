//! OWL 2 QL — lazy reasoning by query REWRITING (design in `dev/owl-ql.md`).
//!
//! Opt-in. Instead of materializing entailments into the (huge, remote) ABox,
//! rewrite the *query* so that evaluating it over the RAW data yields the certain
//! answers. A remote file becomes ontology-aware with no rebuild, and only the
//! bytes the rewritten query touches are fetched.
//!
//! **The TBox** is read from the file's default graph, once per reasoned query:
//! the `rdfs:subClassOf`, `rdfs:subPropertyOf`, `rdfs:domain`, `rdfs:range` and
//! `owl:inverseOf` statements (a few scans of small predicates). The class and
//! property hierarchies are closed here, at rewrite time, so the rewritten
//! query carries the concrete classes and properties to look up rather than a
//! hierarchy walk.
//!
//! **An entailed atom becomes the distinct union of the asserted patterns that
//! entail it** ([`Plan::DistinctUnion`]):
//!
//! - class atom `?x a C` → `?x a D` for every `D ⊑* C`, plus `?x p ?_` for every
//!   `p` whose `rdfs:domain` is `⊑* C`, plus `?_ q ?x` for every `q` whose
//!   `rdfs:range` is `⊑* C` (and their subproperties);
//! - role atom `?x P ?y` → `?x Q ?y` for every `Q ⊑* P`, plus `?y R ?x` for
//!   every `R ⊑* P'` with `P'` an `owl:inverseOf` of `P`.
//!
//! Each solution comes out ONCE: the entailed triple `?x a C` exists once, even
//! when `?x` is typed both `C` and a subclass of it, or is also the subject of a
//! property whose domain is `C`. A plain union would return it once per
//! asserted triple (508,044 rows for 258,268 OpenCitations works typed both
//! `fabio:Expression` and a subtype). The sources stream sorted and merge, so
//! this costs no set of every solution.
//!
//! **Gating is per atom.** A class gets a domain (range) source only for the
//! properties whose domain (range) is `⊑* C`, and a class, a property or a
//! domain/range property the file never uses contributes no source at all. An
//! atom whose sources reduce to itself is left exactly as written — reasoning is
//! zero-overhead where it can add nothing, and one unrelated `rdfs:domain`
//! axiom no longer adds a branch to every class atom of the query.
//!
//! Existential restrictions (`A ⊑ ∃P`) still answer a role atom with a purely
//! existential object through a hierarchy walk ([`existential_branch`]).
//!
//! This runs as a post-lowering pass on the `Plan` tree, only when reasoning is
//! requested — the hot BGP matcher and every default query are untouched.

use super::*;
use crate::bgp::{PatternTerm, TriplePattern};
use crate::{Dictionary, Rete};
use std::collections::{HashMap, HashSet};

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const SUBPROPERTY_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subPropertyOf>";
const RDFS_DOMAIN: &str = "<http://www.w3.org/2000/01/rdf-schema#domain>";
const RDFS_RANGE: &str = "<http://www.w3.org/2000/01/rdf-schema#range>";
const OWL_INVERSE_OF: &str = "<http://www.w3.org/2002/07/owl#inverseOf>";
const OWL_ON_PROPERTY: &str = "<http://www.w3.org/2002/07/owl#onProperty>";
const OWL_SOME_VALUES_FROM: &str = "<http://www.w3.org/2002/07/owl#someValuesFrom>";

/// The slice of the ontology (TBox) the rewrite needs, read from the default
/// graph. Every map is keyed by the *super*-term, so closing a hierarchy below a
/// class or property is a walk down it.
struct QlTbox {
    /// `C` ↦ its direct subclasses (`D rdfs:subClassOf C`).
    sub_classes: HashMap<String, Vec<String>>,
    /// `P` ↦ its direct subproperties (`Q rdfs:subPropertyOf P`).
    sub_props: HashMap<String, Vec<String>>,
    /// Class `D` ↦ the properties declared `p rdfs:domain D`.
    domain_of: HashMap<String, Vec<String>>,
    /// Class `D` ↦ the properties declared `p rdfs:range D`.
    range_of: HashMap<String, Vec<String>>,
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
            sub_classes: by_object(rete, SUBCLASS_OF),
            sub_props: by_object(rete, SUBPROPERTY_OF),
            domain_of: by_object(rete, RDFS_DOMAIN),
            range_of: by_object(rete, RDFS_RANGE),
            inverses: inverse_pairs(rete),
            has_restrictions: has_predicate(rete, OWL_SOME_VALUES_FROM),
        }
    }
    fn is_empty(&self) -> bool {
        self.sub_classes.is_empty()
            && self.sub_props.is_empty()
            && self.domain_of.is_empty()
            && self.range_of.is_empty()
            && self.inverses.is_empty()
            && !self.has_restrictions
    }
    /// `C` and every class below it (`D ⊑* C`), each once.
    fn classes_below(&self, c: &str) -> Vec<String> {
        closure_below(&self.sub_classes, c)
    }
    /// `P` and every property below it (`Q ⊑* P`), each once.
    fn props_below(&self, p: &str) -> Vec<String> {
        closure_below(&self.sub_props, p)
    }
    /// The properties whose use in subject (`domain`) or object position makes
    /// a node a `C`: those declared with a domain (range) `⊑* C`, and every
    /// subproperty of one. Sorted and deduplicated.
    fn typing_props(&self, c: &str, domain: bool) -> Vec<String> {
        let decl = if domain {
            &self.domain_of
        } else {
            &self.range_of
        };
        if decl.is_empty() {
            return Vec::new();
        }
        let mut out: Vec<String> = Vec::new();
        for d in self.classes_below(c) {
            for p in decl.get(&d).into_iter().flatten() {
                out.extend(self.props_below(p));
            }
        }
        out.sort();
        out.dedup();
        out
    }
}

/// `top` and everything reachable from it through `below` (a hierarchy keyed by
/// the super-term), breadth first, each once — cycles included.
fn closure_below(below: &HashMap<String, Vec<String>>, top: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = vec![top.to_string()];
    seen.insert(top.to_string());
    let mut i = 0;
    while i < out.len() {
        if let Some(subs) = below.get(&out[i]) {
            for s in subs {
                if seen.insert(s.clone()) {
                    out.push(s.clone());
                }
            }
        }
        i += 1;
    }
    out
}

/// The statements with predicate `pred`, grouped by object: `o ↦ [s, …]`
/// (reasoning OFF, so no recursion). Bounded — TBox predicates are a small
/// fraction of any graph.
fn by_object(rete: &Rete, pred: &str) -> HashMap<String, Vec<String>> {
    let q = format!("SELECT ?s ?o WHERE {{ ?s {pred} ?o }}");
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    if let Ok((_, sols)) = super::eval_sparql(rete, &q) {
        for b in sols {
            if let (Some(s), Some(o)) = (b.get("s"), b.get("o")) {
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
    let term = |t: &PatternTerm, c: &mut HashMap<String, usize>| {
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
        Plan::DistinctUnion { sources, .. } => {
            for tp in sources {
                term(&tp.s, c);
                term(&tp.p, c);
                term(&tp.o, c);
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
    let mut rw = Rewriter {
        tbox: &tbox,
        dict: rete.dictionary(),
        exq: &exq,
        counter: 0,
    };
    rw.plan(plan)
}

/// Per-query rewrite state: the TBox, the dictionary (a class or property the
/// file never uses contributes no source), the existential variables and the
/// fresh-name counter.
struct Rewriter<'a> {
    tbox: &'a QlTbox,
    dict: &'a Dictionary,
    exq: &'a HashSet<String>,
    counter: usize,
}

impl Rewriter<'_> {
    /// Walk the plan tree; transform each BGP, recurse structurally through
    /// every other operator so reasoning reaches nested patterns
    /// (UNION/OPTIONAL/subquery…).
    fn plan(&mut self, plan: Plan) -> Plan {
        match plan {
            Plan::Bgp(patterns) => self.bgp(patterns),
            Plan::Join(a, b) => Plan::Join(self.boxed(a), self.boxed(b)),
            Plan::Union(a, b) => Plan::Union(self.boxed(a), self.boxed(b)),
            Plan::LeftJoin(a, b, cond) => Plan::LeftJoin(self.boxed(a), self.boxed(b), cond),
            Plan::Filter(f, p) => Plan::Filter(f, self.boxed(p)),
            Plan::Extend(v, e, p) => Plan::Extend(v, e, self.boxed(p)),
            Plan::Minus(a, b) => Plan::Minus(self.boxed(a), self.boxed(b)),
            Plan::Graph(t, p) => Plan::Graph(t, self.boxed(p)),
            Plan::Subquery(mut sel) => {
                sel.plan = self.plan(sel.plan);
                Plan::Subquery(sel)
            }
            // No BGP inside — a path, inline VALUES, a SERVICE block (rewriting
            // a remote sub-query against our TBox would be unsound), or an
            // already-rewritten atom — left unchanged.
            other => other,
        }
    }

    /// Rewrite a boxed sub-plan in place (reusing its allocation).
    fn boxed(&mut self, mut p: Box<Plan>) -> Box<Plan> {
        let inner = std::mem::replace(&mut *p, Plan::Bgp(Vec::new()));
        *p = self.plan(inner);
        p
    }

    /// A fresh, query-unique variable name for a rewrite-introduced binding.
    fn fresh(&mut self, tag: &str) -> String {
        self.counter += 1;
        format!("__ql{tag}{}", self.counter)
    }

    fn uses_class(&self, c: &str) -> bool {
        self.dict.object_id(c).is_some()
    }
    fn uses_predicate(&self, p: &str) -> bool {
        self.dict.predicate_id(p).is_some()
    }

    /// Rewrite one BGP. Each class atom `s a C` and role atom `s P o` that
    /// reasoning can add rows to becomes its own plan, joined with the residual
    /// BGP of every other atom; a BGP with nothing to rewrite is returned as is.
    fn bgp(&mut self, patterns: Vec<TriplePattern>) -> Plan {
        let mut residual: Vec<TriplePattern> = Vec::new();
        let mut extra: Vec<Plan> = Vec::new();
        for tp in patterns {
            let rewritten = if let Some(class) = type_atom_class(&tp) {
                let class = class.to_string();
                self.class_atom(&tp.s, &class)
            } else if let Some(pred) = role_atom_pred(&tp) {
                let pred = pred.to_string();
                self.role_atom(&tp.s, &pred, &tp.o)
            } else {
                None
            };
            match rewritten {
                Some(plan) => extra.push(plan),
                None => residual.push(tp),
            }
        }
        if extra.is_empty() {
            return Plan::Bgp(residual);
        }
        // Conjoin the residual BGP with each extra plan. If every atom was
        // rewritten, start from the first extra plan (no empty-BGP operand).
        let mut iter = extra.into_iter();
        let mut plan = if residual.is_empty() {
            iter.next().expect("extra is non-empty")
        } else {
            Plan::Bgp(residual)
        };
        for p in iter {
            plan = join(plan, p);
        }
        plan
    }

    /// The plan for a class atom `subject a C`, or `None` when reasoning adds
    /// nothing to it (no subclass, domain or range the file uses).
    fn class_atom(&mut self, subject: &PatternTerm, class: &str) -> Option<Plan> {
        let mut sources: Vec<TriplePattern> = Vec::new();
        for d in self.tbox.classes_below(class) {
            if self.uses_class(&d) {
                sources.push(atom(subject.clone(), RDF_TYPE, PatternTerm::Const(d)));
            }
        }
        // Domain: each subject of a property whose domain is `⊑* C` is a C.
        for p in self.tbox.typing_props(class, true) {
            if self.uses_predicate(&p) {
                let anon = PatternTerm::Var(self.fresh("dra"));
                sources.push(atom(subject.clone(), &p, anon));
            }
        }
        // Range: each object of a property whose range is `⊑* C` is a C.
        for p in self.tbox.typing_props(class, false) {
            if self.uses_predicate(&p) {
                let anon = PatternTerm::Var(self.fresh("dra"));
                sources.push(atom(anon, &p, subject.clone()));
            }
        }
        // Nothing beyond the atom itself (or nothing at all, when not even `C`
        // is used): the atom as written is exact and stays in the BGP.
        let only_itself = match sources.as_slice() {
            [] => true,
            [one] => type_atom_class(one) == Some(class),
            _ => false,
        };
        if only_itself {
            return None;
        }
        Some(Plan::DistinctUnion {
            vars: term_vars(&[subject]),
            sources,
        })
    }

    /// The plan for a role atom `s P o`: the distinct union of `s Q o` for each
    /// `Q ⊑* P` and `o R s` for each `R ⊑* P'`, `P'` an inverse of `P` — plus,
    /// when `o` (or `s`) is a purely existential variable and the graph has `∃`
    /// restrictions, the existential branch. `None` when reasoning adds nothing.
    fn role_atom(&mut self, s: &PatternTerm, pred: &str, o: &PatternTerm) -> Option<Plan> {
        let mut sources: Vec<TriplePattern> = Vec::new();
        for q in self.tbox.props_below(pred) {
            if self.uses_predicate(&q) {
                sources.push(atom(s.clone(), &q, o.clone()));
            }
        }
        if let Some(invs) = self.tbox.inverses.get(pred) {
            for inv in invs {
                for q in self.tbox.props_below(inv) {
                    if self.uses_predicate(&q) {
                        sources.push(atom(o.clone(), &q, s.clone()));
                    }
                }
            }
        }
        let has_r = self.tbox.has_restrictions;
        let forward_ex = has_r && is_existential(o, self.exq);
        // Inverse existential `A ⊑ ∃P⁻`: the SUBJECT is existential, so `o` is
        // entailed to be a P-object when it is an A restricted on P's inverse.
        let inverse_ex = has_r
            && is_existential(s, self.exq)
            && self.tbox.inverses.get(pred).is_some_and(|v| !v.is_empty());
        let only_itself = match sources.as_slice() {
            [] => true,
            [one] => role_atom_pred(one) == Some(pred) && one.s == *s,
            _ => false,
        };
        if only_itself && !forward_ex && !inverse_ex {
            return None;
        }
        let mut plan = if sources.is_empty() {
            None
        } else {
            Some(Plan::DistinctUnion {
                vars: term_vars(&[s, o]),
                sources,
            })
        };
        let add = |plan: &mut Option<Plan>, branch: Plan| {
            *plan = Some(match plan.take() {
                Some(p) => union(p, branch),
                None => branch,
            });
        };
        if forward_ex {
            // Forward existential `A ⊑ ∃P`: the OBJECT is existential, so `s`
            // is entailed to have a P-successor when it is (transitively) an A.
            let branch = existential_branch(s.clone(), pred, &mut self.counter);
            add(&mut plan, distinct_on(branch, s));
        }
        if inverse_ex {
            let invs = self.tbox.inverses.get(pred).cloned().unwrap_or_default();
            for q in invs {
                let branch = existential_branch(o.clone(), &q, &mut self.counter);
                add(&mut plan, distinct_on(branch, o));
            }
        }
        // `sources` empty and no existential branch was returned above.
        plan
    }
}

/// The variables among `terms`, in order, each once.
fn term_vars(terms: &[&PatternTerm]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in terms {
        if let PatternTerm::Var(v) = t {
            if !out.contains(v) {
                out.push(v.clone());
            }
        }
    }
    out
}

/// `plan`'s rows reduced to the distinct values of `subject` (one row, or none,
/// for a constant): an existential branch can reach one node through several
/// classes, and its entailed fact holds once.
fn distinct_on(plan: Plan, subject: &PatternTerm) -> Plan {
    let mut sel = Select::default();
    match subject {
        PatternTerm::Var(v) => {
            sel.project = vec![v.clone()];
            sel.distinct = true;
        }
        PatternTerm::Const(_) => sel.limit = Some(1),
    }
    sel.plan = plan;
    Plan::Subquery(Box::new(sel))
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

/// A fresh, query-unique variable name (for the existential branch, which
/// builds its own patterns).
fn fresh(counter: &mut usize, tag: &str) -> String {
    *counter += 1;
    format!("__ql{tag}{}", counter)
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

/// `true` iff `t` is a variable that the query uses purely existentially (occurs
/// once, not returned), so an anonymous `∃` successor is an admissible answer.
fn is_existential(t: &PatternTerm, exq: &HashSet<String>) -> bool {
    matches!(t, PatternTerm::Var(v) if exq.contains(v))
}
