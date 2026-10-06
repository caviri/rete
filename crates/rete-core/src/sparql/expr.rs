//! FILTER / BIND expression evaluation: an [`FExpr`] to a value string or an
//! effective boolean, the SPARQL built-in functions, and the ORDER BY
//! [`SortKey`]. Expressions evaluate against slot [`Row`]s; variables resolve
//! through the per-query memoizing resolver, so a repeated term decodes once
//! per query rather than once per row. EXISTS sub-patterns evaluate against the
//! active graph through the parent's `eval_plan_in` and memoize in the parent's
//! `ExistsCache`.
//!
//! **Errors are values here, not `false`.** SPARQL 1.1 §17.2/§17.3: an
//! expression yields an RDF term *or an error*, and the error propagates
//! through every function and operator except `||`, `&&` (three-valued),
//! `IF` (only its condition), `COALESCE` (skips it) and `BOUND` (never errors).
//! [`FExpr::value`] carries an error as `None`, and the boolean evaluators carry
//! it as `None` of an `Option<bool>` ([`FExpr::ebv3`], [`FExpr::boolean3`]).
//! It is resolved only where the spec says: a FILTER / HAVING / OPTIONAL
//! condition keeps a row only on `Some(true)` ([`FExpr::boolean`]), a BIND or
//! projection leaves the variable unbound, ORDER BY sorts it as "no value".
//! Collapsing it to `false` any earlier made `!CONTAINS(?iri, "x")` true.

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
            // A boolean built-in (isIRI, CONTAINS, REGEX, …) in value position
            // (BIND, a projected expression, a COALESCE or IF argument) is an
            // xsd:boolean (§17.4.2, §17.4.3), or an error. It used to fall
            // through to `func_value`, which has no value for it: unbound.
            FExpr::Func(f, args) if is_boolean_builtin(*f) => {
                func_bool(*f, args, ctx, b).map(|v| Rc::from(bool_literal(v)))
            }
            FExpr::Func(f, args) => func_value(*f, args, ctx, b),
            // COALESCE: the first argument that evaluates without error.
            FExpr::Coalesce(args) => args.iter().find_map(|e| e.value(ctx, b)),
            // IF propagates an error in the condition (e.g. `IF(1/0, …)` is a
            // type error, not the else-branch); only the chosen branch runs.
            FExpr::If(c, t, e) => match c.ebv3(ctx, b)? {
                true => t.value(ctx, b),
                false => e.value(ctx, b),
            },
            // A boolean expression in value position (e.g. `(?y = ?z AS ?eq)`)
            // yields a typed xsd:boolean — or an error (unbound), never a
            // `false` standing in for one.
            FExpr::In(..)
            | FExpr::SameTerm(..)
            | FExpr::Compare(..)
            | FExpr::And(..)
            | FExpr::Or(..)
            | FExpr::Not(..)
            | FExpr::Bound(..) => self.ebv3(ctx, b).map(|v| Rc::from(bool_literal(v))),
            _ => None,
        }
    }

    /// Three-valued effective boolean value **without** access to the active
    /// graph: `Some(true)`, `Some(false)`, or `None` for an *error* (§17.2.2 —
    /// an unbound argument, a type error inside, or a term that has no EBV).
    /// EXISTS needs the graph, so it evaluates to `false` here (value
    /// position); [`Self::boolean3`] is the graph-aware form.
    pub(super) fn ebv3(&self, ctx: &Ctx, b: &Row) -> Option<bool> {
        match self {
            // BOUND never errors.
            FExpr::Bound(v) => Some(ctx.slots.slot(v).is_some_and(|slot| b[slot].is_some())),
            // fn:not of an error is an error (§17.3).
            FExpr::Not(e) => e.ebv3(ctx, b).map(|v| !v),
            FExpr::And(l, r) => logical_and(l.ebv3(ctx, b), || r.ebv3(ctx, b)),
            FExpr::Or(l, r) => logical_or(l.ebv3(ctx, b), || r.ebv3(ctx, b)),
            FExpr::Compare(op, l, r) => {
                let a = l.value(ctx, b)?;
                let c = r.value(ctx, b)?;
                Some(compare(*op, &a, &c))
            }
            FExpr::In(e, list) => in_list(&e.value(ctx, b)?, list, ctx, b),
            // sameTerm is strict term identity — no numeric/lexical coercion.
            FExpr::SameTerm(l, r) => {
                let a = l.value(ctx, b)?;
                let c = r.value(ctx, b)?;
                Some(a == c)
            }
            FExpr::If(c, t, e) => match c.ebv3(ctx, b)? {
                true => t.ebv3(ctx, b),
                false => e.ebv3(ctx, b),
            },
            FExpr::Func(f, args) if is_boolean_builtin(*f) => func_bool(*f, args, ctx, b),
            FExpr::Exists(_) => Some(false),
            // A value-form expression used as a boolean (a bare `true` literal, a
            // `?var`, arithmetic, COALESCE, a value function such as STR …)
            // takes the effective boolean value of its computed term; a term
            // with no EBV, or an error computing it, is an error.
            _ => term_ebv(&self.value(ctx, b)?),
        }
    }

    /// Three-valued, graph-aware effective boolean value (see [`Self::ebv3`]).
    /// `index` is the active graph, so EXISTS evaluates in the current GRAPH
    /// context.
    pub(super) fn boolean3(
        &self,
        ctx: &Ctx,
        index: &GraphIndex,
        b: &Row,
        cache: &mut ExistsCache,
    ) -> Option<bool> {
        match self {
            FExpr::Not(e) => e.boolean3(ctx, index, b, cache).map(|v| !v),
            FExpr::And(l, r) => {
                let lv = l.boolean3(ctx, index, b, cache);
                logical_and(lv, || r.boolean3(ctx, index, b, cache))
            }
            FExpr::Or(l, r) => {
                let lv = l.boolean3(ctx, index, b, cache);
                logical_or(lv, || r.boolean3(ctx, index, b, cache))
            }
            // IF in filter context: the chosen branch may itself contain EXISTS,
            // so recurse through the graph-aware path rather than `ebv3`.
            FExpr::If(c, t, e) => match c.boolean3(ctx, index, b, cache)? {
                true => t.boolean3(ctx, index, b, cache),
                false => e.boolean3(ctx, index, b, cache),
            },
            FExpr::Exists(plan) => {
                let key = plan.as_ref() as *const Plan;
                let entry = cache.entry(key).or_insert_with(|| ExistsEntry {
                    sols: eval_plan_in(ctx, index, None, plan),
                    probe: None,
                });
                if entry.sols.is_empty() {
                    return Some(false);
                }
                if entry.probe.is_none() {
                    entry.probe = Some(build_exists_probe(b, &entry.sols));
                }
                Some(exists_matches(b, entry))
            }
            // Bound / Compare / In / sameTerm / Func — index-independent.
            _ => self.ebv3(ctx, b),
        }
    }

    /// The FILTER / HAVING / OPTIONAL-condition verdict: keep the row only when
    /// the expression is `true`. This is the one place an error is resolved —
    /// "a solution is eliminated when its filter expression is false **or an
    /// error**" (§17.2) — so `!error` is still an error here, not `true`.
    #[inline]
    pub(super) fn boolean(
        &self,
        ctx: &Ctx,
        index: &GraphIndex,
        b: &Row,
        cache: &mut ExistsCache,
    ) -> bool {
        self.boolean3(ctx, index, b, cache) == Some(true)
    }
}

/// SPARQL logical-and over the §17.2 truth table: `F && E = E && F = F`,
/// `T && T = T`, every other combination with an error is an error. The right
/// operand is skipped only when the left is `false` (it cannot change the
/// answer); after a left error it still runs, because `E && F` is `F`.
#[inline]
fn logical_and(l: Option<bool>, r: impl FnOnce() -> Option<bool>) -> Option<bool> {
    match l {
        Some(false) => Some(false),
        Some(true) => r(),
        None => match r() {
            Some(false) => Some(false),
            _ => None,
        },
    }
}

/// SPARQL logical-or over the §17.2 truth table: `T || E = E || T = T`,
/// `F || F = F`, every other combination with an error is an error.
#[inline]
fn logical_or(l: Option<bool>, r: impl FnOnce() -> Option<bool>) -> Option<bool> {
    match l {
        Some(true) => Some(true),
        Some(false) => r(),
        None => match r() {
            Some(true) => Some(true),
            _ => None,
        },
    }
}

/// `lhs IN (list)` per §17.4.1.9: `true` as soon as a member compares equal;
/// otherwise an error if any member failed to evaluate, else `false`. (NOT IN
/// lowers to `!(… IN …)`, which §17.4.1.10 states is equivalent.)
fn in_list(lhs: &str, list: &[FExpr], ctx: &Ctx, b: &Row) -> Option<bool> {
    let mut errored = false;
    for x in list {
        match x.value(ctx, b) {
            Some(m) if compare(Op::Eq, lhs, &m) => return Some(true),
            Some(_) => {}
            None => errored = true,
        }
    }
    (!errored).then_some(false)
}

/// The builtins [`func_bool`] evaluates — the ones whose result *is* a
/// boolean. Every other builtin used as a condition takes the effective boolean
/// value of the term it returns.
fn is_boolean_builtin(f: Builtin) -> bool {
    matches!(
        f,
        Builtin::IsIri
            | Builtin::IsBlank
            | Builtin::IsLiteral
            | Builtin::IsNumeric
            | Builtin::IsTriple
            | Builtin::Contains
            | Builtin::StrStarts
            | Builtin::StrEnds
            | Builtin::Regex
            | Builtin::LangMatches
            | Builtin::GeoSfContains
            | Builtin::GeoSfWithin
            | Builtin::GeoSfIntersects
            | Builtin::GeoSfDisjoint
            | Builtin::GeoSfEquals
            | Builtin::Geo3Contains
            | Builtin::Geo3Within
            | Builtin::Geo3Adjacent
    )
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
    // The xsd:dateTime parts of argument 1, reporting a value that is not one.
    let dtm = || {
        let a = a0()?;
        let r = parse_datetime(&lex(&a));
        if r.is_none() {
            diag::bad_datetime(f, &a);
        }
        r
    };
    match f {
        Builtin::Str => s(lex(&a0()?)),
        // RDF-star accessors return the component TERM (an IRI/literal/quoted
        // triple), not a lexical string; TRIPLE builds a quoted triple from three
        // term values. `None` (a type error) when the argument is not a quoted
        // triple, matching the SPARQL-star spec.
        Builtin::Subject => {
            crate::ingest::quoted_triple_parts(&a0()?).map(|(x, _, _)| Rc::from(x.as_str()))
        }
        Builtin::Predicate => {
            crate::ingest::quoted_triple_parts(&a0()?).map(|(_, x, _)| Rc::from(x.as_str()))
        }
        Builtin::Object => {
            crate::ingest::quoted_triple_parts(&a0()?).map(|(_, _, x)| Rc::from(x.as_str()))
        }
        Builtin::TripleTerm => {
            let sub = a0()?;
            let pred = args.get(1).and_then(|e| e.value(ctx, b))?;
            let obj = args.get(2).and_then(|e| e.value(ctx, b))?;
            Some(Rc::from(format!("<<{sub} {pred} {obj}>>")))
        }
        // STRLEN/UCASE/LCASE/SUBSTR/ENCODE_FOR_URI and the hashes require a STRING
        // literal argument — `string_arg` errors (→ None, a type error) on an IRI,
        // blank node, or non-string typed literal, matching SPARQL (and CONCAT,
        // which already type-checks). Without this, STRLEN(42) wrongly returned 2
        // (Oxigraph differential). `string_arg`'s tag also carries the language onto
        // the UCASE/LCASE/SUBSTR result.
        Builtin::StrLen => {
            let a = a0()?;
            string_arg_of(f, 0, &a)?;
            num(lex(&a).chars().count() as f64)
        }
        Builtin::UCase => {
            let a = a0()?;
            let lang = string_arg_of(f, 0, &a)?;
            sl(lex(&a).to_uppercase(), lang.as_deref())
        }
        Builtin::LCase => {
            let a = a0()?;
            let lang = string_arg_of(f, 0, &a)?;
            sl(lex(&a).to_lowercase(), lang.as_deref())
        }
        Builtin::Abs => num(as_number(&a0()?)?.abs()),
        Builtin::Ceil => num(as_number(&a0()?)?.ceil()),
        Builtin::Floor => num(as_number(&a0()?)?.floor()),
        // SPARQL ROUND rounds a tie toward +∞ (`floor(x + 0.5)`), unlike Rust's
        // `f64::round` which rounds a tie away from zero — so ROUND(-3.5) is -3,
        // not -4 (caught by the Oxigraph differential).
        Builtin::Round => num((as_number(&a0()?)? + 0.5).floor()),
        Builtin::Concat => {
            // Every argument must be a string literal (else a type error); the
            // result keeps the common language tag iff all share that same
            // non-empty tag, and is a simple literal otherwise.
            let mut out = String::new();
            let mut lang: Option<Option<String>> = None;
            for (i, a) in args.iter().enumerate() {
                let v = a.value(ctx, b)?;
                let this = string_arg_of(f, i, &v)?;
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
            let lang = string_arg_of(f, 0, &a)?;
            let chars: Vec<char> = lex(&a).chars().collect();
            let start = as_number(&args.get(1)?.value(ctx, b)?)?.max(1.0) as usize - 1;
            let it = chars.iter().skip(start);
            let out: String = match args.get(2) {
                Some(lenarg) => it
                    .take(as_number(&lenarg.value(ctx, b)?)?.max(0.0) as usize)
                    .collect(),
                None => it.collect(),
            };
            sl(out, lang.as_deref())
        }
        // STRBEFORE/STRAFTER: arg1 and arg2 must both be string literals and be
        // argument-compatible (arg2 simple, or sharing arg1's tag) — else a type
        // error. A found needle (or empty needle) yields a result carrying
        // arg1's language tag; a *not-found* needle yields a simple literal "".
        Builtin::StrBefore => {
            let a = a0()?;
            let c = args.get(1)?.value(ctx, b)?;
            let Some(lang) = before_after_lang(&a, &c) else {
                binary_type_error(f, args, Some(&a), Some(&c));
                return None;
            };
            let (t, needle) = (lex(&a), lex(&args.get(1)?.value(ctx, b)?));
            match (needle.is_empty(), t.find(&needle)) {
                (true, _) => sl(String::new(), lang.as_deref()),
                (false, Some(i)) => sl(t[..i].to_string(), lang.as_deref()),
                (false, None) => s(String::new()),
            }
        }
        Builtin::StrAfter => {
            let a = a0()?;
            let c = args.get(1)?.value(ctx, b)?;
            let Some(lang) = before_after_lang(&a, &c) else {
                binary_type_error(f, args, Some(&a), Some(&c));
                return None;
            };
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
        Builtin::EncodeForUri => {
            let a = a0()?;
            string_arg_of(f, 0, &a)?;
            s(encode_for_uri(&lex(&a)))
        }
        Builtin::Replace => {
            let a = a0()?;
            let lang = string_arg_of(f, 0, &a)?; // arg1 must be a string literal
            let text = lex(&a);
            let pat = lex(&args.get(1)?.value(ctx, b)?);
            let rep = lex(&args.get(2)?.value(ctx, b)?);
            let flags = match args.get(3) {
                Some(e) => lex(&e.value(ctx, b)?),
                None => String::new(),
            };
            let Some(out) = ctx.resolver.regex_replace(&pat, &flags, &text, &rep) else {
                diag::invalid_regex(f, &pat, &flags);
                return None;
            };
            sl(out, lang.as_deref())
        }
        Builtin::Md5 | Builtin::Sha1 | Builtin::Sha256 | Builtin::Sha384 | Builtin::Sha512 => {
            let a = a0()?;
            string_arg_of(f, 0, &a)?;
            s(hash_hex(f, &lex(&a)))
        }
        // Date/time accessors over an xsd:dateTime lexical form.
        Builtin::Year => num(dtm()?.0 as f64),
        Builtin::Month => num(dtm()?.1 as f64),
        Builtin::Day => num(dtm()?.2 as f64),
        Builtin::Hours => num(dtm()?.3 as f64),
        Builtin::Minutes => num(dtm()?.4 as f64),
        Builtin::Seconds => num(dtm()?.5),
        // TZ → the timezone as a simple literal ("Z", "-08:00", or "").
        Builtin::Tz => s(dtm()?.6),
        // TIMEZONE → an xsd:dayTimeDuration; a value with no timezone errors.
        Builtin::Timezone => {
            let dur = tz_to_duration(&dtm()?.6)?;
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
        // GeoSPARQL relations in value position (e.g. `BIND(geof:sfWithin(..) AS ?x)`)
        // → a typed xsd:boolean; a type error → None (unbound).
        Builtin::GeoSfContains
        | Builtin::GeoSfWithin
        | Builtin::GeoSfIntersects
        | Builtin::GeoSfDisjoint
        | Builtin::GeoSfEquals => {
            let r = geo_relation(f, &a0()?, &args.get(1)?.value(ctx, b)?)?;
            Some(Rc::from(bool_literal(r)))
        }
        // geof:distance(g1, g2, unitIRI) → xsd:double.
        Builtin::GeoDistance => {
            let unit = geo_unit(&args.get(2)?.value(ctx, b)?)?;
            let d = crate::geo::distance(
                &wkt_arg(&a0()?)?,
                &wkt_arg(&args.get(1)?.value(ctx, b)?)?,
                unit,
            )?;
            Some(Rc::from(make_literal(
                &fmt_plain(d),
                None,
                Some(XSD_DOUBLE),
            )))
        }
        // geof:envelope(g) → the bounding box as a geo:wktLiteral POLYGON.
        Builtin::GeoEnvelope => {
            let wkt = crate::geo::envelope_wkt(&wkt_arg(&a0()?)?)?;
            Some(Rc::from(make_literal(
                &wkt,
                None,
                Some(crate::geo::GEO_WKT),
            )))
        }
        // geo3 3D relations in value position → xsd:boolean.
        Builtin::Geo3Contains | Builtin::Geo3Within | Builtin::Geo3Adjacent => {
            let gap = args.get(2).and_then(|e| e.value(ctx, b));
            let r = geo3_relation(f, &a0()?, &args.get(1)?.value(ctx, b)?, gap)?;
            Some(Rc::from(bool_literal(r)))
        }
        // geo3:distance3D(g1, g2) → xsd:double (min AABB gap, literal's own unit).
        Builtin::Geo3Distance => {
            let d =
                crate::geo3::distance(&geo3_arg(&a0()?)?, &geo3_arg(&args.get(1)?.value(ctx, b)?)?);
            Some(Rc::from(make_literal(
                &fmt_plain(d),
                None,
                Some(XSD_DOUBLE),
            )))
        }
        _ => None,
    }
}

const XSD_DOUBLE: &str = "http://www.w3.org/2001/XMLSchema#double";

/// A `geo:wktLiteral` (leniently: also a plain/`xsd:string`) literal term →
/// [`crate::geo::Geometry`]. `None` for a non-literal, an other-typed literal
/// (e.g. `geo:gmlLiteral`), or malformed WKT — all type errors.
fn wkt_arg(token: &str) -> Option<crate::geo::Geometry> {
    if !token.starts_with('"') {
        return None;
    }
    match datatype_iri(token).as_deref() {
        Some(crate::geo::GEO_WKT) | Some(XSD_STRING) | None => {}
        Some(_) => return None,
    }
    crate::geo::parse_wkt(&crate::terms::lexical(token))
}

/// Evaluate a GeoSPARQL relation builtin on two term tokens.
fn geo_relation(f: Builtin, t1: &str, t2: &str) -> Option<bool> {
    let rel = match f {
        Builtin::GeoSfContains => crate::geo::Rel::Contains,
        Builtin::GeoSfWithin => crate::geo::Rel::Within,
        Builtin::GeoSfIntersects => crate::geo::Rel::Intersects,
        Builtin::GeoSfDisjoint => crate::geo::Rel::Disjoint,
        Builtin::GeoSfEquals => crate::geo::Rel::Equals,
        _ => return None,
    };
    crate::geo::relate(rel, &wkt_arg(t1)?, &wkt_arg(t2)?)
}

/// A 3D geometry literal (`geo3:wktLiteral3D` / `geo3:box3dLiteral`, leniently also
/// `geo:wktLiteral` / plain string) → its AABB. `None` for any other term (type error).
fn geo3_arg(token: &str) -> Option<crate::geo3::Aabb> {
    if !token.starts_with('"') {
        return None;
    }
    match datatype_iri(token).as_deref() {
        Some(crate::geo3::WKT3)
        | Some(crate::geo3::BOX3D)
        | Some(crate::geo::GEO_WKT)
        | Some(XSD_STRING)
        | None => {}
        Some(_) => return None,
    }
    crate::geo3::parse(&crate::terms::lexical(token))
}

/// Evaluate a geo3 relation builtin on two geometry terms (+ optional gap for adjacency).
fn geo3_relation(f: Builtin, t1: &str, t2: &str, gap: Option<Rc<str>>) -> Option<bool> {
    let rel = match f {
        Builtin::Geo3Contains => crate::geo3::Rel3::Contains,
        Builtin::Geo3Within => crate::geo3::Rel3::Within,
        Builtin::Geo3Adjacent => crate::geo3::Rel3::Adjacent,
        _ => return None,
    };
    let g = gap.and_then(|t| as_number(&t)).unwrap_or(0.0);
    Some(crate::geo3::relate(rel, &geo3_arg(t1)?, &geo3_arg(t2)?, g))
}

/// A units IRI term (`<…/metre>` / `<…/degree>`) → [`crate::geo::Unit`].
fn geo_unit(token: &str) -> Option<crate::geo::Unit> {
    match iri_content(token)? {
        crate::geo::UOM_METRE => Some(crate::geo::Unit::Metre),
        crate::geo::UOM_DEGREE => Some(crate::geo::Unit::Degree),
        _ => None,
    }
}

/// The datatype IRI of a literal term (see [`Builtin::Datatype`]). Returns the
/// IRI content without angle brackets; the call site adds them.
use crate::terms::{
    iri_content, is_iri, lang_tag as lang_of, literal_datatype as datatype_iri, make_literal,
};

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

/// SPARQL 1.1 §17.4.3 "argument compatibility" for the binary string functions
/// CONTAINS, STRSTARTS, STRENDS, STRBEFORE and STRAFTER. `text` is arg1 (the
/// haystack), `pat` is arg2 (the needle). The pair is compatible — and this
/// returns `true` — iff **both** are string literals AND either `pat` is a simple
/// literal or `xsd:string` (no language tag), or `text` and `pat` carry the same
/// language tag. Equivalently, an incompatible pair is one where `pat` has a
/// language tag that differs from `text`'s: a plain / `xsd:string` `pat` is always
/// compatible, and only the *pattern*'s tag (against the text's) matters — a
/// tagged `text` with a simple `pat` stays compatible, but a simple `text` with a
/// tagged `pat` does not. A `false` result is a type error at every call site (row
/// eliminated in a FILTER; unbound in value position).
fn args_compatible(text: &str, pat: &str) -> bool {
    let (Some(l_text), Some(l_pat)) = (string_arg(text), string_arg(pat)) else {
        return false; // a non-string argument is itself a type error
    };
    l_pat.is_none() || l_pat == l_text
}

/// STRBEFORE/STRAFTER: both arguments must be string literals and be
/// argument-compatible ([`args_compatible`]). Returns the tag to put on the
/// result (`arg1`'s), or `None` for a type error.
fn before_after_lang(arg1: &str, arg2: &str) -> Option<Option<String>> {
    if !args_compatible(arg1, arg2) {
        return None;
    }
    // args_compatible guarantees arg1 is a string literal, so this is `Some`.
    string_arg(arg1)
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

/// The effective boolean value of a term (SPARQL 1.1 §17.2.2), or `None` (a
/// type error) for a term that has no EBV: an IRI, a blank node, a
/// language-tagged literal, or a literal of any other datatype. A boolean or
/// numeric literal whose lexical form is invalid for its datatype (e.g.
/// `"abc"^^xsd:integer`) has EBV `false`, not an error.
fn term_ebv(token: &str) -> Option<bool> {
    if !token.starts_with('"') {
        return None;
    }
    match datatype_iri(token).as_deref() {
        Some("http://www.w3.org/2001/XMLSchema#boolean") => Some(matches!(
            crate::terms::lexical(token).as_ref(),
            "true" | "1"
        )),
        Some(dt) if is_numeric_ebv_dt(dt) => {
            Some(as_number(token).is_some_and(|n| n != 0.0 && !n.is_nan()))
        }
        Some(XSD_STRING) => Some(!crate::terms::lexical(token).is_empty()),
        _ => None,
    }
}

/// `isNumeric` (§17.4.2.4): a literal whose datatype is numeric (including the
/// types derived from `xsd:decimal`) and whose lexical form is valid for it.
fn is_numeric_term(token: &str) -> bool {
    if !token.starts_with('"') {
        return false;
    }
    let Some(dt) = datatype_iri(token) else {
        return false;
    };
    let Some(local) = dt.strip_prefix("http://www.w3.org/2001/XMLSchema#") else {
        return false;
    };
    if !is_numeric_ebv_dt(&dt) {
        return false;
    }
    let lex = crate::terms::lexical(token);
    match local {
        "decimal" => is_decimal_lexical(&lex),
        "float" | "double" => parse_xsd_double(&lex).is_some(),
        // xsd:integer and every type derived from it.
        _ => is_int_lexical(&lex),
    }
}

/// XSD numeric datatypes, including those derived from `xsd:decimal`, for the
/// EBV rule ("a numeric type or a typed literal with a datatype derived from a
/// numeric type", §17.2.2).
fn is_numeric_ebv_dt(dt: &str) -> bool {
    dt.strip_prefix("http://www.w3.org/2001/XMLSchema#")
        .is_some_and(|local| {
            matches!(
                local,
                "integer"
                    | "decimal"
                    | "float"
                    | "double"
                    | "int"
                    | "long"
                    | "short"
                    | "byte"
                    | "nonNegativeInteger"
                    | "nonPositiveInteger"
                    | "negativeInteger"
                    | "positiveInteger"
                    | "unsignedInt"
                    | "unsignedLong"
                    | "unsignedShort"
                    | "unsignedByte"
            )
        })
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

/// Evaluate a boolean built-in (type checks / string predicates) to `true`,
/// `false`, or `None` — a SPARQL **error** (an unbound or ill-typed argument,
/// an invalid regex). The error is the caller's to resolve (§17.2): a FILTER
/// drops the row, `!` keeps it an error, BIND leaves the variable unbound.
fn func_bool(f: Builtin, args: &[FExpr], ctx: &Ctx, b: &Row) -> Option<bool> {
    let val = |i: usize| args.get(i).and_then(|e| e.value(ctx, b));
    // CONTAINS/STRSTARTS/STRENDS take two string-literal args that must also be
    // argument-compatible (SPARQL 1.1 §17.4.3). A non-string OR an incompatible
    // pair is a type error (like CONCAT, which type-checks). The error is also
    // reported to the diagnostics sink (cold path only), exactly once, here at
    // its origin — an enclosing `!` / `&&` / `||` propagates it without
    // reporting it again. A plain no-match on valid strings feeds the
    // case-sensitivity hint once.
    let two = |g: fn(&str, &str) -> bool, bit: u8| {
        let (a, c) = (val(0), val(1));
        match (&a, &c) {
            (Some(a), Some(c)) if args_compatible(a, c) => {
                let r = g(&lexical(a), &lexical(c));
                if !r {
                    no_match(ctx, f, bit, args);
                }
                Some(r)
            }
            _ => {
                binary_type_error(f, args, a.as_deref(), c.as_deref());
                None
            }
        }
    };
    match f {
        // The type tests never fail on a bound term; an unbound or erroring
        // argument is an error like any other function's (§17.2).
        Builtin::IsIri => val(0).map(|t| t.starts_with('<')),
        Builtin::IsBlank => val(0).map(|t| t.starts_with("_:")),
        Builtin::IsLiteral => val(0).map(|t| t.starts_with('"')),
        // isNumeric: a literal of a numeric datatype WITH a valid lexical form
        // (§17.4.2.4), not any literal whose text parses as a number, which
        // made isNumeric("10") true for an xsd:string.
        Builtin::IsNumeric => val(0).map(|t| is_numeric_term(&t)),
        // RDF-star: is the argument a quoted triple (`<<s p o>>`)?
        Builtin::IsTriple => val(0).map(|t| crate::terms::is_quoted_triple(&t)),
        Builtin::Contains => two(|a, c| a.contains(c), diag::MISS_CONTAINS),
        Builtin::StrStarts => two(|a, c| a.starts_with(c), diag::MISS_STRSTARTS),
        Builtin::StrEnds => two(|a, c| a.ends_with(c), diag::MISS_STRENDS),
        // REGEX(text, pattern [, flags]) — SPARQL flags i/m/s/x map to inline
        // regex flags. An invalid pattern is an error (and a diagnostic).
        // The matcher is compiled once per query (memoized); literal patterns
        // skip the regex engine entirely.
        Builtin::Regex => {
            let (text, pat) = (val(0), val(1));
            match (&text, &pat) {
                // The text argument must be a string literal — a non-string is a
                // type error, matching SPARQL and CONTAINS above.
                (Some(text), Some(pat)) if string_arg(text).is_some() => {
                    let flags = val(2).map(|t| lexical(&t)).unwrap_or_default();
                    let pat = lexical(pat);
                    match ctx.resolver.regex_try(&pat, &flags, &lexical(text)) {
                        Some(true) => Some(true),
                        Some(false) => {
                            if !flags.contains('i') {
                                no_match(ctx, f, diag::MISS_REGEX, args);
                            }
                            Some(false)
                        }
                        None => {
                            diag::invalid_regex(f, &pat, &flags);
                            None
                        }
                    }
                }
                _ => {
                    regex_type_error(f, args, text.as_deref(), pat.as_deref());
                    None
                }
            }
        }
        // LANGMATCHES(tag, range): basic-filtering language-range match
        // (case-insensitive; "*" matches any non-empty tag).
        Builtin::LangMatches => {
            let (tag, range) = (val(0)?, val(1)?);
            Some(lang_matches(&lexical(&tag), &lexical(&range)))
        }
        // GeoSPARQL topological relations: a malformed / non-geometry argument
        // is a type error.
        Builtin::GeoSfContains
        | Builtin::GeoSfWithin
        | Builtin::GeoSfIntersects
        | Builtin::GeoSfDisjoint
        | Builtin::GeoSfEquals => geo_relation(f, &val(0)?, &val(1)?),
        // geo3 3D relations (a malformed argument is a type error).
        Builtin::Geo3Contains | Builtin::Geo3Within | Builtin::Geo3Adjacent => {
            geo3_relation(f, &val(0)?, &val(1)?, val(2))
        }
        // Not a boolean builtin: callers route those through `is_boolean_builtin`
        // and take the EBV of the value instead.
        _ => None,
    }
}

/// A string predicate returned `false` on valid string arguments: report it to
/// the diagnostics sink once per function per query (a bit test per row after
/// that), for the case-sensitivity hint.
#[inline]
fn no_match(ctx: &Ctx, f: Builtin, bit: u8, args: &[FExpr]) {
    let seen = ctx.miss_mask.get();
    if seen & bit == 0 {
        ctx.miss_mask.set(seen | bit);
        if let Some(needle) = args.get(1) {
            diag::note_no_match(f, needle);
        }
    }
}

/// An argument evaluated to nothing: report it when it is a bare variable
/// (unbound in this row). A nested expression that failed is not reported
/// here — the nested function reports its own error.
fn missing_arg(f: Builtin, i: usize, args: &[FExpr]) {
    if let Some(FExpr::Var(v)) = args.get(i) {
        diag::unbound(f, i + 1, v);
    }
}

/// Why CONTAINS / STRSTARTS / STRENDS raised a type error.
#[cold]
#[inline(never)]
fn binary_type_error(f: Builtin, args: &[FExpr], a: Option<&str>, c: Option<&str>) {
    let mut both_strings = true;
    for (i, v) in [a, c].into_iter().enumerate() {
        match v {
            None => {
                both_strings = false;
                missing_arg(f, i, args);
            }
            Some(t) if string_arg(t).is_none() => {
                both_strings = false;
                diag::bad_arg(f, i + 1, t);
            }
            Some(_) => {}
        }
    }
    if let (true, Some(a), Some(c)) = (both_strings, a, c) {
        diag::lang_mismatch(f, a, c);
    }
}

/// Why REGEX raised a type error (non-string text, or a missing argument).
#[cold]
#[inline(never)]
fn regex_type_error(f: Builtin, args: &[FExpr], text: Option<&str>, pat: Option<&str>) {
    match text {
        None => missing_arg(f, 0, args),
        Some(t) if string_arg(t).is_none() => diag::bad_arg(f, 1, t),
        Some(_) => {}
    }
    if pat.is_none() {
        missing_arg(f, 1, args);
    }
}

/// [`string_arg`] for argument `pos` (0-based) of the value function `f`,
/// reporting a non-string to the diagnostics sink.
#[inline]
fn string_arg_of(f: Builtin, pos: usize, token: &str) -> Option<Option<String>> {
    let r = string_arg(token);
    if r.is_none() {
        diag::bad_arg(f, pos + 1, token);
    }
    r
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{write_file, Rete};
    use crate::index::GraphIndexBuilder;
    use crate::row::{Ctx, Slots, Val};

    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    const GEO: &str = "http://www.opengis.net/ont/geosparql#";

    fn lit(value: &str, datatype: &str) -> String {
        format!("\"{value}\"^^<{datatype}>")
    }

    fn fixture() -> Rete {
        let triples = [("<s>", "<p>", "\"value\"")];
        let mut builder = DictionaryBuilder::new();
        for (s, p, o) in triples {
            builder.observe(s, p, o);
        }
        let dict = builder.build();
        let mut index = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            index.push(dict.encode(s, p, o).unwrap());
        }
        Rete::open(&write_file(&dict, &index.build(), false, &[], 0)).unwrap()
    }

    fn call(ctx: &Ctx, builtin: Builtin, args: &[&str]) -> Option<String> {
        let args = args
            .iter()
            .map(|v| FExpr::Const((*v).to_string()))
            .collect::<Vec<_>>();
        func_value(builtin, &args, ctx, &ctx.slots.empty_row()).map(|v| v.to_string())
    }

    #[test]
    fn expression_values_boolean_errors_and_sort_keys() {
        let rete = fixture();
        let mut slots = Slots::new();
        let x = slots.add("x");
        slots.add("unset");
        let ctx = Ctx::new(&rete, slots);
        let mut row = ctx.slots.empty_row();
        row[x] = Some(Val::Str(Rc::from(lit("2", &format!("{XSD}integer")))));

        let var = FExpr::Var("x".into());
        assert!(var.value(&ctx, &row).unwrap().contains("2"));
        assert_eq!(FExpr::Var("missing".into()).value(&ctx, &row), None);
        for (op, expected) in [
            (ArithOp::Add, 5.0),
            (ArithOp::Sub, -1.0),
            (ArithOp::Mul, 6.0),
            (ArithOp::Div, 2.0 / 3.0),
        ] {
            let expr = FExpr::Arith(
                op,
                Box::new(var.clone()),
                Box::new(FExpr::Const(lit("3", &format!("{XSD}integer")))),
            );
            let actual = as_number(&expr.value(&ctx, &row).unwrap()).unwrap();
            assert!((actual - expected).abs() < 1e-12);
        }
        assert_eq!(
            FExpr::Arith(
                ArithOp::Div,
                Box::new(var.clone()),
                Box::new(FExpr::Const("0".into()))
            )
            .value(&ctx, &row),
            None
        );
        assert_eq!(
            FExpr::Coalesce(vec![
                FExpr::Var("missing".into()),
                FExpr::Const("ok".into())
            ])
            .value(&ctx, &row)
            .as_deref(),
            Some("ok")
        );

        let yes = FExpr::Const(lit("true", &format!("{XSD}boolean")));
        let no = FExpr::Const(lit("false", &format!("{XSD}boolean")));
        assert_eq!(
            FExpr::If(
                Box::new(yes.clone()),
                Box::new(FExpr::Const("then".into())),
                Box::new(FExpr::Const("else".into()))
            )
            .value(&ctx, &row)
            .as_deref(),
            Some("then")
        );
        assert_eq!(
            FExpr::If(
                Box::new(FExpr::Const("<iri-has-no-ebv>".into())),
                Box::new(FExpr::Const("then".into())),
                Box::new(FExpr::Const("else".into()))
            )
            .value(&ctx, &row),
            None
        );
        assert!(FExpr::Bound("x".into()).ebv3(&ctx, &row).unwrap());
        assert!(!FExpr::Bound("unset".into()).ebv3(&ctx, &row).unwrap());
        assert!(FExpr::Not(Box::new(no.clone())).ebv3(&ctx, &row).unwrap());
        assert!(FExpr::And(Box::new(yes.clone()), Box::new(yes.clone()))
            .ebv3(&ctx, &row)
            .unwrap());
        assert!(FExpr::Or(Box::new(no.clone()), Box::new(yes.clone()))
            .ebv3(&ctx, &row)
            .unwrap());
        assert!(FExpr::Compare(
            Op::Lt,
            Box::new(FExpr::Const("2".into())),
            Box::new(FExpr::Const("3".into()))
        )
        .ebv3(&ctx, &row)
        .unwrap());
        assert!(FExpr::In(
            Box::new(FExpr::Const("2".into())),
            vec![FExpr::Const("1".into()), FExpr::Const("2".into())]
        )
        .ebv3(&ctx, &row)
        .unwrap());
        assert!(FExpr::SameTerm(
            Box::new(FExpr::Const("\"x\"".into())),
            Box::new(FExpr::Const("\"x\"".into()))
        )
        .ebv3(&ctx, &row)
        .unwrap());
        assert!(FExpr::If(
            Box::new(no),
            Box::new(FExpr::Const("0".into())),
            Box::new(yes)
        )
        .ebv3(&ctx, &row)
        .unwrap());

        let unbound = SortKey::of(None);
        let one = SortKey::of(Some(Rc::from("1")));
        let two = SortKey::of(Some(Rc::from("2")));
        let text = SortKey::of(Some(Rc::from("z")));
        assert_eq!(unbound.cmp(&unbound), std::cmp::Ordering::Equal);
        assert_eq!(unbound.cmp(&one), std::cmp::Ordering::Less);
        assert_eq!(one.cmp(&unbound), std::cmp::Ordering::Greater);
        assert_eq!(one.cmp(&two), std::cmp::Ordering::Less);
        assert_eq!(text.cmp(&two), std::cmp::Ordering::Greater);
    }

    /// SPARQL 1.1 §17.2 / §17.3 / §17.4.1: an error is a value of its own that
    /// survives `!`, follows the three-valued `||` / `&&` tables, makes IF's
    /// condition an error, is skipped by COALESCE, and is resolved only by the
    /// FILTER verdict (`boolean`) — not collapsed to `false` at the function.
    #[test]
    fn errors_propagate_until_the_spec_resolves_them() {
        let rete = fixture();
        let mut slots = Slots::new();
        slots.add("unset");
        let ctx = Ctx::new(&rete, slots);
        let row = ctx.slots.empty_row();
        let c = |s: &str| Box::new(FExpr::Const(s.to_string()));
        let t = || c(&lit("true", &format!("{XSD}boolean")));
        let f = || c(&lit("false", &format!("{XSD}boolean")));
        // CONTAINS on an IRI: a type error.
        let e = || {
            Box::new(FExpr::Func(
                Builtin::Contains,
                vec![FExpr::Const("<iri>".into()), FExpr::Const("\"i\"".into())],
            ))
        };
        let ebv = |x: FExpr| x.ebv3(&ctx, &row);

        assert_eq!(ebv(*e()), None);
        // fn:not of an error is an error — `!CONTAINS(?iri, …)` is NOT true.
        assert_eq!(ebv(FExpr::Not(e())), None);
        assert_eq!(ebv(FExpr::Not(Box::new(FExpr::Not(e())))), None);

        // The §17.2 truth table, every row that involves an error.
        assert_eq!(ebv(FExpr::Or(t(), e())), Some(true));
        assert_eq!(ebv(FExpr::Or(e(), t())), Some(true));
        assert_eq!(ebv(FExpr::Or(f(), e())), None);
        assert_eq!(ebv(FExpr::Or(e(), f())), None);
        assert_eq!(ebv(FExpr::Or(e(), e())), None);
        assert_eq!(ebv(FExpr::And(t(), e())), None);
        assert_eq!(ebv(FExpr::And(e(), t())), None);
        assert_eq!(ebv(FExpr::And(f(), e())), Some(false));
        assert_eq!(ebv(FExpr::And(e(), f())), Some(false));
        assert_eq!(ebv(FExpr::And(e(), e())), None);
        assert_eq!(ebv(FExpr::Not(Box::new(FExpr::And(e(), f())))), Some(true));

        // IF: an error in the condition is an error; only one branch is taken.
        let pick = |cond: Box<FExpr>| FExpr::If(cond, c("\"y\""), c("\"n\"")).value(&ctx, &row);
        assert_eq!(pick(e()), None);
        assert_eq!(pick(Box::new(FExpr::Not(e()))), None);
        assert_eq!(pick(t()).as_deref(), Some("\"y\""));
        // …and a boolean builtin is a valid condition (it used to error).
        let contains = Box::new(FExpr::Func(
            Builtin::Contains,
            vec![FExpr::Const("\"abc\"".into()), FExpr::Const("\"b\"".into())],
        ));
        assert_eq!(pick(contains).as_deref(), Some("\"y\""));

        // COALESCE skips an error; BOUND never errors.
        let div0 = FExpr::Arith(ArithOp::Div, c("1"), c("0"));
        assert_eq!(
            FExpr::Coalesce(vec![div0.clone(), FExpr::Const("\"x\"".into())])
                .value(&ctx, &row)
                .as_deref(),
            Some("\"x\"")
        );
        assert_eq!(FExpr::Coalesce(vec![div0.clone()]).value(&ctx, &row), None);
        assert_eq!(
            ebv(FExpr::Not(Box::new(FExpr::Bound("unset".into())))),
            Some(true)
        );

        // A type test of an unbound variable is an error like any function's.
        let unset = || FExpr::Var("unset".into());
        assert_eq!(
            ebv(FExpr::Not(Box::new(FExpr::Func(
                Builtin::IsIri,
                vec![unset()]
            )))),
            None
        );
        assert_eq!(
            ebv(FExpr::Not(Box::new(FExpr::SameTerm(
                Box::new(unset()),
                c("\"a\"")
            )))),
            None
        );

        // IN / NOT IN (§17.4.1.9-10): 2 IN (1/0, 2) is true, 2 IN (3, 1/0) an
        // error, 2 IN () false; NOT IN is !(IN).
        let two = || c("2");
        let isin = |list: Vec<FExpr>| FExpr::In(two(), list);
        assert_eq!(ebv(isin(vec![div0.clone(), *two()])), Some(true));
        assert_eq!(ebv(isin(vec![*two(), div0.clone()])), Some(true));
        assert_eq!(ebv(isin(vec![*c("3"), div0.clone()])), None);
        assert_eq!(ebv(isin(vec![])), Some(false));
        assert_eq!(
            ebv(FExpr::Not(Box::new(isin(vec![*c("3"), div0.clone()])))),
            None
        );
        assert_eq!(
            ebv(FExpr::Not(Box::new(isin(vec![div0.clone(), *two()])))),
            Some(false)
        );
        assert_eq!(ebv(FExpr::In(Box::new(unset()), vec![])), None);

        // In value position an error is an error (unbound), not "false".
        assert_eq!(FExpr::Not(e()).value(&ctx, &row), None);
        assert_eq!(
            FExpr::Or(e(), t()).value(&ctx, &row).as_deref(),
            Some(&*bool_literal(true))
        );

        // EBV (§17.2.2): an ill-typed boolean / numeric literal is false, a
        // derived numeric type counts, a language-tagged literal has no EBV.
        assert_eq!(term_ebv(&lit("abc", &format!("{XSD}integer"))), Some(false));
        assert_eq!(
            term_ebv(&lit("maybe", &format!("{XSD}boolean"))),
            Some(false)
        );
        assert_eq!(term_ebv(&lit("5", &format!("{XSD}int"))), Some(true));
        assert_eq!(term_ebv("\"x\"@en"), None);
        // A value function used as a condition takes the EBV of its value.
        assert_eq!(
            ebv(FExpr::Func(
                Builtin::StrLen,
                vec![FExpr::Const("\"\"".into())]
            )),
            Some(false)
        );
    }

    /// Boolean built-ins have a value outside FILTER (§17.4.2 / §17.4.3 return
    /// xsd:boolean), and isNumeric needs a numeric datatype with a valid
    /// lexical form (§17.4.2.4).
    #[test]
    fn boolean_builtins_have_values_and_is_numeric_is_strict() {
        let rete = fixture();
        let mut slots = Slots::new();
        slots.add("unset");
        let ctx = Ctx::new(&rete, slots);
        let row = ctx.slots.empty_row();
        let f = |b: Builtin, args: &[&str]| {
            FExpr::Func(
                b,
                args.iter().map(|a| FExpr::Const(a.to_string())).collect(),
            )
        };
        let t = bool_literal(true);
        let fl = bool_literal(false);

        // In value position: a typed boolean, or unbound on an error.
        assert_eq!(
            f(Builtin::IsIri, &["<x>"]).value(&ctx, &row).as_deref(),
            Some(&*t)
        );
        assert_eq!(
            f(Builtin::IsLiteral, &["<x>"]).value(&ctx, &row).as_deref(),
            Some(&*fl)
        );
        assert_eq!(
            f(Builtin::Contains, &["\"abc\"", "\"b\""])
                .value(&ctx, &row)
                .as_deref(),
            Some(&*t)
        );
        assert_eq!(
            f(Builtin::Contains, &["<x>", "\"b\""]).value(&ctx, &row),
            None
        );
        assert_eq!(
            FExpr::Func(Builtin::IsBlank, vec![FExpr::Var("unset".into())]).value(&ctx, &row),
            None
        );
        // So COALESCE sees a value, and skips only the error.
        assert_eq!(
            FExpr::Coalesce(vec![
                f(Builtin::StrStarts, &["\"abc\"", "\"z\""]),
                FExpr::Const(t.clone())
            ])
            .value(&ctx, &row)
            .as_deref(),
            Some(&*fl)
        );
        assert_eq!(
            FExpr::Coalesce(vec![
                f(Builtin::Regex, &["<x>", "\"z\""]),
                FExpr::Const(t.clone())
            ])
            .value(&ctx, &row)
            .as_deref(),
            Some(&*t)
        );

        // isNumeric: datatype AND lexical form.
        let num = |lex: &str, dt: &str| {
            func_bool(
                Builtin::IsNumeric,
                &[FExpr::Const(lit(lex, &format!("{XSD}{dt}")))],
                &ctx,
                &row,
            )
        };
        assert_eq!(num("12", "integer"), Some(true));
        assert_eq!(num("-1.5", "decimal"), Some(true));
        assert_eq!(num("1e3", "double"), Some(true));
        assert_eq!(num("INF", "float"), Some(true));
        assert_eq!(num("7", "int"), Some(true));
        assert_eq!(num("10", "string"), Some(false));
        assert_eq!(num("abc", "integer"), Some(false));
        assert_eq!(num("1.5", "integer"), Some(false));
        assert_eq!(
            func_bool(
                Builtin::IsNumeric,
                &[FExpr::Const("\"10\"".into())],
                &ctx,
                &row
            ),
            Some(false)
        );
        assert_eq!(
            func_bool(
                Builtin::IsNumeric,
                &[FExpr::Const("<x>".into())],
                &ctx,
                &row
            ),
            Some(false)
        );
    }

    #[test]
    fn value_builtins_cover_strings_dates_hashes_casts_and_rdf_star() {
        let rete = fixture();
        let ctx = Ctx::new(&rete, Slots::new());
        let string = lit("Straße", &format!("{XSD}string"));
        let lang = "\"Hello World\"@en";
        let int = lit("-3", &format!("{XSD}integer"));
        let decimal = lit("2.4", &format!("{XSD}decimal"));

        for (builtin, args) in [
            (Builtin::Str, vec!["<http://ex/a>"]),
            (Builtin::StrLen, vec![string.as_str()]),
            (Builtin::UCase, vec![lang]),
            (Builtin::LCase, vec![lang]),
            (Builtin::Abs, vec![int.as_str()]),
            (Builtin::Ceil, vec![decimal.as_str()]),
            (Builtin::Floor, vec![decimal.as_str()]),
            (Builtin::Round, vec![decimal.as_str()]),
            (Builtin::Concat, vec!["\"a\"@en", "\"b\"@en"]),
            (Builtin::SubStr, vec![lang, "2", "3"]),
            (Builtin::StrBefore, vec![lang, "\" World\""]),
            (Builtin::StrAfter, vec![lang, "\"Hello \""]),
            (Builtin::StrDt, vec!["\"7\"", "<http://ex/type>"]),
            (Builtin::StrLang, vec!["\"hello\"", "\"de\""]),
            (Builtin::Iri, vec!["\"http://ex/a\""]),
            (Builtin::EncodeForUri, vec!["\"a b/ä\""]),
            (
                Builtin::Replace,
                vec!["\"Abba\"", "\"b+\"", "\"X\"", "\"i\""],
            ),
            (Builtin::Md5, vec!["\"abc\""]),
            (Builtin::Sha1, vec!["\"abc\""]),
            (Builtin::Sha256, vec!["\"abc\""]),
            (Builtin::Sha384, vec!["\"abc\""]),
            (Builtin::Sha512, vec!["\"abc\""]),
        ] {
            assert!(call(&ctx, builtin, &args).is_some(), "{builtin:?}");
        }
        assert!(call(&ctx, Builtin::StrBefore, &[lang, "\"missing\""])
            .unwrap()
            .starts_with("\"\""));
        assert!(call(&ctx, Builtin::StrAfter, &[lang, "\"\""])
            .unwrap()
            .contains("Hello World"));
        assert_eq!(call(&ctx, Builtin::StrLen, &["<iri>"]), None);
        assert_eq!(call(&ctx, Builtin::Concat, &["\"ok\"", "<iri>"]), None);
        assert_eq!(
            call(&ctx, Builtin::Replace, &["\"x\"", "\"[\"", "\"y\""]),
            None
        );

        let dt = lit("-2024-07-14T12:34:56.5-08:30", &format!("{XSD}dateTime"));
        for builtin in [
            Builtin::Year,
            Builtin::Month,
            Builtin::Day,
            Builtin::Hours,
            Builtin::Minutes,
            Builtin::Seconds,
            Builtin::Timezone,
            Builtin::Tz,
        ] {
            assert!(call(&ctx, builtin, &[&dt]).is_some(), "{builtin:?}");
        }
        assert!(call(
            &ctx,
            Builtin::Timezone,
            &[&lit("2024-01-01T00:00:00Z", &format!("{XSD}dateTime"))]
        )
        .is_some());
        assert_eq!(
            call(
                &ctx,
                Builtin::Timezone,
                &[&lit("2024-01-01T00:00:00", &format!("{XSD}dateTime"))]
            ),
            None
        );

        let string_true = lit("true", &format!("{XSD}string"));
        let bool_true = lit("true", &format!("{XSD}boolean"));
        for builtin in [
            Builtin::CastInteger,
            Builtin::CastDecimal,
            Builtin::CastFloat,
            Builtin::CastDouble,
            Builtin::CastBoolean,
            Builtin::CastString,
        ] {
            let source = if builtin == Builtin::CastBoolean {
                &string_true
            } else {
                &bool_true
            };
            assert!(call(&ctx, builtin, &[source]).is_some(), "{builtin:?}");
        }
        assert_eq!(
            call(
                &ctx,
                Builtin::CastInteger,
                &[&lit("1.2", &format!("{XSD}string"))]
            ),
            None
        );
        assert!(call(&ctx, Builtin::Rand, &[]).is_some());
        assert!(call(&ctx, Builtin::Uuid, &[])
            .unwrap()
            .starts_with("<urn:uuid:"));
        assert!(call(&ctx, Builtin::StrUuid, &[]).is_some());
        assert!(call(&ctx, Builtin::BNode, &["\"stable\""])
            .unwrap()
            .starts_with("_:b"));
        assert!(call(&ctx, Builtin::BNode, &[]).unwrap().starts_with("_:b"));
        assert_eq!(
            call(&ctx, Builtin::Datatype, &[lang]).unwrap(),
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#langString>"
        );
        assert_eq!(call(&ctx, Builtin::Lang, &[lang]).unwrap(), "\"en\"");
        assert_eq!(call(&ctx, Builtin::Datatype, &["<iri>"]), None);

        let triple = "<<<s> <p> \"o\">>";
        assert_eq!(
            call(&ctx, Builtin::Subject, &[triple]).as_deref(),
            Some("<s>")
        );
        assert_eq!(
            call(&ctx, Builtin::Predicate, &[triple]).as_deref(),
            Some("<p>")
        );
        assert_eq!(
            call(&ctx, Builtin::Object, &[triple]).as_deref(),
            Some("\"o\"")
        );
        assert_eq!(call(&ctx, Builtin::Subject, &["<s>"]), None);
        assert_eq!(
            call(&ctx, Builtin::TripleTerm, &["<s>", "<p>", "\"o\""]).as_deref(),
            Some(triple)
        );
    }

    #[test]
    fn boolean_and_geo_builtins_cover_success_and_type_errors() {
        let rete = fixture();
        let ctx = Ctx::new(&rete, Slots::new());
        let row = ctx.slots.empty_row();
        // `eval` is the three-valued result (`None` = a SPARQL error);
        // `boolean` is the FILTER verdict (true only on `Some(true)`).
        let eval = |builtin, args: &[&str]| {
            func_bool(
                builtin,
                &args
                    .iter()
                    .map(|v| FExpr::Const((*v).to_string()))
                    .collect::<Vec<_>>(),
                &ctx,
                &row,
            )
        };
        let boolean = |builtin, args: &[&str]| eval(builtin, args) == Some(true);
        assert!(boolean(Builtin::IsIri, &["<iri>"]));
        assert!(boolean(Builtin::IsBlank, &["_:b"]));
        assert!(boolean(Builtin::IsLiteral, &["\"x\""]));
        assert!(boolean(
            Builtin::IsNumeric,
            &[&lit("42", &format!("{XSD}integer"))]
        ));
        assert!(boolean(Builtin::IsTriple, &["<<<s> <p> <o>>>"]));
        assert!(boolean(Builtin::Contains, &["\"abc\"", "\"b\""]));
        assert!(boolean(Builtin::StrStarts, &["\"abc\"", "\"a\""]));
        assert!(boolean(Builtin::StrEnds, &["\"abc\"", "\"c\""]));
        assert_eq!(eval(Builtin::Contains, &["<iri>", "\"i\""]), None);
        assert!(boolean(Builtin::Regex, &["\"Abc\"", "\"^a\"", "\"i\""]));
        assert_eq!(eval(Builtin::Regex, &["<iri>", "\"i\""]), None);
        assert!(boolean(Builtin::LangMatches, &["\"en-GB\"", "\"EN\""]));
        assert!(boolean(Builtin::LangMatches, &["\"de\"", "\"*\""]));
        assert_eq!(eval(Builtin::LangMatches, &["\"\"", "\"*\""]), Some(false));

        let wkt = |s: &str| lit(s, &format!("{GEO}wktLiteral"));
        let point = wkt("POINT(1 1)");
        let same = wkt("POINT(1 1)");
        let far = wkt("POINT(5 5)");
        assert!(boolean(Builtin::GeoSfEquals, &[&point, &same]));
        assert!(boolean(Builtin::GeoSfDisjoint, &[&point, &far]));
        assert_eq!(eval(Builtin::GeoSfWithin, &["<iri>", &point]), None);
        assert!(call(&ctx, Builtin::GeoSfEquals, &[&point, &same]).is_some());
        assert!(call(
            &ctx,
            Builtin::GeoDistance,
            &[
                &point,
                &far,
                "<http://www.opengis.net/def/uom/OGC/1.0/metre>"
            ]
        )
        .is_some());
        assert!(call(&ctx, Builtin::GeoEnvelope, &[&point]).is_some());
        assert_eq!(call(&ctx, Builtin::GeoEnvelope, &["<iri>"]), None);
        assert_eq!(
            call(&ctx, Builtin::GeoDistance, &[&point, &far, "<bad-unit>"]),
            None
        );

        // geo3 (3D extension): AABB topology + distance over POINT Z / BOX3D.
        let wkt3 = |s: &str| lit(s, "https://w3id.org/rete/geo3#wktLiteral3D");
        let box3d = |s: &str| lit(s, "https://w3id.org/rete/geo3#box3dLiteral");
        let big = box3d("BOX3D(0 0 0, 10 10 10)");
        let inner = box3d("BOX3D(2 2 2, 4 4 4)");
        let p000 = wkt3("POINT Z(0 0 0)");
        let p_far = wkt3("POINT Z(3 4 12)"); // distance 13 from origin
        assert!(boolean(Builtin::Geo3Contains, &[&big, &inner]));
        assert_eq!(eval(Builtin::Geo3Contains, &[&inner, &big]), Some(false));
        assert!(boolean(Builtin::Geo3Within, &[&inner, &big]));
        // adjacency: two boxes 3 apart on Z — not touching, adjacent within gap 3
        let a = box3d("BOX3D(0 0 0, 1 1 1)");
        let b = box3d("BOX3D(0 0 3, 1 1 4)");
        assert_eq!(eval(Builtin::Geo3Adjacent, &[&a, &b]), Some(false));
        assert!(boolean(Builtin::Geo3Adjacent, &[&a, &b, "3"]));
        assert_eq!(eval(Builtin::Geo3Adjacent, &["<iri>", &b]), None);
        // distance3D → xsd:double "13"
        let d = call(&ctx, Builtin::Geo3Distance, &[&p000, &p_far]).unwrap();
        assert!(d.contains("13"), "distance3D result: {d}");
        assert_eq!(call(&ctx, Builtin::Geo3Distance, &[&p000, "<iri>"]), None);
    }

    #[test]
    fn lexical_numeric_cast_datetime_and_comparison_helpers_reject_bad_inputs() {
        assert_eq!(arith_number("\"2\""), None);
        assert_eq!(arith_number("2"), Some(2.0));
        assert!(is_numeric_dt(Some(&format!("{XSD}double"))));
        assert!(!is_numeric_dt(Some(&format!("{XSD}string"))));
        assert!(is_int_lexical("-12"));
        assert!(!is_int_lexical("+"));
        assert!(is_decimal_lexical(".5"));
        assert!(!is_decimal_lexical("1.2.3"));
        assert_eq!(parse_xsd_double("INF"), Some(f64::INFINITY));
        assert_eq!(parse_xsd_double("-INF"), Some(f64::NEG_INFINITY));
        assert!(parse_xsd_double("NaN").unwrap().is_nan());
        assert_eq!(parse_xsd_double("inf"), None);
        assert_eq!(parse_xsd_double("nope"), None);
        assert_eq!(fmt_plain(2.0), "2");
        assert_eq!(fmt_plain(2.5), "2.5");
        assert_eq!(bool_literal(true), lit("true", &format!("{XSD}boolean")));
        assert_eq!(encode_for_uri("a b/ä"), "a%20b%2F%C3%A4");
        assert_eq!(hash_hex(Builtin::Md5, "abc").len(), 32);
        assert_eq!(parse_datetime("invalid"), None);
        assert_eq!(tz_to_duration(""), None);
        assert_eq!(tz_to_duration("+00:00").as_deref(), Some("PT0S"));
        assert_eq!(tz_to_duration("+01:30").as_deref(), Some("PT1H30M"));
        assert_eq!(term_ebv(&lit("true", &format!("{XSD}boolean"))), Some(true));
        assert_eq!(term_ebv(&lit("0", &format!("{XSD}integer"))), Some(false));
        assert_eq!(term_ebv("<iri>"), None);
        assert!(compare(Op::Eq, "2", "2.0"));
        assert!(compare(Op::Ne, "a", "b"));
        assert!(compare(Op::Le, "a", "b"));
        assert!(compare(Op::Gt, "3", "2"));
        assert!(compare(Op::Ge, "3", "3"));
        assert!(lang_matches("en-US", "en"));
        assert!(!lang_matches("english", "en"));
    }

    /// The string built-ins operate on the WHOLE unescaped lexical value. The
    /// boolean predicates used to scan a literal token only up to its first `"`
    /// — escaped or not — so `CONTAINS("He said \"hi\" loudly", "loudly")` was
    /// false and the text after an embedded quote was invisible to CONTAINS /
    /// STRSTARTS / STRENDS / REGEX / LANGMATCHES. STRLEN and the other
    /// value-returning functions already read the full value; they are pinned
    /// here so the two paths cannot drift apart again.
    #[test]
    fn string_builtins_read_past_embedded_quotes_and_escapes() {
        let rete = fixture();
        let ctx = Ctx::new(&rete, Slots::new());
        let row = ctx.slots.empty_row();
        // `eval` is the three-valued result (`None` = a SPARQL error);
        // `boolean` is the FILTER verdict (true only on `Some(true)`).
        let eval = |builtin, args: &[&str]| {
            func_bool(
                builtin,
                &args
                    .iter()
                    .map(|v| FExpr::Const((*v).to_string()))
                    .collect::<Vec<_>>(),
                &ctx,
                &row,
            )
        };
        let boolean = |builtin, args: &[&str]| eval(builtin, args) == Some(true);
        // N-Triples token for the value `Theory of "Quantum" Gases` (25 chars).
        let quoted = r#""Theory of \"Quantum\" Gases""#;
        // Text before, inside, and after the embedded quotes.
        for needle in [
            "\"Theory\"",
            "\"Quantum\"",
            "\"Gases\"",
            "\"\\\"Quantum\\\" G\"",
        ] {
            assert!(boolean(Builtin::Contains, &[quoted, needle]), "{needle}");
        }
        assert!(boolean(
            Builtin::StrStarts,
            &[quoted, "\"Theory of \\\"Q\""]
        ));
        assert!(boolean(Builtin::StrEnds, &[quoted, "\"\\\" Gases\""]));
        assert!(boolean(Builtin::Regex, &[quoted, "\"gases$\"", "\"i\""]));
        assert!(boolean(
            Builtin::Regex,
            &[quoted, "\"^Theory of .Quantum. Gases$\""]
        ));
        // The needle `Quantum"` (an escaped quote of its own) is in the value…
        assert!(boolean(Builtin::Contains, &[quoted, "\"Quantum\\\"\""]));
        // …while the backslash of the escape is not: `Theory of \` was exactly
        // the truncated value the old scan produced.
        assert_eq!(
            eval(Builtin::Contains, &[quoted, "\"Theory of \\\\\""]),
            Some(false)
        );
        assert_eq!(
            call(&ctx, Builtin::StrBefore, &[quoted, "\"Gases\""]).as_deref(),
            Some(r#""Theory of \"Quantum\" ""#)
        );
        assert_eq!(
            call(&ctx, Builtin::StrAfter, &[quoted, "\"Quantum\""]).as_deref(),
            Some(r#""\" Gases""#)
        );
        assert_eq!(
            call(
                &ctx,
                Builtin::Replace,
                &[quoted, "\"Gases\"", "\"Liquids\""]
            )
            .as_deref(),
            Some(r#""Theory of \"Quantum\" Liquids""#)
        );
        assert_eq!(
            call(&ctx, Builtin::UCase, &[quoted]).as_deref(),
            Some(r#""THEORY OF \"QUANTUM\" GASES""#)
        );
        assert_eq!(
            call(&ctx, Builtin::SubStr, &[quoted, "12", "7"]).as_deref(),
            Some("\"Quantum\"")
        );
        assert_eq!(
            call(&ctx, Builtin::Concat, &[quoted, "\"!\""]).as_deref(),
            Some(r#""Theory of \"Quantum\" Gases!""#)
        );
        assert_eq!(
            call(&ctx, Builtin::StrLen, &[quoted]).as_deref(),
            Some(&*lit("25", &format!("{XSD}integer"))),
            "STRLEN counted the full value before the fix and must keep doing so"
        );

        // Backslash and newline escapes resolve to the characters they denote.
        let escaped = r#""a\\b\nc""#; // the value `a\b<newline>c`
        assert!(boolean(Builtin::Contains, &[escaped, "\"\\\\b\""]));
        assert!(boolean(Builtin::StrEnds, &[escaped, "\"\\nc\""]));
        assert!(boolean(Builtin::Regex, &[escaped, "\"^a.b.c$\"", "\"s\""]));
        assert!(boolean(Builtin::Regex, &[escaped, "\"^c$\"", "\"m\""]));
        assert_eq!(
            call(&ctx, Builtin::StrLen, &[escaped]).as_deref(),
            Some(&*lit("5", &format!("{XSD}integer")))
        );
        assert_eq!(
            call(&ctx, Builtin::EncodeForUri, &[escaped]).as_deref(),
            Some("\"a%5Cb%0Ac\"")
        );

        // A language tag or datatype suffix is never part of the value, and an
        // embedded quote does not make the suffix look like the value's end.
        let tagged = r#""He said \"Quantum\" loudly"@en"#;
        assert!(boolean(Builtin::Contains, &[tagged, "\"loudly\""]));
        assert_eq!(eval(Builtin::Contains, &[tagged, "\"@en\""]), Some(false));
        assert!(boolean(Builtin::StrEnds, &[tagged, "\"loudly\"@en"]));
        assert!(boolean(Builtin::LangMatches, &["\"en-GB\"", "\"en\""]));
        assert_eq!(
            call(&ctx, Builtin::StrAfter, &[tagged, "\"said \""]).as_deref(),
            Some(r#""\"Quantum\" loudly"@en"#),
            "the tag survives on the result, not inside the value"
        );
        assert_eq!(
            call(&ctx, Builtin::StrLen, &[tagged]).as_deref(),
            Some(&*lit("24", &format!("{XSD}integer")))
        );
        let typed = format!(r#""He said \"Quantum\" loudly"^^<{XSD}string>"#);
        assert!(boolean(Builtin::Contains, &[&typed, "\"loudly\""]));
        assert_eq!(eval(Builtin::Contains, &[&typed, "\"^^\""]), Some(false));
        assert!(boolean(Builtin::Regex, &[&typed, "\"loudly$\""]));
        assert_eq!(
            call(&ctx, Builtin::StrBefore, &[&typed, "\" loudly\""]).as_deref(),
            Some(r#""He said \"Quantum\"""#)
        );
        // A non-string datatype is still a type error for the string predicates.
        let int = lit("42", &format!("{XSD}integer"));
        assert_eq!(eval(Builtin::Contains, &[&int, "\"4\""]), None);
    }

    /// SPARQL 1.1 §17.4.3 argument compatibility for the binary string functions.
    /// The pattern (arg2) is compatible iff it is simple/`xsd:string`, or it
    /// shares arg1's language tag; an incompatible pattern is a type error
    /// (row eliminated in FILTER, unbound in value position). Only the pattern's
    /// tag against the text's matters — a tagged text with a simple pattern stays
    /// valid, a simple text with a tagged pattern does not.
    #[test]
    fn binary_string_functions_enforce_argument_compatibility() {
        let rete = fixture();
        let ctx = Ctx::new(&rete, Slots::new());
        let row = ctx.slots.empty_row();
        // `eval` is the three-valued result (`None` = a SPARQL error);
        // `boolean` is the FILTER verdict (true only on `Some(true)`).
        let eval = |builtin, args: &[&str]| {
            func_bool(
                builtin,
                &args
                    .iter()
                    .map(|v| FExpr::Const((*v).to_string()))
                    .collect::<Vec<_>>(),
                &ctx,
                &row,
            )
        };
        let boolean = |builtin, args: &[&str]| eval(builtin, args) == Some(true);

        let en = r#""hello world"@en"#; // text, language-tagged
        let xstr = format!(r#""hello world"^^<{XSD}string>"#); // text, xsd:string

        // --- CONTAINS across every compatibility branch ---------------------
        // simple text / simple pattern → compatible, matches.
        assert!(boolean(
            Builtin::Contains,
            &["\"hello world\"", "\"world\""]
        ));
        // tagged text / simple pattern → compatible (the CAUTION case), matches.
        assert!(boolean(Builtin::Contains, &[en, "\"world\""]));
        // simple text / tagged pattern → INCOMPATIBLE (asymmetry) → type error.
        assert_eq!(
            None,
            eval(Builtin::Contains, &["\"hello world\"", "\"world\"@en"])
        );
        // same language tag → compatible, matches.
        assert!(boolean(Builtin::Contains, &[en, "\"world\"@en"]));
        // different language tags (en vs fr) → INCOMPATIBLE → type error.
        assert_eq!(eval(Builtin::Contains, &[en, "\"world\"@fr"]), None);
        // xsd:string text / xsd:string pattern → compatible, matches.
        assert!(boolean(
            Builtin::Contains,
            &[&xstr, &lit("world", &format!("{XSD}string"))]
        ));
        // tagged text / xsd:string pattern → compatible (pattern untagged), matches.
        assert!(boolean(
            Builtin::Contains,
            &[en, &lit("world", &format!("{XSD}string"))]
        ));
        // xsd:string text / tagged pattern → INCOMPATIBLE (text untagged) → type error.
        assert_eq!(eval(Builtin::Contains, &[&xstr, "\"world\"@en"]), None);

        // --- STRSTARTS / STRENDS share the same helper ----------------------
        assert!(boolean(Builtin::StrStarts, &[en, "\"hello\""])); // simple pattern ok
        assert!(boolean(Builtin::StrStarts, &[en, "\"hello\"@en"])); // same tag ok
        assert_eq!(eval(Builtin::StrStarts, &[en, "\"hello\"@fr"]), None); // diff tag → error
        assert!(boolean(Builtin::StrEnds, &[en, "\"world\"@en"])); // same tag ok
        assert_eq!(eval(Builtin::StrEnds, &[en, "\"world\"@de"]), None); // diff tag → error
        assert_eq!(
            None,
            eval(Builtin::StrEnds, &["\"hello world\"", "\"world\"@en"])
        ); // simple/tagged → error

        // --- STRBEFORE / STRAFTER: incompatible → unbound (None); compatible →
        //     a result that carries arg1's language tag ----------------------
        // Incompatible pattern → type error (None) in value position.
        assert_eq!(call(&ctx, Builtin::StrBefore, &[en, "\"world\"@fr"]), None);
        assert_eq!(call(&ctx, Builtin::StrAfter, &[en, "\"hello \"@de"]), None);
        assert_eq!(
            call(
                &ctx,
                Builtin::StrBefore,
                &["\"hello world\"", "\"world\"@en"]
            ),
            None
        );
        // Simple pattern against tagged text → compatible; result keeps @en.
        assert_eq!(
            call(&ctx, Builtin::StrBefore, &[en, "\"world\""]).as_deref(),
            Some(r#""hello "@en"#)
        );
        // Same-tag pattern → compatible; result keeps @en.
        assert_eq!(
            call(&ctx, Builtin::StrAfter, &[en, "\"hello \"@en"]).as_deref(),
            Some(r#""world"@en"#)
        );
    }
}
