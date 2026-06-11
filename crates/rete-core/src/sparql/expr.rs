//! FILTER / BIND expression evaluation: an [`FExpr`] to a value string or an
//! effective boolean, the SPARQL built-in functions, and the ORDER BY
//! [`SortKey`]. Expressions evaluate against slot [`Row`]s; variables resolve
//! through the per-query memoizing resolver, so a repeated term decodes once
//! per query rather than once per row. EXISTS sub-patterns evaluate against the
//! active graph through the parent's `eval_plan_in` and memoize in the parent's
//! `ExistsCache`.

use std::rc::Rc;

use super::*;
use crate::index::GraphIndex;
use crate::row::{Ctx, Row};

impl FExpr {
    /// Evaluate to a value string against a row (variables resolve, errors
    /// yield `None`).
    pub(super) fn value(&self, ctx: &Ctx, b: &Row) -> Option<Rc<str>> {
        match self {
            FExpr::Var(v) => {
                let slot = ctx.slots.slot(v)?;
                b[slot].as_ref().and_then(|val| ctx.resolver.str_of(val))
            }
            FExpr::Const(c) => Some(Rc::from(c.as_str())),
            FExpr::Arith(op, l, r) => {
                let a = as_number(&l.value(ctx, b)?)?;
                let c = as_number(&r.value(ctx, b)?)?;
                let v = match op {
                    ArithOp::Add => a + c,
                    ArithOp::Sub => a - c,
                    ArithOp::Mul => a * c,
                    ArithOp::Div if c == 0.0 => return None,
                    ArithOp::Div => a / c,
                };
                Some(Rc::from(fmt_num(v)))
            }
            FExpr::Func(f, args) => func_value(*f, args, ctx, b),
            FExpr::Coalesce(args) => args.iter().find_map(|e| e.value(ctx, b)),
            _ => None,
        }
    }

    /// Evaluate as a boolean (SPARQL effective boolean value, simplified:
    /// unbound/error → false). `index` is the active graph (so EXISTS evaluates
    /// in the current GRAPH context).
    pub(super) fn boolean(
        &self,
        ctx: &Ctx,
        index: &GraphIndex,
        b: &Row,
        cache: &mut ExistsCache,
    ) -> bool {
        match self {
            FExpr::Bound(v) => ctx.slots.slot(v).is_some_and(|slot| b[slot].is_some()),
            FExpr::Not(e) => !e.boolean(ctx, index, b, cache),
            FExpr::And(l, r) => l.boolean(ctx, index, b, cache) && r.boolean(ctx, index, b, cache),
            FExpr::Or(l, r) => l.boolean(ctx, index, b, cache) || r.boolean(ctx, index, b, cache),
            FExpr::Compare(op, l, r) => match (l.value(ctx, b), r.value(ctx, b)) {
                (Some(a), Some(c)) => compare(*op, &a, &c),
                _ => false,
            },
            FExpr::Exists(plan) => {
                let key = plan.as_ref() as *const Plan;
                let entry = cache.entry(key).or_insert_with(|| ExistsEntry {
                    sols: eval_plan_in(ctx, index, None, plan),
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
            FExpr::Func(f, args) => func_bool(*f, args, ctx, b),
            _ => false,
        }
    }
}

/// Evaluate a value-returning built-in (string/numeric); `None` for predicates.
fn func_value(f: Builtin, args: &[FExpr], ctx: &Ctx, b: &Row) -> Option<Rc<str>> {
    let a0 = || args.first().and_then(|e| e.value(ctx, b));
    let num = |x: f64| -> Option<Rc<str>> { Some(Rc::from(fmt_num(x))) };
    let s = |x: String| -> Option<Rc<str>> { Some(Rc::from(x)) };
    match f {
        Builtin::Str => s(lexical(&a0()?)),
        Builtin::StrLen => num(lexical(&a0()?).chars().count() as f64),
        Builtin::UCase => s(lexical(&a0()?).to_uppercase()),
        Builtin::LCase => s(lexical(&a0()?).to_lowercase()),
        Builtin::Abs => num(as_number(&a0()?)?.abs()),
        Builtin::Ceil => num(as_number(&a0()?)?.ceil()),
        Builtin::Floor => num(as_number(&a0()?)?.floor()),
        Builtin::Round => num(as_number(&a0()?)?.round()),
        Builtin::Concat => {
            let mut out = String::new();
            for a in args {
                out.push_str(&lexical(&a.value(ctx, b)?));
            }
            s(out)
        }
        Builtin::SubStr => {
            let chars: Vec<char> = lexical(&a0()?).chars().collect();
            let start = as_number(&args.get(1)?.value(ctx, b)?)?.max(1.0) as usize - 1;
            let it = chars.iter().skip(start);
            let out: String = match args.get(2) {
                Some(lenarg) => it
                    .take(as_number(&lenarg.value(ctx, b)?)?.max(0.0) as usize)
                    .collect(),
                None => it.collect(),
            };
            s(out)
        }
        Builtin::StrBefore => {
            let t = lexical(&a0()?);
            let sub = lexical(&args.get(1)?.value(ctx, b)?);
            s(t.find(&sub).map(|i| t[..i].to_string()).unwrap_or_default())
        }
        Builtin::StrAfter => {
            let t = lexical(&a0()?);
            let sub = lexical(&args.get(1)?.value(ctx, b)?);
            s(t.find(&sub)
                .map(|i| t[i + sub.len()..].to_string())
                .unwrap_or_default())
        }
        // DATATYPE(literal) → the datatype IRI term (`<...>`): the explicit
        // `^^<dt>`, else `rdf:langString` for a language-tagged literal, else
        // `xsd:string` for a plain one. A non-literal (IRI/blank) is a type
        // error → `None` (FILTER sees it as false).
        Builtin::Datatype => datatype_iri(&a0()?).map(|iri| Rc::from(format!("<{iri}>"))),
        // LANG(literal) → its language tag as a plain literal (`"en"`), or `""`
        // for a non-language-tagged literal; non-literal → `None`.
        Builtin::Lang => lang_of(&a0()?).map(|l| Rc::from(format!("\"{l}\""))),
        _ => None,
    }
}

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANGSTRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// The part of a literal term token after its closing quote (`"x"^^<dt>` →
/// `^^<dt>`, `"x"@en` → `@en`, `"x"` → ``), or `None` if `t` is not a literal.
/// Uses the last quote so embedded escaped quotes don't confuse the split.
fn literal_suffix(t: &str) -> Option<&str> {
    if !t.starts_with('"') {
        return None;
    }
    let close = t.rfind('"')?;
    (close > 0).then(|| &t[close + 1..])
}

/// The datatype IRI of a literal term (see [`Builtin::Datatype`]).
fn datatype_iri(t: &str) -> Option<String> {
    let suffix = literal_suffix(t)?;
    if let Some(dt) = suffix.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
        Some(dt.to_string())
    } else if suffix.starts_with('@') {
        Some(RDF_LANGSTRING.to_string())
    } else if suffix.is_empty() {
        Some(XSD_STRING.to_string())
    } else {
        None
    }
}

/// The language tag of a literal term, `""` when untagged (see [`Builtin::Lang`]).
fn lang_of(t: &str) -> Option<String> {
    literal_suffix(t).map(|suffix| suffix.strip_prefix('@').unwrap_or("").to_string())
}

/// Evaluate a boolean built-in (type checks / string predicates).
fn func_bool(f: Builtin, args: &[FExpr], ctx: &Ctx, b: &Row) -> bool {
    let val = |i: usize| args.get(i).and_then(|e| e.value(ctx, b));
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
        // The matcher is compiled once per query (memoized); literal patterns
        // skip the regex engine entirely.
        Builtin::Regex => match (val(0), val(1)) {
            (Some(text), Some(pat)) => {
                let flags = val(2).map(|t| lexical(&t)).unwrap_or_default();
                ctx.resolver
                    .regex_match(&lexical(&pat), &flags, &lexical(&text))
            }
            _ => false,
        },
        _ => false,
    }
}

/// A precomputed ORDER BY sort key: unbound (`None`) sorts before bound values;
/// bound values compare numerically when both are numbers, else lexically. The
/// numeric value is parsed once at construction so comparisons never re-parse
/// (same ordering as a numeric-or-lexical compare with unbound sorting first).
pub(super) enum SortKey {
    Unbound,
    Bound(Option<f64>, Rc<str>),
}

impl SortKey {
    pub(super) fn of(v: Option<Rc<str>>) -> Self {
        match v {
            None => SortKey::Unbound,
            Some(s) => SortKey::Bound(as_number(&s), s),
        }
    }

    pub(super) fn cmp(&self, other: &SortKey) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (self, other) {
            (SortKey::Unbound, SortKey::Unbound) => Ordering::Equal,
            (SortKey::Unbound, _) => Ordering::Less,
            (_, SortKey::Unbound) => Ordering::Greater,
            (SortKey::Bound(na, sa), SortKey::Bound(nb, sb)) => match (na, nb) {
                (Some(x), Some(y)) => x.partial_cmp(y).unwrap_or(Ordering::Equal),
                _ => sa.cmp(sb),
            },
        }
    }
}
