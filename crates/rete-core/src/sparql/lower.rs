//! Lowering: parse a SPARQL query with `spargebra` and translate its algebra
//! into the engine's [`Select`]/[`Plan`]/[`FExpr`] forms (SPEC.md §8). This is
//! the front end — it only builds plan/expression values; evaluation lives in
//! the parent module and its `eval`/`aggregate`/`path` siblings.

use super::*;

use crate::bgp::{PatternTerm, TriplePattern};
use spargebra::algebra::{
    AggregateExpression, AggregateFunction, Expression, Function, GraphPattern, OrderExpression,
    PropertyPathExpression, QueryDataset,
};
use spargebra::term::{NamedNodePattern, TermPattern, TriplePattern as SpTriplePattern};
use spargebra::Query;

/// Lower a graph pattern (with its solution modifiers) into a [`Select`].
pub(super) fn lower_pattern(pattern: &GraphPattern) -> Result<Select, SparqlError> {
    let mut sel = Select::default();
    let plan = build(pattern, &mut sel, false)?;
    sel.plan = plan;
    Ok(sel)
}

/// Lower a SELECT's pattern + dataset clause into a [`Select`].
pub(super) fn lower_select(
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

/// Build the evaluation [`Plan`] for a graph pattern, capturing the solution
/// modifiers (projection/DISTINCT/slice) into `sel` as transparent wrappers.
///
/// `in_where` is true once we have descended into the graph-pattern body (past
/// any pattern operator or a GROUP BY's inner). It decides where an `Extend`
/// (`BIND`) lands: an in-pattern BIND becomes an in-tree [`Plan::Extend`] so a
/// following FILTER/join sees it, while a top-level projection alias goes to the
/// post-evaluation [`Select::extends`] list (applied after any aggregation).
fn build(p: &GraphPattern, sel: &mut Select, in_where: bool) -> Result<Plan, SparqlError> {
    match p {
        GraphPattern::Bgp { patterns } => Ok(Plan::Bgp(patterns.iter().map(convert).collect())),
        GraphPattern::Join { left, right } => Ok(Plan::Join(
            Box::new(build(left, sel, true)?),
            Box::new(build(right, sel, true)?),
        )),
        GraphPattern::Union { left, right } => Ok(Plan::Union(
            Box::new(build(left, sel, true)?),
            Box::new(build(right, sel, true)?),
        )),
        GraphPattern::Minus { left, right } => Ok(Plan::Minus(
            Box::new(build(left, sel, true)?),
            Box::new(build(right, sel, true)?),
        )),
        GraphPattern::Graph { name, inner } => {
            let target = match name {
                NamedNodePattern::NamedNode(n) => GraphTarget::Named(n.to_string()),
                NamedNodePattern::Variable(v) => GraphTarget::Var(v.as_str().to_string()),
            };
            Ok(Plan::Graph(target, Box::new(build(inner, sel, true)?)))
        }
        GraphPattern::LeftJoin {
            left,
            right,
            expression,
        } => {
            let cond = expression.as_ref().map(convert_expr).transpose()?;
            Ok(Plan::LeftJoin(
                Box::new(build(left, sel, true)?),
                Box::new(build(right, sel, true)?),
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
            let inner_plan = build(inner, sel, true)?;
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
            // A Project reached *inside* the graph pattern (or after the query's
            // own projection is already set) is a nested SELECT: lower it into
            // its own independent `Select` and evaluate it as a subquery whose
            // projected solutions join with the surrounding pattern.
            if in_where || !sel.project.is_empty() {
                let sub = lower_pattern(p)?;
                return Ok(Plan::Subquery(Box::new(sub)));
            }
            for v in variables {
                sel.project.push(v.as_str().to_string());
            }
            build(inner, sel, in_where)
        }
        GraphPattern::Distinct { inner } | GraphPattern::Reduced { inner } => {
            sel.distinct = true;
            build(inner, sel, in_where)
        }
        GraphPattern::Slice {
            inner,
            start,
            length,
        } => {
            sel.offset = *start;
            sel.limit = *length;
            build(inner, sel, in_where)
        }
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            let by = variables.iter().map(|v| v.as_str().to_string()).collect();
            let mut aggs = Vec::with_capacity(aggregates.len());
            let mut pre: Vec<(String, FExpr)> = Vec::new();
            for (var, ae) in aggregates {
                aggs.push((var.as_str().to_string(), convert_agg(ae, &mut pre)?));
            }
            sel.group = Some(GroupSpec { by, aggs, pre });
            // The group's inner *is* the WHERE pattern — any BIND inside it must
            // run per-row before aggregation, so descend as in-pattern.
            build(inner, sel, true)
        }
        GraphPattern::Extend {
            inner,
            variable,
            expression,
        } => {
            let var = variable.as_str().to_string();
            let fexpr = convert_expr(expression)?;
            if in_where {
                // A BIND inside the graph pattern: keep it in the plan tree so a
                // following FILTER or join observes the bound variable.
                Ok(Plan::Extend(var, fexpr, Box::new(build(inner, sel, true)?)))
            } else {
                // A top-level projection alias `(expr AS ?v)`: applied after the
                // pattern (and after any aggregation) at projection time.
                sel.extends.push((var, fexpr));
                build(inner, sel, in_where)
            }
        }
        GraphPattern::OrderBy { inner, expression } => {
            for oe in expression {
                let (e, desc) = match oe {
                    OrderExpression::Asc(e) => (e, false),
                    OrderExpression::Desc(e) => (e, true),
                };
                sel.order.push((convert_expr(e)?, desc));
            }
            build(inner, sel, in_where)
        }
        // SPARQL 1.1 federated query: the inner pattern is not lowered — it is
        // re-serialized to SPARQL text (spargebra round-trips, prefixes already
        // expanded) and shipped verbatim to the endpoint at evaluation time.
        // Only the variables it can bind are collected, so the returned
        // solutions land in slots and join like any other operand.
        GraphPattern::Service {
            name,
            inner,
            silent,
        } => {
            let endpoint = match name {
                NamedNodePattern::NamedNode(n) => n.as_str().to_string(),
                NamedNodePattern::Variable(_) => {
                    return Err(SparqlError::Unsupported("SERVICE with a variable endpoint"))
                }
            };
            let mut vars = std::collections::BTreeSet::new();
            collect_pattern_variables(inner, &mut vars);
            let query = Query::Select {
                dataset: None,
                pattern: (**inner).clone(),
                base_iri: None,
            }
            .to_string();
            Ok(Plan::Service {
                silent: *silent,
                endpoint,
                vars: vars.into_iter().collect(),
                query,
            })
        }
    }
}

/// Collect the variables a graph pattern can bind — used to give a `SERVICE`
/// block's results their slots. Deliberately an **over-approximation** (e.g. a
/// nested SELECT's non-projected variables are included): an extra slot just
/// stays unbound, while a missed one would silently drop a returned binding.
fn collect_pattern_variables(p: &GraphPattern, out: &mut std::collections::BTreeSet<String>) {
    let term_var = |t: &TermPattern, out: &mut std::collections::BTreeSet<String>| {
        if let TermPattern::Variable(v) = t {
            out.insert(v.as_str().to_string());
        }
    };
    match p {
        GraphPattern::Bgp { patterns } => {
            for tp in patterns {
                term_var(&tp.subject, out);
                if let NamedNodePattern::Variable(v) = &tp.predicate {
                    out.insert(v.as_str().to_string());
                }
                term_var(&tp.object, out);
            }
        }
        GraphPattern::Path {
            subject, object, ..
        } => {
            term_var(subject, out);
            term_var(object, out);
        }
        GraphPattern::Values { variables, .. } => {
            for v in variables {
                out.insert(v.as_str().to_string());
            }
        }
        GraphPattern::Join { left, right }
        | GraphPattern::Union { left, right }
        | GraphPattern::Minus { left, right } => {
            collect_pattern_variables(left, out);
            collect_pattern_variables(right, out);
        }
        GraphPattern::LeftJoin { left, right, .. } => {
            collect_pattern_variables(left, out);
            collect_pattern_variables(right, out);
        }
        GraphPattern::Extend {
            inner, variable, ..
        } => {
            out.insert(variable.as_str().to_string());
            collect_pattern_variables(inner, out);
        }
        GraphPattern::Group {
            inner,
            variables,
            aggregates,
        } => {
            for v in variables {
                out.insert(v.as_str().to_string());
            }
            for (v, _) in aggregates {
                out.insert(v.as_str().to_string());
            }
            collect_pattern_variables(inner, out);
        }
        GraphPattern::Graph { name, inner } => {
            if let NamedNodePattern::Variable(v) = name {
                out.insert(v.as_str().to_string());
            }
            collect_pattern_variables(inner, out);
        }
        GraphPattern::Project { inner, variables } => {
            for v in variables {
                out.insert(v.as_str().to_string());
            }
            collect_pattern_variables(inner, out);
        }
        GraphPattern::Filter { inner, .. }
        | GraphPattern::OrderBy { inner, .. }
        | GraphPattern::Distinct { inner }
        | GraphPattern::Reduced { inner }
        | GraphPattern::Slice { inner, .. }
        | GraphPattern::Service { inner, .. } => collect_pattern_variables(inner, out),
    }
}

fn convert_agg(
    ae: &AggregateExpression,
    pre: &mut Vec<(String, FExpr)>,
) -> Result<Agg, SparqlError> {
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
                // Aggregate over an EXPRESSION (e.g. SUM(?a * 2), AVG(?x + ?y)):
                // compute it into a synthetic per-row column before grouping, then
                // aggregate that column — so all the aggregate machinery (and the
                // summary-safe COUNT path) keeps treating the argument as a slot.
                other => {
                    let name = format!("__agg{}", pre.len());
                    pre.push((name.clone(), convert_expr(other)?));
                    name
                }
            };
            Ok(match name {
                AggregateFunction::Count => Agg::Count(var, *distinct),
                AggregateFunction::Sum => Agg::Sum(var),
                AggregateFunction::Avg => Agg::Avg(var),
                AggregateFunction::Min => Agg::Min(var),
                AggregateFunction::Max => Agg::Max(var),
                AggregateFunction::Sample => Agg::Sample(var),
                AggregateFunction::GroupConcat { separator } => Agg::GroupConcat(
                    var,
                    separator.clone().unwrap_or_else(|| " ".to_string()),
                    *distinct,
                ),
                _ => return Err(SparqlError::Unsupported("aggregate function")),
            })
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
        PropertyPathExpression::NegatedPropertySet(preds) => {
            PathAst::NegatedSet(preds.iter().map(|n| n.to_string()).collect(), false)
        }
    })
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
        Expression::If(c, t, e) => FExpr::If(
            Box::new(convert_expr(c)?),
            Box::new(convert_expr(t)?),
            Box::new(convert_expr(e)?),
        ),
        Expression::In(e, list) => FExpr::In(
            Box::new(convert_expr(e)?),
            list.iter().map(convert_expr).collect::<Result<_, _>>()?,
        ),
        Expression::SameTerm(l, r) => {
            FExpr::SameTerm(Box::new(convert_expr(l)?), Box::new(convert_expr(r)?))
        }
        Expression::Exists(pattern) => {
            // Build the sub-plan with a throwaway Select (its modifiers don't
            // escape the EXISTS). The whole body is a WHERE pattern, so any BIND
            // must stay in-tree (the discarded `sub.extends` would be lost).
            let mut sub = Select::default();
            let plan = build(pattern, &mut sub, true)?;
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
                Function::Datatype => Builtin::Datatype,
                Function::Lang => Builtin::Lang,
                Function::Regex => Builtin::Regex,
                Function::LangMatches => Builtin::LangMatches,
                Function::StrDt => Builtin::StrDt,
                Function::StrLang => Builtin::StrLang,
                Function::Iri => Builtin::Iri,
                Function::EncodeForUri => Builtin::EncodeForUri,
                Function::Replace => Builtin::Replace,
                Function::Md5 => Builtin::Md5,
                Function::Sha1 => Builtin::Sha1,
                Function::Sha256 => Builtin::Sha256,
                Function::Sha384 => Builtin::Sha384,
                Function::Sha512 => Builtin::Sha512,
                Function::Year => Builtin::Year,
                Function::Month => Builtin::Month,
                Function::Day => Builtin::Day,
                Function::Hours => Builtin::Hours,
                Function::Minutes => Builtin::Minutes,
                Function::Seconds => Builtin::Seconds,
                Function::Timezone => Builtin::Timezone,
                Function::Tz => Builtin::Tz,
                Function::Rand => Builtin::Rand,
                Function::Uuid => Builtin::Uuid,
                Function::StrUuid => Builtin::StrUuid,
                Function::BNode => Builtin::BNode,
                // An `xsd:<type>(expr)` constructor parses as a call to the
                // datatype IRI — map the supported XSD casts.
                Function::Custom(nn) => match nn.as_str() {
                    "http://www.w3.org/2001/XMLSchema#integer" => Builtin::CastInteger,
                    "http://www.w3.org/2001/XMLSchema#decimal" => Builtin::CastDecimal,
                    "http://www.w3.org/2001/XMLSchema#float" => Builtin::CastFloat,
                    "http://www.w3.org/2001/XMLSchema#double" => Builtin::CastDouble,
                    "http://www.w3.org/2001/XMLSchema#boolean" => Builtin::CastBoolean,
                    "http://www.w3.org/2001/XMLSchema#string" => Builtin::CastString,
                    // GeoSPARQL geof: functions.
                    "http://www.opengis.net/def/function/geosparql/sfContains" => {
                        Builtin::GeoSfContains
                    }
                    "http://www.opengis.net/def/function/geosparql/sfWithin" => {
                        Builtin::GeoSfWithin
                    }
                    "http://www.opengis.net/def/function/geosparql/sfIntersects" => {
                        Builtin::GeoSfIntersects
                    }
                    "http://www.opengis.net/def/function/geosparql/sfDisjoint" => {
                        Builtin::GeoSfDisjoint
                    }
                    "http://www.opengis.net/def/function/geosparql/sfEquals" => {
                        Builtin::GeoSfEquals
                    }
                    "http://www.opengis.net/def/function/geosparql/distance" => {
                        Builtin::GeoDistance
                    }
                    "http://www.opengis.net/def/function/geosparql/envelope" => {
                        Builtin::GeoEnvelope
                    }
                    _ => return Err(SparqlError::Unsupported("built-in function")),
                },
                _ => return Err(SparqlError::Unsupported("built-in function")),
            };
            let args = params
                .iter()
                .map(convert_expr)
                .collect::<Result<Vec<_>, _>>()?;
            FExpr::Func(builtin, args)
        }
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
