//! The `export` command plus the RDF serialization helpers (Turtle / JSON-LD)
//! shared with the SPARQL CONSTRUCT output and `reason`.

use rete_core::Rete;

use crate::term_to_json;

/// `rete export <file> <format>`: write the graph as N-Quads, Turtle, or JSON-LD.
pub(crate) fn export(file: &str, format: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    match format {
        // N-Quads: lossless dump of the default graph + every named graph.
        "nq" => {
            for (s, p, o) in rete.dump(None) {
                println!("{s} {p} {o} .");
            }
            for g in rete.graph_names() {
                for (s, p, o) in rete.dump(Some(g)) {
                    println!("{s} {p} {o} {g} .");
                }
            }
        }
        // Turtle / JSON-LD are single-graph formats here: emit the default graph.
        "ttl" => print!("{}", export_turtle(&rete.dump(None))),
        "jsonld" => println!("{}", export_jsonld(&rete.dump(None))),
        other => anyhow::bail!("unknown export format: {other}"),
    }
    Ok(())
}

/// Serialize a default-graph triple list (canonical N-Triples tokens) to Turtle.
///
/// The term tokens (`<iri>`, `"lit"`, `"lit"^^<dt>`, `"lit"@lang`, `_:b`) are
/// already valid Turtle term syntax, so they pass through verbatim; we only group
/// statements by subject and abbreviate `rdf:type` to `a` for idiomatic output.
pub(crate) fn export_turtle(triples: &[(String, String, String)]) -> String {
    use std::collections::BTreeMap;

    const RDF_TYPE_IRI: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    // subject → predicate → [objects], all in stable (sorted) order.
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

/// Serialize a default-graph triple list to expanded JSON-LD: an array of node
/// objects keyed by `@id`, each predicate mapping to an array of value objects
/// (`{"@id": …}` for IRIs/bnodes, `{"@value": …}` plus `@type`/`@language` for
/// literals). This is the canonical expanded form, valid against the JSON-LD 1.1
/// algorithm with no `@context`.
pub(crate) fn export_jsonld(triples: &[(String, String, String)]) -> String {
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;

    // subject id → predicate iri → [value objects], stable (sorted) order.
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

/// The JSON-LD `@id` string for a subject/IRI-or-bnode token: the bare IRI for
/// `<iri>`, the `_:b` token verbatim for a blank node.
fn node_id(token: &str) -> String {
    token
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .map(str::to_string)
        .unwrap_or_else(|| token.to_string())
}

/// Classify an object token into a JSON-LD value object (`@id` for IRIs/bnodes,
/// `@value` + optional `@type`/`@language` for literals). Reuses `term_to_json`'s
/// classification so escaping/datatype/lang handling stays consistent.
fn object_to_jsonld(token: &str) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    let t = term_to_json(token);
    match t["type"].as_str() {
        Some("uri") => json!({ "@id": t["value"] }),
        Some("bnode") => json!({ "@id": format!("_:{}", t["value"].as_str().unwrap_or("")) }),
        _ => {
            let mut obj = Map::new();
            obj.insert("@value".into(), t["value"].clone());
            if let Some(dt) = t.get("datatype") {
                obj.insert("@type".into(), dt.clone());
            }
            if let Some(lang) = t.get("xml:lang") {
                obj.insert("@language".into(), lang.clone());
            }
            Value::Object(obj)
        }
    }
}
