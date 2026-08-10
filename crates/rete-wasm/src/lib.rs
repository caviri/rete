//! Browser bindings: load a `.rete` file's bytes (e.g. from `fetch`) and query
//! it entirely in the browser — the same engine the native CLI uses, compiled
//! to wasm. Results come back as JSON strings.

use rete_core::{
    batch_reach_serial, build_adjacency, build_dendrogram, choose_round_for_budget, eval_query,
    eval_query_with, eval_select_communities, eval_sparql, project_graph, schema_classes,
    schema_summary, summary_query_shape, tile_by_community, validate_shacl, BlockCacheReader,
    ByteRange, CountingReader, DataGraph, Header, OffsetReader, QueryOpts, QueryOutput,
    RangeReader, Rete, ReteGraph, ShaclShapes, SliceReader, SummaryQueryShape, SummaryView,
    TermTriple, TripleProvenance, ValidationReport, DEFAULT_BLOCK, DEFAULT_TILE_BUDGET,
};
use std::rc::Rc;
use wasm_bindgen::prelude::*;

/// Version of Rete-owned JSON object envelopes exposed by the browser API.
pub const JSON_SCHEMA_VERSION: u8 = 1;

/// Module init: route Rust panics to `console.error` with their message and
/// location. In release wasm a panic otherwise aborts as a bare
/// `RuntimeError: unreachable` with no clue where — this turns that into a
/// `rete-wasm panic: panicked at '…', src/…:line` line in the devtools console,
/// so an intermittent first-query crash (e.g. a parser tripping on a flaky
/// range read) can actually be diagnosed.
#[wasm_bindgen(start)]
pub fn __start() {
    // The asyncify build must NOT format in the panic hook: the fmt machinery
    // is asyncify-instrumented (everything that can reach panic_fmt is), so a
    // panic raised while the instance is unwinding/rewinding recurses through
    // garbage-returning fmt calls forever (stack overflow / null-function).
    // Report the raw location pointers through a LEAF import instead — no
    // formatting, no instrumented calls — then let panic=abort trap cleanly.
    #[cfg(feature = "asyncify")]
    std::panic::set_hook(Box::new(|info| {
        if let Some(loc) = info.location() {
            let f = loc.file();
            unsafe { rete_panic_report(f.as_ptr(), f.len(), loc.line()) };
        } else {
            unsafe { rete_panic_report(std::ptr::null(), 0, 0) };
        }
    }));
    #[cfg(not(feature = "asyncify"))]
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
        r#"{{"schemaVersion":1,"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
        h.quad_count,
        h.term_count,
        h.pyramid_levels,
        rete.named_graph_count()
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
    let quads = rete_core::ingest::parse_statements(text, format).map_err(err)?;
    if quads.is_empty() {
        return Err(js_error(
            "no statements parsed (empty input or only comments)",
        ));
    }
    let (bytes, _stats) = rete_core::ingest::assemble_dataset(quads, &[]);
    Ok(bytes)
}

/// [`build`], but the file carries a **Dataset Card** written from
/// `card_json` — the same document `rete build --card-file` takes, validated
/// by the same rules ([`rete_core::card::validate_curated_card`]), so a card
/// authored in the browser is one the CLI would also have accepted.
///
/// What the browser can and cannot put in a card, stated plainly because the
/// difference matters to whoever reads the file afterwards:
///
/// - **Curated fields travel in full** — title, description, licence, source,
///   version, creators, publisher, DOI, citation, keywords, theme, the `extra`
///   bag, everything on [`rete_core::card::CURATED_CARD_FIELDS`].
/// - **The four counts are measured, not asserted**: `triple_count`,
///   `quad_count`, `named_graph_count` and `term_count` come from the build's
///   own [`BuildStats`](rete_core::ingest::BuildStats), and any values supplied
///   for them would be ignored (they are not curated fields, so supplying them
///   is already an error). `format_version` is stamped by the writer.
/// - **The derived profile is NOT written.** Predicates, classes,
///   vocabularies, datatypes, languages, class links, hubs, signals and the
///   tiered starter-query library are absent. Their absence is honest absence:
///   the card simply does not carry those keys, exactly as a `rete merge` card
///   does not. Call [`build_with_derived_card`] instead to compute them here —
///   this function stays curated-only so its bytes never change under a caller
///   who did not ask for the extra passes.
/// - **No build-info section** (kind 7) is written: its cost figures come from
///   measuring the starter queries, and there are none to measure.
///
/// Pass an empty string for no card — byte-identical to [`build`].
#[wasm_bindgen]
pub fn build_with_card(text: &str, format: &str, card_json: &str) -> Result<Vec<u8>, JsValue> {
    let quads = rete_core::ingest::parse_statements(text, format).map_err(err)?;
    if quads.is_empty() {
        return Err(js_error(
            "no statements parsed (empty input or only comments)",
        ));
    }
    if card_json.trim().is_empty() {
        let (bytes, _stats) = rete_core::ingest::assemble_dataset(quads, &[]);
        return Ok(bytes);
    }
    let curated = validated_card(card_json)?;
    // The counts are only known once the dictionary and indexes exist, so the
    // card is serialized from inside the writer rather than handed in whole.
    let (bytes, _stats) = rete_core::ingest::assemble_dataset_with(quads, move |stats, _| {
        let card = rete_core::card::compose_curated_card(
            curated,
            stats.default_triples as u64,
            stats.statements as u64,
            stats.named_graphs as u64,
            stats.terms as u64,
            rete_core::CURRENT_FORMAT_VERSION,
        );
        serde_json::to_vec(&card).expect("card serializes")
    });
    Ok(bytes)
}

/// [`build_with_card`], but the card also carries the **auto-derived profile**
/// — the half a browser build used to have to do without (#152).
///
/// Predicates, classes, vocabularies, datatypes, languages, the class-link
/// quotient, hubs, the affordance signals, and the tiered starter-query
/// library are all computed here, by exactly the code `rete build --card`
/// runs ([`rete_core::card::derive_card`]). On the same graph with the same
/// curated document, the metadata section this writes is **byte-identical** to
/// the CLI's.
///
/// Two honest differences remain, and neither is derivation:
///
/// - **Sections are uncompressed** (the wasm build has no zstd *encoder*), so
///   the file is larger than a CLI build of the same graph. Every reader
///   accepts it.
/// - **No build-info section** (kind 7): its cost figures come from *running*
///   the starter queries, which is a benchmark, not a build.
///
/// # Why this is a separate function
///
/// Derivation walks the graph twice more. In a browser, on a paste the user is
/// waiting on, that is a cost they should choose — so [`build_with_card`]
/// keeps writing exactly the bytes it always has, and this is the opt-in.
///
/// Pass an empty string for `card_json` to derive a profile-only card with no
/// curated fields (the equivalent of a bare `rete build --card`).
#[wasm_bindgen]
pub fn build_with_derived_card(
    text: &str,
    format: &str,
    card_json: &str,
) -> Result<Vec<u8>, JsValue> {
    let quads = rete_core::ingest::parse_statements(text, format).map_err(err)?;
    if quads.is_empty() {
        return Err(js_error(
            "no statements parsed (empty input or only comments)",
        ));
    }
    // The typed curated half, held to the same rules `--card-file` is held to.
    let curated = if card_json.trim().is_empty() {
        rete_core::card::CardInput::default()
    } else {
        // Validate through the document validator first, so the wording a
        // browser author sees is the wording `validate_card` shows them while
        // they type, not a raw serde message.
        validated_card(card_json)?;
        rete_core::card::CardInput::from_json_str(card_json).map_err(js_error)?
    };
    let (bytes, _stats) = rete_core::ingest::assemble_dataset_with(quads, move |stats, quads| {
        let card = rete_core::card::derive_card(
            quads,
            stats.terms as u64,
            stats.named_graphs as u64,
            curated,
        );
        // The counts the file actually holds are known only once the indexes
        // have deduplicated the input — the same two-stage stamp the CLI uses.
        rete_core::ingest::DeferredMetadata::new(move |counts| {
            card.with_final_counts(counts).to_json_bytes()
        })
    });
    Ok(bytes)
}

/// Check a curated card document without building anything — so an editor can
/// report the **exact** error `rete build --card-file` would report, while the
/// author is still typing. Returns the empty string when the document is
/// valid, otherwise the error message.
///
/// Deliberately not a boolean: the wording is the useful part (a free-text
/// `theme` is told to use `keywords`; a stray top-level key is told about the
/// `extra` bag), and duplicating that wording in JavaScript is exactly how the
/// two writers would drift apart again.
#[wasm_bindgen]
pub fn validate_card(card_json: &str) -> String {
    check_card(card_json).err().unwrap_or_default()
}

/// Parse + validate a curated card document. The message stays a `String` all
/// the way through — wrapping it in a JS `Error` first and unwrapping it later
/// loses the wording, which is the part worth returning.
fn check_card(card_json: &str) -> Result<serde_json::Value, String> {
    let doc: serde_json::Value =
        serde_json::from_str(card_json).map_err(|e| format!("card is not JSON: {e}"))?;
    rete_core::card::validate_curated_card(&doc)
}

/// [`check_card`] as a JS exception, for the build path.
fn validated_card(card_json: &str) -> Result<serde_json::Value, JsValue> {
    check_card(card_json).map_err(js_error)
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
    .map_err(js_error)
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        .ok_or_else(|| js_error("file has no schema pyramid"))?;
    let classes: Vec<serde_json::Value> = classes.iter().map(|(c, n)| json!([c, n])).collect();
    let relations: Vec<serde_json::Value> = relations
        .iter()
        .map(|(s, p, o, n)| json!([s, p, o, n]))
        .collect();
    Ok(json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
        "classes": classes,
        "relations": relations,
    })
    .to_string())
}

/// The embedded **Dataset Card** — the file's own self-description (title,
/// description, license, provenance, counts, example queries) as the JSON text
/// it was written with, or `undefined` when the file carries none. Reads the
/// metadata section straight out of the buffer.
#[wasm_bindgen]
pub fn card(bytes: &[u8]) -> Result<Option<String>, JsValue> {
    let bytes = rete_core::read_metadata_ranged(&SliceReader::new(bytes)).map_err(err)?;
    Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

/// The embedded **Dataset Card of a remote `.rete`**, in **two small range
/// requests**: the header, then the metadata section it points at — never the
/// dictionary, index, or pyramid. This is the index-free CARD tier: a client
/// learns what a multi-gigabyte graph *is* for a few KB. `undefined` when the
/// file carries no card. Worker-only (synchronous XHR).
#[wasm_bindgen]
pub fn card_url(url: &str) -> Result<Option<String>, JsValue> {
    let reader = RemoteReader::open(url)?.view();
    let bytes = rete_core::read_metadata_ranged(&reader).map_err(err)?;
    Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
}

/// The Dataset Card **and the build record** of a remote `.rete`, in the same
/// budget as the card alone: one header read, then **one coalesced range**
/// covering both sections — the writer lays the kind-7 build-info immediately
/// after the metadata precisely so this holds
/// ([`rete_core::range::read_card_and_build_info_ranged`], pinned by a
/// `rete-core` test). Reading the two separately would have made the CARD tier
/// cost three requests instead of two, which is why there is one export rather
/// than a second `build_info_url`.
///
/// JSON envelope:
/// `{"schemaVersion":1,"card":<text|null>,"build":<text|null>,"text_index":{…}}`.
/// `card` and `build` are the sections' **own bytes** as text, not a
/// re-serialization — the card a client displays is the card the file holds.
/// `text_index` is the one thing the file does *not* store about itself and this
/// reader measures instead (see the private `text_index_json`). Worker-only
/// (synchronous XHR).
#[wasm_bindgen]
pub fn card_and_build_url(url: &str) -> Result<String, JsValue> {
    // The polyglot-aware view: base 0 for a plain .rete, shifted past the HTML
    // shell for a polyglot — so the card of an embedded graph is read with the
    // same two requests as a standalone file's.
    let reader = RemoteReader::open(url)?.view();
    card_build_from(&reader)
}

/// [`card_and_build_url`] for an image already in memory — no I/O at all.
#[wasm_bindgen]
pub fn card_and_build(bytes: &[u8]) -> Result<String, JsValue> {
    card_build_from(&SliceReader::new(bytes))
}

/// The shared body of the two: header (which the card read needs anyway), the
/// coalesced card + build-info range, and the measured text-index signal — so
/// the CARD tier stays at one header read plus one coalesced range, plus the
/// ≤10-byte probe only when there is an index to measure.
fn card_build_from<R: RangeReader>(reader: &R) -> Result<String, JsValue> {
    let (header, card, build) =
        rete_core::read_card_and_build_info_with_header(reader).map_err(err)?;
    let token_table = rete_core::read_text_index_token_table_len_ranged(reader, &header);
    Ok(card_build_envelope(
        card,
        build,
        text_index_json(&header, token_table),
    ))
}

/// Whether a `.rete` carries a **full-text (TEXT_INDEX) section**, measured from
/// the section directory in the 1 KiB header the caller has just read:
/// `{"present":bool,"bytes":N,"token_table_bytes":N}` (the last two only when
/// there is an index).
///
/// This is not in the card, and deliberately so. A file built with
/// `--text-index` answers `FILTER(CONTAINS(…))` by word lookup; one built
/// without it answers the *same query with the same rows* by full scan, so the
/// capability is invisible from the results — and a stored flag would be a claim
/// that can outlive the section it describes. Measuring costs the header (already
/// fetched) plus one ≤10-byte range read for the token-table length: the figure
/// a first search actually pays, several times smaller than the whole section.
///
/// `token_table_bytes` is omitted when it was not measured — absent means
/// "not measured here", never "zero".
fn text_index_json(header: &Header, token_table_bytes: Option<u64>) -> serde_json::Value {
    if header.text_index_len == 0 {
        return serde_json::json!({ "present": false });
    }
    let mut v = serde_json::json!({
        "present": true,
        "bytes": header.text_index_len,
    });
    if let Some(tt) = token_table_bytes {
        v.as_object_mut()
            .expect("an object")
            .insert("token_table_bytes".into(), serde_json::json!(tt));
    }
    v
}

/// The `{card, build, text_index}` envelope both readers return. Absent sections
/// are `null`, never `""` or `{}`: a file built before build-info existed has no
/// build record, and a reader must be able to tell that from one that recorded
/// nothing. `text_index` is always an object — it is measured, so there is
/// always an answer, including for a file with no card at all.
fn card_build_envelope(
    card: Option<Vec<u8>>,
    build: Option<Vec<u8>>,
    text_index: serde_json::Value,
) -> String {
    let text = |b: Option<Vec<u8>>| match b {
        Some(b) if !b.is_empty() => {
            serde_json::Value::String(String::from_utf8_lossy(&b).into_owned())
        }
        _ => serde_json::Value::Null,
    };
    serde_json::json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
        "card": text(card),
        "build": text(build),
        "text_index": text_index,
    })
    .to_string()
}

/// The **true byte length of a remote `.rete`**, in 1–2 tiny range requests —
/// derived from the file's *own* header (the issue-#95 probe: sections are
/// back-to-back and the file ends with the 4-byte `RETE` footer), never from
/// the transport's numbers, which may describe a compressed representation
/// (GitHub Pages HEADs a 71 MB file as its 58 MB gzip) or be hidden from
/// cross-origin JS entirely. This is how a UI can say what "download the whole
/// file" actually costs **before** committing to it.
/// JSON: `{ "schemaVersion": 1, "fileLength": <bytes> }`. Worker-only
/// (synchronous XHR in the sync build).
#[wasm_bindgen]
pub fn file_len_url(url: &str) -> Result<String, JsValue> {
    // The GRAPH's length: for a polyglot that is the appended `.rete`, not the
    // web page wrapped around it — "download the whole file" must quote the
    // bytes a `.rete` consumer would actually take.
    let reader = RemoteReader::open(url)?;
    Ok(format!(
        r#"{{"schemaVersion":{},"fileLength":{}}}"#,
        JSON_SCHEMA_VERSION,
        reader.len()
    ))
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
    rete: Rc<Rete>,
    file_len: usize,
    /// The kind-7 build record's own bytes, lifted at open time. `Rete` keeps
    /// the metadata section but not this one (it is outside the content hash
    /// and no query needs it), and the handle does not retain the buffer — so
    /// it is read once here rather than made unreachable.
    build_info: Option<String>,
}

#[wasm_bindgen]
impl Graph {
    /// Open a `.rete` image and keep it resident for repeated querying.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Graph, JsValue> {
        Ok(Graph {
            rete: Rc::new(open(bytes)?),
            file_len: bytes.len(),
            build_info: rete_core::read_build_info(bytes)
                .ok()
                .flatten()
                .filter(|b| !b.is_empty())
                .map(|b| String::from_utf8_lossy(&b).into_owned()),
        })
    }

    /// A **lazy, resumable cursor** over the quads of this graph — the streaming
    /// export path. See [`QuadCursor`]; `graph` selects one graph (`""` = the
    /// default graph), `None` streams the default graph followed by every named
    /// graph. `s` / `p` / `o` optionally restrict the dump to a triple pattern,
    /// which **prunes tiles** rather than filtering rows.
    pub fn quads(
        &self,
        graph: Option<String>,
        s: Option<String>,
        p: Option<String>,
        o: Option<String>,
    ) -> QuadCursor {
        QuadCursor::start(self.rete.clone(), graph, s, p, o)
    }

    /// See [`info`].
    pub fn info(&self) -> String {
        let h = self.rete.header();
        format!(
            r#"{{"schemaVersion":1,"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
            h.quad_count,
            h.term_count,
            h.pyramid_levels,
            self.rete.named_graph_count()
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

    /// As [`query`], with explicit opt-in toggles: `reason` (OWL 2 QL
    /// entailment) and `union_default` (union default graph — a pattern
    /// outside `GRAPH` matches the merge of the default graph and every named
    /// graph, the Virtuoso / GraphDB / Jena TDB mode; non-standard, so plain
    /// [`Graph::query`] never does this).
    pub fn query_opts(
        &self,
        query: &str,
        format: &str,
        reason: bool,
        union_default: bool,
    ) -> Result<String, JsValue> {
        query_json_with(&self.rete, query, format, "", reason, union_default)
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

    /// Byte length of the TEXT_INDEX section, `0` when the file has none — a
    /// header read, never a fault. The UI asks this to decide whether to offer
    /// full-text search at all, and to state the cost before the first one.
    /// `f64` because the section outgrows `u32`: causenet's is 1.88 GB.
    pub fn text_index_len(&self) -> f64 {
        self.rete.header().text_index_len as f64
    }

    /// Byte length of the TEXT_INDEX's leading **token table** — what a first
    /// [`Graph::text_search_one`] actually faults, and therefore the only honest
    /// number to quote as its cost. [`Graph::text_index_len`] is the whole
    /// section, postings blob included, and overstates it 6.5× on
    /// `epfl-infoscience` (195 MB section, 29 MB token table); the postings are
    /// only ever fetched one list at a time. `0` when the file has no text index
    /// or the length could not be read — the caller must then say nothing about
    /// a token table rather than pass the section length off as one.
    /// `f64` for the same reason as the section length: causenet's table is
    /// 1.88 GB.
    pub fn text_index_token_table_len(&self) -> f64 {
        self.rete.text_index_token_table_len().unwrap_or(0) as f64
    }

    /// [`Graph::text_search`] from ONE phrase: whitespace splits it into words
    /// and **every** word must match (AND), like `rete search --contains a b`.
    /// One string in, one string out — that is what the remote twin's
    /// hand-marshaled asyncify path can carry (a JS array marshaled raw is what
    /// traps), and the UI is a single text box either way. Same JSON envelope.
    pub fn text_search_one(&self, phrase: &str, limit: usize) -> Result<String, JsValue> {
        let words: Vec<String> = phrase.split_whitespace().map(str::to_owned).collect();
        text_search_json(&self.rete, &words, None, limit)
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
            "schemaVersion": JSON_SCHEMA_VERSION,
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
            "schemaVersion": JSON_SCHEMA_VERSION,
            "classes": schema_classes(&self.rete),
            "relations": schema_summary(&self.rete),
        }))
        .map_err(err)
    }

    /// See [`card`] — the Dataset Card of the resident file.
    pub fn card(&self) -> Option<String> {
        self.rete
            .metadata()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    /// See [`card_and_build`] — the card and the build record of the resident
    /// file, in the same envelope the remote path returns, so one caller
    /// handles both sources.
    pub fn card_and_build(&self) -> String {
        let card = self
            .rete
            .metadata()
            .filter(|b| !b.is_empty())
            .map(|b| b.to_vec());
        // A resident open already decoded the whole TEXT_INDEX section, so both
        // figures are free here.
        let text_index =
            text_index_json(self.rete.header(), self.rete.text_index_token_table_len());
        card_build_envelope(
            card,
            self.build_info.as_ref().map(|s| s.as_bytes().to_vec()),
            text_index,
        )
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
        serde_json::to_string(&json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "fileLength": self.file_len,
            "segments": segments,
        }))
        .map_err(err)
    }
}

/// A **lazy, resumable cursor** over the quads of an open `.rete` — the engine
/// side of `for await (const [s, p, o, g] of graph.dump())` in the JS client.
///
/// # Why a cursor and not a callback
///
/// [`Rete::dump_each`] already streams in constant memory, but a Rust callback
/// cannot be *paused* to hand control back to JavaScript: to feed a JS iterator
/// it would have to buffer every quad first, which is exactly the `Vec` that
/// [`Rete::dump`] builds and that OOMs on a large file. This wraps
/// [`Rete::query_batch`] instead, so the scan can be suspended between calls and
/// resumed in place — the whole resume state is one opaque `u64`, never a
/// whole-graph materialization anywhere in the pipeline.
///
/// # Why batched (and not one call per quad)
///
/// Each wasm→JS call costs far more than decoding a triple, and every returned
/// `String` becomes a fresh JS string. Pulling one quad per call would make the
/// boundary the bottleneck; pulling *all* of them would reintroduce the `Vec`.
/// So the JS wrapper asks for `DUMP_BATCH` quads at a time and yields them one
/// by one — bounded, amortized, and lazy. Memory is O(batch), not O(graph).
///
/// # Cost model
///
/// The dictionary is **not** prefetched whole: each batch faults only the
/// chunks its own terms live in, so taking five quads off the front costs five
/// quads' worth of dictionary rather than all of it. Index tiles fault in as the
/// scan advances and stay resident, and so do dictionary chunks, so an
/// **unfiltered** dump driven to the end still ends up fetching essentially the
/// whole file — that is inherent in exporting a graph.
///
/// A **filtered** cursor (a bound `s` / `p` / `o`) is a different shape: the
/// scan routes to the one permutation that sorts on the bound prefix and drops
/// every tile whose synopsis proves it cannot match, from the tile directory,
/// *without fetching it*. On `cordis.rete` (801 MB, six named graphs) dumping
/// one predicate of one graph reads 16 MB where the unfiltered dump of that
/// graph reads 376 MB. Peak memory is O(faulted dictionary + index), never
/// O(quads), either way.
#[wasm_bindgen]
pub struct QuadCursor {
    /// Graph slots not yet streamed; `None` = the default graph.
    pending: std::vec::IntoIter<Option<String>>,
    /// The graph being streamed (`None` = the default graph), and how far into
    /// it we got. The resume state is one opaque `u64` — see `Rete::query_batch`.
    current: Option<Option<String>>,
    cursor: u64,
    /// The triple pattern this dump is restricted to; all-`None` = every quad.
    /// Held as owned tokens because the cursor outlives every call.
    s: Option<String>,
    p: Option<String>,
    o: Option<String>,
    /// Whether the live graph's last batch was its final one.
    slot_done: bool,
    /// Triples resolved by the last `query_batch` that the caller has not taken
    /// yet. A batch ends on a group boundary, so this drains before the next.
    buffered: std::vec::IntoIter<TermTriple>,
    rete: Rc<Rete>,
}

/// Quads per `next_batch` / `next_nquads` call, and the JS wrapper's default.
///
/// 10 000 keeps the per-call boundary cost negligible (one call and ~40 000 JS
/// string allocations amortize a 10 000-triple decode) while bounding the
/// transient buffer at roughly 10 000 × ~120 B ≈ 1.2 MB — small next to the
/// dictionary any dump already needs resident, and flat no matter how many
/// billions of quads follow it.
const DUMP_BATCH: usize = 10_000;

impl QuadCursor {
    /// `graph`: `None` = the default graph followed by every named graph;
    /// `Some("")` = the default graph only; `Some(name)` = that named graph
    /// (bare IRI or `<iri>` token — both are accepted).
    ///
    /// `s` / `p` / `o` are canonical N-Triples term tokens; an empty string is
    /// treated as unbound, so a JS caller can pass `""` for "no filter" without
    /// a separate sentinel. A bound term the dictionary does not know yields an
    /// empty dump without touching the index.
    fn start(
        rete: Rc<Rete>,
        graph: Option<String>,
        s: Option<String>,
        p: Option<String>,
        o: Option<String>,
    ) -> QuadCursor {
        let slots: Vec<Option<String>> = match graph {
            None => std::iter::once(None)
                .chain(rete.graph_names().iter().map(|g| Some((*g).to_string())))
                .collect(),
            Some(g) if g.is_empty() => vec![None],
            Some(g) => vec![Some(canonical_graph_name(&rete, g))],
        };
        // The incompleteness verdict is PER DUMP: clear the sticky flags so a
        // failure from an earlier query does not condemn this one (and so
        // `finish()` reports only what happened while streaming).
        rete.reset_load_failures();
        let term = |t: Option<String>| t.filter(|t| !t.is_empty());
        QuadCursor {
            pending: slots.into_iter(),
            current: None,
            cursor: 0,
            s: term(s),
            p: term(p),
            o: term(o),
            slot_done: false,
            buffered: Vec::new().into_iter(),
            rete,
        }
    }

    /// Pull at most `max` quads, handing each to `sink` as
    /// `(s, p, o, graph)` where `graph` is `None` for the default graph.
    /// Returns how many were emitted; fewer than `max` means the stream ended.
    ///
    /// The scan is resumable rather than held open: each refill calls
    /// `Rete::query_batch`, whose entire resume state is one opaque `u64`. That
    /// is what lets this struct be `'static` — required by `#[wasm_bindgen]` —
    /// without a self-referential iterator borrowing the `Rete` beside it.
    fn pull<F: FnMut(&str, &str, &str, Option<&str>)>(&mut self, max: usize, mut sink: F) -> usize {
        let mut emitted = 0;
        while emitted < max {
            // 1. Hand out what the last batch already resolved.
            if let Some((s, p, o)) = self.buffered.next() {
                let graph = self.current.as_ref().and_then(|g| g.as_deref());
                sink(&s, &p, &o, graph);
                emitted += 1;
                continue;
            }
            // 2. Buffer empty. Open the next graph slot if none is live…
            let Some(slot) = self.current.clone() else {
                match self.pending.next() {
                    Some(next) => {
                        self.current = Some(next);
                        self.cursor = 0;
                        self.slot_done = false;
                    }
                    None => break,
                }
                continue;
            };
            // 3. …or close the live one if its last batch is drained.
            if self.slot_done {
                self.current = None;
                continue;
            }
            // 4. Otherwise resume it. `query_batch` always advances the cursor
            //    or reports done, so this cannot spin.
            let want = (max - emitted).max(DUMP_BATCH);
            let (triples, next, done) = self.rete.query_batch(
                slot.as_deref(),
                self.s.as_deref(),
                self.p.as_deref(),
                self.o.as_deref(),
                self.cursor,
                want,
            );
            self.cursor = next;
            self.slot_done = done;
            self.buffered = triples.into_iter();
        }
        emitted
    }

    /// True once every selected graph has been streamed to its end.
    fn finished(&self) -> bool {
        self.current.is_none() && self.pending.len() == 0 && self.buffered.len() == 0
    }

    /// Refuse to end a dump that silently lost bytes to a failed range fetch —
    /// a truncated export is worse than a failed one.
    fn guard(&self) -> Result<(), JsValue> {
        if self.finished() {
            incomplete_guard(&self.rete, "dump")?;
        }
        Ok(())
    }
}

#[wasm_bindgen]
impl QuadCursor {
    /// Up to `max` quads as a **flat** `string[]` of `[s, p, o, g, s, p, o, g, …]`
    /// N-Triples term tokens, `g` being `""` for the default graph. Flat because
    /// a nested array would allocate one JS array per quad for no gain — the
    /// caller slices it into tuples as it yields them.
    ///
    /// An empty array means the stream is finished; keep calling until you get
    /// one (that final call is what verifies no range fetch failed mid-dump).
    pub fn next_batch(&mut self, max: Option<usize>) -> Result<Vec<String>, JsValue> {
        let max = max.unwrap_or(DUMP_BATCH).max(1);
        let mut out: Vec<String> = Vec::with_capacity(max.min(DUMP_BATCH) * 4);
        self.pull(max, |s, p, o, g| {
            out.push(s.to_string());
            out.push(p.to_string());
            out.push(o.to_string());
            out.push(g.unwrap_or("").to_string());
        });
        self.guard()?;
        Ok(out)
    }

    /// Up to `max` quads already serialized as N-Quads lines in **one** string —
    /// the `.rete` → Oxigraph / N-Quads-file path. One string crossing per batch
    /// instead of four per quad: no per-term JS string, no re-serialization in
    /// JavaScript, and the terms are already canonical N-Triples tokens, so the
    /// lines are emitted verbatim.
    ///
    /// An empty string means the stream is finished.
    pub fn next_nquads(&mut self, max: Option<usize>) -> Result<String, JsValue> {
        let max = max.unwrap_or(DUMP_BATCH).max(1);
        // ~120 B/quad is a typical N-Quads line; the Vec grows if it is wrong.
        let mut out = String::with_capacity(max.min(DUMP_BATCH) * 120);
        self.pull(max, |s, p, o, g| {
            out.push_str(s);
            out.push(' ');
            out.push_str(p);
            out.push(' ');
            out.push_str(o);
            if let Some(g) = g {
                out.push(' ');
                out.push_str(g);
            }
            out.push_str(" .\n");
        });
        self.guard()?;
        Ok(out)
    }

    /// Whether every selected graph has been streamed to its end.
    pub fn done(&self) -> bool {
        self.finished()
    }
}

/// Resolve a caller-supplied graph name to the token the file stores. Graph
/// names are canonical N-Triples terms (`<iri>`), but the JS client hands out
/// bare IRIs, so accept either and prefer an exact match.
fn canonical_graph_name(rete: &Rete, name: String) -> String {
    if rete.graph_names().iter().any(|g| *g == name) {
        return name;
    }
    if name.starts_with('<') || name.starts_with("_:") {
        return name;
    }
    format!("<{name}>")
}

/// The wasm linear memory's current size in bytes — the engine's high-water
/// mark, since wasm memory grows but never shrinks. Exposed so a host can
/// *measure* the streaming-dump memory claim instead of trusting it: sample it
/// before and after a full [`QuadCursor`] drain and the growth stays flat
/// however many quads went by, where materializing them all does not.
#[wasm_bindgen]
pub fn heap_bytes() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        (core::arch::wasm32::memory_size(0) as f64) * 65536.0
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        0.0
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
    reader: RemoteReader,
    rete: Rc<Rete>,
}

#[wasm_bindgen]
impl RemoteGraph {
    /// Open a remote `.rete` over HTTP range and keep it resident for repeated
    /// querying. The first query faults in the dictionary chunks + index tiles it
    /// needs; later queries on this handle reuse them and the block cache.
    #[wasm_bindgen(constructor)]
    pub fn new(url: &str) -> Result<RemoteGraph, JsValue> {
        let (reader, rete) = open_url(url)?;
        Ok(RemoteGraph {
            reader,
            rete: Rc::new(rete),
        })
    }

    /// See [`Graph::quads`] — the SAME lazy cursor, over the lazily range-read
    /// remote handle.
    ///
    /// An **unfiltered** dump is not network-lazy and cannot be: it resolves
    /// every term and visits every tile, so it ends up fetching essentially the
    /// whole file (and what it faults stays resident). A **filtered** one is:
    /// pass `s` / `p` / `o` and the scan routes to one permutation, keeps only
    /// the tiles whose synopsis admits the bound components, and fetches those.
    /// On `cordis.rete` (801 MB) one predicate of one named graph costs 16 MB
    /// instead of 376 MB. To peek at an unfiltered graph, still prefer a `LIMIT`
    /// query. Worker-only in the browser, like every other read here.
    pub fn quads(
        &self,
        graph: Option<String>,
        s: Option<String>,
        p: Option<String>,
        o: Option<String>,
    ) -> QuadCursor {
        QuadCursor::start(self.rete.clone(), graph, s, p, o)
    }

    /// `{ fileLength, bytes, requests, base }` — CUMULATIVE physical fetches
    /// since this session opened. The worker diffs successive calls to report a
    /// single query's traffic (a fully cached re-run adds ~0). `fileLength` is
    /// the **graph's** length and `base` the byte offset it starts at: `0` for an
    /// ordinary `.rete`, and the size of the HTML shell for a polyglot file.
    pub fn stats(&self) -> String {
        format!(
            r#"{{"schemaVersion":1,"fileLength":{},"bytes":{},"requests":{},"base":{}}}"#,
            self.reader.len(),
            self.reader.bytes_read(),
            self.reader.requests(),
            self.reader.base
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
    /// The incompleteness verdict is PER QUERY: reset the sticky failure flags
    /// first, so one transient fetch failure fails only the query it happened
    /// in — not every later query on this session (failed tiles/chunks are
    /// never cached, so they retry here).
    pub fn query(&self, query: &str, format: &str) -> Result<String, JsValue> {
        self.rete.reset_load_failures();
        let s = query_json(&self.rete, query, format, "")?;
        incomplete_guard(&self.rete, "query")?;
        Ok(s)
    }

    /// As [`query`], with OWL 2 QL entailment on (reason over the ontology while
    /// reading only the bytes the rewritten query touches).
    pub fn query_reasoned(&self, query: &str, format: &str) -> Result<String, JsValue> {
        self.rete.reset_load_failures();
        let s = query_json_reasoned(&self.rete, query, format, "")?;
        incomplete_guard(&self.rete, "query")?;
        Ok(s)
    }

    /// As [`query`], with explicit opt-in toggles — see [`Graph::query_opts`].
    /// With `union_default` on, a lazy remote read may fault the index tiles of
    /// every named graph the union touches (the merge is strictly opt-in).
    pub fn query_opts(
        &self,
        query: &str,
        format: &str,
        reason: bool,
        union_default: bool,
    ) -> Result<String, JsValue> {
        self.rete.reset_load_failures();
        let s = query_json_with(&self.rete, query, format, "", reason, union_default)?;
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

    /// See [`Graph::text_index_len`] — read from the resident header, so it
    /// costs no fetch at all. Worth asking before [`RemoteGraph::text_search`]:
    /// it is the size of the section the first search starts pulling over the
    /// wire, so the UI can warn instead of surprising the user.
    pub fn text_index_len(&self) -> f64 {
        self.rete.header().text_index_len as f64
    }

    /// See [`Graph::text_index_token_table_len`] — the figure to quote before
    /// [`RemoteGraph::text_search`], because it is what that first search pulls
    /// over the wire; the section length would promise the user several times
    /// the real bill.
    ///
    /// Unlike [`RemoteGraph::text_index_len`] this is not free: the token
    /// table's length lives in the section's first bytes, not the header, so it
    /// costs ONE ≤10-byte range read (memoized). Trivial next to the table it
    /// measures — but it *is* IO, so the asyncify path must drive this call
    /// rather than treat it as a header field.
    pub fn text_index_token_table_len(&self) -> f64 {
        self.rete.text_index_token_table_len().unwrap_or(0) as f64
    }

    /// See [`Graph::text_search_one`] — over the resident remote handle, with
    /// the same token-table-then-posting-lists fault pattern as
    /// [`RemoteGraph::text_search`]. This is the shape the playground's raw
    /// asyncify glue drives: one string in, one string out, marshaled once.
    pub fn text_search_one(&self, phrase: &str, limit: usize) -> Result<String, JsValue> {
        let words: Vec<String> = phrase.split_whitespace().map(str::to_owned).collect();
        text_search_json(&self.rete, &words, None, limit)
    }

    /// See [`card_url`] — the Dataset Card, over the resident handle's reader
    /// (so the header range it already fetched is served from the block cache).
    pub fn card(&self) -> Result<Option<String>, JsValue> {
        let bytes = rete_core::read_metadata_ranged(&self.reader.view()).map_err(err)?;
        Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    /// See [`card_and_build_url`] — card + build record over the resident
    /// handle's reader, still one coalesced range (and served from the block
    /// cache when the header range is already there).
    pub fn card_and_build(&self) -> Result<String, JsValue> {
        let (card, build) =
            rete_core::read_card_and_build_info_ranged(&self.reader.view()).map_err(err)?;
        // Header-only, deliberately: `token_table_bytes` would cost a range read
        // (see [`RemoteGraph::text_index_token_table_len`]), and this call must
        // stay IO-free beyond the card range so the asyncify driver does not
        // have to suspend inside it. A caller that wants the figure asks for it
        // through that export, which the glue does drive.
        let text_index = text_index_json(self.rete.header(), None);
        Ok(card_build_envelope(card, build, text_index))
    }

    /// See [`schema_url`] — the **baked** schema pyramid over the resident
    /// handle. Deliberately never falls back to the scanning [`schema`]: that
    /// would drag the whole remote file across the wire.
    pub fn schema(&self) -> Result<String, JsValue> {
        use serde_json::json;
        let (classes, relations) = rete_core::read_schema_summary_ranged(&self.reader.view())
            .map_err(err)?
            .ok_or_else(|| js_error("file has no schema pyramid"))?;
        let classes: Vec<serde_json::Value> = classes.iter().map(|(c, n)| json!([c, n])).collect();
        let relations: Vec<serde_json::Value> = relations
            .iter()
            .map(|(s, p, o, n)| json!([s, p, o, n]))
            .collect();
        Ok(json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "kind": "schema",
            "classes": classes,
            "relations": relations,
        })
        .to_string())
    }

    /// See [`info`] — read from the resident header, no extra fetch.
    pub fn info(&self) -> String {
        let h = self.rete.header();
        format!(
            r#"{{"schemaVersion":1,"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
            h.quad_count,
            h.term_count,
            h.pyramid_levels,
            // The cheap count (the section's leading varint) — listing every
            // IRI would walk the whole named-graphs directory over the wire.
            self.rete.named_graph_count()
        )
    }

    /// See [`graph_names`].
    pub fn graph_names(&self) -> Result<String, JsValue> {
        serde_json::to_string(&self.rete.graph_names()).map_err(err)
    }

    /// See [`shacl_url`] — validate over the resident handle. The default graph
    /// validates lazily against the index (only the shapes' targets are
    /// fetched), so a shape over a huge remote file stays cheap.
    pub fn shacl(
        &self,
        shapes_turtle: &str,
        graph: Option<String>,
        format: &str,
    ) -> Result<String, JsValue> {
        self.rete.reset_load_failures();
        let out = shacl_rete(&self.rete, shapes_turtle, graph.as_deref(), format)?;
        incomplete_guard(&self.rete, "validation")?;
        Ok(out)
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

/// Parse the fixed-size header and report the byte ranges a *progressive*
/// client needs for the overview — the dictionary and the pyramid summary — plus
/// the (large) index range it can skip, and the metadata (Dataset Card) range.
/// JSON: `{ "dictOffset","dictLen","pyramidOffset","pyramidLen","indexOffset",
/// "indexLen","metadataOffset","metadataLen" }`.
/// The browser fetches bytes `0..HEADER_LEN`, calls this, then range-fetches only the
/// dict + pyramid — never the index. A host with its own byte reader (Node over a
/// local file, say) can use `metadataOffset`/`metadataLen` to read just the card.
#[wasm_bindgen]
pub fn header_ranges(head: &[u8]) -> Result<String, JsValue> {
    let h = Header::from_bytes(head).map_err(err)?;
    Ok(format!(
        r#"{{"schemaVersion":1,"dictOffset":{},"dictLen":{},"pyramidOffset":{},"pyramidLen":{},"indexOffset":{},"indexLen":{},"metadataOffset":{},"metadataLen":{}}}"#,
        h.dictionary_offset,
        h.dictionary_len,
        h.pyramid_meta_offset,
        h.pyramid_meta_len,
        h.root_dir_offset,
        h.root_dir_len,
        h.metadata_offset,
        h.metadata_len,
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
        .ok_or_else(|| js_error("file has no pyramid summary"))?;
    serde_json::to_string(&serde_json::json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
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
    progressive_query_json(bytes, query).map_err(js_error)
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
                "schemaVersion": JSON_SCHEMA_VERSION,
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
                "schemaVersion": JSON_SCHEMA_VERSION,
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
        _ => return Err("summary query shape is not supported by this WASM build".to_string()),
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
    query_json_with(rete, query, format, extra, reason, false)
}

/// The full-options variant: `reason` (OWL 2 QL) and `union_default` (the
/// opt-in union-default-graph mode — a pattern outside GRAPH matches the merge
/// of the default graph and every named graph; see `QueryOpts`).
fn query_json_with(
    rete: &Rete,
    query: &str,
    format: &str,
    extra: &str,
    reason: bool,
    union_default: bool,
) -> Result<String, JsValue> {
    let out = eval_query_with(
        rete,
        query,
        QueryOpts {
            reason,
            union_default_graph: union_default,
        },
    )
    .map_err(err)?;
    Ok(write_query_json(&out, format, extra))
}

/// Serialize an already-evaluated [`QueryOutput`] into the playground JSON
/// envelope. SELECT / ASK / `CONSTRUCT`-as-triples go through the shared,
/// host-tested `rete_core::results_envelope_json` (the allocation-lean direct
/// writer); a `CONSTRUCT` requested as Turtle / JSON-LD wraps the rendered text
/// (those serializers live here).
fn write_query_json(out: &QueryOutput, format: &str, extra: &str) -> String {
    let mut versioned_extra = format!(r#","schemaVersion":{JSON_SCHEMA_VERSION}"#);
    versioned_extra.push_str(extra);
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
            s.push_str(&versioned_extra);
            s.push('}');
            return s;
        }
    }
    rete_core::results_envelope_json(out, &versioned_extra)
}

/// URL scheme naming a `.rete` that is **already in the page** — a `File` the
/// user picked, or any `Blob` — instead of one on a server.
///
/// A local open used to be `file.arrayBuffer()`: the whole file into a JS
/// `ArrayBuffer`, copied again into wasm linear memory, then `Rete::open`
/// decoding every dictionary chunk up front. That is ~6× the file size resident
/// before a single row is answered, which on wasm32 is a hard wall — so a user
/// could query a 16.8 GB graph over HTTP and not open their own 500 MB file
/// from disk (issue #102).
///
/// The fix is not a second reader. [`XhrRangeReader`] already *is* the lazy
/// reader — length, header-window cache, batching, block cache, polyglot base,
/// progress reporting — and only its bottom transport was HTTP-shaped. A URL
/// with this scheme swaps that transport for `Blob.slice()` +
/// [`web_sys::FileReaderSync`], so every `*_url` entry point ([`sparql_url`],
/// [`card_url`], [`schema_url`], [`RemoteGraph`], …) reads a local file exactly
/// as lazily as a remote one, over the same code.
///
/// `FileReaderSync` is genuinely synchronous and exists **only in workers** —
/// the same constraint sync XHR already imposes on this reader, so it fits the
/// contract with no new suspension points. That is also why the **asyncify**
/// build needs nothing extra: a local read never suspends, so it never reaches
/// `env.rete_fetch_ranges`.
///
/// The blob is registered by JS through [`register_local_file`], which owns the
/// URL→`Blob` map for this wasm instance.
const LOCAL_SCHEME: &str = "rete-local:";

fn is_local_url(url: &str) -> bool {
    url.starts_with(LOCAL_SCHEME)
}

thread_local! {
    /// URL → `Blob` for this wasm instance. Thread-local because a `Blob` is a
    /// JS handle and wasm32 is single-threaded here; the experimental `threads`
    /// build would simply not find a blob registered on another thread, and the
    /// read fails with the message below rather than silently reading nothing.
    static LOCAL_BLOBS: std::cell::RefCell<std::collections::HashMap<String, web_sys::Blob>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Register a local `File`/`Blob` under a `rete-local:…` URL, so every `*_url`
/// entry point can range-read it.
///
/// **Worker-only** (the read uses `FileReaderSync`), and the caller mints the
/// URL: a worker can be torn down and rebuilt — the playground does that on a
/// wasm trap, an engine switch, or a phone memory reclaim — and the page must be
/// able to re-register the same file under the same URL so a resident session
/// key stays stable. Re-registering an existing URL replaces the blob.
#[wasm_bindgen]
pub fn register_local_file(url: &str, blob: &web_sys::Blob) -> Result<(), JsValue> {
    if !is_local_url(url) {
        return Err(js_error(format!(
            "a local file URL must start with `{LOCAL_SCHEME}` (got {url})"
        )));
    }
    LOCAL_BLOBS.with(|map| map.borrow_mut().insert(url.to_string(), blob.clone()));
    Ok(())
}

/// Drop a registration made by [`register_local_file`]. Releases this wasm
/// instance's reference to the `Blob`; any open handle over it stops working.
#[wasm_bindgen]
pub fn forget_local_file(url: &str) -> bool {
    LOCAL_BLOBS.with(|map| map.borrow_mut().remove(url).is_some())
}

fn local_blob(url: &str) -> std::io::Result<web_sys::Blob> {
    LOCAL_BLOBS
        .with(|map| map.borrow().get(url).cloned())
        .ok_or_else(|| {
            std::io::Error::other(format!(
                "no local file is registered as {url} — the page must call \
                 register_local_file(url, file) in THIS worker before opening it"
            ))
        })
}

/// The local transport: one `Blob.slice()` per range, read synchronously.
///
/// Batched like the HTTP one (the engine coalesces ranges before it gets here),
/// but with no round trip to pay for — a slice is a view over bytes the browser
/// already has on disk, so the only real cost is the copy of what was asked for.
fn read_local_ranges(url: &str, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
    let (offsets, lens, total) = checked_async_layout(ranges)?;
    if total == 0 {
        return Ok(ranges.iter().map(|_| Vec::new()).collect());
    }
    let blob = local_blob(url)?;
    let size = blob.size();
    let reader = web_sys::FileReaderSync::new().map_err(|e| {
        std::io::Error::other(format!(
            "FileReaderSync is unavailable ({e:?}) — a local .rete is read from a Web Worker only, \
             never the main thread"
        ))
    })?;
    let mut out = Vec::with_capacity(ranges.len());
    for (&offset, &len) in offsets.iter().zip(&lens) {
        let start = offset as f64;
        let end = start + f64::from(len);
        if end > size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("range {offset}..{end} is past the end of {url} ({size} bytes)"),
            ));
        }
        let slice = blob
            .slice_with_f64_and_f64(start, end)
            .map_err(|e| std::io::Error::other(format!("Blob.slice failed on {url}: {e:?}")))?;
        let buffer = reader
            .read_as_array_buffer(&slice)
            .map_err(|e| std::io::Error::other(format!("FileReaderSync failed on {url}: {e:?}")))?;
        let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
        if bytes.len() != len as usize {
            return Err(std::io::Error::other(format!(
                "short local read: got {} of {len} bytes at offset {offset} from {url}",
                bytes.len()
            )));
        }
        out.push(bytes);
    }
    // One batch is one "request" to the UI, matching what the asyncify HTTP
    // reader reports for its concurrent batch — so the playground's range
    // inspector and `stats()` describe local and remote reads on one scale.
    report_progress(total, ranges);
    Ok(out)
}

/// HTTP `Range` reader over **synchronous** XMLHttpRequest — the bridge that
/// lets the lazily-faulting `Rete::open_ranged_lazy` run in the browser with
/// the synchronous engine untouched. Browsers permit sync XHR with a binary
/// response **only inside Web Workers**: call [`sparql_url`] from a worker,
/// never the main thread (where the browser throws).
///
/// Also the **local** reader: a `rete-local:` URL (see [`LOCAL_SCHEME`]) swaps
/// the transport for `Blob.slice()` + `FileReaderSync` and changes nothing else.
struct XhrRangeReader {
    url: String,
    len: u64,
    /// The resource's first [`rete_core::HEADER_LEN`] bytes, cached once.
    ///
    /// Opening a `.rete` reads this window several times over — the sync length
    /// probe reads it, [`polyglot_base`](Self::polyglot_base) needs it, and every
    /// `read_*_ranged` helper starts by reading the header — and the bytes cannot
    /// change under us: the file is immutable for the session, and a host that
    /// served different bytes for the same range would already have broken every
    /// other read. So the first reader to pay for it fills this in and the rest
    /// are served from memory, which is what keeps the CARD tier at its
    /// advertised two requests.
    head: std::sync::OnceLock<Vec<u8>>,
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
    /// LEAF panic reporter (deliberately NOT in asyncify-imports): the panic
    /// hook passes the raw `Location` pointers so the host can log file:line
    /// without any formatting — see `__start`.
    fn rete_panic_report(file_ptr: *const u8, file_len: usize, line: u32);
}

fn checked_async_layout(ranges: &[(u64, u64)]) -> std::io::Result<(Vec<u64>, Vec<u32>, usize)> {
    let offs: Vec<u64> = ranges.iter().map(|&(offset, _)| offset).collect();
    let lens: Vec<u32> = ranges
        .iter()
        .map(|&(_, len)| {
            u32::try_from(len).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("range length {len} exceeds the wasm32 u32 length type"),
                )
            })
        })
        .collect::<std::io::Result<_>>()?;
    let total = lens.iter().try_fold(0usize, |sum, &len| {
        sum.checked_add(len as usize).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "range length sum overflow",
            )
        })
    })?;
    let _total_u32 = u32::try_from(total).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "range response exceeds the wasm32 address space",
        )
    })?;
    Ok((offs, lens, total))
}

fn split_range_response(
    ranges: &[(u64, u64)],
    dst: Vec<u8>,
    got: usize,
    source: &str,
) -> std::io::Result<Vec<Vec<u8>>> {
    let (_, lens, total) = checked_async_layout(ranges)?;
    if dst.len() != total {
        return Err(std::io::Error::other(format!(
            "async fetch destination has {} bytes, expected {total} for {source}",
            dst.len()
        )));
    }
    if got != total {
        return Err(std::io::Error::other(format!(
            "async fetch returned {got} of {total} bytes for {source}"
        )));
    }

    let mut out = Vec::with_capacity(ranges.len());
    let mut pos = 0usize;
    for len in lens {
        let end = pos + len as usize;
        out.push(dst[pos..end].to_vec());
        pos = end;
    }
    Ok(out)
}

#[cfg(test)]
mod async_range_tests {
    use super::{checked_async_layout, split_range_response};

    #[test]
    fn rejects_a_range_larger_than_the_wasm32_length_type() {
        let error = checked_async_layout(&[(0, u64::from(u32::MAX) + 1)]).unwrap_err();
        assert!(error.to_string().contains("u32"));
    }

    #[test]
    fn rejects_a_total_larger_than_the_wasm32_address_space() {
        let error = checked_async_layout(&[(0, u64::from(u32::MAX)), (0, 1)]).unwrap_err();
        assert!(error.to_string().contains("address space"));
    }

    #[test]
    fn accepts_no_ranges_and_preserves_zero_length_ranges() {
        let (offs, lens, total) = checked_async_layout(&[]).unwrap();
        assert!(offs.is_empty());
        assert!(lens.is_empty());
        assert_eq!(total, 0);

        let (_, lens, total) = checked_async_layout(&[(7, 0), (9, 0)]).unwrap();
        assert_eq!(lens, vec![0, 0]);
        assert_eq!(total, 0);
    }

    #[test]
    fn rejects_a_short_javascript_response() {
        let error = split_range_response(&[(0, 4)], vec![1, 2, 3, 4], 3, "fixture").unwrap_err();
        assert!(error.to_string().contains("returned 3 of 4 bytes"));
    }

    #[test]
    fn only_the_local_scheme_routes_to_the_blob_transport() {
        use super::is_local_url;
        assert!(is_local_url("rete-local:1/mirbase.rete"));
        // Everything the reader has ever been given before must stay on HTTP —
        // a URL that merely CONTAINS the token is not a local file.
        assert!(!is_local_url("https://example.org/rete-local:1/x.rete"));
        assert!(!is_local_url(
            "https://data.graphplaza.com/mirbase/mirbase.rete"
        ));
        assert!(!is_local_url("file:///tmp/x.rete"));
        assert!(!is_local_url(""));
    }
}

#[cfg(feature = "asyncify")]
impl XhrRangeReader {
    /// Fetch all `ranges` through the async import in one suspend/resume, then
    /// split the concatenated bytes back into per-range buffers.
    fn read_ranges_async(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let (offs, lens, total) = checked_async_layout(ranges)?;
        if total == 0 {
            return Ok(ranges.iter().map(|_| Vec::new()).collect());
        }
        let mut dst = vec![0u8; total];
        if offs.len() != ranges.len() || lens.len() != ranges.len() || dst.len() != total {
            return Err(std::io::Error::other(
                "async range layout does not match its allocated buffers",
            ));
        }
        // SAFETY: self.url, offs, and lens own their pointer/length pairs for
        // this call; dst owns exactly total writable bytes. The imported
        // function contract writes at most that concatenated length and returns
        // before any of these allocations leave scope.
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
        let out = split_range_response(ranges, dst, got, &self.url)?;
        report_progress(total, ranges);
        Ok(out)
    }
}

impl XhrRangeReader {
    /// Probe the resource length. HTTP's own signals are unreliable
    /// cross-origin — a host may advertise the size of a **compressed**
    /// representation in `Content-Length` (GitHub Pages HEADs a 71 MB `.rete`
    /// as its 58 MB gzip; `Content-Encoding` is not CORS-safelisted, so JS
    /// cannot even see the lie) while range requests address the identity
    /// bytes, and may hide `Content-Range` by omitting
    /// `Access-Control-Expose-Headers` (GitHub Pages, Zenodo). So the probe
    /// reads the file's OWN first KiB — the `.rete` header, whose section
    /// directory pins the exact length (issue #95) — and only falls back to
    /// the transport's numbers for a resource that isn't a `.rete`.
    fn open(url: &str) -> Result<Self, JsValue> {
        // A LOCAL blob needs no probe at all: its length is a property, not a
        // network fact, and none of the reasons the HTTP probe exists (a
        // compressing host, a hidden Content-Range, a 405 on HEAD) can apply.
        // The header window is faulted on first use like any other range, so
        // opening a local file costs exactly ONE read before the engine starts.
        // Placed above both build variants because it is the same either way.
        if is_local_url(url) {
            // `Blob.size` is an f64: reject NaN/∞/0 explicitly rather than
            // letting `as u64` saturate one of them into a plausible length.
            let len = local_blob(url).map_err(|e| js_error(e.to_string()))?.size();
            if !len.is_finite() || len < 1.0 {
                return Err(js_error(format!("{url} is an empty local file")));
            }
            return Ok(Self {
                url: url.to_string(),
                len: len as u64,
                head: std::sync::OnceLock::new(),
            });
        }
        // Asyncify build: probe the length via the async import — no sync XHR.
        // The JS side (`__reteDoLen` in scripts/build_playground.py) applies
        // this same derive-from-the-header-then-validate-the-footer strategy.
        #[cfg(feature = "asyncify")]
        {
            let mut len: u64 = 0;
            // SAFETY: url owns url.as_ptr() for exactly url.len() bytes and len
            // owns one writable u64. The import writes only that value and
            // returns before either borrowed allocation leaves scope.
            let ok = unsafe { rete_file_len(url.as_ptr(), url.len(), &mut len as *mut u64) };
            if ok == 0 || len == 0 {
                return Err(js_error(format!("could not determine length of {url}")));
            }
            // The async length probe reads the window on the JS side and only
            // hands back the length, so the first `head_window()` pays for it.
            Ok(Self {
                url: url.to_string(),
                len,
                head: std::sync::OnceLock::new(),
            })
        }
        #[cfg(not(feature = "asyncify"))]
        {
            // Hugging Face's Space gateway is intermittently flaky on the length
            // probe (a 200 with no Content-Length, a chunked response with neither
            // header, …). A fresh attempt usually lands on a healthy response,
            // so retry a few times before surfacing the error.
            let mut last = format!("could not determine length of {url}");
            for _ in 0..4 {
                match Self::probe_len(url) {
                    Ok((len, head)) => {
                        let cached = std::sync::OnceLock::new();
                        if !head.is_empty() {
                            let _ = cached.set(head);
                        }
                        return Ok(Self {
                            url: url.to_string(),
                            len,
                            head: cached,
                        });
                    }
                    Err(e) => last = e,
                }
            }
            Err(js_error(last))
        }
    }

    /// The cached first [`rete_core::HEADER_LEN`] bytes, fetching them once if
    /// nobody has yet. `None` only when the resource cannot be read at all — the
    /// open that follows reports the real error.
    fn head_window(&self) -> Option<&[u8]> {
        if let Some(head) = self.head.get() {
            return Some(head);
        }
        let head = self.fetch_at(0, rete_core::HEADER_LEN as u64).ok()?;
        let _ = self.head.set(head);
        self.head.get().map(Vec::as_slice)
    }

    /// Where the `.rete` starts inside this resource: `0` for an ordinary file,
    /// and the size of the HTML shell for a **polyglot** — a file that is both a
    /// web page and a graph, whose first bytes carry a `RETE-BASE:` marker naming
    /// the offset. Reads only the header window, which the open that follows
    /// needs anyway and now gets from [`head_window`](Self::head_window).
    fn polyglot_base(&self) -> u64 {
        let Some(head) = self.head_window() else {
            return 0;
        };
        if head.starts_with(&rete_core::MAGIC) {
            return 0;
        }
        rete_core::detect_polyglot_base(head).unwrap_or(0)
    }

    /// A HEAD length probe: `Content-Length` is CORS-safelisted (readable
    /// cross-origin with no `access-control-expose-headers` entry needed).
    /// **Last-resort fallback only**: on a transparently-compressing host this
    /// is the size of the gzip representation, not the file (issue #95), which
    /// is exactly why [`probe_len`](Self::probe_len) prefers the file's own
    /// header. Returns `None` on a non-2xx (some hosts 405 HEAD) or a
    /// missing/zero length.
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

    /// One ranged `GET bytes=first-last`, returning `(status, total from a
    /// visible Content-Range, body)`. The total is `None` when the header is
    /// absent, hidden by CORS, or `bytes a-b/*`.
    #[cfg(not(feature = "asyncify"))]
    fn ranged_get(url: &str, first: u64, last: u64) -> Result<(u16, Option<u64>, Vec<u8>), String> {
        let err = |m: &str| m.to_string();
        let xhr = web_sys::XmlHttpRequest::new().map_err(|_| err("xhr"))?;
        xhr.open_with_async("GET", url, false)
            .map_err(|_| err("open"))?;
        xhr.set_response_type(web_sys::XmlHttpRequestResponseType::Arraybuffer);
        xhr.set_request_header("Range", &format!("bytes={first}-{last}"))
            .map_err(|_| err("range header"))?;
        xhr.send()
            .map_err(|_| format!("probe {url}: network error"))?;
        let status = xhr.status().map_err(|_| err("status"))?;
        // `Content-Range: bytes 0-1023/12345` — the part after `/` is the total.
        let total = xhr
            .get_response_header("Content-Range")
            .ok()
            .flatten()
            .and_then(|v| {
                v.rsplit('/')
                    .next()
                    .and_then(|t| t.trim().parse::<u64>().ok())
            });
        let body = xhr
            .response()
            .ok()
            .map(|r| js_sys::Uint8Array::new(&r).to_vec())
            .unwrap_or_default();
        Ok((status, total, body))
    }

    /// One length probe. Reads the resource's first KiB over a ranged `GET`;
    /// if that is a `.rete` header, the length is **derived from the file
    /// itself** (`Header::expected_file_len`: sections are back-to-back and
    /// the file ends with the 4-byte `RETE` footer) and, unless a visible
    /// `Content-Range` already confirms it, **validated** by reading those 4
    /// footer bytes — one extra tiny request, only on hosts whose headers are
    /// unusable. Non-`.rete` resources fall back to `Content-Range`, then
    /// HEAD. A 206's `Content-Length` is never believed (it is the size of
    /// the partial body, and taking it as the file size made every later read
    /// "range out of bounds").
    ///
    /// Returns the length **and the head window it read** — the caller keeps it
    /// so polyglot detection costs no second request.
    #[cfg(not(feature = "asyncify"))]
    fn probe_len(url: &str) -> Result<(u64, Vec<u8>), String> {
        let (status, cr_total, body) = Self::ranged_get(url, 0, rete_core::HEADER_LEN as u64 - 1)?;
        // Only a genuine header window is worth keeping (a Range-ignoring host
        // may have sent the whole file here).
        let head = |body: &Vec<u8>| {
            if body.len() <= rete_core::HEADER_LEN {
                body.clone()
            } else {
                body[..rete_core::HEADER_LEN].to_vec()
            }
        };
        if status == 200 {
            // Host ignored Range and sent the whole (decoded) body. Range
            // reads are rejected loudly in read_at anyway; report the honest
            // decoded length so the failure names the real problem there.
            if !body.is_empty() {
                return Ok((body.len() as u64, head(&body)));
            }
            return Err(format!("probe {url}: 200 with an empty body"));
        }
        if status != 206 {
            return Err(format!("probe {url}: status {status}"));
        }

        // A `.rete` self-describes its length; that is the only signal a
        // compressing/CORS-hiding host cannot skew (issue #95).
        if let Ok(header) = rete_core::Header::from_bytes(&body) {
            if let Some(derived) = header.expected_file_len() {
                if cr_total == Some(derived) {
                    return Ok((derived, head(&body))); // transport agrees — no extra request
                }
                // Ask the file: its last 4 bytes are the `RETE` footer. A 206
                // with exactly those bytes proves `derived` addresses real
                // identity bytes; anything else means the file is truncated
                // or the host's ranges don't address the file's bytes.
                let (ts, _, tail) = Self::ranged_get(url, derived - 4, derived - 1)?;
                if ts == 206 && tail == rete_core::MAGIC {
                    return Ok((derived, head(&body)));
                }
                return Err(format!(
                    "length probe disagrees with the file: its header derives \
                     {derived} bytes but the host{} has no RETE footer at \
                     {}..{} (tail probe: status {ts}) — the file is truncated, \
                     or the host serves ranges over a compressed representation",
                    match cr_total {
                        Some(t) => format!(" reports {t} bytes and"),
                        None => String::from(" hides Content-Range and"),
                    },
                    derived - 4,
                    derived,
                ));
            }
        }
        if body.starts_with(&[0x1f, 0x8b]) {
            // The ranged body is a slice of a GZIP stream: the host applied
            // `Content-Encoding` to the range response, so byte offsets do not
            // address the file's bytes at all. No length can fix that.
            return Err(format!(
                "the host serves HTTP ranges over a gzip-compressed representation \
                 of {url}; range offsets cannot address the file's bytes"
            ));
        }

        // Not a `.rete` header: fall back to the transport's signals. A POLYGLOT
        // lands here — its byte 0 is `<`, so the header parse above failed — and the
        // head we keep is exactly what `polyglot_base` needs to find the graph.
        if let Some(total) = cr_total {
            return Ok((total, head(&body)));
        }
        if let Some(len) = Self::head_len(url) {
            return Ok((len, head(&body)));
        }
        Err(format!("could not determine length of {url}"))
    }
}

impl RangeReader for XhrRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    /// The asyncify build fires its batched ranges as one concurrent
    /// `Promise.all` of fetches; the sync build is serial XHR unless the
    /// opt-in COI fetch-worker pool (`globalThis.reteReadMany`) is installed.
    /// 16 matches the pool size and the CLI's thread fan-out.
    fn concurrency(&self) -> usize {
        // A local blob has no round trip to hide, so the planner may probe as
        // freely as the concurrent HTTP readers do — and a wider batch means
        // fewer crossings of the wasm↔JS boundary, which is the only real cost
        // here. Same number as the asyncify reader and the CLI's fan-out.
        if is_local_url(&self.url) {
            return 16;
        }
        #[cfg(feature = "asyncify")]
        {
            16
        }
        #[cfg(not(feature = "asyncify"))]
        {
            let has_pool =
                js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str("reteReadMany"))
                    .map(|h| h.is_function())
                    .unwrap_or(false);
            if has_pool {
                16
            } else {
                1
            }
        }
    }

    /// Reads that land entirely inside the cached header window are answered
    /// from memory: opening a file re-reads those bytes several times (length
    /// probe, polyglot marker, then every `read_*_ranged` helper's own header
    /// read) and they are immutable for the session, so paying for them once is
    /// the difference between the CARD tier costing two requests and four.
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        if let Some(head) = self.head.get() {
            if let Some(end) = offset.checked_add(len) {
                if end <= head.len() as u64 {
                    return Ok(head[offset as usize..end as usize].to_vec());
                }
            }
        }
        self.fetch_at(offset, len)
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
        // Local: the whole batch is sliced in one call, in either build.
        if is_local_url(&self.url) {
            return read_local_ranges(&self.url, ranges);
        }
        // Asyncify build: fetch the whole batch concurrently in one suspend.
        #[cfg(feature = "asyncify")]
        {
            self.read_ranges_async(ranges)
        }
        #[cfg(not(feature = "asyncify"))]
        {
            let (_, _, total) = checked_async_layout(ranges)?;
            if ranges.len() > 1 {
                if let Some(buf) = self.read_many_via_pool(ranges) {
                    if buf.len() == total {
                        return split_range_response(ranges, buf, total, "globalThis.reteReadMany");
                    }
                }
            }
            ranges.iter().map(|&(o, l)| self.read_at(o, l)).collect()
        }
    }
}

/// The raw transport, below the header cache in [`RangeReader::read_at`].
impl XhrRangeReader {
    /// One real range request. Never consults the cached header window — this is
    /// what fills it.
    fn fetch_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        // Local: one `Blob.slice()` + `FileReaderSync`, in either build.
        if is_local_url(&self.url) {
            return read_local_ranges(&self.url, &[(offset, len)])?
                .pop()
                .ok_or_else(|| std::io::Error::other("local read returned no range"));
        }
        // Asyncify build: a single read is a 1-range async fetch (suspends).
        #[cfg(feature = "asyncify")]
        {
            Ok(self
                .read_ranges_async(&[(offset, len)])?
                .into_iter()
                .next()
                .unwrap())
        }
        #[cfg(not(feature = "asyncify"))]
        {
            let js = |e: JsValue| std::io::Error::other(format!("XHR error: {e:?}"));
            let xhr = web_sys::XmlHttpRequest::new().map_err(js)?;
            xhr.open_with_async("GET", &self.url, false).map_err(js)?;
            xhr.set_response_type(web_sys::XmlHttpRequestResponseType::Arraybuffer);
            let end = offset
                .checked_add(len - 1)
                .ok_or_else(|| std::io::Error::other("HTTP range end overflow"))?;
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
            let requested = usize::try_from(len)
                .map_err(|_| std::io::Error::other("range exceeds wasm32 memory"))?;
            buf.truncate(requested);
            // Report this fetch to an optional progress hook so a worker can stream
            // live "N requests · M bytes" updates to the UI *during* the otherwise
            // opaque synchronous query (postMessage works mid-sync-call).
            report_progress(buf.len(), &[(offset, len)]);
            Ok(buf)
        }
    }
}

/// Notify an optional `globalThis.reteProgress(bytes, spans, n)` hook of one
/// completed fetch (the worker forwards it to the UI and keeps a per-query
/// log). `spans` is a JS array of `"start-end"` byte-offset strings — capped at
/// [`PROGRESS_SPAN_CAP`] entries so a huge coalesced batch doesn't build a huge
/// array — and `n` is the TRUE span count. The offsets used to be dropped here
/// (only the byte count was forwarded), which left the playground's
/// "Range requests" inspector promising a `start-end` column it could never
/// fill. A no-op when unset.
const PROGRESS_SPAN_CAP: usize = 256;

fn report_progress(bytes: usize, ranges: &[(u64, u64)]) {
    let g = js_sys::global();
    if let Ok(f) = js_sys::Reflect::get(&g, &JsValue::from_str("reteProgress")) {
        if let Ok(f) = f.dyn_into::<js_sys::Function>() {
            let spans = js_sys::Array::new();
            for &(offset, len) in ranges.iter().take(PROGRESS_SPAN_CAP) {
                let end = offset + len.saturating_sub(1);
                spans.push(&JsValue::from_str(&format!("{offset}-{end}")));
            }
            let _ = f.call3(
                &JsValue::NULL,
                &JsValue::from_f64(bytes as f64),
                &spans.into(),
                &JsValue::from_f64(ranges.len() as f64),
            );
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
        let (_, checked_lens, _) = checked_async_layout(ranges).ok()?;
        let count = u32::try_from(ranges.len()).ok()?;
        let offs = js_sys::Float64Array::new_with_length(count);
        let lens = js_sys::Float64Array::new_with_length(count);
        const MAX_SAFE_JS_INTEGER: u64 = (1_u64 << 53) - 1;
        for (i, (&(offset, _), &len)) in ranges.iter().zip(&checked_lens).enumerate() {
            if offset > MAX_SAFE_JS_INTEGER {
                return None;
            }
            let index = u32::try_from(i).ok()?;
            offs.set_index(index, offset as f64);
            lens.set_index(index, f64::from(len));
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

/// The physical range reader behind a remote open, plus the **base offset** of
/// the `.rete` inside the resource.
///
/// For an ordinary `.rete` the base is `0` and every method is a pass-through.
/// For a **polyglot** — one object that is simultaneously an HTML page and a
/// graph, byte 0 being `<` — the base is where the appended `.rete` starts, read
/// from the `RETE-BASE:` marker the page carries in its first
/// [`rete_core::HEADER_LEN`] bytes (written by
/// `experiments/polyglot/build_polyglot.py`). Every graph read then goes through
/// [`RemoteReader::view`], an [`OffsetReader`] that makes the embedded graph read
/// as if it began at byte 0 — so a polyglot is range-read exactly as lazily as a
/// plain file instead of being downloaded whole.
///
/// [`len`](Self::len) reports the **graph's** length (the shell excluded), while
/// `bytes_read`/`requests` stay the true wire cost, shell probe included.
#[derive(Clone)]
struct RemoteReader {
    counting: std::sync::Arc<CountingReader<XhrRangeReader>>,
    base: u64,
}

impl RemoteReader {
    /// Open `url` and resolve its polyglot base — free on the sync build, where
    /// the length probe has already read the header window.
    fn open(url: &str) -> Result<Self, JsValue> {
        let xhr = XhrRangeReader::open(url)?;
        let base = xhr.polyglot_base();
        Ok(Self {
            counting: std::sync::Arc::new(CountingReader::new(xhr)),
            base,
        })
    }

    /// The graph-relative reader: absolute for a plain `.rete`, shifted past the
    /// HTML shell for a polyglot.
    fn view(&self) -> OffsetReader<std::sync::Arc<CountingReader<XhrRangeReader>>> {
        OffsetReader::new(self.counting.clone(), self.base)
    }

    /// Byte length of the embedded `.rete` (not of the enclosing resource).
    fn len(&self) -> u64 {
        self.counting.len().saturating_sub(self.base)
    }

    fn bytes_read(&self) -> u64 {
        self.counting.bytes_read()
    }

    fn requests(&self) -> u64 {
        self.counting.requests()
    }
}

fn open_url(url: &str) -> Result<(RemoteReader, Rete), JsValue> {
    // `reader` counts the PHYSICAL fetches; a read-through block cache above it
    // turns the query's scattered range reads into a few aligned block fetches
    // (and reuses them) — working over any single-range backend (S3, a CDN),
    // not just a multi-range gateway. The block fetches still go through
    // `read_many`, so a multi-range host coalesces them further. The block size
    // is auto-tuned from the file size (known at open, no download).
    let reader = RemoteReader::open(url)?;
    let cached = std::sync::Arc::new(BlockCacheReader::new(
        reader.counting.clone(),
        auto_block(reader.len()),
    ));
    // The offset shim sits ABOVE the cache so blocks stay aligned to the
    // resource's absolute offsets (a plain file's base is 0 — a no-op).
    let mut rete = Rete::open_ranged_lazy(OffsetReader::new(cached, reader.base)).map_err(err)?;
    // SERVICE blocks federate to remote SPARQL endpoints over sync XHR.
    rete.set_service_client(Box::new(XhrServiceClient));
    Ok((reader, rete))
}

/// Turn a partial lazy fetch into a hard error: a `*_url` task must never
/// return a result computed over silently-incomplete data.
fn incomplete_guard(rete: &Rete, what: &str) -> Result<(), JsValue> {
    if rete.index_incomplete() {
        return Err(js_error(format!(
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
    .map_err(js_error)?;
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        return Err(js_error(
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
    serde_json::to_string(&json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
        "kind": "select",
        "vars": vars,
        "rows": rows,
        "communities": communities,
    }))
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
    serde_json::to_string(&json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
        "fileLength": bytes.len(),
        "segments": segments,
    }))
    .map_err(err)
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
        return serde_json::to_string(&json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "rounds": 0,
            "levels": [],
        }))
        .map_err(err);
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
    serde_json::to_string(&json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
        "rounds": rounds,
        "levels": levels,
    }))
    .map_err(err)
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
            return Err(js_error(
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
/// `reasoning_json` envelope (no `remote` block).
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
            _ => return Err(js_error(
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        .ok_or_else(|| js_error("file has no schema pyramid"))?;
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
    let reader = RemoteReader::open(url)?;
    let points = rete_core::read_schema_coherence_ranged(&reader.view())
        .map_err(err)?
        .ok_or_else(|| js_error("file has no schema pyramid"))?;
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
    let reader = RemoteReader::open(url)?;
    let (classes, relations) = rete_core::read_schema_summary_ranged(&reader.view())
        .map_err(err)?
        .ok_or_else(|| js_error("file has no schema pyramid"))?;
    let classes: Vec<serde_json::Value> = classes.iter().map(|(c, n)| json!([c, n])).collect();
    let relations: Vec<serde_json::Value> = relations
        .iter()
        .map(|(s, p, o, n)| json!([s, p, o, n]))
        .collect();
    Ok(json!({
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
        "schemaVersion": JSON_SCHEMA_VERSION,
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
    js_error(e.to_string())
}

fn js_error(message: impl AsRef<str>) -> JsValue {
    js_sys::Error::new(message.as_ref()).into()
}
