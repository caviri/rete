#![cfg(target_arch = "wasm32")]

use js_sys::Error;
use rete_core::format::HEADER_LEN;
use rete_wasm::{build, header_ranges, summary_overview, Graph};
use serde_json::Value;
use wasm_bindgen::JsCast;
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const FIXTURE: &str = concat!(
    "<http://example.test/alice> <http://example.test/knows> ",
    "<http://example.test/bob> <http://example.test/graph> .\n",
    "<http://example.test/bob> <http://example.test/name> ",
    "\"Bob\" <http://example.test/graph> .\n",
);

fn fixture() -> Vec<u8> {
    build(FIXTURE, "nq").expect("browser fixture should build")
}

#[wasm_bindgen_test]
fn graph_opens_queries_and_reports_named_graphs() {
    let bytes = fixture();
    let graph = Graph::new(&bytes).expect("stable writer output should open");

    let names: Value = serde_json::from_str(&graph.graph_names().unwrap()).unwrap();
    assert_eq!(names, serde_json::json!(["<http://example.test/graph>"]));

    let result: Value = serde_json::from_str(
        &graph
            .query(
                "SELECT ?s WHERE { GRAPH <http://example.test/graph> { ?s <http://example.test/name> \"Bob\" } }",
                "json",
            )
            .unwrap(),
    )
    .unwrap();
    assert_eq!(result["schemaVersion"], 1);
    assert_eq!(result["kind"], "select");
    assert_eq!(result["rows"][0]["s"], "<http://example.test/bob>");
}

#[wasm_bindgen_test]
fn header_ranges_has_a_versioned_contract() {
    let bytes = fixture();
    let ranges: Value =
        serde_json::from_str(&header_ranges(&bytes[..HEADER_LEN]).unwrap()).unwrap();
    assert_eq!(ranges["schemaVersion"], 1);
    assert!(ranges["dictLen"].as_u64().unwrap() > 0);
    assert!(ranges["indexLen"].as_u64().unwrap() > 0);

    let overview: Value = serde_json::from_str(&summary_overview(&bytes).unwrap()).unwrap();
    assert_eq!(overview["schemaVersion"], 1);
    assert!(overview["communities"].is_u64());
    assert!(overview["predicateTotals"].is_array());
}

#[wasm_bindgen_test]
fn malformed_file_is_a_javascript_error() {
    let error = match Graph::new(b"not a rete file") {
        Ok(_) => panic!("malformed bytes must fail"),
        Err(error) => error,
    };
    assert!(
        error.is_instance_of::<Error>(),
        "bindings must throw Error objects, not strings"
    );
}
