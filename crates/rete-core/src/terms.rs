//! Term identifiers and N-Triples term-token helpers.
//!
//! Two distinct things live in one place here because they describe the same
//! domain — "what is a term, and how is it identified":
//!
//! 1. **ID aliases.** Every term in a `.rete` file is interned into the
//!    dictionary and addressed by a `u32`. Code passes these `u32`s through
//!    many signatures where the bare type says nothing about *which* id space a
//!    value lives in. The aliases below ([`NodeId`], [`SubjectId`],
//!    [`PredicateId`], [`ObjectId`]) are documentation: they are all `u32`
//!    today (so they cost nothing and mix freely), but they let a signature
//!    state its intent — `subject_node(sid: SubjectId) -> NodeId` reads as the
//!    role-id → unified-node mapping it is. A later pass can promote them to
//!    true newtypes (`struct NodeId(u32)`) without touching call sites that
//!    already name the alias.
//!
//! 2. **Term-token helpers.** A [`TermToken`] is the textual form of a term as
//!    it appears in N-Triples and in the dictionary: an IRI `<http://…>`, a
//!    blank node `_:b0`, or a literal `"text"`, `"text"@en`, `"text"^^<dt>`.
//!    Several modules (SPARQL evaluation, SHACL validation, doc rendering)
//!    independently grew the same little parsers for "is this an IRI", "what's
//!    the lexical value", "what's the datatype". They are consolidated here so
//!    there is one definition of the term grammar to reason about.

use std::borrow::Cow;

/// A dictionary id in the **unified node space** — the single id space that
/// covers every term that ever appears as a subject or an object. This is the
/// id reachability, the community pyramid, and the graph index work in.
pub type NodeId = u32;

/// A dictionary id in the **subject** id space (terms seen in subject
/// position). Map to a [`NodeId`] with [`Dictionary::subject_node`].
///
/// [`Dictionary::subject_node`]: crate::dictionary::Dictionary::subject_node
pub type SubjectId = u32;

/// A dictionary id in the **predicate** id space. Predicates have their own
/// dense id space and are never part of the unified node space.
pub type PredicateId = u32;

/// A dictionary id in the **object** id space (terms seen in object position).
/// Map to a [`NodeId`] with [`Dictionary::object_node`].
///
/// [`Dictionary::object_node`]: crate::dictionary::Dictionary::object_node
pub type ObjectId = u32;

/// The textual form of an RDF term as stored in the dictionary and emitted in
/// N-Triples: an IRI (`<…>`), a blank node (`_:…`), or a literal (`"…"`,
/// optionally with an `@lang` or `^^<datatype>` suffix). An alias for `str`;
/// it names intent at API boundaries that take a term rather than arbitrary
/// text.
pub type TermToken = str;

const XSD_STRING: &str = "http://www.w3.org/2001/XMLSchema#string";
const RDF_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#langString";

/// Is `t` an IRI term (`<…>`)?
#[inline]
pub fn is_iri(t: &TermToken) -> bool {
    // A quoted triple (`<< … >>`, RDF-star) also starts with `<` and ends with
    // `>`, so exclude it explicitly — it is its own term kind, not an IRI.
    t.starts_with('<') && !t.starts_with("<<") && t.ends_with('>')
}

/// Is `t` a **quoted triple** term (`<< s p o >>`, RDF-star)? These appear only
/// in subject/object position and are stored in the dictionary as their
/// canonical N-Triples-star surface, exactly like any other term.
#[inline]
pub fn is_quoted_triple(t: &TermToken) -> bool {
    t.starts_with("<<") && t.ends_with(">>")
}

/// The content of an IRI term without its angle brackets (`<http://x>` →
/// `http://x`), or `None` if `t` is not an IRI term.
#[inline]
pub fn iri_content(t: &TermToken) -> Option<&str> {
    if is_quoted_triple(t) {
        return None;
    }
    t.strip_prefix('<').and_then(|s| s.strip_suffix('>'))
}

/// Is `t` a blank-node term (`_:…`)?
#[inline]
pub fn is_blank(t: &TermToken) -> bool {
    t.starts_with("_:")
}

/// Is `t` a literal term (`"…"`)?
#[inline]
pub fn is_literal(t: &TermToken) -> bool {
    t.starts_with('"')
}

/// Index of the closing quote of a literal term, scanning from the opening
/// quote and honoring `\"` escapes. `t` must start with `"`.
fn closing_quote(t: &TermToken) -> usize {
    let bytes = t.as_bytes();
    let mut i = 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'"' => break,
            _ => i += 1,
        }
    }
    i.min(t.len())
}

/// The **lexical value** of a literal term — the text between the quotes with
/// N-Triples escapes resolved — or `None` for IRIs and blank nodes. The
/// datatype and language suffix are dropped (`"42"^^<…int>` → `42`,
/// `"hi"@en` → `hi`).
pub fn literal_lexical(token: &TermToken) -> Option<String> {
    if !is_literal(token) {
        return None;
    }
    Some(unescape_literal(&token[1..closing_quote(token)]))
}

/// The **lexical value** of any term: a literal's unescaped body, an IRI's
/// content, or a blank-node token unchanged. Always succeeds. Useful where a
/// plain comparable string is wanted regardless of term kind.
pub fn lexical(token: &TermToken) -> Cow<'_, str> {
    if is_literal(token) {
        Cow::Owned(unescape_literal(&token[1..closing_quote(token)]))
    } else if let Some(iri) = iri_content(token) {
        Cow::Borrowed(iri)
    } else {
        Cow::Borrowed(token)
    }
}

/// The part of a literal term after its closing quote (`"x"^^<dt>` → `^^<dt>`,
/// `"x"@en` → `@en`, `"x"` → ``), or `None` if `token` is not a literal.
fn literal_suffix(token: &TermToken) -> Option<&str> {
    if !is_literal(token) {
        return None;
    }
    token.get(closing_quote(token) + 1..)
}

/// The datatype IRI **content** of a literal term (no angle brackets): the
/// explicit `^^<dt>`, else `rdf:langString` for a language-tagged literal,
/// else `xsd:string` for a plain one. `None` for a non-literal or a malformed
/// suffix.
pub fn literal_datatype(token: &TermToken) -> Option<String> {
    let suffix = literal_suffix(token)?;
    if let Some(dt) = suffix.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
        Some(dt.to_string())
    } else if suffix.starts_with('@') {
        Some(RDF_LANG_STRING.to_string())
    } else if suffix.is_empty() {
        Some(XSD_STRING.to_string())
    } else {
        None
    }
}

/// The language tag of a literal term (`"hi"@en` → `en`), `""` when the literal
/// is untagged, or `None` for a non-literal.
pub fn lang_tag(token: &TermToken) -> Option<String> {
    literal_suffix(token).map(|s| s.strip_prefix('@').unwrap_or("").to_string())
}

/// Numeric value of a term: the lexical part of a literal parsed as `f64`
/// (`"30"^^<…int>` → `30.0`) or a bare numeric token, else `None`.
pub fn as_number(token: &TermToken) -> Option<f64> {
    let lex = if let Some(rest) = token.strip_prefix('"') {
        &rest[..rest.find('"')?]
    } else {
        token
    };
    lex.parse::<f64>().ok()
}

/// Escape a string for use as the body of an N-Triples literal (`"…"`): the
/// inverse of [`unescape_literal`] for the characters that must be escaped
/// (`\`, `"`, newline, carriage return, tab). The common case (no special
/// characters) returns the input untouched.
pub fn escape_literal(s: &str) -> String {
    if !s.contains(['\\', '"', '\n', '\r', '\t']) {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            _ => out.push(c),
        }
    }
    out
}

/// Build a literal term token from a (raw, unescaped) lexical value, attaching
/// an optional non-empty language tag (`@lang`) or datatype IRI content
/// (`^^<dt>`). `lang` wins over `datatype` if both are given (a tagged literal
/// is implicitly `rdf:langString`).
pub fn make_literal(lexical: &str, lang: Option<&str>, datatype: Option<&str>) -> String {
    let body = escape_literal(lexical);
    match (lang.filter(|l| !l.is_empty()), datatype) {
        (Some(l), _) => format!("\"{body}\"@{l}"),
        (None, Some(dt)) => format!("\"{body}\"^^<{dt}>"),
        (None, None) => format!("\"{body}\""),
    }
}

/// Resolve the N-Triples escape sequences in a literal's body to actual chars
/// (`\n`, `\t`, `\"`, `\\`, `\uXXXX`, `\UXXXXXXXX`, …). Strings without a
/// backslash — the overwhelming majority — are returned untouched.
pub fn unescape_literal(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let unicode = |chars: &mut std::str::Chars, n: usize, out: &mut String| {
            let hex: String = chars.take(n).collect();
            match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                Some(ch) => out.push(ch),
                None => out.push('\u{FFFD}'),
            }
        };
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('b') => out.push('\u{08}'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{0C}'),
            Some('"') => out.push('"'),
            Some('\'') => out.push('\''),
            Some('\\') => out.push('\\'),
            Some('u') => unicode(&mut chars, 4, &mut out),
            Some('U') => unicode(&mut chars, 8, &mut out),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn term_kinds() {
        assert!(is_iri("<http://example.org/x>"));
        assert!(!is_iri("\"x\""));
        assert!(!is_iri("_:b0"));
        assert!(is_blank("_:b0"));
        assert!(is_literal("\"x\"@en"));
        assert_eq!(iri_content("<http://x>"), Some("http://x"));
        assert_eq!(iri_content("\"x\""), None);
    }

    #[test]
    fn lexical_values() {
        assert_eq!(literal_lexical("\"hello\""), Some("hello".to_string()));
        assert_eq!(literal_lexical("\"42\"^^<int>"), Some("42".to_string()));
        assert_eq!(literal_lexical("\"hi\"@en"), Some("hi".to_string()));
        assert_eq!(literal_lexical("<http://x>"), None);
        // any-term lexical
        assert_eq!(lexical("\"hi\"@en"), "hi");
        assert_eq!(lexical("<http://x>"), "http://x");
        assert_eq!(lexical("_:b0"), "_:b0");
    }

    #[test]
    fn datatype_and_lang() {
        assert_eq!(literal_datatype("\"42\"^^<int>").as_deref(), Some("int"));
        assert_eq!(
            literal_datatype("\"hi\"@en").as_deref(),
            Some(RDF_LANG_STRING)
        );
        assert_eq!(literal_datatype("\"plain\"").as_deref(), Some(XSD_STRING));
        assert_eq!(literal_datatype("<http://x>"), None);
        assert_eq!(lang_tag("\"hi\"@en").as_deref(), Some("en"));
        assert_eq!(lang_tag("\"plain\"").as_deref(), Some(""));
        assert_eq!(lang_tag("<http://x>"), None);
    }

    #[test]
    fn numbers() {
        assert_eq!(as_number("\"30\"^^<int>"), Some(30.0));
        assert_eq!(as_number("3.5"), Some(3.5));
        assert_eq!(as_number("\"nope\""), None);
        assert_eq!(as_number("<http://x>"), None);
    }

    #[test]
    fn escapes() {
        assert_eq!(unescape_literal("plain"), "plain");
        assert_eq!(unescape_literal("a\\nb"), "a\nb");
        assert_eq!(unescape_literal("a\\\"b"), "a\"b");
        assert_eq!(unescape_literal("\\u0041"), "A");
        // an escaped quote inside the body is honored by the closing-quote scan
        assert_eq!(literal_lexical("\"a\\\"b\"@en"), Some("a\"b".to_string()));
    }
}
