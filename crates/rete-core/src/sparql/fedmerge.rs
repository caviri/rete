//! Solution modifiers over a federation (`rete federate`): each source answers
//! the query on its own, so ORDER BY, OFFSET, LIMIT and aggregates have to be
//! applied (or merged) once more over the sources' rows, which the caller holds
//! as term tokens.
//!
//! [`FederationPlan::new`] reads the query's top-level modifiers off its
//! algebra and **refuses** what the merged rows cannot support, naming why:
//!
//! * ORDER BY keys must be projected variables (the merge sees projected rows
//!   only), not expressions;
//! * an aggregate query is merged only when every projected column is a GROUP BY
//!   variable or a plain `COUNT`, `SUM`, `MIN` or `MAX`; `AVG`, `SAMPLE`,
//!   `GROUP_CONCAT`, `COUNT(DISTINCT …)`, HAVING and expressions over an
//!   aggregate are refused. `MIN` and `MAX` merge exactly; `COUNT` and `SUM` add
//!   the sources' totals, which counts a triple stored in two sources twice, so
//!   they are merged only when the caller says the sources are `disjoint`;
//! * a CONSTRUCT with OFFSET or LIMIT, or with an aggregate, is refused (its
//!   LIMIT counts solutions, not triples).
//!
//! [`FederationPlan::source_query`] gives the text each source runs: the
//! query itself, or with `LIMIT offset+limit` pushed down (an upper bound: the
//! rows past it cannot be among the merged ones), or with its ORDER BY / LIMIT /
//! OFFSET removed when the aggregate merge takes them over.
//! [`FederationPlan::finish`] then folds the aggregates, sorts, and slices.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::rc::Rc;

use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, GraphPattern, OrderExpression,
};
use spargebra::Query;

use super::lower::parse_query;
use super::value::OrderKey;
use super::SparqlError;
use crate::bgp::Binding;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

fn refuse(why: impl Into<String>) -> SparqlError {
    SparqlError::Federation(why.into())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fold {
    Count,
    Sum,
    Min,
    Max,
}

#[derive(Debug, Clone)]
struct Grouping {
    /// GROUP BY variables that are projected.
    by: Vec<String>,
    /// `(projected variable, how the sources' values combine)`.
    cols: Vec<(String, Fold)>,
}

/// How to run one query over several sources and merge their rows: see the
/// module docs.
#[derive(Debug, Clone)]
pub struct FederationPlan {
    /// A SELECT (the only form the modifiers apply to).
    select: bool,
    distinct: bool,
    offset: usize,
    limit: Option<usize>,
    /// ORDER BY keys: `(variable, descending)`.
    order: Vec<(String, bool)>,
    group: Option<Grouping>,
}

impl FederationPlan {
    /// Plan `query`. `disjoint` says no triple is stored in two sources, which
    /// lets `COUNT` and `SUM` be added up.
    pub fn new(query: &str, disjoint: bool) -> Result<FederationPlan, SparqlError> {
        let parsed = parse_query(query)?;
        let (select, pattern) = match &parsed {
            Query::Select { pattern, .. } => (true, pattern),
            Query::Construct { pattern, .. } => (false, pattern),
            // ASK and DESCRIBE have no modifiers to merge.
            _ => {
                return Ok(FederationPlan {
                    select: false,
                    distinct: false,
                    offset: 0,
                    limit: None,
                    order: Vec::new(),
                    group: None,
                })
            }
        };

        // Peel the top-level modifiers in the order spargebra nests them:
        // Slice, Distinct/Reduced, Project, OrderBy, select expressions and
        // HAVING, Group — each once and in that order, so a sub-SELECT's own
        // modifiers are never mistaken for the query's.
        let mut offset = 0;
        let mut limit = None;
        let mut distinct = false;
        let mut project: Option<&[spargebra::term::Variable]> = None;
        let mut order: Option<&[OrderExpression]> = None;
        let mut extends: Vec<(&str, &Expression)> = Vec::new();
        let mut having = false;
        let mut group = None;
        let mut stage = 0;
        let mut p: &GraphPattern = pattern;
        loop {
            let (next, inner): (u8, &GraphPattern) = match p {
                GraphPattern::Slice {
                    inner,
                    start,
                    length,
                } if stage < 1 => {
                    offset = *start;
                    limit = *length;
                    (1, &**inner)
                }
                GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } if stage < 2 => {
                    distinct = true;
                    (2, &**inner)
                }
                GraphPattern::Project { inner, variables } if stage < 3 => {
                    project = Some(variables);
                    (3, &**inner)
                }
                GraphPattern::OrderBy { inner, expression } if stage < 4 => {
                    order = Some(expression);
                    (4, &**inner)
                }
                GraphPattern::Extend {
                    inner,
                    variable,
                    expression,
                } if stage <= 5 => {
                    extends.push((variable.as_str(), expression));
                    (5, &**inner)
                }
                GraphPattern::Filter { inner, .. } if stage <= 5 => {
                    having = true;
                    (5, &**inner)
                }
                GraphPattern::Group {
                    variables,
                    aggregates,
                    ..
                } if stage < 6 => {
                    group = Some((variables, aggregates));
                    break;
                }
                _ => break,
            };
            stage = next;
            p = inner;
        }

        if !select {
            if offset != 0 || limit.is_some() {
                return Err(refuse(
                    "a CONSTRUCT with OFFSET or LIMIT: they count solutions, and each source \
                     would cut its own. Use a SELECT, or run the CONSTRUCT on one source",
                ));
            }
            if group.is_some() {
                return Err(refuse(
                    "a CONSTRUCT over an aggregate is computed per source. Use a SELECT",
                ));
            }
            return Ok(FederationPlan {
                select: false,
                distinct: false,
                offset: 0,
                limit: None,
                order: Vec::new(),
                group: None,
            });
        }

        let project: Vec<String> = project
            .map(|v| v.iter().map(|v| v.as_str().to_string()).collect())
            .unwrap_or_default();

        let mut keys = Vec::new();
        for o in order.unwrap_or(&[]) {
            let (e, desc) = match o {
                OrderExpression::Asc(e) => (e, false),
                OrderExpression::Desc(e) => (e, true),
            };
            match e {
                Expression::Variable(v) if project.iter().any(|p| p == v.as_str()) => {
                    keys.push((v.as_str().to_string(), desc))
                }
                Expression::Variable(v) => {
                    return Err(refuse(format!(
                        "ORDER BY ?{}, which the query does not project: the merged rows hold \
                         only the projected variables. Add it to the SELECT",
                        v.as_str()
                    )))
                }
                _ => {
                    return Err(refuse(
                        "ORDER BY an expression: the merge sorts the projected variables. Bind \
                         the expression to a variable in the SELECT ((expr AS ?k)) and order by \
                         that",
                    ))
                }
            }
        }

        let grouping = match group {
            None => None,
            Some((by, aggregates)) => {
                if having {
                    return Err(refuse(
                        "HAVING filters groups that are only partial in each source",
                    ));
                }
                // `(COUNT(*) AS ?n)` is `?n = ?agg0` over the group's `?agg0`.
                let alias = |name: &str| -> String {
                    extends
                        .iter()
                        .find_map(|(v, e)| match e {
                            Expression::Variable(a) if a.as_str() == name => Some((*v).to_string()),
                            _ => None,
                        })
                        .unwrap_or_else(|| name.to_string())
                };
                if let Some((v, _)) = extends
                    .iter()
                    .find(|(_, e)| !matches!(e, Expression::Variable(a) if aggregates.iter().any(|(n, _)| n == a)))
                {
                    return Err(refuse(format!(
                        "?{v} is an expression over a group or an aggregate. Select the \
                         aggregate itself and compute the expression afterwards"
                    )));
                }
                let mut cols = Vec::new();
                for (name, agg) in aggregates {
                    let out = alias(name.as_str());
                    if !project.contains(&out) {
                        continue; // computed but not selected (ORDER BY it is refused above)
                    }
                    let fold = match agg {
                        AggregateExpression::CountSolutions { distinct: false } => Fold::Count,
                        AggregateExpression::FunctionCall {
                            name: AggregateFunction::Count,
                            distinct: false,
                            ..
                        } => Fold::Count,
                        AggregateExpression::FunctionCall {
                            name: AggregateFunction::Sum,
                            distinct: false,
                            ..
                        } => Fold::Sum,
                        AggregateExpression::FunctionCall {
                            name: AggregateFunction::Min,
                            ..
                        } => Fold::Min,
                        AggregateExpression::FunctionCall {
                            name: AggregateFunction::Max,
                            ..
                        } => Fold::Max,
                        other => {
                            let what = match other {
                                AggregateExpression::CountSolutions { .. }
                                | AggregateExpression::FunctionCall {
                                    name: AggregateFunction::Count,
                                    ..
                                } => "COUNT(DISTINCT …)",
                                AggregateExpression::FunctionCall {
                                    name: AggregateFunction::Sum,
                                    ..
                                } => "SUM(DISTINCT …)",
                                AggregateExpression::FunctionCall {
                                    name: AggregateFunction::Avg,
                                    ..
                                } => "AVG",
                                AggregateExpression::FunctionCall {
                                    name: AggregateFunction::GroupConcat { .. },
                                    ..
                                } => "GROUP_CONCAT",
                                AggregateExpression::FunctionCall {
                                    name: AggregateFunction::Sample,
                                    ..
                                } => "SAMPLE",
                                _ => "this aggregate",
                            };
                            return Err(refuse(format!(
                                "{what} cannot be merged from per-source results. Only COUNT, \
                                 SUM, MIN and MAX can; or query one source"
                            )));
                        }
                    };
                    if matches!(fold, Fold::Count | Fold::Sum) && !disjoint {
                        return Err(refuse(
                            "COUNT and SUM add each source's total, which counts a triple stored \
                             in two sources twice. If the sources share no triples, say so with \
                             --disjoint",
                        ));
                    }
                    cols.push((out, fold));
                }
                let by_vars: Vec<String> = by.iter().map(|v| v.as_str().to_string()).collect();
                for p in &project {
                    if !by_vars.contains(p) && !cols.iter().any(|(c, _)| c == p) {
                        return Err(refuse(format!(
                            "?{p} is neither a GROUP BY variable nor an aggregate"
                        )));
                    }
                }
                Some(Grouping {
                    by: by_vars
                        .into_iter()
                        .filter(|v| project.contains(v))
                        .collect(),
                    cols,
                })
            }
        };

        Ok(FederationPlan {
            select: true,
            distinct,
            offset,
            limit,
            order: keys,
            group: grouping,
        })
    }

    /// Whether equal rows from different sources are one row. An aggregate's
    /// rows are not: two sources can each count 5.
    pub fn dedup_rows(&self) -> bool {
        self.group.is_none()
    }

    /// The query each source should run for the merge to give the right rows.
    pub fn source_query(&self, query: &str) -> Result<String, SparqlError> {
        if !self.select {
            return Ok(query.to_string());
        }
        // What a source must not do itself: an OFFSET (the merge skips rows
        // once, over all sources), and, for an aggregate, a LIMIT (it would cut
        // the groups the merge adds up).
        let cuts = self.offset > 0 || (self.limit.is_some() && self.group.is_some());
        let unchanged = |why: &str| -> Result<String, SparqlError> {
            if cuts {
                Err(refuse(format!(
                    "cannot take this query's {why} off the per-source queries to merge it"
                )))
            } else {
                // Not understood well enough to rewrite, and a source's own
                // LIMIT is only an upper bound: run as written, merge after.
                Ok(query.to_string())
            }
        };
        let Some(scan) = Scan::of(query) else {
            return unchanged("OFFSET / LIMIT");
        };
        if self.group.is_some() {
            // Each source returns its whole groups; the merge sorts and slices.
            return Ok(scan.head(query).trim_end().to_string());
        }
        if self.limit.is_none() && self.offset == 0 {
            return Ok(query.to_string());
        }
        let mut head = scan.head(query).trim_end().to_string();
        let mut limit = String::new();
        if let Some(l) = self.limit {
            // Each source's first `offset + limit` rows (in the query's order)
            // hold every row the merged result can need. Sources are merged as
            // a set, so the cut must count distinct rows: DISTINCT per source.
            if !self.distinct {
                let Some(at) = scan.select_end else {
                    return unchanged("OFFSET / LIMIT");
                };
                head.insert_str(at, " DISTINCT");
            }
            limit = format!(" LIMIT {}", self.offset.saturating_add(l));
        }
        let order = scan.order(query).trim();
        let order = if order.is_empty() {
            String::new()
        } else {
            format!(" {order}")
        };
        Ok(format!("{head}{order}{limit}"))
    }

    /// The merged rows: aggregates folded, ORDER BY, OFFSET and LIMIT applied.
    pub fn finish(&self, rows: Vec<Binding>) -> Vec<Binding> {
        if !self.select {
            return rows;
        }
        let mut rows = match &self.group {
            Some(g) => fold_groups(g, rows),
            None => rows,
        };
        if !self.order.is_empty() {
            let keyed: Vec<Vec<Option<OrderKey>>> = rows
                .iter()
                .map(|r| {
                    self.order
                        .iter()
                        .map(|(v, _)| r.get(v).map(|t| OrderKey::new(Rc::from(t.as_str()))))
                        .collect()
                })
                .collect();
            let mut idx: Vec<usize> = (0..rows.len()).collect();
            idx.sort_by(|&a, &b| {
                for (k, (_, desc)) in self.order.iter().enumerate() {
                    // Unbound sorts lowest (§15.1).
                    let c = match (&keyed[a][k], &keyed[b][k]) {
                        (None, None) => Ordering::Equal,
                        (None, Some(_)) => Ordering::Less,
                        (Some(_), None) => Ordering::Greater,
                        (Some(x), Some(y)) => x.cmp(y),
                    };
                    let c = if *desc { c.reverse() } else { c };
                    if c != Ordering::Equal {
                        return c;
                    }
                }
                Ordering::Equal
            });
            let mut taken: Vec<Option<Binding>> = rows.into_iter().map(Some).collect();
            rows = idx.into_iter().filter_map(|i| taken[i].take()).collect();
        }
        let skip = self.offset.min(rows.len());
        rows.drain(..skip);
        if let Some(l) = self.limit {
            rows.truncate(l);
        }
        rows
    }
}

// --- aggregates ---------------------------------------------------------------

fn fold_groups(g: &Grouping, rows: Vec<Binding>) -> Vec<Binding> {
    let mut groups: BTreeMap<Vec<Option<String>>, Vec<Binding>> = BTreeMap::new();
    for r in rows {
        let key = g.by.iter().map(|v| r.get(v).cloned()).collect();
        groups.entry(key).or_default().push(r);
    }
    // An aggregate without GROUP BY always has one group, even over no rows.
    if groups.is_empty() && g.by.is_empty() {
        groups.insert(Vec::new(), Vec::new());
    }
    groups
        .into_iter()
        .map(|(key, members)| {
            let mut out = Binding::new();
            for (v, val) in g.by.iter().zip(key) {
                if let Some(val) = val {
                    out.insert(v.clone(), val);
                }
            }
            for (col, fold) in &g.cols {
                let vals: Vec<Option<&str>> = members
                    .iter()
                    .map(|m| m.get(col).map(String::as_str))
                    .collect();
                if let Some(v) = fold_values(*fold, &vals) {
                    out.insert(col.clone(), v);
                }
            }
            out
        })
        .collect()
}

#[derive(Clone, Copy, PartialEq, PartialOrd)]
enum Kind {
    Integer,
    Decimal,
    Float,
    Double,
}

/// A numeric token as `(kind, integer value, float value)`.
fn number(tok: &str) -> Option<(Kind, i128, f64)> {
    let (lex, dt) = match crate::terms::literal_parts(tok) {
        Some((body, sfx)) => (
            crate::terms::unescape_literal(body),
            sfx.strip_prefix("^^<")?.strip_suffix('>')?.to_string(),
        ),
        None => (
            tok.to_string(),
            format!(
                "{XSD}{}",
                if tok.contains(['e', 'E']) {
                    "double"
                } else if tok.contains('.') {
                    "decimal"
                } else {
                    "integer"
                }
            ),
        ),
    };
    let kind = match dt.strip_prefix(XSD)? {
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "positiveInteger" | "nonPositiveInteger" | "negativeInteger" | "unsignedInt"
        | "unsignedLong" | "unsignedShort" | "unsignedByte" => Kind::Integer,
        "decimal" => Kind::Decimal,
        "float" => Kind::Float,
        "double" => Kind::Double,
        _ => return None,
    };
    let lex = lex.trim();
    let int = if kind == Kind::Integer {
        lex.parse::<i128>().ok()?
    } else {
        0
    };
    let f = match kind {
        Kind::Integer => int as f64,
        _ => lex.parse::<f64>().ok()?,
    };
    Some((kind, int, f))
}

fn fold_values(fold: Fold, vals: &[Option<&str>]) -> Option<String> {
    match fold {
        Fold::Count | Fold::Sum => {
            // An unbound total is an error in its source, and so in the sum.
            let mut kind = Kind::Integer;
            let mut int: Option<i128> = Some(0);
            let mut float = 0.0f64;
            for v in vals {
                let (k, i, f) = number(v.as_deref()?)?;
                if k > kind {
                    kind = k;
                }
                int = int.and_then(|s| s.checked_add(i));
                float += f;
            }
            Some(match kind {
                Kind::Integer => crate::terms::make_literal(
                    &int?.to_string(),
                    None,
                    Some(&format!("{XSD}integer")),
                ),
                Kind::Decimal => {
                    // Shortest form past binary-float noise, as rete's own SUM.
                    let cleaned: f64 = format!("{float:.14e}").parse().unwrap_or(float);
                    crate::terms::make_literal(
                        &cleaned.to_string(),
                        None,
                        Some(&format!("{XSD}decimal")),
                    )
                }
                Kind::Float => crate::terms::make_literal(
                    &format!("{float:E}"),
                    None,
                    Some(&format!("{XSD}float")),
                ),
                Kind::Double => crate::terms::make_literal(
                    &format!("{float:E}"),
                    None,
                    Some(&format!("{XSD}double")),
                ),
            })
        }
        Fold::Min | Fold::Max => {
            // A source whose group had no value leaves it unbound: skipped.
            let mut best: Option<OrderKey> = None;
            for v in vals.iter().flatten() {
                let k = OrderKey::new(Rc::from(*v));
                let take = match &best {
                    None => true,
                    Some(b) => match k.cmp(b) {
                        Ordering::Less => fold == Fold::Min,
                        Ordering::Greater => fold == Fold::Max,
                        Ordering::Equal => false,
                    },
                };
                if take {
                    best = Some(k);
                }
            }
            best.map(|k| k.token.to_string())
        }
    }
}

// --- the query text -------------------------------------------------------------

/// Where the top-level `SELECT` keyword ends and the solution modifiers
/// (`ORDER BY … LIMIT … OFFSET …`) start, found without parsing: the parsed
/// algebra has no source positions. Only words at brace and parenthesis depth 0
/// count, and strings, IRIs and comments are skipped, so a sub-SELECT's
/// modifiers and a literal "LIMIT" are never mistaken for the query's.
struct Scan {
    /// Byte index just after the first top-level `SELECT`.
    select_end: Option<usize>,
    /// Start of the first top-level ORDER / LIMIT / OFFSET; `query.len()` if none.
    modifiers: usize,
    /// The ORDER BY clause's start and end (the first LIMIT / OFFSET after it).
    order: Option<(usize, usize)>,
}

impl Scan {
    fn of(q: &str) -> Option<Scan> {
        let b = q.as_bytes();
        let mut depth = 0i32;
        let mut i = 0;
        let mut select_end = None;
        let mut order_start = None;
        let mut limit_start = None;
        let mut first_mod = None;
        while i < b.len() {
            match b[i] {
                b'#' => {
                    while i < b.len() && b[i] != b'\n' {
                        i += 1;
                    }
                }
                b'"' | b'\'' => {
                    let quote = b[i];
                    let long = b.get(i + 1) == Some(&quote) && b.get(i + 2) == Some(&quote);
                    i += if long { 3 } else { 1 };
                    loop {
                        match b.get(i) {
                            None => return None,
                            Some(b'\\') => i += 2,
                            Some(&c) if c == quote => {
                                if !long {
                                    i += 1;
                                    break;
                                }
                                if b.get(i + 1) == Some(&quote) && b.get(i + 2) == Some(&quote) {
                                    i += 3;
                                    break;
                                }
                                i += 1;
                            }
                            Some(_) => i += 1,
                        }
                    }
                }
                b'<' => {
                    // An IRI reference has no whitespace; `<` as an operator
                    // is followed by one before any `>`.
                    let mut j = i + 1;
                    while j < b.len() && !b[j].is_ascii_whitespace() && b[j] != b'>' && b[j] != b'<'
                    {
                        j += 1;
                    }
                    i = if b.get(j) == Some(&b'>') {
                        j + 1
                    } else {
                        i + 1
                    };
                }
                b'{' | b'(' | b'[' => {
                    depth += 1;
                    i += 1;
                }
                b'}' | b')' | b']' => {
                    depth -= 1;
                    i += 1;
                }
                c if c.is_ascii_alphabetic() && depth == 0 => {
                    let start = i;
                    let prev_ok = start == 0
                        || !(b[start - 1].is_ascii_alphanumeric()
                            || matches!(
                                b[start - 1],
                                b'_' | b':' | b'?' | b'$' | b'-' | b'.' | b'@'
                            ));
                    while i < b.len()
                        && (b[i].is_ascii_alphanumeric() || matches!(b[i], b'_' | b':' | b'-'))
                    {
                        i += 1;
                    }
                    if !prev_ok {
                        continue;
                    }
                    let word = &q[start..i];
                    if word.eq_ignore_ascii_case("SELECT") && select_end.is_none() {
                        select_end = Some(i);
                    } else if word.eq_ignore_ascii_case("ORDER") && order_start.is_none() {
                        let rest = q[i..].trim_start();
                        if rest.len() >= 2 && rest[..2].eq_ignore_ascii_case("BY") {
                            order_start = Some(start);
                            first_mod.get_or_insert(start);
                        }
                    } else if (word.eq_ignore_ascii_case("LIMIT")
                        || word.eq_ignore_ascii_case("OFFSET"))
                        && limit_start.is_none()
                    {
                        limit_start = Some(start);
                        first_mod.get_or_insert(start);
                    }
                }
                _ => i += 1,
            }
        }
        if depth != 0 {
            return None;
        }
        let modifiers = first_mod.unwrap_or(q.len());
        // After the modifiers' start only ORDER BY …, then LIMIT n / OFFSET n.
        if let Some(ls) = limit_start {
            let mut toks = q[ls..].split_whitespace();
            while let Some(kw) = toks.next() {
                let n = toks.next()?;
                if !(kw.eq_ignore_ascii_case("LIMIT") || kw.eq_ignore_ascii_case("OFFSET"))
                    || n.parse::<u64>().is_err()
                {
                    return None;
                }
            }
        }
        Some(Scan {
            select_end,
            modifiers,
            order: order_start.map(|s| (s, limit_start.unwrap_or(q.len()))),
        })
    }

    /// The query up to its solution modifiers.
    fn head<'a>(&self, q: &'a str) -> &'a str {
        &q[..self.modifiers]
    }

    /// The ORDER BY clause, or `""`.
    fn order<'a>(&self, q: &'a str) -> &'a str {
        self.order.map_or("", |(s, e)| &q[s..e])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pairs: &[(&str, &str)]) -> Binding {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn int(n: i64) -> String {
        format!("\"{n}\"^^<{XSD}integer>")
    }

    fn plan(q: &str) -> FederationPlan {
        FederationPlan::new(q, true).unwrap()
    }

    fn why(q: &str, disjoint: bool) -> String {
        FederationPlan::new(q, disjoint).unwrap_err().to_string()
    }

    #[test]
    fn order_by_offset_and_limit_apply_to_the_merged_rows() {
        let p = plan("SELECT ?x WHERE { ?x ?p ?o } ORDER BY DESC(?x) OFFSET 1 LIMIT 2");
        let rows = ["<b>", "<d>", "<a>", "<c>"]
            .iter()
            .map(|x| row(&[("x", x)]))
            .collect();
        let got: Vec<String> = p.finish(rows).into_iter().map(|r| r["x"].clone()).collect();
        assert_eq!(got, ["<c>", "<b>"]);
    }

    #[test]
    fn unbound_sorts_first_and_numbers_by_value() {
        let p = plan("SELECT ?n ?m WHERE { ?x ?p ?n OPTIONAL { ?x ?q ?m } } ORDER BY ?m ?n");
        let rows = vec![
            row(&[("n", &int(10)), ("m", &int(2))]),
            row(&[("n", &int(9))]),
            row(&[("n", &int(8)), ("m", &int(2))]),
        ];
        let got: Vec<String> = p.finish(rows).into_iter().map(|r| r["n"].clone()).collect();
        assert_eq!(got, [int(9), int(8), int(10)]);
    }

    #[test]
    fn order_by_must_use_projected_variables() {
        let e = why("SELECT ?x WHERE { ?x ?p ?o } ORDER BY ?o", true);
        assert!(e.contains("does not project"), "{e}");
        let e = why("SELECT ?x WHERE { ?x ?p ?o } ORDER BY STR(?x)", true);
        assert!(e.contains("an expression"), "{e}");
        // `SELECT *` projects everything in scope.
        plan("SELECT * WHERE { ?x ?p ?o } ORDER BY ?o");
    }

    #[test]
    fn limit_is_pushed_down_as_an_upper_bound() {
        let q = "PREFIX e: <http://e/>\nSELECT ?x WHERE { ?x e:p \"ORDER BY LIMIT 1\" } ORDER BY ?x LIMIT 5 OFFSET 2";
        let p = plan(q);
        assert_eq!(
            p.source_query(q).unwrap(),
            "PREFIX e: <http://e/>\nSELECT DISTINCT ?x WHERE { ?x e:p \"ORDER BY LIMIT 1\" } ORDER BY ?x LIMIT 7"
        );
        // Already DISTINCT; no ORDER BY.
        let q = "select distinct ?x where { ?x ?p ?o } limit 3";
        assert_eq!(
            plan(q).source_query(q).unwrap(),
            "select distinct ?x where { ?x ?p ?o } LIMIT 3"
        );
        // A sub-SELECT's LIMIT is not the query's: nothing to push.
        let q = "SELECT ?x WHERE { { SELECT ?x WHERE { ?x ?p ?o } LIMIT 2 } }";
        assert_eq!(plan(q).source_query(q).unwrap(), q);
        // OFFSET alone is taken off: the merge skips the rows once.
        let q = "SELECT ?x WHERE { ?x ?p ?o } ORDER BY ?x OFFSET 2";
        assert_eq!(
            plan(q).source_query(q).unwrap(),
            "SELECT ?x WHERE { ?x ?p ?o } ORDER BY ?x"
        );
    }

    #[test]
    fn construct_modifiers_are_refused() {
        let e = why("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o } LIMIT 2", true);
        assert!(e.contains("CONSTRUCT"), "{e}");
        plan("CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }");
        plan("ASK { ?s ?p ?o }");
    }

    #[test]
    fn count_and_sum_add_up_min_and_max_pick() {
        let q = "SELECT ?g (COUNT(*) AS ?n) (SUM(?v) AS ?s) (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) \
                 WHERE { ?x ?g ?v } GROUP BY ?g ORDER BY DESC(?n) LIMIT 1";
        let p = plan(q);
        assert!(!p.dedup_rows());
        assert_eq!(
            p.source_query(q).unwrap(),
            "SELECT ?g (COUNT(*) AS ?n) (SUM(?v) AS ?s) (MIN(?v) AS ?lo) (MAX(?v) AS ?hi) \
             WHERE { ?x ?g ?v } GROUP BY ?g"
        );
        let mk = |g: &str, n: i64, s: &str, lo: i64, hi: i64| {
            row(&[
                ("g", g),
                ("n", &int(n)),
                ("s", s),
                ("lo", &int(lo)),
                ("hi", &int(hi)),
            ])
        };
        let dec = |x: &str| format!("\"{x}\"^^<{XSD}decimal>");
        let rows = vec![
            mk("<a>", 2, &int(5), 1, 4),
            mk("<b>", 1, &int(9), 9, 9),
            mk("<a>", 2, &dec("0.5"), 0, 3),
        ];
        let got = p.finish(rows);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["g"], "<a>");
        assert_eq!(got[0]["n"], int(4));
        assert_eq!(got[0]["s"], dec("5.5"));
        assert_eq!(got[0]["lo"], int(0));
        assert_eq!(got[0]["hi"], int(4));
    }

    #[test]
    fn an_aggregate_without_group_by_is_one_row_even_over_nothing() {
        let p = plan("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
        let got = p.finish(Vec::new());
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["n"], int(0));
    }

    #[test]
    fn a_source_error_stays_an_error_in_the_sum() {
        let p = plan("SELECT (SUM(?v) AS ?s) WHERE { ?s ?p ?v }");
        let got = p.finish(vec![row(&[("s", &int(1))]), row(&[])]);
        assert!(!got[0].contains_key("s"));
    }

    #[test]
    fn aggregates_that_cannot_merge_are_refused() {
        for (agg, name) in [
            ("AVG(?v)", "AVG"),
            ("SAMPLE(?v)", "SAMPLE"),
            ("GROUP_CONCAT(?v)", "GROUP_CONCAT"),
            ("COUNT(DISTINCT ?v)", "COUNT(DISTINCT"),
        ] {
            let e = why(&format!("SELECT ({agg} AS ?a) WHERE {{ ?s ?p ?v }}"), true);
            assert!(e.contains(name), "{agg}: {e}");
        }
        let e = why(
            "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?v } GROUP BY ?p HAVING (COUNT(*) > 1)",
            true,
        );
        assert!(e.contains("HAVING"), "{e}");
        let e = why("SELECT ((COUNT(*) + 1) AS ?n) WHERE { ?s ?p ?v }", true);
        assert!(e.contains("expression"), "{e}");
        let e = why("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?v }", false);
        assert!(e.contains("--disjoint"), "{e}");
        // MIN and MAX are exact whether or not the sources overlap.
        FederationPlan::new("SELECT (MAX(?v) AS ?m) WHERE { ?s ?p ?v }", false).unwrap();
    }
}
