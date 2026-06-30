//! Direct tests for the NON-deterministic SPARQL built-ins (RAND / UUID /
//! STRUUID / BNODE). They can't be checked against Oxigraph (different output
//! every call), so assert their structural contract instead — covering the
//! `expr.rs` branches the differential oracle can't reach. (NOW() is not a built-
//! in in rete — there's no wall clock, consistent with the wasm-compat constraint
//! that keeps `std::time` out of the core.)

use rete_core::{
    build_pyramid_meta, eval_sparql, write_file, DictionaryBuilder, GraphIndexBuilder, Rete,
    DEFAULT_TILE_BUDGET,
};

fn rete() -> Rete {
    let mut db = DictionaryBuilder::new();
    db.observe("<http://ex/s>", "<http://ex/p>", "<http://ex/o>");
    let dict = db.build();
    let id = dict
        .encode("<http://ex/s>", "<http://ex/p>", "<http://ex/o>")
        .unwrap();
    let mut ib = GraphIndexBuilder::new();
    ib.push(id);
    let (meta, levels) = build_pyramid_meta(&dict, &[id], DEFAULT_TILE_BUDGET);
    Rete::open(&write_file(&dict, &ib.build(), false, &meta, levels)).unwrap()
}

/// The value of `?r` from a one-row query (the single triple is the row source).
fn val(rete: &Rete, q: &str) -> String {
    let (_, sols) = eval_sparql(rete, q).expect("query evaluates");
    sols.first()
        .and_then(|b| b.get("r").cloned())
        .unwrap_or_default()
}

/// The lexical inside a literal token `"…"^^<…>` / `"…"` (else the token).
fn lex(token: &str) -> String {
    token
        .strip_prefix('"')
        .and_then(|r| r.split('"').next())
        .unwrap_or(token)
        .to_string()
}

#[test]
fn rand_is_in_unit_interval() {
    let r = rete();
    for _ in 0..25 {
        let v = val(&r, "SELECT (RAND() AS ?r) WHERE { ?s ?p ?o }");
        let n: f64 = lex(&v)
            .parse()
            .unwrap_or_else(|_| panic!("RAND not numeric: {v}"));
        assert!((0.0..1.0).contains(&n), "RAND out of [0,1): {n}");
    }
}

#[test]
fn uuid_is_a_urn_uuid_iri() {
    let v = val(&rete(), "SELECT (STR(UUID()) AS ?r) WHERE { ?s ?p ?o }");
    assert!(
        lex(&v).contains("urn:uuid:"),
        "UUID not a urn:uuid: IRI: {v}"
    );
}

#[test]
fn struuid_has_uuid_shape_and_varies() {
    let r = rete();
    let a = lex(&val(&r, "SELECT (STRUUID() AS ?r) WHERE { ?s ?p ?o }"));
    let b = lex(&val(&r, "SELECT (STRUUID() AS ?r) WHERE { ?s ?p ?o }"));
    assert_eq!(a.len(), 36, "STRUUID wrong length: {a}");
    assert_eq!(a.matches('-').count(), 4, "STRUUID wrong shape: {a}");
    assert_ne!(a, b, "STRUUID must differ across calls");
}

#[test]
fn bnode_is_a_blank_node() {
    let v = val(&rete(), "SELECT (BNODE() AS ?r) WHERE { ?s ?p ?o }");
    assert!(v.starts_with("_:"), "BNODE not a blank node: {v}");
}
