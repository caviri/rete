//! SPARQL 1.1 federated query (`SERVICE`) — the client seam and result parsing.
//!
//! The engine itself performs no I/O (the same rule as [`RangeReader`]): a
//! `SERVICE <endpoint> { … }` block is lowered to a plan node that, at
//! evaluation time, hands the serialized sub-query to a host-injected
//! [`ServiceClient`] and joins the returned solutions back into the pipeline.
//! The CLI backs the seam with an HTTP client, the wasm bindings with XHR; a
//! `Rete` with no client attached errors on (non-SILENT) `SERVICE`.
//!
//! Solutions cross the seam as [`Binding`]s of variable → **N-Triples term
//! token** (`<iri>`, `"lit"@lang`, `"lit"^^<dt>`, `_:b0`) — the same canonical
//! tokens the engine uses everywhere — so [`parse_sparql_json_results`] is the
//! one place the SPARQL protocol's JSON shape is understood.
//!
//! [`RangeReader`]: crate::reader::RangeReader

use crate::bgp::Binding;

/// Executes one SPARQL query against a remote endpoint (the SPARQL Protocol)
/// and returns its solutions. Implementations own transport, auth, and
/// timeouts; they typically `POST` the query with
/// `Accept: application/sparql-results+json` and feed the body through
/// [`parse_sparql_json_results`]. Errors are strings, surfaced verbatim as the
/// query error (or, under `SERVICE SILENT`, swallowed per the spec) — name the
/// endpoint in the message, the engine adds no prefix.
pub trait ServiceClient: Send + Sync {
    fn query(&self, endpoint: &str, query: &str) -> Result<Vec<Binding>, String>;
}

/// Parse a SPARQL 1.1 Query Results JSON document (`application/sparql-results+json`)
/// into bindings of variable → N-Triples term token. An ASK document (no
/// `results`) yields no bindings. Unknown per-binding fields are ignored;
/// `xsd:string` datatypes are dropped (a simple literal — matching how plain
/// literals are tokenized everywhere else in the engine).
pub fn parse_sparql_json_results(body: &str) -> Result<Vec<Binding>, String> {
    let doc: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("results are not JSON: {e}"))?;
    let Some(results) = doc.get("results") else {
        return Ok(Vec::new()); // an ASK response — no solutions to join
    };
    let bindings = results
        .get("bindings")
        .and_then(|b| b.as_array())
        .ok_or("malformed results: no bindings array")?;
    let mut out = Vec::with_capacity(bindings.len());
    for sol in bindings {
        let obj = sol.as_object().ok_or("malformed solution")?;
        let mut b = Binding::new();
        for (var, term) in obj {
            if let Some(token) = json_term_token(term) {
                b.insert(var.clone(), token);
            }
        }
        out.push(b);
    }
    Ok(out)
}

/// One JSON result term → its N-Triples token, `None` for a shape this reader
/// doesn't recognize (the binding is then treated as unbound — never a panic).
fn json_term_token(term: &serde_json::Value) -> Option<String> {
    let ty = term.get("type")?.as_str()?;
    let value = term.get("value")?.as_str()?;
    match ty {
        "uri" => Some(format!("<{value}>")),
        "bnode" => Some(format!("_:{value}")),
        // Virtuoso emits the legacy "typed-literal"; treat it as "literal".
        "literal" | "typed-literal" => {
            let mut tok = String::with_capacity(value.len() + 8);
            tok.push('"');
            escape_literal_into(value, &mut tok);
            tok.push('"');
            if let Some(lang) = term.get("xml:lang").and_then(|l| l.as_str()) {
                tok.push('@');
                tok.push_str(lang);
            } else if let Some(dt) = term.get("datatype").and_then(|d| d.as_str()) {
                // xsd:string is the simple-literal datatype — keep the plain
                // token so it joins with locally-ingested plain literals.
                if dt != "http://www.w3.org/2001/XMLSchema#string" {
                    tok.push_str("^^<");
                    tok.push_str(dt);
                    tok.push('>');
                }
            }
            Some(tok)
        }
        _ => None,
    }
}

/// Escape a literal's lexical form for an N-Triples token (the JSON carries it
/// unescaped).
fn escape_literal_into(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
}

// --- the serializer (the inverse of the parser above) --------------------------
//
// `rete serve` speaks the SPARQL Protocol, so it must EMIT
// `application/sparql-results+json` too. Serializing here, next to the parser,
// keeps the whole JSON↔token mapping in one file (and round-trip-testable).

/// Serialize a SELECT result as a SPARQL 1.1 Query Results JSON document.
/// `vars` fixes the `head` order (pass the projection); solutions' bindings are
/// term tokens exactly as the engine returns them.
pub fn sparql_json_results(vars: &[String], solutions: &[Binding]) -> String {
    use serde_json::{json, Map, Value};
    let bindings: Vec<Value> = solutions
        .iter()
        .map(|sol| {
            let mut obj = Map::new();
            for (var, token) in sol {
                if let Some(term) = token_json_term(token) {
                    obj.insert(var.clone(), term);
                }
            }
            Value::Object(obj)
        })
        .collect();
    json!({
        "head": { "vars": vars },
        "results": { "bindings": bindings },
    })
    .to_string()
}

/// Serialize an ASK result as a SPARQL 1.1 Query Results JSON document.
pub fn sparql_json_ask(boolean: bool) -> String {
    serde_json::json!({ "head": {}, "boolean": boolean }).to_string()
}

/// One N-Triples term token → its JSON result term. `None` for a malformed
/// token (the binding is then omitted, never a panic).
fn token_json_term(token: &str) -> Option<serde_json::Value> {
    use crate::terms;
    use serde_json::json;
    if let Some(iri) = terms::iri_content(token) {
        return Some(json!({ "type": "uri", "value": iri }));
    }
    if terms::is_blank(token) {
        return Some(json!({ "type": "bnode", "value": token.strip_prefix("_:")? }));
    }
    if terms::is_literal(token) {
        let value = terms::literal_lexical(token)?;
        let lang = terms::lang_tag(token)?;
        if !lang.is_empty() {
            return Some(json!({ "type": "literal", "value": value, "xml:lang": lang }));
        }
        let dt = terms::literal_datatype(token)?;
        if dt == "http://www.w3.org/2001/XMLSchema#string" {
            return Some(json!({ "type": "literal", "value": value }));
        }
        return Some(json!({ "type": "literal", "value": value, "datatype": dt }));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_every_term_shape() {
        let body = r#"{
            "head": {"vars": ["s", "l", "n", "b", "u"]},
            "results": {"bindings": [
                {
                    "s": {"type": "uri", "value": "http://ex/a"},
                    "l": {"type": "literal", "value": "hi", "xml:lang": "en"},
                    "n": {"type": "literal", "value": "42",
                          "datatype": "http://www.w3.org/2001/XMLSchema#integer"},
                    "b": {"type": "bnode", "value": "b0"}
                },
                {
                    "s": {"type": "uri", "value": "http://ex/b"},
                    "l": {"type": "typed-literal", "value": "plain",
                          "datatype": "http://www.w3.org/2001/XMLSchema#string"}
                }
            ]}
        }"#;
        let sols = parse_sparql_json_results(body).unwrap();
        assert_eq!(sols.len(), 2);
        assert_eq!(sols[0]["s"], "<http://ex/a>");
        assert_eq!(sols[0]["l"], "\"hi\"@en");
        assert_eq!(
            sols[0]["n"],
            "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>"
        );
        assert_eq!(sols[0]["b"], "_:b0");
        // `u` is unbound in both rows; xsd:string normalizes to a plain literal.
        assert!(!sols[0].contains_key("u"));
        assert_eq!(sols[1]["l"], "\"plain\"");
    }

    #[test]
    fn escapes_literal_lexical_forms() {
        let body = r#"{"results": {"bindings": [
            {"l": {"type": "literal", "value": "a \"q\"\nb\\c"}}
        ]}}"#;
        let sols = parse_sparql_json_results(body).unwrap();
        assert_eq!(sols[0]["l"], "\"a \\\"q\\\"\\nb\\\\c\"");
    }

    #[test]
    fn ask_and_garbage_documents() {
        // An ASK response has no results member — zero solutions, not an error.
        assert!(parse_sparql_json_results(r#"{"boolean": true}"#)
            .unwrap()
            .is_empty());
        assert!(parse_sparql_json_results("not json").is_err());
        assert!(parse_sparql_json_results(r#"{"results": {}}"#).is_err());
    }

    /// The serializer is the parser's inverse: every term shape (IRI, plain /
    /// lang / typed / escaped literal, bnode, unbound) survives
    /// serialize→parse unchanged — the property `rete serve` relies on when a
    /// rete SERVICE client queries a rete endpoint.
    #[test]
    fn serialize_parse_round_trips_every_term_shape() {
        let vars: Vec<String> = ["s", "l", "n", "b", "e", "u"]
            .iter()
            .map(|v| v.to_string())
            .collect();
        let sols = vec![
            binding_of(&[
                ("s", "<http://ex/a>"),
                ("l", "\"hi\"@en"),
                ("n", "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
                ("b", "_:b0"),
                ("e", "\"a \\\"q\\\"\\nb\\\\c\""),
                // `u` unbound
            ]),
            binding_of(&[("s", "<http://ex/b>"), ("l", "\"plain\"")]),
        ];
        let doc = sparql_json_results(&vars, &sols);
        let back = parse_sparql_json_results(&doc).unwrap();
        assert_eq!(back, sols, "serialize→parse must be the identity");
        // ASK round-trips to zero solutions (the parser's ASK contract).
        assert!(parse_sparql_json_results(&sparql_json_ask(true))
            .unwrap()
            .is_empty());
    }

    fn binding_of(pairs: &[(&str, &str)]) -> Binding {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
}
