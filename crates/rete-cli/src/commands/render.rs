//! Result rendering shared across the query commands: print a `QueryOutput` in
//! readable or SPARQL-Results-JSON form (`print_query_output`), and the
//! N-Triples-token → JSON term classification (`term_to_json`) and literal
//! lexical extraction (`literal_lexical`) reused by `export` and `communities`.

use rete_core::QueryOutput;

/// Print a query result: SPARQL Results JSON when `json`, else a readable form.
pub(crate) fn print_query_output(result: &QueryOutput, json: bool) {
    if json {
        println!("{}", results_json(result));
        return;
    }
    match result {
        QueryOutput::Ask(b) => println!("{b}"),
        QueryOutput::Construct(triples) => {
            for (s, p, o) in triples {
                println!("{s} {p} {o} .");
            }
            eprintln!("{} triple(s)", triples.len());
        }
        QueryOutput::Select(project, solutions) => {
            for sol in solutions {
                let keys: Vec<&String> = if project.is_empty() {
                    sol.keys().collect()
                } else {
                    project.iter().collect()
                };
                let row: Vec<String> = keys
                    .iter()
                    .map(|k| format!("?{k}={}", sol.get(*k).map(String::as_str).unwrap_or("")))
                    .collect();
                println!("{}", row.join("  "));
            }
            eprintln!("{} solution(s)", solutions.len());
        }
    }
}

/// Render a query result as SPARQL Results JSON (W3C
/// `application/sparql-results+json`), pretty-printed.
fn results_json(result: &QueryOutput) -> String {
    serde_json::to_string_pretty(&query_output_json(result)).unwrap_or_default()
}

pub(crate) fn query_output_json(result: &QueryOutput) -> serde_json::Value {
    use serde_json::{json, Map, Value};
    match result {
        QueryOutput::Ask(b) => json!({ "head": {}, "boolean": b }),
        QueryOutput::Select(project, solutions) => {
            // Variable order: the projection, else the union of solution keys.
            let mut vars: Vec<String> = project.clone();
            if vars.is_empty() {
                let mut seen = std::collections::BTreeSet::new();
                for s in solutions {
                    for k in s.keys() {
                        if seen.insert(k.clone()) {
                            vars.push(k.clone());
                        }
                    }
                }
            }
            let bindings: Vec<Value> = solutions
                .iter()
                .map(|s| {
                    let mut obj = Map::new();
                    for v in &vars {
                        if let Some(term) = s.get(v) {
                            obj.insert(v.clone(), term_to_json(term));
                        }
                    }
                    Value::Object(obj)
                })
                .collect();
            json!({ "head": { "vars": vars }, "results": { "bindings": bindings } })
        }
        // CONSTRUCT isn't a results-set; emit the triples as JSON for convenience.
        QueryOutput::Construct(triples) => {
            let arr: Vec<Value> = triples.iter().map(|(s, p, o)| json!([s, p, o])).collect();
            json!({ "triples": arr })
        }
    }
}

/// Classify an N-Triples term token into a SPARQL-JSON RDF term object.
pub(crate) fn term_to_json(token: &str) -> serde_json::Value {
    use serde_json::json;
    if let Some(iri) = token.strip_prefix('<').and_then(|s| s.strip_suffix('>')) {
        return json!({ "type": "uri", "value": iri });
    }
    if let Some(b) = token.strip_prefix("_:") {
        return json!({ "type": "bnode", "value": b });
    }
    if token.starts_with('"') {
        // Closing quote (honoring \" escapes), then optional ^^<dt> / @lang.
        let bytes = token.as_bytes();
        let mut i = 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => break,
                _ => i += 1,
            }
        }
        // The JSON `value` is the *unescaped* lexical form; the token carries
        // N-Triples escapes (\", \n, \\, \uXXXX …) that must be resolved first,
        // or serde would re-escape them and emit a doubly-escaped string.
        let value = unescape_nt(&token[1..i.min(token.len())]);
        let rest = token.get(i + 1..).unwrap_or("");
        if let Some(dt) = rest.strip_prefix("^^<").and_then(|s| s.strip_suffix('>')) {
            return json!({ "type": "literal", "value": value, "datatype": dt });
        }
        if let Some(lang) = rest.strip_prefix('@') {
            return json!({ "type": "literal", "value": value, "xml:lang": lang });
        }
        return json!({ "type": "literal", "value": value });
    }
    json!({ "type": "literal", "value": token })
}

/// If `token` is a literal (`"…"`, `"…"^^<dt>`, or `"…"@lang`), return its
/// **lexical value** with N-Triples escapes resolved (the bare string, no quotes,
/// datatype, or language tag). Returns `None` for IRIs and blank nodes. This is
/// the text a topic model consumes — the same quote/`^^`/`@` stripping that
/// `term_to_json` performs for the literal case.
pub(crate) use rete_core::terms::literal_lexical;

/// Resolve the N-Triples escape sequences in a literal's body to actual chars.
fn unescape_nt(s: &str) -> String {
    rete_core::terms::unescape_literal(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rete_core::Binding;
    use serde_json::json;

    #[test]
    fn term_json_classification() {
        assert_eq!(
            term_to_json("<http://ex/a>"),
            json!({"type":"uri","value":"http://ex/a"})
        );
        assert_eq!(term_to_json("_:b0"), json!({"type":"bnode","value":"b0"}));
        assert_eq!(
            term_to_json("\"plain\""),
            json!({"type":"literal","value":"plain"})
        );
        assert_eq!(
            term_to_json("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
            json!({"type":"literal","value":"30",
                   "datatype":"http://www.w3.org/2001/XMLSchema#integer"})
        );
        assert_eq!(
            term_to_json("\"hi\"@en"),
            json!({"type":"literal","value":"hi","xml:lang":"en"})
        );
        // Escapes in the literal body are resolved to their actual characters,
        // so serde emits a singly-escaped JSON string (not a doubly-escaped one).
        assert_eq!(
            term_to_json(r#""he said \"hi\"\nbye""#),
            json!({"type":"literal","value":"he said \"hi\"\nbye"})
        );
        assert_eq!(
            term_to_json(r#""tab\there\\end""#),
            json!({"type":"literal","value":"tab\there\\end"})
        );
        // \u escape → actual code point. The token body is the 6 ASCII chars
        // backslash-u-0-0-E-9; the decoded value is the single char U+00E9 (é).
        assert_eq!(
            term_to_json("\"caf\\u00E9\""),
            json!({"type":"literal","value":"caf\u{E9}"})
        );
        // An escaped quote must not be mistaken for the closing quote that
        // precedes a datatype tag.
        assert_eq!(
            term_to_json(r#""a\"b"^^<http://ex/dt>"#),
            json!({"type":"literal","value":"a\"b","datatype":"http://ex/dt"})
        );
    }

    #[test]
    fn literal_lexical_extraction() {
        // Plain literal → bare lexical value.
        assert_eq!(
            literal_lexical("\"hello world\"").as_deref(),
            Some("hello world")
        );
        // Datatype and language tags are stripped, value only.
        assert_eq!(
            literal_lexical("\"30\"^^<http://www.w3.org/2001/XMLSchema#integer>").as_deref(),
            Some("30")
        );
        assert_eq!(
            literal_lexical("\"bonjour\"@fr").as_deref(),
            Some("bonjour")
        );
        // Escapes are resolved; an escaped quote is not the closing quote.
        assert_eq!(
            literal_lexical(r#""he said \"hi\"\nbye""#).as_deref(),
            Some("he said \"hi\"\nbye")
        );
        // IRIs and blank nodes are not literals.
        assert_eq!(literal_lexical("<http://ex/a>"), None);
        assert_eq!(literal_lexical("_:b0"), None);
    }

    #[test]
    fn select_results_json_shape() {
        let mut b = Binding::new();
        b.insert("p".into(), "<http://ex/Alice>".into());
        let out = QueryOutput::Select(vec!["p".into()], vec![b]);
        let v: serde_json::Value = serde_json::from_str(&results_json(&out)).unwrap();
        assert_eq!(v["head"]["vars"][0], "p");
        assert_eq!(v["results"]["bindings"][0]["p"]["value"], "http://ex/Alice");
    }

    #[test]
    fn ask_results_json() {
        let v: serde_json::Value =
            serde_json::from_str(&results_json(&QueryOutput::Ask(true))).unwrap();
        assert_eq!(v["boolean"], true);
    }
}
