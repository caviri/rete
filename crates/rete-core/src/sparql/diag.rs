//! Query diagnostics: a side channel for the expression **type errors** that
//! SPARQL silently resolves: a FILTER or HAVING drops the row (an error stays an
//! error through `!`), BIND, projection and ORDER BY see an unbound value.
//!
//! `FILTER(CONTAINS(?iri, "x"))` is not unsupported — it is a type error,
//! because CONTAINS takes string literals and an IRI is not one, and the spec
//! says a FILTER whose expression errors drops the row. That is correct and it
//! stays: results are never changed by this module. What changes is that the error is
//! *counted*, so a query that came back empty can say why.
//!
//! Recording happens only on the error path (and, for the case-sensitivity
//! hint, once per function per query on the no-match path), into a
//! thread-local sink — SPARQL evaluation is single-threaded, and a
//! thread-local keeps two queries running on two server threads from
//! mixing their warnings. The sink is bounded: one entry per
//! `(function, argument position, argument kind)`, holding a count and the
//! first offending value.

use std::cell::RefCell;

use super::{ArithOp, Builtin, FExpr, Op, QueryOutput};

/// Where a type error was raised: a built-in function, or an operator
/// (`+ - * /`, a comparison, `IN`), which SPARQL defines as functions too
/// (`op:numeric-add`, `op:numeric-equal`, …) but which have no call syntax.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Site {
    Fn(Builtin),
    /// The operator's SPARQL spelling: `+`, `<=`, `IN`, …
    Op(&'static str),
}

impl From<Builtin> for Site {
    fn from(f: Builtin) -> Self {
        Site::Fn(f)
    }
}

impl Site {
    pub(crate) fn arith(op: ArithOp) -> Site {
        Site::Op(match op {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
        })
    }

    pub(crate) fn compare(op: Op) -> Site {
        Site::Op(match op {
            Op::Eq => "=",
            Op::Ne => "!=",
            Op::Lt => "<",
            Op::Le => "<=",
            Op::Gt => ">",
            Op::Ge => ">=",
        })
    }

    fn name(self) -> &'static str {
        match self {
            Site::Fn(f) => fn_name(f),
            Site::Op(sym) => sym,
        }
    }

    /// The family a site belongs to, which decides the hint.
    fn family(self) -> Family {
        match self {
            Site::Fn(Builtin::Abs | Builtin::Ceil | Builtin::Floor | Builtin::Round) => {
                Family::Numeric
            }
            Site::Op("+" | "-" | "*" | "/") => Family::Numeric,
            Site::Op(_) => Family::Compare,
            Site::Fn(Builtin::Lang | Builtin::Datatype) => Family::LiteralAccessor,
            Site::Fn(Builtin::StrDt | Builtin::StrLang) => Family::Constructor,
            Site::Fn(_) => Family::Other,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Family {
    Numeric,
    Compare,
    LiteralAccessor,
    Constructor,
    Other,
}

/// How serious a [`QueryWarning`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningSeverity {
    /// A SPARQL type error was raised and silently absorbed (FILTER drops the
    /// row, BIND → unbound). The results are spec-correct; the query probably is not
    /// what its author meant.
    TypeError,
    /// No error happened; a guess at why a query came back empty (for example,
    /// string matching is case-sensitive). Phrased as a suggestion.
    Hint,
}

impl WarningSeverity {
    /// `"type-error"` or `"hint"` — the JSON spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            WarningSeverity::TypeError => "type-error",
            WarningSeverity::Hint => "hint",
        }
    }
}

/// One diagnostic about a query evaluation: a counted class of type errors, or
/// a hint. Returned by [`eval_query_with_warnings`](super::eval_query_with_warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct QueryWarning {
    pub severity: WarningSeverity,
    /// The SPARQL function name, upper-case (`"CONTAINS"`), or the operator's
    /// spelling for an operator: `+` `-` `*` `/`, `=` `!=` `<` `<=` `>` `>=`,
    /// `IN`.
    pub function: String,
    /// 1-based argument position the problem is about (0 = not argument-specific).
    /// For an operator, 1 is the left operand and 2 the right one.
    pub argument: usize,
    /// What the argument was: `iri`, `blank-node`, `quoted-triple`, `numeric`,
    /// `typed-literal`, `string`, `language-tagged`, `invalid-number`,
    /// `language-mismatch`, `unbound`, `invalid-regex`, `not-a-datetime`,
    /// `division-by-zero` (argument 2 of `/`), or `case-sensitive` for the
    /// hint.
    pub kind: String,
    /// How many evaluations raised this error (an evaluation is one row reaching
    /// the expression; LIMIT / ASK stop early, so this is not a data count).
    pub count: u64,
    /// The first offending value, truncated to 80 characters (plus `…`).
    pub sample: Option<String>,
    /// A short, actionable suggestion.
    pub hint: String,
    /// The whole warning as one plain-language sentence.
    pub message: String,
}

/// Samples are cut to this many characters (plus `…`).
const SAMPLE_CHARS: usize = 80;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArgKind {
    Iri,
    Blank,
    QuotedTriple,
    Numeric,
    TypedLiteral,
    LangMismatch,
    Unbound,
    InvalidRegex,
    NotDatetime,
    /// A string literal (plain or `xsd:string`) where a number, a literal
    /// accessor's operand or a simple literal was needed.
    StringLit,
    /// A language-tagged literal where a simple literal was needed.
    LangTagged,
    /// A literal of a numeric datatype whose lexical form is not a number.
    InvalidNumber,
    /// The divisor of `/` was zero.
    DivByZero,
}

impl ArgKind {
    fn as_str(self) -> &'static str {
        match self {
            ArgKind::Iri => "iri",
            ArgKind::Blank => "blank-node",
            ArgKind::QuotedTriple => "quoted-triple",
            ArgKind::Numeric => "numeric",
            ArgKind::TypedLiteral => "typed-literal",
            ArgKind::LangMismatch => "language-mismatch",
            ArgKind::Unbound => "unbound",
            ArgKind::InvalidRegex => "invalid-regex",
            ArgKind::NotDatetime => "not-a-datetime",
            ArgKind::StringLit => "string",
            ArgKind::LangTagged => "language-tagged",
            ArgKind::InvalidNumber => "invalid-number",
            ArgKind::DivByZero => "division-by-zero",
        }
    }
}

struct Entry {
    site: Site,
    pos: usize,
    kind: ArgKind,
    count: u64,
    sample: String,
    /// Kind-specific detail: a datatype, the two language tags, a regex error.
    detail: String,
}

/// Bits of the case-sensitivity probe ([`note_no_match`]).
pub(crate) const MISS_CONTAINS: u8 = 1;
pub(crate) const MISS_STRSTARTS: u8 = 2;
pub(crate) const MISS_STRENDS: u8 = 4;
pub(crate) const MISS_REGEX: u8 = 8;

#[derive(Default)]
struct Sink {
    entries: Vec<Entry>,
    /// Constant needles of string predicates that returned `false` on valid
    /// string arguments at least once: `(function, needle)`.
    misses: Vec<(Builtin, String)>,
}

thread_local! {
    static SINK: RefCell<Sink> = RefCell::new(Sink::default());
}

/// Start a fresh diagnostic scope (drops anything a previous query left).
pub(crate) fn begin() {
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        s.entries.clear();
        s.misses.clear();
    });
}

/// Record one type error. `describe` builds `(sample, detail)` and runs only
/// for the first occurrence of a `(func, pos, kind)` key.
#[cold]
#[inline(never)]
pub(crate) fn record(
    site: impl Into<Site>,
    pos: usize,
    kind: ArgKind,
    describe: impl FnOnce() -> (String, String),
) {
    let site = site.into();
    SINK.with(|s| {
        let mut s = s.borrow_mut();
        if let Some(e) = s
            .entries
            .iter_mut()
            .find(|e| e.site == site && e.pos == pos && e.kind == kind)
        {
            e.count += 1;
            return;
        }
        let (sample, detail) = describe();
        s.entries.push(Entry {
            site,
            pos,
            kind,
            count: 1,
            sample: truncate(&sample),
            detail,
        });
    });
}

/// Record that a string predicate returned no match on valid string
/// arguments. Called once per function per query (the caller keeps a bit in
/// `Ctx`); the needle is kept only when it is a constant in the query.
#[cold]
#[inline(never)]
pub(crate) fn note_no_match(func: Builtin, needle: &FExpr) {
    if let FExpr::Const(c) = needle {
        let lex = crate::terms::literal_lexical(c).unwrap_or_else(|| c.clone());
        SINK.with(|s| s.borrow_mut().misses.push((func, lex)));
    }
}

/// A string function rejected the bound term `token` at 1-based `pos`.
#[cold]
#[inline(never)]
pub(crate) fn bad_arg(func: Builtin, pos: usize, token: &str) {
    record(func, pos, kind_of(token), || {
        (token.to_string(), detail_of(token))
    });
}

/// Both arguments of a binary string function are strings, but argument 2
/// carries a language tag that argument 1 does not share (§17.4.3).
#[cold]
#[inline(never)]
pub(crate) fn lang_mismatch(func: Builtin, text: &str, pat: &str) {
    record(func, 2, ArgKind::LangMismatch, || {
        let tag = |t: &str| match crate::terms::lang_tag(t).filter(|l| !l.is_empty()) {
            Some(l) => format!("@{l}"),
            None => "no tag".to_string(),
        };
        (pat.to_string(), format!("{} vs {}", tag(pat), tag(text)))
    });
}

/// The argument at 1-based `pos` is the variable `var`, unbound in this row.
#[cold]
#[inline(never)]
pub(crate) fn unbound(site: impl Into<Site>, pos: usize, var: &str) {
    record(site, pos, ArgKind::Unbound, || {
        (format!("?{var}"), String::new())
    });
}

/// The REGEX / REPLACE pattern does not compile.
#[cold]
#[inline(never)]
pub(crate) fn invalid_regex(func: Builtin, pattern: &str, flags: &str) {
    record(func, 2, ArgKind::InvalidRegex, || {
        (
            pattern.to_string(),
            crate::row::Resolver::regex_error(pattern, flags),
        )
    });
}

/// A date/time accessor got a value that does not parse as an xsd:dateTime.
#[cold]
#[inline(never)]
pub(crate) fn bad_datetime(func: Builtin, token: &str) {
    record(func, 1, ArgKind::NotDatetime, || {
        let detail = match crate::terms::literal_datatype(token) {
            Some(dt) if token.starts_with('"') => short_dt(&dt),
            _ => kind_of(token).as_str().to_string(),
        };
        (token.to_string(), detail)
    });
}

/// A numeric function or an arithmetic operator got a term that is not a
/// number (by the rule that site applies: arithmetic takes numeric-typed
/// literals only, ABS / CEIL / FLOOR / ROUND any literal whose text parses).
#[cold]
#[inline(never)]
pub(crate) fn not_a_number(site: impl Into<Site>, pos: usize, token: &str) {
    let kind = match term_kind(token) {
        ArgKind::Numeric => ArgKind::InvalidNumber,
        k => k,
    };
    record(site, pos, kind, || {
        (token.to_string(), detail_of_any(token))
    });
}

/// `/` with a zero divisor (`token` is the divisor as written).
#[cold]
#[inline(never)]
pub(crate) fn div_by_zero(token: &str) {
    record(Site::Op("/"), 2, ArgKind::DivByZero, || {
        (token.to_string(), detail_of(token))
    });
}

/// LANG / DATATYPE got a term that is not a literal.
#[cold]
#[inline(never)]
pub(crate) fn not_a_literal(func: Builtin, token: &str) {
    record(func, 1, kind_of(token), || {
        (token.to_string(), String::new())
    });
}

/// STRDT / STRLANG argument 1 is not a simple literal, or STRDT argument 2 is
/// not an IRI.
#[cold]
#[inline(never)]
pub(crate) fn bad_constructor_arg(func: Builtin, pos: usize, token: &str) {
    record(func, pos, term_kind(token), || {
        (token.to_string(), detail_of_any(token))
    });
}

/// Classify any term, telling string and language-tagged literals apart from
/// other typed ones (no allocation for IRIs and blank nodes).
fn term_kind(token: &str) -> ArgKind {
    if !token.starts_with('"') {
        return kind_of(token);
    }
    if crate::terms::lang_tag(token).is_some_and(|l| !l.is_empty()) {
        return ArgKind::LangTagged;
    }
    match crate::terms::literal_datatype(token) {
        Some(dt) if dt == "http://www.w3.org/2001/XMLSchema#string" => ArgKind::StringLit,
        None => ArgKind::StringLit,
        Some(dt) if is_numeric_datatype(&dt) => ArgKind::Numeric,
        Some(_) => ArgKind::TypedLiteral,
    }
}

/// [`detail_of`], but a language-tagged literal reports its tag (`@en`).
fn detail_of_any(token: &str) -> String {
    match crate::terms::lang_tag(token).filter(|l| !l.is_empty()) {
        Some(l) if token.starts_with('"') => format!("@{l}"),
        _ => detail_of(token),
    }
}

/// Classify a bound term that a string function rejected (no allocation for
/// IRIs and blank nodes, the common case).
fn kind_of(token: &str) -> ArgKind {
    if token.starts_with("<<") {
        ArgKind::QuotedTriple
    } else if token.starts_with('<') {
        ArgKind::Iri
    } else if token.starts_with("_:") {
        ArgKind::Blank
    } else if crate::terms::literal_datatype(token).is_some_and(|dt| is_numeric_datatype(&dt)) {
        ArgKind::Numeric
    } else {
        ArgKind::TypedLiteral
    }
}

/// The short datatype of a literal (`xsd:integer`), else `""`.
fn detail_of(token: &str) -> String {
    match crate::terms::literal_datatype(token) {
        Some(dt) if token.starts_with('"') => short_dt(&dt),
        _ => String::new(),
    }
}

fn is_numeric_datatype(dt: &str) -> bool {
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

fn short_dt(dt: &str) -> String {
    match dt.strip_prefix("http://www.w3.org/2001/XMLSchema#") {
        Some(local) => format!("xsd:{local}"),
        None => format!("<{dt}>"),
    }
}

fn truncate(s: &str) -> String {
    match s.char_indices().nth(SAMPLE_CHARS) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// SPARQL spelling of a builtin, for messages.
pub(crate) fn fn_name(f: Builtin) -> &'static str {
    match f {
        Builtin::Str => "STR",
        Builtin::StrLen => "STRLEN",
        Builtin::UCase => "UCASE",
        Builtin::LCase => "LCASE",
        Builtin::Concat => "CONCAT",
        Builtin::SubStr => "SUBSTR",
        Builtin::StrBefore => "STRBEFORE",
        Builtin::StrAfter => "STRAFTER",
        Builtin::Contains => "CONTAINS",
        Builtin::StrStarts => "STRSTARTS",
        Builtin::StrEnds => "STRENDS",
        Builtin::Regex => "REGEX",
        Builtin::Replace => "REPLACE",
        Builtin::EncodeForUri => "ENCODE_FOR_URI",
        Builtin::Md5 => "MD5",
        Builtin::Sha1 => "SHA1",
        Builtin::Sha256 => "SHA256",
        Builtin::Sha384 => "SHA384",
        Builtin::Sha512 => "SHA512",
        Builtin::Year => "YEAR",
        Builtin::Month => "MONTH",
        Builtin::Day => "DAY",
        Builtin::Hours => "HOURS",
        Builtin::Minutes => "MINUTES",
        Builtin::Seconds => "SECONDS",
        Builtin::Timezone => "TIMEZONE",
        Builtin::Tz => "TZ",
        Builtin::Abs => "ABS",
        Builtin::Ceil => "CEIL",
        Builtin::Floor => "FLOOR",
        Builtin::Round => "ROUND",
        Builtin::Lang => "LANG",
        Builtin::Datatype => "DATATYPE",
        Builtin::StrDt => "STRDT",
        Builtin::StrLang => "STRLANG",
        _ => "function",
    }
}

fn ordinal(site: Site, pos: usize) -> String {
    match (site, pos) {
        (Site::Op(_), 1) => "the left operand".to_string(),
        (Site::Op(_), 2) => "the right operand".to_string(),
        _ => format!("argument {pos}"),
    }
}

fn rows(n: u64) -> String {
    if n == 1 {
        "1 row".to_string()
    } else {
        format!("{n} rows")
    }
}

/// The hint for a site family other than the string functions, or `None` to
/// keep the kind's own hint (which was written for the string functions).
fn family_hint(family: Family, pos: usize, kind: ArgKind) -> Option<&'static str> {
    use ArgKind::*;
    Some(match (family, kind) {
        (_, Unbound | InvalidRegex | NotDatetime | LangMismatch | DivByZero) => return None,
        (Family::Numeric, StringLit) => {
            "a string is not a number; cast one that holds a number first, e.g. \
             xsd:decimal(?x), or restrict the variable with isNumeric()"
        }
        (Family::Numeric, InvalidNumber) => {
            "its text is not a valid number for its datatype; restrict the variable with \
             isNumeric(), which checks the lexical form"
        }
        (Family::Numeric, _) => "it is not a number; restrict the variable with isNumeric()",
        (Family::LiteralAccessor, _) => {
            "LANG() and DATATYPE() take a literal; restrict the variable with isLiteral()"
        }
        (Family::Constructor, Blank | QuotedTriple) if pos == 1 => {
            "argument 1 must be a simple literal; restrict the variable with isLiteral()"
        }
        (Family::Constructor, _) if pos == 1 => {
            "argument 1 must be a simple literal (no language tag, no datatype); use STR() on \
             it to take its lexical form"
        }
        (Family::Constructor, _) => {
            "argument 2 of STRDT must be a datatype IRI, e.g. xsd:integer; IRI(?x) turns a \
             string into one"
        }
        (Family::Compare | Family::Other, _) => return None,
    })
}

impl Entry {
    fn into_warning(self) -> QueryWarning {
        let name = self.site.name();
        let arg = ordinal(self.site, self.pos);
        let n = rows(self.count);
        let dt = |what: &str| {
            if self.detail.is_empty() {
                what.to_string()
            } else {
                format!("{what} ({})", self.detail)
            }
        };
        let (what, hint) = match self.kind {
            ArgKind::Iri => (
                "an IRI".to_string(),
                "wrap it in STR() to match the IRI's text".to_string(),
            ),
            ArgKind::Blank => (
                "a blank node".to_string(),
                "blank nodes are not strings; restrict the variable with isLiteral()".to_string(),
            ),
            ArgKind::QuotedTriple => (
                "a quoted triple".to_string(),
                "quoted triples are not strings; restrict the variable with isLiteral()"
                    .to_string(),
            ),
            ArgKind::Numeric => (
                dt("a number"),
                "wrap it in STR() to match its lexical form".to_string(),
            ),
            ArgKind::TypedLiteral => (
                dt("a non-string typed literal"),
                "wrap it in STR() to match its lexical form".to_string(),
            ),
            ArgKind::StringLit => (
                "a string".to_string(),
                "wrap it in STR() to match its lexical form".to_string(),
            ),
            ArgKind::LangTagged => (
                dt("a language-tagged literal"),
                "use STR() on it to drop the language tag".to_string(),
            ),
            ArgKind::InvalidNumber => (
                dt("a literal whose text is not a valid number for its datatype"),
                String::new(),
            ),
            ArgKind::LangMismatch => (
                format!(
                    "a literal whose language tag does not match argument 1 ({})",
                    self.detail
                ),
                "the language tags differ; use STR() on both arguments".to_string(),
            ),
            ArgKind::Unbound => (
                format!("an unbound variable ({})", self.sample),
                "the variable has no value in those rows (an OPTIONAL that did not match, \
                 or a misspelled variable); use BOUND() or COALESCE() if that is intended"
                    .to_string(),
            ),
            ArgKind::InvalidRegex => (
                "an invalid regex pattern".to_string(),
                format!(
                    "invalid regex pattern ({}); rete uses Rust regex syntax, which has no \
                     look-around or back-references",
                    self.detail
                ),
            ),
            ArgKind::NotDatetime => (
                dt("a value that is not an xsd:dateTime"),
                "this function needs an xsd:dateTime; for a year in a plain or xsd:date value \
                 use SUBSTR(STR(?x), 1, 4)"
                    .to_string(),
            ),
            ArgKind::DivByZero => (
                "zero".to_string(),
                "guard the divisor, e.g. IF(?d != 0, ?n / ?d, 0), or FILTER(?d != 0) first"
                    .to_string(),
            ),
        };
        let hint = match family_hint(self.site.family(), self.pos, self.kind) {
            Some(h) => h.to_string(),
            None => hint,
        };
        let mut message = match (self.kind, self.site) {
            (ArgKind::DivByZero, _) => format!("{name} divided by zero in {n}: a SPARQL error"),
            (_, Site::Op(_)) => {
                format!("Operator {name} received {what} as {arg} in {n}: a SPARQL type error")
            }
            _ => format!("{name} received {what} as {arg} in {n}: a SPARQL type error"),
        };
        message.push_str(
            ", which makes a FILTER drop the row, even under ! (BIND leaves the variable unbound)",
        );
        if !matches!(self.kind, ArgKind::Unbound | ArgKind::DivByZero) && !self.sample.is_empty() {
            message.push_str(&format!("; e.g. {}", self.sample));
        }
        message.push_str(&format!(". Hint: {hint}."));
        QueryWarning {
            severity: WarningSeverity::TypeError,
            function: name.to_string(),
            argument: self.pos,
            kind: self.kind.as_str().to_string(),
            count: self.count,
            sample: (!self.sample.is_empty()).then_some(self.sample),
            hint,
            message,
        }
    }
}

fn is_empty_result(out: &QueryOutput) -> bool {
    match out {
        QueryOutput::Select(_, rows) => rows.is_empty(),
        QueryOutput::Ask(b) => !*b,
        QueryOutput::Construct(t) => t.is_empty(),
    }
}

/// Close the scope opened by [`begin`]: turn the sink into warnings for `out`.
pub(crate) fn finish(out: &QueryOutput) -> Vec<QueryWarning> {
    let (entries, misses) = SINK.with(|s| {
        let mut s = s.borrow_mut();
        (
            std::mem::take(&mut s.entries),
            std::mem::take(&mut s.misses),
        )
    });
    let errored = !entries.is_empty();
    let mut out_w: Vec<QueryWarning> = entries.into_iter().map(Entry::into_warning).collect();
    // Case-sensitivity hint: only for an empty result, only when no type
    // error offers a better explanation, only for a constant needle that has
    // letters with case, and only when the predicate actually ran on string
    // values and said no. It is a suggestion, never an error.
    if !errored && is_empty_result(out) {
        let mut seen: Vec<(Builtin, String)> = Vec::new();
        for (func, needle) in misses {
            if needle.to_lowercase() == needle.to_uppercase()
                || seen.iter().any(|(f, n)| *f == func && *n == needle)
            {
                continue;
            }
            seen.push((func, needle.clone()));
            let name = fn_name(func);
            let hint = if func == Builtin::Regex {
                format!("add the \"i\" flag: REGEX(?x, \"{needle}\", \"i\")")
            } else {
                format!(
                    "compare case-folded: {name}(LCASE(?x), \"{}\")",
                    needle.to_lowercase()
                )
            };
            out_w.push(QueryWarning {
                severity: WarningSeverity::Hint,
                function: name.to_string(),
                argument: 2,
                kind: "case-sensitive".to_string(),
                count: 0,
                sample: Some(truncate(&needle)),
                message: format!(
                    "No results. {name} ran on string values but never matched \"{}\"; \
                     string matching is case-sensitive. If you expected matches, {hint}.",
                    truncate(&needle)
                ),
                hint,
            });
        }
    }
    out_w
}

/// The warnings as a JSON array (`[]` when empty), in the field order of
/// [`QueryWarning`] with `argKind` for `kind`.
pub fn warnings_json(warnings: &[QueryWarning]) -> String {
    use crate::results::push_json_string;
    let mut s = String::from("[");
    for (i, w) in warnings.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str(r#"{"severity":"#);
        push_json_string(&mut s, w.severity.as_str());
        s.push_str(r#","function":"#);
        push_json_string(&mut s, &w.function);
        s.push_str(&format!(r#","argument":{}"#, w.argument));
        s.push_str(r#","argKind":"#);
        push_json_string(&mut s, &w.kind);
        s.push_str(&format!(r#","count":{}"#, w.count));
        s.push_str(r#","sample":"#);
        match &w.sample {
            Some(v) => push_json_string(&mut s, v),
            None => s.push_str("null"),
        }
        s.push_str(r#","hint":"#);
        push_json_string(&mut s, &w.hint);
        s.push_str(r#","message":"#);
        push_json_string(&mut s, &w.message);
        s.push('}');
    }
    s.push(']');
    s
}
