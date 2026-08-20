//! The `export` command plus the RDF serialization helpers (Turtle / JSON-LD)
//! shared with the SPARQL CONSTRUCT output and `reason`.

use crate::commands::range_source::open_local;
use crate::commands::render::term_to_json;

/// Which slice of the dataset `rete export` should write.
///
/// Every field is a **pruning** filter, not a post-hoc row test: they become the
/// triple pattern `Rete::dump_filtered_each` routes on, so exporting one
/// predicate of a 33 GB graph fetches the tiles that predicate lives in and
/// nothing else. See that method for the measured before/after.
#[derive(Default, Clone)]
pub(crate) struct ExportFilter {
    /// `None` = the default graph followed by every named graph (the lossless
    /// N-Quads dump). `Some(None)` = the default graph only. `Some(Some(iri))`
    /// = that named graph only.
    pub graph: Option<Option<String>>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    pub object: Option<String>,
}

impl ExportFilter {
    /// The graph slots to write, in order.
    pub(crate) fn slots(&self, rete: &rete_core::Rete) -> Vec<Option<String>> {
        match &self.graph {
            None => std::iter::once(None)
                .chain(rete.graph_names().iter().map(|g| Some((*g).to_string())))
                .collect(),
            Some(None) => vec![None],
            Some(Some(g)) => vec![Some(canonical_graph(rete, g))],
        }
    }

    fn terms(&self) -> (Option<&str>, Option<&str>, Option<&str>) {
        (
            self.subject.as_deref(),
            self.predicate.as_deref(),
            self.object.as_deref(),
        )
    }
}

/// Resolve a user-supplied graph name to the token the file stores. Graph names
/// are canonical N-Triples terms (`<iri>`), but a shell user types the bare IRI;
/// accept either, preferring an exact match. Same rule as the wasm client's
/// `canonical_graph_name`, so `--graph` behaves identically in both.
pub(crate) fn canonical_graph(rete: &rete_core::Rete, name: &str) -> String {
    if rete.graph_names().contains(&name) {
        return name.to_string();
    }
    if name.starts_with('<') || name.starts_with("_:") {
        return name.to_string();
    }
    format!("<{name}>")
}

/// Canonicalize a user-supplied filter term to the N-Triples token the
/// dictionary stores: `<iri>`, `"literal"`, `"lit"@en`, `"lit"^^<dt>`, `_:b`
/// pass through, and a bare IRI gets its angle brackets. A term the dictionary
/// does not know matches nothing — which is a legitimate answer, not an error.
pub(crate) fn canonical_term(term: &str) -> String {
    if term.starts_with('<') || term.starts_with('"') || term.starts_with("_:") {
        term.to_string()
    } else {
        format!("<{term}>")
    }
}

/// `rete export <file> --format <fmt>`: write the graph — or a filtered slice of
/// it — as N-Quads, Turtle, or JSON-LD.
///
/// `sanitize_iris` percent-encodes IRIs that are outside the N-Triples/N-Quads
/// `IRIREF` grammar and RFC 3987 (see `rete_core::iri`), so the dump is
/// something a strict store will actually load. It is **opt-in**: escaping
/// changes the IRI, so a sanitized dump no longer joins against the file it came
/// from. What it changed goes to stderr — stdout is the dump.
pub(crate) fn export(
    file: &str,
    format: &str,
    filter: &ExportFilter,
    sanitize_iris: bool,
) -> anyhow::Result<()> {
    let rete = open_local(file)?;
    let (s, p, o) = filter.terms();
    // One report for the whole dump, so the summary is a single total across
    // every graph slot. `None` when the flag is off: the terms then take the
    // zero-cost `Cow::Borrowed` path and the export is byte-identical to before.
    let mut iris = sanitize_iris.then(rete_core::iri::IriReport::default);
    match format {
        // N-Quads: lossless dump of the selected graph(s).
        // Streamed (dump_filtered_each) so a 100M+ triple file serializes in
        // constant memory instead of materializing every term into a Vec.
        "nq" => {
            use std::io::Write;
            let stdout = std::io::stdout();
            let mut out = std::io::BufWriter::new(stdout.lock());
            for slot in filter.slots(&rete) {
                match &slot {
                    None => rete.dump_filtered_each(None, s, p, o, |s, p, o| {
                        let (s, p, o) = clean(&mut iris, s, p, o);
                        let _ = writeln!(out, "{s} {p} {o} .");
                    }),
                    Some(g) => {
                        // The graph term labels every line of this slot, so it
                        // is sanitized — and therefore counted — ONCE per graph,
                        // not once per quad. The lookup keeps the original
                        // token: it is the file's key, not the dump's text.
                        let label = match iris.as_mut() {
                            Some(r) => r.sanitize(g).into_owned(),
                            None => g.clone(),
                        };
                        rete.dump_filtered_each(Some(g), s, p, o, |s, p, o| {
                            let (s, p, o) = clean(&mut iris, s, p, o);
                            let _ = writeln!(out, "{s} {p} {o} {label} .");
                        })
                    }
                }
            }
            out.flush()?;
        }
        // Turtle / JSON-LD are single-graph formats here: the default graph
        // unless `--graph` names one (they have no default-vs-named distinction,
        // so an all-graphs export would silently merge them).
        "ttl" | "jsonld" => {
            let g = match &filter.graph {
                None | Some(None) => None,
                Some(Some(g)) => Some(canonical_graph(&rete, g)),
            };
            let mut triples = rete.query_in_graph(g.as_deref(), s, p, o);
            if let Some(report) = iris.as_mut() {
                for t in triples.iter_mut() {
                    let (s, p, o) = (
                        report.sanitize(&t.0).into_owned(),
                        report.sanitize(&t.1).into_owned(),
                        report.sanitize(&t.2).into_owned(),
                    );
                    *t = (s, p, o);
                }
            }
            if format == "ttl" {
                print!("{}", export_turtle(&triples));
            } else {
                println!("{}", export_jsonld(&triples));
            }
        }
        other => anyhow::bail!("unknown export format: {other}"),
    }
    if let Some(report) = iris.as_ref() {
        crate::commands::iri_report::report_sanitized(report);
    }
    Ok(())
}

/// Sanitize one quad's three terms when the flag is on, or hand them straight
/// back when it is off. Returned as owned `String`s only where a repair
/// happened; `Cow` keeps the untouched (overwhelming) majority allocation-free.
fn clean<'a>(
    report: &mut Option<rete_core::iri::IriReport>,
    s: &'a str,
    p: &'a str,
    o: &'a str,
) -> (
    std::borrow::Cow<'a, str>,
    std::borrow::Cow<'a, str>,
    std::borrow::Cow<'a, str>,
) {
    use std::borrow::Cow;
    match report.as_mut() {
        Some(r) => (r.sanitize(s), r.sanitize(p), r.sanitize(o)),
        None => (Cow::Borrowed(s), Cow::Borrowed(p), Cow::Borrowed(o)),
    }
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
