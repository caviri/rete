//! SPARQL 1.2 triple terms on the pinned SPARQL-star parser.
//!
//! `spargebra` 0.3 (rete's parser) reads the SPARQL-star community-group
//! syntax: `<< s p o >>` is a quoted triple, and `<<( s p o )>>` does not
//! parse. SPARQL 1.2 changed both: `<<( s p o )>>` is a **triple term**, and
//! `<< s p o >>` is a **reified triple** — a reifier `?r` with
//! `?r rdf:reifies <<( s p o )>>`. rete stores a triple term and a quoted
//! triple as the same token (`<<s p o>>`), so the data side already reads and
//! writes both spellings; this module brings the query side level with it by
//! rewriting the query TEXT before it is parsed, outside strings, IRIs and
//! comments:
//!
//! - `<<( s p o )>>` → `<< s p o >>`, always: a triple-term pattern, matching
//!   the stored token. Valid SPARQL 1.1 + star queries never contain `<<(`.
//! - Under a leading `VERSION "1.2…"` declaration, `<< s p o >>` →
//!   `[ rdf:reifies << s p o >> ]`: a fresh blank node (a non-distinguished
//!   variable) that reifies the triple term, which is exactly SPARQL 1.2's
//!   expansion. The blank-node property list is legal wherever 1.2 allows a
//!   reified triple (subject or object of a triple pattern, a path end, a
//!   CONSTRUCT template) and is a parse error where 1.2 forbids one (an
//!   expression, `VALUES`), as it should be.
//! - Without the declaration, `<< s p o >>` keeps its SPARQL-star meaning (a
//!   quoted-triple pattern), so existing queries — the starter queries every
//!   published card carries — answer as before.
//!
//! Two SPARQL 1.2 forms are refused by name under the declaration rather than
//! read in their SPARQL-star sense: a named reifier `<< s p o ~ ?r >>` and the
//! annotation syntax `s p o {| … |}`. Write them out as
//! `?r rdf:reifies <<( s p o )>>`.

use std::borrow::Cow;

use super::SparqlError;

const RDF_REIFIES: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies>";

/// The value of a leading `VERSION "…"` declaration (after leading whitespace
/// and `#` comments), if the query has one.
pub(super) fn version(query: &str) -> Option<&str> {
    let mut rest = query;
    loop {
        let t = rest.trim_start();
        if let Some(after_hash) = t.strip_prefix('#') {
            rest = after_hash.split_once('\n')?.1;
            continue;
        }
        if t.len() < 7 || !t[..7].eq_ignore_ascii_case("VERSION") {
            return None;
        }
        let after = t[7..].trim_start();
        let q = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
        let body = &after[1..];
        return body.find(q).map(|end| &body[..end]);
    }
}

/// Whether a `VERSION` value selects the SPARQL 1.2 meaning of `<< … >>`.
pub(super) fn is_sparql12(version: Option<&str>) -> bool {
    version.is_some_and(|v| v.starts_with("1.2"))
}

/// One open `<<` construct while scanning.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Open {
    /// `<<(` — a triple term.
    TripleTerm,
    /// `<<` — a quoted triple (SPARQL-star) or a reified triple (1.2).
    Quoted,
}

/// Rewrite `query` (the text after any `VERSION` declaration) as described in
/// the module docs. Borrowed when there is nothing to rewrite.
pub(super) fn rewrite(query: &str, sparql12: bool) -> Result<Cow<'_, str>, SparqlError> {
    let rewrites = if sparql12 {
        query.contains("<<") || query.contains("{|")
    } else {
        query.contains("<<(")
    };
    if !rewrites {
        return Ok(Cow::Borrowed(query));
    }
    let b = query.as_bytes();
    let mut out = String::with_capacity(query.len() + 64);
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 0;
    // Copy `query[from..to]` verbatim.
    let copy = |out: &mut String, from: usize, to: usize| out.push_str(&query[from..to]);
    while i < b.len() {
        match b[i] {
            b'#' => {
                let end = query[i..].find('\n').map_or(b.len(), |n| i + n);
                copy(&mut out, i, end);
                i = end;
            }
            b'"' | b'\'' => {
                let end = string_end(b, i);
                copy(&mut out, i, end);
                i = end;
            }
            b'\\' => {
                // A local-name escape (`ex:a\~b`): the escaped char is literal.
                let end = (i + 1 + utf8_len(b.get(i + 1).copied())).min(b.len());
                copy(&mut out, i, end);
                i = end;
            }
            b'<' if b.get(i + 1) == Some(&b'<') => {
                if b.get(i + 2) == Some(&b'(') {
                    stack.push(Open::TripleTerm);
                    out.push_str("<<");
                    i += 3;
                } else {
                    stack.push(Open::Quoted);
                    out.push_str(if sparql12 {
                        // A reified triple: a fresh reifier of the triple term.
                        "[ "
                    } else {
                        "<<"
                    });
                    if sparql12 {
                        out.push_str(RDF_REIFIES);
                        out.push_str(" <<");
                    }
                    i += 2;
                }
            }
            b'<' => {
                // An IRI, or the `<` operator.
                let end = iri_end(b, i);
                copy(&mut out, i, end);
                i = end;
            }
            b')' if b.get(i + 1) == Some(&b'>')
                && b.get(i + 2) == Some(&b'>')
                && stack.last() == Some(&Open::TripleTerm) =>
            {
                stack.pop();
                out.push_str(">>");
                i += 3;
            }
            b'>' if b.get(i + 1) == Some(&b'>') && stack.last() == Some(&Open::Quoted) => {
                stack.pop();
                out.push_str(if sparql12 { ">> ]" } else { ">>" });
                i += 2;
            }
            b'~' if sparql12 && stack.last() == Some(&Open::Quoted) => {
                return Err(SparqlError::Unsupported(
                    "a named reifier `<< s p o ~ ?r >>` under VERSION \"1.2\"; \
                     write `?r rdf:reifies <<( s p o )>>`",
                ));
            }
            b'{' if sparql12 && b.get(i + 1) == Some(&b'|') => {
                return Err(SparqlError::Unsupported(
                    "the annotation syntax `{| … |}` under VERSION \"1.2\"; \
                     write `?r rdf:reifies <<( s p o )>>`",
                ));
            }
            _ => {
                let n = utf8_len(Some(b[i]));
                copy(&mut out, i, (i + n).min(b.len()));
                i += n;
            }
        }
    }
    Ok(Cow::Owned(out))
}

/// The byte length of the UTF-8 sequence led by `lead` (1 for ASCII).
fn utf8_len(lead: Option<u8>) -> usize {
    match lead {
        Some(c) if c >= 0xF0 => 4,
        Some(c) if c >= 0xE0 => 3,
        Some(c) if c >= 0xC0 => 2,
        _ => 1,
    }
}

/// The end (exclusive) of the string literal opening at `start` (`"`, `'`, or a
/// triple-quoted long form), honoring `\` escapes. An unterminated string runs
/// to the end; the parser reports it.
fn string_end(b: &[u8], start: usize) -> usize {
    let q = b[start];
    let long = b.get(start + 1) == Some(&q) && b.get(start + 2) == Some(&q);
    let mut i = start + if long { 3 } else { 1 };
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == q => {
                if !long {
                    return i + 1;
                }
                if b.get(i + 1) == Some(&q) && b.get(i + 2) == Some(&q) {
                    return i + 3;
                }
                i += 1;
            }
            b'\n' | b'\r' if !long => return i,
            _ => i += 1,
        }
    }
    b.len()
}

/// The end (exclusive) of an IRI reference opening at `start` (`<…>`), or
/// `start + 1` when the `<` opens no IRI (the less-than operator).
fn iri_end(b: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'>' => return i + 1,
            b'<' | b'"' | b'{' | b'}' | b'|' | b'^' | b'`' | b'\\' => return start + 1,
            c if c <= b' ' => return start + 1,
            _ => i += 1,
        }
    }
    start + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(q: &str, v12: bool) -> String {
        rewrite(q, v12).unwrap().into_owned()
    }

    #[test]
    fn triple_terms_become_quoted_triples_in_either_mode() {
        let q = "SELECT * WHERE { ?s <http://ex/p> <<( ?a ?b \"x)>>\" )>> }";
        let want = "SELECT * WHERE { ?s <http://ex/p> << ?a ?b \"x)>>\" >> }";
        assert_eq!(rw(q, false), want);
        assert_eq!(rw(q, true), want);
        // Nested.
        assert_eq!(
            rw("<<( <<( :a :b :c )>> :d :e )>>", false),
            "<< << :a :b :c >> :d :e >>"
        );
    }

    #[test]
    fn quoted_triples_keep_their_meaning_without_the_declaration() {
        let q = "SELECT * WHERE { << ?s ?p ?o >> <http://ex/source> ?src }";
        assert!(matches!(rewrite(q, false).unwrap(), Cow::Borrowed(_)));
    }

    #[test]
    fn under_sparql12_a_quoted_triple_is_a_reifier() {
        assert_eq!(
            rw("{ << :a :b ?o >> :q ?v . ?x :r << :c :d <<( :e :f :g )>> >> }", true),
            format!(
                "{{ [ {RDF_REIFIES} << :a :b ?o >> ] :q ?v . ?x :r [ {RDF_REIFIES} << :c :d << :e :f :g >> >> ] }}"
            )
        );
    }

    #[test]
    fn strings_iris_comments_and_operators_are_left_alone() {
        let q = "# << not a triple\nSELECT * WHERE { ?s ?p \"<<x>>\" , '''a <<( b'''. \
                 FILTER(?a < ?b && ?c<<http://ex/x>) }";
        // The `<<` inside the FILTER is SPARQL-star syntax for a quoted triple
        // opener only if followed by a term; here it is `<` then an IRI.
        let out = rw(q, false);
        assert!(out.starts_with("# << not a triple\n"));
        assert!(out.contains("\"<<x>>\""));
        assert!(out.contains("'''a <<( b'''"));
    }

    #[test]
    fn named_reifiers_and_annotations_are_refused_under_sparql12() {
        assert!(rewrite("{ << :a :b :c ~ ?r >> :q ?v }", true).is_err());
        assert!(rewrite("{ :a :b :c {| :q ?v |} }", true).is_err());
        // Without the declaration the annotation is SPARQL-star's, untouched.
        assert!(rewrite("{ :a :b :c {| :q ?v |} }", false).is_ok());
        // A `~` in a local-name escape is not a reifier.
        assert!(rewrite("{ << :a :b ex:c\\~d >> :q ?v }", true).is_ok());
    }

    #[test]
    fn version_reads_the_leading_declaration() {
        assert_eq!(version("VERSION \"1.2\" SELECT * {}"), Some("1.2"));
        assert_eq!(
            version("# c\n  version '1.2-basic'\nASK {}"),
            Some("1.2-basic")
        );
        assert_eq!(version("SELECT * { ?s ?p \"VERSION\" }"), None);
        assert!(is_sparql12(Some("1.2")) && !is_sparql12(Some("1.1")) && !is_sparql12(None));
    }
}
