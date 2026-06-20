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
    build_pyramid_meta_with, write_dataset_with_metadata, DictionaryBuilder, GraphIndexBuilder,
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
    #[error("io: {0}")]
    Io(String),
}

/// Estimate the statement count of an N-Triples/N-Quads text from its newline
/// count, so the output `Vec` can be pre-sized — a big build otherwise pays
/// repeated `Vec` doublings, each briefly holding ~2× the (large) spine. An
/// over-estimate by a few blank/comment lines is harmless (it's a capacity hint).
fn estimate_statements(input: &str) -> usize {
    bytecount_newlines(input).max(1)
}

/// Count `\n` bytes — one linear pass, far cheaper than the parse it sizes.
fn bytecount_newlines(input: &str) -> usize {
    input.as_bytes().iter().filter(|&&b| b == b'\n').count()
}

/// Parse one N-Quads line into a quad, or `None` for a blank/comment line.
/// Shared by the whole-text [`parse_quads`] and the streaming [`parse_reader`].
fn parse_nq_line(raw: &str, lineno: usize) -> Result<Option<RawQuad>, IngestError> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let stripped = line
        .strip_suffix('.')
        .ok_or(IngestError::Line(lineno, "missing trailing '.'"))?
        .trim_end();
    let (s, rest) = take_term(stripped).ok_or(IngestError::Line(lineno, "bad subject"))?;
    let (p, rest) =
        take_term(rest.trim_start()).ok_or(IngestError::Line(lineno, "bad predicate"))?;
    let (o, rest) = take_term(rest.trim_start()).ok_or(IngestError::Line(lineno, "bad object"))?;
    let rest = rest.trim();
    let graph = if rest.is_empty() {
        None
    } else {
        let (g, tail) = take_term(rest).ok_or(IngestError::Line(lineno, "bad graph"))?;
        if !tail.trim().is_empty() {
            return Err(IngestError::Line(lineno, "trailing content after graph"));
        }
        Some(g)
    };
    Ok(Some((s, p, o, graph)))
}

/// Parse one N-Triples line into a triple, or `None` for a blank/comment line.
fn parse_nt_line(raw: &str, lineno: usize) -> Result<Option<RawTriple>, IngestError> {
    let line = raw.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let stripped = line
        .strip_suffix('.')
        .ok_or(IngestError::Line(lineno, "missing trailing '.'"))?
        .trim_end();
    let (s, rest) = take_term(stripped).ok_or(IngestError::Line(lineno, "bad subject"))?;
    let (p, rest) =
        take_term(rest.trim_start()).ok_or(IngestError::Line(lineno, "bad predicate"))?;
    let (o, rest) = take_term(rest.trim_start()).ok_or(IngestError::Line(lineno, "bad object"))?;
    if !rest.trim().is_empty() {
        return Err(IngestError::Line(lineno, "trailing content after object"));
    }
    Ok(Some((s, p, o)))
}

/// Parse N-Quads text: `subject predicate object [graph] .` per line.
pub fn parse_quads(input: &str) -> Result<Vec<RawQuad>, IngestError> {
    let mut out = Vec::with_capacity(estimate_statements(input));
    for (i, raw) in input.lines().enumerate() {
        if let Some(q) = parse_nq_line(raw, i + 1)? {
            out.push(q);
        }
    }
    Ok(out)
}

/// Parse N-Triples text into raw term-token triples, skipping blank/comment lines.
pub fn parse(input: &str) -> Result<Vec<RawTriple>, IngestError> {
    let mut out = Vec::with_capacity(estimate_statements(input));
    for (i, raw) in input.lines().enumerate() {
        if let Some(t) = parse_nt_line(raw, i + 1)? {
            out.push(t);
        }
    }
    Ok(out)
}

/// **Stream-parse** N-Triples (`"nt"`) or N-Quads (`"nq"`) from a reader, one
/// line at a time, so the whole input text is **never resident** — the big-build
/// memory win over reading the file into a `String` first (each line String is
/// transient, freed every iteration). `cap` pre-sizes the output `Vec` (e.g.
/// `file_len / 64`) to avoid reallocation doublings of the large spine. Turtle is
/// not streamable here (oxttl needs the whole input); callers use the text path
/// for `"ttl"`.
pub fn parse_reader<R: std::io::BufRead>(
    reader: R,
    format: &str,
    cap: usize,
) -> Result<Vec<RawQuad>, IngestError> {
    if format != "nt" && format != "nq" {
        return Err(IngestError::UnknownFormat(format.to_string()));
    }
    let mut out = Vec::with_capacity(cap);
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| IngestError::Io(e.to_string()))?;
        if format == "nq" {
            if let Some(q) = parse_nq_line(&line, i + 1)? {
                out.push(q);
            }
        } else if let Some((s, p, o)) = parse_nt_line(&line, i + 1)? {
            out.push((s, p, o, None));
        }
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
pub fn assemble_dataset(quads: Vec<RawQuad>, metadata: &[u8]) -> (Vec<u8>, BuildStats) {
    let blob = metadata.to_vec();
    assemble_dataset_with(quads, move |_, _| blob)
}

/// Like [`assemble_dataset`], but the metadata payload is derived from the
/// [`BuildStats`] right before serialization — for metadata that embeds counts
/// only known after the dictionary and indexes are built (the Dataset Card).
/// Returning an empty `Vec` is byte-identical to a metadata-free build.
pub fn assemble_dataset_with(
    quads: Vec<RawQuad>,
    metadata: impl FnOnce(&BuildStats, &[RawQuad]) -> Vec<u8>,
) -> (Vec<u8>, BuildStats) {
    assemble_dataset_with_opts(quads, true, None, metadata)
}

/// Like [`assemble_dataset_with`], but `with_pyramid = false` skips the Louvain
/// community pyramid entirely — no pyramid section is written (header length 0).
/// SPARQL / SHACL / triple / reachability queries don't use the pyramid, so a
/// pyramid-less file is fully queryable and markedly smaller (the pyramid is the
/// largest section on highly-connected graphs). Only the community / summary /
/// progressive paths need it.
pub fn assemble_dataset_with_opts(
    quads: Vec<RawQuad>,
    with_pyramid: bool,
    type_override: Option<&str>,
    metadata: impl FnOnce(&BuildStats, &[RawQuad]) -> Vec<u8>,
) -> (Vec<u8>, BuildStats) {
    use std::collections::BTreeMap;

    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();

    let mut default_triples = Vec::new();
    let mut named: BTreeMap<String, Vec<(u32, u32, u32)>> = BTreeMap::new();
    for (s, p, o, g) in &quads {
        let t = dict.encode(s, p, o).expect("observed term");
        match g {
            None => default_triples.push(t),
            Some(graph) => named.entry(graph.clone()).or_default().push(t),
        }
    }
    let has_named = !named.is_empty();

    // Derive the metadata blob (the Dataset Card) from the raw quads NOW, while
    // they are resident — then DROP them before the memory-heavy pyramid + index
    // phases. On a big build the string quads are the largest working set (every
    // term an owned String, heavily duplicated) and are fully redundant with the
    // dictionary + id-triples once encoded, so freeing them here cuts peak RAM by
    // their whole size. `pyramid_levels` is not known yet (0 in the callback);
    // it is filled into the returned `stats` below, and no metadata callback
    // depends on it (the card derives from the quads + term/graph counts only).
    let mut stats = BuildStats {
        statements: quads.len(),
        default_triples: default_triples.len(),
        named_graphs: named.len(),
        terms: dict.term_count() as usize,
        pyramid_levels: 0,
    };
    let blob = metadata(&stats, &quads);
    drop(quads);

    let (meta, levels) = if with_pyramid {
        build_pyramid_meta_with(&dict, &default_triples, DEFAULT_TILE_BUDGET, type_override)
    } else {
        (Vec::new(), 0)
    };
    stats.pyramid_levels = levels;

    // Build the indexes from the OWNED id-triples (move, no per-triple copy); the
    // default-graph triples were borrowed by the pyramid above and are consumed
    // here, freeing them as the permutations are built.
    let def = GraphIndexBuilder::from_triples(default_triples).build();
    let named_indexes: Vec<(String, crate::GraphIndex)> = named
        .into_iter()
        .map(|(g, ts)| (g, GraphIndexBuilder::from_triples(ts).build()))
        .collect();

    let bytes =
        write_dataset_with_metadata(&dict, &def, &named_indexes, has_named, &meta, levels, &blob);
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
    fn parse_reader_matches_text_parse() {
        // The streaming reader must produce exactly the same quads as parsing the
        // whole text — including blank/comment skipping, a named graph, and CRLF.
        let nt = "<http://ex/a> <http://ex/p> <http://ex/b> .\r\n\
                  # comment\n\
                  \n\
                  _:b0 <http://ex/q> \"lit\"@en .\n";
        let via_text = parse_statements(nt, "nt").unwrap();
        let via_reader = parse_reader(std::io::Cursor::new(nt), "nt", 0).unwrap();
        assert_eq!(via_text, via_reader);

        let nq = "<http://ex/a> <http://ex/p> <http://ex/b> .\n\
                  <http://ex/a> <http://ex/p> <http://ex/c> <http://ex/g> .\n";
        assert_eq!(
            parse_quads(nq).unwrap(),
            parse_reader(std::io::Cursor::new(nq), "nq", 0).unwrap()
        );
        // Turtle is not streamable here.
        assert!(parse_reader(std::io::Cursor::new(nt), "ttl", 0).is_err());
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
        let (bytes, stats) = assemble_dataset(quads, &[]);
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
