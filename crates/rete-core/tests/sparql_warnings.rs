//! FILTER type-error diagnostics (`eval_query_with_warnings`).
//!
//! The query shapes an LLM agent writes when it decides CONTAINS / STRSTARTS /
//! REGEX are "unsupported": each is a SPARQL type error that FILTER turns into
//! `false`. The row counts must stay exactly what they were (spec-correct, the
//! same as Oxigraph and Jena); what is new is the warning that says why.

use rete_core::{
    eval_query, eval_query_with_warnings, warnings_json, write_file, DictionaryBuilder,
    GraphIndexBuilder, QueryOpts, QueryOutput, QueryWarning, Rete, SparqlError, WarningSeverity,
};

const P: &str = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ";
const MAP: &str = "<http://ex.org/map/geneva-1572>";

fn graph() -> Vec<u8> {
    let triples = [
        (
            MAP,
            "<http://www.w3.org/2000/01/rdf-schema#label>",
            "\"Geneva town plan\"@en",
        ),
        (
            MAP,
            "<http://ex.org/year>",
            "\"1572\"^^<http://www.w3.org/2001/XMLSchema#integer>",
        ),
        (MAP, "<http://ex.org/note>", "\"a (plain) note\""),
        (MAP, "<http://ex.org/made>", "\"1572-03-01\""),
    ];
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for (s, p, o) in triples {
        ib.push(dict.encode(s, p, o).unwrap());
    }
    write_file(&dict, &ib.build(), false, &[], 0)
}

fn run(rete: &Rete, q: &str) -> (usize, Vec<QueryWarning>) {
    let q = format!("{P}{q}");
    let (out, w) = eval_query_with_warnings(rete, &q, QueryOpts::default()).unwrap();
    // The plain entry point must agree row for row: diagnostics never change results.
    let plain = eval_query(rete, &q).unwrap();
    let n = match (&out, &plain) {
        (QueryOutput::Select(_, a), QueryOutput::Select(_, b)) => {
            assert_eq!(a, b);
            a.len()
        }
        (QueryOutput::Ask(a), QueryOutput::Ask(b)) => {
            assert_eq!(a, b);
            *a as usize
        }
        _ => panic!("unexpected output"),
    };
    (n, w)
}

fn one(w: &[QueryWarning]) -> &QueryWarning {
    assert_eq!(w.len(), 1, "expected exactly one warning, got {w:#?}");
    &w[0]
}

#[test]
fn the_reported_table_keeps_its_row_counts_and_gains_warnings() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();

    // Correct shapes: rows, no warnings.
    for q in [
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva")) }"#,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(STR(?s), "geneva")) }"#,
        r#"SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(STR(?y), "15")) }"#,
    ] {
        let (n, w) = run(&rete, q);
        assert_eq!((n, w.len()), (1, 0), "{q}: {w:#?}");
    }

    // CONTAINS on an IRI.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(w.severity, WarningSeverity::TypeError);
    assert_eq!(
        (w.function.as_str(), w.argument, w.kind.as_str(), w.count),
        ("CONTAINS", 1, "iri", 1)
    );
    assert_eq!(w.sample.as_deref(), Some(MAP));
    assert!(w.hint.contains("STR()"), "{}", w.hint);
    assert!(
        w.message.contains("CONTAINS received an IRI as argument 1"),
        "{}",
        w.message
    );

    // STRSTARTS on an xsd:integer.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s <http://ex.org/year> ?y FILTER(STRSTARTS(?y, "15")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(
        (w.function.as_str(), w.argument, w.kind.as_str()),
        ("STRSTARTS", 1, "numeric")
    );
    assert!(w.message.contains("xsd:integer"), "{}", w.message);
    assert!(w.hint.contains("STR()"));

    // REGEX on an IRI.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?s, "geneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(
        (w.function.as_str(), w.argument, w.kind.as_str()),
        ("REGEX", 1, "iri")
    );

    // Language-incompatible arguments (§17.4.3).
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva"@fr)) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(
        (w.function.as_str(), w.argument, w.kind.as_str()),
        ("CONTAINS", 2, "language-mismatch")
    );
    assert!(
        w.message.contains("@fr") && w.message.contains("@en"),
        "{}",
        w.message
    );
    assert!(w.hint.contains("STR() on both"));

    // Case mismatch: no type error — only the (clearly marked) hint.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "geneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(w.severity, WarningSeverity::Hint);
    assert_eq!(
        (w.function.as_str(), w.kind.as_str()),
        ("CONTAINS", "case-sensitive")
    );
    assert!(w.hint.contains("LCASE"), "{}", w.hint);
}

#[test]
fn case_hint_is_withheld_when_it_would_be_false_confidence() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    for q in [
        // Rows came back: no hint.
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "Geneva") || CONTAINS(?l, "zzz")) }"#,
        // Empty because the pattern matched nothing — CONTAINS never ran.
        r#"SELECT ?s WHERE { ?s <http://ex.org/none> ?l FILTER(CONTAINS(?l, "geneva")) }"#,
        // A needle with no letters cannot be a case problem.
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?l, "1999")) }"#,
        // REGEX already case-insensitive.
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?l, "lausanne", "i")) }"#,
        // Needle is not a constant.
        r#"SELECT ?s WHERE { ?s rdfs:label ?l . ?s <http://ex.org/note> ?n FILTER(CONTAINS(?l, ?n)) }"#,
    ] {
        let (_, w) = run(&rete, q);
        assert!(w.is_empty(), "{q}: {w:#?}");
    }
    // A type error is the better explanation: it wins over the case hint.
    let (_, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(?o, "geneva")) }"#,
    );
    assert!(
        w.iter().all(|w| w.severity == WarningSeverity::TypeError),
        "{w:#?}"
    );
    // REGEX without the i flag does get it.
    let (_, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?l, "^geneva")) }"#,
    );
    assert_eq!(one(&w).kind, "case-sensitive");
    assert!(one(&w).hint.contains("\"i\""));
}

#[test]
fn other_error_kinds_unbound_regex_and_nested_value_functions() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();

    // Misspelled / never-bound variable.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?lable, "Geneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!((w.kind.as_str(), w.argument), ("unbound", 1));
    assert_eq!(w.sample.as_deref(), Some("?lable"));

    // Invalid regex (look-around is not in Rust regex syntax).
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(REGEX(?l, "(?<=G)eneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!(
        (w.function.as_str(), w.argument, w.kind.as_str()),
        ("REGEX", 2, "invalid-regex")
    );
    assert!(w.hint.contains("invalid regex pattern"), "{}", w.hint);

    // Nested: LCASE gets the IRI; the error is reported once, at LCASE.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(LCASE(?s), "geneva")) }"#,
    );
    assert_eq!(n, 0);
    let w = one(&w);
    assert_eq!((w.function.as_str(), w.kind.as_str()), ("LCASE", "iri"));

    // BIND: the error leaves the variable unbound (row kept) — still reported.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s ?u WHERE { ?s <http://ex.org/year> ?y BIND(UCASE(?y) AS ?u) }"#,
    );
    assert_eq!(n, 1);
    assert_eq!(one(&w).kind, "numeric");

    // YEAR on something that is not an xsd:dateTime.
    let (_, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s <http://ex.org/made> ?d FILTER(YEAR(?d) = 1572) }"#,
    );
    assert_eq!(one(&w).kind, "not-a-datetime");

    // ASK goes through the same path.
    let (n, w) = run(
        &rete,
        r#"ASK { ?s rdfs:label ?l FILTER(STRENDS(?s, "1572")) }"#,
    );
    assert_eq!(n, 0);
    assert_eq!(one(&w).function, "STRENDS");

    // Counting is per evaluation, bounded to one entry per key.
    let (_, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(?s, "x")) }"#,
    );
    assert_eq!(one(&w).count, 4);
}

/// An error that propagates through `!`, `||`, `&&`, IF or a BIND is still
/// reported exactly once per evaluation — at the function that raised it —
/// and the row counts follow SPARQL 1.1 §17.2: `!error` is an error, so
/// `FILTER(!CONTAINS(?iri, …))` drops the row instead of keeping it.
#[test]
fn propagated_errors_are_reported_once_and_drop_the_row() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();

    for (q, rows) in [
        // `!error` is an error: the row is dropped (it used to be kept).
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(!CONTAINS(?s, "geneva")) }"#,
            0,
        ),
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(!(!CONTAINS(?s, "geneva"))) }"#,
            0,
        ),
        // error || false is an error; !(error || false) too.
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(!(CONTAINS(?s, "x") || false)) }"#,
            0,
        ),
        // error || true is true; error && false is false, so its negation keeps the row.
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "x") || true) }"#,
            1,
        ),
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(!(CONTAINS(?s, "x") && false)) }"#,
            1,
        ),
        // IF with an erroring condition is an error.
        (
            r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(IF(CONTAINS(?s, "x"), true, true)) }"#,
            0,
        ),
        // BIND keeps the row and leaves the variable unbound.
        (
            r#"SELECT ?s ?b WHERE { ?s rdfs:label ?l BIND(!CONTAINS(?s, "x") AS ?b) }"#,
            1,
        ),
    ] {
        let (n, w) = run(&rete, q);
        assert_eq!(n, rows, "{q}");
        let w = one(&w);
        assert_eq!(
            (w.function.as_str(), w.argument, w.kind.as_str(), w.count),
            ("CONTAINS", 1, "iri", 1),
            "{q}: the error is counted once, where it was raised"
        );
        assert!(
            w.message.contains("drop the row, even under !"),
            "{}",
            w.message
        );
    }

    // The BIND above leaves ?b unbound rather than binding `true`.
    let q = format!(r#"{P}SELECT ?b WHERE {{ ?s rdfs:label ?l BIND(!CONTAINS(?s, "x") AS ?b) }}"#);
    match eval_query(&rete, &q).unwrap() {
        QueryOutput::Select(_, rows) => assert!(!rows[0].contains_key("b"), "{rows:?}"),
        _ => panic!("unexpected output"),
    }

    // `error && x` evaluates x too (E && F is F), so two errors in one row are
    // two reports, one per function — not the left one twice.
    let (n, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "x") && STRSTARTS(?s, "y")) }"#,
    );
    assert_eq!(n, 0);
    assert_eq!(w.len(), 2, "{w:#?}");
    assert!(w.iter().all(|w| w.count == 1), "{w:#?}");
}

#[test]
fn warnings_serialize_and_unknown_functions_fail_loudly() {
    let bytes = graph();
    let rete = Rete::open(&bytes).unwrap();
    let (_, w) = run(
        &rete,
        r#"SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }"#,
    );
    let json: serde_json::Value = serde_json::from_str(&warnings_json(&w)).unwrap();
    assert_eq!(json[0]["severity"], "type-error");
    assert_eq!(json[0]["function"], "CONTAINS");
    assert_eq!(json[0]["argument"], 1);
    assert_eq!(json[0]["argKind"], "iri");
    assert_eq!(json[0]["count"], 1);
    assert_eq!(json[0]["sample"], MAP);
    assert_eq!(warnings_json(&[]), "[]");

    // An extension-function IRI rete does not implement is an error, not a silent false.
    let err = eval_query(
        &rete,
        "SELECT ?s WHERE { ?s ?p ?o FILTER(<http://ex.org/fn#match>(?o, \"x\")) }",
    )
    .unwrap_err();
    assert!(
        matches!(&err, SparqlError::UnsupportedFunction(iri) if iri == "http://ex.org/fn#match")
    );
    assert!(
        err.to_string().contains("<http://ex.org/fn#match>"),
        "{err}"
    );
    // An unknown bare name is a parse error.
    assert!(matches!(
        eval_query(&rete, "SELECT ?s WHERE { ?s ?p ?o FILTER(NOSUCHFN(?o)) }"),
        Err(SparqlError::Parse(_))
    ));
}
