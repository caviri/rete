//! SPARQL parsing, lowering, and evaluation (SPEC.md §8).
//!
//! Queries are parsed with `spargebra`, lowered into a small plan algebra, and
//! evaluated against the `.rete` indexes. BGPs use the integer-space hash-join
//! engine in [`bgp`]; filters, joins, OPTIONAL, UNION, MINUS, VALUES, property
//! paths, named graphs, aggregates, and query forms are handled in the sibling
//! lowering/evaluation modules. Unsupported features are rejected explicitly
//! rather than silently dropped.
//!
//! [`bgp`]: crate::bgp

use spargebra::Query;

use crate::bgp::{Binding, PatternTerm, TriplePattern};
use crate::file::Rete;

mod aggregate;
mod eval;
mod expr;
mod lower;
mod path;

use eval::{ask_solution, instantiate, raw_solutions, run_select, run_select_communities};
use lower::{lower_pattern, lower_select};
pub use lower::{parse_select, query_predicates};
// Re-exported so the sibling modules' `use super::*` can reach the evaluator
// (expr's FILTER EXISTS evaluates a sub-plan via `eval_plan_in`).
pub(crate) use eval::eval_plan_in;

#[derive(Debug, thiserror::Error)]
pub enum SparqlError {
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported query feature: {0}")]
    Unsupported(&'static str),
}

/// A lowered SELECT query: solution modifiers plus the evaluation plan tree.
#[derive(Debug, Clone)]
pub struct Select {
    /// Projected variable names (empty = SELECT *).
    pub project: Vec<String>,
    /// DISTINCT (or REDUCED) requested.
    pub distinct: bool,
    /// OFFSET (solutions to skip).
    pub offset: usize,
    /// LIMIT (max solutions), if any.
    pub limit: Option<usize>,
    /// GROUP BY + aggregates, applied after the plan and before projection.
    pub group: Option<GroupSpec>,
    /// BIND/aggregate-alias assignments `(result_var, expr)`, applied after
    /// aggregation (e.g. `(COUNT(?f) AS ?n)` aliases an internal var to `?n`).
    pub extends: Vec<(String, FExpr)>,
    /// ORDER BY keys `(expr, descending)`, applied before projection/slice.
    pub order: Vec<(FExpr, bool)>,
    /// HAVING conditions, applied after aggregation.
    pub having: Vec<FExpr>,
    /// `FROM <iri>` graphs: when non-empty, the query's default graph is the
    /// union (RDF merge) of these named graphs.
    pub from: Vec<String>,
    /// `FROM NAMED <iri>` graphs: when `Some`, `GRAPH` may only see these.
    pub from_named: Option<Vec<String>>,
    /// The graph-pattern evaluation plan.
    pub plan: Plan,
}

impl Default for Select {
    fn default() -> Self {
        Select {
            project: Vec::new(),
            distinct: false,
            offset: 0,
            limit: None,
            group: None,
            extends: Vec::new(),
            order: Vec::new(),
            having: Vec::new(),
            from: Vec::new(),
            from_named: None,
            plan: Plan::Bgp(Vec::new()),
        }
    }
}

/// GROUP BY specification: grouping variables and result aggregates.
#[derive(Debug, Clone)]
pub struct GroupSpec {
    /// Variables to group by (empty = single group over all solutions).
    pub by: Vec<String>,
    /// `(result_variable, aggregate)` pairs.
    pub aggs: Vec<(String, Agg)>,
}

/// A supported aggregate function.
#[derive(Debug, Clone)]
pub enum Agg {
    /// COUNT(*) — number of solutions in the group.
    CountStar {
        distinct: bool,
    },
    /// COUNT(?v) — number of (optionally distinct) bound values.
    Count(String, bool),
    Sum(String),
    Avg(String),
    Min(String),
    Max(String),
    /// SAMPLE(?v) — any one value from the group.
    Sample(String),
    /// GROUP_CONCAT(\[DISTINCT\] ?v; SEPARATOR=...) — values joined by the
    /// separator (deduplicated when `distinct`).
    GroupConcat(String, String, bool),
}

/// A SPARQL graph-pattern evaluation plan (the supported algebra subset).
#[derive(Debug, Clone)]
pub enum Plan {
    /// Basic graph pattern: triple patterns joined on shared variables.
    Bgp(Vec<TriplePattern>),
    /// Conjunction of two patterns (inner join on shared variables).
    Join(Box<Plan>, Box<Plan>),
    /// UNION: all solutions of either side.
    Union(Box<Plan>, Box<Plan>),
    /// OPTIONAL (left join): left solutions, extended by the right where it
    /// matches (and passes the optional condition), kept as-is where it doesn't.
    LeftJoin(Box<Plan>, Box<Plan>, Option<FExpr>),
    /// FILTER over an inner pattern.
    Filter(FExpr, Box<Plan>),
    /// In-pattern `BIND(expr AS ?var)`: each inner solution extended with `?var`
    /// (left unbound where `expr` errors). Distinct from the projection-time
    /// alias list (`Select::extends`) — this one is *inside* the graph pattern,
    /// so a following FILTER or join sees the bound variable.
    Extend(String, FExpr, Box<Plan>),
    /// A property path `subject <path> object`.
    Path(PatternTerm, PathAst, PatternTerm),
    /// Inline `VALUES`: variable names and rows of optional ground-term tokens
    /// (`None` = UNDEF).
    Values(Vec<String>, Vec<Vec<Option<String>>>),
    /// `MINUS`: left solutions, minus those compatible with a right solution
    /// that shares at least one bound variable.
    Minus(Box<Plan>, Box<Plan>),
    /// `GRAPH <iri>|?g { … }` — evaluate the inner pattern against a named graph.
    Graph(GraphTarget, Box<Plan>),
    /// A nested `SELECT` subquery: evaluated independently to its projected
    /// solutions, which then join with the surrounding pattern on shared
    /// variables (only the subquery's projected variables are visible outside).
    Subquery(Box<Select>),
}

/// The target of a `GRAPH` block.
#[derive(Debug, Clone)]
pub enum GraphTarget {
    /// `GRAPH <iri>` — one specific named graph.
    Named(String),
    /// `GRAPH ?g` — every named graph, binding the variable to its IRI.
    Var(String),
}

/// Path repetition operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rep {
    /// exactly one hop (a plain predicate)
    One,
    /// `+` — one or more hops (transitive closure)
    OneOrMore,
    /// `*` — zero or more hops (reflexive-transitive closure)
    ZeroOrMore,
    /// `?` — zero or one hop
    ZeroOrOne,
}

/// A lowered property path expression, evaluated as a binary relation over the
/// graph's nodes.
#[derive(Debug, Clone)]
pub enum PathAst {
    /// A single predicate, optionally reversed.
    Pred(String, bool),
    /// Repetition (`*`/`+`/`?`) of a sub-path.
    Rep(Box<PathAst>, Rep),
    /// Sequence `a/b` (relational composition).
    Seq(Box<PathAst>, Box<PathAst>),
    /// Alternative `a|b` (union).
    Alt(Box<PathAst>, Box<PathAst>),
    /// Negated property set `!(p1|…|pn)` — one step over any predicate **not**
    /// in the set, in the given direction (`reversed` for the `^p` members,
    /// which `spargebra` wraps in a `Reverse`).
    NegatedSet(Vec<String>, bool),
}

/// Comparison operators supported in FILTER.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Arithmetic operators (numeric).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
}

/// Supported SPARQL built-in functions (the unambiguous subset over our
/// term-token model).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    Str,
    StrLen,
    UCase,
    LCase,
    Abs,
    Ceil,
    Floor,
    Round,
    Concat,
    SubStr,
    StrBefore,
    StrAfter,
    Contains,
    StrStarts,
    StrEnds,
    IsIri,
    IsBlank,
    IsLiteral,
    IsNumeric,
    Datatype,
    Lang,
    Regex,
    LangMatches,
    StrDt,
    StrLang,
    Iri,
    EncodeForUri,
    Replace,
    Md5,
    Sha1,
    Sha256,
    Sha384,
    Sha512,
    Year,
    Month,
    Day,
    Hours,
    Minutes,
    Seconds,
    Timezone,
    Tz,
    CastInteger,
    CastDecimal,
    CastFloat,
    CastDouble,
    CastBoolean,
    CastString,
}

/// A small boolean/comparison expression for FILTER (a subset of SPARQL exprs).
#[derive(Debug, Clone)]
pub enum FExpr {
    Var(String),
    /// A constant term token (IRI/literal) or numeric literal's text.
    Const(String),
    /// Numeric arithmetic on two sub-expressions.
    Arith(ArithOp, Box<FExpr>, Box<FExpr>),
    /// A built-in function call.
    Func(Builtin, Vec<FExpr>),
    /// `COALESCE(...)` — first sub-expression that yields a value.
    Coalesce(Vec<FExpr>),
    /// `IF(cond, then, else)` — evaluate `cond` as a boolean and pick a branch.
    If(Box<FExpr>, Box<FExpr>, Box<FExpr>),
    /// `expr IN (a, b, …)` — true if `expr` value-equals any list member.
    In(Box<FExpr>, Vec<FExpr>),
    /// `sameTerm(a, b)` — strict term identity (no value coercion).
    SameTerm(Box<FExpr>, Box<FExpr>),
    Compare(Op, Box<FExpr>, Box<FExpr>),
    And(Box<FExpr>, Box<FExpr>),
    Or(Box<FExpr>, Box<FExpr>),
    Not(Box<FExpr>),
    Bound(String),
    /// `EXISTS { … }` — true if the sub-pattern has a solution compatible with
    /// the current binding. (`NOT EXISTS` is `Not(Exists(..))`.)
    Exists(Box<Plan>),
}

/// Memoizes each EXISTS sub-plan's solutions (plus a lazily-built semi-join
/// index) within one filter application.
type ExistsCache = std::collections::HashMap<*const Plan, ExistsEntry>;

/// A cached EXISTS sub-plan: its solutions and a semi-join index built on first
/// probe, so repeated probes are O(1) instead of O(sols) — turning FILTER (NOT)
/// EXISTS over a BGP from O(L×R) into O(L+R), like the MINUS anti-join.
struct ExistsEntry {
    sols: Vec<crate::row::Row>,
    probe: Option<ExistsProbe>,
}

/// The semi-join index: solution rows keyed by the slots they share with the
/// probing rows. A probe `b` satisfies EXISTS iff some solution is compatible
/// with it (agrees on every shared slot).
struct ExistsProbe {
    /// All slots bound by some solution (ascending).
    svars: Vec<usize>,
    /// The shared slots with the probing rows (ascending) — the index key.
    jvars: Vec<usize>,
    /// `jvars`-value tuples of the solutions bound on all of `jvars`.
    keys: std::collections::HashSet<Vec<crate::row::Val>>,
    /// Solutions missing a `jvars` slot (e.g. via a nested OPTIONAL): scanned.
    partial: Vec<crate::row::Row>,
}

/// Build the semi-join index for `sols`, keyed by the slots shared with the
/// probe row `b`.
fn build_exists_probe(b: &crate::row::Row, sols: &[crate::row::Row]) -> ExistsProbe {
    let mask = crate::row::bound_mask(sols, b.len());
    let svars: Vec<usize> = (0..b.len()).filter(|&i| mask[i]).collect();
    let jvars: Vec<usize> = svars.iter().copied().filter(|&i| b[i].is_some()).collect();
    let mut keys = std::collections::HashSet::new();
    let mut partial = Vec::new();
    for s in sols {
        match jvars
            .iter()
            .map(|&i| s[i].clone())
            .collect::<Option<Vec<crate::row::Val>>>()
        {
            Some(k) => {
                keys.insert(k);
            }
            None => partial.push(s.clone()),
        }
    }
    ExistsProbe {
        svars,
        jvars,
        keys,
        partial,
    }
}

/// Does `b` satisfy the cached EXISTS? Uses the keyed index when `b`'s shared
/// slots match the index's `jvars` (the common, homogeneous case); otherwise
/// falls back to scanning all solutions (exact semantics on irregular rows).
fn exists_matches(b: &crate::row::Row, entry: &ExistsEntry) -> bool {
    let probe = entry.probe.as_ref().unwrap();
    let bj: Vec<usize> = probe
        .svars
        .iter()
        .copied()
        .filter(|&i| b[i].is_some())
        .collect();
    if bj == probe.jvars {
        let k: Vec<crate::row::Val> = probe.jvars.iter().map(|&i| b[i].clone().unwrap()).collect();
        probe.keys.contains(&k)
            || probe
                .partial
                .iter()
                .any(|s| crate::row::compatible_rows(b, s))
    } else {
        entry.sols.iter().any(|s| crate::row::compatible_rows(b, s))
    }
}

/// Lexical value of a term token: the text inside a literal's quotes, else the
/// IRI/blank-node text unchanged.
fn lexical(token: &str) -> String {
    if let Some(rest) = token.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return rest[..end].to_string();
        }
    }
    token.to_string()
}

/// Numeric value of a term: the lexical part of a literal (`"30"^^...` → 30) or
/// a bare numeric token, else `None`. (`term_number` is the crate-visible name
/// used by the row resolver's memoized parse.)
pub(crate) fn term_number(s: &str) -> Option<f64> {
    as_number(s)
}

use crate::terms::as_number;

/// Compare two term values numerically when both are numbers, else lexically.
fn compare(op: Op, a: &str, b: &str) -> bool {
    use std::cmp::Ordering;
    let ord = match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) => match x.partial_cmp(&y) {
            Some(o) => o,
            None => return false, // NaN
        },
        _ => a.cmp(b),
    };
    match op {
        Op::Eq => ord == Ordering::Equal,
        Op::Ne => ord != Ordering::Equal,
        Op::Lt => ord == Ordering::Less,
        Op::Le => ord != Ordering::Greater,
        Op::Gt => ord == Ordering::Greater,
        Op::Ge => ord != Ordering::Less,
    }
}

/// The result of evaluating any SPARQL query form.
#[derive(Debug, Clone)]
pub enum QueryOutput {
    /// SELECT: projected variables and their solution rows.
    Select(Vec<String>, Vec<Binding>),
    /// ASK: whether the pattern has any solution.
    Ask(bool),
    /// CONSTRUCT: the constructed triples as `(s, p, o)` term tokens.
    Construct(Vec<(String, String, String)>),
}

/// Query shapes that can be answered exactly from [`SummaryView`] predicate
/// totals, without opening the triple index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SummaryQueryShape {
    /// `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }`
    PredicateCount { predicate: String, variable: String },
    /// `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`
    TripleCount { variable: String },
    /// `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`
    PredicateTotals {
        predicate_variable: String,
        count_variable: String,
    },
    /// `SELECT DISTINCT ?p WHERE { ?s ?p ?o }`
    PredicateList { variable: String },
    /// `SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`
    PredicateDistinctCount { variable: String },
    /// `ASK { ?s ?p ?o }`
    TripleExists,
    /// `ASK { ?s <p> ?o }`
    PredicateExists { predicate: String },
}

/// A single triple pattern that can be answered by the range-routed permutation
/// reader. `None` means the position is a variable/wildcard; `Some(term)` means
/// the query pins that term.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoutedTriplePattern {
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
}

/// Classify queries whose graph access is exactly one default-graph triple
/// pattern. Solution modifiers (projection, LIMIT, aggregate wrappers) do not
/// change the underlying range access, but named graphs, FROM, joins, filters,
/// paths, and other algebra need the full SPARQL evaluator.
pub fn routed_triple_pattern(query: &str) -> Result<Option<RoutedTriplePattern>, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    let sel = match parsed {
        Query::Select {
            pattern, dataset, ..
        } => lower_select(&pattern, &dataset)?,
        Query::Ask { pattern, .. } => lower_pattern(&pattern)?,
        Query::Construct { pattern, .. } => lower_pattern(&pattern)?,
        Query::Describe { pattern, .. } => lower_pattern(&pattern)?,
    };
    if !sel.from.is_empty() || sel.from_named.is_some() {
        return Ok(None);
    }
    let Plan::Bgp(patterns) = sel.plan else {
        return Ok(None);
    };
    let [tp] = patterns.as_slice() else {
        return Ok(None);
    };
    Ok(Some(RoutedTriplePattern {
        subject: term_const(&tp.s),
        predicate: term_const(&tp.p),
        object: term_const(&tp.o),
    }))
}

fn term_const(term: &PatternTerm) -> Option<String> {
    match term {
        PatternTerm::Const(t) => Some(t.clone()),
        PatternTerm::Var(_) => None,
    }
}

/// Classify SPARQL queries that can be answered exactly from the pyramid
/// summary's per-predicate totals. This is intentionally conservative: anything
/// with constants, repeated variables, filters, joins, paths, named graphs,
/// ORDER BY, OFFSET/LIMIT, or non-summary-safe aggregates still requires the
/// index. The only accepted DISTINCT shape is a predicate list over one fully
/// unbound triple pattern.
pub fn summary_query_shape(query: &str) -> Result<Option<SummaryQueryShape>, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    match parsed {
        Query::Select {
            pattern, dataset, ..
        } => {
            let sel = lower_select(&pattern, &dataset)?;
            if !sel.from.is_empty()
                || sel.from_named.is_some()
                || sel.offset != 0
                || sel.limit.is_some()
                || !sel.order.is_empty()
                || !sel.having.is_empty()
            {
                return Ok(None);
            }
            if sel.distinct {
                if sel.group.is_some() || !sel.extends.is_empty() {
                    return Ok(None);
                }
                let [projected] = sel.project.as_slice() else {
                    return Ok(None);
                };
                let Some(SummaryPatternShape::AnyPredicate { variable }) =
                    single_summary_pattern(&sel.plan)
                else {
                    return Ok(None);
                };
                return if projected == &variable {
                    Ok(Some(SummaryQueryShape::PredicateList { variable }))
                } else {
                    Ok(None)
                };
            }
            let Some(group) = &sel.group else {
                return Ok(None);
            };
            if group.aggs.len() != 1 {
                return Ok(None);
            }
            match group.by.as_slice() {
                [] => match &group.aggs[0].1 {
                    Agg::CountStar { distinct: false } => {
                        let Some(variable) = public_aggregate_variable(&sel, &group.aggs[0].0)
                        else {
                            return Ok(None);
                        };
                        Ok(single_summary_pattern(&sel.plan).map(|shape| match shape {
                            SummaryPatternShape::Predicate(predicate) => {
                                SummaryQueryShape::PredicateCount {
                                    predicate,
                                    variable,
                                }
                            }
                            SummaryPatternShape::AnyPredicate { .. } => {
                                SummaryQueryShape::TripleCount { variable }
                            }
                        }))
                    }
                    Agg::Count(counted, true) => {
                        let Some(public_variable) =
                            public_aggregate_variable(&sel, &group.aggs[0].0)
                        else {
                            return Ok(None);
                        };
                        let Some(SummaryPatternShape::AnyPredicate { variable }) =
                            single_summary_pattern(&sel.plan)
                        else {
                            return Ok(None);
                        };
                        if &variable != counted {
                            return Ok(None);
                        }
                        Ok(Some(SummaryQueryShape::PredicateDistinctCount {
                            variable: public_variable,
                        }))
                    }
                    _ => Ok(None),
                },
                [group_var] => {
                    if !matches!(group.aggs[0].1, Agg::CountStar { distinct: false }) {
                        return Ok(None);
                    }
                    let Some(SummaryPatternShape::AnyPredicate { variable }) =
                        single_summary_pattern(&sel.plan)
                    else {
                        return Ok(None);
                    };
                    if &variable != group_var {
                        return Ok(None);
                    }
                    let Some(count_variable) =
                        public_group_aggregate_variable(&sel, &group.aggs[0].0, group_var)
                    else {
                        return Ok(None);
                    };
                    Ok(Some(SummaryQueryShape::PredicateTotals {
                        predicate_variable: group_var.clone(),
                        count_variable,
                    }))
                }
                _ => Ok(None),
            }
        }
        Query::Ask { pattern, .. } => {
            let sel = lower_pattern(&pattern)?;
            Ok(single_summary_pattern(&sel.plan).map(|shape| match shape {
                SummaryPatternShape::Predicate(predicate) => {
                    SummaryQueryShape::PredicateExists { predicate }
                }
                SummaryPatternShape::AnyPredicate { .. } => SummaryQueryShape::TripleExists,
            }))
        }
        Query::Construct { .. } | Query::Describe { .. } => Ok(None),
    }
}

enum SummaryPatternShape {
    Predicate(String),
    AnyPredicate { variable: String },
}

fn single_summary_pattern(plan: &Plan) -> Option<SummaryPatternShape> {
    let Plan::Bgp(patterns) = plan else {
        return None;
    };
    let [tp] = patterns.as_slice() else {
        return None;
    };
    let (PatternTerm::Var(s), PatternTerm::Var(o)) = (&tp.s, &tp.o) else {
        return None;
    };
    if s == o {
        return None;
    }
    match &tp.p {
        PatternTerm::Const(p) => Some(SummaryPatternShape::Predicate(p.clone())),
        PatternTerm::Var(p) if p != s && p != o => Some(SummaryPatternShape::AnyPredicate {
            variable: p.clone(),
        }),
        _ => None,
    }
}

fn public_aggregate_variable(sel: &Select, aggregate_var: &str) -> Option<String> {
    let [projected] = sel.project.as_slice() else {
        return None;
    };
    if projected == aggregate_var {
        return Some(projected.clone());
    }
    sel.extends.iter().find_map(|(var, expr)| match expr {
        FExpr::Var(source) if var == projected && source == aggregate_var => Some(var.clone()),
        _ => None,
    })
}

fn public_group_aggregate_variable(
    sel: &Select,
    aggregate_var: &str,
    group_var: &str,
) -> Option<String> {
    let [projected_group, projected_aggregate] = sel.project.as_slice() else {
        return None;
    };
    if projected_group != group_var {
        return None;
    }
    if projected_aggregate == aggregate_var {
        return Some(projected_aggregate.clone());
    }
    sel.extends.iter().find_map(|(var, expr)| match expr {
        FExpr::Var(source) if var == projected_aggregate && source == aggregate_var => {
            Some(var.clone())
        }
        _ => None,
    })
}

/// One community's contribution to a community-split evaluation: how many
/// member subjects it holds and how many solution rows it produced.
#[derive(Debug, Clone, Copy)]
pub struct CommunityPartial {
    pub community: usize,
    pub subjects: usize,
    pub rows: usize,
}

/// The outcome of a community-split SELECT: the projected variables, the
/// merged solution rows, and each community's contribution.
pub type CommunitySelect = (Vec<String>, Vec<Binding>, Vec<CommunityPartial>);

/// Evaluate a SELECT **per pyramid community**, then merge: each community's
/// subjects are pushed into the plan as a VALUES binding, the partial rows
/// are concatenated, and the solution modifiers (GROUP BY / ORDER BY / LIMIT
/// / DISTINCT) run once on the union — so the rows are identical to
/// [`eval_query`]'s answer. Sound only for subject-star queries over the
/// default graph (every triple pattern sharing one subject variable; FILTERs
/// allowed); anything else returns [`SparqlError::Unsupported`] rather than a
/// possibly-wrong split answer. `round` picks the dendrogram granularity
/// (`None` = the build's tile-budget round). Also returns each community's
/// subject and row counts for display.
pub fn eval_select_communities(
    rete: &Rete,
    query: &str,
    round: Option<usize>,
) -> Result<CommunitySelect, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    match parsed {
        Query::Select {
            pattern, dataset, ..
        } => run_select_communities(rete, &lower_select(&pattern, &dataset)?, round),
        _ => Err(SparqlError::Unsupported(
            "community-split evaluation supports SELECT queries only",
        )),
    }
}

/// Evaluate any supported SPARQL query form (SELECT / ASK / CONSTRUCT).
pub fn eval_query(rete: &Rete, query: &str) -> Result<QueryOutput, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    match parsed {
        Query::Select {
            pattern, dataset, ..
        } => {
            let (vars, rows) = run_select(rete, &lower_select(&pattern, &dataset)?);
            Ok(QueryOutput::Select(vars, rows))
        }
        Query::Ask { pattern, .. } => {
            let sel = lower_pattern(&pattern)?;
            Ok(QueryOutput::Ask(ask_solution(rete, &sel)))
        }
        Query::Construct {
            template, pattern, ..
        } => {
            let sel = lower_pattern(&pattern)?;
            let (ctx, sols) = raw_solutions(rete, &sel);
            Ok(QueryOutput::Construct(instantiate(&ctx, &template, &sols)))
        }
        Query::Describe {
            pattern, dataset, ..
        } => {
            // The projected variables' values are the resources to describe;
            // we return each one's outgoing triples (concise bounded description).
            let sel = lower_select(&pattern, &dataset)?;
            let (ctx, rows) = raw_solutions(rete, &sel);
            let mut resources = std::collections::BTreeSet::new();
            for row in &rows {
                if sel.project.is_empty() {
                    for val in row.iter().flatten() {
                        if let Some(t) = ctx.resolver.str_of(val) {
                            resources.insert(t.to_string());
                        }
                    }
                } else {
                    for v in &sel.project {
                        if let Some(val) = ctx.slots.slot(v).and_then(|s| row[s].as_ref()) {
                            if let Some(t) = ctx.resolver.str_of(val) {
                                resources.insert(t.to_string());
                            }
                        }
                    }
                }
            }
            let mut triples = std::collections::BTreeSet::new();
            for r in &resources {
                for t in rete.query(Some(r), None, None) {
                    triples.insert(t);
                }
            }
            Ok(QueryOutput::Construct(triples.into_iter().collect()))
        }
    }
}

/// Parse and evaluate a SELECT against a file, applying the plan then
/// projection, DISTINCT, OFFSET, and LIMIT. Returns `(projected_vars,
/// solutions)`.
pub fn eval_sparql(rete: &Rete, query: &str) -> Result<(Vec<String>, Vec<Binding>), SparqlError> {
    let sel = parse_select(query)?;
    Ok(run_select(rete, &sel))
}

/// Format a computed number as an N-Triples *typed* literal so the result
/// serializer emits its datatype (SPARQL requires arithmetic/aggregates/numeric
/// functions to yield typed numerics, not bare strings). Whole values are
/// `xsd:integer`; fractional ones `xsd:decimal` — the common cases in the data
/// (`xsd:double` would need operand-type tracking we don't carry through `f64`).
pub(crate) fn fmt_num_typed(x: f64) -> String {
    if x.fract() == 0.0 {
        format!(
            "\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            x as i64
        )
    } else {
        // Round to 15 significant digits before emitting the shortest form, so a
        // sum/avg of decimals that lands on a binary-float artifact (e.g.
        // 11.100000000000001) serializes as the intended "11.1".
        let cleaned: f64 = format!("{x:.14e}").parse().unwrap_or(x);
        format!("\"{cleaned}\"^^<http://www.w3.org/2001/XMLSchema#decimal>")
    }
}

/// Push a reverse through a path (reverses each predicate and swaps sequences).
fn reverse(ast: PathAst) -> PathAst {
    match ast {
        PathAst::Pred(p, r) => PathAst::Pred(p, !r),
        PathAst::Rep(inner, rep) => PathAst::Rep(Box::new(reverse(*inner)), rep),
        PathAst::Seq(a, b) => PathAst::Seq(Box::new(reverse(*b)), Box::new(reverse(*a))),
        PathAst::Alt(a, b) => PathAst::Alt(Box::new(reverse(*a)), Box::new(reverse(*b))),
        PathAst::NegatedSet(s, r) => PathAst::NegatedSet(s, !r),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::index::GraphIndexBuilder;
    use crate::write_file;

    fn rete_from(triples: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        write_file(&dict, &ib.build(), false, &[], 0)
    }

    #[test]
    fn parses_select_with_two_patterns() {
        let q = r#"
            PREFIX ex: <http://ex/>
            SELECT ?x ?z WHERE { ?x ex:knows ?y . ?y ex:knows ?z }
        "#;
        let sel = parse_select(q).unwrap();
        assert_eq!(sel.project, vec!["x", "z"]);
        match &sel.plan {
            Plan::Bgp(p) => assert_eq!(p.len(), 2),
            other => panic!("expected a BGP plan, got {other:?}"),
        }
    }

    #[test]
    fn evaluates_two_hop_select() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = r#"PREFIX ex: <http://ex/>
                   SELECT ?x ?z WHERE { ?x ex:knows ?y . ?y ex:knows ?z }"#;
        let (proj, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(proj, vec!["x", "z"]);
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["x"], "<http://ex/Alice>");
        assert_eq!(sols[0]["z"], "<http://ex/Carol>");
    }

    #[test]
    fn union_returns_both_sides() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/likes>", "<http://ex/Tea>"),
            ("<http://ex/Bob>", "<http://ex/hates>", "<http://ex/Tea>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?p WHERE { \
                 { ?p ex:likes ex:Tea } UNION { ?p ex:hates ex:Tea } }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut who: Vec<&str> = sols.iter().map(|b| b["p"].as_str()).collect();
        who.sort();
        assert_eq!(who, vec!["<http://ex/Alice>", "<http://ex/Bob>"]);
    }

    #[test]
    fn optional_keeps_left_when_right_absent() {
        // Alice has an email, Bob doesn't. OPTIONAL email keeps both people.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/name>", "\"Alice\""),
            ("<http://ex/Bob>", "<http://ex/name>", "\"Bob\""),
            ("<http://ex/Alice>", "<http://ex/email>", "\"a@ex\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?p ?e WHERE { \
                 ?p ex:name ?n . OPTIONAL { ?p ex:email ?e } }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 2, "both people present");
        let alice = sols.iter().find(|b| b["p"] == "<http://ex/Alice>").unwrap();
        assert_eq!(alice["e"], "\"a@ex\"");
        let bob = sols.iter().find(|b| b["p"] == "<http://ex/Bob>").unwrap();
        assert!(!bob.contains_key("e"), "Bob has no email binding");
    }

    #[test]
    fn numeric_filter_on_typed_literal() {
        // ages 30 and 25; FILTER(?age > 27) keeps only Alice.
        let xsd = "<http://www.w3.org/2001/XMLSchema#integer>";
        let bytes = rete_from(&[
            (
                "<http://ex/Alice>",
                "<http://ex/age>",
                &format!("\"30\"^^{xsd}"),
            ),
            (
                "<http://ex/Bob>",
                "<http://ex/age>",
                &format!("\"25\"^^{xsd}"),
            ),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?p WHERE { ?p ex:age ?age . FILTER(?age > 27) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["p"], "<http://ex/Alice>");
    }

    #[test]
    fn filter_equality_and_boolean_logic() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/city>", "<http://ex/NYC>"),
            ("<http://ex/Bob>", "<http://ex/city>", "<http://ex/LA>"),
            ("<http://ex/Carol>", "<http://ex/city>", "<http://ex/NYC>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?p WHERE { ?p ex:city ?c . FILTER(?c = ex:NYC && ?p != ex:Carol) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["p"], "<http://ex/Alice>");
    }

    #[test]
    fn distinct_collapses_duplicate_projections() {
        // Dave is reachable in two hops via both Bob and Carol.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Dave>"),
            ("<http://ex/Carol>", "<http://ex/knows>", "<http://ex/Dave>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let base = "PREFIX ex: <http://ex/> SELECT {} ?z WHERE { ?x ex:knows ?y . ?y ex:knows ?z }";

        let (_, non_distinct) = eval_sparql(&rete, &base.replace("{}", "")).unwrap();
        assert_eq!(non_distinct.len(), 2); // {z:Dave} twice

        let (proj, distinct) = eval_sparql(&rete, &base.replace("{}", "DISTINCT")).unwrap();
        assert_eq!(proj, vec!["z"]);
        assert_eq!(distinct.len(), 1);
        assert_eq!(distinct[0]["z"], "<http://ex/Dave>");
        // Projection dropped ?x and ?y.
        assert!(!distinct[0].contains_key("x"));
    }

    #[test]
    fn transitive_path_reachability() {
        // A -> B -> C -> D chain.
        let bytes = rete_from(&[
            ("<http://ex/A>", "<http://ex/k>", "<http://ex/B>"),
            ("<http://ex/B>", "<http://ex/k>", "<http://ex/C>"),
            ("<http://ex/C>", "<http://ex/k>", "<http://ex/D>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();

        // A k+ ?y  → B, C, D (one or more hops).
        let q = "PREFIX ex: <http://ex/> SELECT ?y WHERE { ex:A ex:k+ ?y }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut ys: Vec<&str> = sols.iter().map(|b| b["y"].as_str()).collect();
        ys.sort();
        assert_eq!(ys, vec!["<http://ex/B>", "<http://ex/C>", "<http://ex/D>"]);

        // A k* ?y includes A itself (zero-length).
        let q0 = "PREFIX ex: <http://ex/> SELECT ?y WHERE { ex:A ex:k* ?y }";
        let (_, s0) = eval_sparql(&rete, q0).unwrap();
        assert!(s0.iter().any(|b| b["y"] == "<http://ex/A>"));
        assert_eq!(s0.len(), 4); // A, B, C, D
    }

    #[test]
    fn sequence_and_alternative_paths() {
        // Alice -parent-> Bob -parent-> Carol; Alice -stepparent-> Dave.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/parent>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/parent>", "<http://ex/Carol>"),
            (
                "<http://ex/Alice>",
                "<http://ex/stepparent>",
                "<http://ex/Dave>",
            ),
        ]);
        let rete = Rete::open(&bytes).unwrap();

        // grandparent = parent/parent : Alice -> Carol.
        let q = "PREFIX ex: <http://ex/> SELECT ?g WHERE { ex:Alice ex:parent/ex:parent ?g }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["g"], "<http://ex/Carol>");

        // any-parent = parent|stepparent : Alice -> {Bob, Dave}.
        let qa = "PREFIX ex: <http://ex/> \
                  SELECT ?p WHERE { ex:Alice ex:parent|ex:stepparent ?p }";
        let (_, sa) = eval_sparql(&rete, qa).unwrap();
        let mut ps: Vec<&str> = sa.iter().map(|b| b["p"].as_str()).collect();
        ps.sort();
        assert_eq!(ps, vec!["<http://ex/Bob>", "<http://ex/Dave>"]);
    }

    #[test]
    fn group_concat_aggregate() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> \
                 SELECT (GROUP_CONCAT(?f; SEPARATOR=\"|\") AS ?fs) WHERE { ex:Alice ex:knows ?f }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        // GROUP_CONCAT yields a simple literal — strip the quotes before splitting.
        let fs = sols[0]["fs"].trim_matches('"');
        let mut parts: Vec<&str> = fs.split('|').collect();
        parts.sort();
        assert_eq!(parts, vec!["<http://ex/Bob>", "<http://ex/Carol>"]);
    }

    #[test]
    fn group_by_having() {
        // Alice knows 2, Bob knows 1.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // Only people who know more than one person → Alice.
        let q = "PREFIX ex: <http://ex/> SELECT ?p (COUNT(?f) AS ?n) \
                 WHERE { ?p ex:knows ?f } GROUP BY ?p HAVING (COUNT(?f) > 1)";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["p"], "<http://ex/Alice>");
        assert_eq!(
            sols[0]["n"],
            "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
    }

    #[test]
    fn count_group_by() {
        // Alice knows Bob & Carol (2); Bob knows Carol (1).
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p ex:knows ?f } GROUP BY ?p";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut counts: Vec<(String, String)> = sols
            .iter()
            .map(|b| (b["p"].clone(), b["n"].clone()))
            .collect();
        counts.sort();
        assert_eq!(
            counts,
            vec![
                (
                    "<http://ex/Alice>".into(),
                    "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>".into()
                ),
                (
                    "<http://ex/Bob>".into(),
                    "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>".into()
                ),
            ]
        );
    }

    #[test]
    fn global_count_star() {
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"),
            ("<http://ex/b>", "<http://ex/p>", "<http://ex/2>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let (_, sols) = eval_sparql(&rete, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }").unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(
            sols[0]["n"],
            "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
    }

    #[test]
    fn summary_query_shape_classifies_only_exact_predicate_totals() {
        let count = summary_query_shape(
            "PREFIX ex: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s ex:p ?o }",
        )
        .unwrap();
        assert_eq!(
            count,
            Some(SummaryQueryShape::PredicateCount {
                predicate: "<http://ex/p>".into(),
                variable: "n".into(),
            })
        );

        let total = summary_query_shape("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }").unwrap();
        assert_eq!(
            total,
            Some(SummaryQueryShape::TripleCount {
                variable: "n".into(),
            })
        );

        let by_pred =
            summary_query_shape("SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p")
                .unwrap();
        assert_eq!(
            by_pred,
            Some(SummaryQueryShape::PredicateTotals {
                predicate_variable: "p".into(),
                count_variable: "n".into(),
            })
        );

        let predicates = summary_query_shape("SELECT DISTINCT ?p WHERE { ?s ?p ?o }").unwrap();
        assert_eq!(
            predicates,
            Some(SummaryQueryShape::PredicateList {
                variable: "p".into(),
            })
        );

        let predicate_count =
            summary_query_shape("SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }").unwrap();
        assert_eq!(
            predicate_count,
            Some(SummaryQueryShape::PredicateDistinctCount {
                variable: "n".into(),
            })
        );

        let ask = summary_query_shape("PREFIX ex: <http://ex/> ASK { ?s ex:p ?o }").unwrap();
        assert_eq!(
            ask,
            Some(SummaryQueryShape::PredicateExists {
                predicate: "<http://ex/p>".into(),
            })
        );

        let any_ask = summary_query_shape("ASK { ?s ?p ?o }").unwrap();
        assert_eq!(any_ask, Some(SummaryQueryShape::TripleExists));

        let constrained =
            summary_query_shape("PREFIX ex: <http://ex/> ASK { ex:a ex:p ?o }").unwrap();
        assert_eq!(constrained, None);

        let filtered =
            summary_query_shape("PREFIX ex: <http://ex/> ASK { ?s ex:p ?o FILTER(?s = ?o) }")
                .unwrap();
        assert_eq!(filtered, None);
    }

    #[test]
    fn filter_exists_and_not_exists() {
        // Alice knows Bob & Carol; Bob knows Dave; Carol knows nobody.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Dave>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();

        // Friends of Alice who themselves know someone → Bob.
        let q_exists = "PREFIX ex: <http://ex/> SELECT ?f WHERE { \
            ex:Alice ex:knows ?f . FILTER EXISTS { ?f ex:knows ?x } }";
        let (_, e) = eval_sparql(&rete, q_exists).unwrap();
        assert_eq!(
            e.iter().map(|b| b["f"].as_str()).collect::<Vec<_>>(),
            vec!["<http://ex/Bob>"]
        );

        // Friends of Alice who know nobody → Carol.
        let q_not = "PREFIX ex: <http://ex/> SELECT ?f WHERE { \
            ex:Alice ex:knows ?f . FILTER NOT EXISTS { ?f ex:knows ?x } }";
        let (_, n) = eval_sparql(&rete, q_not).unwrap();
        assert_eq!(
            n.iter().map(|b| b["f"].as_str()).collect::<Vec<_>>(),
            vec!["<http://ex/Carol>"]
        );
    }

    #[test]
    fn ask_repeated_variable_pattern() {
        // ASK's fast path must NOT take the single-pattern scan shortcut when a
        // variable repeats across positions (`?x knows ?x`) — only Bob self-knows.
        let yes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Bob>"),
        ]);
        let rete = Rete::open(&yes).unwrap();
        match eval_query(&rete, "PREFIX ex: <http://ex/> ASK { ?x ex:knows ?x }").unwrap() {
            QueryOutput::Ask(b) => assert!(b, "Bob knows himself"),
            other => panic!("expected Ask, got {other:?}"),
        }
        // With no self-edge, the index still has a `knows` triple, so a naive
        // first-match probe would wrongly say true; the guard must reject it.
        let no = rete_from(&[("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>")]);
        let rete = Rete::open(&no).unwrap();
        match eval_query(&rete, "PREFIX ex: <http://ex/> ASK { ?x ex:knows ?x }").unwrap() {
            QueryOutput::Ask(b) => assert!(!b, "nobody knows themselves"),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn minus_excludes_compatible() {
        // Alice knows Bob and Carol; Bob knows Carol; Carol knows no one.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // People Alice knows who themselves know nobody → Carol.
        let q = "PREFIX ex: <http://ex/> SELECT ?f WHERE { \
                 ex:Alice ex:knows ?f . MINUS { ?f ex:knows ?x } }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let fs: Vec<&str> = sols.iter().map(|b| b["f"].as_str()).collect();
        assert_eq!(fs, vec!["<http://ex/Carol>"]);
    }

    #[test]
    fn filter_exists_disjoint_variable() {
        // EXISTS over a sub-pattern that shares NO variable with the outer row is
        // true iff the sub-pattern has any solution — so NOT EXISTS removes ALL
        // rows (the exact case where NOT EXISTS differs from MINUS). Guards the
        // semi-join index's empty-`jvars` path.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/a>", "<http://ex/Person>"),
            ("<http://ex/Bob>", "<http://ex/a>", "<http://ex/Person>"),
            ("<http://ex/Tea>", "<http://ex/a>", "<http://ex/Drink>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q_none = "PREFIX ex: <http://ex/> SELECT ?x WHERE { \
            ?x ex:a ex:Person FILTER NOT EXISTS { ?y ex:a ex:Drink } }";
        assert!(eval_sparql(&rete, q_none).unwrap().1.is_empty());
        let q_all = "PREFIX ex: <http://ex/> SELECT ?x WHERE { \
            ?x ex:a ex:Person FILTER EXISTS { ?y ex:a ex:Drink } }";
        assert_eq!(eval_sparql(&rete, q_all).unwrap().1.len(), 2);
    }

    #[test]
    fn minus_disjoint_domain_keeps_all() {
        // MINUS with no shared variable must remove nothing (SPARQL semantics) —
        // the hash anti-join's `jv.is_empty()` guard. Alice/Bob both kept even
        // though the right pattern has solutions.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/a>", "<http://ex/Person>"),
            ("<http://ex/Bob>", "<http://ex/a>", "<http://ex/Person>"),
            ("<http://ex/Tea>", "<http://ex/a>", "<http://ex/Drink>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?x WHERE { \
                 ?x ex:a ex:Person MINUS { ?y ex:a ex:Drink } }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut xs: Vec<&str> = sols.iter().map(|b| b["x"].as_str()).collect();
        xs.sort();
        assert_eq!(xs, vec!["<http://ex/Alice>", "<http://ex/Bob>"]);
    }

    #[test]
    fn values_pushdown_selects_subset() {
        // VALUES with several rows pushes each into the scan; the result must be
        // exactly the union (here: two of three disciplines).
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/d>", "<http://ex/Bio>"),
            ("<http://ex/b>", "<http://ex/d>", "<http://ex/Phys>"),
            ("<http://ex/c>", "<http://ex/d>", "<http://ex/Chem>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?p ?disc WHERE { \
            VALUES ?disc { ex:Bio ex:Phys } ?p ex:d ?disc }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut got: Vec<(String, String)> = sols
            .iter()
            .map(|b| (b["p"].clone(), b["disc"].clone()))
            .collect();
        got.sort();
        assert_eq!(
            got,
            vec![
                ("<http://ex/a>".into(), "<http://ex/Bio>".into()),
                ("<http://ex/b>".into(), "<http://ex/Phys>".into()),
            ]
        );
    }

    #[test]
    fn values_inline_data_joins() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            (
                "<http://ex/Alice>",
                "<http://ex/knows>",
                "<http://ex/Carol>",
            ),
            ("<http://ex/Dave>", "<http://ex/knows>", "<http://ex/Eve>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // Restrict to Alice's friends via VALUES.
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?f WHERE { VALUES ?p { ex:Alice } ?p ex:knows ?f }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut fs: Vec<&str> = sols.iter().map(|b| b["f"].as_str()).collect();
        fs.sort();
        assert_eq!(fs, vec!["<http://ex/Bob>", "<http://ex/Carol>"]);
    }

    #[test]
    fn graph_queries_over_named_graphs() {
        use crate::write_dataset;
        // Shared dict; social graph has knows edges, profile graph has ages.
        let mut db = DictionaryBuilder::new();
        let edges = [
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/age>", "\"25\""),
        ];
        for (s, p, o) in edges {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut social = GraphIndexBuilder::new();
        social.push(dict.encode(edges[0].0, edges[0].1, edges[0].2).unwrap());
        let mut profile = GraphIndexBuilder::new();
        profile.push(dict.encode(edges[1].0, edges[1].1, edges[1].2).unwrap());
        let named = vec![
            ("<http://ex/social>".to_string(), social.build()),
            ("<http://ex/profile>".to_string(), profile.build()),
        ];
        let bytes = write_dataset(
            &dict,
            &GraphIndexBuilder::new().build(),
            &named,
            true,
            &[],
            0,
        );
        let rete = Rete::open(&bytes).unwrap();

        // GRAPH <iri>: knows edge only in the social graph.
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?f WHERE { GRAPH ex:social { ex:Alice ex:knows ?f } }";
        let (_, s) = eval_sparql(&rete, q).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["f"], "<http://ex/Bob>");

        // GRAPH ?g: which graph holds the age triple?
        let q2 = "PREFIX ex: <http://ex/> \
                  SELECT ?g WHERE { GRAPH ?g { ex:Bob ex:age ?a } }";
        let (_, s2) = eval_sparql(&rete, q2).unwrap();
        assert_eq!(s2.len(), 1);
        assert_eq!(s2[0]["g"], "<http://ex/profile>");

        // EXISTS inside GRAPH evaluates in that graph: the `age` triple exists in
        // the profile graph but NOT in the social graph.
        let q3 = "PREFIX ex: <http://ex/> SELECT ?f WHERE { \
                  GRAPH ex:profile { ?f ex:age ?a . FILTER EXISTS { ?f ex:age ?a2 } } }";
        assert_eq!(eval_sparql(&rete, q3).unwrap().1.len(), 1);
        let q4 = "PREFIX ex: <http://ex/> SELECT ?s WHERE { \
                  GRAPH ex:social { ?s ex:knows ?o . FILTER NOT EXISTS { ?s ex:age ?a } } }";
        // In the social graph, no `age` triples exist → NOT EXISTS keeps the row.
        assert_eq!(eval_sparql(&rete, q4).unwrap().1.len(), 1);
    }

    #[test]
    fn from_unions_named_graphs() {
        use crate::write_dataset;
        // social graph: Alice knows Bob. profile graph: Bob knows Carol.
        // A join spanning both only works if FROM merges them into the default.
        let mut db = DictionaryBuilder::new();
        let t = [
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ];
        for (s, p, o) in t {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode(t[0].0, t[0].1, t[0].2).unwrap());
        let mut g2 = GraphIndexBuilder::new();
        g2.push(dict.encode(t[1].0, t[1].1, t[1].2).unwrap());
        let named = vec![
            ("<http://ex/social>".to_string(), g1.build()),
            ("<http://ex/profile>".to_string(), g2.build()),
        ];
        let bytes = write_dataset(
            &dict,
            &GraphIndexBuilder::new().build(),
            &named,
            true,
            &[],
            0,
        );
        let rete = Rete::open(&bytes).unwrap();

        // Without FROM the default graph is empty → no join.
        let q0 = "PREFIX ex: <http://ex/> SELECT ?z WHERE { ?x ex:knows ?y . ?y ex:knows ?z }";
        assert!(eval_sparql(&rete, q0).unwrap().1.is_empty());

        // FROM both graphs → the cross-graph join (Alice→Bob→Carol) succeeds.
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?z FROM ex:social FROM ex:profile \
                 WHERE { ?x ex:knows ?y . ?y ex:knows ?z }";
        let (_, s) = eval_sparql(&rete, q).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0]["z"], "<http://ex/Carol>");

        // FROM NAMED restricts GRAPH ?g to the listed graph only.
        let qn = "PREFIX ex: <http://ex/> SELECT ?g FROM NAMED ex:social \
                  WHERE { GRAPH ?g { ?x ex:knows ?y } }";
        let (_, sn) = eval_sparql(&rete, qn).unwrap();
        let gs: Vec<&str> = sn.iter().map(|b| b["g"].as_str()).collect();
        assert_eq!(gs, vec!["<http://ex/social>"]); // profile excluded
    }

    #[test]
    fn describe_resource() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Alice>", "<http://ex/age>", "\"30\""),
            ("<http://ex/Bob>", "<http://ex/age>", "\"25\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // DESCRIBE <Alice> → Alice's two outgoing triples.
        match eval_query(&rete, "DESCRIBE <http://ex/Alice>").unwrap() {
            QueryOutput::Construct(t) => assert_eq!(t.len(), 2),
            other => panic!("expected Construct, got {other:?}"),
        }
        // DESCRIBE ?x WHERE { ?x ex:age ?a } → describes Alice and Bob.
        let q = "PREFIX ex: <http://ex/> DESCRIBE ?x WHERE { ?x ex:age ?a }";
        match eval_query(&rete, q).unwrap() {
            QueryOutput::Construct(t) => {
                // Alice has 2 triples, Bob has 1 → 3 total.
                assert_eq!(t.len(), 3);
            }
            other => panic!("expected Construct, got {other:?}"),
        }
    }

    #[test]
    fn ask_and_construct() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
            ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Carol>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();

        // ASK: is there any knows edge? yes; a likes edge? no.
        match eval_query(&rete, "PREFIX ex: <http://ex/> ASK { ?a ex:knows ?b }").unwrap() {
            QueryOutput::Ask(b) => assert!(b),
            other => panic!("expected Ask, got {other:?}"),
        }
        match eval_query(&rete, "PREFIX ex: <http://ex/> ASK { ?a ex:likes ?b }").unwrap() {
            QueryOutput::Ask(b) => assert!(!b),
            other => panic!("expected Ask, got {other:?}"),
        }

        // CONSTRUCT a reverse `knownBy` graph.
        let q = "PREFIX ex: <http://ex/> \
                 CONSTRUCT { ?b ex:knownBy ?a } WHERE { ?a ex:knows ?b }";
        match eval_query(&rete, q).unwrap() {
            QueryOutput::Construct(mut triples) => {
                triples.sort();
                assert_eq!(triples.len(), 2);
                assert!(triples.contains(&(
                    "<http://ex/Bob>".into(),
                    "<http://ex/knownBy>".into(),
                    "<http://ex/Alice>".into(),
                )));
            }
            other => panic!("expected Construct, got {other:?}"),
        }
    }

    #[test]
    fn substr_strbefore_strafter() {
        let bytes = rete_from(&[("<http://ex/a>", "<http://ex/name>", "\"Alice Smith\"")]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?first ?last ?ini WHERE { \
            ?p ex:name ?n . \
            BIND(STRBEFORE(?n, \" \") AS ?first) \
            BIND(STRAFTER(?n, \" \") AS ?last) \
            BIND(SUBSTR(?n, 1, 1) AS ?ini) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        // String built-ins return proper literal terms (quoted), not bare text.
        assert_eq!(sols[0]["first"], "\"Alice\"");
        assert_eq!(sols[0]["last"], "\"Smith\"");
        assert_eq!(sols[0]["ini"], "\"A\"");
    }

    #[test]
    fn concat_and_coalesce() {
        // Alice has a nickname, Bob doesn't.
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/name>", "\"Alice\""),
            ("<http://ex/Alice>", "<http://ex/nick>", "\"Al\""),
            ("<http://ex/Bob>", "<http://ex/name>", "\"Bob\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // COALESCE falls back to name when nick is unbound; CONCAT builds a label.
        let q = "PREFIX ex: <http://ex/> SELECT ?label WHERE { \
            ?p ex:name ?name . OPTIONAL { ?p ex:nick ?nick } \
            BIND(CONCAT(\"@\", COALESCE(?nick, ?name)) AS ?label) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut labels: Vec<&str> = sols.iter().map(|b| b["label"].as_str()).collect();
        labels.sort();
        assert_eq!(labels, vec!["\"@Al\"", "\"@Bob\""]);
    }

    #[test]
    fn builtin_functions() {
        let bytes = rete_from(&[
            ("<http://ex/Alice>", "<http://ex/name>", "\"Alice Smith\""),
            ("<http://ex/Bob>", "<http://ex/name>", "\"Bob Jones\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // CONTAINS on the literal value, and STRLEN as a computed value.
        let q = "PREFIX ex: <http://ex/> SELECT ?p ?len WHERE { \
            ?p ex:name ?n . FILTER(CONTAINS(?n, \"Smith\")) BIND(STRLEN(?n) AS ?len) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["p"], "<http://ex/Alice>");
        assert_eq!(
            sols[0]["len"],
            "\"11\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        ); // "Alice Smith"
    }

    #[test]
    fn bind_arithmetic() {
        let xsd = "<http://www.w3.org/2001/XMLSchema#integer>";
        let bytes = rete_from(&[(
            "<http://ex/a>",
            "<http://ex/age>",
            &format!("\"30\"^^{xsd}"),
        )]);
        let rete = Rete::open(&bytes).unwrap();
        // BIND a computed value, and FILTER on arithmetic.
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?next WHERE { ?p ex:age ?age . BIND(?age + 1 AS ?next) FILTER(?age * 2 > 50) }";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(
            sols[0]["next"],
            "\"31\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
    }

    #[test]
    fn bind_value_is_visible_to_a_following_filter_and_join() {
        // A BIND inside the WHERE pattern must be evaluated before a *following*
        // FILTER (and join) can reference it — the bound var is in-tree, not a
        // projection-time alias.
        let xsd = "<http://www.w3.org/2001/XMLSchema#integer>";
        let n = |v: i32| format!("\"{v}\"^^{xsd}");
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/v>", &n(1)),
            ("<http://ex/b>", "<http://ex/v>", &n(2)),
            ("<http://ex/c>", "<http://ex/v>", &n(3)),
            // a node whose :v equals b's value+1, for the join case
            ("<http://ex/x>", "<http://ex/v>", &n(3)),
        ]);
        let rete = Rete::open(&bytes).unwrap();

        // FILTER references the BIND'd ?z.
        let q1 = "PREFIX ex: <http://ex/> SELECT ?s WHERE { \
            ?s ex:v ?o . BIND(?o + 1 AS ?z) FILTER(?z = 3) }";
        let (_, s1) = eval_sparql(&rete, q1).unwrap();
        assert_eq!(s1.len(), 1, "only b (2+1=3) passes");
        assert_eq!(s1[0]["s"], "<http://ex/b>");

        // A following triple pattern joins on the BIND'd ?z.
        let q2 = "PREFIX ex: <http://ex/> SELECT ?s ?s2 WHERE { \
            ?s ex:v ?o . BIND(?o + 1 AS ?z) ?s2 ex:v ?z }";
        let (_, s2) = eval_sparql(&rete, q2).unwrap();
        // a (1→2) joins b (:v 2); b (2→3) joins c and x (:v 3); c (3→4) no match.
        let mut pairs: Vec<(String, String)> = s2
            .iter()
            .map(|b| (b["s"].clone(), b["s2"].clone()))
            .collect();
        pairs.sort();
        assert_eq!(
            pairs,
            vec![
                ("<http://ex/a>".to_string(), "<http://ex/b>".to_string()),
                ("<http://ex/b>".to_string(), "<http://ex/c>".to_string()),
                ("<http://ex/b>".to_string(), "<http://ex/x>".to_string()),
            ]
        );
    }

    #[test]
    fn order_by_numeric_desc_then_limit() {
        let xsd = "<http://www.w3.org/2001/XMLSchema#integer>";
        let bytes = rete_from(&[
            (
                "<http://ex/a>",
                "<http://ex/age>",
                &format!("\"30\"^^{xsd}"),
            ),
            (
                "<http://ex/b>",
                "<http://ex/age>",
                &format!("\"25\"^^{xsd}"),
            ),
            (
                "<http://ex/c>",
                "<http://ex/age>",
                &format!("\"40\"^^{xsd}"),
            ),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // Oldest two, descending by age.
        let q = "PREFIX ex: <http://ex/> \
                 SELECT ?p WHERE { ?p ex:age ?age } ORDER BY DESC(?age) LIMIT 2";
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let ps: Vec<&str> = sols.iter().map(|b| b["p"].as_str()).collect();
        assert_eq!(ps, vec!["<http://ex/c>", "<http://ex/a>"]); // 40, 30
    }

    #[test]
    fn limit_early_out_two_hop_join() {
        // A→B, B→C, B→D, C→E. Two-hop join (?x k ?y . ?y k ?z) has 3 solutions:
        // (A,B,C), (A,B,D), (B,C,E). The LIMIT early-out must preserve the count
        // contract and only ever yield genuine solutions.
        let bytes = rete_from(&[
            ("<http://ex/A>", "<http://ex/k>", "<http://ex/B>"),
            ("<http://ex/B>", "<http://ex/k>", "<http://ex/C>"),
            ("<http://ex/B>", "<http://ex/k>", "<http://ex/D>"),
            ("<http://ex/C>", "<http://ex/k>", "<http://ex/E>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?x ?z WHERE { ?x ex:k ?y . ?y ex:k ?z }";
        let (_, full) = eval_sparql(&rete, q).unwrap();
        assert_eq!(full.len(), 3);

        let (_, one) = eval_sparql(&rete, &format!("{q} LIMIT 1")).unwrap();
        assert_eq!(one.len(), 1);
        // The early-out row must be a real solution of the full query.
        assert!(one.iter().all(|r| full.contains(r)));

        // LIMIT above the total returns everything; OFFSET composes.
        let (_, all) = eval_sparql(&rete, &format!("{q} LIMIT 100")).unwrap();
        assert_eq!(all.len(), 3);
        let (_, off) = eval_sparql(&rete, &format!("{q} LIMIT 100 OFFSET 2")).unwrap();
        assert_eq!(off.len(), 1);
    }

    #[test]
    fn limit_early_out_filter_over_bgp() {
        // FILTER over a BGP under LIMIT streams and stops early; the result must
        // match the unlimited filtered query's prefix.
        let xsd = "<http://www.w3.org/2001/XMLSchema#integer>";
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/n>", &format!("\"10\"^^{xsd}")),
            ("<http://ex/b>", "<http://ex/n>", &format!("\"20\"^^{xsd}")),
            ("<http://ex/c>", "<http://ex/n>", &format!("\"30\"^^{xsd}")),
            ("<http://ex/d>", "<http://ex/n>", &format!("\"40\"^^{xsd}")),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let q = "PREFIX ex: <http://ex/> SELECT ?p WHERE { ?p ex:n ?v FILTER(?v > 15) }";
        let (_, full) = eval_sparql(&rete, q).unwrap();
        assert_eq!(full.len(), 3); // b, c, d
        let (_, two) = eval_sparql(&rete, &format!("{q} LIMIT 2")).unwrap();
        assert_eq!(two.len(), 2);
        assert!(two.iter().all(|r| full.contains(r)));
    }

    #[test]
    fn distinct_bgp_fast_matches_general() {
        // a/b both →x, b→y, c→z. The integer-DISTINCT fast path must collapse on
        // the projection and apply OFFSET/LIMIT after dedup.
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/p>", "<http://ex/x>"),
            ("<http://ex/b>", "<http://ex/p>", "<http://ex/x>"),
            ("<http://ex/b>", "<http://ex/p>", "<http://ex/y>"),
            ("<http://ex/c>", "<http://ex/p>", "<http://ex/z>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        // DISTINCT ?o → {x, y, z} = 3 (the two ?s→x rows collapse).
        let q = "PREFIX ex: <http://ex/> SELECT DISTINCT ?o WHERE { ?s ex:p ?o }";
        let (proj, sols) = eval_sparql(&rete, q).unwrap();
        assert_eq!(proj, vec!["o"]);
        let mut os: Vec<&str> = sols.iter().map(|b| b["o"].as_str()).collect();
        os.sort();
        assert_eq!(os, vec!["<http://ex/x>", "<http://ex/y>", "<http://ex/z>"]);
        // LIMIT applies after dedup.
        assert_eq!(
            eval_sparql(&rete, &format!("{q} LIMIT 2")).unwrap().1.len(),
            2
        );
        // A 2-var DISTINCT keeps every (s, o) pair → 4 rows.
        let q2 = "PREFIX ex: <http://ex/> SELECT DISTINCT ?s ?o WHERE { ?s ex:p ?o }";
        assert_eq!(eval_sparql(&rete, q2).unwrap().1.len(), 4);
    }

    #[test]
    fn limit_caps_solutions() {
        let bytes = rete_from(&[
            ("<http://ex/a>", "<http://ex/p>", "<http://ex/1>"),
            ("<http://ex/b>", "<http://ex/p>", "<http://ex/2>"),
            ("<http://ex/c>", "<http://ex/p>", "<http://ex/3>"),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let (_, sols) =
            eval_sparql(&rete, "SELECT ?x WHERE { ?x <http://ex/p> ?y } LIMIT 2").unwrap();
        assert_eq!(sols.len(), 2);
    }
}
