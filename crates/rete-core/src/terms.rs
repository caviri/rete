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
/// RDF 1.2 base-direction language string: `"…"@lang--dir` (dir = `ltr`/`rtl`).
const RDF_DIR_LANG_STRING: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#dirLangString";

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
        // RDF 1.2: a language string WITH a base direction (`@lang--dir`) is an
        // `rdf:dirLangString`; a plain `@lang` is `rdf:langString`. `--` never
        // occurs in a well-formed BCP-47 tag, so this is unambiguous.
        if suffix.contains("--") {
            Some(RDF_DIR_LANG_STRING.to_string())
        } else {
            Some(RDF_LANG_STRING.to_string())
        }
    } else if suffix.is_empty() {
        Some(XSD_STRING.to_string())
    } else {
        None
    }
}

/// The language tag of a literal term (`"hi"@en` → `en`), `""` when the literal
/// is untagged, or `None` for a non-literal. For an RDF 1.2 directional string
/// (`"x"@ar--rtl`) this is the LANGUAGE only (`ar`) — the base direction is
/// separate (see [`lang_dir`]), matching SPARQL 1.2 `LANG`.
pub fn lang_tag(token: &TermToken) -> Option<String> {
    literal_suffix(token).map(|s| {
        s.strip_prefix('@')
            .unwrap_or("")
            .split("--")
            .next()
            .unwrap_or("")
            .to_string()
    })
}

/// The base direction of an RDF 1.2 directional language string (`"x"@ar--rtl` →
/// `rtl`), or `None` if the literal has no direction (or is not a literal).
pub fn lang_dir(token: &TermToken) -> Option<String> {
    let tag = literal_suffix(token)?.strip_prefix('@')?;
    tag.split("--").nth(1).map(str::to_string)
}

/// Numeric value of a term: the lexical part of a literal parsed as `f64`
/// (`"30"^^<…int>` → `30.0`) or a bare numeric token, else `None`.
///
/// The literal's lexical value is taken through the escape-aware
/// [`literal_lexical`] — closing quote located correctly, body unescaped, any
/// `^^<datatype>` suffix dropped — rather than scanning to the *first* `"`. A
/// value containing an embedded escaped quote (`"1\"2"`) is therefore read
/// whole and simply fails to parse, instead of being truncated at the escaped
/// quote. A language-tagged literal (`"5"@en`) is never a numeric literal in
/// SPARQL/RDF, so it yields `None`.
pub fn as_number(token: &TermToken) -> Option<f64> {
    if is_literal(token) {
        if lang_tag(token).is_some_and(|tag| !tag.is_empty()) {
            return None;
        }
        literal_lexical(token)?.parse::<f64>().ok()
    } else {
        token.parse::<f64>().ok()
    }
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

/// Rewrite a term token into the **RDF 1.2 triple-term surface** for a text
/// serializer.
///
/// rete stores a quoted triple in one canonical token, the RDF-star surface
/// `<<s p o>>` — see `ingest::take_term`, which accepts both surfaces
/// on ingest and canonicalises them to that one. Current RDF 1.2 parsers
/// (oxttl 0.2 and anything built on it, including the `oxigraph` CLI) do not
/// read that surface: in N-Triples/N-Quads they **reject** it outright, and in
/// Turtle/TriG they read `<< s p o >>` as a *reifier* — one statement silently
/// becomes two, with a blank node where the triple term was. So a dump in the
/// stored surface is not interoperable, quietly in one format and loudly in the
/// other. This is the translation that makes it so, at write time only: nothing
/// about the file changes.
///
/// Returns:
///
/// * `Some(Borrowed(token))` when `token` is not a quoted triple. This is the
///   overwhelming majority of terms and the only cost is a two-byte prefix
///   check, so a dump with no quoted triples in it is byte-for-byte unchanged.
/// * `Some(Owned(…))` with the token rewritten to `<<( s p o )>>`, recursively:
///   a triple term nested in the object slot is rewritten too.
/// * `None` when the token has **no RDF 1.2 spelling at all**. RDF 1.2 puts a
///   triple term in *object position only* — the grammar is
///   `tripleTerm ::= '<<(' ttSubject predicate ttObject ')>>'` with
///   `ttSubject ::= iri | BlankNode` — so a quoted triple standing in the
///   subject slot of another quoted triple cannot be written. (The caller is
///   responsible for the same rule at statement level: a quoted triple in the
///   *statement's* subject or predicate slot is equally unwritable, and the
///   caller is the one that knows which slot a term came from.)
///
/// A token that is not well-formed — `<<` … `>>` that does not parse as three
/// terms — also yields `None` rather than a mangled rewrite.
pub fn rdf12_triple_term(token: &TermToken) -> Option<Cow<'_, TermToken>> {
    if !is_quoted_triple(token) {
        return Some(Cow::Borrowed(token));
    }
    rewrite_rdf12(token).map(Cow::Owned)
}

/// The owned half of [`rdf12_triple_term`], split out so the recursion does not
/// re-run the `is_quoted_triple` fast path on a token it already classified.
fn rewrite_rdf12(token: &TermToken) -> Option<String> {
    let (s, p, o) = crate::ingest::quoted_triple_parts(token)?;
    // `ttSubject ::= iri | BlankNode` and `predicate ::= iri`: neither slot
    // admits a triple term, at any depth.
    if is_quoted_triple(&s) || is_quoted_triple(&p) {
        return None;
    }
    // `ttObject` does admit one, which is where nesting lives.
    let o = if is_quoted_triple(&o) {
        rewrite_rdf12(&o)?
    } else {
        o
    };
    Some(format!("<<( {s} {p} {o} )>>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- the RDF 1.2 writer surface ----------------------------------------

    #[test]
    fn a_plain_term_is_borrowed_unchanged() {
        // The hot path. Every term that is not a quoted triple comes back
        // borrowed, which is what makes a dump of a quoted-triple-free file
        // byte-for-byte what it was.
        for t in [
            "<http://example.org/x>",
            "_:b0",
            "\"lit\"@en",
            "\"5\"^^<http://www.w3.org/2001/XMLSchema#integer>",
            "\"a > b\"",
        ] {
            match rdf12_triple_term(t) {
                Some(Cow::Borrowed(got)) => assert_eq!(got, t),
                other => panic!("{t} should borrow unchanged, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_object_triple_term_gets_the_rdf12_surface() {
        assert_eq!(
            rdf12_triple_term("<<<http://ex/s> <http://ex/p> <http://ex/o>>>").unwrap(),
            "<<( <http://ex/s> <http://ex/p> <http://ex/o> )>>"
        );
    }

    #[test]
    fn nesting_in_the_object_slot_recurses() {
        // `ttObject` admits another triple term, so depth works — and the
        // recursion has to rewrite the inner one too, not just the outer.
        assert_eq!(
            rdf12_triple_term(
                "<<<http://ex/a> <http://ex/b> <<<http://ex/x> <http://ex/y> <http://ex/z>>>>>"
            )
            .unwrap(),
            "<<( <http://ex/a> <http://ex/b> <<( <http://ex/x> <http://ex/y> <http://ex/z> )>> )>>"
        );
    }

    #[test]
    fn a_literal_object_survives_verbatim() {
        // Term boundaries come from `take_term`, not from splitting on spaces,
        // so a literal carrying spaces, a `>` and a `<<` does not derail it.
        assert_eq!(
            rdf12_triple_term("<<_:b1 <http://ex/p> \"a > b << c\"@en>>").unwrap(),
            "<<( _:b1 <http://ex/p> \"a > b << c\"@en )>>"
        );
    }

    #[test]
    fn a_triple_term_in_a_subject_slot_has_no_rdf12_spelling() {
        // RDF 1.2: `ttSubject ::= iri | BlankNode`. A quoted triple nested in
        // another one's subject cannot be written, at any depth, and the honest
        // answer is `None` rather than a token no parser accepts.
        assert!(rdf12_triple_term(
            "<<<<<http://ex/x> <http://ex/y> <http://ex/z>>> <http://ex/p> <http://ex/o>>>"
        )
        .is_none());
        // …including one level down.
        assert!(rdf12_triple_term(
            "<<<http://ex/a> <http://ex/b> <<<<<http://ex/x> <http://ex/y> <http://ex/z>>> <http://ex/p> <http://ex/o>>>>>"
        )
        .is_none());
    }

    #[test]
    fn a_malformed_quoted_triple_is_refused_not_mangled() {
        assert!(rdf12_triple_term("<<<http://ex/s> <http://ex/p>>>").is_none());
        assert!(rdf12_triple_term("<<>>").is_none());
    }

    #[test]
    fn the_rewrite_is_what_ingest_accepts_back() {
        // The round-trip property, at the term level: what the writer emits is
        // what `take_term` canonicalises back to the stored token. This is why
        // rete -> nq -> rete is safe in either surface.
        let stored =
            "<<<http://ex/a> <http://ex/b> <<<http://ex/x> <http://ex/y> <http://ex/z>>>>>";
        let written = rdf12_triple_term(stored).unwrap().into_owned();
        let (back, rest) = crate::ingest::take_term(&written).unwrap();
        assert_eq!(back, stored);
        assert!(rest.trim().is_empty());
    }

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
    fn as_number_escape_aware() {
        // Plain and typed numeric literals parse as before.
        assert_eq!(
            as_number("\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            Some(42.0)
        );
        assert_eq!(
            as_number("\"12.5\"^^<http://www.w3.org/2001/XMLSchema#decimal>"),
            Some(12.5)
        );
        assert_eq!(
            as_number("\"6.022e23\"^^<http://www.w3.org/2001/XMLSchema#double>"),
            Some(6.022e23)
        );
        // Plain literal, negative, and leading-`+`.
        assert_eq!(as_number("\"5\""), Some(5.0));
        assert_eq!(as_number("\"-5\"^^<int>"), Some(-5.0));
        assert_eq!(as_number("\"+7\""), Some(7.0));
        // Non-numeric literal → None.
        assert_eq!(as_number("\"not a number\""), None);
        // A value with an EMBEDDED escaped quote must be read whole (`1"2`),
        // fail to parse, and never be truncated to `1` (or panic). This is the
        // escape-aware path: the old first-`"` scan would have stopped early.
        assert_eq!(as_number("\"1\\\"2\""), None);
        // IRI and blank node → None.
        assert_eq!(as_number("<http://example.org/n>"), None);
        assert_eq!(as_number("_:b0"), None);
        // A language-tagged literal is never numeric (SPARQL): `"5"@en` → None.
        assert_eq!(as_number("\"5\"@en"), None);
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
