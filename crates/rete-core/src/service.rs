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
}
