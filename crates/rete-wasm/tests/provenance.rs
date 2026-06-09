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
fn why_triples_returns_browser_camel_case_provenance() {
    let bytes = build_rete(&[
        ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
        ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Alice>"),
        ("<http://ex/Alice>", "<http://ex/age>", "\"30\""),
    ]);

    let out: Value = serde_json::from_str(
        &rete_wasm::why_triples_json(&bytes, None, Some("<http://ex/knows>"), None)
            .expect("predicate provenance"),
    )
    .unwrap();

    assert_eq!(out["pattern"]["predicate"], "<http://ex/knows>");
    assert_eq!(out["resultCount"], 2);
    assert_eq!(out["results"].as_array().unwrap().len(), 2);

    let first = &out["results"][0];
    assert_eq!(first["terms"]["predicate"], "<http://ex/knows>");
    assert_eq!(first["provenance"]["graph"], "default");
    assert_eq!(first["provenance"]["indexPermutation"], "POS");
    assert_eq!(first["provenance"]["indexSection"], 1);
    assert!(
        first["provenance"]["dictionaryRange"]["len"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert!(first["provenance"]["indexRange"]["len"].as_u64().unwrap() > 0);
    assert!(first["provenance"]["pyramidRange"]["len"].as_u64().unwrap() > 0);
    assert_eq!(first["provenance"]["tile"]["available"], false);
    assert_eq!(first["provenance"]["tile"]["reason"], "not_materialized");
}

#[test]
fn why_triples_chooses_representative_index_permutations() {
    let bytes = build_rete(&[
        ("<http://ex/Alice>", "<http://ex/knows>", "<http://ex/Bob>"),
        ("<http://ex/Bob>", "<http://ex/knows>", "<http://ex/Alice>"),
        ("<http://ex/Alice>", "<http://ex/age>", "\"30\""),
    ]);

    let spo: Value = serde_json::from_str(
        &rete_wasm::why_triples_json(
            &bytes,
            Some("<http://ex/Alice>"),
            Some("<http://ex/knows>"),
            None,
        )
        .expect("spo provenance"),
    )
    .unwrap();
    assert_eq!(spo["results"][0]["provenance"]["indexPermutation"], "SPO");

    let pos: Value = serde_json::from_str(
        &rete_wasm::why_triples_json(&bytes, None, Some("<http://ex/knows>"), None)
            .expect("pos provenance"),
    )
    .unwrap();
    assert_eq!(pos["results"][0]["provenance"]["indexPermutation"], "POS");

    let osp: Value = serde_json::from_str(
        &rete_wasm::why_triples_json(&bytes, None, None, Some("<http://ex/Bob>"))
            .expect("osp provenance"),
    )
    .unwrap();
    assert_eq!(osp["results"][0]["provenance"]["indexPermutation"], "OSP");
}
