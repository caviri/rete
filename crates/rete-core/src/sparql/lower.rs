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
    let plan = build(pattern, &mut sel)?;
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
                // An `xsd:<type>(expr)` constructor parses as a call to the
                // datatype IRI — map the supported XSD casts.
                Function::Custom(nn) => match nn.as_str() {
                    "http://www.w3.org/2001/XMLSchema#integer" => Builtin::CastInteger,
                    "http://www.w3.org/2001/XMLSchema#decimal" => Builtin::CastDecimal,
                    "http://www.w3.org/2001/XMLSchema#float" => Builtin::CastFloat,
                    "http://www.w3.org/2001/XMLSchema#double" => Builtin::CastDouble,
                    "http://www.w3.org/2001/XMLSchema#boolean" => Builtin::CastBoolean,
                    "http://www.w3.org/2001/XMLSchema#string" => Builtin::CastString,
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
