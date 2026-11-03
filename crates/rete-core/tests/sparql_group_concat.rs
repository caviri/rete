//! GROUP_CONCAT is CONCAT over its elements (SPARQL 1.1 §18.5.1.7), and CONCAT
//! takes string literals. Oxigraph agrees (see `crates/bench/tests/
//! differential.rs`): an IRI, a blank node, a number or any other typed literal
//! makes the aggregate an error, so it is unbound. Strings join into a plain
//! literal (a language tag is dropped: SPARQL 1.1 and the W3C tests want it
//! plain, Oxigraph keeps a tag all elements share); an empty group is `""`. An IRI used to come
//! out as `<http://e/i>`, brackets and all.

use rete_core::{eval_query, QueryOutput, Rete};

const DATA: &str = "\
<http://g/iri> <http://g/p> <http://e/i> .
<http://g/iri> <http://g/p> \"a\" .
<http://g/bn> <http://g/p> _:z .
<http://g/bn> <http://g/p> \"a\" .
<http://g/mix> <http://g/p> \"7\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://g/mix> <http://g/p> _:z .
<http://g/mix> <http://g/p> <http://e/i> .
<http://g/plain> <http://g/p> \"a\" .
<http://g/plain> <http://g/p> \"b\"^^<http://www.w3.org/2001/XMLSchema#string> .
<http://g/num> <http://g/p> \"7\"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://g/typed> <http://g/p> \"x\"^^<http://g/dt> .
<http://g/en> <http://g/p> \"a\"@en .
<http://g/en> <http://g/p> \"b\"@en .
<http://g/mixed> <http://g/p> \"a\"@en .
<http://g/mixed> <http://g/p> \"b\"@fr .
";

fn concat(rete: &Rete, subject: &str, agg: &str) -> Option<String> {
    let q = format!("SELECT ({agg} AS ?g) WHERE {{ <http://g/{subject}> <http://g/p> ?o }}");
    match eval_query(rete, &q).unwrap() {
        QueryOutput::Select(_, rows) => {
            assert_eq!(rows.len(), 1, "{q}");
            rows[0].get("g").cloned()
        }
        other => panic!("expected a SELECT result, got {other:?}"),
    }
}

/// A group's text, parts sorted (the order inside a group is not defined).
fn sorted(v: Option<String>) -> Option<String> {
    v.map(|t| {
        let (body, tag) = t.rsplit_once('"').unwrap();
        let mut parts: Vec<&str> = body.trim_start_matches('"').split('|').collect();
        parts.sort();
        format!("\"{}\"{tag}", parts.join("|"))
    })
}

#[test]
fn group_concat_element_kinds() {
    let quads = rete_core::ingest::parse_statements(DATA, "nt").unwrap();
    let (image, _) = rete_core::ingest::assemble_dataset(quads, &[]);
    let r = Rete::open(&image).unwrap();
    let agg = "GROUP_CONCAT(?o; SEPARATOR=\"|\")";

    // IRI, blank node, number, other typed literal: an error, so unbound.
    for s in ["iri", "bn", "mix", "num", "typed"] {
        assert_eq!(concat(&r, s, agg), None, "{s}");
    }
    // The IRI's text is still reachable through STR, without brackets.
    assert_eq!(
        sorted(concat(&r, "iri", "GROUP_CONCAT(STR(?o); SEPARATOR=\"|\")")),
        Some("\"a|http://e/i\"".into())
    );
    // Plain and xsd:string literals join into a plain literal.
    assert_eq!(sorted(concat(&r, "plain", agg)), Some("\"a|b\"".into()));
    // The default separator is a space.
    assert_eq!(
        sorted(concat(&r, "plain", "GROUP_CONCAT(?o)")).map(|s| s.replace(' ', "|")),
        Some("\"a|b\"".into())
    );
    // Language-tagged elements join too; the result is a plain literal.
    assert_eq!(sorted(concat(&r, "en", agg)), Some("\"a|b\"".into()));
    assert_eq!(sorted(concat(&r, "mixed", agg)), Some("\"a|b\"".into()));
    // An empty group is the empty string.
    assert_eq!(concat(&r, "nobody", agg), Some("\"\"".into()));
    // DISTINCT.
    assert_eq!(
        concat(&r, "num", "GROUP_CONCAT(DISTINCT ?o)"),
        None,
        "DISTINCT does not rescue a number"
    );
}
