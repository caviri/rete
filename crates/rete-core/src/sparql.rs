//! SPARQL `SELECT` → BGP translation (SPEC.md §8).
//!
//! Parses a query with `spargebra` and lowers its algebra into the [`bgp`]
//! engine. v0 supports SELECT over a basic graph pattern (plus the structural
//! wrappers `Project`/`Join`/`Distinct`/`Reduced`/`OrderBy`/`Slice`/`Extend`,
//! whose triple patterns are collected). FILTER/OPTIONAL/UNION and aggregation
//! are later stages and are reported as unsupported rather than silently
//! dropped.
//!
//! [`bgp`]: crate::bgp

use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression,
    PropertyPathExpression,
};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern};
use spargebra::Query;

use crate::bgp::{eval_bgp_int_in, term_of_value, Binding, IntBinding, PatternTerm, TriplePattern};
use crate::file::Rete;
use crate::index::{GraphIndex, GraphIndexBuilder};
use spargebra::algebra::QueryDataset;

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
    /// GROUP_CONCAT(?v; SEPARATOR=...) — values joined by the separator.
    GroupConcat(String, String),
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
    Regex,
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
    sols: Vec<Binding>,
    probe: Option<ExistsProbe>,
}

/// The semi-join index: solution rows keyed by the variables they share with the
/// probing rows. A probe `b` satisfies EXISTS iff some solution is compatible
/// with it (agrees on every shared variable).
struct ExistsProbe {
    /// All variables bound by some solution (sorted).
    svars: Vec<String>,
    /// The shared variables with the probing rows (sorted) — the index key.
    jvars: Vec<String>,
    /// `jvars`-value tuples of the solutions bound on all of `jvars`.
    keys: std::collections::HashSet<Vec<String>>,
    /// Solutions missing a `jvars` variable (e.g. via a nested OPTIONAL): scanned.
    partial: Vec<Binding>,
}

/// Build the semi-join index for `sols`, keyed by the variables shared with the
/// probe row `b`.
fn build_exists_probe(b: &Binding, sols: &[Binding]) -> ExistsProbe {
    let mut svset: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for s in sols {
        svset.extend(s.keys().map(String::as_str));
    }
    let svars: Vec<String> = svset.iter().map(|s| s.to_string()).collect();
    let jvars: Vec<String> = svars
        .iter()
        .filter(|v| b.contains_key(*v))
        .cloned()
        .collect();
    let mut keys = std::collections::HashSet::new();
    let mut partial = Vec::new();
    for s in sols {
        if jvars.iter().all(|v| s.contains_key(v)) {
            keys.insert(jvars.iter().map(|v| s[v].clone()).collect::<Vec<String>>());
        } else {
            partial.push(s.clone());
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
/// variables match the index's `jvars` (the common, homogeneous case); otherwise
/// falls back to scanning all solutions (exact semantics on irregular rows).
fn exists_matches(b: &Binding, entry: &ExistsEntry) -> bool {
    let probe = entry.probe.as_ref().unwrap();
    let bj: Vec<String> = probe
        .svars
        .iter()
        .filter(|v| b.contains_key(*v))
        .cloned()
        .collect();
    if bj == probe.jvars {
        let k: Vec<String> = probe.jvars.iter().map(|v| b[v].clone()).collect();
        probe.keys.contains(&k) || probe.partial.iter().any(|s| compatible(b, s))
    } else {
        entry.sols.iter().any(|s| compatible(b, s))
    }
}

impl FExpr {
    /// Evaluate to a value string against a binding (variables resolve, errors
    /// yield `None`).
    fn value(&self, b: &Binding) -> Option<String> {
        match self {
            FExpr::Var(v) => b.get(v).cloned(),
            FExpr::Const(c) => Some(c.clone()),
            FExpr::Arith(op, l, r) => {
                let a = as_number(&l.value(b)?)?;
                let c = as_number(&r.value(b)?)?;
                let v = match op {
                    ArithOp::Add => a + c,
                    ArithOp::Sub => a - c,
                    ArithOp::Mul => a * c,
                    ArithOp::Div if c == 0.0 => return None,
                    ArithOp::Div => a / c,
                };
                Some(fmt_num(v))
            }
            FExpr::Func(f, args) => func_value(*f, args, b),
            FExpr::Coalesce(args) => args.iter().find_map(|e| e.value(b)),
            _ => None,
        }
    }

    /// Evaluate as a boolean (SPARQL effective boolean value, simplified:
    /// unbound/error → false). `index` is the active graph (so EXISTS evaluates
    /// in the current GRAPH context).
    fn boolean(
        &self,
        rete: &Rete,
        index: &GraphIndex,
        b: &Binding,
        cache: &mut ExistsCache,
    ) -> bool {
        match self {
            FExpr::Bound(v) => b.contains_key(v),
            FExpr::Not(e) => !e.boolean(rete, index, b, cache),
            FExpr::And(l, r) => {
                l.boolean(rete, index, b, cache) && r.boolean(rete, index, b, cache)
            }
            FExpr::Or(l, r) => l.boolean(rete, index, b, cache) || r.boolean(rete, index, b, cache),
            FExpr::Compare(op, l, r) => match (l.value(b), r.value(b)) {
                (Some(a), Some(c)) => compare(*op, &a, &c),
                _ => false,
            },
            FExpr::Exists(plan) => {
                let key = plan.as_ref() as *const Plan;
                let entry = cache.entry(key).or_insert_with(|| ExistsEntry {
                    sols: eval_plan_in(rete, index, None, plan),
                    probe: None,
                });
                if entry.sols.is_empty() {
                    return false;
                }
                if entry.probe.is_none() {
                    entry.probe = Some(build_exists_probe(b, &entry.sols));
                }
                exists_matches(b, entry)
            }
            FExpr::Func(f, args) => func_bool(*f, args, b),
            _ => false,
        }
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

/// Evaluate a value-returning built-in (string/numeric); `None` for predicates.
fn func_value(f: Builtin, args: &[FExpr], b: &Binding) -> Option<String> {
    let a0 = || args.first().and_then(|e| e.value(b));
    let num = |x: f64| Some(fmt_num(x));
    match f {
        Builtin::Str => Some(lexical(&a0()?)),
        Builtin::StrLen => num(lexical(&a0()?).chars().count() as f64),
        Builtin::UCase => Some(lexical(&a0()?).to_uppercase()),
        Builtin::LCase => Some(lexical(&a0()?).to_lowercase()),
        Builtin::Abs => num(as_number(&a0()?)?.abs()),
        Builtin::Ceil => num(as_number(&a0()?)?.ceil()),
        Builtin::Floor => num(as_number(&a0()?)?.floor()),
        Builtin::Round => num(as_number(&a0()?)?.round()),
        Builtin::Concat => {
            let mut s = String::new();
            for a in args {
                s.push_str(&lexical(&a.value(b)?));
            }
            Some(s)
        }
        Builtin::SubStr => {
            let s: Vec<char> = lexical(&a0()?).chars().collect();
            let start = as_number(&args.get(1)?.value(b)?)?.max(1.0) as usize - 1;
            let it = s.iter().skip(start);
            let out: String = match args.get(2) {
                Some(lenarg) => it
                    .take(as_number(&lenarg.value(b)?)?.max(0.0) as usize)
                    .collect(),
                None => it.collect(),
            };
            Some(out)
        }
        Builtin::StrBefore => {
            let s = lexical(&a0()?);
            let sub = lexical(&args.get(1)?.value(b)?);
            Some(s.find(&sub).map(|i| s[..i].to_string()).unwrap_or_default())
        }
        Builtin::StrAfter => {
            let s = lexical(&a0()?);
            let sub = lexical(&args.get(1)?.value(b)?);
            Some(
                s.find(&sub)
                    .map(|i| s[i + sub.len()..].to_string())
                    .unwrap_or_default(),
            )
        }
        _ => None,
    }
}

/// Evaluate a boolean built-in (type checks / string predicates).
fn func_bool(f: Builtin, args: &[FExpr], b: &Binding) -> bool {
    let val = |i: usize| args.get(i).and_then(|e| e.value(b));
    let two = |g: fn(&str, &str) -> bool| match (val(0), val(1)) {
        (Some(a), Some(c)) => g(&lexical(&a), &lexical(&c)),
        _ => false,
    };
    match f {
        Builtin::IsIri => val(0).is_some_and(|t| t.starts_with('<')),
        Builtin::IsBlank => val(0).is_some_and(|t| t.starts_with("_:")),
        Builtin::IsLiteral => val(0).is_some_and(|t| t.starts_with('"')),
        Builtin::IsNumeric => val(0).and_then(|t| as_number(&t)).is_some(),
        Builtin::Contains => two(|a, c| a.contains(c)),
        Builtin::StrStarts => two(|a, c| a.starts_with(c)),
        Builtin::StrEnds => two(|a, c| a.ends_with(c)),
        // REGEX(text, pattern [, flags]) — SPARQL flags i/m/s/x map to inline
        // regex flags. An invalid pattern yields no match rather than erroring.
        Builtin::Regex => match (val(0), val(1)) {
            (Some(text), Some(pat)) => {
                let flags = val(2).map(|t| lexical(&t)).unwrap_or_default();
                let mut inline = String::new();
                let on: String = ['i', 'm', 's', 'x']
                    .iter()
                    .filter(|c| flags.contains(**c))
                    .collect();
                if !on.is_empty() {
                    inline = format!("(?{on})");
                }
                regex_lite::Regex::new(&format!("{inline}{}", lexical(&pat)))
                    .map(|re| re.is_match(&lexical(&text)))
                    .unwrap_or(false)
            }
            _ => false,
        },
        _ => false,
    }
}

/// Two bindings are compatible when they agree on every shared variable.
fn compatible(a: &Binding, b: &Binding) -> bool {
    a.iter()
        .all(|(k, v)| b.get(k).map(|w| w == v).unwrap_or(true))
}

/// Numeric value of a term: the lexical part of a literal (`"30"^^...` → 30) or
/// a bare numeric token, else `None`.
fn as_number(s: &str) -> Option<f64> {
    let lex = if let Some(rest) = s.strip_prefix('"') {
        &rest[..rest.find('"')?]
    } else {
        s
    };
    lex.parse::<f64>().ok()
}

/// Ordering of two term values: numeric when both are numbers, else lexical.
fn cmp_values(a: &str, b: &str) -> std::cmp::Ordering {
    match (as_number(a), as_number(b)) {
        (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
        _ => a.cmp(b),
    }
}

/// Ordering for ORDER BY: unbound (`None`) sorts before bound values.
fn cmp_opt(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, _) => Ordering::Less,
        (_, None) => Ordering::Greater,
        (Some(x), Some(y)) => cmp_values(x, y),
    }
}

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

/// Lower a graph pattern (with its solution modifiers) into a [`Select`].
fn lower_pattern(pattern: &GraphPattern) -> Result<Select, SparqlError> {
    let mut sel = Select::default();
    let plan = build(pattern, &mut sel)?;
    sel.plan = plan;
    Ok(sel)
}

/// Lower a SELECT's pattern + dataset clause into a [`Select`].
fn lower_select(
    pattern: &GraphPattern,
    dataset: &Option<QueryDataset>,
) -> Result<Select, SparqlError> {
    let mut sel = lower_pattern(pattern)?;
    if let Some(ds) = dataset {
        sel.from = ds.default.iter().map(|n| n.to_string()).collect();
        sel.from_named = ds
            .named
            .as_ref()
            .map(|gs| gs.iter().map(|n| n.to_string()).collect());
    }
    Ok(sel)
}

/// Parse a SPARQL `SELECT` query and lower it to a [`Select`].
pub fn parse_select(query: &str) -> Result<Select, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    match parsed {
        Query::Select {
            pattern, dataset, ..
        } => lower_select(&pattern, &dataset),
        _ => Err(SparqlError::Unsupported("only SELECT is supported")),
    }
}

/// Collect the **concrete predicate IRIs** a query constrains on — i.e. every
/// IRI that appears in the predicate position of a triple pattern, or as a plain
/// predicate inside a property path. Variable predicates (`?p`) and the special
/// `a` (`rdf:type`) keyword are normalized to their IRI tokens (`<…>`).
///
/// This is what `rete federate` uses to prune shards: a source whose predicate
/// set is disjoint from this set cannot contribute a row and can be skipped.
/// Returns an empty set when the query pins no concrete predicate (e.g. every
/// pattern uses a variable predicate) — callers should then query every source.
pub fn query_predicates(query: &str) -> Result<std::collections::BTreeSet<String>, SparqlError> {
    let parsed = Query::parse(query, None).map_err(|e| SparqlError::Parse(e.to_string()))?;
    let mut preds = std::collections::BTreeSet::new();
    let pattern = match &parsed {
        Query::Select { pattern, .. } => pattern,
        Query::Ask { pattern, .. } => pattern,
        Query::Construct { pattern, .. } => pattern,
        Query::Describe { pattern, .. } => pattern,
    };
    collect_pattern_predicates(pattern, &mut preds);
    Ok(preds)
}

/// Walk a `GraphPattern`, adding every concrete predicate IRI to `out`.
fn collect_pattern_predicates(p: &GraphPattern, out: &mut std::collections::BTreeSet<String>) {
    match p {
        GraphPattern::Bgp { patterns } => {
            for tp in patterns {
                if let NamedNodePattern::NamedNode(n) = &tp.predicate {
                    out.insert(n.to_string());
                }
            }
        }
        GraphPattern::Path {
            path: PropertyPathExpression::NamedNode(n),
            ..
        } => {
            out.insert(n.to_string());
        }
        GraphPattern::Path { path, .. } => collect_path_predicates(path, out),
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            collect_pattern_predicates(left, out);
            collect_pattern_predicates(right, out);
        }
        GraphPattern::LeftJoin { left, right, .. } => {
            collect_pattern_predicates(left, out);
            collect_pattern_predicates(right, out);
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::Extend { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Project { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Group { inner, .. }
        | GraphPattern::Service { inner, .. }
        | GraphPattern::Graph { inner, .. } => collect_pattern_predicates(inner, out),
        _ => {}
    }
}

/// Walk a (non-plain) property-path expression for its concrete predicate IRIs.
fn collect_path_predicates(
    path: &PropertyPathExpression,
    out: &mut std::collections::BTreeSet<String>,
) {
    match path {
        PropertyPathExpression::NamedNode(n) => {
            out.insert(n.to_string());
        }
        PropertyPathExpression::Reverse(inner)
        | PropertyPathExpression::ZeroOrMore(inner)
        | PropertyPathExpression::OneOrMore(inner)
        | PropertyPathExpression::ZeroOrOne(inner) => collect_path_predicates(inner, out),
        PropertyPathExpression::Sequence(a, b) | PropertyPathExpression::Alternative(a, b) => {
            collect_path_predicates(a, out);
            collect_path_predicates(b, out);
        }
        PropertyPathExpression::NegatedPropertySet(_) => {}
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
            let sols = raw_solutions(rete, &sel);
            Ok(QueryOutput::Construct(instantiate(&template, &sols)))
        }
        Query::Describe {
            pattern, dataset, ..
        } => {
            // The projected variables' values are the resources to describe;
            // we return each one's outgoing triples (concise bounded description).
            let sel = lower_select(&pattern, &dataset)?;
            let rows = raw_solutions(rete, &sel);
            let mut resources = std::collections::BTreeSet::new();
            for row in &rows {
                if sel.project.is_empty() {
                    resources.extend(row.values().cloned());
                } else {
                    for v in &sel.project {
                        if let Some(val) = row.get(v) {
                            resources.insert(val.clone());
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

/// Evaluate an `ASK`: does the query have any solution? Streams and stops at the
/// first solution for the common shapes; defers to the full evaluator only where
/// a solution's existence depends on aggregation/HAVING/post-aggregate aliases.
fn ask_solution(rete: &Rete, sel: &Select) -> bool {
    // A grouped query always yields at least one group (so ASK over it hinges on
    // HAVING); BIND aliases may be referenced by HAVING. These need the full
    // aggregate path — fall back to materializing.
    if sel.group.is_some() || !sel.having.is_empty() || !sel.extends.is_empty() {
        return !raw_solutions(rete, sel).is_empty();
    }
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    plan_exists(rete, active, sel.from_named.as_deref(), &sel.plan)
}

/// Does `plan` have at least one solution against `index`? Streams the
/// streamable shapes and stops at the first solution; falls back to eager
/// evaluation (testing non-emptiness) for shapes that need a full join/filter —
/// which still benefit from the now-lazy per-pattern scan.
fn plan_exists(rete: &Rete, index: &GraphIndex, nf: Option<&[String]>, plan: &Plan) -> bool {
    match plan {
        Plan::Bgp(patterns) => crate::bgp::bgp_exists(rete, index, patterns),
        Plan::Union(l, r) => plan_exists(rete, index, nf, l) || plan_exists(rete, index, nf, r),
        Plan::Values(_, rows) => !rows.is_empty(),
        _ => !eval_plan_in(rete, index, nf, plan).is_empty(),
    }
}

/// The cap (OFFSET + LIMIT) for a pure-LIMIT early-out, or `None` if the query
/// has an ORDER BY / DISTINCT / aggregate / HAVING that must see the full
/// solution set first. BIND (`extends`) is allowed: it adds columns without
/// dropping rows, so it can be applied to the capped prefix afterwards.
fn early_out_cap(sel: &Select) -> Option<usize> {
    if sel.order.is_empty() && !sel.distinct && sel.group.is_none() && sel.having.is_empty() {
        sel.limit.map(|l| sel.offset.saturating_add(l))
    } else {
        None
    }
}

/// Evaluate `plan` to at most `cap` solutions, stopping early where the shape
/// allows (BGP join, FILTER-over-BGP, UNION, VALUES); other shapes fall back to
/// full evaluation truncated to `cap` (correct, just no early-out). Sound only
/// when the caller has no ORDER BY/DISTINCT/aggregate (see [`early_out_cap`]), so
/// any `cap`-sized prefix of solutions is a valid result.
fn eval_plan_capped(
    rete: &Rete,
    index: &GraphIndex,
    nf: Option<&[String]>,
    plan: &Plan,
    cap: usize,
) -> Vec<Binding> {
    if cap == 0 {
        return Vec::new();
    }
    match plan {
        Plan::Bgp(patterns) => crate::bgp::BgpSolutions::new(rete, index, patterns)
            .take(cap)
            .collect(),
        // A FILTER over a BGP streams the join and keeps passing rows until `cap`.
        Plan::Filter(expr, inner) if matches!(inner.as_ref(), Plan::Bgp(_)) => {
            let Plan::Bgp(patterns) = inner.as_ref() else {
                unreachable!()
            };
            let mut out = Vec::new();
            let mut cache = ExistsCache::new();
            for b in crate::bgp::BgpSolutions::new(rete, index, patterns) {
                if expr.boolean(rete, index, &b, &mut cache) {
                    out.push(b);
                    if out.len() >= cap {
                        break;
                    }
                }
            }
            out
        }
        Plan::Union(l, r) => {
            let mut out = eval_plan_capped(rete, index, nf, l, cap);
            if out.len() < cap {
                let need = cap - out.len();
                out.extend(eval_plan_capped(rete, index, nf, r, need));
            }
            out
        }
        Plan::Values(vars, rows) => rows
            .iter()
            .take(cap)
            .map(|row| {
                vars.iter()
                    .zip(row.iter())
                    .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
                    .collect()
            })
            .collect(),
        _ => eval_plan_in(rete, index, nf, plan)
            .into_iter()
            .take(cap)
            .collect(),
    }
}

/// Fast path for `SELECT DISTINCT ?vars WHERE { BGP }`: dedup on the *integer*
/// bindings and resolve only the survivors to terms. When a distinct projection
/// collapses many matched rows to a few values (e.g. `DISTINCT ?discipline` over
/// every paper), this skips a term resolution + `Binding` allocation per matched
/// row — only the distinct projections are ever resolved. Applies OFFSET/LIMIT
/// after dedup (the caller guarantees no ORDER BY/GROUP/HAVING/BIND).
fn distinct_bgp_fast(
    rete: &Rete,
    sel: &Select,
    patterns: &[TriplePattern],
) -> (Vec<String>, Vec<Binding>) {
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    let dict = rete.dictionary();

    let mut seen: std::collections::HashSet<Vec<Option<i64>>> = std::collections::HashSet::new();
    let mut rows: Vec<Binding> = Vec::new();
    for ib in &eval_bgp_int_in(rete, active, patterns) {
        // Dedup key over the projected variables (an unprojected/unbound var is
        // `None`, so its absence is part of the distinct identity).
        let key: Vec<Option<i64>> = sel.project.iter().map(|v| ib.get(v).copied()).collect();
        if !seen.insert(key) {
            continue;
        }
        let mut b = Binding::new();
        for v in &sel.project {
            if let Some(&val) = ib.get(v) {
                if let Some(t) = term_of_value(dict, val) {
                    b.insert(v.clone(), t);
                }
            }
        }
        rows.push(b);
    }
    let rows = rows
        .into_iter()
        .skip(sel.offset)
        .take(sel.limit.unwrap_or(usize::MAX))
        .collect();
    (sel.project.clone(), rows)
}

/// Raw solutions for a lowered pattern: plan + GROUP BY + aggregate aliases,
/// before projection/DISTINCT/slice (which are SELECT-specific).
fn raw_solutions(rete: &Rete, sel: &Select) -> Vec<Binding> {
    // The active default graph: `FROM` makes it the union of named graphs.
    let merged = if sel.from.is_empty() {
        None
    } else {
        Some(merge_graphs(rete, &sel.from))
    };
    let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
    let nf = sel.from_named.as_deref();

    let mut raw = match (&sel.group, &sel.plan) {
        // Fast path: GROUP BY over a single BGP aggregates on integer bindings,
        // resolving only the group keys to terms (not every matched row).
        (Some(g), Plan::Bgp(patterns)) => {
            aggregate_int(rete, eval_bgp_int_in(rete, active, patterns), g)
        }
        (Some(g), _) => aggregate(eval_plan_in(rete, active, nf, &sel.plan), g),
        (None, _) => eval_plan_in(rete, active, nf, &sel.plan),
    };
    if !sel.extends.is_empty() {
        for row in &mut raw {
            for (var, expr) in sel.extends.iter().rev() {
                if let Some(v) = expr.value(row) {
                    row.insert(var.clone(), v);
                }
            }
        }
    }
    // HAVING runs on the aggregated (and aliased) rows.
    if !sel.having.is_empty() {
        let mut cache = ExistsCache::new();
        raw.retain(|b| {
            sel.having
                .iter()
                .all(|f| f.boolean(rete, active, b, &mut cache))
        });
    }
    raw
}

/// Build the RDF merge (union of triples) of the given named graphs as a single
/// index. All graphs share the dataset dictionary, so integer triples combine
/// directly. Missing graphs contribute nothing.
fn merge_graphs(rete: &Rete, graphs: &[String]) -> GraphIndex {
    let mut b = GraphIndexBuilder::new();
    for g in graphs {
        if let Some(gi) = rete.graph_index(g) {
            for t in gi.match_pattern((None, None, None)) {
                b.push(t);
            }
        }
    }
    b.build()
}

/// Instantiate a CONSTRUCT template against solutions (triples with any unbound
/// variable are dropped; the result is deduplicated).
fn instantiate(template: &[SpTriplePattern], sols: &[Binding]) -> Vec<(String, String, String)> {
    let mut set = std::collections::BTreeSet::new();
    for b in sols {
        for tp in template {
            if let (Some(s), Some(p), Some(o)) = (
                inst_term(&tp.subject, b),
                inst_named(&tp.predicate, b),
                inst_term(&tp.object, b),
            ) {
                set.insert((s, p, o));
            }
        }
    }
    set.into_iter().collect()
}

fn inst_term(t: &TermPattern, b: &Binding) -> Option<String> {
    match t {
        TermPattern::NamedNode(n) => Some(n.to_string()),
        TermPattern::Literal(l) => Some(l.to_string()),
        TermPattern::BlankNode(bn) => Some(bn.to_string()),
        TermPattern::Variable(v) => b.get(v.as_str()).cloned(),
    }
}

fn inst_named(n: &NamedNodePattern, b: &Binding) -> Option<String> {
    match n {
        NamedNodePattern::NamedNode(nn) => Some(nn.to_string()),
        NamedNodePattern::Variable(v) => b.get(v.as_str()).cloned(),
    }
}

/// Parse and evaluate a SELECT against a file, applying the plan then
/// projection, DISTINCT, OFFSET, and LIMIT. Returns `(projected_vars,
/// solutions)`.
pub fn eval_sparql(rete: &Rete, query: &str) -> Result<(Vec<String>, Vec<Binding>), SparqlError> {
    let sel = parse_select(query)?;
    Ok(run_select(rete, &sel))
}

/// Run a lowered SELECT: raw solutions, ORDER BY, then projection, DISTINCT,
/// and slice (the SPARQL solution-modifier sequence).
fn run_select(rete: &Rete, sel: &Select) -> (Vec<String>, Vec<Binding>) {
    // Fast path: `SELECT DISTINCT ?vars WHERE { BGP }` (no ORDER BY/GROUP/HAVING/
    // BIND) dedups on integer bindings and resolves only the survivors.
    if sel.distinct
        && sel.group.is_none()
        && sel.having.is_empty()
        && sel.order.is_empty()
        && sel.extends.is_empty()
        && !sel.project.is_empty()
    {
        if let Plan::Bgp(patterns) = &sel.plan {
            return distinct_bgp_fast(rete, sel, patterns);
        }
    }

    let mut raw = match early_out_cap(sel) {
        // Pure LIMIT/OFFSET with no ORDER BY/DISTINCT/aggregate: produce only the
        // rows we need and stop. LIMIT without ORDER BY may return any subset, so
        // a streamed prefix of solutions is spec-compliant.
        Some(cap) => {
            let merged = if sel.from.is_empty() {
                None
            } else {
                Some(merge_graphs(rete, &sel.from))
            };
            let active = merged.as_ref().unwrap_or_else(|| rete.default_index());
            let mut rows =
                eval_plan_capped(rete, active, sel.from_named.as_deref(), &sel.plan, cap);
            // BIND (extends) add columns without dropping rows — apply post-cap.
            for row in &mut rows {
                for (var, expr) in sel.extends.iter().rev() {
                    if let Some(v) = expr.value(row) {
                        row.insert(var.clone(), v);
                    }
                }
            }
            rows
        }
        None => raw_solutions(rete, sel),
    };

    // ORDER BY runs before projection so it can see unprojected variables.
    if !sel.order.is_empty() {
        raw.sort_by(|a, b| {
            for (expr, desc) in &sel.order {
                let ord = cmp_opt(&expr.value(a), &expr.value(b));
                let ord = if *desc { ord.reverse() } else { ord };
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            std::cmp::Ordering::Equal
        });
    }

    // Project to the requested variables (SELECT * keeps everything).
    let mut rows: Vec<Binding> = raw
        .into_iter()
        .map(|b| {
            if sel.project.is_empty() {
                b
            } else {
                sel.project
                    .iter()
                    .filter_map(|v| b.get(v).map(|val| (v.clone(), val.clone())))
                    .collect()
            }
        })
        .collect();

    if sel.distinct {
        // Hash the row's (sorted) key/value pairs directly — `Binding` iterates
        // in key order, so the tuple vector is a canonical DISTINCT key without
        // the per-row `format!` Debug round-trip.
        let mut seen: std::collections::HashSet<Vec<(String, String)>> =
            std::collections::HashSet::new();
        rows.retain(|row| seen.insert(row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()));
    }

    let rows = rows
        .into_iter()
        .skip(sel.offset)
        .take(sel.limit.unwrap_or(usize::MAX))
        .collect();

    (sel.project.clone(), rows)
}

/// Build the evaluation [`Plan`] for a graph pattern, capturing the solution
/// modifiers (projection/DISTINCT/slice) into `sel` as transparent wrappers.
fn build(p: &GraphPattern, sel: &mut Select) -> Result<Plan, SparqlError> {
    match p {
        GraphPattern::Bgp { patterns } => Ok(Plan::Bgp(patterns.iter().map(convert).collect())),
        GraphPattern::Join { left, right } => Ok(Plan::Join(
            Box::new(build(left, sel)?),
            Box::new(build(right, sel)?),
        )),
        GraphPattern::Union { left, right } => Ok(Plan::Union(
            Box::new(build(left, sel)?),
            Box::new(build(right, sel)?),
        )),
        GraphPattern::Minus { left, right } => Ok(Plan::Minus(
            Box::new(build(left, sel)?),
            Box::new(build(right, sel)?),
        )),
        GraphPattern::Graph { name, inner } => {
            let target = match name {
                NamedNodePattern::NamedNode(n) => GraphTarget::Named(n.to_string()),
                NamedNodePattern::Variable(v) => GraphTarget::Var(v.as_str().to_string()),
            };
            Ok(Plan::Graph(target, Box::new(build(inner, sel)?)))
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            let cond = expression.as_ref().map(convert_expr).transpose()?;
            Ok(Plan::LeftJoin(
                Box::new(build(left, sel)?),
                Box::new(build(right, sel)?),
                cond,
            ))
        }
        GraphPattern::Path {
            subject,
            path,
            object,
        } => Ok(Plan::Path(
            term_to_pattern(subject),
            lower_path(path)?,
            term_to_pattern(object),
        )),
        GraphPattern::Values {
            variables,
            bindings,
        } => {
            let vars = variables.iter().map(|v| v.as_str().to_string()).collect();
            let rows = bindings
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|g| g.as_ref().map(|t| t.to_string()))
                        .collect()
                })
                .collect();
            Ok(Plan::Values(vars, rows))
        }
        GraphPattern::Filter { expr, inner } => {
            // A filter sitting *above* a GROUP BY is a HAVING: it must run after
            // aggregation, not on the raw bindings.
            let had_group = sel.group.is_some();
            let inner_plan = build(inner, sel)?;
            let fexpr = convert_expr(expr)?;
            if sel.group.is_some() && !had_group {
                sel.having.push(fexpr);
                Ok(inner_plan)
            } else {
                Ok(Plan::Filter(fexpr, Box::new(inner_plan)))
            }
        }
        // Transparent solution-modifier wrappers: record and descend.
        GraphPattern::Project { inner, variables } => {
            // The first Project is the query's own projection. A *second* one
            // reached during descent is a nested SELECT (subquery), which we do
            // not evaluate as an independent scope — reject it clearly instead of
            // silently flattening its variables into the outer projection (which
            // would produce wrong results, not an error).
            if !sel.project.is_empty() {
                return Err(SparqlError::Unsupported(
                    "subqueries (nested SELECT) are not supported",
                ));
            }
            for v in variables {
                sel.project.push(v.as_str().to_string());
            }
            build(inner, sel)
        }
        GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
            sel.distinct = true;
            build(inner, sel)
        }
        GraphPattern::Slice {
            inner,
            start,
            length,
        } => {
            sel.offset = *start;
            sel.limit = *length;
            build(inner, sel)
        }
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            let by = variables.iter().map(|v| v.as_str().to_string()).collect();
            let mut aggs = Vec::with_capacity(aggregates.len());
            for (var, ae) in aggregates {
                aggs.push((var.as_str().to_string(), convert_agg(ae)?));
            }
            sel.group = Some(GroupSpec { by, aggs });
            build(inner, sel)
        }
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            sel.extends
                .push((variable.as_str().to_string(), convert_expr(expression)?));
            build(inner, sel)
        }
        GraphPattern::OrderBy { inner, expression } => {
            for oe in expression {
                let (e, desc) = match oe {
                    OrderExpression::Asc(e) => (e, false),
                    OrderExpression::Desc(e) => (e, true),
                };
                sel.order.push((convert_expr(e)?, desc));
            }
            build(inner, sel)
        }
        _ => Err(SparqlError::Unsupported(
            "unsupported SPARQL construct (e.g. SERVICE federation)",
        )),
    }
}

fn convert_agg(ae: &AggregateExpression) -> Result<Agg, SparqlError> {
    match ae {
        AggregateExpression::CountSolutions { distinct } => Ok(Agg::CountStar {
            distinct: *distinct,
        }),
        AggregateExpression::FunctionCall {
            name,
            expr,
            distinct,
        } => {
            let var = match expr {
                Expression::Variable(v) => v.as_str().to_string(),
                _ => return Err(SparqlError::Unsupported("aggregate over non-variable")),
            };
            Ok(match name {
                AggregateFunction::Count => Agg::Count(var, *distinct),
                AggregateFunction::Sum => Agg::Sum(var),
                AggregateFunction::Avg => Agg::Avg(var),
                AggregateFunction::Min => Agg::Min(var),
                AggregateFunction::Max => Agg::Max(var),
                AggregateFunction::Sample => Agg::Sample(var),
                AggregateFunction::GroupConcat { separator } => {
                    Agg::GroupConcat(var, separator.clone().unwrap_or_else(|| " ".to_string()))
                }
                _ => return Err(SparqlError::Unsupported("aggregate function")),
            })
        }
    }
}

/// Group `rows` and compute aggregates, producing one binding per group.
fn aggregate(rows: Vec<Binding>, g: &GroupSpec) -> Vec<Binding> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<Vec<String>, Vec<Binding>> = BTreeMap::new();
    for r in rows {
        let key: Vec<String> =
            g.by.iter()
                .map(|v| r.get(v).cloned().unwrap_or_default())
                .collect();
        groups.entry(key).or_default().push(r);
    }
    // A grouped query with no rows still yields one (empty/zero) group.
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }

    let mut out = Vec::new();
    for (key, members) in groups {
        let mut b = Binding::new();
        for (v, val) in g.by.iter().zip(key.iter()) {
            if !val.is_empty() {
                b.insert(v.clone(), val.clone());
            }
        }
        for (res_var, agg) in &g.aggs {
            if let Some(val) = compute_agg(agg, &members) {
                b.insert(res_var.clone(), val);
            }
        }
        out.push(b);
    }
    out
}

/// Integer-binding aggregation: group on tagged i64 keys and resolve only the
/// group-key terms (and any values an aggregate actually needs). Mirrors
/// [`aggregate`] but avoids resolving every row to terms.
fn aggregate_int(rete: &Rete, rows: Vec<IntBinding>, g: &GroupSpec) -> Vec<Binding> {
    use std::collections::BTreeMap;
    let dict = rete.dictionary();
    const UNBOUND: i64 = i64::MIN;

    let mut groups: BTreeMap<Vec<i64>, Vec<IntBinding>> = BTreeMap::new();
    for r in rows {
        let key: Vec<i64> =
            g.by.iter()
                .map(|v| r.get(v).copied().unwrap_or(UNBOUND))
                .collect();
        groups.entry(key).or_default().push(r);
    }
    if g.by.is_empty() && groups.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }

    let mut out = Vec::new();
    for (key, members) in groups {
        let mut b = Binding::new();
        for (v, &val) in g.by.iter().zip(key.iter()) {
            if val != UNBOUND {
                if let Some(t) = term_of_value(dict, val) {
                    b.insert(v.clone(), t);
                }
            }
        }
        for (res_var, agg) in &g.aggs {
            if let Some(val) = compute_agg_int(rete, agg, &members) {
                b.insert(res_var.clone(), val);
            }
        }
        out.push(b);
    }
    out
}

fn compute_agg_int(rete: &Rete, agg: &Agg, members: &[IntBinding]) -> Option<String> {
    let dict = rete.dictionary();
    // Values of a variable across members, resolved to terms (only when needed).
    let nums = |var: &str| -> Vec<f64> {
        members
            .iter()
            .filter_map(|m| m.get(var))
            .filter_map(|&v| term_of_value(dict, v))
            .filter_map(|t| as_number(&t))
            .collect()
    };
    match agg {
        Agg::CountStar { .. } => Some(fmt_num(members.len() as f64)),
        Agg::Count(var, distinct) => {
            let n = if *distinct {
                members
                    .iter()
                    .filter_map(|m| m.get(var).copied())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            } else {
                members.iter().filter(|m| m.contains_key(var)).count()
            };
            Some(fmt_num(n as f64))
        }
        Agg::Sum(var) => Some(fmt_num(nums(var).iter().sum())),
        Agg::Avg(var) => {
            let v = nums(var);
            (!v.is_empty()).then(|| fmt_num(v.iter().sum::<f64>() / v.len() as f64))
        }
        Agg::Sample(var) => members
            .iter()
            .find_map(|m| m.get(var))
            .and_then(|&v| term_of_value(dict, v)),
        Agg::GroupConcat(var, sep) => Some(
            members
                .iter()
                .filter_map(|m| m.get(var))
                .filter_map(|&v| term_of_value(dict, v))
                .map(|t| lexical(&t))
                .collect::<Vec<_>>()
                .join(sep),
        ),
        Agg::Min(var) | Agg::Max(var) => {
            let want_min = matches!(agg, Agg::Min(_));
            let terms: Vec<String> = members
                .iter()
                .filter_map(|m| m.get(var))
                .filter_map(|&v| term_of_value(dict, v))
                .collect();
            terms.into_iter().reduce(|cur, v| {
                let take = match (as_number(&v), as_number(&cur)) {
                    (Some(a), Some(b)) if want_min => a < b,
                    (Some(a), Some(b)) => a > b,
                    _ if want_min => v < cur,
                    _ => v > cur,
                };
                if take {
                    v
                } else {
                    cur
                }
            })
        }
    }
}

fn compute_agg(agg: &Agg, members: &[Binding]) -> Option<String> {
    let nums = |var: &str| -> Vec<f64> {
        members
            .iter()
            .filter_map(|m| m.get(var))
            .filter_map(|v| as_number(v))
            .collect()
    };
    match agg {
        Agg::CountStar { .. } => Some(fmt_num(members.len() as f64)),
        Agg::Count(var, distinct) => {
            let n = if *distinct {
                members
                    .iter()
                    .filter_map(|m| m.get(var).cloned())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
            } else {
                members.iter().filter(|m| m.contains_key(var)).count()
            };
            Some(fmt_num(n as f64))
        }
        Agg::Sum(var) => Some(fmt_num(nums(var).iter().sum())),
        Agg::Avg(var) => {
            let v = nums(var);
            if v.is_empty() {
                None
            } else {
                Some(fmt_num(v.iter().sum::<f64>() / v.len() as f64))
            }
        }
        Agg::Min(var) => extreme(members, var, true),
        Agg::Max(var) => extreme(members, var, false),
        Agg::Sample(var) => members.iter().find_map(|m| m.get(var).cloned()),
        Agg::GroupConcat(var, sep) => Some(
            members
                .iter()
                .filter_map(|m| m.get(var))
                .map(|t| lexical(t))
                .collect::<Vec<_>>()
                .join(sep),
        ),
    }
}

/// Min (`want_min`) or Max of a variable's values: numeric when both compare as
/// numbers, else lexical.
fn extreme(members: &[Binding], var: &str, want_min: bool) -> Option<String> {
    let mut best: Option<String> = None;
    for v in members.iter().filter_map(|m| m.get(var)) {
        best = Some(match best {
            None => v.clone(),
            Some(cur) => {
                let take = match (as_number(v), as_number(&cur)) {
                    (Some(a), Some(b)) if want_min => a < b,
                    (Some(a), Some(b)) => a > b,
                    _ if want_min => v < &cur,
                    _ => v > &cur,
                };
                if take {
                    v.clone()
                } else {
                    cur
                }
            }
        });
    }
    best
}

/// Format a numeric aggregate result (drop the fraction for integral values).
fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 {
        format!("{}", x as i64)
    } else {
        format!("{x}")
    }
}

/// Evaluate a plan against a specific graph `index` (the active graph).
/// `named_filter` (from `FROM NAMED`) restricts which graphs `GRAPH` may see.
fn eval_plan_in(
    rete: &Rete,
    index: &GraphIndex,
    named_filter: Option<&[String]>,
    plan: &Plan,
) -> Vec<Binding> {
    let recur = |p: &Plan| eval_plan_in(rete, index, named_filter, p);
    // A named graph is visible unless FROM NAMED excludes it.
    let visible = |name: &str| named_filter.is_none_or(|f| f.iter().any(|g| g == name));
    match plan {
        Plan::Bgp(patterns) => {
            let dict = rete.dictionary();
            eval_bgp_int_in(rete, index, patterns)
                .into_iter()
                .map(|ib| {
                    ib.into_iter()
                        .filter_map(|(k, v)| term_of_value(dict, v).map(|t| (k, t)))
                        .collect()
                })
                .collect()
        }
        Plan::Path(subj, spec, obj) => eval_path(rete, index, subj, spec, obj),
        Plan::Values(vars, rows) => rows
            .iter()
            .map(|row| {
                vars.iter()
                    .zip(row.iter())
                    .filter_map(|(v, val)| val.as_ref().map(|t| (v.clone(), t.clone())))
                    .collect()
            })
            .collect(),
        Plan::Filter(expr, inner) => {
            let mut v = recur(inner);
            let mut cache = ExistsCache::new();
            v.retain(|b| expr.boolean(rete, index, b, &mut cache));
            v
        }
        Plan::Union(l, r) => {
            let mut v = recur(l);
            v.extend(recur(r));
            v
        }
        Plan::Minus(l, r) => minus_hash(recur(l), recur(r)),
        Plan::Join(l, r) => hash_join_solutions(rete, index, recur(l), recur(r), false, None),
        Plan::LeftJoin(l, r, cond) => {
            hash_join_solutions(rete, index, recur(l), recur(r), true, cond.as_ref())
        }
        // GRAPH switches the active graph index (subject to FROM NAMED).
        Plan::Graph(GraphTarget::Named(iri), inner) => match rete.graph_index(iri) {
            Some(gi) if visible(iri) => eval_plan_in(rete, gi, named_filter, inner),
            _ => Vec::new(),
        },
        Plan::Graph(GraphTarget::Var(var), inner) => {
            let mut out = Vec::new();
            for (name, gi) in rete.named_graphs() {
                if !visible(name) {
                    continue;
                }
                for mut sol in eval_plan_in(rete, gi, named_filter, inner) {
                    match sol.get(var) {
                        Some(existing) if existing != name => continue,
                        _ => {
                            sol.insert(var.clone(), name.clone());
                        }
                    }
                    out.push(sol);
                }
            }
            out
        }
    }
}

/// Lower a `spargebra` property path into a [`PathAst`].
fn lower_path(p: &PropertyPathExpression) -> Result<PathAst, SparqlError> {
    Ok(match p {
        PropertyPathExpression::NamedNode(n) => PathAst::Pred(n.to_string(), false),
        PropertyPathExpression::Reverse(inner) => reverse(lower_path(inner)?),
        PropertyPathExpression::OneOrMore(inner) => {
            PathAst::Rep(Box::new(lower_path(inner)?), Rep::OneOrMore)
        }
        PropertyPathExpression::ZeroOrMore(inner) => {
            PathAst::Rep(Box::new(lower_path(inner)?), Rep::ZeroOrMore)
        }
        PropertyPathExpression::ZeroOrOne(inner) => {
            PathAst::Rep(Box::new(lower_path(inner)?), Rep::ZeroOrOne)
        }
        PropertyPathExpression::Sequence(a, b) => {
            PathAst::Seq(Box::new(lower_path(a)?), Box::new(lower_path(b)?))
        }
        PropertyPathExpression::Alternative(a, b) => {
            PathAst::Alt(Box::new(lower_path(a)?), Box::new(lower_path(b)?))
        }
        PropertyPathExpression::NegatedPropertySet(_) => {
            return Err(SparqlError::Unsupported("negated property set path"))
        }
    })
}

/// Push a reverse through a path (reverses each predicate and swaps sequences).
fn reverse(ast: PathAst) -> PathAst {
    match ast {
        PathAst::Pred(p, r) => PathAst::Pred(p, !r),
        PathAst::Rep(inner, rep) => PathAst::Rep(Box::new(reverse(*inner)), rep),
        PathAst::Seq(a, b) => PathAst::Seq(Box::new(reverse(*b)), Box::new(reverse(*a))),
        PathAst::Alt(a, b) => PathAst::Alt(Box::new(reverse(*a)), Box::new(reverse(*b))),
    }
}

/// Cached adjacency keyed by `(predicate, reversed)` → start node → successor
/// nodes. Everything is in the unified integer node space — no term strings.
type AdjCache =
    std::collections::HashMap<(String, bool), std::collections::BTreeMap<u32, Vec<u32>>>;

/// Successor nodes of `start` along a single predicate (built and cached on
/// first use, directly from integer node pairs — no term resolution).
fn successors(
    rete: &Rete,
    index: &GraphIndex,
    cache: &mut AdjCache,
    pred: &str,
    rev: bool,
    start: u32,
) -> Vec<u32> {
    let key = (pred.to_string(), rev);
    if !cache.contains_key(&key) {
        let dict = rete.dictionary();
        let pairs: Vec<(u32, u32)> = match dict.predicate_id(pred) {
            Some(pid) => index
                .match_pattern((None, Some(pid), None))
                .into_iter()
                .map(|(s, _p, o)| (dict.subject_node(s), dict.object_node(o)))
                .collect(),
            None => Vec::new(),
        };
        let mut adj: std::collections::BTreeMap<u32, Vec<u32>> = Default::default();
        for (s, o) in pairs {
            let (from, to) = if rev { (o, s) } else { (s, o) };
            adj.entry(from).or_default().push(to);
        }
        cache.insert(key.clone(), adj);
    }
    cache
        .get(&key)
        .and_then(|a| a.get(&start))
        .cloned()
        .unwrap_or_default()
}

/// Nodes reachable from `start` along `ast` — forward from the start node, so a
/// bound endpoint never triggers a global closure. Integer node space.
fn reach_from(
    rete: &Rete,
    index: &GraphIndex,
    ast: &PathAst,
    start: u32,
    cache: &mut AdjCache,
) -> std::collections::BTreeSet<u32> {
    use std::collections::BTreeSet;
    match ast {
        PathAst::Pred(p, rev) => successors(rete, index, cache, p, *rev, start)
            .into_iter()
            .collect(),
        PathAst::Alt(a, b) => {
            let mut r = reach_from(rete, index, a, start, cache);
            r.extend(reach_from(rete, index, b, start, cache));
            r
        }
        PathAst::Seq(a, b) => {
            let mids = reach_from(rete, index, a, start, cache);
            let mut out = BTreeSet::new();
            for m in &mids {
                out.extend(reach_from(rete, index, b, *m, cache));
            }
            out
        }
        PathAst::Rep(inner, rep) => match rep {
            Rep::One => reach_from(rete, index, inner, start, cache),
            Rep::ZeroOrOne => {
                let mut r = reach_from(rete, index, inner, start, cache);
                r.insert(start);
                r
            }
            Rep::OneOrMore | Rep::ZeroOrMore => {
                let mut visited = BTreeSet::new();
                let mut stack: Vec<u32> = reach_from(rete, index, inner, start, cache)
                    .into_iter()
                    .collect();
                while let Some(n) = stack.pop() {
                    if visited.insert(n) {
                        for m in reach_from(rete, index, inner, n, cache) {
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

fn bind_pair(subj: &PatternTerm, obj: &PatternTerm, a: &str, b: &str) -> Option<Binding> {
    let mut binding = Binding::new();
    for (term, val) in [(subj, a), (obj, b)] {
        if let PatternTerm::Var(v) = term {
            match binding.get(v) {
                Some(existing) if existing != val => return None,
                _ => {
                    binding.insert(v.clone(), val.to_string());
                }
            }
        }
    }
    Some(binding)
}

/// Evaluate a property path to solution bindings. Traversal runs on integer node
/// IDs; terms are resolved only for the produced bindings. A constant endpoint is
/// pushed down so unbounded paths don't enumerate the whole graph.
fn eval_path(
    rete: &Rete,
    index: &GraphIndex,
    subj: &PatternTerm,
    ast: &PathAst,
    obj: &PatternTerm,
) -> Vec<Binding> {
    let dict = rete.dictionary();
    let mut cache = AdjCache::new();
    let mut out = Vec::new();

    match (subj, obj) {
        // Bound subject: forward search from its node.
        (PatternTerm::Const(s), _) => {
            let Some(sn) = dict.node_of_term(s) else {
                return Vec::new();
            };
            let obj_node = match obj {
                PatternTerm::Const(o) => Some(dict.node_of_term(o)),
                _ => None,
            };
            for e in reach_from(rete, index, ast, sn, &mut cache) {
                if let Some(on) = obj_node {
                    if on != Some(e) {
                        continue;
                    }
                }
                if let Some(et) = dict.node_term(e) {
                    if let Some(b) = bind_pair(subj, obj, s, &et) {
                        out.push(b);
                    }
                }
            }
        }
        // Bound object only: search backward along the reversed path.
        (PatternTerm::Var(_), PatternTerm::Const(o)) => {
            let Some(on) = dict.node_of_term(o) else {
                return Vec::new();
            };
            let rev = reverse(ast.clone());
            for s in reach_from(rete, index, &rev, on, &mut cache) {
                if let Some(st) = dict.node_term(s) {
                    if let Some(b) = bind_pair(subj, obj, &st, o) {
                        out.push(b);
                    }
                }
            }
        }
        // Both unbound: enumerate from every node (inherently expensive).
        (PatternTerm::Var(_), PatternTerm::Var(_)) => {
            for start in 0..dict.node_count() {
                for e in reach_from(rete, index, ast, start, &mut cache) {
                    if let (Some(st), Some(et)) = (dict.node_term(start), dict.node_term(e)) {
                        if let Some(b) = bind_pair(subj, obj, &st, &et) {
                            out.push(b);
                        }
                    }
                }
            }
        }
    }

    out.sort();
    out.dedup();
    out
}

/// Merge two bindings if compatible (shared variables agree), else `None`.
fn merge(a: &Binding, b: &Binding) -> Option<Binding> {
    let mut out = a.clone();
    for (k, v) in b {
        match out.get(k) {
            Some(existing) if existing != v => return None,
            Some(_) => {}
            None => {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    Some(out)
}

/// Is a left row eliminated by a right row under `MINUS`? True iff they share at
/// least one variable and agree on every shared variable (SPARQL `MINUS`:
/// disjoint-domain rows never eliminate, and a disagreement keeps the left row).
fn minus_compatible(lb: &Binding, rb: &Binding) -> bool {
    let mut shared = false;
    for (k, v) in lb {
        if let Some(w) = rb.get(k) {
            if v != w {
                return false;
            }
            shared = true;
        }
    }
    shared
}

/// `MINUS`: keep each left row unless some right row is [`minus_compatible`] with
/// it. Replaces the O(L×R) nested loop with an O(L + R) hash anti-join in the
/// common case (both sides bind the shared variables in every row): the shared
/// variables `jv` index the right rows, so a fully-bound left row is eliminated
/// by a single set lookup. Rows missing a shared variable (only via a nested
/// OPTIONAL/UNION) fall back to a scan, preserving exact semantics.
fn minus_hash(left: Vec<Binding>, right: Vec<Binding>) -> Vec<Binding> {
    use std::collections::HashSet;
    if left.is_empty() || right.is_empty() {
        return left;
    }
    // Shared variables: those appearing in some left row AND some right row.
    let lvars: HashSet<&str> = left
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    let mut jv: Vec<String> = right
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .filter(|v| lvars.contains(v))
        .map(String::from)
        .collect();
    jv.sort();
    jv.dedup();
    if jv.is_empty() {
        // Disjoint domains ⇒ MINUS eliminates nothing.
        return left;
    }
    let key_of =
        |b: &Binding| -> Option<Vec<String>> { jv.iter().map(|v| b.get(v).cloned()).collect() };
    // Right rows fully bound on the shared vars index a set; the rest must be
    // scanned (a left row could share only a sub-domain with them).
    let mut full: HashSet<Vec<String>> = HashSet::new();
    let mut partial_right: Vec<&Binding> = Vec::new();
    for r in &right {
        match key_of(r) {
            Some(k) => {
                full.insert(k);
            }
            None => partial_right.push(r),
        }
    }
    left.into_iter()
        .filter(|lb| {
            match key_of(lb) {
                // Fully bound on the shared vars: a fully-bound right row
                // eliminates it iff their keys match; otherwise only a partial
                // right row could.
                Some(k) => {
                    if full.contains(&k) {
                        return false;
                    }
                    !partial_right.iter().any(|rb| minus_compatible(lb, rb))
                }
                // Missing a shared var: must check every right row.
                None => !right.iter().any(|rb| minus_compatible(lb, rb)),
            }
        })
        .collect()
}

/// Hash join two solution sets on the variables they share, emitting every
/// compatible merge. `optional = true` is a left join (OPTIONAL): a left row
/// with no surviving match is emitted unchanged, and `cond` (the OPTIONAL's
/// filter) decides which merges count as a match.
///
/// This replaces the O(L×R) nested-loop merge with an O(L + R + matches) hash
/// join in the common case where the join variables are bound in every row.
/// Rows missing a join variable (only possible via a nested OPTIONAL) fall back
/// to being tried against all candidates, preserving exact SPARQL semantics —
/// `merge` still does the final compatibility check on every shared variable.
fn hash_join_solutions(
    rete: &Rete,
    index: &GraphIndex,
    left: Vec<Binding>,
    right: Vec<Binding>,
    optional: bool,
    cond: Option<&FExpr>,
) -> Vec<Binding> {
    use std::collections::{HashMap, HashSet};
    if left.is_empty() {
        return Vec::new();
    }
    if right.is_empty() {
        return if optional { left } else { Vec::new() };
    }
    // Join variables: names that occur in both sides.
    let lvars: HashSet<&str> = left
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    let mut jset: HashSet<&str> = right
        .iter()
        .flat_map(|b| b.keys().map(String::as_str))
        .collect();
    jset.retain(|v| lvars.contains(v));
    let mut jv: Vec<String> = jset.into_iter().map(String::from).collect();
    jv.sort();

    // Key = the join-var values, when all are bound; `None` ⇒ a join var is
    // unbound in this row (a partial that must be matched against everything).
    let key_of =
        |b: &Binding| -> Option<Vec<String>> { jv.iter().map(|v| b.get(v).cloned()).collect() };
    let mut buckets: HashMap<Vec<String>, Vec<usize>> = HashMap::new();
    let mut partial: Vec<usize> = Vec::new();
    for (i, r) in right.iter().enumerate() {
        match key_of(r) {
            Some(k) => buckets.entry(k).or_default().push(i),
            None => partial.push(i),
        }
    }

    let mut out = Vec::new();
    let mut cache = ExistsCache::new();
    let mut emit = |a: &Binding, r: &Binding, out: &mut Vec<Binding>, matched: &mut bool| {
        if let Some(m) = merge(a, r) {
            if cond.is_none_or(|f| f.boolean(rete, index, &m, &mut cache)) {
                out.push(m);
                *matched = true;
            }
        }
    };
    for a in &left {
        let mut matched = false;
        match key_of(a) {
            Some(k) => {
                if let Some(idxs) = buckets.get(&k) {
                    for &i in idxs {
                        emit(a, &right[i], &mut out, &mut matched);
                    }
                }
                for &i in &partial {
                    emit(a, &right[i], &mut out, &mut matched);
                }
            }
            // `a` itself lacks a join var: every right row is a candidate.
            None => {
                for r in &right {
                    emit(a, r, &mut out, &mut matched);
                }
            }
        }
        if optional && !matched {
            out.push(a.clone());
        }
    }
    out
}

/// Translate a `spargebra` expression into the supported [`FExpr`] subset.
fn convert_expr(e: &Expression) -> Result<FExpr, SparqlError> {
    let bin = |op, l: &Expression, r: &Expression| -> Result<FExpr, SparqlError> {
        Ok(FExpr::Compare(
            op,
            Box::new(convert_expr(l)?),
            Box::new(convert_expr(r)?),
        ))
    };
    let arith = |op, l: &Expression, r: &Expression| -> Result<FExpr, SparqlError> {
        Ok(FExpr::Arith(
            op,
            Box::new(convert_expr(l)?),
            Box::new(convert_expr(r)?),
        ))
    };
    Ok(match e {
        Expression::Variable(v) => FExpr::Var(v.as_str().to_string()),
        Expression::NamedNode(n) => FExpr::Const(n.to_string()),
        Expression::Literal(l) => FExpr::Const(l.to_string()),
        Expression::Equal(l, r) => bin(Op::Eq, l, r)?,
        Expression::Greater(l, r) => bin(Op::Gt, l, r)?,
        Expression::GreaterOrEqual(l, r) => bin(Op::Ge, l, r)?,
        Expression::Less(l, r) => bin(Op::Lt, l, r)?,
        Expression::LessOrEqual(l, r) => bin(Op::Le, l, r)?,
        Expression::And(l, r) => FExpr::And(Box::new(convert_expr(l)?), Box::new(convert_expr(r)?)),
        Expression::Or(l, r) => FExpr::Or(Box::new(convert_expr(l)?), Box::new(convert_expr(r)?)),
        Expression::Not(inner) => FExpr::Not(Box::new(convert_expr(inner)?)),
        Expression::Bound(v) => FExpr::Bound(v.as_str().to_string()),
        Expression::Add(l, r) => arith(ArithOp::Add, l, r)?,
        Expression::Subtract(l, r) => arith(ArithOp::Sub, l, r)?,
        Expression::Multiply(l, r) => arith(ArithOp::Mul, l, r)?,
        Expression::Divide(l, r) => arith(ArithOp::Div, l, r)?,
        Expression::Coalesce(items) => FExpr::Coalesce(
            items
                .iter()
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        Expression::UnaryPlus(e) => convert_expr(e)?,
        Expression::UnaryMinus(e) => FExpr::Arith(
            ArithOp::Sub,
            Box::new(FExpr::Const("0".into())),
            Box::new(convert_expr(e)?),
        ),
        Expression::Exists(pattern) => {
            // Build the sub-plan with a throwaway Select (its modifiers don't
            // escape the EXISTS).
            let mut sub = Select::default();
            let plan = build(pattern, &mut sub)?;
            FExpr::Exists(Box::new(plan))
        }
        Expression::FunctionCall(func, params) => {
            let builtin = match func {
                Function::Str => Builtin::Str,
                Function::Concat => Builtin::Concat,
                Function::SubStr => Builtin::SubStr,
                Function::StrBefore => Builtin::StrBefore,
                Function::StrAfter => Builtin::StrAfter,
                Function::StrLen => Builtin::StrLen,
                Function::UCase => Builtin::UCase,
                Function::LCase => Builtin::LCase,
                Function::Abs => Builtin::Abs,
                Function::Ceil => Builtin::Ceil,
                Function::Floor => Builtin::Floor,
                Function::Round => Builtin::Round,
                Function::Contains => Builtin::Contains,
                Function::StrStarts => Builtin::StrStarts,
                Function::StrEnds => Builtin::StrEnds,
                Function::IsIri => Builtin::IsIri,
                Function::IsBlank => Builtin::IsBlank,
                Function::IsLiteral => Builtin::IsLiteral,
                Function::IsNumeric => Builtin::IsNumeric,
                Function::Regex => Builtin::Regex,
                _ => return Err(SparqlError::Unsupported("built-in function")),
            };
            let args = params
                .iter()
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?;
            FExpr::Func(builtin, args)
        }
        _ => return Err(SparqlError::Unsupported("filter expression form")),
    })
}

fn convert(tp: &SpTriplePattern) -> TriplePattern {
    TriplePattern {
        s: term_to_pattern(&tp.subject),
        p: named_to_pattern(&tp.predicate),
        o: term_to_pattern(&tp.object),
    }
}

fn term_to_pattern(t: &TermPattern) -> PatternTerm {
    match t {
        TermPattern::NamedNode(n) => PatternTerm::Const(n.to_string()),
        TermPattern::Literal(l) => PatternTerm::Const(l.to_string()),
        // A blank node in a query pattern is a non-distinguished variable (and
        // spargebra uses one as the join var when expanding fixed paths like
        // `a/b`). Its label is stable across occurrences, so it joins correctly.
        TermPattern::BlankNode(b) => PatternTerm::Var(b.to_string()),
        TermPattern::Variable(v) => PatternTerm::Var(v.as_str().to_string()),
    }
}

fn named_to_pattern(n: &NamedNodePattern) -> PatternTerm {
    match n {
        NamedNodePattern::NamedNode(nn) => PatternTerm::Const(nn.to_string()),
        NamedNodePattern::Variable(v) => PatternTerm::Var(v.as_str().to_string()),
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
        let mut parts: Vec<&str> = sols[0]["fs"].split('|').collect();
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
        assert_eq!(sols[0]["n"], "2");
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
                ("<http://ex/Alice>".into(), "2".into()),
                ("<http://ex/Bob>".into(), "1".into()),
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
        assert_eq!(sols[0]["n"], "2");
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
        assert_eq!(sols[0]["first"], "Alice");
        assert_eq!(sols[0]["last"], "Smith");
        assert_eq!(sols[0]["ini"], "A");
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
        assert_eq!(labels, vec!["@Al", "@Bob"]);
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
        assert_eq!(sols[0]["len"], "11"); // "Alice Smith"
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
        assert_eq!(sols[0]["next"], "31");
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
