//! Python bindings for the rete engine: open a `.rete` file — local path,
//! HTTP(S) URL, in-memory bytes, or a user-supplied reader — and query it with
//! SPARQL. The method surface mirrors the wasm `Graph`/`RemoteGraph`
//! (`crates/rete-wasm`); results use the same JSON envelope, parsed into
//! Python values by the pure-Python `rete_graph` wrapper package.
//!
//! Every potentially slow call runs inside `Python::allow_threads`, so remote
//! range fetches (which fan out over a thread pool) never hold the GIL.

mod readers;

#[cfg(not(target_os = "emscripten"))]
use std::io::Read;
use std::sync::Arc;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use rete_core::{
    eval_query, eval_query_reasoned, results_envelope_json, schema_classes, schema_summary,
    validate_shacl, BlockCacheReader, CountingReader, DataGraph, RangeReader, Rete, ReteGraph,
    ShaclShapes, DEFAULT_BLOCK,
};
#[cfg(not(target_os = "emscripten"))]
use rete_core::{parse_sparql_json_results, Binding, ServiceClient};

#[cfg(not(target_os = "emscripten"))]
use readers::HttpRangeReader;
use readers::{AnyReader, LocalRangeReader, PyRangeReader};

/// Version of the JSON envelopes (mirrors rete-wasm's `JSON_SCHEMA_VERSION`).
const JSON_SCHEMA_VERSION: u8 = 1;

/// Cap on a SERVICE response body — a runaway endpoint must not exhaust RAM.
#[cfg(not(target_os = "emscripten"))]
const MAX_SERVICE_RESPONSE: u64 = 256 * 1024 * 1024;

/// `SERVICE <endpoint> { … }` transport: SPARQL Protocol over blocking HTTP,
/// the native twin of the CLI's `HttpServiceClient`. Native-only — a SERVICE
/// block on the Pyodide build reports the engine's no-client error.
#[cfg(not(target_os = "emscripten"))]
struct HttpServiceClient;

#[cfg(not(target_os = "emscripten"))]
impl ServiceClient for HttpServiceClient {
    fn query(&self, endpoint: &str, query: &str) -> Result<Vec<Binding>, String> {
        let agent = ureq::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build();
        let resp = agent
            .post(endpoint)
            .set("Accept", "application/sparql-results+json")
            // Public endpoints (Wikidata in particular) require an identifying
            // User-Agent and may throttle or reject the library default.
            .set(
                "User-Agent",
                "rete-graph (python; SPARQL SERVICE federation)",
            )
            .send_form(&[("query", query)])
            .map_err(|e| e.to_string())?;
        let mut body = String::new();
        resp.into_reader()
            .take(MAX_SERVICE_RESPONSE)
            .read_to_string(&mut body)
            .map_err(|e| format!("reading results: {e}"))?;
        parse_sparql_json_results(&body)
    }
}

/// Block size for the read-through cache over remote/lazy readers, auto-tuned
/// from the file size (same policy as the playground).
fn auto_block(len: u64) -> u64 {
    const MB: u64 = 1 << 20;
    let mult: u64 = if len > 100 * MB {
        8 // 512 KiB
    } else if len > 10 * MB {
        4 // 256 KiB
    } else {
        2 // 128 KiB
    };
    mult * DEFAULT_BLOCK
}

fn runtime_err<E: std::fmt::Display>(e: E) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Open a reader lazily: header + dictionary/index directories now, tile
/// payloads faulted in per query — the same path the playground's remote mode
/// uses, so a selective query over a multi-GB file fetches only KBs.
fn open_lazy(reader: AnyReader) -> PyResult<Graph> {
    let file_len = reader.len();
    let counting = Arc::new(CountingReader::new(reader));
    let cached = Arc::new(BlockCacheReader::new(
        counting.clone(),
        auto_block(file_len),
    ));
    #[allow(unused_mut)]
    let mut rete = Rete::open_ranged_lazy(cached).map_err(runtime_err)?;
    #[cfg(not(target_os = "emscripten"))]
    rete.set_service_client(Box::new(HttpServiceClient));
    Ok(Graph {
        rete,
        reader: Some(counting),
        file_len,
    })
}

/// A `.rete` opened once and kept resident, so repeated queries reuse the
/// decoded dictionary chunks, faulted index tiles, and the block cache.
#[pyclass(module = "rete_graph")]
struct Graph {
    rete: Rete,
    /// Counts PHYSICAL range fetches for lazy opens (`None` for in-memory).
    reader: Option<Arc<CountingReader<AnyReader>>>,
    file_len: u64,
}

impl Graph {
    /// Turn a partial lazy fetch into a hard error: results computed over
    /// silently-incomplete data must never reach the caller.
    fn incomplete_guard(&self) -> PyResult<()> {
        if self.rete.index_incomplete() {
            return Err(PyRuntimeError::new_err(
                "a range fetch failed mid-query; refusing to return incomplete results — retry",
            ));
        }
        Ok(())
    }

    /// The incompleteness verdict is PER CALL on this resident handle: reset
    /// the sticky failure flags before evaluating, so one transient network
    /// failure fails only the call it happened in — failed tiles/chunks are
    /// never cached, so the next call simply retries them. (Same shape as the
    /// browser engine's RemoteGraph entry points.)
    fn fresh_verdict(&self) {
        self.rete.reset_load_failures();
    }
}

#[pymethods]
impl Graph {
    /// Validate the graph against SHACL Core shapes written in Turtle.
    /// Lazy-aware: over the default graph only the shapes' targets are
    /// fetched (the index is consulted in place); a named `graph` view
    /// materializes that graph first. `format` is "json" (default) or "ttl".
    #[pyo3(signature = (shapes_turtle, *, graph=None, format="json"))]
    fn shacl(
        &self,
        py: Python<'_>,
        shapes_turtle: &str,
        graph: Option<&str>,
        format: &str,
    ) -> PyResult<String> {
        self.fresh_verdict();
        let out = py
            .allow_threads(|| -> Result<String, String> {
                let shapes = ShaclShapes::parse_turtle(shapes_turtle).map_err(|e| e.to_string())?;
                let report = match graph {
                    None => validate_shacl(&ReteGraph::new(&self.rete), &shapes),
                    Some(g) => validate_shacl(&DataGraph::from_rete(&self.rete, Some(g)), &shapes),
                };
                Ok(match format {
                    "ttl" => report.to_turtle(),
                    _ => report.to_json(),
                })
            })
            .map_err(PyRuntimeError::new_err)?;
        self.incomplete_guard()?;
        Ok(out)
    }

    /// Run a SPARQL query (SELECT / ASK / CONSTRUCT / DESCRIBE). Returns the
    /// JSON envelope `{"kind": "select", "vars": [...], "rows": [...]}` etc.;
    /// the Python wrapper parses it into values. `reason=True` turns on OWL 2
    /// QL entailment by query rewriting.
    #[pyo3(signature = (query, *, reason=false))]
    fn query(&self, py: Python<'_>, query: &str, reason: bool) -> PyResult<String> {
        self.fresh_verdict();
        let out = py
            .allow_threads(|| {
                if reason {
                    eval_query_reasoned(&self.rete, query)
                } else {
                    eval_query(&self.rete, query)
                }
            })
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        self.incomplete_guard()?;
        Ok(results_envelope_json(
            &out,
            &format!(r#","schemaVersion":{JSON_SCHEMA_VERSION}"#),
        ))
    }

    /// Header summary as JSON:
    /// `{"quads": N, "terms": N, "pyramidLevels": N, "namedGraphs": N}`.
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

    /// The named-graph IRIs of a dataset.
    fn graph_names(&self) -> Vec<String> {
        self.rete
            .graph_names()
            .into_iter()
            .map(str::to_owned)
            .collect()
    }

    /// Label prefix search over the pyramid's label index:
    /// `[(label, subject), ...]`.
    #[pyo3(signature = (prefix, limit=20))]
    fn prefix_search(&self, py: Python<'_>, prefix: &str, limit: usize) -> Vec<(String, String)> {
        py.allow_threads(|| self.rete.prefix_search(prefix, limit))
    }

    /// Full-text word / CONTAINS search over the file's TEXT_INDEX section
    /// (built with `rete build --text-index`); returns matching subjects.
    #[pyo3(signature = (words, contains_prefix=None, limit=100))]
    fn text_search(
        &self,
        py: Python<'_>,
        words: Vec<String>,
        contains_prefix: Option<String>,
        limit: usize,
    ) -> Vec<String> {
        py.allow_threads(|| {
            let refs: Vec<&str> = words.iter().map(String::as_str).collect();
            self.rete
                .text_search(&refs, contains_prefix.as_deref(), limit)
        })
    }

    /// The ontology profile as JSON:
    /// `{"classes": [[iri, count], ...], "relations": [[sClass, pred, oClass, count], ...]}`.
    fn schema(&self, py: Python<'_>) -> PyResult<String> {
        let (classes, relations) =
            py.allow_threads(|| (schema_classes(&self.rete), schema_summary(&self.rete)));
        serde_json::to_string(&serde_json::json!({
            "schemaVersion": JSON_SCHEMA_VERSION,
            "classes": classes,
            "relations": relations,
        }))
        .map_err(runtime_err)
    }

    /// `{"fileLength": N, "bytes": N, "requests": N}` — CUMULATIVE physical
    /// range fetches since open. In-memory graphs report zero fetches.
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

    /// The file's content hash (blake3-16, hex) — a stable cache key.
    fn content_hash(&self) -> String {
        self.rete
            .header()
            .content_hash
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    /// The embedded Dataset Card JSON, or `None` when the file carries none.
    /// Works on eager (bytes) and lazy (path/URL/reader) opens — the lazy path
    /// fetches only the metadata section's byte range.
    fn card(&self, py: Python<'_>) -> PyResult<Option<String>> {
        if let Some(reader) = &self.reader {
            let bytes = py
                .allow_threads(|| rete_core::read_metadata_ranged(reader.as_ref()))
                .map_err(runtime_err)?;
            return Ok(bytes.map(|b| String::from_utf8_lossy(&b).into_owned()));
        }
        Ok(self
            .rete
            .metadata()
            .map(|b| String::from_utf8_lossy(b).into_owned()))
    }

    #[getter]
    fn quads(&self) -> u64 {
        self.rete.header().quad_count
    }

    #[getter]
    fn terms(&self) -> u64 {
        self.rete.header().term_count
    }

    fn __repr__(&self) -> String {
        let h = self.rete.header();
        format!(
            "<rete Graph: {} quads, {} terms, {} bytes>",
            h.quad_count, h.term_count, self.file_len
        )
    }
}

/// Open a local `.rete` file lazily (positional reads; no whole-file load).
#[pyfunction]
fn open_path(py: Python<'_>, path: String) -> PyResult<Graph> {
    py.allow_threads(|| {
        let reader = LocalRangeReader::open(&path).map_err(runtime_err)?;
        open_lazy(AnyReader::Local(reader))
    })
}

/// Open a remote `.rete` over HTTP range requests, lazily. `headers` ride on
/// every request (auth tokens, custom User-Agent). Native-only: the Pyodide
/// build's `open()` routes URLs through the sync-XHR Python reader instead.
#[cfg(not(target_os = "emscripten"))]
#[pyfunction]
#[pyo3(signature = (url, headers=None))]
fn open_url(
    py: Python<'_>,
    url: String,
    headers: Option<std::collections::HashMap<String, String>>,
) -> PyResult<Graph> {
    let headers: Vec<(String, String)> =
        headers.map(|h| h.into_iter().collect()).unwrap_or_default();
    py.allow_threads(|| {
        let reader = HttpRangeReader::open(&url, headers).map_err(runtime_err)?;
        open_lazy(AnyReader::Http(reader))
    })
}

/// Open a `.rete` image held in memory (eager: sections decode now).
#[pyfunction]
fn open_bytes(py: Python<'_>, data: Vec<u8>) -> PyResult<Graph> {
    py.allow_threads(|| {
        #[allow(unused_mut)]
        let mut rete = Rete::open(&data).map_err(runtime_err)?;
        #[cfg(not(target_os = "emscripten"))]
        rete.set_service_client(Box::new(HttpServiceClient));
        Ok(Graph {
            rete,
            reader: None,
            file_len: data.len() as u64,
        })
    })
}

/// Open via a Python reader object: `read_at(offset, length) -> bytes` plus a
/// length (`len()` method or `__len__`). Backed by fsspec, this reaches any
/// authenticated store (S3, GCS, Azure) with zero code here.
#[pyfunction]
fn open_reader(py: Python<'_>, obj: PyObject) -> PyResult<Graph> {
    let reader = PyRangeReader::new(py, obj)?;
    py.allow_threads(|| open_lazy(AnyReader::Py(reader)))
}

/// Build a complete `.rete` file image from RDF text. `format` is `"nt"`,
/// `"nq"` (named graphs become a dataset), or `"ttl"`.
#[pyfunction]
#[pyo3(signature = (text, format="nt"))]
fn build(py: Python<'_>, text: String, format: &str) -> PyResult<Py<PyBytes>> {
    let bytes = py.allow_threads(|| -> PyResult<Vec<u8>> {
        let quads = rete_core::ingest::parse_statements(&text, format)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        if quads.is_empty() {
            return Err(PyValueError::new_err(
                "no statements parsed (empty input or only comments)",
            ));
        }
        let (bytes, _stats) = rete_core::ingest::assemble_dataset(quads, &[]);
        Ok(bytes)
    })?;
    Ok(PyBytes::new(py, &bytes).unbind())
}

/// Serialize the Dataset Card for the metadata section: the caller's curated
/// fields plus the count fields the CLI card schema REQUIRES (they have no
/// serde defaults over there), so `rete card` / `rete info` can always read a
/// Python-built card. Count semantics mirror the CLI: `quad_count` = all
/// statements, `triple_count` = default-graph triples.
fn card_bytes(
    curated: Option<serde_json::Value>,
    stats: &rete_core::ingest::BuildStats,
) -> Vec<u8> {
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

/// Full-option build behind `rete_graph.Builder`: multiple parsed sources,
/// an optional Dataset Card, pyramid on/off + algorithm, opt-in text index,
/// and a forced type predicate. Returns `(file_bytes, stats_json)`.
#[pyfunction]
#[pyo3(signature = (sources, card=None, pyramid=true, pyramid_algo="louvain", text_index=false, type_predicate=None))]
fn build_dataset(
    py: Python<'_>,
    sources: Vec<(String, String)>,
    card: Option<String>,
    pyramid: bool,
    pyramid_algo: &str,
    text_index: bool,
    type_predicate: Option<String>,
) -> PyResult<(Py<PyBytes>, String)> {
    let algo = rete_core::PyramidAlgo::from_cli(pyramid_algo).ok_or_else(|| {
        PyValueError::new_err(format!(
            "unknown pyramid_algo {pyramid_algo:?} (expected \"louvain\" or \"types\")"
        ))
    })?;
    // Validate the card up front so a bad payload is a clean ValueError, not a
    // silently empty metadata section.
    let curated: Option<serde_json::Value> = match card {
        None => None,
        Some(json) => {
            let value: serde_json::Value = serde_json::from_str(&json)
                .map_err(|e| PyValueError::new_err(format!("card is not valid JSON: {e}")))?;
            if !value.is_object() {
                return Err(PyValueError::new_err("card must be a JSON object"));
            }
            Some(value)
        }
    };
    let (bytes, stats) = py.allow_threads(|| -> PyResult<_> {
        let mut quads = Vec::new();
        for (text, format) in &sources {
            quads.extend(
                rete_core::ingest::parse_statements(text, format)
                    .map_err(|e| PyValueError::new_err(e.to_string()))?,
            );
        }
        if quads.is_empty() {
            return Err(PyValueError::new_err(
                "no statements parsed (empty input or only comments)",
            ));
        }
        Ok(rete_core::ingest::assemble_dataset_with_opts_algo(
            quads,
            pyramid,
            text_index,
            type_predicate.as_deref(),
            algo,
            move |stats, _quads| card_bytes(curated, stats),
        ))
    })?;
    let stats_json = format!(
        r#"{{"statements":{},"defaultTriples":{},"namedGraphs":{},"terms":{},"pyramidLevels":{}}}"#,
        stats.statements,
        stats.default_triples,
        stats.named_graphs,
        stats.terms,
        stats.pyramid_levels
    );
    Ok((PyBytes::new(py, &bytes).unbind(), stats_json))
}

#[pymodule]
fn _rete(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Graph>()?;
    m.add_function(wrap_pyfunction!(open_path, m)?)?;
    #[cfg(not(target_os = "emscripten"))]
    m.add_function(wrap_pyfunction!(open_url, m)?)?;
    m.add_function(wrap_pyfunction!(open_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(open_reader, m)?)?;
    m.add_function(wrap_pyfunction!(build, m)?)?;
    m.add_function(wrap_pyfunction!(build_dataset, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // The binding's version tracks the binding; this reports the engine actually
    // compiled into the wheel, which is what "does my install support X?" means.
    m.add("__engine_version__", rete_core::VERSION)?;
    Ok(())
}
