//! Browser bindings: load a `.rete` file's bytes (e.g. from `fetch`) and query
//! it entirely in the browser — the same engine the native CLI uses, compiled
//! to wasm. Results come back as JSON strings.

use rete_core::{
    batch_reach_serial, build_adjacency, build_dendrogram, choose_round_for_budget, eval_query,
    eval_sparql, project_graph, schema_classes, schema_summary, tile_by_community, Header,
    QueryOutput, Rete, SliceReader, SummaryView, DEFAULT_TILE_BUDGET,
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
    use serde_json::{json, Map, Value};
    let rete = open(bytes)?;
    let out = eval_query(&rete, query).map_err(err)?;
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
    serde_json::to_string(&v).map_err(err)
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
