use rete_core::{
    build_pyramid_meta, write_dataset, DictionaryBuilder, GraphIndexBuilder, DEFAULT_TILE_BUDGET,
};
use serde_json::Value;

fn build_rete(triples: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in triples {
        db.observe(s, p, o);
    }
    let dict = db.build();

    let encoded: Vec<_> = triples
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).expect("observed term"))
        .collect();
    let mut index = GraphIndexBuilder::new();
    for t in &encoded {
        index.push(*t);
    }
    let index = index.build();
    let (meta, levels) = build_pyramid_meta(&dict, &encoded, DEFAULT_TILE_BUDGET);
    write_dataset(&dict, &index, &[], false, &meta, levels)
}

#[test]
fn progressive_query_answers_summary_safe_shapes_without_index() {
    let bytes = build_rete(&[
        ("<http://ex/a>", "<http://ex/knows>", "<http://ex/b>"),
        ("<http://ex/b>", "<http://ex/knows>", "<http://ex/a>"),
        ("<http://ex/a>", "<http://ex/age>", "\"30\""),
    ]);

    let count: Value = serde_json::from_str(
        &rete_wasm::progressive_query(
            &bytes,
            "PREFIX ex: <http://ex/> SELECT (COUNT(*) AS ?total) WHERE { ?s ex:knows ?o }",
        )
        .expect("predicate count"),
    )
    .unwrap();
    assert_eq!(count["kind"], "select");
    assert_eq!(count["vars"], serde_json::json!(["total"]));
    assert_eq!(
        count["rows"][0]["total"],
        "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );
    assert_eq!(count["progressive"]["stage"], "summary");
    assert_eq!(count["progressive"]["queryShape"], "predicate_count");
    assert_eq!(count["progressive"]["predicate"], "<http://ex/knows>");
    assert_eq!(count["progressive"]["readsIndex"], false);

    let total: Value = serde_json::from_str(
        &rete_wasm::progressive_query(&bytes, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")
            .expect("triple count"),
    )
    .unwrap();
    assert_eq!(total["progressive"]["queryShape"], "triple_count");
    assert_eq!(
        total["rows"][0]["n"],
        "\"3\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );

    let totals: Value = serde_json::from_str(
        &rete_wasm::progressive_query(
            &bytes,
            "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p",
        )
        .expect("predicate totals"),
    )
    .unwrap();
    assert_eq!(totals["kind"], "select");
    assert_eq!(totals["vars"], serde_json::json!(["p", "n"]));
    assert_eq!(totals["rows"][0]["p"], "<http://ex/knows>");
    assert_eq!(
        totals["rows"][0]["n"],
        "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );
    assert_eq!(totals["rows"][1]["p"], "<http://ex/age>");
    assert_eq!(
        totals["rows"][1]["n"],
        "\"1\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );
    assert_eq!(totals["progressive"]["queryShape"], "predicate_totals");
    assert_eq!(totals["progressive"]["readsIndex"], false);

    let predicates: Value = serde_json::from_str(
        &rete_wasm::progressive_query(&bytes, "SELECT DISTINCT ?p WHERE { ?s ?p ?o }")
            .expect("predicate list"),
    )
    .unwrap();
    assert_eq!(predicates["kind"], "select");
    assert_eq!(predicates["vars"], serde_json::json!(["p"]));
    assert_eq!(predicates["rows"][0]["p"], "<http://ex/knows>");
    assert_eq!(predicates["rows"][1]["p"], "<http://ex/age>");
    assert_eq!(predicates["progressive"]["queryShape"], "predicate_list");
    assert_eq!(predicates["progressive"]["readsIndex"], false);

    let predicate_count: Value = serde_json::from_str(
        &rete_wasm::progressive_query(
            &bytes,
            "SELECT (COUNT(DISTINCT ?p) AS ?predicateCount) WHERE { ?s ?p ?o }",
        )
        .expect("predicate distinct count"),
    )
    .unwrap();
    assert_eq!(predicate_count["kind"], "select");
    assert_eq!(
        predicate_count["vars"],
        serde_json::json!(["predicateCount"])
    );
    assert_eq!(
        predicate_count["rows"][0]["predicateCount"],
        "\"2\"^^<http://www.w3.org/2001/XMLSchema#integer>"
    );
    assert_eq!(
        predicate_count["progressive"]["queryShape"],
        "predicate_distinct_count"
    );
    assert_eq!(predicate_count["progressive"]["readsIndex"], false);

    let ask: Value = serde_json::from_str(
        &rete_wasm::progressive_query(&bytes, "PREFIX ex: <http://ex/> ASK { ?s ex:knows ?o }")
            .expect("ask"),
    )
    .unwrap();
    assert_eq!(ask["kind"], "ask");
    assert_eq!(ask["boolean"], true);
    assert_eq!(ask["progressive"]["queryShape"], "predicate_exists");

    let any_ask: Value = serde_json::from_str(
        &rete_wasm::progressive_query(&bytes, "ASK { ?s ?p ?o }").expect("any ask"),
    )
    .unwrap();
    assert_eq!(any_ask["kind"], "ask");
    assert_eq!(any_ask["boolean"], true);
    assert_eq!(any_ask["progressive"]["queryShape"], "triple_exists");
    assert_eq!(any_ask["progressive"]["readsIndex"], false);

    let err = rete_wasm::progressive_query_json(
        &bytes,
        "PREFIX ex: <http://ex/> SELECT ?s WHERE { ?s ex:knows ?o }",
    )
    .expect_err("non-summary shape rejected");
    assert!(err.contains("not exactly answerable"));
}
