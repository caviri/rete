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
    build_pyramid_meta_algo, Dictionary, DictionaryBuilder, GraphIndexBuilder, PyramidAlgo,
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
    #[error("rdf/xml: {0}")]
    RdfXml(String),
    #[error("unknown input format: {0} (expected nt, nq, ttl, or rdf/xml)")]
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
    let mut out = Vec::with_capacity(cap);
    stream_reader(reader, format, &mut |q| out.push(q))?;
    Ok(out)
}

/// **Stream** N-Triples (`"nt"`) / N-Quads (`"nq"`) from a reader, invoking `f`
/// once per parsed quad — like [`parse_reader`] but **without collecting** into a
/// `Vec`. Each quad's term Strings are owned by `f` (and dropped when it returns
/// if it doesn't retain them), so a caller that only needs to *observe* every
/// term — e.g. the two-pass [`assemble_dataset_streaming`] building its
/// dictionary — never materializes the whole quad multiset. The big-graph,
/// low-RAM ingest primitive. Blank/comment lines are skipped; a parse error stops
/// the stream and is returned.
pub fn stream_reader<R: std::io::BufRead>(
    reader: R,
    format: &str,
    f: &mut dyn FnMut(RawQuad),
) -> Result<(), IngestError> {
    if format != "nt" && format != "nq" {
        return Err(IngestError::UnknownFormat(format.to_string()));
    }
    for (i, line) in reader.lines().enumerate() {
        let line = line.map_err(|e| IngestError::Io(e.to_string()))?;
        if format == "nq" {
            if let Some(q) = parse_nq_line(&line, i + 1)? {
                f(q);
            }
        } else if let Some((s, p, o)) = parse_nt_line(&line, i + 1)? {
            f((s, p, o, None));
        }
    }
    Ok(())
}

/// Parse Turtle into canonical N-Triples-token triples via oxttl.
pub fn parse_turtle(text: &str) -> Result<Vec<RawTriple>, IngestError> {
    let mut out = Vec::new();
    // `with_quoted_triples` accepts RDF-star quoted triples (`<< s p o >>`) in
    // subject/object position; oxrdf's `Term::Triple` then Displays as the
    // canonical `<< … >>` token our N-Triples-star tokenizer also emits.
    for r in oxttl::TurtleParser::new()
        .with_quoted_triples()
        .for_reader(text.as_bytes())
    {
        let t = r.map_err(|e| IngestError::Turtle(e.to_string()))?;
        out.push((
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        ));
    }
    Ok(out)
}

/// Parse RDF/XML into canonical N-Triples-token triples via oxrdfxml. This is how
/// most OWL ontologies ship (`.rdf`/`.owl`/`.xml` with an `rdf:RDF` root) — so rete
/// ingests them directly, no external conversion. (OWL/XML — the non-RDF functional
/// XML serialization — is a different language; convert it with owlready2 first.)
pub fn parse_rdfxml(text: &str) -> Result<Vec<RawTriple>, IngestError> {
    let mut out = Vec::new();
    for r in oxrdfxml::RdfXmlParser::new().for_reader(text.as_bytes()) {
        let t = r.map_err(|e| IngestError::RdfXml(e.to_string()))?;
        out.push((
            t.subject.to_string(),
            t.predicate.to_string(),
            t.object.to_string(),
        ));
    }
    Ok(out)
}

/// Parse one text input by format name (`"nt"`, `"nq"`, `"ttl"`, or `"rdfxml"`)
/// into quads (triples land in the default graph).
pub fn parse_statements(text: &str, format: &str) -> Result<Vec<RawQuad>, IngestError> {
    match format {
        "nq" => parse_quads(text),
        "ttl" => Ok(parse_turtle(text)?
            .into_iter()
            .map(|(s, p, o)| (s, p, o, None))
            .collect()),
        "rdfxml" => Ok(parse_rdfxml(text)?
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
pub(crate) fn take_term(s: &str) -> Option<(String, &str)> {
    let bytes = s.as_bytes();
    let first = *bytes.first()?;
    match first {
        // Quoted triple (RDF-star): `<< subject predicate object >>`, where the
        // inner terms are themselves terms (so this recurses — nesting works).
        // `<<` starts with `<`, and an IRI scan would stop at the first inner `>`,
        // so it must be handled before the plain-IRI case. Re-emitted in the
        // canonical `<< s p o >>` surface (single spaces) — identical to
        // oxrdf's `Triple` Display, so N-Triples-star and Turtle-star produce the
        // same dictionary token and dedupe.
        b'<' if bytes.get(1) == Some(&b'<') => {
            let inner = s[2..].trim_start();
            let (subj, r) = take_term(inner)?;
            let (pred, r) = take_term(r.trim_start())?;
            let (obj, r) = take_term(r.trim_start())?;
            let rest = r.trim_start().strip_prefix(">>")?;
            // Canonical surface = oxrdf's `Triple` Display: `<<s p o>>` (tight
            // brackets, single spaces between components). Matching it exactly is
            // what makes N-Triples-star and Turtle-star (which round-trips through
            // oxrdf) produce the SAME dictionary token, so they dedupe and a query
            // written either way matches either source.
            Some((format!("<<{subj} {pred} {obj}>>"), rest))
        }
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
                // Language tag: '@' then BCP-47 subtags `[a-zA-Z0-9-]+`. Stop at
                // the first char that can't be part of a tag — normally the
                // whitespace before the predicate, but ALSO the `>>` that closes
                // a quoted triple when this literal is its object (`"x"@en>>`),
                // where there is no separating whitespace.
                let mut j = end + 1; // past '@'
                while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
                    j += 1;
                }
                end = j;
            }
            Some((s[..end].to_string(), &s[end..]))
        }
        _ => None,
    }
}

/// Split a canonical quoted-triple token `<<s p o>>` (RDF-star) into its three
/// component term tokens, or `None` if `t` is not a quoted triple. Reuses
/// [`take_term`] for term-boundary scanning, so nested quoting parses correctly.
/// The inverse of the `<<…>>` construction; used by the SUBJECT/PREDICATE/OBJECT
/// SPARQL-star builtins.
pub(crate) fn quoted_triple_parts(t: &str) -> Option<(String, String, String)> {
    let inner = t.strip_prefix("<<")?.strip_suffix(">>")?.trim();
    let (s, r) = take_term(inner)?;
    let (p, r) = take_term(r.trim_start())?;
    let (o, r) = take_term(r.trim_start())?;
    if !r.trim().is_empty() {
        return None;
    }
    Some((s, p, o))
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
    assemble_dataset_with_opts(quads, true, false, None, metadata)
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
    with_text_index: bool,
    type_override: Option<&str>,
    metadata: impl FnOnce(&BuildStats, &[RawQuad]) -> Vec<u8>,
) -> (Vec<u8>, BuildStats) {
    assemble_dataset_with_opts_algo(
        quads,
        with_pyramid,
        with_text_index,
        type_override,
        PyramidAlgo::Louvain,
        metadata,
    )
}

/// Like [`assemble_dataset_with_opts`], but selects the community [`PyramidAlgo`]
/// (the in-memory build path for `rete build --pyramid-algo …`).
#[allow(clippy::too_many_arguments)]
pub fn assemble_dataset_with_opts_algo(
    quads: Vec<RawQuad>,
    with_pyramid: bool,
    with_text_index: bool,
    type_override: Option<&str>,
    algo: PyramidAlgo,
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

    // Derive the metadata blob (the Dataset Card) from the raw quads NOW, while
    // they are resident — then DROP them before the memory-heavy pyramid + index
    // phases. On a big build the string quads are the largest working set (every
    // term an owned String, heavily duplicated) and are fully redundant with the
    // dictionary + id-triples once encoded, so freeing them here cuts peak RAM by
    // their whole size. `pyramid_levels` is not known yet (0 in the callback);
    // it is filled into the returned `stats` by `finish_assembly`, and no metadata
    // callback depends on it (the card derives from the quads + term/graph counts).
    let stats = BuildStats {
        statements: quads.len(),
        default_triples: default_triples.len(),
        named_graphs: named.len(),
        terms: dict.term_count() as usize,
        pyramid_levels: 0,
    };
    let blob = metadata(&stats, &quads);
    drop(quads);

    finish_assembly(
        dict,
        default_triples,
        named,
        with_pyramid,
        with_text_index,
        type_override,
        algo,
        blob,
        stats,
    )
}

/// **Two-pass, low-RAM** assembly: build a `.rete` by **streaming** the input(s)
/// twice instead of holding every parsed quad in memory. `stream` is invoked
/// **twice** and MUST replay the exact same quads in the same order each time —
/// pass 1 observes every term into the dictionary; pass 2 encodes them to
/// id-triples. The raw string quads (every term an owned String, heavily
/// duplicated — by far the largest working set on a big graph) are **never
/// collected**, so peak RAM is bounded by the dictionary + id-triples + index
/// rather than the string-quad multiset. Output is **byte-identical** to
/// [`assemble_dataset_with_opts`] on the same quads (same dictionary, same
/// id-triples in file order, same downstream pipeline).
///
/// The metadata callback derives the Dataset Card from the built dictionary +
/// default-graph id-triples (resolving terms through the dictionary), since the
/// raw quads were never retained. `stream` propagates parse/IO errors.
pub fn assemble_dataset_streaming<S>(
    stream: S,
    with_pyramid: bool,
    with_text_index: bool,
    type_override: Option<&str>,
    metadata: impl FnOnce(&BuildStats, &Dictionary, &[(u32, u32, u32)]) -> Vec<u8>,
) -> Result<(Vec<u8>, BuildStats), IngestError>
where
    S: FnMut(&mut dyn FnMut(RawQuad)) -> Result<(), IngestError>,
{
    assemble_dataset_streaming_algo(
        stream,
        with_pyramid,
        with_text_index,
        type_override,
        PyramidAlgo::Louvain,
        metadata,
    )
}

/// Like [`assemble_dataset_streaming`], but selects the community [`PyramidAlgo`]
/// (the streaming, low-RAM build path for `rete build --pyramid-algo …`).
#[allow(clippy::too_many_arguments)]
pub fn assemble_dataset_streaming_algo<S>(
    mut stream: S,
    with_pyramid: bool,
    with_text_index: bool,
    type_override: Option<&str>,
    algo: PyramidAlgo,
    metadata: impl FnOnce(&BuildStats, &Dictionary, &[(u32, u32, u32)]) -> Vec<u8>,
) -> Result<(Vec<u8>, BuildStats), IngestError>
where
    S: FnMut(&mut dyn FnMut(RawQuad)) -> Result<(), IngestError>,
{
    use std::collections::BTreeMap;

    // Pass 1: observe every term (the dictionary dedups; the string quads are
    // freed line by line and never collected).
    let mut db = DictionaryBuilder::new();
    stream(&mut |(s, p, o, _g)| db.observe(&s, &p, &o))?;
    let dict = db.build();

    // Pass 2: encode each quad to an id-triple, bucketing named graphs.
    let mut default_triples: Vec<(u32, u32, u32)> = Vec::new();
    let mut named: BTreeMap<String, Vec<(u32, u32, u32)>> = BTreeMap::new();
    stream(&mut |(s, p, o, g)| {
        let t = dict.encode(&s, &p, &o).expect("observed term");
        match g {
            None => default_triples.push(t),
            Some(graph) => named.entry(graph).or_default().push(t),
        }
    })?;

    let statements = default_triples.len() + named.values().map(Vec::len).sum::<usize>();
    let stats = BuildStats {
        statements,
        default_triples: default_triples.len(),
        named_graphs: named.len(),
        terms: dict.term_count() as usize,
        pyramid_levels: 0,
    };
    let blob = metadata(&stats, &dict, &default_triples);
    Ok(finish_assembly(
        dict,
        default_triples,
        named,
        with_pyramid,
        with_text_index,
        type_override,
        algo,
        blob,
        stats,
    ))
}

/// The shared tail of every build path: from a finished dictionary + encoded
/// id-triples (default graph + named), build the community pyramid, the optional
/// full-text index, the permutation indexes, and serialize the file image. Takes
/// the id-triples **by value** so they are freed as the permutations consume them.
#[allow(clippy::too_many_arguments)]
fn finish_assembly(
    dict: Dictionary,
    default_triples: Vec<(u32, u32, u32)>,
    named: std::collections::BTreeMap<String, Vec<(u32, u32, u32)>>,
    with_pyramid: bool,
    with_text_index: bool,
    type_override: Option<&str>,
    algo: PyramidAlgo,
    blob: Vec<u8>,
    mut stats: BuildStats,
) -> (Vec<u8>, BuildStats) {
    let has_named = !named.is_empty();

    // The pyramid and the full-text index are built FIRST, while the dictionary and
    // the default id-triples are both still resident — both need the two together.
    let (meta, levels) = if with_pyramid {
        build_pyramid_meta_algo(
            &dict,
            &default_triples,
            DEFAULT_TILE_BUDGET,
            type_override,
            algo,
        )
    } else {
        (Vec::new(), 0)
    };
    stats.pyramid_levels = levels;

    let text_index = if with_text_index {
        crate::file::compute_text_index(&dict, &default_triples)
    } else {
        Vec::new()
    };

    // From here the dictionary is only needed for its own serialized bytes. Encode
    // it, capture the header term count, then DROP it before building the
    // permutation indexes — which work purely on id-triples and never touch the
    // dictionary. On a large graph this frees the single biggest resident structure
    // (the dictionary) right when the index sort needs the headroom. The output is
    // byte-for-byte identical to serializing the dictionary inline.
    let codec = crate::file::writer_codec();
    let dict_container = crate::file::encode_dict_container(&dict, codec);
    let term_count = dict.term_count() as u64;
    let has_quoted_triples = dict.has_quoted_triples();
    drop(dict);

    // Build each graph's six permutations one-at-a-time on a large graph (a single
    // permuted copy resident at a time, each sort still parallel) instead of all
    // six concurrently; below the threshold the faster all-permutations-parallel
    // build is used (its 6x transient copy is negligible there). `build_seq` is
    // byte-identical to `build`.
    let build_index = |triples: Vec<(u32, u32, u32)>| -> crate::GraphIndex {
        let n = triples.len();
        let b = GraphIndexBuilder::from_triples(triples);
        if n > LOWMEM_TRIPLE_THRESHOLD {
            b.build_seq()
        } else {
            b.build()
        }
    };
    let def = build_index(default_triples);
    let named_indexes: Vec<(String, crate::GraphIndex)> = named
        .into_iter()
        .map(|(g, ts)| (g, build_index(ts)))
        .collect();

    let bytes = crate::file::write_dataset_from_parts(
        &dict_container,
        term_count,
        &def,
        &named_indexes,
        has_named,
        has_quoted_triples,
        &meta,
        levels,
        &blob,
        &text_index,
        codec,
    );
    (bytes, stats)
}

/// Above this default-graph triple count, the index is built one permutation at a
/// time (low peak RAM) instead of all six in parallel. Chosen so typical builds
/// keep the faster parallel path while multi-hundred-million-triple graphs stay
/// within a bounded memory budget.
const LOWMEM_TRIPLE_THRESHOLD: usize = 30_000_000;

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

    /// RDF-star: a quoted triple whose object is a **language-tagged literal**
    /// sits directly against the closing `>>` with no separating whitespace
    /// (`"name"@fr>>`). The language-tag scan must stop at `>` — a regression
    /// guard for the greedy scan-to-whitespace that swallowed `@fr>>` as the tag.
    #[test]
    fn quoted_triple_langtagged_object() {
        let s = r#"<<<http://ex/sp> <http://ex/name> "Hirondelle rustique"@fr>>"#;
        let (tok, rest) = take_term(s).unwrap();
        assert_eq!(
            tok,
            r#"<<<http://ex/sp> <http://ex/name> "Hirondelle rustique"@fr>>"#
        );
        assert_eq!(rest, "");
        // and it round-trips through a full annotation line
        let line = r#"<<<http://ex/sp> <http://ex/name> "Oreneta vulgar"@ca>> <http://purl.org/dc/terms/source> "Catalogue of Life" ."#;
        let t = parse(line).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(
            t[0].0,
            r#"<<<http://ex/sp> <http://ex/name> "Oreneta vulgar"@ca>>"#
        );
        assert_eq!(t[0].2, r#""Catalogue of Life""#);
        // a plain lang-tagged object still terminates at whitespace
        assert_eq!(take_term(r#""x"@pt-BR ."#).unwrap().0, r#""x"@pt-BR"#);
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

    /// RDF/XML (how most OWL ontologies ship) parses to the same canonical tokens,
    /// including the abbreviated typed-node syntax and `rdf:resource` references.
    #[test]
    fn parses_rdfxml_owl() {
        let xml = r#"<?xml version="1.0"?>
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
                     xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
                     xmlns:owl="http://www.w3.org/2002/07/owl#">
              <owl:Class rdf:about="http://ex/Dog">
                <rdfs:subClassOf rdf:resource="http://ex/Animal"/>
                <rdfs:label>Dog</rdfs:label>
              </owl:Class>
            </rdf:RDF>"#;
        let triples = parse_rdfxml(xml).unwrap();
        // rdf:type owl:Class, rdfs:subClassOf, rdfs:label = 3 triples.
        assert_eq!(triples.len(), 3);
        assert!(triples.iter().any(|(s, p, o)| s == "<http://ex/Dog>"
            && p == "<http://www.w3.org/2000/01/rdf-schema#subClassOf>"
            && o == "<http://ex/Animal>"));
        // Same data through the format dispatcher (triples → default graph).
        assert_eq!(parse_statements(xml, "rdfxml").unwrap().len(), 3);
        // Malformed XML is a clear RdfXml error, not a silent empty parse.
        assert!(matches!(
            parse_statements("<rdf:RDF><not closed", "rdfxml"),
            Err(IngestError::RdfXml(_))
        ));
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

    /// The exact minimal input the playground Build tab uses: two triples, no
    /// literals, no rdf:type, no named graph — the smallest graph that still
    /// builds a community pyramid. (The wasm `build()` panic this guards was a
    /// `std::time::Instant::now()` in the pyramid timing path; native has a clock
    /// so this passes here, while the playground harness exercises the wasm path.)
    #[test]
    fn assemble_minimal_typeless_graph() {
        let text = "<http://ex/A> <http://ex/knows> <http://ex/B> .\n\
                    <http://ex/B> <http://ex/knows> <http://ex/C> .\n";
        let quads = parse_statements(text, "nt").unwrap();
        let (bytes, stats) = assemble_dataset(quads, &[]);
        assert_eq!(stats.default_triples, 2);
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.query(None, Some("<http://ex/knows>"), None).len(), 2);
    }
}
