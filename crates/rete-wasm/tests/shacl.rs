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
fn shacl_binding_reports_conformance_and_violations_as_json() {
    let bytes = build_rete(&[
        (
            "<http://ex/alice>",
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>",
            "<http://ex/Person>",
        ),
        (
            "<http://ex/alice>",
            "<http://ex/email>",
            "\"alice@example.org\"",
        ),
    ]);
    let shapes = r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .

        ex:PersonShape a sh:NodeShape ;
          sh:targetClass ex:Person ;
          sh:property [
            sh:path ex:email ;
            sh:minCount 2
          ] .
    "#;

    let report: Value = serde_json::from_str(
        &rete_wasm::shacl(&bytes, shapes, None, "json").expect("shacl report"),
    )
    .unwrap();

    assert_eq!(report["conforms"], false);
    assert_eq!(
        report["results"][0]["sourceConstraintComponent"],
        "http://www.w3.org/ns/shacl#MinCountConstraintComponent"
    );
}
