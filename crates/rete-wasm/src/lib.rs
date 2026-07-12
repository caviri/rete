//! Browser bindings: load a `.rete` file's bytes (e.g. from `fetch`) and query
//! it entirely in the browser — the same engine the native CLI uses, compiled
//! to wasm. Results come back as JSON strings.

use rete_core::{
    batch_reach_serial, build_adjacency, build_dendrogram, choose_round_for_budget, eval_query,
    eval_query_reasoned, eval_select_communities, eval_sparql, project_graph, schema_classes,
    schema_summary, summary_query_shape, tile_by_community, validate_shacl, BlockCacheReader,
    ByteRange, CountingReader, DataGraph, Header, QueryOutput, RangeReader, Rete, ReteGraph,
    ShaclShapes, SliceReader, SummaryQueryShape, SummaryView, TripleProvenance, ValidationReport,
    DEFAULT_BLOCK, DEFAULT_TILE_BUDGET,
};
use wasm_bindgen::prelude::*;

/// Module init: route Rust panics to `console.error` with their message and
/// location. In release wasm a panic otherwise aborts as a bare
/// `RuntimeError: unreachable` with no clue where — this turns that into a
/// `rete-wasm panic: panicked at '…', src/…:line` line in the devtools console,
/// so an intermittent first-query crash (e.g. a parser tripping on a flaky
/// range read) can actually be diagnosed.
#[wasm_bindgen(start)]
pub fn __start() {
    std::panic::set_hook(Box::new(|info| {
        web_sys::console::error_1(&JsValue::from_str(&format!("rete-wasm panic: {info}")));
    }));
}

/// Header summary as JSON: `{ "quads": N, "terms": N, "pyramidLevels": N }`.
#[wasm_bindgen]
pub fn info(bytes: &[u8]) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    let h = rete.header();
    Ok(format!(
        r#"{{"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
        h.quad_count,
        h.term_count,
        h.pyramid_levels,
        rete.graph_names().len()
    ))
}

/// The named-graph IRIs of a dataset, as a JSON array.
#[wasm_bindgen]
pub fn graph_names(bytes: &[u8]) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    serde_json::to_string(&rete.graph_names()).map_err(err)
}

/// Build a complete `.rete` file image from RDF text, entirely in the browser.
///
/// `format` is `"nt"` (N-Triples), `"nq"` (N-Quads; named graphs become a
/// dataset), or `"ttl"` (Turtle). Returns the file bytes (a `Uint8Array`),
/// ready to download or to hand straight back to the query functions. The wasm
/// build has no zstd *encoder*, so sections are written uncompressed (codec
/// `NONE`) — larger than a CLI build of the same data, but every reader
/// accepts it; rebuild with `rete build` for a compressed file.
#[wasm_bindgen]
pub fn build(text: &str, format: &str) -> Result<Vec<u8>, JsValue> {
    let quads = rete_core::ingest::parse_statements(text, format)
        .map_err(|e| JsValue::from_str(&e.to_string()))?;
    if quads.is_empty() {
        return Err(JsValue::from_str(
            "no statements parsed (empty input or only comments)",
        ));
    }
    let (bytes, _stats) = rete_core::ingest::assemble_dataset(quads, &[]);
    Ok(bytes)
}

/// Evaluate a triple pattern; `null`/`undefined` positions are wildcards.
/// Returns a JSON array of `[subject, predicate, object]` triples.
#[wasm_bindgen]
pub fn query_triples(
    bytes: &[u8],
    subject: Option<String>,
    predicate: Option<String>,
    object: Option<String>,
) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    let rows = rete.query(subject.as_deref(), predicate.as_deref(), object.as_deref());
    serde_json::to_string(&rows).map_err(err)
}

/// Explain why each triple-pattern match is present in the `.rete` file.
///
/// `null`/`undefined` positions are wildcards. The JSON uses browser-facing
/// camelCase fields:
/// `{ "pattern", "resultCount", "results": [{ "terms", "ids", "provenance" }] }`.
#[wasm_bindgen]
pub fn why_triples(
    bytes: &[u8],
    subject: Option<String>,
    predicate: Option<String>,
    object: Option<String>,
) -> Result<String, JsValue> {
    why_triples_json(
        bytes,
        subject.as_deref(),
        predicate.as_deref(),
        object.as_deref(),
    )
    .map_err(|e| JsValue::from_str(&e))
}

/// Native-testable implementation for [`why_triples`].
pub fn why_triples_json(
    bytes: &[u8],
    subject: Option<&str>,
    predicate: Option<&str>,
    object: Option<&str>,
) -> Result<String, String> {
    use serde_json::json;

    let rete = Rete::open(bytes).map_err(|e| e.to_string())?;
    let results = rete.query_with_provenance(subject, predicate, object);
    let out = json!({
        "pattern": {
            "subject": subject,
            "predicate": predicate,
            "object": object,
        },
        "resultCount": results.len(),
        "results": results.iter().map(provenance_json).collect::<Vec<_>>(),
    });
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

/// Run a SPARQL SELECT; returns a JSON array of solution objects
/// (`{ "var": "value", ... }`).
#[wasm_bindgen]
pub fn query_sparql(bytes: &[u8], query: &str) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    let (_vars, solutions) = eval_sparql(&rete, query).map_err(err)?;
    serde_json::to_string(&solutions).map_err(err)
}

/// The ontology profile (the semantic coarse graph), as JSON:
/// `{ "classes": [["<iri>", count], ...],
///    "relations": [["sClass","pred","oClass", count], ...] }`.
/// The "overview first" payload — render it before fetching any detail.
#[wasm_bindgen]
pub fn schema(bytes: &[u8]) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    let classes = schema_classes(&rete);
    let relations = schema_summary(&rete);
    serde_json::to_string(&serde_json::json!({
        "classes": classes,
        "relations": relations,
    }))
    .map_err(err)
}

/// Read the **baked** schema pyramid (classes + class-level relations) straight
/// from the file's schema block via a slice reader — no triple scan. The
/// in-memory twin of [`schema_url`]: a cached big graph gets its schema from a
/// few KB of the buffer instead of dumping every triple (seconds on a 150 MB
/// file). Errors when the file carries no schema pyramid, so callers fall back
/// to the scanning [`schema`].
#[wasm_bindgen]
pub fn schema_packed(bytes: &[u8]) -> Result<String, JsValue> {
    use serde_json::json;
    let (classes, relations) = rete_core::read_schema_summary_ranged(&SliceReader::new(bytes))
        .map_err(err)?
        .ok_or_else(|| JsValue::from_str("file has no schema pyramid"))?;
    let classes: Vec<serde_json::Value> = classes.iter().map(|(c, n)| json!([c, n])).collect();
    let relations: Vec<serde_json::Value> = relations
        .iter()
        .map(|(s, p, o, n)| json!([s, p, o, n]))
        .collect();
    Ok(json!({ "classes": classes, "relations": relations }).to_string())
}

/// A `.rete` opened **once** and kept resident, so a client (the playground's
/// cached/in-memory mode) can run many queries on a big file without re-copying
/// the whole buffer into wasm and re-decoding its dictionary on every call. The
/// methods mirror the free functions above but operate on the already-open
/// [`Rete`]. The few index-free readers (`schema_packed`, `progressive_query`,
/// `check_schema`) stay free functions — they read small ranges from the buffer
/// and are called rarely (once at load / on demand), so a handle buys little.
#[wasm_bindgen]
pub struct Graph {
    rete: Rete,
    file_len: usize,
}

#[wasm_bindgen]
impl Graph {
    /// Open a `.rete` image and keep it resident for repeated querying.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Graph, JsValue> {
        Ok(Graph {
            rete: open(bytes)?,
            file_len: bytes.len(),
        })
    }

    /// See [`info`].
    pub fn info(&self) -> String {
        let h = self.rete.header();
        format!(
            r#"{{"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
            h.quad_count,
            h.term_count,
            h.pyramid_levels,
            self.rete.graph_names().len()
        )
    }

    /// See [`graph_names`].
    pub fn graph_names(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.rete.graph_names()).map_err(err)
    }

    /// See [`query`].
    pub fn query(&self, query: &str, format: &str) -> Result<String, JsValue> {
        query_json(&self.rete, query, format, "")
    }

    /// As [`query`], with OWL 2 QL entailment on (`rdfs:subClassOf` /
    /// `subPropertyOf` / `domain` / `range` reasoning by query rewriting).
    pub fn query_reasoned(&self, query: &str, format: &str) -> Result<String, JsValue> {
        query_json_reasoned(&self.rete, query, format, "")
    }

    /// See [`query_triples`].
    pub fn query_triples(
        &self,
        subject: Option<String>,
        predicate: Option<String>,
        object: Option<String>,
    ) -> Result<String, JsValue> {
        let rows = self
            .rete
            .query(subject.as_deref(), predicate.as_deref(), object.as_deref());
        serde_json::to_string(&rows).map_err(err)
    }

    /// See [`prefix_search`].
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Result<String, JsValue> {
        prefix_search_json(&self.rete, prefix, limit)
    }

    /// See [`text_search`].
    pub fn text_search(
        &self,
        words: Vec<String>,
        contains_prefix: Option<String>,
        limit: usize,
    ) -> Result<String, JsValue> {
        text_search_json(&self.rete, &words, contains_prefix.as_deref(), limit)
    }

    /// See [`why_triples`].
    pub fn why_triples(
        &self,
        subject: Option<String>,
        predicate: Option<String>,
        object: Option<String>,
    ) -> Result<String, JsValue> {
        use serde_json::json;
        let results = self.rete.query_with_provenance(
            subject.as_deref(),
            predicate.as_deref(),
            object.as_deref(),
        );
        let out = json!({
            "pattern": { "subject": subject, "predicate": predicate, "object": object },
            "resultCount": results.len(),
            "results": results.iter().map(provenance_json).collect::<Vec<_>>(),
        });
        serde_json::to_string(&out).map_err(err)
    }

    /// See [`schema`] — the live (scanning) profile; prefer `schema_packed` when
    /// the file carries a pyramid.
    pub fn schema(&self) -> Result<String, JsValue> {
        serde_json::to_string(&serde_json::json!({
            "classes": schema_classes(&self.rete),
            "relations": schema_summary(&self.rete),
        }))
        .map_err(err)
    }

    /// See [`query_communities`].
    pub fn query_communities(&self, query: &str, round: Option<usize>) -> Result<String, JsValue> {
        query_communities_value(&self.rete, query, round)
    }

    /// See [`reach`].
    pub fn reach(&self, predicate: &str, seeds: &str, reverse: bool) -> Result<String, JsValue> {
        reach_rete(&self.rete, predicate, seeds, reverse)
    }

    /// See [`shacl`].
    pub fn shacl(
        &self,
        shapes_turtle: &str,
        graph: Option<String>,
        format: &str,
    ) -> Result<String, JsValue> {
        shacl_rete(&self.rete, shapes_turtle, graph.as_deref(), format)
    }

    /// See [`reason`].
    pub fn reason(&self, graph: Option<String>) -> String {
        let base = self.rete.dump(graph.as_deref());
        reasoning_json(&rete_core::reason(&base), None)
    }

    /// See [`pyramid_tree`].
    pub fn pyramid_tree(&self) -> Result<String, JsValue> {
        pyramid_tree_value(&self.rete)
    }

    /// See [`file_layout`].
    pub fn file_layout(&self) -> Result<String, JsValue> {
        use serde_json::json;
        let segments: Vec<serde_json::Value> = self
            .rete
            .file_layout()
            .iter()
            .map(|s| json!({ "kind": s.kind, "label": s.label, "offset": s.offset, "len": s.len }))
            .collect();
        serde_json::to_string(&json!({ "fileLength": self.file_len, "segments": segments }))
            .map_err(err)
    }
}

/// A remote `.rete` opened **once over HTTP range** and kept resident in the
/// worker, so repeated queries on the same URL reuse (a) the block cache — any
/// 64 KiB block fetched once is served from memory by [`BlockCacheReader`] — and
/// (b) the lazily faulted index tiles + decoded dictionary chunks that live
/// inside the resident [`Rete`]. The free [`sparql_url`] re-opens the file on
/// every call, so its block cache dies after one query; this handle keeps it.
/// The counting reader stays reachable so the worker can read cumulative
/// bytes/requests and show how little a cache-hit query actually fetched.
/// **Worker-only** (synchronous range-read XHR).
#[wasm_bindgen]
pub struct RemoteGraph {
    reader: std::sync::Arc<CountingReader<XhrRangeReader>>,
    rete: Rete,
}

#[wasm_bindgen]
impl RemoteGraph {
    /// Open a remote `.rete` over HTTP range and keep it resident for repeated
    /// querying. The first query faults in the dictionary chunks + index tiles it
    /// needs; later queries on this handle reuse them and the block cache.
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Result<RemoteGraph, JsValue> {
        let (reader, rete) = open_url(url)?;
        Ok(RemoteGraph { reader, rete })
    }

    /// `{ fileLength, bytes, requests }` — CUMULATIVE physical fetches since this
    /// session opened. The worker diffs successive calls to report a single
    /// query's traffic (a fully cached re-run adds ~0).
    pub fn stats(&self) -> String {
        format!(
            r#"{{"fileLength":{},"bytes":{},"requests":{}}}"#,
            self.reader.len(),
            self.reader.bytes_read(),
            self.reader.requests()
        )
    }

    /// The file's content hash (blake3-16, hex). The worker keys its session
    /// cache by this rather than the URL, so two URLs of the same file share the
    /// cache — and it's the stable key a future IndexedDB block store (L3) needs
    /// to survive page reloads.
    pub fn content_hash(&self) -> String {
        self.rete
            .header()
            .content_hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// See [`sparql_url`] — same query, but over the resident, cached handle.
    pub fn query(&self, query: &str, format: &str) -> Result<String, JsValue> {
        let s = query_json(&self.rete, query, format, "")?;
        incomplete_guard(&self.rete, "query")?;
        Ok(s)
    }

    /// As [`query`], with OWL 2 QL entailment on (reason over the ontology while
    /// reading only the bytes the rewritten query touches).
    pub fn query_reasoned(&self, query: &str, format: &str) -> Result<String, JsValue> {
        let s = query_json_reasoned(&self.rete, query, format, "")?;
        incomplete_guard(&self.rete, "query")?;
        Ok(s)
    }

    /// See [`prefix_search`] — over the resident, cached remote handle. Faults the
    /// pyramid (where the label index lives) on the first call, then serves the
    /// search from memory.
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Result<String, JsValue> {
        prefix_search_json(&self.rete, prefix, limit)
    }

    /// See [`text_search`] — over the resident remote handle. Faults the TEXT_INDEX
    /// token table on the first call, then fetches only the queried posting lists
    /// (never the whole postings blob), serving repeat searches from memory.
    pub fn text_search(
        &self,
        words: Vec<String>,
        contains_prefix: Option<String>,
        limit: usize,
    ) -> Result<String, JsValue> {
        text_search_json(&self.rete, &words, contains_prefix.as_deref(), limit)
    }
}

/// Serialize a label prefix search as `[{"label":…,"subject":…}]`.
fn prefix_search_json(rete: &Rete, prefix: &str, limit: usize) -> Result<String, JsValue> {
    use serde_json::json;
    let hits: Vec<serde_json::Value> = rete
        .prefix_search(prefix, limit)
        .into_iter()
        .map(|(label, subject)| json!({ "label": label, "subject": subject }))
        .collect();
    serde_json::to_string(&hits).map_err(err)
}

/// Serialize a full-text (word/CONTAINS) search as `[{"subject":…}]`.
fn text_search_json(
    rete: &Rete,
    words: &[String],
    contains_prefix: Option<&str>,
    limit: usize,
) -> Result<String, JsValue> {
    use serde_json::json;
    let word_refs: Vec<&str> = words.iter().map(String::as_str).collect();
    let hits: Vec<serde_json::Value> = rete
        .text_search(&word_refs, contains_prefix, limit)
        .into_iter()
        .map(|subject| json!({ "subject": subject }))
        .collect();
    serde_json::to_string(&hits).map_err(err)
}

/// Parse just the 128-byte header and report the byte ranges a *progressive*
/// client needs for the overview — the dictionary and the pyramid summary — plus
/// the (large) index range it can skip. JSON:
/// `{ "dictOffset","dictLen","pyramidOffset","pyramidLen","indexOffset","indexLen" }`.
/// The browser fetches bytes 0..128, calls this, then range-fetches only the
/// dict + pyramid — never the index.
#[wasm_bindgen]
pub fn header_ranges(head: &[u8]) -> Result<String, JsValue> {
    let h = Header::from_bytes(head).map_err(err)?;
    Ok(format!(
        r#"{{"dictOffset":{},"dictLen":{},"pyramidOffset":{},"pyramidLen":{},"indexOffset":{},"indexLen":{}}}"#,
        h.dictionary_offset,
        h.dictionary_len,
        h.pyramid_meta_offset,
        h.pyramid_meta_len,
        h.root_dir_offset,
        h.root_dir_len,
    ))
}

/// Build the coarse-graph overview from a buffer in which only the header,
/// dictionary, and pyramid-summary ranges are populated — the index region may be
/// absent (zero-filled), because the summary path provably never reads it (see
/// the `ranged` test in rete-core). Returns JSON:
/// `{ "round", "communities", "predicateTotals": [["<iri>", count], ...] }`.
/// This is the "overview first, drill down later" payload, fetched in ~3 ranges.
#[wasm_bindgen]
pub fn summary_overview(bytes: &[u8]) -> Result<String, JsValue> {
    let view = SummaryView::open_ranged(&SliceReader::new(bytes))
        .map_err(err)?
        .ok_or_else(|| JsValue::from_str("file has no pyramid summary"))?;
    serde_json::to_string(&serde_json::json!({
        "round": view.round,
        "communities": view.community_count(),
        "predicateTotals": view.predicate_totals(),
    }))
    .map_err(err)
}

/// Answer a conservative subset of SPARQL exactly from the pyramid summary,
/// without opening the triple index. Unsupported query shapes return an error
/// instead of silently falling back to a full scan.
#[wasm_bindgen]
pub fn progressive_query(bytes: &[u8], query: &str) -> Result<String, JsValue> {
    progressive_query_json(bytes, query).map_err(|e| JsValue::from_str(&e))
}

/// Native-testable implementation for [`progressive_query`].
pub fn progressive_query_json(bytes: &[u8], query: &str) -> Result<String, String> {
    use serde_json::json;

    let shape = summary_query_shape(query)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "query is not exactly answerable from the summary".to_string())?;

    let reader = CountingReader::new(SliceReader::new(bytes));
    let file_bytes = reader.len();
    let view = SummaryView::open_ranged(&reader)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "file has no pyramid summary".to_string())?;

    let value = match shape {
        SummaryQueryShape::PredicateCount {
            predicate,
            variable,
        } => {
            let count = u64::from(view.predicate_total(&predicate));
            select_count_response(
                &variable,
                count,
                progressive_meta(
                    &reader,
                    file_bytes,
                    "predicate_count",
                    Some(&predicate),
                    json!(count),
                ),
            )
        }
        SummaryQueryShape::TripleCount { variable } => {
            let count = summary_total(&view);
            select_count_response(
                &variable,
                count,
                progressive_meta(&reader, file_bytes, "triple_count", None, json!(count)),
            )
        }
        SummaryQueryShape::PredicateTotals {
            predicate_variable,
            count_variable,
        } => {
            let totals = view.predicate_totals();
            select_predicate_totals_response(
                &predicate_variable,
                &count_variable,
                &totals,
                progressive_meta(
                    &reader,
                    file_bytes,
                    "predicate_totals",
                    None,
                    json!(&totals),
                ),
            )
        }
        SummaryQueryShape::PredicateList { variable } => {
            let predicates = predicate_list(&view);
            select_predicate_list_response(
                &variable,
                &predicates,
                progressive_meta(
                    &reader,
                    file_bytes,
                    "predicate_list",
                    None,
                    json!(&predicates),
                ),
            )
        }
        SummaryQueryShape::PredicateDistinctCount { variable } => {
            let count = predicate_count(&view);
            select_count_response(
                &variable,
                count,
                progressive_meta(
                    &reader,
                    file_bytes,
                    "predicate_distinct_count",
                    None,
                    json!(count),
                ),
            )
        }
        SummaryQueryShape::TripleExists => {
            let exists = summary_total(&view) > 0;
            json!({
                "kind": "ask",
                "boolean": exists,
                "progressive": progressive_meta(
                    &reader,
                    file_bytes,
                    "triple_exists",
                    None,
                    json!(exists),
                ),
            })
        }
        SummaryQueryShape::PredicateExists { predicate } => {
            let exists = view.predicate_total(&predicate) > 0;
            json!({
                "kind": "ask",
                "boolean": exists,
                "progressive": progressive_meta(
                    &reader,
                    file_bytes,
                    "predicate_exists",
                    Some(&predicate),
                    json!(exists),
                ),
            })
        }
    };

    serde_json::to_string(&value).map_err(|e| e.to_string())
}

/// Run any SPARQL form (SELECT / ASK / CONSTRUCT / DESCRIBE) and serialize the
/// result for the playground. Returns a JSON string the page parses; a `"kind"`
/// field (`"select"|"ask"|"construct"`) tells the UI how to render it.
///
/// `format` controls the construct serialization and the select fallback:
/// - SELECT → `{ "kind":"select", "vars":[...], "rows":[ {var:value,...} ] }`
///   (`format` is ignored for SELECT — the table view is always available).
/// - ASK → `{ "kind":"ask", "boolean": true|false }`.
/// - CONSTRUCT/DESCRIBE →
///   - `format=="ttl"`    → `{ "kind":"construct", "format":"ttl",    "text": "<turtle>" }`
///   - `format=="jsonld"` → `{ "kind":"construct", "format":"jsonld", "text": "<json-ld>" }`
///   - otherwise (`table`/`json`) → `{ "kind":"construct", "triples": [[s,p,o], ...] }`.
#[wasm_bindgen]
pub fn query(bytes: &[u8], query: &str, format: &str) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    query_json(&rete, query, format, "")
}

/// Prefix-search the label index of an embedded `.rete` image: the subjects whose
/// label starts with `prefix` (case-insensitive), as `[{"label":…,"subject":…}]`,
/// capped at `limit`. Answers from the bounded label-index block in the
/// pyramid-meta — no literal scan. Empty array when the file has no label index.
#[wasm_bindgen]
pub fn prefix_search(bytes: &[u8], prefix: &str, limit: usize) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    prefix_search_json(&rete, prefix, limit)
}

/// Full-text (word/CONTAINS) search over an embedded `.rete` image: the subjects
/// whose literals contain **every** word in `words` (whole-word, case-insensitive
/// — AND), optionally also a word starting with `contains_prefix`, as
/// `[{"subject":…}]`, capped at `limit`. Answers from the TEXT_INDEX section.
/// Empty array when the file has none (`build --text-index`).
#[wasm_bindgen]
pub fn text_search(
    bytes: &[u8],
    words: Vec<String>,
    contains_prefix: Option<String>,
    limit: usize,
) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    text_search_json(&rete, &words, contains_prefix.as_deref(), limit)
}

/// Evaluate any SPARQL form against an open [`Rete`] and serialize the playground
/// JSON envelope (shared by [`query`] and [`sparql_url`]). `extra` is a raw JSON
/// fragment of additional object members appended before the closing brace (e.g.
/// `,"remote":{…}` for [`sparql_url`]); pass `""` for none.
///
/// This writes the JSON **directly into a `String`** rather than building an
/// intermediate `serde_json::Value` tree and stringifying it: on a large SELECT
/// the tree path allocates ~25× the payload (a `BTreeMap` per row + two String
/// clones per cell) and costs more than the query itself; writing direct cuts the
/// serialization peak heap ~13× and the time ~10× (see `rete-bench --query-mem`).
fn query_json(rete: &Rete, query: &str, format: &str, extra: &str) -> Result<String, JsValue> {
    query_json_opt(rete, query, format, extra, false)
}

/// As [`query_json`], but with OWL 2 QL entailment on (the query is rewritten so
/// the answer includes ontology-entailed solutions, computed over the raw data).
fn query_json_reasoned(
    rete: &Rete,
    query: &str,
    format: &str,
    extra: &str,
) -> Result<String, JsValue> {
    query_json_opt(rete, query, format, extra, true)
}

fn query_json_opt(
    rete: &Rete,
    query: &str,
    format: &str,
    extra: &str,
    reason: bool,
) -> Result<String, JsValue> {
    let out = if reason {
        eval_query_reasoned(rete, query)
    } else {
        eval_query(rete, query)
    }
    .map_err(err)?;
    Ok(write_query_json(&out, format, extra))
}

/// Serialize an already-evaluated [`QueryOutput`] into the playground JSON
/// envelope. SELECT / ASK / `CONSTRUCT`-as-triples go through the shared,
/// host-tested `rete_core::results_envelope_json` (the allocation-lean direct
/// writer); a `CONSTRUCT` requested as Turtle / JSON-LD wraps the rendered text
/// (those serializers live here).
fn write_query_json(out: &QueryOutput, format: &str, extra: &str) -> String {
    if let QueryOutput::Construct(triples) = out {
        let text = match format {
            "ttl" => Some(("ttl", to_turtle(triples))),
            "jsonld" => Some(("jsonld", to_jsonld(triples))),
            _ => None,
        };
        if let Some((fmt, text)) = text {
            let mut s = String::from(r#"{"kind":"construct","format":""#);
            s.push_str(fmt);
            s.push_str(r#"","text":"#);
            rete_core::push_json_string(&mut s, &text);
            s.push_str(extra);
            s.push('}');
            return s;
        }
    }
    rete_core::results_envelope_json(out, extra)
}

/// HTTP `Range` reader over **synchronous** XMLHttpRequest — the bridge that
/// lets the lazily-faulting `Rete::open_ranged_lazy` run in the browser with
/// the synchronous engine untouched. Browsers permit sync XHR with a binary
/// response **only inside Web Workers**: call [`sparql_url`] from a worker,
/// never the main thread (where the browser throws).
struct XhrRangeReader {
    url: String,
    len: u64,
}

// Asyncify variant only (feature = "asyncify"): one dedicated async import in
// module "env". The worker provides it as a `Promise.all` of `fetch` over the N
// ranges, writing the spans concatenated into `dst` and returning the total
// bytes. `wasm-opt --asyncify --pass-arg=asyncify-imports@env.rete_fetch_ranges`
// instruments every fn that can reach it to SUSPEND the wasm while the worker
// awaits the fetches, then RESUME — concurrent reads with no SAB / cross-origin
// isolation. Proven in dev/asyncify-wbg-probe (wasm-bindgen × Asyncify).
#[cfg(feature = "asyncify")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn rete_fetch_ranges(
        url_ptr: *const u8,
        url_len: usize,
        offs_ptr: *const u64,
        lens_ptr: *const u32,
        n: usize,
        dst_ptr: *mut u8,
    ) -> usize;
    /// Async length probe (a `bytes=0-0` fetch; reads the total from
    /// `Content-Range`). Writes the u64 length to `out_ptr`, returns 1 on
    /// success. Lets the asyncify build open a file with NO sync XHR at all.
    fn rete_file_len(url_ptr: *const u8, url_len: usize, out_ptr: *mut u64) -> usize;
}

#[cfg(feature = "asyncify")]
impl XhrRangeReader {
    /// Fetch all `ranges` through the async import in one suspend/resume, then
    /// split the concatenated bytes back into per-range buffers.
    fn read_ranges_async(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let total: usize = ranges.iter().map(|&(_, l)| l as usize).sum();
        if total == 0 {
            return Ok(ranges.iter().map(|_| Vec::new()).collect());
        }
        let offs: Vec<u64> = ranges.iter().map(|&(o, _)| o).collect();
        let lens: Vec<u32> = ranges.iter().map(|&(_, l)| l as u32).collect();
        let mut dst = vec![0u8; total];
        let got = unsafe {
            rete_fetch_ranges(
                self.url.as_ptr(),
                self.url.len(),
                offs.as_ptr(),
                lens.as_ptr(),
                ranges.len(),
                dst.as_mut_ptr(),
            )
        };
        if got != total {
            return Err(std::io::Error::other(format!(
                "async fetch returned {got} of {total} bytes for {}",
                self.url
            )));
        }
        report_progress(total);
        let mut out = Vec::with_capacity(ranges.len());
        let mut pos = 0usize;
        for &(_, l) in ranges {
            let end = pos + l as usize;
            out.push(dst[pos..end].to_vec());
            pos = end;
        }
        Ok(out)
    }
}

impl XhrRangeReader {
    /// Probe the resource length. Some hosts reject `HEAD` (Hugging Face's
    /// signed-redirect storage answers `405`), so use a one-byte ranged `GET`
    /// and read the total from the `Content-Range` header (`bytes 0-0/TOTAL`),
    /// falling back to `Content-Length` if the host doesn't send a range.
    fn open(url: &str) -> Result<Self, JsValue> {
        // Asyncify build: probe the length via the async import — no sync XHR.
        #[cfg(feature = "asyncify")]
        {
            let mut len: u64 = 0;
            let ok = unsafe { rete_file_len(url.as_ptr(), url.len(), &mut len as *mut u64) };
            if ok == 0 || len == 0 {
                return Err(JsValue::from_str(&format!(
                    "could not determine length of {url}"
                )));
            }
            return Ok(Self {
                url: url.to_string(),
                len,
            });
        }
        #[cfg(not(feature = "asyncify"))]
        {
            // Prefer a HEAD: its `Content-Length` is the full file size AND is a
            // CORS-safelisted response header, so it is readable cross-origin even when
            // the host does NOT expose `Content-Range` (e.g. Zenodo). The ranged-GET
            // probe below cannot see a hidden `Content-Range` and would mis-read the
            // 1-byte partial `Content-Length` as the size ("range out of bounds"). Falls
            // through when the host rejects HEAD (HF's signed-redirect storage 405s it).
            if let Some(len) = Self::head_len(url) {
                return Ok(Self {
                    url: url.to_string(),
                    len,
                });
            }
            // Hugging Face's Space gateway is intermittently flaky on the length
            // probe (a 200 with no Content-Length, a chunked response with neither
            // header, …). A fresh ranged GET usually lands on a healthy response,
            // so retry a few times before surfacing the error.
            let mut last = format!("could not determine length of {url}");
            for _ in 0..4 {
                match Self::probe_len(url) {
                    Ok(len) => {
                        return Ok(Self {
                            url: url.to_string(),
                            len,
                        })
                    }
                    Err(e) => last = e,
                }
            }
            Err(JsValue::from_str(&last))
        }
    }

    /// A HEAD length probe: `Content-Length` is the full size and is CORS-safelisted
    /// (readable cross-origin with no `access-control-expose-headers` entry needed).
    /// Returns `None` on a non-2xx (some hosts 405 HEAD) or a missing/zero length, so
    /// the caller can fall back to the ranged-GET probe.
    #[cfg(not(feature = "asyncify"))]
    fn head_len(url: &str) -> Option<u64> {
        let xhr = web_sys::XmlHttpRequest::new().ok()?;
        xhr.open_with_async("HEAD", url, false).ok()?;
        xhr.send().ok()?;
        let status = xhr.status().ok()?;
        if !(200..300).contains(&status) {
            return None;
        }
        xhr.get_response_header("Content-Length")
            .ok()
            .flatten()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&n| n > 0)
    }

    /// One length probe: a one-byte ranged `GET`, reading the total from
    /// `Content-Range` (`bytes 0-0/TOTAL`), falling back to `Content-Length`.
    #[cfg(not(feature = "asyncify"))]
    fn probe_len(url: &str) -> Result<u64, String> {
        let err = |m: &str| m.to_string();
        let xhr = web_sys::XmlHttpRequest::new().map_err(|_| err("xhr"))?;
        xhr.open_with_async("GET", url, false)
            .map_err(|_| err("open"))?;
        xhr.set_response_type(web_sys::XmlHttpRequestResponseType::Arraybuffer);
        xhr.set_request_header("Range", "bytes=0-0")
            .map_err(|_| err("range header"))?;
        xhr.send()
            .map_err(|_| format!("probe {url}: network error"))?;
        let status = xhr.status().map_err(|_| err("status"))?;
        if status != 206 && !(200..300).contains(&status) {
            return Err(format!("probe {url}: status {status}"));
        }
        // `Content-Range: bytes 0-0/12345` — the part after `/` is the total.
        xhr.get_response_header("Content-Range")
            .ok()
            .flatten()
            .and_then(|v| {
                v.rsplit('/')
                    .next()
                    .and_then(|t| t.trim().parse::<u64>().ok())
            })
            .or_else(|| {
                xhr.get_response_header("Content-Length")
                    .ok()
                    .flatten()
                    .and_then(|v| v.parse::<u64>().ok())
            })
            .ok_or_else(|| format!("could not determine length of {url}"))
    }
}

impl RangeReader for XhrRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        // Asyncify build: a single read is a 1-range async fetch (suspends).
        #[cfg(feature = "asyncify")]
        {
            return Ok(self
                .read_ranges_async(&[(offset, len)])?
                .into_iter()
                .next()
                .unwrap());
        }
        #[cfg(not(feature = "asyncify"))]
        {
            let js = |e: JsValue| std::io::Error::other(format!("XHR error: {e:?}"));
            let xhr = web_sys::XmlHttpRequest::new().map_err(js)?;
            xhr.open_with_async("GET", &self.url, false).map_err(js)?;
            xhr.set_response_type(web_sys::XmlHttpRequestResponseType::Arraybuffer);
            let end = offset + len - 1; // HTTP ranges are inclusive
            xhr.set_request_header("Range", &format!("bytes={offset}-{end}"))
                .map_err(js)?;
            xhr.send().map_err(js)?;
            let status = xhr.status().map_err(js)?;
            // Same contract as the CLI's HttpRangeReader: a 200 means the server
            // ignored Range and is returning the whole body — reject it loudly
            // rather than silently mis-slicing.
            if status != 206 {
                return Err(std::io::Error::other(format!(
                    "server ignored Range (status {status}, expected 206 Partial Content) for {}; \
                 the host must support HTTP range requests",
                    self.url
                )));
            }
            let resp = xhr.response().map_err(js)?;
            let mut buf = js_sys::Uint8Array::new(&resp).to_vec();
            if (buf.len() as u64) < len {
                return Err(std::io::Error::other(format!(
                    "short range response: got {} of {len} bytes at offset {offset} from {}",
                    buf.len(),
                    self.url
                )));
            }
            buf.truncate(len as usize);
            // Report this fetch to an optional progress hook so a worker can stream
            // live "N requests · M bytes" updates to the UI *during* the otherwise
            // opaque synchronous query (postMessage works mid-sync-call).
            report_progress(buf.len());
            Ok(buf)
        }
    }

    /// Synchronous XHR can't run two requests at once on one thread, so the
    /// engine's sequential faults serialize their round trips. If the page
    /// installs `globalThis.reteReadMany(url, offsets, lens)` — a fetch-worker
    /// pool that fetches the ranges in parallel and blocks (via SAB/Atomics)
    /// until done — use it; it returns one buffer with the spans concatenated
    /// in order, or `null`/throws if it can't (no cross-origin isolation, a
    /// non-206, a short read). On `null` we fall back to the sequential reads
    /// below, which keep the rigorous per-range validation.
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        // Asyncify build: fetch the whole batch concurrently in one suspend.
        #[cfg(feature = "asyncify")]
        {
            return self.read_ranges_async(ranges);
        }
        #[cfg(not(feature = "asyncify"))]
        {
            if ranges.len() > 1 {
                if let Some(buf) = self.read_many_via_pool(ranges) {
                    let total: u64 = ranges.iter().map(|&(_, l)| l).sum();
                    if buf.len() as u64 == total {
                        let mut out = Vec::with_capacity(ranges.len());
                        let mut pos = 0usize;
                        for &(_, l) in ranges {
                            let end = pos + l as usize;
                            out.push(buf[pos..end].to_vec());
                            pos = end;
                        }
                        return Ok(out);
                    }
                }
            }
            ranges.iter().map(|&(o, l)| self.read_at(o, l)).collect()
        }
    }
}

/// Notify an optional `globalThis.reteProgress(bytes)` hook of one completed
/// range fetch (the worker forwards it to the UI). A no-op when unset.
fn report_progress(bytes: usize) {
    let g = js_sys::global();
    if let Ok(f) = js_sys::Reflect::get(&g, &JsValue::from_str("reteProgress")) {
        if let Ok(f) = f.dyn_into::<js_sys::Function>() {
            let _ = f.call1(&JsValue::NULL, &JsValue::from_f64(bytes as f64));
        }
    }
}

#[cfg(not(feature = "asyncify"))]
impl XhrRangeReader {
    /// Call the optional JS parallel-fetch hook. Returns the concatenated span
    /// bytes on full success, or `None` if the hook is absent, threw, or
    /// returned anything other than a `Uint8Array` (→ sequential fallback).
    fn read_many_via_pool(&self, ranges: &[(u64, u64)]) -> Option<Vec<u8>> {
        let global = js_sys::global();
        let hook = js_sys::Reflect::get(&global, &JsValue::from_str("reteReadMany")).ok()?;
        if !hook.is_function() {
            return None;
        }
        let hook = hook.dyn_into::<js_sys::Function>().ok()?;
        let offs = js_sys::Float64Array::new_with_length(ranges.len() as u32);
        let lens = js_sys::Float64Array::new_with_length(ranges.len() as u32);
        for (i, &(o, l)) in ranges.iter().enumerate() {
            offs.set_index(i as u32, o as f64);
            lens.set_index(i as u32, l as f64);
        }
        let res = hook
            .call3(&JsValue::NULL, &JsValue::from_str(&self.url), &offs, &lens)
            .ok()?;
        if res.is_null() || res.is_undefined() {
            return None;
        }
        Some(js_sys::Uint8Array::new(&res).to_vec())
    }
}

/// The browser's [`rete_core::ServiceClient`]: a `SERVICE <endpoint> { … }`
/// block POSTs its sub-query to the endpoint over synchronous XHR (worker-only,
/// like every other read here — the engine is synchronous) and rete-core parses
/// the SPARQL JSON results. The endpoint must send CORS headers (the public
/// ones — Wikidata, DBpedia — do).
struct XhrServiceClient;

impl rete_core::ServiceClient for XhrServiceClient {
    fn query(&self, endpoint: &str, query: &str) -> Result<Vec<rete_core::Binding>, String> {
        let e = |m: &str| format!("{endpoint}: {m}");
        let xhr = web_sys::XmlHttpRequest::new().map_err(|_| e("xhr"))?;
        xhr.open_with_async("POST", endpoint, false)
            .map_err(|_| e("open"))?;
        xhr.set_request_header("Content-Type", "application/x-www-form-urlencoded")
            .map_err(|_| e("header"))?;
        xhr.set_request_header("Accept", "application/sparql-results+json")
            .map_err(|_| e("header"))?;
        let body = format!(
            "query={}",
            String::from(js_sys::encode_uri_component(query))
        );
        xhr.send_with_opt_str(Some(&body))
            .map_err(|_| e("network error (endpoint down or no CORS)"))?;
        let status = xhr.status().map_err(|_| e("status"))?;
        if !(200..300).contains(&status) {
            return Err(e(&format!("HTTP {status}")));
        }
        let text = xhr
            .response_text()
            .map_err(|_| e("read"))?
            .unwrap_or_default();
        rete_core::parse_sparql_json_results(&text).map_err(|m| e(&m))
    }
}

/// Run a SPARQL query against a **remote `.rete` URL**, fetching only the
/// byte ranges the query needs: header, dictionary chunk directories, tile
/// directories, then just the touched chunks and tiles (full scans coalesce
/// adjacent tiles into batched range reads). Returns the same JSON envelope
/// as [`query`], plus `"remote": { "fileLength", "bytes", "requests" }` —
/// how little of the file the query actually pulled.
///
/// **Worker-only.** This uses synchronous XHR (the engine is synchronous and
/// wasm cannot block on `fetch`); browsers allow that off the main thread
/// only, so call it from a Web Worker. A failed range fetch mid-query is an
/// error, never a silently incomplete result. The host must answer `Range`
/// requests with `206` (and send CORS headers if cross-origin).
/// Open a remote `.rete` lazily over HTTP range reads, returning the counting
/// reader (for byte/request stats) and the `Rete`. The seam every `*_url`
/// task shares with [`sparql_url`].
/// Auto-tune the block-cache size from the FILE SIZE — known for free at open
/// from the `Content-Range` of the single `bytes=0-0` request (one byte, no
/// download; it's what `stats().fileLength` reports). Remote reads are
/// round-trip-bound, so a bigger block means far fewer requests; benchmarked on
/// wikidata-1GB: 64 KiB = 262 reqs / 63 s, 256 KiB = 83 / 27 s, 512 KiB = 51 / 19 s.
/// Bigger files (bigger working sets + dictionaries) get bigger blocks as
/// multiples of the 64 KiB `DEFAULT_BLOCK`; small files keep over-fetch modest.
/// Per-query override comes via `RemoteGraph::with_block` (the Settings control).
fn auto_block(len: u64) -> u64 {
    const MB: u64 = 1 << 20;
    let mult: u64 = if len > 100 * MB {
        8 // 512 KiB — chebi, wikidata-100mb, ohm-full, wikidata-1GB, causenet
    } else if len > 10 * MB {
        4 // 256 KiB
    } else {
        2 // 128 KiB — small remotes (chemotion, getty-ulan)
    };
    mult * DEFAULT_BLOCK
}

fn open_url(url: &str) -> Result<(std::sync::Arc<CountingReader<XhrRangeReader>>, Rete), JsValue> {
    // `reader` counts the PHYSICAL fetches; a read-through block cache above it
    // turns the query's scattered range reads into a few aligned block fetches
    // (and reuses them) — working over any single-range backend (S3, a CDN),
    // not just a multi-range gateway. The block fetches still go through
    // `read_many`, so a multi-range host coalesces them further. The block size
    // is auto-tuned from the file size (known at open, no download).
    let reader = std::sync::Arc::new(CountingReader::new(XhrRangeReader::open(url)?));
    let cached = std::sync::Arc::new(BlockCacheReader::new(
        reader.clone(),
        auto_block(reader.len()),
    ));
    let mut rete = Rete::open_ranged_lazy(cached).map_err(err)?;
    // SERVICE blocks federate to remote SPARQL endpoints over sync XHR.
    rete.set_service_client(Box::new(XhrServiceClient));
    Ok((reader, rete))
}

/// Turn a partial lazy fetch into a hard error: a `*_url` task must never
/// return a result computed over silently-incomplete data.
fn incomplete_guard(rete: &Rete, what: &str) -> Result<(), JsValue> {
    if rete.index_incomplete() {
        return Err(JsValue::from_str(&format!(
            "a range fetch failed mid-{what}; refusing to return incomplete results"
        )));
    }
    Ok(())
}

/// Triple-pattern provenance over a **remote** `.rete` URL (lazy range reads):
/// which permutation/section/byte-ranges answer the pattern. Worker-only.
#[wasm_bindgen]
pub fn why_url(
    url: &str,
    subject: Option<String>,
    predicate: Option<String>,
    object: Option<String>,
) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let out = why_triples_rete(
        &rete,
        subject.as_deref(),
        predicate.as_deref(),
        object.as_deref(),
    )
    .map_err(|e| JsValue::from_str(&e))?;
    incomplete_guard(&rete, "query")?;
    let _ = reader;
    Ok(out)
}

/// Provenance over an already-open `Rete` (shared by [`why_triples`] in-memory
/// and [`why_url`] remote).
fn why_triples_rete(
    rete: &Rete,
    subject: Option<&str>,
    predicate: Option<&str>,
    object: Option<&str>,
) -> Result<String, String> {
    use serde_json::json;
    let results = rete.query_with_provenance(subject, predicate, object);
    let out = json!({
        "pattern": { "subject": subject, "predicate": predicate, "object": object },
        "resultCount": results.len(),
        "results": results.iter().map(provenance_json).collect::<Vec<_>>(),
    });
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

#[wasm_bindgen]
pub fn sparql_url(url: &str, query: &str, format: &str) -> Result<String, JsValue> {
    // Use the block-caching `open_url` seam so scattered range reads within this
    // one query that land in the same 64 KiB block fetch it once. (The resident
    // `RemoteGraph` extends this reuse across queries.)
    let (reader, rete) = open_url(url)?;
    let file_length = reader.len();
    // Evaluate first (this is what faults tiles), then refuse incomplete results.
    let out = eval_query(&rete, query).map_err(err)?;
    if rete.index_incomplete() {
        return Err(JsValue::from_str(
            "a range fetch failed mid-query; refusing to return incomplete results",
        ));
    }
    // The `remote` member's values are plain numbers — no escaping needed.
    let extra = format!(
        r#","remote":{{"fileLength":{},"bytes":{},"requests":{}}}"#,
        file_length,
        reader.bytes_read(),
        reader.requests(),
    );
    Ok(write_query_json(&out, format, &extra))
}

/// Evaluate a SELECT with the **community-split strategy**: every basic graph
/// pattern is decomposed into subject stars, each star is evaluated per
/// pyramid community (the members pushed in as a VALUES binding), and the
/// stars are recombined with global hash joins — so multi-hop joins work and
/// cross-community solutions survive. FILTER / UNION / OPTIONAL / MINUS
/// recurse; paths and GRAPH blocks evaluate globally inside the split; GROUP
/// BY / ORDER BY / LIMIT / DISTINCT run once on the merged rows. Answers are
/// identical to [`query`]'s. Refused only when nothing in the query can
/// split (no BGP with a variable subject) or for FROM / FROM NAMED. JSON:
/// the SELECT envelope plus `"communities": [{ "community", "subjects",
/// "rows" }, …]` (rows contributed per community across all split stars).
#[wasm_bindgen]
pub fn query_communities(
    bytes: &[u8],
    query: &str,
    round: Option<usize>,
) -> Result<String, JsValue> {
    query_communities_value(&open(bytes)?, query, round)
}

/// [`query_communities`] against an already-open [`Rete`] (shared with [`Graph`]).
fn query_communities_value(
    rete: &Rete,
    query: &str,
    round: Option<usize>,
) -> Result<String, JsValue> {
    use serde_json::{json, Map, Value};
    let (mut vars, solutions, partials) =
        eval_select_communities(rete, query, round).map_err(err)?;
    if vars.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        for s in &solutions {
            for k in s.keys() {
                if seen.insert(k.clone()) {
                    vars.push(k.clone());
                }
            }
        }
    }
    let rows: Vec<Value> = solutions
        .iter()
        .map(|s| {
            let mut obj = Map::new();
            for var in &vars {
                if let Some(term) = s.get(var) {
                    obj.insert(var.clone(), Value::String(term.clone()));
                }
            }
            Value::Object(obj)
        })
        .collect();
    let communities: Vec<Value> = partials
        .iter()
        .map(|p| json!({ "community": p.community, "subjects": p.subjects, "rows": p.rows }))
        .collect();
    serde_json::to_string(
        &json!({ "kind": "select", "vars": vars, "rows": rows, "communities": communities }),
    )
    .map_err(err)
}

/// The file's byte layout, for the playground's byte-map view. JSON:
/// `{ "fileLength": N, "segments": [ { "kind", "label", "offset", "len" } ] }`
/// — segments sorted by offset; uncovered bytes are container framing.
#[wasm_bindgen]
pub fn file_layout(bytes: &[u8]) -> Result<String, JsValue> {
    use serde_json::json;
    let rete = open(bytes)?;
    let segments: Vec<serde_json::Value> = rete
        .file_layout()
        .iter()
        .map(|s| json!({ "kind": s.kind, "label": s.label, "offset": s.offset, "len": s.len }))
        .collect();
    serde_json::to_string(&json!({ "fileLength": bytes.len(), "segments": segments })).map_err(err)
}

/// The full community pyramid as a tree — the "cluster of clusters" view.
/// Per dendrogram round (index 0 = finest, last = coarsest), every community
/// with its member-node count, triple count (triples whose subject belongs to
/// it), and its parent community at the next-coarser round (`null` at the
/// top). JSON:
/// `{ "rounds": N, "levels": [ [ { "id", "nodes", "triples", "parent" } ] ] }`.
#[wasm_bindgen]
pub fn pyramid_tree(bytes: &[u8]) -> Result<String, JsValue> {
    pyramid_tree_value(&open(bytes)?)
}

/// [`pyramid_tree`] against an already-open [`Rete`] (shared with [`Graph`]).
fn pyramid_tree_value(rete: &Rete) -> Result<String, JsValue> {
    use serde_json::json;
    use std::collections::BTreeMap;
    let dict = rete.dictionary();
    let ids = rete.match_ids((None, None, None));
    let g = project_graph(dict, &ids);
    let dend = build_dendrogram(&g);
    let rounds = dend.rounds();
    if rounds == 0 {
        return serde_json::to_string(&json!({ "rounds": 0, "levels": [] })).map_err(err);
    }
    let n = g.node_count();
    let mut levels = Vec::with_capacity(rounds);
    for r in 0..rounds {
        let mut nodes_per: BTreeMap<usize, usize> = BTreeMap::new();
        let mut rep: BTreeMap<usize, usize> = BTreeMap::new();
        for node in 0..n {
            let c = dend.base_community(node, r);
            *nodes_per.entry(c).or_default() += 1;
            rep.entry(c).or_insert(node);
        }
        let mut triples_per: BTreeMap<usize, usize> = BTreeMap::new();
        for &(s, _, _) in &ids {
            let c = dend.base_community(dict.subject_node(s) as usize, r);
            *triples_per.entry(c).or_default() += 1;
        }
        let level: Vec<serde_json::Value> = nodes_per
            .iter()
            .map(|(&c, &nodes)| {
                let parent = if r + 1 < rounds {
                    json!(dend.base_community(rep[&c], r + 1))
                } else {
                    json!(null)
                };
                json!({
                    "id": c,
                    "nodes": nodes,
                    "triples": triples_per.get(&c).copied().unwrap_or(0),
                    "parent": parent,
                })
            })
            .collect();
        levels.push(level);
    }
    serde_json::to_string(&json!({ "rounds": rounds, "levels": levels })).map_err(err)
}

/// Recompute the Louvain community decomposition and report, per community, its
/// member-subject count and triple count. Powers the "split by community"
/// strategy view in the playground. JSON:
/// `[{ "community": N, "size": M, "triples": K }, ...]`, ordered by community id.
///
/// `round` selects the dendrogram granularity; `None` chooses the round for the
/// default tile budget (the same choice the native build makes).
#[wasm_bindgen]
pub fn communities(bytes: &[u8], round: Option<usize>) -> Result<String, JsValue> {
    use std::collections::HashSet;
    let rete = open(bytes)?;
    let dict = rete.dictionary();
    let ids = rete.match_ids((None, None, None));
    let g = project_graph(dict, &ids);
    let dend = build_dendrogram(&g);
    let round =
        round.unwrap_or_else(|| choose_round_for_budget(dict, &ids, &dend, DEFAULT_TILE_BUDGET));
    let tiles = tile_by_community(dict, &ids, &dend, round);
    let arr: Vec<serde_json::Value> = tiles
        .iter()
        .map(|t| {
            let members: HashSet<u32> = t.triples.iter().map(|&(s, _, _)| s).collect();
            serde_json::json!({
                "community": t.community,
                "size": members.len(),
                "triples": t.triples.len(),
            })
        })
        .collect();
    serde_json::to_string(&arr).map_err(err)
}

/// Multi-source transitive reachability over one relation, run **serially**
/// (the browser engine is single-threaded — the native CLI's
/// `rete reach --parallel` fans one task per seed). For each seed, the set of
/// nodes it transitively reaches; with `reverse`, the set that reaches it
/// (impact analysis).
///
/// - `predicate` — the relation IRI token, e.g. `<http://ex/dependsOn>`.
/// - `seeds` — a JSON array of seed IRI tokens (e.g. `["<http://ex/app>"]`); a
///   bare single IRI string is also accepted.
/// - `reverse` — traverse edges backward ("who reaches the seed?").
///
/// Returns a JSON array, one entry per seed in input order:
/// `[{ "seed":"<iri>", "reached":["<iri>",...], "count":N }, ...]`.
/// A seed not present in the graph yields `{ "seed":"...", "error":"not in graph" }`
/// instead of failing the whole call.
#[wasm_bindgen]
pub fn reach(bytes: &[u8], predicate: &str, seeds: &str, reverse: bool) -> Result<String, JsValue> {
    reach_rete(&open(bytes)?, predicate, seeds, reverse)
}

/// Multi-source reachability over a **remote** `.rete` URL (lazy HTTP range
/// reads): builds adjacency for `predicate` by faulting only that predicate's
/// tiles, then BFS from each seed. Worker-only (synchronous XHR).
#[wasm_bindgen]
pub fn reach_url(
    url: &str,
    predicate: &str,
    seeds: &str,
    reverse: bool,
) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let out = reach_rete(&rete, predicate, seeds, reverse)?;
    incomplete_guard(&rete, "reach")?;
    let _ = reader;
    Ok(out)
}

fn reach_rete(rete: &Rete, predicate: &str, seeds: &str, reverse: bool) -> Result<String, JsValue> {
    use std::collections::HashMap;
    let dict = rete.dictionary();

    // Accept either a JSON array of IRIs or a single bare IRI string.
    let seed_iris: Vec<String> = match serde_json::from_str::<Vec<String>>(seeds) {
        Ok(v) => v,
        Err(_) => match serde_json::from_str::<String>(seeds) {
            Ok(s) => vec![s],
            Err(_) => vec![seeds.to_string()],
        },
    };

    // Adjacency in unified node space for the chosen direction.
    let adj: HashMap<u32, Vec<u32>> = if reverse {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for (s, o) in rete.predicate_pairs(predicate) {
            m.entry(o).or_default().push(s); // who points at o
        }
        m
    } else {
        build_adjacency(rete, predicate)
    };

    // Resolve known seeds to node ids; remember unknowns to report inline.
    let mut known_idx: Vec<usize> = Vec::new();
    let mut seed_nodes: Vec<u32> = Vec::new();
    for (i, iri) in seed_iris.iter().enumerate() {
        if let Some(n) = dict.node_of_term(iri) {
            known_idx.push(i);
            seed_nodes.push(n);
        }
    }

    let sets = batch_reach_serial(&adj, &seed_nodes);

    // Map known-seed position -> its reach set, then assemble in input order.
    let mut by_input: HashMap<usize, &std::collections::BTreeSet<u32>> = HashMap::new();
    for (k, &i) in known_idx.iter().enumerate() {
        by_input.insert(i, &sets[k]);
    }

    let arr: Vec<serde_json::Value> = seed_iris
        .iter()
        .enumerate()
        .map(|(i, iri)| match by_input.get(&i) {
            Some(set) => {
                let reached: Vec<String> = set.iter().filter_map(|&n| dict.node_term(n)).collect();
                serde_json::json!({
                    "seed": iri,
                    "count": reached.len(),
                    "reached": reached,
                })
            }
            None => serde_json::json!({ "seed": iri, "error": "not in graph" }),
        })
        .collect();

    serde_json::to_string(&arr).map_err(err)
}

/// Validate a `.rete` graph against SHACL Core shapes written in Turtle.
///
/// The default graph is validated unless `graph` names a dataset graph IRI.
/// `format` is one of:
/// - `"json"`: structured validation report from rete-core
/// - `"ttl"`: Turtle validation report
/// - anything else: compact text report
///
/// A non-conformant graph returns a report; it is not a JS exception. Exceptions
/// are reserved for parse/open errors.
#[wasm_bindgen]
pub fn shacl(
    bytes: &[u8],
    shapes_turtle: &str,
    graph: Option<String>,
    format: &str,
) -> Result<String, JsValue> {
    shacl_rete(&open(bytes)?, shapes_turtle, graph.as_deref(), format)
}

/// Validate a **remote** `.rete` graph (lazy HTTP range reads) against SHACL
/// Core shapes written in Turtle. Validating the **default** graph routes every
/// lookup as a range read ([`ReteGraph`]), so a targeted shape faults only the
/// tiles holding its targets — not the whole graph. A named graph (`graph`) still
/// materializes (the routed view is default-graph only). Worker-only (sync XHR).
#[wasm_bindgen]
pub fn shacl_url(
    url: &str,
    shapes_turtle: &str,
    graph: Option<String>,
    format: &str,
) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let out = shacl_rete(&rete, shapes_turtle, graph.as_deref(), format)?;
    incomplete_guard(&rete, "validation")?;
    let _ = reader;
    Ok(out)
}

fn shacl_rete(
    rete: &Rete,
    shapes_turtle: &str,
    graph: Option<&str>,
    format: &str,
) -> Result<String, JsValue> {
    let shapes = ShaclShapes::parse_turtle(shapes_turtle).map_err(err)?;
    // Default graph: validate over the index directly (lazy — fetch only targets).
    // A named graph still materializes (ReteGraph views the default graph only).
    let report = match graph {
        None => validate_shacl(&ReteGraph::new(rete), &shapes),
        Some(g) => validate_shacl(&DataGraph::from_rete(rete, Some(g)), &shapes),
    };
    Ok(format_report(&report, format))
}

fn shacl_over(data: DataGraph, shapes_turtle: &str, format: &str) -> Result<String, JsValue> {
    let shapes = ShaclShapes::parse_turtle(shapes_turtle).map_err(err)?;
    Ok(format_report(&validate_shacl(&data, &shapes), format))
}

fn format_report(report: &ValidationReport, format: &str) -> String {
    match format {
        "json" => report.to_json(),
        "ttl" => report.to_turtle(),
        _ => format_shacl_text(report),
    }
}

/// **Chain a SPARQL subset, then SHACL over it.** Evaluates a `CONSTRUCT` over
/// the remote `.rete` (touching only the tiles its patterns need), then validates
/// the resulting subgraph against the Turtle `shapes`. Where [`shacl_url`] selects
/// by *shape target* (validate every Person…), this selects by an explicit
/// `CONSTRUCT` — "validate just the slice this query carves out, in place".
/// Worker-only (synchronous XHR).
#[wasm_bindgen]
pub fn shacl_construct_url(
    url: &str,
    construct: &str,
    shapes_turtle: &str,
    format: &str,
) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let triples = match eval_query(&rete, construct).map_err(err)? {
        QueryOutput::Construct(t) => t,
        _ => {
            return Err(JsValue::from_str(
                "the subset query must be a CONSTRUCT — it builds the subgraph SHACL validates",
            ))
        }
    };
    incomplete_guard(&rete, "query")?;
    let out = shacl_over(DataGraph::from_triples(triples), shapes_turtle, format)?;
    let _ = reader;
    Ok(out)
}

// --- OWL RL / RDFS coherence checking ----------------------------------------
//
// The reasoner (`rete_core::reason`) is already slice-shaped, so these are near
// twins of the SHACL functions above: `reason`/`reason_url` materialize the whole
// graph (Tier-2, the complete check), while `reason_construct_url` validates only
// the slice a CONSTRUCT selects (Tier-1, selective range reads).

/// The fixed, **selective** coherence subgraph for [`reason_construct_url`]
/// (Tier-1). A UNION of constant-predicate branches: each routes to exactly one
/// predicate's index tiles, so the client faults only `rdf:type` + the class/
/// equality T-Box predicates over one warm lazy cache — not the whole graph.
///
/// The T-Box branches are MANDATORY: the reasoner finds disjoint-class clashes
/// only *after* `subClassOf` type-propagation, so omitting `subClassOf`/
/// `disjointWith`/`sameAs` would silently miss propagation-dependent
/// contradictions. Property-characteristic axioms (Functional/Symmetric/
/// Transitive) ride inside the `rdf:type` slice. Instance-level
/// FunctionalProperty / domain / range clashes need the property's own
/// assertions — use [`reason_url`] (Tier-2) for those.
pub const COHERENCE_CONSTRUCT: &str = r#"
PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX owl: <http://www.w3.org/2002/07/owl#>
CONSTRUCT {
  ?x rdf:type ?c .
  ?sub rdfs:subClassOf ?sup .
  ?c1 owl:disjointWith ?c2 .
  ?s1 owl:sameAs ?s2 .
  ?f1 owl:differentFrom ?f2 .
}
WHERE {
  { ?x rdf:type ?c }
  UNION { ?sub rdfs:subClassOf ?sup }
  UNION { ?c1 owl:disjointWith ?c2 }
  UNION { ?s1 owl:sameAs ?s2 }
  UNION { ?f1 owl:differentFrom ?f2 }
}
"#;

/// Build the playground JSON envelope for a [`rete_core::Reasoning`] result,
/// shared by the in-memory and remote entrypoints. `remote` is
/// `(fileLength, bytes, requests)` for the `*_url` variants, `None` in-memory.
/// JSON: `{ "kind":"reasoning", "coherent":bool, "inferredCount":N,
/// "inconsistencies":[{"kind","detail"}], ["remote":{...}] }`.
fn reasoning_json(r: &rete_core::Reasoning, remote: Option<(u64, u64, u64)>) -> String {
    use serde_json::json;
    let inconsistencies: Vec<serde_json::Value> = r
        .inconsistencies
        .iter()
        .map(|i| json!({ "kind": i.kind, "detail": i.detail }))
        .collect();
    let mut v = json!({
        "kind": "reasoning",
        "coherent": r.inconsistencies.is_empty(),
        "inferredCount": r.inferred.len(),
        "inconsistencies": inconsistencies,
    });
    if let (Some(obj), Some((file_length, bytes, requests))) = (v.as_object_mut(), remote) {
        obj.insert(
            "remote".to_string(),
            json!({ "fileLength": file_length, "bytes": bytes, "requests": requests }),
        );
    }
    v.to_string()
}

/// Run the OWL RL / RDFS reasoner over an **in-memory** `.rete` and report the
/// inferred-triple count plus any incoherent points (logical contradictions).
/// `graph` selects a named graph; the default graph is used when omitted. This is
/// the complete (Tier-2) check — it materializes the whole graph. Returns the
/// [`reasoning_json`] envelope (no `remote` block).
#[wasm_bindgen]
pub fn reason(bytes: &[u8], graph: Option<String>) -> Result<String, JsValue> {
    let rete = open(bytes)?;
    let base = rete.dump(graph.as_deref());
    let result = rete_core::reason(&base);
    Ok(reasoning_json(&result, None))
}

/// Run the reasoner over a **remote** `.rete` URL — the complete (Tier-2) check.
/// It materializes the whole graph, so this faults in the dataset's chunks/tiles
/// as it reads (≈ the whole file); use [`reason_construct_url`] for the cheaper
/// selective check. Worker-only (synchronous XHR). A failed range fetch mid-read
/// is an error, never a silently-incomplete (and thus possibly false "coherent")
/// result. JSON adds `"remote": { fileLength, bytes, requests }`.
#[wasm_bindgen]
pub fn reason_url(url: &str, graph: Option<String>) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let base = rete.dump(graph.as_deref());
    let result = rete_core::reason(&base);
    incomplete_guard(&rete, "reasoning")?;
    let out = reasoning_json(
        &result,
        Some((reader.len(), reader.bytes_read(), reader.requests())),
    );
    let _ = reader;
    Ok(out)
}

/// **Selective (Tier-1) coherence check** over a remote `.rete`: evaluate a
/// CONSTRUCT (touching only the tiles its constant-predicate patterns need), then
/// reason over just that subgraph. Pass [`COHERENCE_CONSTRUCT`] for the standard
/// class/equality coherence slice, or a custom CONSTRUCT to scope the check
/// further. Unlike [`reason_url`] (which materializes the whole graph), this
/// fetches only the slice the CONSTRUCT selects. Worker-only (synchronous XHR).
#[wasm_bindgen]
pub fn reason_construct_url(url: &str, construct: &str) -> Result<String, JsValue> {
    let (reader, rete) = open_url(url)?;
    let triples =
        match eval_query(&rete, construct).map_err(err)? {
            QueryOutput::Construct(t) => t,
            _ => return Err(JsValue::from_str(
                "the subset query must be a CONSTRUCT — it builds the subgraph the reasoner checks",
            )),
        };
    incomplete_guard(&rete, "query")?;
    let result = rete_core::reason(&triples);
    let out = reasoning_json(
        &result,
        Some((reader.len(), reader.bytes_read(), reader.requests())),
    );
    let _ = reader;
    Ok(out)
}

/// Build the Tier-0 schema-coherence JSON envelope from the incoherent points.
/// `remote` is `(fileLength, bytes, requests)` for the `*_url` variant.
fn schema_coherence_json(
    points: &[rete_core::Inconsistency],
    remote: Option<(u64, u64, u64)>,
) -> String {
    use serde_json::json;
    let schema_points: Vec<serde_json::Value> = points
        .iter()
        .map(|i| json!({ "kind": i.kind, "detail": i.detail }))
        .collect();
    let mut v = json!({
        "kind": "schemaCoherence",
        "coherent": points.is_empty(),
        "schemaPoints": schema_points,
        // The defining property: this answer never touched the index OR dictionary.
        "readsIndex": false,
    });
    if let (Some(obj), Some((file_length, bytes, requests))) = (v.as_object_mut(), remote) {
        obj.insert(
            "remote".to_string(),
            json!({ "fileLength": file_length, "bytes": bytes, "requests": requests }),
        );
    }
    v.to_string()
}

/// **Index-free schema coherence (Tier-0)** over an in-memory `.rete`: read only
/// the header + pyramid-meta (never the dictionary or the triple index) and report
/// schema-level incoherent points (subClassOf cycles, unsatisfiable classes).
/// Errors if the file ships no schema pyramid.
#[wasm_bindgen]
pub fn check_schema(bytes: &[u8]) -> Result<String, JsValue> {
    let points = rete_core::read_schema_coherence_ranged(&SliceReader::new(bytes))
        .map_err(err)?
        .ok_or_else(|| JsValue::from_str("file has no schema pyramid"))?;
    Ok(schema_coherence_json(&points, None))
}

/// **Index-free schema coherence (Tier-0) over a remote `.rete` URL.** Reads only
/// TWO ranges — the header and the trailing schema block (the header records its
/// length) — never the dictionary, the community summary, or the triple index. So
/// it's a flat **~1–8 KB at any graph size** (8.1 KB of a 48.8 MB file; see
/// docs/BENCHMARK.md), making it the cheap "is the ontology coherent?" gate.
/// Worker-only (synchronous XHR); a failed range fetch is an error, never a false
/// "coherent".
#[wasm_bindgen]
pub fn check_schema_url(url: &str) -> Result<String, JsValue> {
    let reader = CountingReader::new(XhrRangeReader::open(url)?);
    let points = rete_core::read_schema_coherence_ranged(&reader)
        .map_err(err)?
        .ok_or_else(|| JsValue::from_str("file has no schema pyramid"))?;
    Ok(schema_coherence_json(
        &points,
        Some((reader.len(), reader.bytes_read(), reader.requests())),
    ))
}

/// The schema summary (classes + relations) read from the schema pyramid over
/// HTTP range — a Schema view of a remote graph without downloading it.
/// Worker-only (synchronous XHR). JSON:
/// `{ "kind":"schema", "classes":[[iri,count]], "relations":[[s,p,o,count]], "remote":{…} }`.
#[wasm_bindgen]
pub fn schema_url(url: &str) -> Result<String, JsValue> {
    use serde_json::json;
    let reader = CountingReader::new(XhrRangeReader::open(url)?);
    let (classes, relations) = rete_core::read_schema_summary_ranged(&reader)
        .map_err(err)?
        .ok_or_else(|| JsValue::from_str("file has no schema pyramid"))?;
    let classes: Vec<serde_json::Value> = classes.iter().map(|(c, n)| json!([c, n])).collect();
    let relations: Vec<serde_json::Value> = relations
        .iter()
        .map(|(s, p, o, n)| json!([s, p, o, n]))
        .collect();
    Ok(json!({
        "kind": "schema",
        "classes": classes,
        "relations": relations,
        "remote": { "fileLength": reader.len(), "bytes": reader.bytes_read(), "requests": reader.requests() },
    })
    .to_string())
}

fn format_shacl_text(report: &ValidationReport) -> String {
    if report.conforms {
        return "conforms: true\n".to_string();
    }
    let mut out = format!("conforms: false\nresults: {}\n", report.results.len());
    for result in &report.results {
        out.push_str("\n- focus: ");
        out.push_str(&result.focus_node);
        if let Some(value) = &result.value_node {
            out.push_str("\n  value: ");
            out.push_str(value);
        }
        if let Some(path) = &result.result_path {
            out.push_str("\n  path: ");
            out.push_str(path);
        }
        out.push_str("\n  component: ");
        out.push_str(&result.source_constraint_component);
        out.push_str("\n  severity: ");
        out.push_str(&result.severity.iri());
        for message in &result.messages {
            out.push_str("\n  message: ");
            out.push_str(message);
        }
        out.push('\n');
    }
    out
}

// --- EXPERIMENTAL: real browser-thread parallelism (feature = "threads") ------
//
// Only compiled when the `threads` feature is on (a nightly + build-std wasm
// build, served cross-origin-isolated). The default build never sees any of
// this, so it stays single-threaded and `file://`-able.

/// Re-export wasm-bindgen-rayon's thread-pool initializer. JS MUST
/// `await initThreadPool(n)` once (after the module `init()`) before calling
/// [`reach_parallel`], or any rayon use will panic. Backed by Web Workers +
/// `SharedArrayBuffer`, so the page must be cross-origin isolated (COOP/COEP).
#[cfg(feature = "threads")]
pub use wasm_bindgen_rayon::init_thread_pool;

/// Multi-source transitive reachability run on **real browser threads** via
/// `wasm-bindgen-rayon` — the threaded twin of [`reach`]. Same arguments, same
/// JSON result shape; the only difference is it calls
/// [`rete_core::parallel::batch_reach_parallel`] (one rayon task per seed)
/// instead of the serial loop. Use it to benchmark the 14× batch-reachability
/// workload against [`reach`] in the browser.
///
/// Requires `await initThreadPool(navigator.hardwareConcurrency)` first.
#[cfg(feature = "threads")]
#[wasm_bindgen]
pub fn reach_parallel(
    bytes: &[u8],
    predicate: &str,
    seeds: &str,
    reverse: bool,
) -> Result<String, JsValue> {
    use std::collections::HashMap;
    let rete = open(bytes)?;
    let dict = rete.dictionary();

    // Accept either a JSON array of IRIs or a single bare IRI string (mirrors `reach`).
    let seed_iris: Vec<String> = match serde_json::from_str::<Vec<String>>(seeds) {
        Ok(v) => v,
        Err(_) => match serde_json::from_str::<String>(seeds) {
            Ok(s) => vec![s],
            Err(_) => vec![seeds.to_string()],
        },
    };

    // Adjacency in unified node space for the chosen direction.
    let adj: HashMap<u32, Vec<u32>> = if reverse {
        let mut m: HashMap<u32, Vec<u32>> = HashMap::new();
        for (s, o) in rete.predicate_pairs(predicate) {
            m.entry(o).or_default().push(s); // who points at o
        }
        m
    } else {
        build_adjacency(&rete, predicate)
    };

    // Resolve known seeds to node ids; remember unknowns to report inline.
    let mut known_idx: Vec<usize> = Vec::new();
    let mut seed_nodes: Vec<u32> = Vec::new();
    for (i, iri) in seed_iris.iter().enumerate() {
        if let Some(n) = dict.node_of_term(iri) {
            known_idx.push(i);
            seed_nodes.push(n);
        }
    }

    // The one line that differs from serial `reach`: real-thread fan-out.
    let sets = rete_core::parallel::batch_reach_parallel(&adj, &seed_nodes);

    // Map known-seed position -> its reach set, then assemble in input order.
    let mut by_input: HashMap<usize, &std::collections::BTreeSet<u32>> = HashMap::new();
    for (k, &i) in known_idx.iter().enumerate() {
        by_input.insert(i, &sets[k]);
    }

    let arr: Vec<serde_json::Value> = seed_iris
        .iter()
        .enumerate()
        .map(|(i, iri)| match by_input.get(&i) {
            Some(set) => {
                let reached: Vec<String> = set.iter().filter_map(|&n| dict.node_term(n)).collect();
                serde_json::json!({
                    "seed": iri,
                    "count": reached.len(),
                    "reached": reached,
                })
            }
            None => serde_json::json!({ "seed": iri, "error": "not in graph" }),
        })
        .collect();

    serde_json::to_string(&arr).map_err(err)
}

// --- CONSTRUCT serializers (mirrored from rete-cli, kept minimal) ------------

const RDF_TYPE_IRI: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const XSD_INTEGER_IRI: &str = "http://www.w3.org/2001/XMLSchema#integer";

fn summary_total(view: &SummaryView) -> u64 {
    view.summary.iter().map(|edge| u64::from(edge.count)).sum()
}

fn predicate_list(view: &SummaryView) -> Vec<String> {
    view.predicate_totals()
        .into_iter()
        .map(|(predicate, _)| predicate)
        .collect()
}

fn predicate_count(view: &SummaryView) -> u64 {
    view.predicate_totals().len() as u64
}

fn select_count_response(
    variable: &str,
    count: u64,
    progressive: serde_json::Value,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let mut row = Map::new();
    row.insert(variable.to_string(), Value::String(integer_literal(count)));
    json!({
        "kind": "select",
        "vars": [variable],
        "rows": [Value::Object(row)],
        "progressive": progressive,
    })
}

fn select_predicate_list_response(
    variable: &str,
    predicates: &[String],
    progressive: serde_json::Value,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let rows: Vec<Value> = predicates
        .iter()
        .map(|predicate| {
            let mut row = Map::new();
            row.insert(variable.to_string(), Value::String(predicate.clone()));
            Value::Object(row)
        })
        .collect();

    json!({
        "kind": "select",
        "vars": [variable],
        "rows": rows,
        "progressive": progressive,
    })
}

fn select_predicate_totals_response(
    predicate_variable: &str,
    count_variable: &str,
    totals: &[(String, u32)],
    progressive: serde_json::Value,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let rows: Vec<Value> = totals
        .iter()
        .map(|(predicate, count)| {
            let mut row = Map::new();
            row.insert(
                predicate_variable.to_string(),
                Value::String(predicate.clone()),
            );
            row.insert(
                count_variable.to_string(),
                Value::String(integer_literal(u64::from(*count))),
            );
            Value::Object(row)
        })
        .collect();

    json!({
        "kind": "select",
        "vars": [predicate_variable, count_variable],
        "rows": rows,
        "progressive": progressive,
    })
}

fn progressive_meta<R: RangeReader>(
    reader: &CountingReader<R>,
    file_bytes: u64,
    query_shape: &str,
    predicate: Option<&str>,
    value: serde_json::Value,
) -> serde_json::Value {
    use serde_json::{json, Map, Value};

    let mut meta = Map::new();
    meta.insert("stage".into(), json!("summary"));
    meta.insert("exact".into(), json!(true));
    meta.insert("readsIndex".into(), json!(false));
    meta.insert("queryShape".into(), json!(query_shape));
    meta.insert("value".into(), value);
    meta.insert("bytes".into(), json!(reader.bytes_read()));
    meta.insert("requests".into(), json!(reader.requests()));
    meta.insert("fileBytes".into(), json!(file_bytes));
    if let Some(predicate) = predicate {
        meta.insert("predicate".into(), json!(predicate));
    }
    Value::Object(meta)
}

fn integer_literal(value: u64) -> String {
    format!("\"{}\"^^<{}>", value, XSD_INTEGER_IRI)
}

fn provenance_json(m: &TripleProvenance) -> serde_json::Value {
    use serde_json::json;

    let tile = match (&m.tile, m.tile_range) {
        (Some(id), Some(range)) => json!({
            "available": true,
            "id": id,
            "range": range_json(range),
        }),
        (Some(id), None) => json!({
            "available": true,
            "id": id,
        }),
        _ => json!({
            "available": false,
            "reason": "not_materialized",
        }),
    };

    json!({
        "terms": {
            "subject": m.terms.0,
            "predicate": m.terms.1,
            "object": m.terms.2,
        },
        "ids": {
            "subject": m.ids.0,
            "predicate": m.ids.1,
            "object": m.ids.2,
        },
        "provenance": {
            "graph": m.graph.as_deref().unwrap_or("default"),
            "matchedPattern": {
                "subject": m.matched_pattern.0,
                "predicate": m.matched_pattern.1,
                "object": m.matched_pattern.2,
            },
            "indexPermutation": m.index_permutation.name(),
            "indexSection": m.index_permutation.section_index(),
            "dictionaryRange": range_json(m.dictionary_range),
            "indexRange": range_json(m.index_range),
            "indexSectionRange": range_json(m.index_section_range),
            "pyramidRange": m.pyramid_range.map(range_json),
            "tile": tile,
        },
    })
}

fn range_json(range: ByteRange) -> serde_json::Value {
    serde_json::json!({
        "offset": range.offset,
        "len": range.len,
        "end": range.end(),
    })
}

/// Serialize a triple list (canonical N-Triples tokens) to Turtle: group by
/// subject, sort predicates/objects, abbreviate `rdf:type` to `a`. The tokens are
/// already valid Turtle term syntax, so they pass through verbatim.
fn to_turtle(triples: &[(String, String, String)]) -> String {
    use std::collections::BTreeMap;
    let mut by_subject: BTreeMap<&str, BTreeMap<&str, Vec<&str>>> = BTreeMap::new();
    for (s, p, o) in triples {
        by_subject
            .entry(s)
            .or_default()
            .entry(p)
            .or_default()
            .push(o);
    }
    let mut out = String::new();
    for (s, preds) in &by_subject {
        out.push_str(s);
        out.push('\n');
        let pred_count = preds.len();
        for (i, (p, objs)) in preds.iter().enumerate() {
            let pred = if *p == RDF_TYPE_IRI { "a" } else { p };
            let objects = objs.join(" , ");
            let terminator = if i + 1 == pred_count { " ." } else { " ;" };
            out.push_str(&format!("    {pred} {objects}{terminator}\n"));
        }
        out.push('\n');
    }
    out
}

/// Serialize a triple list to expanded JSON-LD (array of node objects keyed by
/// `@id`; literals as `@value` + optional `@type`/`@language`).
fn to_jsonld(triples: &[(String, String, String)]) -> String {
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;
    let mut nodes: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    for (s, p, o) in triples {
        let id = node_id(s);
        let pred = p
            .strip_prefix('<')
            .and_then(|x| x.strip_suffix('>'))
            .unwrap_or(p)
            .to_string();
        nodes
            .entry(id)
            .or_default()
            .entry(pred)
            .or_default()
            .push(object_to_jsonld(o));
    }
    let arr: Vec<Value> = nodes
        .into_iter()
        .map(|(id, preds)| {
            let mut obj = Map::new();
            obj.insert("@id".into(), json!(id));
            for (pred, vals) in preds {
                obj.insert(pred, Value::Array(vals));
            }
            Value::Object(obj)
        })
        .collect();
    serde_json::to_string_pretty(&Value::Array(arr)).unwrap_or_default()
}

/// JSON-LD `@id` for a subject token: bare IRI for `<iri>`, `_:b` verbatim.
fn node_id(token: &str) -> String {
    token
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .map(str::to_string)
        .unwrap_or_else(|| token.to_string())
}

/// Classify an object token into a JSON-LD value object.
fn object_to_jsonld(token: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    if let Some(iri) = token.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return json!({ "@id": iri });
    }
    if let Some(b) = token.strip_prefix("_:") {
        return json!({ "@id": format!("_:{b}") });
    }
    if token.starts_with('"') {
        let bytes = token.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        let value = unescape_nt(&token[1..i.min(token.len())]);
        let rest = token.get(i + 1..).unwrap_or("");
        let mut obj = Map::new();
        obj.insert("@value".into(), Value::String(value));
        if let Some(dt) = rest.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
            obj.insert("@type".into(), json!(dt));
        } else if let Some(lang) = rest.strip_prefix('@') {
            obj.insert("@language".into(), json!(lang));
        }
        return Value::Object(obj);
    }
    json!({ "@value": token })
}

/// Resolve the N-Triples escape sequences in a literal's body to actual chars.
fn unescape_nt(s: &str) -> String {
    rete_core::terms::unescape_literal(s)
}

fn open(bytes: &[u8]) -> Result<Rete, JsValue> {
    let mut rete = Rete::open(bytes).map_err(err)?;
    // SERVICE blocks federate to remote SPARQL endpoints over sync XHR.
    rete.set_service_client(Box::new(XhrServiceClient));
    Ok(rete)
}

fn err<E: std::fmt::Display>(e: E) -> JsValue {
    JsValue::from_str(&e.to_string())
}
