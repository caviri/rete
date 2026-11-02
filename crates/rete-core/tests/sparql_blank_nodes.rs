//! A blank node has no lexical form, so it never turns into a string. SPARQL
//! 1.1 defines `STR` for literals and IRIs only (§17.4.2.5); the string
//! functions take string literals (§17.4.3), and `IRI`, `STRLANG`, `REGEX`,
//! `REPLACE` and `LANGMATCHES` take simple literals for the arguments they read
//! by text. Each of those, given a blank node, is an expression error: the
//! `BIND` leaves its variable unbound and a `FILTER` drops the row. What used to
//! happen is that the label (`_:b0`) was used as if it were text.

use rete_core::{eval_query, QueryOutput, Rete};

const DATA: &str = "\
_:b0 <http://ex/p> \"x\" .
<http://ex/a> <http://ex/p> _:b0 .
<http://ex/a> <http://ex/q> \"b0\" .
<http://ex/c> <http://ex/p> \"y\"@en .
";

fn open() -> Rete {
    let quads = rete_core::ingest::parse_statements(DATA, "nt").unwrap();
    let (image, _) = rete_core::ingest::assemble_dataset(quads, &[]);
    Rete::open(&image).unwrap()
}

fn select(rete: &Rete, q: &str) -> Vec<std::collections::BTreeMap<String, String>> {
    match eval_query(rete, q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("expected a SELECT result, got {other:?}"),
    }
}

/// Rows of `?x <p> ?o` where the blank node is the OBJECT (?o = _:b0), with the
/// expression bound as ?t.
fn bound_over_bnode(rete: &Rete, expr: &str) -> Option<String> {
    let q = format!(
        "SELECT ?t WHERE {{ <http://ex/a> <http://ex/p> ?b . FILTER(isBlank(?b)) BIND({expr} AS ?t) }}"
    );
    let rows = select(rete, &q);
    assert_eq!(
        rows.len(),
        1,
        "{expr}: the one blank-node row stays: {rows:?}"
    );
    rows[0].get("t").cloned()
}

#[test]
fn str_of_a_blank_node_is_unbound() {
    let r = open();
    // The row stays (a BIND error leaves the variable unbound); there is no value.
    assert_eq!(bound_over_bnode(&r, "STR(?b)"), None);
    // STR of an IRI and of a literal still answer.
    let rows = select(
        &r,
        "SELECT ?s ?t WHERE { ?s <http://ex/p> ?o . FILTER(!isBlank(?s)) BIND(STR(?s) AS ?t) }",
    );
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().any(|b| b["t"] == "\"http://ex/a\""));
    let lit = select(
        &r,
        "SELECT ?t WHERE { <http://ex/c> <http://ex/p> ?o BIND(STR(?o) AS ?t) }",
    );
    assert_eq!(lit[0]["t"], "\"y\"");
}

#[test]
fn a_filter_over_str_of_a_blank_node_drops_the_row() {
    let r = open();
    // `_:b0` is the label, not a value: no row may match it as text, and the
    // negation of an error is an error, so the row is dropped both ways.
    for f in [
        "STR(?s) = \"_:b0\"",
        "CONTAINS(STR(?s), \"b0\")",
        "REGEX(STR(?s), \"^_:\")",
        "STRSTARTS(STR(?s), \"_:\")",
        "!(STR(?s) = \"x\")",
    ] {
        let q = format!(
            "SELECT ?s WHERE {{ ?s <http://ex/p> \"x\" . FILTER(isBlank(?s)) FILTER({f}) }}"
        );
        assert!(select(&r, &q).is_empty(), "{f} matched a blank node");
    }
}

#[test]
fn the_string_functions_refuse_a_blank_node() {
    let r = open();
    // Already errors before this change; kept so they stay so.
    for e in [
        "STRLEN(?b)",
        "CONCAT(?b, \"x\")",
        "UCASE(?b)",
        "SUBSTR(?b, 1)",
        "ENCODE_FOR_URI(?b)",
        "MD5(?b)",
    ] {
        assert_eq!(bound_over_bnode(&r, e), None, "{e}");
    }
}

#[test]
fn arguments_read_by_text_refuse_a_blank_node() {
    let r = open();
    for e in [
        // IRI(?b) used to make `<_:b0>`.
        "IRI(?b)",
        "URI(?b)",
        // the language tag and the pattern were the label.
        "STRLANG(\"x\", ?b)",
        "REPLACE(\"abc\", ?b, \"x\")",
        "REPLACE(\"abc\", \"b\", ?b)",
        "REPLACE(\"abc\", \"b\", \"x\", ?b)",
        "IF(REGEX(\"a_:b0\", ?b), \"hit\", \"miss\")",
        "IF(REGEX(\"abc\", \"b\", ?b), \"hit\", \"miss\")",
        "IF(LANGMATCHES(?b, \"*\"), \"hit\", \"miss\")",
        "IF(LANGMATCHES(\"en\", ?b), \"hit\", \"miss\")",
    ] {
        assert_eq!(bound_over_bnode(&r, e), None, "{e}");
    }
}

#[test]
fn the_same_functions_still_work_on_what_they_accept() {
    let r = open();
    let one = |e: &str| {
        let rows = select(
            &r,
            &format!("SELECT ?t WHERE {{ <http://ex/c> <http://ex/p> ?o BIND({e} AS ?t) }}"),
        );
        rows[0].get("t").cloned()
    };
    assert_eq!(
        one("IRI(\"http://ex/z\")").as_deref(),
        Some("<http://ex/z>")
    );
    assert_eq!(one("STRLANG(\"x\", \"fr\")").as_deref(), Some("\"x\"@fr"));
    assert_eq!(
        one("REPLACE(\"abc\", \"b\", \"X\")").as_deref(),
        Some("\"aXc\"")
    );
    assert_eq!(
        one("REPLACE(\"aBc\", \"b\", \"X\", \"i\")").as_deref(),
        Some("\"aXc\"")
    );
    assert_eq!(
        one("IF(REGEX(\"abc\", \"B\", \"i\"), \"hit\", \"miss\")").as_deref(),
        Some("\"hit\"")
    );
    assert_eq!(
        one("IF(LANGMATCHES(LANG(?o), \"EN\"), \"hit\", \"miss\")").as_deref(),
        Some("\"hit\"")
    );
    assert_eq!(one("STR(?o)").as_deref(), Some("\"y\""));
    assert_eq!(one("STRLEN(?o)").map(|s| s.contains('1')), Some(true));
}

#[test]
fn a_blank_node_is_still_a_term() {
    let r = open();
    // isBlank, equality of the node with itself, DISTINCT and ORDER BY see the
    // node, not its label as text.
    let rows = select(
        &r,
        "SELECT DISTINCT ?b WHERE { ?x <http://ex/p> ?b . FILTER(isBlank(?b)) }",
    );
    assert_eq!(rows.len(), 1);
    let both = select(
        &r,
        "SELECT ?s WHERE { ?s <http://ex/p> \"x\" . <http://ex/a> <http://ex/p> ?b . FILTER(?s = ?b) }",
    );
    assert_eq!(both.len(), 1);
}
