//! Ingestion: parse RDF text (N-Triples / N-Quads / Turtle) and assemble a
//! complete `.rete` file image. Shared by the CLI's `build`/`validate` commands
//! and the wasm bindings (the playground's in-browser builder).
//!
//! The N-Triples/N-Quads reader is line-based and keeps terms as their exact
//! canonical token strings (`<iri>`, `_:bnode`, `"lit"`, `"lit"^^<dt>`,
//! `"lit"@lang`) so they double as dictionary keys and so a `query` can match
//! by the same string. This is not a full RDF 1.1 parser — it covers the
//! canonical N-Triples surface (and is deliberately tolerant of IRIs a strict
//! parser would reject), enough for v0 ingestion. Turtle goes through `oxttl`.
//!
//! Assembly degrades with the build features: without the `compression`
//! feature (e.g. on wasm) sections are written with the `NONE` codec — larger
//! files, byte-compatible readers.

use crate::{
    build_pyramid_meta, write_dataset_with_metadata, DictionaryBuilder, GraphIndexBuilder,
    DEFAULT_TILE_BUDGET,
};

/// A parsed triple as three canonical term tokens.
pub type RawTriple = (String, String, String);

/// A parsed quad: the triple plus an optional graph term (`None` = default graph).
pub type RawQuad = (String, String, String, Option<String>);

#[derive(Debug, thiserror::Error)]
pub enum IngestError {
    #[error("line {0}: {1}")]
    Line(usize, &'static str),
    #[error("turtle: {0}")]
    Turtle(String),
    #[error("unknown input format: {0} (expected nt, nq, or ttl)")]
    UnknownFormat(String),
}

/// Parse N-Quads text: `subject predicate object [graph] .` per line.
pub fn parse_quads(input: &str) -> Result<Vec<RawQuad>, IngestError> {
    let mut out = Vec::new();
    for (i, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stripped = line
            .strip_suffix('.')
            .ok_or(IngestError::Line(i + 1, "missing trailing '.'"))?
            .trim_end();
        let (s, rest) = take_term(stripped).ok_or(IngestError::Line(i + 1, "bad subject"))?;
        let (p, rest) =
            take_term(rest.trim_start()).ok_or(IngestError::Line(i + 1, "bad predicate"))?;
        let (o, rest) =
            take_term(rest.trim_start()).ok_or(IngestError::Line(i + 1, "bad object"))?;
        let rest = rest.trim();
        let graph = if rest.is_empty() {
            None
        } else {
            let (g, tail) = take_term(rest).ok_or(IngestError::Line(i + 1, "bad graph"))?;
            if !tail.trim().is_empty() {
                return Err(IngestError::Line(i + 1, "trailing content after graph"));
            }
            Some(g)
        };
        out.push((s, p, o, graph));
    }
    Ok(out)
}

/// Parse N-Triples text into raw term-token triples, skipping blank/comment lines.
pub fn parse(input: &str) -> Result<Vec<RawTriple>, IngestError> {
    let mut out = Vec::new();
    for (i, raw) in input.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let stripped = line
            .strip_suffix('.')
            .ok_or(IngestError::Line(i + 1, "missing trailing '.'"))?
            .trim_end();
        let (s, rest) = take_term(stripped).ok_or(IngestError::Line(i + 1, "bad subject"))?;
        let (p, rest) =
            take_term(rest.trim_start()).ok_or(IngestError::Line(i + 1, "bad predicate"))?;
        let (o, rest) =
            take_term(rest.trim_start()).ok_or(IngestError::Line(i + 1, "bad object"))?;
        if !rest.trim().is_empty() {
            return Err(IngestError::Line(i + 1, "trailing content after object"));
        }
        out.push((s, p, o));
    }
    Ok(out)
}

/// Parse Turtle into canonical N-Triples-token triples via oxttl.
pub fn parse_turtle(text: &str) -> Result<Vec<RawTriple>, IngestError> {
    let mut out = Vec::new();
    for r in oxttl::TurtleParser::new().for_reader(text.as_bytes()) {
        let t = r.map_err(|e| IngestError::Turtle(e.to_string()))?;
        out.push((
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        ));
    }
    Ok(out)
}

/// Parse one text input by format name (`"nt"`, `"nq"`, or `"ttl"`) into quads
/// (triples land in the default graph).
pub fn parse_statements(text: &str, format: &str) -> Result<Vec<RawQuad>, IngestError> {
    match format {
        "nq" => parse_quads(text),
        "ttl" => Ok(parse_turtle(text)?
            .into_iter()
            .map(|(s, p, o)| (s, p, o, None))
            .collect()),
        "nt" => Ok(parse(text)?
            .into_iter()
            .map(|(s, p, o)| (s, p, o, None))
            .collect()),
        other => Err(IngestError::UnknownFormat(other.to_string())),
    }
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

/// Counts describing an assembled file, for status lines and UIs.
#[derive(Debug, Clone, Copy)]
pub struct BuildStats {
    pub statements: usize,
    pub default_triples: usize,
    pub named_graphs: usize,
    pub terms: usize,
    pub pyramid_levels: u16,
}

/// Assemble a complete `.rete` file image from parsed quads: one shared
/// dictionary, the default-graph index, one index per named graph, and the
/// community pyramid. `metadata` is the opaque metadata-section payload (the
/// CLI puts a JSON Dataset Card there); pass `&[]` for none — that is
/// byte-identical to a metadata-free build.
pub fn assemble_dataset(quads: &[RawQuad], metadata: &[u8]) -> (Vec<u8>, BuildStats) {
    assemble_dataset_with(quads, |_| metadata.to_vec())
}

/// Like [`assemble_dataset`], but the metadata payload is derived from the
/// [`BuildStats`] right before serialization — for metadata that embeds counts
/// only known after the dictionary and indexes are built (the Dataset Card).
/// Returning an empty `Vec` is byte-identical to a metadata-free build.
pub fn assemble_dataset_with(
    quads: &[RawQuad],
    metadata: impl FnOnce(&BuildStats) -> Vec<u8>,
) -> (Vec<u8>, BuildStats) {
    use std::collections::BTreeMap;

    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in quads {
        db.observe(s, p, o);
    }
    let dict = db.build();

    let mut default_triples = Vec::new();
    let mut named: BTreeMap<String, Vec<(u32, u32, u32)>> = BTreeMap::new();
    for (s, p, o, g) in quads {
        let t = dict.encode(s, p, o).expect("observed term");
        match g {
            None => default_triples.push(t),
            Some(graph) => named.entry(graph.clone()).or_default().push(t),
        }
    }
    let has_named = !named.is_empty();

    let mut def = GraphIndexBuilder::new();
    for &t in &default_triples {
        def.push(t);
    }
    let named_indexes: Vec<(String, crate::GraphIndex)> = named
        .into_iter()
        .map(|(g, ts)| {
            let mut b = GraphIndexBuilder::new();
            for t in ts {
                b.push(t);
            }
            (g, b.build())
        })
        .collect();

    let (meta, levels) = build_pyramid_meta(&dict, &default_triples, DEFAULT_TILE_BUDGET);
    let stats = BuildStats {
        statements: quads.len(),
        default_triples: default_triples.len(),
        named_graphs: named_indexes.len(),
        terms: dict.term_count() as usize,
        pyramid_levels: levels,
    };
    let blob = metadata(&stats);
    let bytes = write_dataset_with_metadata(
        &dict,
        &def.build(),
        &named_indexes,
        has_named,
        &meta,
        levels,
        &blob,
    );
    (bytes, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rete;

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

    #[test]
    fn parse_statements_dispatches_by_format() {
        let nt = "<http://ex/a> <http://ex/p> <http://ex/b> .";
        assert_eq!(parse_statements(nt, "nt").unwrap().len(), 1);
        let ttl = "@prefix ex: <http://ex/> .\nex:A ex:knows ex:B , ex:C .";
        assert_eq!(parse_statements(ttl, "ttl").unwrap().len(), 2);
        assert!(parse_statements(nt, "trig").is_err());
    }

    /// Text in, queryable file image out — the whole in-memory build path the
    /// wasm `build()` binding uses, including a named graph.
    #[test]
    fn assemble_dataset_round_trips() {
        let text = "<http://ex/a> <http://ex/knows> <http://ex/b> .\n\
                    <http://ex/b> <http://ex/knows> <http://ex/c> .\n\
                    <http://ex/a> <http://ex/age> \"30\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n\
                    <http://ex/a> <http://ex/p> <http://ex/d> <http://ex/g1> .";
        let quads = parse_statements(text, "nq").unwrap();
        let (bytes, stats) = assemble_dataset(&quads, &[]);
        assert_eq!(stats.statements, 4);
        assert_eq!(stats.default_triples, 3);
        assert_eq!(stats.named_graphs, 1);
        assert!(stats.terms >= 7);

        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(
            rete.query(None, Some("<http://ex/knows>"), None).len(),
            2,
            "default-graph pattern query"
        );
        assert_eq!(rete.graph_names(), vec!["<http://ex/g1>"]);
        let out = crate::eval_query(
            &rete,
            "SELECT ?x WHERE { ?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z }",
        )
        .unwrap();
        match out {
            crate::QueryOutput::Select(_, rows) => assert_eq!(rows.len(), 1),
            other => panic!("expected select result, got {other:?}"),
        }
    }
}
