//! Running one query as many smaller ones, split by the values of one
//! variable — so a long extraction can go in short, resumable chunks (the
//! Claude Desktop extension's `partition_by`).
//!
//! The split is exact when every solution binds the variable: each solution
//! then belongs to exactly one chunk, the one holding its value, so the
//! chunks' solutions together are the query's. [`plan`] checks that, refuses
//! the solution modifiers a single chunk cannot apply (ORDER BY, an aggregate
//! over more than one key), and writes the **keys query**: the variable's
//! distinct values from a required `VALUES` list on it when the query has one,
//! else over the one required triple pattern that binds it with constants and
//! is cheapest to scan. That is a superset of the values the solutions take —
//! a key with no solution just makes an empty chunk — and listing it reads
//! nothing or one range scan, where the query itself may be one read per
//! entity. [`seed`] then cuts the query's own `VALUES` list down to a chunk's
//! keys, or places them as `VALUES` on the first required pattern block that
//! binds the variable, where the engine's VALUES pushdown turns each key into
//! an index lookup.

use std::collections::{BTreeSet, HashSet};

use serde::Serialize;
use spargebra::algebra::GraphPattern;
use spargebra::term::{
    GroundTerm, NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern, Variable,
};
use spargebra::Query;

use super::eval::scan_estimate;
use super::lower::{lower_select, parse_query};
use super::{Plan, SparqlError};
use crate::bgp::PatternTerm;
use crate::file::Rete;

/// How to run a query as chunks split by one variable: see [`crate::partition_plan`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PartitionPlan {
    /// The variable split on, without its `?`.
    pub var: String,
    /// `"select"` or `"construct"`.
    pub form: &'static str,
    /// `SELECT DISTINCT ?var …`: every value the variable takes in a solution,
    /// and possibly more (a key with no solution makes an empty chunk).
    pub keys_query: String,
    /// Where the keys query lists them from: `"values"`, a required `VALUES`
    /// list on the variable (no read); `"pattern"`, the cheapest required
    /// triple pattern binding it with constants alone (one range scan);
    /// `"where"`, the whole WHERE clause, when neither exists — then listing
    /// them may cost as much as the query.
    pub keys_from: &'static str,
    /// The query's top-level OFFSET and LIMIT. Each chunk drops them; they
    /// apply once, to the chunks' solutions taken in key order.
    pub offset: usize,
    pub limit: Option<usize>,
    /// Whether two chunks can yield the same row, so the merged rows need
    /// deduplicating: a DISTINCT that does not project the variable, and every
    /// CONSTRUCT (a graph is a set of triples).
    pub dedupe: bool,
}

/// `?w`, `$w` or `w` → `w`.
pub(super) fn var_name(var: &str) -> &str {
    var.trim().trim_start_matches(['?', '$'])
}

fn refuse(var: &str, reason: impl Into<String>) -> SparqlError {
    SparqlError::Partition(format!("by ?{var}: {}", reason.into()))
}

/// The query's top-level solution modifiers, peeled off its algebra, and the
/// WHERE pattern under them.
struct Shape<'q> {
    form: &'static str,
    offset: usize,
    limit: Option<usize>,
    distinct: bool,
    /// The top-level projection (a CONSTRUCT has none).
    projected: Option<&'q [Variable]>,
    ordered: bool,
    /// GROUP BY's variables, when the query aggregates.
    group: Option<&'q [Variable]>,
    /// Variables a top-level select expression `(… AS ?v)` assigns.
    computed: Vec<&'q Variable>,
    where_: &'q GraphPattern,
}

/// Peel the top-level modifiers in the order spargebra nests them — Slice,
/// Distinct/Reduced, Project, OrderBy, select expressions and HAVING, Group —
/// each at most once and only in that order, so a nested sub-SELECT's own
/// modifiers are never mistaken for the query's.
fn shape(query: &Query) -> Result<Shape<'_>, &'static str> {
    let (form, pattern) = match query {
        Query::Select { pattern, .. } => ("select", pattern),
        Query::Construct { pattern, .. } => ("construct", pattern),
        Query::Ask { .. } => {
            return Err("an ASK answer is one boolean — there is nothing to split")
        }
        Query::Describe { .. } => {
            return Err("split a SELECT of the resources instead of a DESCRIBE")
        }
    };
    let mut s = Shape {
        form,
        offset: 0,
        limit: None,
        distinct: false,
        projected: None,
        ordered: false,
        group: None,
        computed: Vec::new(),
        where_: pattern,
    };
    let mut stage = 0;
    let mut p: &GraphPattern = pattern;
    loop {
        let (next, inner): (u8, &GraphPattern) = match p {
            GraphPattern::Slice {
                inner,
                start,
                length,
            } if stage < 1 => {
                s.offset = *start;
                s.limit = *length;
                (1, &**inner)
            }
            GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } if stage < 2 => {
                s.distinct = true;
                (2, &**inner)
            }
            GraphPattern::Project { inner, variables } if stage < 3 => {
                s.projected = Some(variables);
                (3, &**inner)
            }
            GraphPattern::OrderBy { inner, .. } if stage < 4 => {
                s.ordered = true;
                (4, &**inner)
            }
            GraphPattern::Extend {
                inner, variable, ..
            } if stage <= 5 => {
                s.computed.push(variable);
                (5, &**inner)
            }
            GraphPattern::Filter { inner, .. } if stage <= 5 => (5, &**inner),
            GraphPattern::Group {
                inner, variables, ..
            } if stage < 6 => {
                s.group = Some(variables);
                (6, &**inner)
            }
            _ => break,
        };
        stage = next;
        p = inner;
        if stage == 6 {
            break; // the group's inner is the WHERE pattern
        }
    }
    s.where_ = p;
    Ok(s)
}

/// Refuse a split that would change the answer. Shared by [`plan`] and the
/// chunk evaluation, so a chunk is never run on a query its plan refused.
pub(super) fn check(query: &Query, var: &str) -> Result<(), SparqlError> {
    checked(query, var).map(|_| ())
}

fn checked<'q>(query: &'q Query, var: &str) -> Result<Shape<'q>, SparqlError> {
    let s = shape(query).map_err(|r| refuse(var, r))?;
    if s.ordered {
        return Err(refuse(
            var,
            "ORDER BY needs every row at once. Drop it here and sort the result afterwards (or \
             sort each chunk's rows yourself)",
        ));
    }
    if let Some(by) = s.group {
        if !by.iter().any(|v| v.as_str() == var) {
            return Err(refuse(
                var,
                "an aggregate over more than one value of the variable would be computed per \
                 chunk. Split by a GROUP BY variable, or aggregate after collecting the rows",
            ));
        }
    }
    if s.computed.iter().any(|v| v.as_str() == var) {
        return Err(refuse(
            var,
            "the variable is computed by a select expression, not matched",
        ));
    }
    if !certain(s.where_).contains(var) {
        return Err(refuse(
            var,
            "some solutions may leave the variable unbound (it is matched only inside OPTIONAL, \
             one side of UNION, a BIND, a SERVICE or a sub-SELECT that hides it), and those rows \
             would belong to no chunk. Split by a variable every row has",
        ));
    }
    Ok(s)
}

/// Plan running `query` as chunks split by `?var`: the checks of [`check`],
/// then the keys query. Reads only the tile directory, to pick the cheapest
/// pattern to list the keys from — never tile data.
pub(super) fn plan(rete: &Rete, query: &str, var: &str) -> Result<PartitionPlan, SparqlError> {
    let var = var_name(var);
    if var.is_empty() || Variable::new(var).is_err() {
        return Err(SparqlError::Partition(format!(
            "by \"{var}\": not a variable name"
        )));
    }
    let parsed = parse_query(query)?;
    let s = checked(&parsed, var)?;
    let dataset = match &parsed {
        Query::Select { dataset, .. } | Query::Construct { dataset, .. } => dataset.clone(),
        _ => None,
    };
    let v = Variable::new_unchecked(var);

    let mut core = Vec::new();
    core_patterns(s.where_, var, &mut core);
    // The cheapest by tile bytes; with no estimate (a multi-graph FROM), the
    // first, as written.
    let cost = |tp: &SpTriplePattern| -> Option<u64> {
        let one = GraphPattern::Bgp {
            patterns: vec![tp.clone()],
        };
        let sel = lower_select(&one, &dataset).ok()?;
        scan_estimate(rete, &sel)
    };
    let best = core
        .iter()
        .min_by_key(|tp| cost(tp).unwrap_or(u64::MAX))
        .copied();
    let (keys_from, body) = match (values_keys(s.where_, var), best) {
        (Some(listed), _) => (
            "values",
            GraphPattern::Values {
                variables: vec![v.clone()],
                bindings: listed.into_iter().map(|t| vec![Some(t)]).collect(),
            },
        ),
        (None, Some(tp)) => (
            "pattern",
            GraphPattern::Bgp {
                patterns: vec![tp.clone()],
            },
        ),
        (None, None) => ("where", s.where_.clone()),
    };
    let keys_query = Query::Select {
        dataset,
        pattern: GraphPattern::Distinct {
            inner: Box::new(GraphPattern::Project {
                inner: Box::new(body),
                variables: vec![v],
            }),
        },
        base_iri: None,
    }
    .to_string();

    let projects_var = s
        .projected
        .is_none_or(|vs| vs.iter().any(|x| x.as_str() == var));
    Ok(PartitionPlan {
        var: var.to_string(),
        form: s.form,
        keys_query,
        keys_from,
        offset: s.offset,
        limit: s.limit,
        dedupe: s.form == "construct" || (s.distinct && !projects_var),
    })
}

/// The variables bound in EVERY solution of `p` — conservative: when unsure,
/// a variable is left out (and a split by it refused).
fn certain(p: &GraphPattern) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match p {
        GraphPattern::Bgp { patterns } => {
            for tp in patterns {
                term_vars(&tp.subject, &mut out);
                if let NamedNodePattern::Variable(v) = &tp.predicate {
                    out.insert(v.as_str().to_string());
                }
                term_vars(&tp.object, &mut out);
            }
        }
        GraphPattern::Path {
            subject, object, ..
        } => {
            term_vars(subject, &mut out);
            term_vars(object, &mut out);
        }
        GraphPattern::Join { left, right } => {
            out = certain(left);
            out.extend(certain(right));
        }
        GraphPattern::LeftJoin { left, .. } | GraphPattern::Minus { left, .. } => {
            out = certain(left)
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::OrderBy { inner, .. } => out = certain(inner),
        GraphPattern::Union { left, right } => {
            let r = certain(right);
            out = certain(left)
                .into_iter()
                .filter(|v| r.contains(v))
                .collect();
        }
        GraphPattern::Graph { name, inner } => {
            out = certain(inner);
            if let NamedNodePattern::Variable(v) = name {
                out.insert(v.as_str().to_string());
            }
        }
        GraphPattern::Values {
            variables,
            bindings,
        } => {
            for (i, v) in variables.iter().enumerate() {
                if bindings
                    .iter()
                    .all(|row| row.get(i).is_some_and(Option::is_some))
                {
                    out.insert(v.as_str().to_string());
                }
            }
        }
        // A sub-SELECT shows only what it projects.
        GraphPattern::Project { inner, variables } => {
            let c = certain(inner);
            out = variables
                .iter()
                .map(|v| v.as_str().to_string())
                .filter(|v| c.contains(v))
                .collect();
        }
        GraphPattern::Group {
            inner, variables, ..
        } => {
            let c = certain(inner);
            out = variables
                .iter()
                .map(|v| v.as_str().to_string())
                .filter(|v| c.contains(v))
                .collect();
        }
        // SERVICE answers from elsewhere; anything newer: nothing is certain.
        _ => {}
    }
    out
}

fn term_vars(t: &TermPattern, out: &mut BTreeSet<String>) {
    match t {
        TermPattern::Variable(v) => {
            out.insert(v.as_str().to_string());
        }
        TermPattern::Triple(tp) => {
            term_vars(&tp.subject, out);
            if let NamedNodePattern::Variable(v) = &tp.predicate {
                out.insert(v.as_str().to_string());
            }
            term_vars(&tp.object, out);
        }
        _ => {}
    }
}

/// The required triple patterns of `p` whose only variable is `var`: each
/// one must hold for every solution, so the values of `var` it matches are a
/// superset of the keys. Not looked for under GRAPH (the pattern means
/// something else outside it), UNION, OPTIONAL's or MINUS's right side, or a
/// sub-SELECT (whose `?var` may be another variable of the same name).
fn core_patterns<'q>(p: &'q GraphPattern, var: &str, out: &mut Vec<&'q SpTriplePattern>) {
    match p {
        GraphPattern::Bgp { patterns } => {
            for tp in patterns {
                let mut vars = BTreeSet::new();
                let mut blank = false;
                for t in [&tp.subject, &tp.object] {
                    term_vars(t, &mut vars);
                    blank |= has_blank(t);
                }
                if let NamedNodePattern::Variable(v) = &tp.predicate {
                    vars.insert(v.as_str().to_string());
                }
                if !blank && vars.len() == 1 && vars.contains(var) {
                    out.push(tp);
                }
            }
        }
        GraphPattern::Join { left, right } => {
            core_patterns(left, var, out);
            core_patterns(right, var, out);
        }
        GraphPattern::LeftJoin { left, .. }
        | GraphPattern::Minus { left, .. }
        | GraphPattern::Filter { inner: left, .. }
        | GraphPattern::Extend { inner: left, .. } => core_patterns(left, var, out),
        _ => {}
    }
}

/// The values of `var` in a required `VALUES` list that binds it in every row
/// (in order, without repeats): the solutions can take no other, so they are
/// the keys, listed without a read. The shortest such list when there are
/// several. Searched where [`core_patterns`] searches.
fn values_keys(p: &GraphPattern, var: &str) -> Option<Vec<GroundTerm>> {
    match p {
        GraphPattern::Values {
            variables,
            bindings,
        } => {
            let i = variables.iter().position(|v| v.as_str() == var)?;
            let mut seen = HashSet::new();
            let mut out = Vec::new();
            for row in bindings {
                let term = row.get(i)?.as_ref()?; // an UNDEF: not every row binds it
                if seen.insert(term.to_string()) {
                    out.push(term.clone());
                }
            }
            Some(out)
        }
        GraphPattern::Join { left, right } => {
            match (values_keys(left, var), values_keys(right, var)) {
                (Some(a), Some(b)) => Some(if a.len() <= b.len() { a } else { b }),
                (a, b) => a.or(b),
            }
        }
        GraphPattern::LeftJoin { left, .. }
        | GraphPattern::Minus { left, .. }
        | GraphPattern::Filter { inner: left, .. }
        | GraphPattern::Extend { inner: left, .. } => values_keys(left, var),
        _ => None,
    }
}

/// A blank node in a pattern is an unnamed variable.
fn has_blank(t: &TermPattern) -> bool {
    match t {
        TermPattern::BlankNode(_) => true,
        TermPattern::Triple(tp) => has_blank(&tp.subject) || has_blank(&tp.object),
        _ => false,
    }
}

/// `plan` with a chunk's `keys` seeded in. When the query has its own required
/// `VALUES` list on `var`, that list is cut down to the keys — the chunk is
/// then the query a caller would write for those keys alone. Otherwise
/// `Join(VALUES, bgp)` on the first required pattern block that mentions
/// `var`, where the engine's VALUES pushdown substitutes each key into the
/// scan; around the whole plan when no such block exists (still exact, as
/// `var` is certain — just a hash join).
pub(super) fn seed(plan: Plan, var: &str, keys: &[String]) -> Plan {
    if let Some(narrowed) = narrow_values(&plan, var, keys) {
        return narrowed;
    }
    let values = Plan::Values(
        vec![var.to_string()],
        keys.iter().map(|k| vec![Some(k.clone())]).collect(),
    );
    match seed_in(&plan, var, &values) {
        Some(seeded) => seeded,
        None => Plan::Join(Box::new(values), Box::new(plan)),
    }
}

/// The first required `VALUES` list on `var`, keeping only the rows whose
/// value is one of `keys`. Only when every key is found in it as written: a
/// key spelled otherwise (a literal escaped differently from the list), or
/// one the list lacks, leaves the seeding to [`seed_in`], which matches terms
/// rather than text.
fn narrow_values(plan: &Plan, var: &str, keys: &[String]) -> Option<Plan> {
    match plan {
        Plan::Values(vars, rows) => {
            let i = vars.iter().position(|v| v == var)?;
            let wanted: HashSet<&str> = keys.iter().map(String::as_str).collect();
            let kept: Vec<Vec<Option<String>>> = rows
                .iter()
                .filter(|row| cell(row, i).is_some_and(|t| wanted.contains(t)))
                .cloned()
                .collect();
            let found: HashSet<&str> = kept.iter().filter_map(|row| cell(row, i)).collect();
            (found.len() == wanted.len()).then(|| Plan::Values(vars.clone(), kept))
        }
        Plan::Join(l, r) => match narrow_values(l, var, keys) {
            Some(nl) => Some(Plan::Join(Box::new(nl), r.clone())),
            None => narrow_values(r, var, keys).map(|nr| Plan::Join(l.clone(), Box::new(nr))),
        },
        Plan::LeftJoin(l, r, c) => {
            narrow_values(l, var, keys).map(|nl| Plan::LeftJoin(Box::new(nl), r.clone(), c.clone()))
        }
        Plan::Minus(l, r) => {
            narrow_values(l, var, keys).map(|nl| Plan::Minus(Box::new(nl), r.clone()))
        }
        Plan::Filter(e, inner) => {
            narrow_values(inner, var, keys).map(|ni| Plan::Filter(e.clone(), Box::new(ni)))
        }
        Plan::Extend(v, e, inner) => narrow_values(inner, var, keys)
            .map(|ni| Plan::Extend(v.clone(), e.clone(), Box::new(ni))),
        _ => None,
    }
}

fn seed_in(plan: &Plan, var: &str, values: &Plan) -> Option<Plan> {
    match plan {
        Plan::Bgp(patterns) => patterns
            .iter()
            .any(|p| {
                [&p.s, &p.p, &p.o]
                    .into_iter()
                    .any(|t| matches!(t, PatternTerm::Var(v) if v == var))
            })
            .then(|| Plan::Join(Box::new(values.clone()), Box::new(plan.clone()))),
        Plan::Join(l, r) => match seed_in(l, var, values) {
            Some(nl) => Some(Plan::Join(Box::new(nl), r.clone())),
            None => seed_in(r, var, values).map(|nr| Plan::Join(l.clone(), Box::new(nr))),
        },
        Plan::LeftJoin(l, r, c) => {
            seed_in(l, var, values).map(|nl| Plan::LeftJoin(Box::new(nl), r.clone(), c.clone()))
        }
        Plan::Minus(l, r) => seed_in(l, var, values).map(|nl| Plan::Minus(Box::new(nl), r.clone())),
        Plan::Filter(e, inner) => {
            seed_in(inner, var, values).map(|ni| Plan::Filter(e.clone(), Box::new(ni)))
        }
        Plan::Extend(v, e, inner) => {
            seed_in(inner, var, values).map(|ni| Plan::Extend(v.clone(), e.clone(), Box::new(ni)))
        }
        _ => None,
    }
}

/// Row `row`'s value in column `i`, if bound.
fn cell(row: &[Option<String>], i: usize) -> Option<&str> {
    row.get(i).and_then(|t| t.as_deref())
}
