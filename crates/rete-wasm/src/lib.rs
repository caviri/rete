//! Browser bindings: load a `.rete` file's bytes (e.g. from `fetch`) and query
//! it entirely in the browser — the same engine the native CLI uses, compiled
//! to wasm. Results come back as JSON strings.

use rete_core::{
    batch_reach_serial, build_adjacency, build_dendrogram, choose_round_for_budget, eval_query,
    eval_select_communities, eval_sparql, project_graph, schema_classes, schema_summary,
    summary_query_shape, tile_by_community, validate_shacl, ByteRange, CountingReader, DataGraph,
    Header, QueryOutput, RangeReader, Rete, ShaclShapes, SliceReader, SummaryQueryShape,
    SummaryView, TripleProvenance, ValidationReport, DEFAULT_TILE_BUDGET,
};
use wasm_bindgen::prelude::*;

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
    let (bytes, _stats) = rete_core::ingest::assemble_dataset(&quads, &[]);
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
    let v = query_value(&rete, query, format)?;
    serde_json::to_string(&v).map_err(err)
}

/// Evaluate any SPARQL form against an open [`Rete`] and build the playground
/// JSON envelope (shared by [`query`] and [`sparql_url`]).
fn query_value(rete: &Rete, query: &str, format: &str) -> Result<serde_json::Value, JsValue> {
    use serde_json::{json, Map, Value};
    let out = eval_query(rete, query).map_err(err)?;
    let v = match out {
        QueryOutput::Ask(b) => json!({ "kind": "ask", "boolean": b }),
        QueryOutput::Select(project, solutions) => {
            // Variable order: the projection, else the union of solution keys.
            let mut vars: Vec<String> = project;
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
            json!({ "kind": "select", "vars": vars, "rows": rows })
        }
        QueryOutput::Construct(triples) => match format {
            "ttl" => json!({ "kind": "construct", "format": "ttl", "text": to_turtle(&triples) }),
            "jsonld" => {
                json!({ "kind": "construct", "format": "jsonld", "text": to_jsonld(&triples) })
            }
            _ => {
                let arr: Vec<Value> = triples.iter().map(|(s, p, o)| json!([s, p, o])).collect();
                json!({ "kind": "construct", "triples": arr })
            }
        },
    };
    Ok(v)
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

impl XhrRangeReader {
    /// Probe the resource length with a HEAD request.
    fn open(url: &str) -> Result<Self, JsValue> {
        let xhr = web_sys::XmlHttpRequest::new()?;
        xhr.open_with_async("HEAD", url, false)?;
        xhr.send()?;
        let status = xhr.status()?;
        if !(200..300).contains(&status) {
            return Err(JsValue::from_str(&format!("HEAD {url}: status {status}")));
        }
        let len = xhr
            .get_response_header("Content-Length")?
            .and_then(|v| v.parse::<u64>().ok())
            .ok_or_else(|| {
                JsValue::from_str(&format!("server did not report Content-Length for {url}"))
            })?;
        Ok(Self {
            url: url.to_string(),
            len,
        })
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
        Ok(buf)
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
        ranges
            .iter()
            .map(|&(o, l)| self.read_at(o, l))
            .collect()
    }
}

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
            .call3(
                &JsValue::NULL,
                &JsValue::from_str(&self.url),
                &offs,
                &lens,
            )
            .ok()?;
        if res.is_null() || res.is_undefined() {
            return None;
        }
        Some(js_sys::Uint8Array::new(&res).to_vec())
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
#[wasm_bindgen]
pub fn sparql_url(url: &str, query: &str, format: &str) -> Result<String, JsValue> {
    let reader = std::sync::Arc::new(CountingReader::new(XhrRangeReader::open(url)?));
    let file_length = reader.len();
    let rete = Rete::open_ranged_lazy(reader.clone()).map_err(err)?;
    let mut v = query_value(&rete, query, format)?;
    if rete.index_incomplete() {
        return Err(JsValue::from_str(
            "a range fetch failed mid-query; refusing to return incomplete results",
        ));
    }
    if let Some(obj) = v.as_object_mut() {
        obj.insert(
            "remote".to_string(),
            serde_json::json!({
                "fileLength": file_length,
                "bytes": reader.bytes_read(),
                "requests": reader.requests(),
            }),
        );
    }
    serde_json::to_string(&v).map_err(err)
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
    use serde_json::{json, Map, Value};
    let rete = open(bytes)?;
    let (mut vars, solutions, partials) =
        eval_select_communities(&rete, query, round).map_err(err)?;
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
    use serde_json::json;
    use std::collections::BTreeMap;
    let rete = open(bytes)?;
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
    use std::collections::HashMap;
    let rete = open(bytes)?;
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
    let rete = open(bytes)?;
    let data = DataGraph::from_rete(&rete, graph.as_deref());
    let shapes = ShaclShapes::parse_turtle(shapes_turtle).map_err(err)?;
    let report = validate_shacl(&data, &shapes);
    Ok(match format {
        "json" => report.to_json(),
        "ttl" => report.to_turtle(),
        _ => format_shacl_text(&report),
    })
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

fn open(bytes: &[u8]) -> Result<Rete, JsValue> {
    Rete::open(bytes).map_err(err)
}

fn err<E: std::fmt::Display>(e: E) -> JsValue {
    JsValue::from_str(&e.to_string())
}
