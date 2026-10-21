//! The value space SPARQL's operators work in: what `=`, `!=`, `<`, `>`, `<=`,
//! `>=`, `IN`, ORDER BY, MIN / MAX, SUM / AVG and arithmetic see in a term.
//!
//! SPARQL 1.1 §17.3 picks the XPath function behind an operator from the
//! operands' **datatypes**, never from what their text looks like:
//!
//! - two numerics (`xsd:integer`, `decimal`, `float`, `double` and the types
//!   derived from them) compare by value — `op:numeric-equal` and friends, with
//!   the usual promotion, so `"1"^^xsd:integer = "1.0"^^xsd:decimal` and a
//!   `NaN` equals nothing, itself included;
//! - two simple literals / `xsd:string`s compare by code point
//!   (`fn:compare(…) = 0`), so `"01" = "1"`, `"1.0" = "1"`, `"1e3" = "1000"` and
//!   `"inf" = "Infinity"` are all **false** and `"NaN" = "NaN"` is true;
//! - `xsd:boolean`, `xsd:dateTime` (and, as extensions shared with Oxigraph, the
//!   other XSD date/time types and the durations) compare by value;
//! - everything else falls to RDFterm-equal (§17.4.1.7): equal when it is the
//!   same term, a **type error** for two different literals whose datatype is
//!   not understood, `false` otherwise (an IRI is not a literal, a string is not
//!   a number). `<` and friends on such a pair are type errors.
//!
//! A language-tagged literal equals another with the same text and tag (tags
//! compare case-insensitively, as BCP 47 tags do), and orders against one with
//! the same tag. Literals of a numeric datatype whose text is not valid for it
//! (`"abc"^^xsd:integer`) are ill-typed: they behave like an unknown datatype.
//!
//! ORDER BY (§15.1) orders unbound < blank nodes < IRIs < literals < quoted
//! triples. Literals group by kind — text (strings, language-tagged literals,
//! unknown datatypes and ill-typed values, by text), then numbers, booleans,
//! date/time values and durations — each kind in its `<` order. MIN / MAX use
//! the same order (§18.5.1.5-6). See [`order_cmp`].
//!
//! A zoned and an unzoned date/time (`"2006-08-23Z"` vs `"2006-08-23"`) less
//! than 14 hours apart are neither equal nor unequal without an implicit
//! timezone, which rete does not assume: comparing them is an error, as the
//! DAWG tests `open-world/date-1` / `date-2` require.
//!
//! Where Oxigraph departs from the XPath functions the spec names, rete follows
//! the spec: `NaN < 1` and `NaN <= NaN` are false, not an error / true;
//! `false < true` is true, not an error; `<a> <= <a>` is a type error, not true;
//! and the zoned/unzoned pair above is an error, not false.
//!
//! [`Class`] is lifetime-free so ORDER BY can classify a row's key once; the
//! textual parts (a string's value, a language tag) are read back from the term
//! token when needed, without allocating unless the literal carries escapes.

use std::borrow::Cow;
use std::cmp::Ordering;

use super::Op;

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";
const RDF_DIR_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

/// What a term is to the operators: its kind and, for the datatypes compared
/// by value, that value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Class {
    Blank,
    Iri,
    /// An RDF-star quoted triple (`<< s p o >>`).
    Triple,
    /// A simple literal or an `xsd:string`.
    Str,
    /// An `rdf:langString` or `rdf:dirLangString`.
    Lang,
    Bool(bool),
    Num(Num),
    Time(Temporal),
    Dur(Duration),
    /// A literal of any other datatype, or of a known one whose lexical form is
    /// not valid for it: equal only to the same term, ordered by nothing.
    Other,
}

/// Classify a term token (see the module docs for the datatypes recognised).
pub(crate) fn classify(token: &str) -> Class {
    if token.starts_with("<<") && token.ends_with(">>") {
        return Class::Triple;
    }
    if token.starts_with('<') {
        return Class::Iri;
    }
    if token.starts_with("_:") {
        return Class::Blank;
    }
    let Some((body, suffix)) = crate::terms::literal_parts(token) else {
        return bare(token);
    };
    if suffix.is_empty() {
        return Class::Str;
    }
    if suffix.starts_with('@') {
        return Class::Lang;
    }
    let Some(local) = suffix
        .strip_prefix("^^<")
        .and_then(|s| s.strip_suffix('>'))
        .and_then(|dt| dt.strip_prefix(XSD))
    else {
        return Class::Other;
    };
    if local == "string" {
        return Class::Str;
    }
    let lex = unescape(body);
    let lex = lex.as_ref();
    let class = match local {
        "boolean" => match lex {
            "true" | "1" => Some(Class::Bool(true)),
            "false" | "0" => Some(Class::Bool(false)),
            _ => None,
        },
        "integer" | "int" | "long" | "short" | "byte" | "nonNegativeInteger"
        | "nonPositiveInteger" | "negativeInteger" | "positiveInteger" | "unsignedInt"
        | "unsignedLong" | "unsignedShort" | "unsignedByte" => {
            Num::exact(lex, NumKind::Integer).map(Class::Num)
        }
        "decimal" => Num::exact(lex, NumKind::Decimal).map(Class::Num),
        "double" => xsd_double(lex).map(|f| Class::Num(Num::double(f))),
        "float" => Num::float(lex).map(Class::Num),
        "dateTime" | "dateTimeStamp" => Temporal::parse(lex, TKind::DateTime).map(Class::Time),
        "date" => Temporal::parse(lex, TKind::Date).map(Class::Time),
        "time" => Temporal::parse(lex, TKind::Time).map(Class::Time),
        "gYear" => Temporal::parse(lex, TKind::GYear).map(Class::Time),
        "gYearMonth" => Temporal::parse(lex, TKind::GYearMonth).map(Class::Time),
        "gMonth" => Temporal::parse(lex, TKind::GMonth).map(Class::Time),
        "gDay" => Temporal::parse(lex, TKind::GDay).map(Class::Time),
        "gMonthDay" => Temporal::parse(lex, TKind::GMonthDay).map(Class::Time),
        "duration" => Duration::parse(lex, DKind::Any).map(Class::Dur),
        "yearMonthDuration" => Duration::parse(lex, DKind::YearMonth).map(Class::Dur),
        "dayTimeDuration" => Duration::parse(lex, DKind::DayTime).map(Class::Dur),
        _ => None,
    };
    class.unwrap_or(Class::Other)
}

/// Is `dt` (a datatype IRI's content) one of the XSD datatypes [`classify`]
/// reads a value from? A literal of one that classifies as [`Class::Other`]
/// is ill-typed rather than of an unknown datatype.
pub(crate) fn compared_by_value(dt: &str) -> bool {
    dt.strip_prefix(XSD).is_some_and(|local| {
        matches!(
            local,
            "string"
                | "boolean"
                | "integer"
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
                | "decimal"
                | "double"
                | "float"
                | "dateTime"
                | "dateTimeStamp"
                | "date"
                | "time"
                | "gYear"
                | "gYearMonth"
                | "gMonth"
                | "gDay"
                | "gMonthDay"
                | "duration"
                | "yearMonthDuration"
                | "dayTimeDuration"
        )
    })
}

/// A token that is none of IRI / blank node / literal — only ever an internal
/// constant (the `0` that unary minus lowers to): a number if it reads as one.
fn bare(token: &str) -> Class {
    if let Some(n) = Num::exact(token, NumKind::Integer) {
        return Class::Num(n);
    }
    if let Some(n) = Num::exact(token, NumKind::Decimal) {
        return Class::Num(n);
    }
    match xsd_double(token) {
        Some(f) => Class::Num(Num::double(f)),
        None => Class::Other,
    }
}

/// The numeric value of a term for arithmetic and SUM / AVG: a literal of a
/// numeric datatype with a valid lexical form (or an internal bare number),
/// else `None` — a string is not a number, whatever its text.
pub(crate) fn numeric(token: &str) -> Option<f64> {
    match classify(token) {
        Class::Num(n) => Some(n.to_f64()),
        _ => None,
    }
}

/// Is `token` a NaN `xsd:double` / `xsd:float` — the one term not equal to
/// itself?
fn is_nan(token: &str) -> bool {
    token.starts_with("\"NaN\"^^<")
        && matches!(classify(token), Class::Num(n) if n.to_f64().is_nan())
}

// --- the operators -----------------------------------------------------------

/// `a op b` (§17.3): `Some(bool)`, or `None` for a type error. (The evaluator
/// goes through `expr::compare_terms`, which also records diagnostics.)
#[cfg(test)]
pub(crate) fn compare(op: Op, a: &str, b: &str) -> Option<bool> {
    if let Some(v) = compare_identical(op, a, b) {
        return v;
    }
    compare_classified(op, a, &classify(a), b, &classify(b))
}

/// The answer when `a` and `b` are the same token, without classifying
/// either (the common case for `=` on joined values): equal unless NaN. `None`
/// when they differ — classify and use [`compare_classified`].
#[inline]
pub(crate) fn compare_identical(op: Op, a: &str, b: &str) -> Option<Option<bool>> {
    if a != b || !matches!(op, Op::Eq | Op::Ne) {
        return None;
    }
    let eq = !is_nan(a);
    Some(Some(if op == Op::Eq { eq } else { !eq }))
}

/// `a op b` on already-classified operands.
pub(crate) fn compare_classified(op: Op, a: &str, ca: &Class, b: &str, cb: &Class) -> Option<bool> {
    match op {
        Op::Eq => equals(a, ca, b, cb),
        Op::Ne => equals(a, ca, b, cb).map(|e| !e),
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            let ord = match operator_cmp(a, ca, b, cb)? {
                Some(o) => o,
                // A NaN: op:numeric-less-than / -greater-than are false, and so
                // are their `or numeric-equal` forms.
                None => return Some(false),
            };
            Some(match op {
                Op::Lt => ord == Ordering::Less,
                Op::Le => ord != Ordering::Greater,
                Op::Gt => ord == Ordering::Greater,
                _ => ord != Ordering::Less,
            })
        }
    }
}

/// RDFterm-equal with the §17.3 value comparisons: `Some(bool)`, or `None` for
/// a type error (two different literals that cannot be compared).
fn equals(a: &str, ca: &Class, b: &str, cb: &Class) -> Option<bool> {
    use Class::*;
    if a == b {
        return Some(!matches!(ca, Num(n) if n.to_f64().is_nan()));
    }
    Some(match (ca, cb) {
        // Not the same token, and IRIs / blank nodes are equal only to
        // themselves. (A quoted triple whose object is an equal-valued but
        // differently spelled literal is reported unequal: rete keeps the
        // canonical token, so a triple is identified by it.)
        (Iri | Blank | Triple, _) | (_, Iri | Blank | Triple) => false,
        (Lang, Lang) => lexical(a) == lexical(b) && tag(a).eq_ignore_ascii_case(tag(b)),
        (Lang, _) | (_, Lang) => false,
        (Other, Other) => {
            // The same term spelled with different escapes is still the same.
            if lexical(a) == lexical(b) && datatype(a) == datatype(b) {
                true
            } else {
                return None;
            }
        }
        (Other, _) | (_, Other) => return None,
        (Str, Str) => lexical(a) == lexical(b),
        (Bool(x), Bool(y)) => x == y,
        (Num(x), Num(y)) => x.cmp(y) == Some(Ordering::Equal),
        (Time(x), Time(y)) => return x.equals(y),
        (Dur(x), Dur(y)) => x == y,
        // Two known datatypes that are not comparable: a string is not a
        // number, a date is not a boolean.
        _ => false,
    })
}

/// The order `<` / `>` / `<=` / `>=` use: `None` for a type error (no `<` is
/// defined for the pair), `Some(None)` for numerics that are unordered (a NaN).
fn operator_cmp(a: &str, ca: &Class, b: &str, cb: &Class) -> Option<Option<Ordering>> {
    use Class::*;
    match (ca, cb) {
        (Num(x), Num(y)) => Some(x.cmp(y)),
        _ => literal_cmp(a, ca, b, cb).map(Some),
    }
}

/// `<` between two literals where SPARQL defines it, else `None`. Numerics
/// order with a NaN unordered (`None`).
fn literal_cmp(a: &str, ca: &Class, b: &str, cb: &Class) -> Option<Ordering> {
    use Class::*;
    match (ca, cb) {
        (Str, Str) => Some(lexical(a).cmp(&lexical(b))),
        (Lang, Lang) if tag(a).eq_ignore_ascii_case(tag(b)) => Some(lexical(a).cmp(&lexical(b))),
        (Bool(x), Bool(y)) => Some(x.cmp(y)),
        (Num(x), Num(y)) => x.cmp(y),
        (Time(x), Time(y)) => x.partial_cmp(y),
        (Dur(x), Dur(y)) => x.partial_cmp(y),
        _ => None,
    }
}

/// The ORDER BY order (§15.1) of two bound terms — a **total** order, as a
/// sort requires (Rust's sort may panic on a comparator that is not one).
///
/// Blank nodes, then IRIs (by their text), then literals, then quoted triples.
/// Literals group by kind, in this order:
///
/// 1. text — simple, `xsd:string`, language-tagged literals, literals of an
///    unknown datatype and ill-typed ones (`"820"^^xsd:gYear`, which XSD does
///    not accept), together by (lexical form, datatype, language tag): how
///    Oxigraph orders them, so labels with and without a tag interleave
///    alphabetically;
/// 2. numbers, by value (a NaN after every number);
/// 3. booleans; 4. date/time values (by kind, then on the time line);
///    5. durations.
///
/// Within a kind this agrees with `<` wherever `<` is defined. The order
/// *between* kinds is implementation-defined (§15.1); putting text first keeps
/// the values that are not of a column's real type (a stray string, an
/// ill-typed year) out of the top of a `DESC` / `MAX`. Ties (`1` and `1.0`)
/// break on the token.
pub(crate) fn order_cmp(a: &str, ca: &Class, b: &str, cb: &Class) -> Ordering {
    use Class::*;
    let rank = |c: &Class| match c {
        Blank => 0u8,
        Iri => 1,
        Str | Lang | Other => 2,
        Num(_) => 3,
        Bool(_) => 4,
        Time(_) => 5,
        Dur(_) => 6,
        Triple => 7,
    };
    rank(ca)
        .cmp(&rank(cb))
        .then_with(|| match (ca, cb) {
            (Iri, Iri) => crate::terms::iri_content(a)
                .unwrap_or(a)
                .cmp(crate::terms::iri_content(b).unwrap_or(b)),
            (Num(x), Num(y)) => x.order(y),
            (Bool(x), Bool(y)) => x.cmp(y),
            (Time(x), Time(y)) => x.order(y),
            (Dur(x), Dur(y)) => x.order_key().cmp(&y.order_key()),
            (Str | Lang | Other, Str | Lang | Other) => (lexical(a), datatype(a), lower(tag(a)))
                .cmp(&(lexical(b), datatype(b), lower(tag(b)))),
            // Blank nodes and quoted triples: by the token, below.
            _ => Ordering::Equal,
        })
        .then_with(|| a.cmp(b))
}

/// An ORDER BY / MIN / MAX key, classified once.
#[derive(Debug, Clone)]
pub(crate) struct OrderKey {
    pub(crate) token: std::rc::Rc<str>,
    pub(crate) class: Class,
}

impl OrderKey {
    pub(crate) fn new(token: std::rc::Rc<str>) -> Self {
        let class = classify(&token);
        OrderKey { token, class }
    }

    pub(crate) fn cmp(&self, other: &OrderKey) -> Ordering {
        order_cmp(&self.token, &self.class, &other.token, &other.class)
    }
}

// --- term text ---------------------------------------------------------------

fn unescape(body: &str) -> Cow<'_, str> {
    if body.contains('\\') {
        Cow::Owned(crate::terms::unescape_literal(body))
    } else {
        Cow::Borrowed(body)
    }
}

/// A literal's lexical form, unescaped; a non-literal's token.
fn lexical(token: &str) -> Cow<'_, str> {
    match crate::terms::literal_parts(token) {
        Some((body, _)) => unescape(body),
        None => Cow::Borrowed(token),
    }
}

/// A literal's language tag including any `--dir` (`""` when untagged).
fn tag(token: &str) -> &str {
    crate::terms::literal_parts(token)
        .and_then(|(_, s)| s.strip_prefix('@'))
        .unwrap_or("")
}

fn lower(s: &str) -> Cow<'_, str> {
    if s.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(s.to_ascii_lowercase())
    } else {
        Cow::Borrowed(s)
    }
}

/// A literal's datatype IRI (`xsd:string` for a simple literal,
/// `rdf:langString` / `rdf:dirLangString` for a tagged one); `""` otherwise.
fn datatype(token: &str) -> &str {
    match crate::terms::literal_parts(token) {
        Some((_, "")) => XSD_STRING,
        Some((_, s)) if s.starts_with('@') => {
            if s.contains("--") {
                RDF_DIR_LANG_STRING
            } else {
                RDF_LANG_STRING
            }
        }
        Some((_, s)) => s
            .strip_prefix("^^<")
            .and_then(|s| s.strip_suffix('>'))
            .unwrap_or(s),
        None => "",
    }
}

// --- numbers -----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumKind {
    Integer,
    Decimal,
    Float,
    Double,
}

/// A numeric literal's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Num {
    kind: NumKind,
    /// The lexical form read at `f64` precision: what arithmetic and SUM / AVG
    /// use (unchanged from before value-aware comparison existed).
    f: f64,
    /// `xsd:float` only: the value at its own precision, which comparisons
    /// use (XPath promotes a float to a double by value, so `"0.1"^^xsd:float`
    /// is not `"0.1"^^xsd:double`).
    f32v: f32,
    /// `xsd:integer` / `xsd:decimal`: `mantissa / 10^scale`, exact, when the
    /// digits fit an `i128` (compared exactly, so two 20-digit identifiers one
    /// apart are not equal).
    exact: Option<(i128, u32)>,
}

impl Num {
    fn double(f: f64) -> Num {
        Num {
            kind: NumKind::Double,
            f,
            f32v: f as f32,
            exact: None,
        }
    }

    fn float(lex: &str) -> Option<Num> {
        let f = xsd_double(lex)?;
        let f32v = if f.is_finite() {
            lex.parse::<f32>().ok()?
        } else {
            f as f32
        };
        Some(Num {
            kind: NumKind::Float,
            f,
            f32v,
            exact: None,
        })
    }

    /// An `xsd:integer` (`[+-]?[0-9]+`) or `xsd:decimal`
    /// (`[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)`) lexical form.
    fn exact(lex: &str, kind: NumKind) -> Option<Num> {
        let (neg, body) = match lex.as_bytes().first()? {
            b'-' => (true, &lex[1..]),
            b'+' => (false, &lex[1..]),
            _ => (false, lex),
        };
        let (int, frac) = match body.split_once('.') {
            Some((i, f)) if kind == NumKind::Decimal => (i, f),
            Some(_) => return None,
            None => (body, ""),
        };
        let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
        if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
            return None;
        }
        let frac = frac.trim_end_matches('0');
        let mut mant: Option<i128> = Some(0);
        for c in int.bytes().chain(frac.bytes()) {
            mant = mant
                .and_then(|m| m.checked_mul(10))
                .and_then(|m| m.checked_add(i128::from(c - b'0')));
        }
        let exact = mant.map(|m| (if neg { -m } else { m }, frac.len() as u32));
        let f = lex.parse::<f64>().ok()?;
        Some(Num {
            kind,
            f,
            f32v: f as f32,
            exact,
        })
    }

    pub(crate) fn to_f64(self) -> f64 {
        self.f
    }

    fn as_double(&self) -> f64 {
        if self.kind == NumKind::Float {
            f64::from(self.f32v)
        } else {
            self.f
        }
    }

    fn as_float(&self) -> f32 {
        if self.kind == NumKind::Float {
            self.f32v
        } else {
            self.f as f32
        }
    }

    /// XPath numeric comparison with type promotion (integer ⊂ decimal ⊂
    /// float ⊂ double); `None` when a NaN makes the pair unordered.
    fn cmp(&self, other: &Num) -> Option<Ordering> {
        use NumKind::*;
        match (self.kind, other.kind) {
            (Double, _) | (_, Double) => self.as_double().partial_cmp(&other.as_double()),
            (Float, _) | (_, Float) => self.as_float().partial_cmp(&other.as_float()),
            _ => match (self.exact, other.exact) {
                (Some(x), Some(y)) => exact_cmp(x, y).or_else(|| self.f.partial_cmp(&other.f)),
                _ => self.f.partial_cmp(&other.f),
            },
        }
    }

    /// The ORDER BY order: by value as a double (`-0` = `0`), a NaN after
    /// every number. Total, unlike [`Num::cmp`].
    fn order(&self, other: &Num) -> Ordering {
        let key = |n: &Num| {
            let d = n.as_double();
            (d.is_nan(), if d == 0.0 { 0.0 } else { d })
        };
        let ((nan_a, a), (nan_b, b)) = (key(self), key(other));
        nan_a.cmp(&nan_b).then_with(|| {
            if nan_a {
                Ordering::Equal
            } else {
                a.total_cmp(&b)
            }
        })
    }
}

/// Compare `m1 / 10^s1` with `m2 / 10^s2` exactly; `None` on overflow.
fn exact_cmp((m1, s1): (i128, u32), (m2, s2): (i128, u32)) -> Option<Ordering> {
    let scale = |m: i128, by: u32| 10i128.checked_pow(by).and_then(|p| m.checked_mul(p));
    Some(if s1 >= s2 {
        m1.cmp(&scale(m2, s1 - s2)?)
    } else {
        scale(m1, s2 - s1)?.cmp(&m2)
    })
}

/// An `xsd:double` / `xsd:float` lexical form (XSD 1.1:
/// `[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([Ee][+-]?[0-9]+)?`, `[+-]?INF`, `NaN`)
/// to its value. Stricter than Rust's parser, which also takes `inf`,
/// `infinity` and `nan` in any case — not XSD lexical forms.
pub(crate) fn xsd_double(lex: &str) -> Option<f64> {
    match lex {
        "INF" | "+INF" => return Some(f64::INFINITY),
        "-INF" => return Some(f64::NEG_INFINITY),
        "NaN" => return Some(f64::NAN),
        _ => {}
    }
    let body = lex.strip_prefix(['+', '-']).unwrap_or(lex);
    let (mantissa, exp) = match body.find(['e', 'E']) {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    let (int, frac) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    if (int.is_empty() && frac.is_empty()) || !digits(int) || !digits(frac) {
        return None;
    }
    if let Some(e) = exp {
        let e = e.strip_prefix(['+', '-']).unwrap_or(e);
        if e.is_empty() || !digits(e) {
            return None;
        }
    }
    lex.parse::<f64>().ok()
}

// --- dates and times -----------------------------------------------------------

/// The XSD date/time datatypes compared by value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TKind {
    DateTime,
    Date,
    Time,
    GYear,
    GYearMonth,
    GMonth,
    GDay,
    GMonthDay,
}

/// Fractional seconds are kept to 18 digits (as `xsd:decimal` is in Oxigraph).
const FRAC: i128 = 1_000_000_000_000_000_000;
/// The ±14:00 window XSD uses to order a zoned and an unzoned value.
const FOURTEEN_HOURS: i128 = 14 * 3600 * FRAC;

/// A date/time value: a point on the time line in units of 10⁻¹⁸ s, in UTC
/// when the value has a timezone and in its own local time when it does not.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Temporal {
    kind: TKind,
    value: i128,
    zoned: bool,
}

impl Temporal {
    fn parse(lex: &str, kind: TKind) -> Option<Temporal> {
        let (rest, tz) = split_timezone(lex)?;
        // XSD's reference values for the partial types: a leap year (1972) so
        // --02-29 is valid, and December so ---31 is.
        let (y, m, d, secs) = match kind {
            TKind::DateTime => {
                let (date, time) = rest.split_once('T')?;
                let (y, m, d) = parse_ymd(date)?;
                (y, m, d, parse_hms(time)?)
            }
            TKind::Date => {
                let (y, m, d) = parse_ymd(rest)?;
                (y, m, d, 0)
            }
            TKind::Time => (1972, 12, 31, parse_hms(rest)?),
            TKind::GYear => (parse_year(rest)?, 1, 1, 0),
            TKind::GYearMonth => {
                let (y, m) = rest.rsplit_once('-')?;
                (parse_year(y)?, two_digits(m)?, 1, 0)
            }
            TKind::GMonth => (1972, two_digits(rest.strip_prefix("--")?)?, 1, 0),
            TKind::GDay => (1972, 12, two_digits(rest.strip_prefix("---")?)?, 0),
            TKind::GMonthDay => {
                let (m, d) = rest.strip_prefix("--")?.split_once('-')?;
                (1972, two_digits(m)?, two_digits(d)?, 0)
            }
        };
        if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
            return None;
        }
        // `24:00:00` is the first instant of the next day (of the same day for
        // a bare time, which has no next day).
        let secs = if kind == TKind::Time {
            secs % (86_400 * FRAC)
        } else {
            secs
        };
        let local = i128::from(days_from_civil(y, m, d)) * 86_400 * FRAC + secs;
        Some(Temporal {
            kind,
            value: local - i128::from(tz.unwrap_or(0)) * 60 * FRAC,
            zoned: tz.is_some(),
        })
    }

    /// `op:dateTime-equal` (and its siblings). rete has no implicit timezone,
    /// so a zoned and an unzoned value within 14 hours of each other may or
    /// may not be equal: that is indeterminate, an error — what the DAWG test
    /// `open-world/date-2` requires of `"2006-08-23Z"^^xsd:date !=
    /// "2006-08-23"^^xsd:date` (Oxigraph answers false). Further apart they
    /// are certainly unequal.
    fn equals(&self, other: &Temporal) -> Option<bool> {
        if self.kind != other.kind {
            return Some(false);
        }
        if self.zoned == other.zoned {
            return Some(self.value == other.value);
        }
        // Certainly apart (more than 14 h): unequal. Within the window: unknown.
        self.partial_cmp(other).map(|_| false)
    }

    /// The XSD partial order: a zoned and an unzoned value are ordered only
    /// when they are more than 14 hours apart.
    fn partial_cmp(&self, other: &Temporal) -> Option<Ordering> {
        if self.kind != other.kind {
            return None;
        }
        if self.zoned == other.zoned {
            return Some(self.value.cmp(&other.value));
        }
        let (lo, hi) = (
            self.value.cmp(&(other.value - FOURTEEN_HOURS)),
            self.value.cmp(&(other.value + FOURTEEN_HOURS)),
        );
        (lo == hi).then_some(lo)
    }

    /// The ORDER BY order: by kind, then on the time line (an unzoned value
    /// read as UTC), then unzoned first. Total, and it agrees with
    /// [`Temporal::partial_cmp`] wherever that is determinate.
    fn order(&self, other: &Temporal) -> Ordering {
        (self.kind as u8, self.value, self.zoned).cmp(&(other.kind as u8, other.value, other.zoned))
    }
}

/// Split a trailing `Z` / `±hh:mm` timezone off (minutes east of UTC).
fn split_timezone(lex: &str) -> Option<(&str, Option<i32>)> {
    if let Some(rest) = lex.strip_suffix('Z') {
        return Some((rest, Some(0)));
    }
    let b = lex.as_bytes();
    if b.len() >= 6 && matches!(b[b.len() - 6], b'+' | b'-') && b[b.len() - 3] == b':' {
        let (rest, tz) = lex.split_at(lex.len() - 6);
        // A `-` that ends a date (`2020-05-01`) is not a timezone: a timezone
        // follows a digit and the whole value would not parse otherwise.
        let h = two_digits(&tz[1..3])?;
        let m = two_digits(&tz[4..6])?;
        if h > 14 || m > 59 || (h == 14 && m != 0) {
            return None;
        }
        let mins = (h * 60 + m) as i32;
        return Some((
            rest,
            Some(if b[b.len() - 6] == b'-' { -mins } else { mins }),
        ));
    }
    Some((lex, None))
}

/// `-?YYYY-MM-DD` (four or more year digits).
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let (ym, d) = s.rsplit_once('-')?;
    let (y, m) = ym.rsplit_once('-')?;
    Some((parse_year(y)?, two_digits(m)?, two_digits(d)?))
}

fn parse_year(s: &str) -> Option<i64> {
    let digits = s.strip_prefix('-').unwrap_or(s);
    if digits.len() < 4 || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

fn two_digits(s: &str) -> Option<u32> {
    (s.len() == 2 && s.bytes().all(|c| c.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

/// `hh:mm:ss(.s+)?` → seconds since midnight in 10⁻¹⁸ s (`24:00:00` allowed).
fn parse_hms(s: &str) -> Option<i128> {
    let mut parts = s.splitn(3, ':');
    let h = two_digits(parts.next()?)?;
    let m = two_digits(parts.next()?)?;
    let sec = parts.next()?;
    let (whole, frac) = sec.split_once('.').unwrap_or((sec, ""));
    let whole = two_digits(whole)?;
    if !frac.bytes().all(|c| c.is_ascii_digit()) || (sec.contains('.') && frac.is_empty()) {
        return None;
    }
    let frac_units = frac_to_units(frac);
    if h == 24 {
        return (m == 0 && whole == 0 && frac_units == 0).then_some(86_400 * FRAC);
    }
    if h > 23 || m > 59 || whole > 59 {
        return None;
    }
    Some(i128::from(h * 3600 + m * 60 + whole) * FRAC + frac_units)
}

/// Fractional-second digits as 10⁻¹⁸ s units (digits past the 18th dropped).
fn frac_to_units(frac: &str) -> i128 {
    let mut units: i128 = 0;
    for i in 0..18 {
        let d = frac.as_bytes().get(i).map_or(0, |c| i128::from(c - b'0'));
        units = units * 10 + d;
    }
    units
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days from 1970-01-01 to a proleptic Gregorian date (astronomical years, as
/// XSD 1.1 numbers them: year 0 is 1 BCE).
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = i64::from((m + 9) % 12);
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

// --- durations -------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DKind {
    Any,
    YearMonth,
    DayTime,
}

/// An `xsd:duration` (or `yearMonthDuration` / `dayTimeDuration`): months and
/// seconds (10⁻¹⁸ s units), which XSD keeps apart because a month has no fixed
/// length. Equal when both parts are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Duration {
    months: i64,
    secs: i128,
}

impl Duration {
    /// `-?P(nY)?(nM)?(nD)?(T(nH)?(nM)?(n(.n)?S)?)?`, at least one part, and a
    /// `T` only before a time part.
    fn parse(lex: &str, kind: DKind) -> Option<Duration> {
        let (neg, rest) = match lex.strip_prefix('-') {
            Some(r) => (true, r),
            None => (false, lex),
        };
        let rest = rest.strip_prefix('P')?;
        let (date, time) = match rest.split_once('T') {
            Some((d, t)) => (d, Some(t)),
            None => (rest, None),
        };
        let mut any = false;
        let mut months: i64 = 0;
        let mut secs: i128 = 0;
        let mut take = |s: &str, units: &[u8]| -> Option<Vec<(u8, String)>> {
            let mut out = Vec::new();
            let mut cur = String::new();
            let mut next_unit = 0;
            for c in s.bytes() {
                if c.is_ascii_digit() || c == b'.' {
                    cur.push(c as char);
                    continue;
                }
                let pos = units[next_unit..].iter().position(|&u| u == c)? + next_unit;
                if cur.is_empty() {
                    return None;
                }
                out.push((c, std::mem::take(&mut cur)));
                next_unit = pos + 1;
                any = true;
            }
            cur.is_empty().then_some(out)
        };
        let int = |v: &str| -> Option<i128> {
            if v.is_empty() || !v.bytes().all(|c| c.is_ascii_digit()) {
                return None;
            }
            v.parse().ok()
        };
        for (u, v) in take(date, b"YMD")? {
            match u {
                b'Y' => {
                    months = months.checked_add(i64::try_from(int(&v)?).ok()?.checked_mul(12)?)?
                }
                b'M' => months = months.checked_add(i64::try_from(int(&v)?).ok()?)?,
                _ => secs = secs.checked_add(int(&v)?.checked_mul(86_400 * FRAC)?)?,
            }
        }
        let mut time_parts = 0;
        if let Some(t) = time {
            for (u, v) in take(t, b"HMS")? {
                time_parts += 1;
                secs = secs.checked_add(match u {
                    b'H' => int(&v)?.checked_mul(3600 * FRAC)?,
                    b'M' => int(&v)?.checked_mul(60 * FRAC)?,
                    _ => {
                        let (w, f) = v.split_once('.').unwrap_or((&v, ""));
                        if v.contains('.') && f.is_empty() {
                            return None;
                        }
                        int(w)?.checked_mul(FRAC)? + frac_to_units(f)
                    }
                })?;
            }
            if time_parts == 0 {
                return None;
            }
        }
        if !any {
            return None;
        }
        let has_date_days = date.contains('D');
        let has_ym = date.contains('Y') || date.contains('M');
        match kind {
            DKind::YearMonth if has_date_days || time.is_some() => return None,
            DKind::DayTime if has_ym => return None,
            _ => {}
        }
        if neg {
            months = -months;
            secs = -secs;
        }
        Some(Duration { months, secs })
    }

    /// XSD's duration order: compare the two durations added to four
    /// reference dates (1696-09-01, 1697-02-01, 1903-03-01, 1903-07-01) and
    /// return the answer only when all four agree.
    fn partial_cmp(&self, other: &Duration) -> Option<Ordering> {
        let mut result = None;
        for start in REFERENCE_MONTHS {
            let o = self.at(start)?.cmp(&other.at(start)?);
            match result {
                None => result = Some(o),
                Some(r) if r != o => return None,
                _ => {}
            }
        }
        result
    }

    /// The end of this duration added to the first day of `(year, month)`, in
    /// 10⁻¹⁸ s since 1970.
    fn at(&self, (y, m): (i64, u32)) -> Option<i128> {
        let total = y
            .checked_mul(12)?
            .checked_add(i64::from(m) - 1 + self.months)?;
        let (y2, m2) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
        Some(i128::from(days_from_civil(y2, m2, 1)) * 86_400 * FRAC + self.secs)
    }

    /// The ORDER BY key: the duration's end from the first reference date,
    /// then its parts — total, and in [`Duration::partial_cmp`]'s order
    /// wherever that is determinate.
    fn order_key(&self) -> (i128, i64, i128) {
        (
            self.at(REFERENCE_MONTHS[0]).unwrap_or(i128::MAX),
            self.months,
            self.secs,
        )
    }
}

/// XSD's four reference starting points for ordering durations.
const REFERENCE_MONTHS: [(i64, u32); 4] = [(1696, 9), (1697, 2), (1903, 3), (1903, 7)];

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(lex: &str, local: &str) -> String {
        format!("\"{lex}\"^^<{XSD}{local}>")
    }

    fn s(lex: &str) -> String {
        format!("\"{lex}\"")
    }

    fn eq(a: &str, b: &str) -> Option<bool> {
        compare(Op::Eq, a, b)
    }

    /// The OpenAIRE QA repro (`e_nan_cmp.rq`): plain strings compare as text.
    #[test]
    fn plain_strings_are_never_numbers() {
        for (a, b, want) in [
            ("Nan", "Nan", true),
            ("NaN", "NaN", true),
            ("nan", "nan", true),
            ("1.0", "1", false),
            ("01", "1", false),
            ("inf", "Infinity", false),
            ("abc", "abc", true),
            ("1e3", "1000", false),
        ] {
            assert_eq!(eq(&s(a), &s(b)), Some(want), "{a} = {b}");
            assert_eq!(compare(Op::Ne, &s(a), &s(b)), Some(!want), "{a} != {b}");
        }
        // xsd:string is the same type as a simple literal.
        assert_eq!(eq(&s("x"), &lit("x", "string")), Some(true));
        assert_eq!(eq(&s("01"), &lit("1", "string")), Some(false));
        // `<` on strings is code-point order: "10" < "9".
        assert_eq!(compare(Op::Lt, &s("10"), &s("9")), Some(true));
        assert_eq!(compare(Op::Gt, &s("b"), &s("a")), Some(true));
        assert_eq!(compare(Op::Le, &s("a"), &s("a")), Some(true));
    }

    #[test]
    fn numerics_compare_by_value_across_types() {
        let one = lit("1", "integer");
        assert_eq!(eq(&one, &lit("1.0", "decimal")), Some(true));
        assert_eq!(eq(&one, &lit("01", "integer")), Some(true));
        assert_eq!(eq(&one, &lit("1e0", "double")), Some(true));
        assert_eq!(eq(&one, &lit("1", "int")), Some(true));
        assert_eq!(eq(&one, &lit("+1", "unsignedByte")), Some(true));
        assert_eq!(
            eq(&lit("1e3", "double"), &lit("1000", "integer")),
            Some(true)
        );
        assert_eq!(eq(&lit("-0", "double"), &lit("0", "double")), Some(true));
        assert_eq!(eq(&lit("-0", "integer"), &lit("0", "decimal")), Some(true));
        assert_eq!(eq(&lit("INF", "double"), &lit("INF", "float")), Some(true));
        assert_eq!(
            eq(&lit("-INF", "double"), &lit("INF", "double")),
            Some(false)
        );
        // Exact integers past 2^53: one apart is not equal.
        assert_eq!(
            eq(
                &lit("9007199254740993", "integer"),
                &lit("9007199254740992", "integer")
            ),
            Some(false)
        );
        assert_eq!(
            eq(&lit("0.30", "decimal"), &lit("0.3", "decimal")),
            Some(true)
        );
        // xsd:float keeps its own precision.
        assert_eq!(eq(&lit("0.1", "float"), &lit("0.1", "double")), Some(false));
        assert_eq!(eq(&lit("0.5", "float"), &lit("0.5", "double")), Some(true));
        assert_eq!(compare(Op::Lt, &one, &lit("1.5", "decimal")), Some(true));
        assert_eq!(
            compare(Op::Ge, &lit("2", "integer"), &lit("1.5", "double")),
            Some(true)
        );
    }

    #[test]
    fn nan_equals_nothing_and_orders_nowhere() {
        let nan = lit("NaN", "double");
        assert_eq!(eq(&nan, &nan), Some(false));
        assert_eq!(compare(Op::Ne, &nan, &nan), Some(true));
        assert_eq!(eq(&nan, &lit("NaN", "float")), Some(false));
        for op in [Op::Lt, Op::Le, Op::Gt, Op::Ge] {
            assert_eq!(compare(op, &nan, &nan), Some(false), "{op:?}");
            assert_eq!(
                compare(op, &nan, &lit("1", "integer")),
                Some(false),
                "{op:?}"
            );
        }
        // Only the XSD spellings are numbers: "nan"^^xsd:double is ill-typed.
        assert_eq!(classify(&lit("nan", "double")), Class::Other);
        assert_eq!(classify(&lit("inf", "double")), Class::Other);
        assert_eq!(classify(&lit("Infinity", "double")), Class::Other);
    }

    #[test]
    fn mixed_kinds_are_false_or_type_errors() {
        let one = lit("1", "integer");
        // A string is not a number: unequal, and unordered (a type error).
        assert_eq!(eq(&s("1"), &one), Some(false));
        assert_eq!(compare(Op::Ne, &s("1"), &one), Some(true));
        assert_eq!(compare(Op::Lt, &s("1"), &one), None);
        // IRIs and blank nodes: equal only to themselves, never ordered.
        assert_eq!(eq("<http://a>", "<http://a>"), Some(true));
        assert_eq!(eq("<http://a>", "<http://b>"), Some(false));
        assert_eq!(eq("<http://a>", &s("http://a")), Some(false));
        assert_eq!(compare(Op::Lt, "<http://a>", "<http://b>"), None);
        assert_eq!(compare(Op::Le, "<http://a>", "<http://a>"), None);
        assert_eq!(eq("_:b0", "_:b0"), Some(true));
        assert_eq!(eq("_:b0", "_:b1"), Some(false));
        // Unknown datatypes: the same term is equal; different ones are a type
        // error against each other and against a string; false against an IRI.
        let t1 = "\"a\"^^<http://ex/t>";
        let t2 = "\"b\"^^<http://ex/t>";
        assert_eq!(eq(t1, t1), Some(true));
        assert_eq!(eq(t1, t2), None);
        assert_eq!(compare(Op::Ne, t1, t2), None);
        assert_eq!(eq(t1, &s("a")), None);
        assert_eq!(eq(t1, &one), None);
        assert_eq!(eq(t1, "<http://a>"), Some(false));
        assert_eq!(eq(t1, "\"a\"@en"), Some(false));
        assert_eq!(compare(Op::Lt, t1, t1), None);
        // An ill-typed numeric is an unknown value, not a number.
        let bad = lit("abc", "integer");
        assert_eq!(eq(&bad, &bad), Some(true));
        assert_eq!(eq(&bad, &one), None);
        assert_eq!(eq(&lit("1e3", "integer"), &lit("1000", "integer")), None);
    }

    #[test]
    fn language_tags_compare_case_insensitively() {
        assert_eq!(eq("\"a\"@en", "\"a\"@EN"), Some(true));
        assert_eq!(eq("\"a\"@en", "\"a\"@fr"), Some(false));
        assert_eq!(eq("\"a\"@en", &s("a")), Some(false));
        assert_eq!(eq("\"a\"@en", "\"b\"@en"), Some(false));
        assert_eq!(compare(Op::Lt, "\"a\"@en", "\"b\"@EN"), Some(true));
        assert_eq!(compare(Op::Lt, "\"a\"@en", "\"b\"@fr"), None);
        assert_eq!(compare(Op::Lt, "\"a\"@en", &s("b")), None);
        assert_eq!(eq("\"a\"@ar--rtl", "\"a\"@AR--rtl"), Some(true));
        assert_eq!(eq("\"a\"@ar--rtl", "\"a\"@ar"), Some(false));
    }

    #[test]
    fn booleans_and_datetimes_compare_by_value() {
        assert_eq!(
            eq(&lit("1", "boolean"), &lit("true", "boolean")),
            Some(true)
        );
        assert_eq!(eq(&lit("true", "boolean"), &s("true")), Some(false));
        assert_eq!(
            compare(Op::Lt, &lit("false", "boolean"), &lit("true", "boolean")),
            Some(true)
        );
        assert_eq!(eq(&lit("yes", "boolean"), &lit("true", "boolean")), None);

        let dt = |l: &str| lit(l, "dateTime");
        assert_eq!(
            eq(
                &dt("2020-01-01T00:00:00Z"),
                &dt("2020-01-01T01:00:00+01:00")
            ),
            Some(true)
        );
        assert_eq!(
            eq(
                &dt("2020-01-01T00:00:00.50Z"),
                &dt("2020-01-01T00:00:00.5Z")
            ),
            Some(true)
        );
        assert_eq!(
            eq(&dt("2020-01-01T24:00:00"), &dt("2020-01-02T00:00:00")),
            Some(true)
        );
        // Zoned vs unzoned: indeterminate (an error) within ±14 h, unequal and
        // ordered beyond (DAWG open-world/date-1, date-2).
        assert_eq!(
            eq(&dt("2020-01-01T00:00:00Z"), &dt("2020-01-01T00:00:00")),
            None
        );
        assert_eq!(
            eq(&dt("2020-01-01T00:00:00Z"), &dt("2020-01-03T00:00:00")),
            Some(false)
        );
        assert_eq!(
            eq(&lit("2006-08-23Z", "date"), &lit("2006-08-23", "date")),
            None
        );
        assert_eq!(
            compare(
                Op::Ne,
                &lit("2001-01-01Z", "date"),
                &lit("2006-08-23", "date")
            ),
            Some(true)
        );
        assert_eq!(
            compare(
                Op::Lt,
                &dt("2020-01-01T00:00:00Z"),
                &dt("2020-01-01T10:00:00")
            ),
            None
        );
        assert_eq!(
            compare(
                Op::Lt,
                &dt("2020-01-01T00:00:00Z"),
                &dt("2020-01-02T15:00:00")
            ),
            Some(true)
        );
        assert_eq!(
            compare(
                Op::Gt,
                &dt("2021-01-01T00:00:00"),
                &dt("2020-12-31T23:59:59")
            ),
            Some(true)
        );
        // Years before 1 and past 9999.
        assert_eq!(
            compare(
                Op::Lt,
                &dt("-0044-03-15T00:00:00"),
                &dt("0001-01-01T00:00:00")
            ),
            Some(true)
        );
        assert_eq!(
            compare(
                Op::Lt,
                &dt("9999-12-31T00:00:00"),
                &dt("10000-01-01T00:00:00")
            ),
            Some(true)
        );
        // A dateTime is not a date, and an invalid one is ill-typed.
        assert_eq!(
            eq(&dt("2020-01-01T00:00:00"), &lit("2020-01-01", "date")),
            Some(false)
        );
        assert_eq!(classify(&dt("2020-02-30T00:00:00")), Class::Other);
        assert_eq!(classify(&dt("2020-01-01")), Class::Other);
        assert_eq!(
            compare(Op::Lt, &dt("2020-01-01T00:00:00"), &s("2021")),
            None
        );

        assert_eq!(
            compare(
                Op::Lt,
                &lit("2019-12-31", "date"),
                &lit("2020-01-01", "date")
            ),
            Some(true)
        );
        assert_eq!(
            eq(
                &lit("2020-01-01Z", "date"),
                &lit("2020-01-01+00:00", "date")
            ),
            Some(true)
        );
        assert_eq!(
            compare(Op::Lt, &lit("999", "gYear"), &lit("1000", "gYear")),
            None
        );
        assert_eq!(
            compare(Op::Lt, &lit("0999", "gYear"), &lit("1000", "gYear")),
            Some(true)
        );
        assert_eq!(
            compare(
                Op::Gt,
                &lit("2020-02", "gYearMonth"),
                &lit("2020-01", "gYearMonth")
            ),
            Some(true)
        );
        assert_eq!(
            eq(&lit("--02-29", "gMonthDay"), &lit("--02-29", "gMonthDay")),
            Some(true)
        );
        assert_eq!(
            compare(Op::Lt, &lit("---01", "gDay"), &lit("---31", "gDay")),
            Some(true)
        );
        assert_eq!(
            compare(Op::Lt, &lit("--01", "gMonth"), &lit("--12", "gMonth")),
            Some(true)
        );
        assert_eq!(
            compare(Op::Lt, &lit("09:00:00", "time"), &lit("10:00:00", "time")),
            Some(true)
        );
    }

    #[test]
    fn durations_compare_by_value() {
        let d = |l: &str| lit(l, "duration");
        assert_eq!(eq(&d("PT60M"), &d("PT1H")), Some(true));
        assert_eq!(eq(&d("P1Y"), &d("P12M")), Some(true));
        assert_eq!(eq(&d("P1D"), &lit("PT24H", "dayTimeDuration")), Some(true));
        assert_eq!(compare(Op::Lt, &d("PT59M"), &d("PT1H")), Some(true));
        assert_eq!(compare(Op::Lt, &d("P1M"), &d("P30D")), None); // 28..31 days
        assert_eq!(compare(Op::Lt, &d("P1M"), &d("P32D")), Some(true));
        assert_eq!(compare(Op::Gt, &d("-P1D"), &d("-P2D")), Some(true));
        assert_eq!(classify(&d("P")), Class::Other);
        assert_eq!(classify(&d("PT")), Class::Other);
        assert_eq!(classify(&d("P1H")), Class::Other);
        assert_eq!(classify(&lit("P1D", "yearMonthDuration")), Class::Other);
        assert_eq!(classify(&lit("P1Y", "dayTimeDuration")), Class::Other);
        assert_eq!(eq(&d("PT1.50S"), &d("PT1.5S")), Some(true));
    }

    #[test]
    fn order_by_puts_kinds_in_spec_order_and_values_in_value_order() {
        let key = |t: &str| OrderKey::new(std::rc::Rc::from(t));
        let mut v: Vec<OrderKey> = [
            "\"b\"",
            "<http://z>",
            "_:b1",
            &lit("10", "integer"),
            &lit("9", "integer"),
            "\"10\"",
            "\"9\"",
            "<http://a>",
            &lit("2.5", "decimal"),
        ]
        .iter()
        .map(|t| key(t))
        .collect();
        v.sort_by(|a, b| a.cmp(b));
        let got: Vec<&str> = v.iter().map(|k| &*k.token).collect();
        assert_eq!(
            got,
            [
                "_:b1",
                "<http://a>",
                "<http://z>",
                // Text first, by code point; then numbers by value.
                "\"10\"",
                "\"9\"",
                "\"b\"",
                &lit("2.5", "decimal"),
                &lit("9", "integer"),
                &lit("10", "integer"),
            ]
        );
        // An ill-typed year (XSD wants four digits) is text: it sorts before
        // every valid one, so it never tops a DESC or a MAX.
        assert_eq!(
            key(&lit("820", "gYear")).cmp(&key(&lit("1000", "gYear"))),
            Ordering::Less
        );
        assert_eq!(
            key(&lit("9999", "gYear")).cmp(&key(&lit("820", "gYear"))),
            Ordering::Greater
        );
        // A NaN after every number, -0 with 0, equal values by their token.
        let nan = key(&lit("NaN", "double"));
        assert_eq!(nan.cmp(&key(&lit("INF", "double"))), Ordering::Greater);
        assert_eq!(nan.cmp(&nan), Ordering::Equal);
        // -0 ties with 0 on value; the token breaks the tie.
        assert_eq!(
            key(&lit("-0", "double")).cmp(&key(&lit("0", "integer"))),
            Ordering::Less
        );
        assert_eq!(
            key(&lit("-0", "double")).cmp(&key(&lit("-1", "integer"))),
            Ordering::Greater
        );
        assert_eq!(
            key(&lit("1", "integer")).cmp(&key(&lit("1.0", "decimal"))),
            Ordering::Less
        );
        // IRIs by their text, not their token (`<a!>` vs `<a>`).
        assert_eq!(key("<a>").cmp(&key("<a!>")), Ordering::Less);
        // Tagged and untagged text interleave by text.
        assert_eq!(key("\"apple\"@en").cmp(&key("\"banana\"")), Ordering::Less);
        assert_eq!(
            key("\"cherry\"@en").cmp(&key("\"banana\"")),
            Ordering::Greater
        );
    }

    /// The ORDER BY comparator must be a total order over any mix of terms, or
    /// a sort may panic: check antisymmetry and transitivity on a mixed bag.
    #[test]
    fn order_by_is_a_total_order() {
        let terms: Vec<String> = vec![
            lit("10", "integer"),
            lit("9", "integer"),
            lit("9.0", "decimal"),
            lit("0.1", "float"),
            lit("0.1", "decimal"),
            lit("0.1", "double"),
            lit("NaN", "double"),
            lit("-0", "double"),
            lit("0", "integer"),
            lit("9007199254740993", "integer"),
            lit("9007199254740992", "integer"),
            lit("true", "boolean"),
            lit("2020-01-01T00:00:00Z", "dateTime"),
            lit("2020-01-01T05:00:00", "dateTime"),
            lit("2020-01-01", "date"),
            lit("P1M", "duration"),
            lit("P30D", "duration"),
            lit("abc", "integer"),
            "\"10\"".into(),
            "\"9\"".into(),
            "\"a\"@en".into(),
            "\"a\"@fr".into(),
            "\"a\"".into(),
            "\"a\"^^<http://ex/t>".into(),
            "<http://a>".into(),
            "_:b".into(),
            "<< <http://a> <http://b> <http://c> >>".into(),
        ];
        let keys: Vec<OrderKey> = terms
            .iter()
            .map(|t| OrderKey::new(std::rc::Rc::from(t.as_str())))
            .collect();
        for a in &keys {
            for b in &keys {
                assert_eq!(a.cmp(b), b.cmp(a).reverse(), "{} vs {}", a.token, b.token);
                for c in &keys {
                    if a.cmp(b) != Ordering::Greater && b.cmp(c) != Ordering::Greater {
                        assert_ne!(
                            a.cmp(c),
                            Ordering::Greater,
                            "{} <= {} <= {}",
                            a.token,
                            b.token,
                            c.token
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn numeric_rejects_strings() {
        assert_eq!(numeric(&s("5")), None);
        assert_eq!(numeric(&lit("5", "int")), Some(5.0));
        assert_eq!(numeric(&lit("5.5", "decimal")), Some(5.5));
        assert_eq!(numeric(&lit("x", "decimal")), None);
        assert_eq!(numeric("0"), Some(0.0));
        assert_eq!(numeric("<http://x>"), None);
        assert_eq!(numeric("\"5\"@en"), None);
        assert_eq!(xsd_double("1."), Some(1.0));
        assert_eq!(xsd_double(".5e1"), Some(5.0));
        assert_eq!(xsd_double("+INF"), Some(f64::INFINITY));
        assert_eq!(xsd_double("1e"), None);
        assert_eq!(xsd_double("+nan"), None);
        assert_eq!(xsd_double("."), None);
        assert_eq!(xsd_double(" 1"), None);
    }
}
