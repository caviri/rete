//! FILTER / BIND expression evaluation: an [`FExpr`] to a value string or an
//! effective boolean, the SPARQL built-in functions, and the ORDER BY
//! [`SortKey`]. EXISTS sub-patterns evaluate against the active graph through the
//! parent's `eval_plan_in` and memoize in the parent's `ExistsCache`.

use super::*;

use crate::bgp::Binding;
use crate::file::Rete;
use crate::index::GraphIndex;

impl FExpr {
    /// Evaluate to a value string against a binding (variables resolve, errors
    /// yield `None`).
    pub(super) fn value(&self, b: &Binding) -> Option<String> {
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
    pub(super) fn boolean(
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

/// A precomputed ORDER BY sort key: unbound (`None`) sorts before bound values;
/// bound values compare numerically when both are numbers, else lexically. The
/// numeric value is parsed once at construction so comparisons never re-parse
/// (same ordering as a numeric-or-lexical compare with unbound sorting first).
pub(super) enum SortKey {
    Unbound,
    Bound(Option<f64>, String),
}

impl SortKey {
    pub(super) fn of(v: Option<String>) -> Self {
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
