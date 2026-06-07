//! A minimal N-Triples reader for `rete build`.
//!
//! N-Triples is line-based: `subject predicate object .`. Terms are kept as
//! their exact canonical token strings (`<iri>`, `_:bnode`, `"lit"`,
//! `"lit"^^<dt>`, `"lit"@lang`) so they double as dictionary keys and so a
//! `query` can match by the same string. This is not a full RDF 1.1 parser —
//! it covers the canonical N-Triples surface, enough for v0 ingestion.

/// A parsed triple as three canonical term tokens.
pub type RawTriple = (String, String, String);

#[derive(Debug, thiserror::Error)]
pub enum NtError {
    #[error("line {0}: {1}")]
    Line(usize, &'static str),
}

/// A parsed quad: the triple plus an optional graph term (`None` = default graph).
pub type RawQuad = (String, String, String, Option<String>);

/// Parse N-Quads text: `subject predicate object [graph] .` per line.
pub fn parse_quads(input: &str) -> Result<Vec<RawQuad>, NtError> {
    let mut out = Vec::new();
    for (i, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stripped = line
            .strip_suffix('.')
            .ok_or(NtError::Line(i + 1, "missing trailing '.'"))?
            .trim_end();
        let (s, rest) = take_term(stripped).ok_or(NtError::Line(i + 1, "bad subject"))?;
        let (p, rest) =
            take_term(rest.trim_start()).ok_or(NtError::Line(i + 1, "bad predicate"))?;
        let (o, rest) = take_term(rest.trim_start()).ok_or(NtError::Line(i + 1, "bad object"))?;
        let rest = rest.trim();
        let graph = if rest.is_empty() {
            None
        } else {
            let (g, tail) = take_term(rest).ok_or(NtError::Line(i + 1, "bad graph"))?;
            if !tail.trim().is_empty() {
                return Err(NtError::Line(i + 1, "trailing content after graph"));
            }
            Some(g)
        };
        out.push((s, p, o, graph));
    }
    Ok(out)
}

/// Parse N-Triples text into raw term-token triples, skipping blank/comment lines.
pub fn parse(input: &str) -> Result<Vec<RawTriple>, NtError> {
    let mut out = Vec::new();
    for (i, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stripped = line
            .strip_suffix('.')
            .ok_or(NtError::Line(i + 1, "missing trailing '.'"))?
            .trim_end();
        let (s, rest) = take_term(stripped).ok_or(NtError::Line(i + 1, "bad subject"))?;
        let (p, rest) =
            take_term(rest.trim_start()).ok_or(NtError::Line(i + 1, "bad predicate"))?;
        let (o, rest) = take_term(rest.trim_start()).ok_or(NtError::Line(i + 1, "bad object"))?;
        if !rest.trim().is_empty() {
            return Err(NtError::Line(i + 1, "trailing content after object"));
        }
        out.push((s, p, o));
    }
    Ok(out)
}

/// Take one term from the front of `s`, returning `(term, remainder)`.
fn take_term(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    let first = *bytes.first()?;
    match first {
        b'<' => {
            // IRI ref: up to the closing '>'.
            let end = s.find('>')?;
            Some((s[..=end].to_string(), &s[end + 1..]))
        }
        b'_' => {
            // Blank node: up to whitespace.
            let end = s.find(char::is_whitespace).unwrap_or(s.len());
            Some((s[..end].to_string(), &s[end..]))
        }
        b'"' => {
            // Literal: closing unescaped quote, then optional ^^<dt> or @lang.
            let mut i = 1;
            let b = s.as_bytes();
            while i < b.len() {
                match b[i] {
                    b'\\' => i += 2, // skip escaped char
                    b'"' => break,
                    _ => i += 1,
                }
            }
            if i >= b.len() {
                return None; // unterminated
            }
            let mut end = i + 1; // past closing quote
            if s[end..].starts_with("^^<") {
                let close = s[end..].find('>')? + end;
                end = close + 1;
            } else if s[end..].starts_with('@') {
                let stop = s[end..]
                    .find(char::is_whitespace)
                    .map(|p| p + end)
                    .unwrap_or(s.len());
                end = stop;
            }
            Some((s[..end].to_string(), &s[end..]))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iris_bnodes_literals() {
        let input = r#"
            # a comment
            <http://ex/Alice> <http://ex/knows> <http://ex/Bob> .
            <http://ex/Alice> <http://ex/age> "30"^^<http://www.w3.org/2001/XMLSchema#integer> .
            <http://ex/Bob> <http://ex/label> "Bob"@en .
            _:b0 <http://ex/p> "plain" .
        "#;
        let t = parse(input).unwrap();
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].0, "<http://ex/Alice>");
        assert_eq!(t[0].2, "<http://ex/Bob>");
        assert_eq!(t[1].2, "\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>");
        assert_eq!(t[2].2, "\"Bob\"@en");
        assert_eq!(t[3].0, "_:b0");
        assert_eq!(t[3].2, "\"plain\"");
    }

    #[test]
    fn literal_with_spaces_and_escaped_quote() {
        let input = r#"<http://ex/s> <http://ex/p> "a \"quoted\" phrase here" ."#;
        let t = parse(input).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].2, r#""a \"quoted\" phrase here""#);
    }

    #[test]
    fn rejects_missing_dot() {
        assert!(parse("<a> <b> <c>").is_err());
    }

    #[test]
    fn parses_quads_with_and_without_graph() {
        let input = "<http://ex/a> <http://ex/p> <http://ex/b> .\n\
                     <http://ex/a> <http://ex/p> <http://ex/c> <http://ex/g> .";
        let q = parse_quads(input).unwrap();
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].3, None); // default graph
        assert_eq!(q[1].3, Some("<http://ex/g>".to_string()));
    }
}
