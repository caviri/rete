use rete_core::format::{Header, Rete, CURRENT_FORMAT_VERSION};
use rete_core::query::{eval_query, QueryOutput};
use rete_core::range::{RangeReader, SliceReader, SummaryView};
use rete_core::reasoning::{reason, REASON_RULESET};
use rete_core::validation::{validate_shacl, ReteGraph, ShaclShapes};

#[test]
fn documented_facade_is_available_to_external_crates() {
    assert_eq!(CURRENT_FORMAT_VERSION, 0x05);
    let bytes = include_bytes!("fixtures/v1/minimal.rete");
    let header = Header::from_bytes(&bytes[..rete_core::format::HEADER_LEN]).unwrap();
    assert_eq!(header.version, 0x05);
    let graph = Rete::open(bytes).unwrap();
    assert_eq!(graph.query(None, None, None).len(), 2);
}

#[test]
fn all_stable_facades_support_the_documented_entry_points() {
    let bytes = include_bytes!("fixtures/v1/minimal.rete");
    let graph = Rete::open(bytes).unwrap();

    match eval_query(&graph, "SELECT ?s WHERE { ?s ?p ?o }").unwrap() {
        QueryOutput::Select(vars, rows) => {
            assert_eq!(vars, ["s"]);
            assert_eq!(rows.len(), 2);
        }
        _ => panic!("SELECT returned a non-SELECT result"),
    }

    let reader = SliceReader::new(bytes);
    assert_eq!(reader.len(), bytes.len() as u64);
    let _summary = SummaryView::open_ranged(&reader).unwrap();

    let shapes = ShaclShapes::parse_turtle("").unwrap();
    let report = validate_shacl(&ReteGraph::new(&graph), &shapes);
    assert!(report.conforms);

    let reasoning = reason(&graph.query(None, None, None));
    assert_eq!(REASON_RULESET, "owl-rl-subset/v1");
    assert!(reasoning.inconsistencies.is_empty());
}
