//! The `export` command plus the RDF serialization helpers (Turtle / JSON-LD)
//! shared with the SPARQL CONSTRUCT output and `reason`.

use rete_core::Rete;

use crate::commands::render::term_to_json;

/// `rete export <file> <format>`: write the graph as N-Quads, Turtle, or JSON-LD.
pub(crate) fn export(file: &str, format: &str) -> anyhow::Result<()> {
    let bytes = std::fs::read(file)?;
    let rete = Rete::open(&bytes)?;
    match format {
        // N-Quads: lossless dump of the default graph + every named graph.
        // Streamed (dump_each) so a 100M+ triple file serializes in constant
        // memory instead of materializing every term into a Vec (which OOMs).
        "nq" => {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut out = std::io::BufWriter::new(stdout.lock());
            rete.dump_each(None, |s, p, o| {
                let _ = writeln!(out, "{s} {p} {o} .");
            });
            let names: Vec<String> = rete.graph_names().iter().map(|s| s.to_string()).collect();
            for g in names {
                rete.dump_each(Some(&g), |s, p, o| {
                    let _ = writeln!(out, "{s} {p} {o} {g} .");
                });
            }
            out.flush()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_triples() -> Vec<(String, String, String)> {
        vec![
            (
                "<http://ex/Alice>".into(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
                "<http://ex/Person>".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/age>".into(),
                "\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/label>".into(),
                "\"héllo \\\"quote\\\"\"@en".into(),
            ),
            (
                "<http://ex/Alice>".into(),
                "<http://ex/knows>".into(),
                "_:b0".into(),
            ),
        ]
    }

    #[test]
    fn turtle_export_groups_and_abbreviates() {
        let ttl = export_turtle(&sample_triples());
        // One subject block, predicates sorted, `rdf:type` shown as `a`.
        assert!(ttl.starts_with("<http://ex/Alice>\n"));
        assert!(ttl.contains("    a <http://ex/Person>"), "got:\n{ttl}");
        // Datatype literal passes through verbatim (valid Turtle term syntax).
        assert!(
            ttl.contains("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            "got:\n{ttl}"
        );
        // Lang tag + escaped quote preserved exactly.
        assert!(ttl.contains("\"héllo \\\"quote\\\"\"@en"), "got:\n{ttl}");
        // Blank node passes through; statement list ends with ` .`.
        assert!(ttl.contains("_:b0"));
        assert!(ttl.trim_end().ends_with(" ."));
    }

    #[test]
    fn jsonld_export_expanded_shape() {
        let v: serde_json::Value = serde_json::from_str(&export_jsonld(&sample_triples())).unwrap();
        let node = &v[0];
        assert_eq!(node["@id"], "http://ex/Alice");
        // IRI object → {"@id": …}; rdf:type is a normal predicate IRI (not @type).
        assert_eq!(
            node["http://www.w3.org/1999/02/22-rdf-syntax-ns#type"][0]["@id"],
            "http://ex/Person"
        );
        // Typed literal → @value + @type, with the unescaped lexical form.
        let age = &node["http://ex/age"][0];
        assert_eq!(age["@value"], "30");
        assert_eq!(age["@type"], "http://www.w3.org/2001/XMLSchema#integer");
        // Lang-tagged literal → @value + @language; escapes resolved to chars.
        let label = &node["http://ex/label"][0];
        assert_eq!(label["@value"], "héllo \"quote\"");
        assert_eq!(label["@language"], "en");
        // Blank node object → {"@id": "_:b0"}.
        assert_eq!(node["http://ex/knows"][0]["@id"], "_:b0");
    }
}
