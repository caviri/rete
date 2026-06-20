//! Serialize a query result into the playground JSON envelope, written
//! **directly into a `String`** — no intermediate `serde_json::Value` tree.
//!
//! On a large `SELECT` the tree path (`serde_json::Map` per row + a key clone and
//! a term clone per cell, then a second pass to stringify) allocates ~25× the
//! payload and costs more than the query itself. Writing the JSON straight into a
//! buffer cuts the serialization peak heap ~13× and the time ~10× (measured with
//! `rete-bench --query-mem`). This is the form the WASM `query()` returns across
//! the worker boundary, so the saved allocation matters most there.

use crate::sparql::QueryOutput;

/// Serialize `out` as `{ "kind": …, … }`. `extra` is a raw JSON fragment of
/// additional object members appended before the closing brace (e.g.
/// `,"remote":{…}`); pass `""` for none. `CONSTRUCT` is rendered as a `triples`
/// array — the text formats (Turtle / JSON-LD) are handled by the caller, which
/// owns those serializers.
///
/// Row object keys are emitted in variable order (the `vars` array), not sorted;
/// consumers read rows by variable name, so the order is presentational only.
pub fn results_envelope_json(out: &QueryOutput, extra: &str) -> String {
    let mut s = String::from("{");
    match out {
        QueryOutput::Ask(b) => {
            s.push_str(r#""kind":"ask","boolean":"#);
            s.push_str(if *b { "true" } else { "false" });
        }
        QueryOutput::Select(project, solutions) => {
            // Variable order: the projection, else the union of solution keys.
            let mut vars: Vec<&str> = project.iter().map(String::as_str).collect();
            if vars.is_empty() {
                let mut seen = std::collections::BTreeSet::new();
                for sol in solutions {
                    for k in sol.keys() {
                        if seen.insert(k.as_str()) {
                            vars.push(k.as_str());
                        }
                    }
                }
            }
            s.push_str(r#""kind":"select","vars":["#);
            for (i, v) in vars.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                push_json_string(&mut s, v);
            }
            s.push_str(r#"],"rows":["#);
            for (i, sol) in solutions.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push('{');
                let mut first = true;
                for var in &vars {
                    if let Some(term) = sol.get(*var) {
                        if !first {
                            s.push(',');
                        }
                        first = false;
                        push_json_string(&mut s, var);
                        s.push(':');
                        push_json_string(&mut s, term);
                    }
                }
                s.push('}');
            }
            s.push(']');
        }
        QueryOutput::Construct(triples) => {
            s.push_str(r#""kind":"construct","triples":["#);
            for (i, (a, b, c)) in triples.iter().enumerate() {
                if i > 0 {
                    s.push(',');
                }
                s.push('[');
                push_json_string(&mut s, a);
                s.push(',');
                push_json_string(&mut s, b);
                s.push(',');
                push_json_string(&mut s, c);
                s.push(']');
            }
            s.push(']');
        }
    }
    s.push_str(extra);
    s.push('}');
    s
}

/// Append `v` to `out` as a JSON string literal (RFC 8259 escaping): the
/// mandatory escapes (`"`, `\`, and C0 controls, the common ones in short form),
/// every other char — including all UTF-8 — passed through. Matches what
/// `serde_json` emits for a string by default.
pub fn push_json_string(out: &mut String, v: &str) {
    out.push('"');
    for c in v.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Binding;
    use serde_json::{json, Value};

    fn parse(s: &str) -> Value {
        serde_json::from_str(s).unwrap_or_else(|e| panic!("invalid JSON ({e}): {s}"))
    }

    fn row(pairs: &[(&str, &str)]) -> Binding {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn ask_envelope() {
        assert_eq!(
            parse(&results_envelope_json(&QueryOutput::Ask(true), "")),
            json!({"kind":"ask","boolean":true})
        );
        assert_eq!(
            parse(&results_envelope_json(&QueryOutput::Ask(false), "")),
            json!({"kind":"ask","boolean":false})
        );
    }

    #[test]
    fn select_envelope_matches_reference() {
        let out = QueryOutput::Select(
            vec!["s".into(), "o".into()],
            vec![
                row(&[("s", "<a>"), ("o", "<b>")]),
                // a row missing a projected var → that key is simply absent.
                row(&[("s", "<c>")]),
            ],
        );
        let got = parse(&results_envelope_json(&out, ""));
        let want = json!({
            "kind": "select",
            "vars": ["s", "o"],
            "rows": [ {"s": "<a>", "o": "<b>"}, {"s": "<c>"} ],
        });
        assert_eq!(got, want);
    }

    #[test]
    fn select_unprojected_uses_union_of_keys() {
        let out = QueryOutput::Select(vec![], vec![row(&[("y", "1"), ("x", "2")])]);
        let got = parse(&results_envelope_json(&out, ""));
        // vars is the union of solution keys (any order, as a set).
        let vars: std::collections::BTreeSet<String> = got["vars"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            vars,
            ["x".to_string(), "y".to_string()].into_iter().collect()
        );
        assert_eq!(got["rows"][0]["x"], json!("2"));
    }

    #[test]
    fn escaping_matches_serde_json() {
        // Terms with quotes, backslashes, controls, and unicode must escape exactly
        // as serde_json would (the reference).
        let tricky = "a\"b\\c\nd\te\r\u{08}\u{0c}\u{1}f—🜨";
        let out = QueryOutput::Select(vec!["v".into()], vec![row(&[("v", tricky)])]);
        let got = parse(&results_envelope_json(&out, ""));
        assert_eq!(got["rows"][0]["v"], json!(tricky));
        // And the raw bytes match serde_json's own string encoding.
        let mut buf = String::new();
        push_json_string(&mut buf, tricky);
        assert_eq!(buf, serde_json::to_string(tricky).unwrap());
    }

    #[test]
    fn construct_triples_and_extra_member() {
        let out = QueryOutput::Construct(vec![("<s>".into(), "<p>".into(), "\"lit\"".into())]);
        let got = parse(&results_envelope_json(&out, r#","remote":{"bytes":42}"#));
        assert_eq!(got["kind"], json!("construct"));
        assert_eq!(got["triples"], json!([["<s>", "<p>", "\"lit\""]]));
        assert_eq!(got["remote"]["bytes"], json!(42));
    }

    #[test]
    fn empty_select_is_valid() {
        let out = QueryOutput::Select(vec!["x".into()], vec![]);
        let got = parse(&results_envelope_json(&out, ""));
        assert_eq!(got, json!({"kind":"select","vars":["x"],"rows":[]}));
    }
}
