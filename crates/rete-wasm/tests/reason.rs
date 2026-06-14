//! Native tests for the in-memory `reason` binding (the `*_url` twins use
//! synchronous XHR and are worker-only, so they can't run here — but they share
//! the same `reasoning_json` envelope and `rete_core::reason` core, so an
//! in-memory check covers the JSON shape and the verdict logic).

use rete_core::{
    build_pyramid_meta, write_dataset, DictionaryBuilder, GraphIndexBuilder, DEFAULT_TILE_BUDGET,
};
use serde_json::Value;

const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const DISJOINT_WITH: &str = "<http://www.w3.org/2002/07/owl#disjointWith>";

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
fn reason_reports_a_directly_typed_disjoint_clash() {
    // x is typed as both C and D, which are owl:disjointWith — an incoherent point.
    let bytes = build_rete(&[
        ("<http://ex/C>", DISJOINT_WITH, "<http://ex/D>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/C>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/D>"),
    ]);

    let report: Value = serde_json::from_str(&rete_wasm::reason(&bytes, None).unwrap()).unwrap();

    assert_eq!(report["kind"], "reasoning");
    assert_eq!(report["coherent"], false);
    assert_eq!(report["inconsistencies"][0]["kind"], "disjoint-classes");
    // In-memory → no remote cost block.
    assert!(report.get("remote").is_none());
}

#[test]
fn reason_reports_a_propagated_disjoint_clash() {
    // The clash only surfaces after type propagation: x a C, C ⊑ D, D disjointWith E,
    // x a E. This is the case the mandatory subClassOf branch of COHERENCE_CONSTRUCT
    // exists to preserve.
    let bytes = build_rete(&[
        ("<http://ex/C>", SUBCLASS_OF, "<http://ex/D>"),
        ("<http://ex/D>", DISJOINT_WITH, "<http://ex/E>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/C>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/E>"),
    ]);

    let report: Value = serde_json::from_str(&rete_wasm::reason(&bytes, None).unwrap()).unwrap();

    assert_eq!(report["coherent"], false);
    assert_eq!(report["inconsistencies"][0]["kind"], "disjoint-classes");
    // It also entailed at least one new triple (x a D).
    assert!(report["inferredCount"].as_u64().unwrap() >= 1);
}

#[test]
fn coherence_construct_selects_a_sufficient_slice() {
    // The Tier-1 path: eval COHERENCE_CONSTRUCT over the graph, then reason over
    // ONLY that slice. The slice must carry enough T-Box (subClassOf + disjointWith)
    // for the propagated clash to surface — even though `name`/`knows` noise triples
    // are present and deliberately NOT pulled into the slice.
    use rete_core::{eval_query, QueryOutput, Rete};

    let bytes = build_rete(&[
        ("<http://ex/C>", SUBCLASS_OF, "<http://ex/D>"),
        ("<http://ex/D>", DISJOINT_WITH, "<http://ex/E>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/C>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/E>"),
        // Noise the coherence slice should not need:
        ("<http://ex/x>", "<http://ex/name>", "\"Xavier\""),
        ("<http://ex/x>", "<http://ex/knows>", "<http://ex/y>"),
        ("<http://ex/y>", "<http://ex/name>", "\"Yolanda\""),
    ]);

    let rete = Rete::open(&bytes).unwrap();
    let triples = match eval_query(&rete, rete_wasm::COHERENCE_CONSTRUCT).unwrap() {
        QueryOutput::Construct(t) => t,
        other => panic!("expected CONSTRUCT, got {other:?}"),
    };

    // The slice holds only rdf:type + the T-Box predicates, never name/knows.
    assert!(triples
        .iter()
        .all(|(_, p, _)| { p == RDF_TYPE || p == SUBCLASS_OF || p == DISJOINT_WITH }));

    let result = rete_core::reason(&triples);
    assert!(
        result
            .inconsistencies
            .iter()
            .any(|i| i.kind == "disjoint-classes"),
        "the coherence slice must surface the propagated disjoint clash, got {:?}",
        result.inconsistencies
    );
}

#[test]
fn reason_reports_a_coherent_graph_as_coherent() {
    let bytes = build_rete(&[
        ("<http://ex/C>", SUBCLASS_OF, "<http://ex/D>"),
        ("<http://ex/x>", RDF_TYPE, "<http://ex/C>"),
    ]);

    let report: Value = serde_json::from_str(&rete_wasm::reason(&bytes, None).unwrap()).unwrap();

    assert_eq!(report["coherent"], true);
    assert_eq!(report["inconsistencies"].as_array().unwrap().len(), 0);
}
