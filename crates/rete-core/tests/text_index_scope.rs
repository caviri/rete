//! The full-text index's **scope** (`rete build --text-index-graphs`,
//! `--text-index-include`, `--text-index-exclude`) against SPARQL: the
//! `FILTER(CONTAINS(…))` pushdown prunes subjects with the index only where the
//! index covers the active graph and the pattern's predicate. Every query here
//! must return exactly the rows a file without any text index returns, for every
//! scope — the index may only ever make a query faster.

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{eval_sparql, Rete, TextIndexGraphs, TextIndexScope};

fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> RawQuad {
    (
        format!("<http://ex/{s}>"),
        format!("<http://ex/{p}>"),
        o.to_string(),
        g.map(|g| format!("<http://ex/{g}>")),
    )
}

/// Data in the default graph, provenance in two named graphs: the shape of an
/// OpenCitations entity and its `…/prov/` graph.
fn quads() -> Vec<RawQuad> {
    vec![
        quad("e1", "title", "\"Glucose metabolism\"", None),
        quad("e1", "desc", "\"The entity has been modified.\"", None),
        quad("e2", "title", "\"Benzene ring\"", None),
        quad("e3", "desc", "\"modified twice\"", None),
        quad("e3", "kind", "<http://ex/Thing>", None),
        quad("p1", "desc", "\"The entity has been created.\"", Some("g1")),
        quad("p1", "note", "\"created by an agent\"", Some("g1")),
        quad(
            "p2",
            "desc",
            "\"The entity has been modified.\"",
            Some("g2"),
        ),
    ]
}

fn build(scope: Option<TextIndexScope>) -> Vec<u8> {
    let (bytes, _) = assemble_dataset_with_opts(quads(), false, scope, None, |_, _| Vec::new());
    bytes
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, &format!("PREFIX ex: <http://ex/> {q}")).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("s").cloned()).collect();
    v.sort();
    v
}

fn scopes() -> Vec<(&'static str, TextIndexScope)> {
    let none: [&str; 0] = [];
    vec![
        ("default", TextIndexScope::default()),
        (
            "all graphs",
            TextIndexScope::from_options(TextIndexGraphs::All, none, none).unwrap(),
        ),
        (
            "exclude desc",
            TextIndexScope::from_options(TextIndexGraphs::Default, none, ["http://ex/desc"])
                .unwrap(),
        ),
        (
            "include desc, all graphs",
            TextIndexScope::from_options(TextIndexGraphs::All, ["http://ex/desc"], none).unwrap(),
        ),
        (
            "include title",
            TextIndexScope::from_options(TextIndexGraphs::Default, ["<http://ex/title>"], none)
                .unwrap(),
        ),
    ]
}

const QUERIES: &[&str] = &[
    // The default graph, a covered or an excluded predicate.
    "SELECT ?s WHERE { ?s ex:desc ?o FILTER(CONTAINS(?o, \"modified\")) }",
    "SELECT ?s WHERE { ?s ex:title ?o FILTER(CONTAINS(?o, \"Glucose\")) }",
    // A variable predicate: only an every-predicate index may prune it.
    "SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"modified\")) }",
    // Inside GRAPH: a default-graph index never saw these literals (the
    // "CONTAINS inside GRAPH returns 0 rows" defect).
    "SELECT ?s WHERE { GRAPH ex:g1 { ?s ?p ?o FILTER(CONTAINS(?o, \"created\")) } }",
    "SELECT ?s WHERE { GRAPH ?g { ?s ex:desc ?o FILTER(CONTAINS(?o, \"modified\")) } }",
    // FROM makes a named graph the active default graph.
    "SELECT ?s FROM ex:g1 WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"created\")) }",
];

#[test]
fn every_scope_answers_contains_like_a_file_without_an_index() {
    let plain = build(None);
    let rete_plain = Rete::open(&plain).unwrap();
    for q in QUERIES {
        assert!(
            !rows(&rete_plain, q).is_empty(),
            "fixture answers nothing: {q}"
        );
    }
    for (name, scope) in scopes() {
        let bytes = build(Some(scope.clone()));
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(
            rete.header().text_index_partial,
            scope.restricts_predicates()
        );
        for q in QUERIES {
            assert_eq!(
                rows(&rete, q),
                rows(&rete_plain, q),
                "scope {name}: indexed and plain diverge for {q}"
            );
        }
    }
}

/// `rete search` reads the same scope: named-graph text only under `all`,
/// excluded predicates never.
#[test]
fn text_search_follows_the_scope() {
    let search = |scope: TextIndexScope, word: &str| {
        let bytes = build(Some(scope));
        Rete::open(&bytes).unwrap().text_search(&[word], None, 0)
    };
    let all = scopes();
    assert_eq!(search(all[0].1.clone(), "created"), Vec::<String>::new());
    assert_eq!(search(all[1].1.clone(), "created"), ["<http://ex/p1>"]);
    assert_eq!(
        search(all[0].1.clone(), "modified"),
        ["<http://ex/e1>", "<http://ex/e3>"]
    );
    assert_eq!(
        search(all[1].1.clone(), "modified"),
        ["<http://ex/e1>", "<http://ex/e3>", "<http://ex/p2>"]
    );
    assert_eq!(search(all[2].1.clone(), "modified"), Vec::<String>::new());
    assert_eq!(search(all[2].1.clone(), "glucose"), ["<http://ex/e1>"]);
    assert_eq!(search(all[4].1.clone(), "modified"), Vec::<String>::new());
}

/// The default scope is the pre-0.4.0 index, byte for byte: no scope block,
/// section kind 6.
#[test]
fn default_scope_is_the_unscoped_index() {
    let unscoped = {
        let (b, _) = assemble_dataset_with_opts(quads(), false, true, None, |_, _| Vec::new());
        b
    };
    assert_eq!(build(Some(TextIndexScope::default())), unscoped);
    let h = rete_core::Header::from_bytes(&unscoped).unwrap();
    assert!(h.text_index_len > 0 && !h.text_index_partial);
}
