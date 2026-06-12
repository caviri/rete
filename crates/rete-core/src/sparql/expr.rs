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
                let a = arith_number(&l.value(ctx, b)?)?;
                let c = arith_number(&r.value(ctx, b)?)?;
                let v = match op {
                    ArithOp::Add => a + c,
                    ArithOp::Sub => a - c,
                    ArithOp::Mul => a * c,
                    ArithOp::Div if c == 0.0 => return None,
                    ArithOp::Div => a / c,
                };
                Some(Rc::from(fmt_num_typed(v)))
            }
            FExpr::Func(f, args) => func_value(*f, args, ctx, b),
            FExpr::Coalesce(args) => args.iter().find_map(|e| e.value(ctx, b)),
            // IF propagates an error in the condition (e.g. `IF(1/0, …)` is a
            // type error, not the else-branch).
            FExpr::If(c, t, e) => match c.ebv_opt(ctx, b) {
                Some(true) => t.value(ctx, b),
                Some(false) => e.value(ctx, b),
                None => None,
            },
            // A boolean expression in value position (e.g. `(?y = ?z AS ?eq)`)
            // yields a typed xsd:boolean.
            FExpr::In(..)
            | FExpr::SameTerm(..)
            | FExpr::Compare(..)
            | FExpr::And(..)
            | FExpr::Or(..)
            | FExpr::Not(..)
            | FExpr::Bound(..) => Some(Rc::from(bool_literal(self.ebv(ctx, b)))),
            _ => None,
        }
    }

    /// Three-valued effective boolean value: `None` is an *error* (a missing
    /// value, or a term with no EBV), distinct from `Some(false)`. Used where the
    /// distinction matters — the condition of `IF`. Boolean-form sub-expressions
    /// produce a typed `xsd:boolean` through [`Self::value`], so the EBV reduces
    /// to inspecting that term.
    fn ebv_opt(&self, ctx: &Ctx, b: &Row) -> Option<bool> {
        term_ebv(&self.value(ctx, b)?)
    }

    /// Effective boolean value **without** access to the active graph, so EXISTS
    /// (which needs the graph) evaluates to `false` here. Used in value position
    /// (e.g. the condition of `IF` inside a BIND) and as the shared core of
    /// [`Self::boolean`] for the index-independent forms.
    fn ebv(&self, ctx: &Ctx, b: &Row) -> bool {
        match self {
            FExpr::Bound(v) => ctx.slots.slot(v).is_some_and(|slot| b[slot].is_some()),
            FExpr::Not(e) => !e.ebv(ctx, b),
            FExpr::And(l, r) => l.ebv(ctx, b) && r.ebv(ctx, b),
            FExpr::Or(l, r) => l.ebv(ctx, b) || r.ebv(ctx, b),
            FExpr::Compare(op, l, r) => match (l.value(ctx, b), r.value(ctx, b)) {
                (Some(a), Some(c)) => compare(*op, &a, &c),
                _ => false,
            },
            FExpr::In(e, list) => match e.value(ctx, b) {
                Some(v) => list
                    .iter()
                    .any(|x| x.value(ctx, b).is_some_and(|m| compare(Op::Eq, &v, &m))),
                None => false,
            },
            // sameTerm is strict term identity — no numeric/lexical coercion.
            FExpr::SameTerm(l, r) => match (l.value(ctx, b), r.value(ctx, b)) {
                (Some(a), Some(c)) => a == c,
                _ => false,
            },
            FExpr::If(c, t, e) => {
                if c.ebv(ctx, b) {
                    t.ebv(ctx, b)
                } else {
                    e.ebv(ctx, b)
                }
            }
            FExpr::Func(f, args) => func_bool(*f, args, ctx, b),
            _ => false,
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
            FExpr::Not(e) => !e.boolean(ctx, index, b, cache),
            FExpr::And(l, r) => l.boolean(ctx, index, b, cache) && r.boolean(ctx, index, b, cache),
            FExpr::Or(l, r) => l.boolean(ctx, index, b, cache) || r.boolean(ctx, index, b, cache),
            // IF in filter context: the chosen branch may itself contain EXISTS,
            // so recurse through the graph-aware path rather than `ebv`.
            FExpr::If(c, t, e) => {
                if c.boolean(ctx, index, b, cache) {
                    t.boolean(ctx, index, b, cache)
                } else {
                    e.boolean(ctx, index, b, cache)
                }
            }
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
            // Bound / Compare / In / sameTerm / Func — index-independent.
            _ => self.ebv(ctx, b),
        }
    }
}

/// Evaluate a value-returning built-in (string/numeric); `None` for predicates.
fn func_value(f: Builtin, args: &[FExpr], ctx: &Ctx, b: &Row) -> Option<Rc<str>> {
    let a0 = || args.first().and_then(|e| e.value(ctx, b));
    let num = |x: f64| -> Option<Rc<str>> { Some(Rc::from(fmt_num_typed(x))) };
    // Unescaped lexical value of a term token (the text the SPARQL string
    // functions operate on); `make_literal` re-escapes when building results.
    let lex = |t: &str| crate::terms::lexical(t).into_owned();
    // A plain (simple) literal result.
    let s = |x: String| -> Option<Rc<str>> { Some(Rc::from(make_literal(&x, None, None))) };
    // A string-valued result carrying `lang` (when non-empty), per the SPARQL
    // "string literal" preservation rules for UCASE/LCASE/SUBSTR/STR{BEFORE,AFTER}.
    let sl = |x: String, lang: Option<&str>| -> Option<Rc<str>> {
        Some(Rc::from(make_literal(&x, lang, None)))
    };
    match f {
        Builtin::Str => s(lex(&a0()?)),
        Builtin::StrLen => num(lex(&a0()?).chars().count() as f64),
        Builtin::UCase => {
            let a = a0()?;
            sl(lex(&a).to_uppercase(), keep_lang(&a).as_deref())
        }
        Builtin::LCase => {
            let a = a0()?;
            sl(lex(&a).to_lowercase(), keep_lang(&a).as_deref())
        }
        Builtin::Abs => num(as_number(&a0()?)?.abs()),
        Builtin::Ceil => num(as_number(&a0()?)?.ceil()),
        Builtin::Floor => num(as_number(&a0()?)?.floor()),
        Builtin::Round => num(as_number(&a0()?)?.round()),
        Builtin::Concat => {
            // Every argument must be a string literal (else a type error); the
            // result keeps the common language tag iff all share that same
            // non-empty tag, and is a simple literal otherwise.
            let mut out = String::new();
            let mut lang: Option<Option<String>> = None;
            for a in args {
                let v = a.value(ctx, b)?;
                let this = string_arg(&v)?;
                out.push_str(&lex(&v));
                lang = Some(match lang {
                    None => this,
                    Some(prev) if prev == this => prev,
                    Some(_) => None,
                });
            }
            sl(out, lang.flatten().as_deref())
        }
        Builtin::SubStr => {
            let a = a0()?;
            let chars: Vec<char> = lex(&a).chars().collect();
            let start = as_number(&args.get(1)?.value(ctx, b)?)?.max(1.0) as usize - 1;
            let it = chars.iter().skip(start);
            let out: String = match args.get(2) {
                Some(lenarg) => it
                    .take(as_number(&lenarg.value(ctx, b)?)?.max(0.0) as usize)
                    .collect(),
                None => it.collect(),
            };
            sl(out, keep_lang(&a).as_deref())
        }
        // STRBEFORE/STRAFTER: arg1 and arg2 must both be string literals and be
        // argument-compatible (arg2 simple, or sharing arg1's tag) — else a type
        // error. A found needle (or empty needle) yields a result carrying
        // arg1's language tag; a *not-found* needle yields a simple literal "".
        Builtin::StrBefore => {
            let a = a0()?;
            let lang = before_after_lang(&a, &args.get(1)?.value(ctx, b)?)?;
            let (t, needle) = (lex(&a), lex(&args.get(1)?.value(ctx, b)?));
            match (needle.is_empty(), t.find(&needle)) {
                (true, _) => sl(String::new(), lang.as_deref()),
                (false, Some(i)) => sl(t[..i].to_string(), lang.as_deref()),
                (false, None) => s(String::new()),
            }
        }
        Builtin::StrAfter => {
            let a = a0()?;
            let lang = before_after_lang(&a, &args.get(1)?.value(ctx, b)?)?;
            let (t, needle) = (lex(&a), lex(&args.get(1)?.value(ctx, b)?));
            match (needle.is_empty(), t.find(&needle)) {
                (true, _) => sl(t, lang.as_deref()),
                (false, Some(i)) => sl(t[i + needle.len()..].to_string(), lang.as_deref()),
                (false, None) => s(String::new()),
            }
        }
        // STRDT/STRLANG require a simple string arg1 (RDF 1.1): a lang-tagged or
        // otherwise-typed literal is a type error.
        Builtin::StrDt => {
            let v = simple_string(&a0()?)?;
            let dt = iri_content(&args.get(1)?.value(ctx, b)?)?.to_string();
            Some(Rc::from(make_literal(&v, None, Some(&dt))))
        }
        Builtin::StrLang => {
            let v = simple_string(&a0()?)?;
            let lang = lex(&args.get(1)?.value(ctx, b)?);
            Some(Rc::from(make_literal(&v, Some(&lang), None)))
        }
        // IRI()/URI(): an IRI argument passes through; a string becomes <...>.
        Builtin::Iri => {
            let a = a0()?;
            if is_iri(&a) {
                Some(a)
            } else {
                Some(Rc::from(format!("<{}>", lex(&a))))
            }
        }
        Builtin::EncodeForUri => s(encode_for_uri(&lex(&a0()?))),
        Builtin::Replace => {
            let a = a0()?;
            let lang = string_arg(&a)?; // arg1 must be a string literal
            let text = lex(&a);
            let pat = lex(&args.get(1)?.value(ctx, b)?);
            let rep = lex(&args.get(2)?.value(ctx, b)?);
            let flags = match args.get(3) {
                Some(e) => lex(&e.value(ctx, b)?),
                None => String::new(),
            };
            let out = ctx.resolver.regex_replace(&pat, &flags, &text, &rep)?;
            sl(out, lang.as_deref())
        }
        Builtin::Md5 | Builtin::Sha1 | Builtin::Sha256 | Builtin::Sha384 | Builtin::Sha512 => {
            s(hash_hex(f, &lex(&a0()?)))
        }
        // Date/time accessors over an xsd:dateTime lexical form.
        Builtin::Year => num(parse_datetime(&lex(&a0()?))?.0 as f64),
        Builtin::Month => num(parse_datetime(&lex(&a0()?))?.1 as f64),
        Builtin::Day => num(parse_datetime(&lex(&a0()?))?.2 as f64),
        Builtin::Hours => num(parse_datetime(&lex(&a0()?))?.3 as f64),
        Builtin::Minutes => num(parse_datetime(&lex(&a0()?))?.4 as f64),
        Builtin::Seconds => num(parse_datetime(&lex(&a0()?))?.5),
        // TZ → the timezone as a simple literal ("Z", "-08:00", or "").
        Builtin::Tz => s(parse_datetime(&lex(&a0()?))?.6),
        // TIMEZONE → an xsd:dayTimeDuration; a value with no timezone errors.
        Builtin::Timezone => {
            let dur = tz_to_duration(&parse_datetime(&lex(&a0()?))?.6)?;
            Some(Rc::from(make_literal(
                &dur,
                None,
                Some("http://www.w3.org/2001/XMLSchema#dayTimeDuration"),
            )))
        }
        Builtin::CastInteger
        | Builtin::CastDecimal
        | Builtin::CastFloat
        | Builtin::CastDouble
        | Builtin::CastBoolean
        | Builtin::CastString => cast_to(&a0()?, f),
        // RAND() → an xsd:double in [0, 1).
        Builtin::Rand => {
            let r = (random_u64() >> 11) as f64 / (1u64 << 53) as f64;
            Some(Rc::from(make_literal(
                &format!("{r}"),
                None,
                Some("http://www.w3.org/2001/XMLSchema#double"),
            )))
        }
        // UUID() → a urn:uuid: IRI; STRUUID() → the bare UUID as a simple literal.
        Builtin::Uuid => Some(Rc::from(format!("<urn:uuid:{}>", uuid_v4()))),
        Builtin::StrUuid => s(uuid_v4()),
        // BNODE(): a fresh blank node; BNODE("str"): a blank node keyed by the
        // string (same string → same node within a result, via a stable hash).
        Builtin::BNode => match args.first().and_then(|e| e.value(ctx, b)) {
            Some(v) => {
                use std::hash::{Hash, Hasher};
                let mut h = std::collections::hash_map::DefaultHasher::new();
                lex(&v).hash(&mut h);
                Some(Rc::from(format!("_:b{:016x}", h.finish())))
            }
            None => Some(Rc::from(format!("_:b{:016x}", random_u64()))),
        },
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

/// The datatype IRI of a literal term (see [`Builtin::Datatype`]). Returns the
/// IRI content without angle brackets; the call site adds them.
use crate::terms::{
    iri_content, is_iri, lang_tag as lang_of, literal_datatype as datatype_iri, make_literal,
};

/// The language tag to carry onto a string-function result: the argument's tag
/// when it is a literal with a non-empty tag, else `None`.
fn keep_lang(token: &str) -> Option<String> {
    lang_of(token).filter(|l| !l.is_empty())
}

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";

/// If `token` is a *string* literal — plain, `xsd:string`, or language-tagged —
/// returns its language tag (`None` when untagged). Returns the outer `None`
/// for a non-string term (IRI, blank, or non-string typed literal), which the
/// string built-ins treat as a type error.
fn string_arg(token: &str) -> Option<Option<String>> {
    if !is_iri(token) && token.starts_with('"') {
        if let Some(l) = lang_of(token).filter(|l| !l.is_empty()) {
            return Some(Some(l));
        }
        if datatype_iri(token).as_deref() == Some(XSD_STRING) {
            return Some(None);
        }
    }
    None
}

/// The unescaped lexical value of `token` iff it is a *simple* string literal
/// (no language tag, datatype `xsd:string` or none); `None` otherwise. STRDT
/// and STRLANG require this of their first argument (RDF 1.1).
fn simple_string(token: &str) -> Option<String> {
    match string_arg(token) {
        Some(None) => Some(crate::terms::lexical(token).into_owned()),
        _ => None,
    }
}

/// STRBEFORE/STRAFTER: both arguments must be string literals and be
/// argument-compatible (`arg2` simple, or sharing `arg1`'s tag). Returns the
/// tag to put on the result (`arg1`'s), or `None` for a type error.
fn before_after_lang(arg1: &str, arg2: &str) -> Option<Option<String>> {
    let l1 = string_arg(arg1)?;
    let l2 = string_arg(arg2)?;
    match l2 {
        None => Some(l1),
        Some(_) if l2 == l1 => Some(l1),
        Some(_) => None,
    }
}

/// Parse an `xsd:dateTime` lexical form `[-]YYYY-MM-DDThh:mm:ss[.fff][TZ]` into
/// `(year, month, day, hour, minute, second, timezone)`. The timezone is the
/// raw suffix (`"Z"`, `"-08:00"`, …) or `""` when absent.
#[allow(clippy::type_complexity)]
fn parse_datetime(lex: &str) -> Option<(i64, u32, u32, u32, u32, f64, String)> {
    let (neg, rest) = match lex.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, lex),
    };
    let (date, timetz) = rest.split_once('T')?;
    let mut dp = date.split('-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: u32 = dp.next()?.parse().ok()?;
    let day: u32 = dp.next()?.parse().ok()?;
    let (time, tz) = if let Some(i) = timetz.find('Z') {
        (&timetz[..i], &timetz[i..])
    } else if let Some(i) = timetz.rfind(['+', '-']) {
        (&timetz[..i], &timetz[i..])
    } else {
        (timetz, "")
    };
    let mut tp = time.split(':');
    let hour: u32 = tp.next()?.parse().ok()?;
    let minute: u32 = tp.next()?.parse().ok()?;
    let second: f64 = tp.next()?.parse().ok()?;
    let year = if neg { -year } else { year };
    Some((year, month, day, hour, minute, second, tz.to_string()))
}

/// Convert a dateTime timezone suffix to an `xsd:dayTimeDuration` lexical form
/// (`"Z"` → `PT0S`, `"-08:00"` → `-PT8H`). `None` (a type error) when there is
/// no timezone, which is what `TIMEZONE` returns for an unzoned value.
fn tz_to_duration(tz: &str) -> Option<String> {
    if tz.is_empty() {
        return None;
    }
    if tz == "Z" {
        return Some("PT0S".to_string());
    }
    let sign = tz.starts_with('-');
    let h: u32 = tz.get(1..3)?.parse().ok()?;
    let m: u32 = tz.get(4..6)?.parse().ok()?;
    if h == 0 && m == 0 {
        return Some("PT0S".to_string());
    }
    let mut out = String::new();
    if sign {
        out.push('-');
    }
    out.push_str("PT");
    if h > 0 {
        out.push_str(&format!("{h}H"));
    }
    if m > 0 {
        out.push_str(&format!("{m}M"));
    }
    Some(out)
}

/// The effective boolean value of a term, or `None` (a type error) for a term
/// that has no EBV (an IRI, blank node, or non-boolean/numeric/string literal).
fn term_ebv(token: &str) -> Option<bool> {
    match datatype_iri(token).as_deref() {
        Some("http://www.w3.org/2001/XMLSchema#boolean") => {
            match crate::terms::lexical(token).as_ref() {
                "true" | "1" => Some(true),
                "false" | "0" => Some(false),
                _ => None,
            }
        }
        Some(dt) if is_numeric_dt(Some(dt)) => as_number(token).map(|n| n != 0.0 && !n.is_nan()),
        Some(XSD_STRING) => Some(!crate::terms::lexical(token).is_empty()),
        _ => None,
    }
}

/// 8 random bytes as a `u64` (0 on the unlikely RNG failure — keeps the builtin
/// total rather than erroring).
fn random_u64() -> u64 {
    let mut buf = [0u8; 8];
    let _ = getrandom::getrandom(&mut buf);
    u64::from_le_bytes(buf)
}

/// A random version-4 UUID in canonical `8-4-4-4-12` hex form.
fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::getrandom(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40; // version 4
    b[8] = (b[8] & 0x3f) | 0x80; // variant 1
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Numeric value of a term for **arithmetic** (`+ - * /`): only numeric-typed
/// literals count; a string (or other non-numeric) literal is a type error
/// (`None`). This is stricter than the lenient [`as_number`] used for ordering
/// and REGEX flags, which parses any literal's lexical form.
fn arith_number(token: &str) -> Option<f64> {
    match datatype_iri(token) {
        Some(dt) => is_numeric_dt(Some(&dt)).then(|| as_number(token)).flatten(),
        None => as_number(token),
    }
}

/// The XSD numeric datatype IRIs accepted as a numeric *source* for a cast.
fn is_numeric_dt(dt: Option<&str>) -> bool {
    matches!(
        dt,
        Some(
            "http://www.w3.org/2001/XMLSchema#integer"
                | "http://www.w3.org/2001/XMLSchema#decimal"
                | "http://www.w3.org/2001/XMLSchema#float"
                | "http://www.w3.org/2001/XMLSchema#double"
        )
    )
}

fn is_int_lexical(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    !body.is_empty() && body.bytes().all(|b| b.is_ascii_digit())
}

fn is_decimal_lexical(s: &str) -> bool {
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |x: &str| x.bytes().all(|b| b.is_ascii_digit());
    (!int.is_empty() || !frac.is_empty()) && digits(int) && digits(frac)
}

/// Parse an XSD double/float lexical form to `f64` (handling `INF`/`-INF`/`NaN`).
fn parse_xsd_double(s: &str) -> Option<f64> {
    match s {
        "INF" | "+INF" => Some(f64::INFINITY),
        "-INF" => Some(f64::NEG_INFINITY),
        "NaN" => Some(f64::NAN),
        // Reject the lowercase spellings Rust would otherwise accept.
        _ if s.eq_ignore_ascii_case("inf")
            || s.eq_ignore_ascii_case("nan")
            || s.eq_ignore_ascii_case("infinity") =>
        {
            None
        }
        _ => s.parse::<f64>().ok(),
    }
}

/// XSD constructor casts (`xsd:integer(x)`, …). String sources are validated
/// against the target's lexical space (an invalid form is a type error →
/// `None`); numeric/boolean *typed* sources convert by value. The harness
/// compares numerics by value, so only validity and the numeric value matter.
fn cast_to(token: &str, target: Builtin) -> Option<Rc<str>> {
    const NS: &str = "http://www.w3.org/2001/XMLSchema#";
    let dt = datatype_iri(token);
    let is_string_src = dt.as_deref() == Some(XSD_STRING);
    let is_numeric_src = is_numeric_dt(dt.as_deref());
    let is_bool_src = dt.as_deref() == Some("http://www.w3.org/2001/XMLSchema#boolean");
    let raw = crate::terms::lexical(token);
    let src = raw.trim();
    let typed = |lex: String, ty: &str| {
        Some(Rc::from(make_literal(
            &lex,
            None,
            Some(&format!("{NS}{ty}")),
        )))
    };
    // The numeric value of a numeric- or boolean-typed source (`true` → 1).
    let bool_true = src == "true" || src == "1";
    let numeric_src = || -> Option<f64> {
        if is_numeric_src {
            as_number(token)
        } else if is_bool_src {
            Some(if bool_true { 1.0 } else { 0.0 })
        } else {
            None
        }
    };
    match target {
        // Numeric/boolean sources canonicalize (0.0 → "0", "0"^^xsd:boolean →
        // "false"); a string literal or IRI keeps its lexical value.
        Builtin::CastString if is_numeric_src => typed(fmt_plain(as_number(token)?), "string"),
        Builtin::CastString if is_bool_src => {
            typed(if bool_true { "true" } else { "false" }.into(), "string")
        }
        Builtin::CastString if dt.is_some() || is_iri(token) => typed(raw.into_owned(), "string"),
        Builtin::CastBoolean if is_bool_src => {
            typed(if bool_true { "true" } else { "false" }.into(), "boolean")
        }
        Builtin::CastBoolean if is_string_src => match src {
            "true" | "1" => typed("true".into(), "boolean"),
            "false" | "0" => typed("false".into(), "boolean"),
            _ => None,
        },
        Builtin::CastBoolean if is_numeric_src => typed(
            if as_number(token)? != 0.0 {
                "true"
            } else {
                "false"
            }
            .into(),
            "boolean",
        ),
        Builtin::CastInteger => {
            let n: i64 = if is_string_src {
                is_int_lexical(src)
                    .then(|| src.trim_start_matches('+').parse().ok())
                    .flatten()?
            } else {
                numeric_src()?.trunc() as i64
            };
            typed(n.to_string(), "integer")
        }
        Builtin::CastDecimal => {
            let v: f64 = if is_string_src {
                is_decimal_lexical(src)
                    .then(|| src.parse().ok())
                    .flatten()?
            } else {
                numeric_src()?
            };
            typed(fmt_plain(v), "decimal")
        }
        Builtin::CastFloat | Builtin::CastDouble => {
            let v: f64 = if is_string_src {
                parse_xsd_double(src)?
            } else {
                numeric_src()?
            };
            let ty = if matches!(target, Builtin::CastFloat) {
                "float"
            } else {
                "double"
            };
            typed(fmt_plain(v), ty)
        }
        _ => None,
    }
}

/// Format an `f64` as a plain decimal lexical (no forced scientific notation);
/// whole values print without a trailing `.0`.
fn fmt_plain(v: f64) -> String {
    if v == v.trunc() && v.is_finite() {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// `"true"`/`"false"` typed as `xsd:boolean` — a boolean expression's value in
/// value position (e.g. `BIND(?a IN (1,2) AS ?x)`).
fn bool_literal(v: bool) -> String {
    let lex = if v { "true" } else { "false" };
    format!("\"{lex}\"^^<http://www.w3.org/2001/XMLSchema#boolean>")
}

/// Percent-encode for `ENCODE_FOR_URI`: unreserved characters (RFC 3986
/// `ALPHA / DIGIT / - . _ ~`) pass through; everything else becomes `%XX` over
/// the UTF-8 bytes.
fn encode_for_uri(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Lowercase hex digest for the SPARQL hash built-ins, over the UTF-8 bytes of
/// the lexical value.
fn hash_hex(f: Builtin, s: &str) -> String {
    use sha2::Digest;
    let bytes = s.as_bytes();
    match f {
        Builtin::Md5 => format!("{:x}", md5::Md5::digest(bytes)),
        Builtin::Sha1 => format!("{:x}", sha1::Sha1::digest(bytes)),
        Builtin::Sha256 => format!("{:x}", sha2::Sha256::digest(bytes)),
        Builtin::Sha384 => format!("{:x}", sha2::Sha384::digest(bytes)),
        Builtin::Sha512 => format!("{:x}", sha2::Sha512::digest(bytes)),
        _ => unreachable!("hash_hex called with non-hash builtin"),
    }
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
        // LANGMATCHES(tag, range): basic-filtering language-range match
        // (case-insensitive; "*" matches any non-empty tag).
        Builtin::LangMatches => match (val(0), val(1)) {
            (Some(tag), Some(range)) => lang_matches(&lexical(&tag), &lexical(&range)),
            _ => false,
        },
        _ => false,
    }
}

/// RFC 4647 basic-filtering match of a language `tag` against a `range`.
fn lang_matches(tag: &str, range: &str) -> bool {
    if range == "*" {
        return !tag.is_empty();
    }
    let (tag, range) = (tag.to_ascii_lowercase(), range.to_ascii_lowercase());
    tag == range || tag.starts_with(&format!("{range}-"))
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
