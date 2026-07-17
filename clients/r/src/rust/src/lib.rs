//! R bindings (extendr) for the rete engine: open a `.rete` file — local
//! path, HTTP(S) URL, or a raw vector — and query it with SPARQL. The method
//! surface mirrors the Python client; structured results cross into R as the
//! engine's JSON envelope and are parsed by the package's R layer. Errors
//! surface as ordinary R conditions via `throw_r_error`.

mod readers;

use std::sync::Arc;

use extendr_api::prelude::*;
use rete_core::{
    eval_query, eval_query_reasoned, results_envelope_json, schema_classes, schema_summary,
    BlockCacheReader, CountingReader, RangeReader, Rete, DEFAULT_BLOCK,
};

use readers::{AnyReader, HttpRangeReader, LocalRangeReader};

/// Version of the JSON envelopes (mirrors the other clients).
const JSON_SCHEMA_VERSION: u8 = 1;

fn fail<E: std::fmt::Display>(e: E) -> ! {
    throw_r_error(e.to_string())
}

/// Block size for the read-through cache, auto-tuned from the file size
/// (same policy as the playground and the Python client).
fn auto_block(len: u64) -> u64 {
    const MB: u64 = 1 << 20;
    let mult: u64 = if len > 100 * MB {
        8
    } else if len > 10 * MB {
        4
    } else {
        2
    };
    mult * DEFAULT_BLOCK
}

fn open_lazy(reader: AnyReader) -> RGraph {
    let file_len = reader.len();
    let counting = Arc::new(CountingReader::new(reader));
    let cached = Arc::new(BlockCacheReader::new(counting.clone(), auto_block(file_len)));
    let rete = match Rete::open_ranged_lazy(cached) {
        Ok(rete) => rete,
        Err(e) => fail(e),
    };
    RGraph {
        rete,
        reader: Some(counting),
        file_len,
    }
}

/// A `.rete` opened once and kept resident, so repeated queries reuse the
/// decoded dictionary chunks, faulted index tiles, and the block cache.
/// The struct-level `#[extendr]` generates the Robj conversions (extendr 0.8
/// splits them from the impl-level macro).
#[extendr]
struct RGraph {
    rete: Rete,
    reader: Option<Arc<CountingReader<AnyReader>>>,
    file_len: u64,
}

#[extendr]
impl RGraph {
    /// Open a local `.rete` file lazily (positional reads).
    fn from_path(path: &str) -> Self {
        match LocalRangeReader::open(path) {
            Ok(reader) => open_lazy(AnyReader::Local(reader)),
            Err(e) => fail(e),
        }
    }

    /// Open a remote `.rete` over HTTP range requests, lazily.
    fn from_url(url: &str) -> Self {
        match HttpRangeReader::open(url) {
            Ok(reader) => open_lazy(AnyReader::Http(reader)),
            Err(e) => fail(e),
        }
    }

    /// Open a `.rete` image held in memory (a raw vector from R).
    fn from_bytes(data: Robj) -> Self {
        let Some(bytes) = data.as_raw_slice() else {
            fail("`source` must be a raw vector holding a complete .rete image");
        };
        let rete = match Rete::open(bytes) {
            Ok(rete) => rete,
            Err(e) => fail(e),
        };
        RGraph {
            rete,
            reader: None,
            file_len: bytes.len() as u64,
        }
    }

    /// Run a SPARQL query; returns the JSON result envelope (parsed in R).
    /// `reason = TRUE` turns on OWL 2 QL entailment by query rewriting.
    fn query(&self, query: &str, reason: bool) -> String {
        let out = match if reason {
            eval_query_reasoned(&self.rete, query)
        } else {
            eval_query(&self.rete, query)
        } {
            Ok(out) => out,
            Err(e) => fail(e),
        };
        if self.rete.index_incomplete() {
            fail("a range fetch failed mid-query; refusing to return incomplete results — retry");
        }
        results_envelope_json(
            &out,
            &format!(r#","schemaVersion":{JSON_SCHEMA_VERSION}"#),
        )
    }

    /// Header summary as JSON.
    fn info(&self) -> String {
        let h = self.rete.header();
        format!(
            r#"{{"schemaVersion":{JSON_SCHEMA_VERSION},"quads":{},"terms":{},"pyramidLevels":{},"namedGraphs":{}}}"#,
            h.quad_count,
            h.term_count,
            h.pyramid_levels,
            self.rete.graph_names().len()
        )
    }

    /// Named-graph IRI tokens of a dataset (cleaned in R).
    fn graph_names(&self) -> Vec<String> {
        self.rete
            .graph_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Label prefix search as JSON `[{"label":…,"subject":…}, …]`.
    fn prefix_search(&self, prefix: &str, limit: i32) -> String {
        let hits: Vec<serde_json::Value> = self
            .rete
            .prefix_search(prefix, limit.max(0) as usize)
            .into_iter()
            .map(|(label, subject)| serde_json::json!({ "label": label, "subject": subject }))
            .collect();
        serde_json::to_string(&hits).unwrap_or_else(|e| fail(e))
    }

    /// Full-text search over the TEXT_INDEX; returns subject tokens.
    fn text_search(&self, words: Vec<String>, contains: &str, limit: i32) -> Vec<String> {
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        let contains = if contains.is_empty() {
            None
        } else {
            Some(contains)
        };
        self.rete
            .text_search(&refs, contains, limit.max(0) as usize)
    }

    /// Class/predicate profile as JSON.
    fn schema(&self) -> String {
        serde_json::to_string(&serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "classes": schema_classes(&self.rete),
            "relations": schema_summary(&self.rete),
        }))
        .unwrap_or_else(|e| fail(e))
    }

    /// The embedded Dataset Card JSON, or "" when the file carries none.
    /// Lazy opens fetch only the metadata section's byte range.
    fn card(&self) -> String {
        if let Some(reader) = &self.reader {
            let bytes = match rete_core::read_metadata_ranged(reader.as_ref()) {
                Ok(bytes) => bytes,
                Err(e) => fail(e),
            };
            return bytes
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
        }
        self.rete
            .metadata()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default()
    }

    /// Cumulative physical fetch counters as JSON.
    fn stats(&self) -> String {
        let (bytes, requests) = match &self.reader {
            Some(r) => (r.bytes_read(), r.requests()),
            None => (0, 0),
        };
        format!(
            r#"{{"schemaVersion":{JSON_SCHEMA_VERSION},"fileLength":{},"bytes":{bytes},"requests":{requests}}}"#,
            self.file_len
        )
    }

    /// blake3-16 hex content hash.
    fn content_hash(&self) -> String {
        self.rete
            .header()
            .content_hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }
}

/// Serialize the Dataset Card: caller's curated JSON plus the count fields
/// the CLI card schema requires (mirrors the Python client's card_bytes).
fn card_bytes(curated: Option<serde_json::Value>, stats: &rete_core::ingest::BuildStats) -> Vec<u8> {
    let Some(mut value) = curated else {
        return Vec::new();
    };
    let obj = value.as_object_mut().expect("validated as object");
    obj.insert("triple_count".into(), stats.default_triples.into());
    obj.insert("quad_count".into(), stats.statements.into());
    obj.insert("named_graph_count".into(), stats.named_graphs.into());
    obj.insert("term_count".into(), stats.terms.into());
    obj.insert(
        "format_version".into(),
        rete_core::CURRENT_FORMAT_VERSION.into(),
    );
    serde_json::to_vec(&value).unwrap_or_default()
}

/// Build a complete `.rete` file image from RDF text. `card_json` may be ""
/// (no card); `pyramid_algo` is "louvain", "types", or "none". Internal —
/// users call the documented `rete_build()`.
/// @noRd
#[extendr]
fn build_dataset(
    text: &str,
    format: &str,
    card_json: &str,
    pyramid_algo: &str,
    text_index: bool,
) -> Vec<u8> {
    let (with_pyramid, algo) = match pyramid_algo {
        "none" => (false, rete_core::PyramidAlgo::Louvain),
        other => match rete_core::PyramidAlgo::from_cli(other) {
            Some(algo) => (true, algo),
            None => fail(format!("unknown pyramid algo {other:?}")),
        },
    };
    let curated: Option<serde_json::Value> = if card_json.is_empty() {
        None
    } else {
        match serde_json::from_str::<serde_json::Value>(card_json) {
            Ok(value) if value.is_object() => Some(value),
            Ok(_) => fail("card must be a JSON object"),
            Err(e) => fail(format!("card is not valid JSON: {e}")),
        }
    };
    let quads = match rete_core::ingest::parse_statements(text, format) {
        Ok(quads) => quads,
        Err(e) => fail(e),
    };
    if quads.is_empty() {
        fail("no statements parsed (empty input or only comments)");
    }
    let (bytes, _stats) = rete_core::ingest::assemble_dataset_with_opts_algo(
        quads,
        with_pyramid,
        text_index,
        None,
        algo,
        move |stats, _quads| card_bytes(curated, stats),
    );
    bytes
}

// Macro to generate exports.
extendr_module! {
    mod rete;
    fn build_dataset;
    impl RGraph;
}
