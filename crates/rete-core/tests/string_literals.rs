//! `"x"` and `"x"^^xsd:string` are ONE term in RDF 1.1.
//!
//! rete compared them as two: a pipeline that wrote typed strings could not be
//! matched by a plain constant (OpenCitations' pipelines worked around it with
//! rule R-ocdm-2). Now every input syntax stores the plain spelling, query
//! constants and computed values use it, and a file built before this — which
//! may hold the typed spelling — still matches a plain constant and shows the
//! plain spelling in its results.

use rete_core::ingest::{assemble_dataset, parse_statements};
use rete_core::{eval_sparql, write_file, DictionaryBuilder, GraphIndexBuilder, Rete};

const XS: &str = "http://www.w3.org/2001/XMLSchema#string";

fn stored_terms(rete: &Rete) -> Vec<String> {
    let mut v: Vec<String> = rete.dump(None).into_iter().map(|(_, _, o)| o).collect();
    v.sort();
    v
}

#[test]
fn every_input_syntax_stores_the_plain_spelling() {
    let inputs = [
        (
            "nt",
            format!("<http://ex/a> <http://ex/v> \"x\"^^<{XS}> .\n<http://ex/b> <http://ex/v> \"x\" .\n"),
        ),
        (
            "nq",
            format!("<http://ex/a> <http://ex/v> \"x\"^^<{XS}> <http://ex/g> .\n<http://ex/b> <http://ex/v> \"x\" .\n"),
        ),
        (
            "ttl",
            format!("<http://ex/a> <http://ex/v> \"x\"^^<{XS}> .\n<http://ex/b> <http://ex/v> \"x\" .\n"),
        ),
        (
            "rdfxml",
            format!(
                r#"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://ex/">
  <rdf:Description rdf:about="http://ex/a"><ex:v rdf:datatype="{XS}">x</ex:v></rdf:Description>
  <rdf:Description rdf:about="http://ex/b"><ex:v>x</ex:v></rdf:Description>
</rdf:RDF>"#
            ),
        ),
    ];
    for (format, text) in inputs {
        let quads = parse_statements(&text, format).unwrap();
        assert!(quads.iter().all(|q| q.2 == "\"x\""), "{format}: {quads:?}");
    }
    // Inside a quoted triple too (N-Triples-star and RDF 1.2 triple terms).
    let nt = format!(
        "<http://ex/s> <http://ex/says> << <http://ex/a> <http://ex/v> \"x\"^^<{XS}> >> .\n\
         <http://ex/s> <http://ex/says> <<( <http://ex/b> <http://ex/v> \"y\"^^<{XS}> )>> .\n"
    );
    let objects: Vec<String> = parse_statements(&nt, "nt")
        .unwrap()
        .into_iter()
        .map(|q| q.2)
        .collect();
    assert_eq!(
        objects,
        vec![
            "<<<http://ex/a> <http://ex/v> \"x\">>".to_string(),
            "<<<http://ex/b> <http://ex/v> \"y\">>".to_string(),
        ]
    );
    // One term, so one dictionary entry and two subjects for it.
    let quads = parse_statements(&inputs_nt(), "nt").unwrap();
    let (bytes, _) = assemble_dataset(quads, &[]);
    let rete = Rete::open(&bytes).unwrap();
    assert_eq!(stored_terms(&rete), vec!["\"x\"", "\"x\""]);
    let (_, sols) = eval_sparql(&rete, "SELECT ?s WHERE { ?s <http://ex/v> \"x\" }").unwrap();
    assert_eq!(sols.len(), 2);
}

fn inputs_nt() -> String {
    format!("<http://ex/a> <http://ex/v> \"x\"^^<{XS}> .\n<http://ex/b> <http://ex/v> \"x\" .\n")
}

/// A file as an older build wrote it: the typed spelling in the dictionary.
fn old_file() -> Rete {
    let typed = format!("\"x\"^^<{XS}>");
    let triples = [
        ("<http://ex/a>", "<http://ex/v>", typed.as_str()),
        ("<http://ex/b>", "<http://ex/w>", "\"y\""),
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
    Rete::open(&write_file(&dict, &ib.build(), false, &[], 0)).unwrap()
}

#[test]
fn an_older_file_with_typed_strings_matches_plain_constants() {
    let rete = old_file();
    let one = |q: &str| -> Vec<String> {
        let (_, sols) = eval_sparql(&rete, q).unwrap();
        let mut v: Vec<String> = sols
            .iter()
            .map(|b| b.values().cloned().collect::<Vec<_>>().join(" "))
            .collect();
        v.sort();
        v
    };
    // A plain constant, and the typed one (the parser spells it plain).
    assert_eq!(
        one("SELECT ?s WHERE { ?s <http://ex/v> \"x\" }"),
        vec!["<http://ex/a>"]
    );
    assert_eq!(
        one(&format!(
            "SELECT ?s WHERE {{ ?s <http://ex/v> \"x\"^^<{XS}> }}"
        )),
        vec!["<http://ex/a>"]
    );
    // VALUES, a join with a computed value, sameTerm and = all see one term.
    assert_eq!(
        one("SELECT ?s WHERE { VALUES ?o { \"x\" } ?s <http://ex/v> ?o }"),
        vec!["<http://ex/a>"]
    );
    assert_eq!(
        one("SELECT ?s WHERE { BIND(STR(\"x\") AS ?o) ?s <http://ex/v> ?o }"),
        vec!["<http://ex/a>"]
    );
    assert_eq!(
        one("SELECT ?s WHERE { ?s <http://ex/v> ?o FILTER(sameTerm(?o, \"x\")) }"),
        vec!["<http://ex/a>"]
    );
    assert_eq!(
        one(&format!(
            "SELECT ?s WHERE {{ ?s <http://ex/v> ?o FILTER(?o = STRDT(\"x\", <{XS}>)) }}"
        )),
        vec!["<http://ex/a>"]
    );
    // Results show the canonical spelling; DATATYPE is still xsd:string.
    assert_eq!(
        one("SELECT ?o WHERE { <http://ex/a> <http://ex/v> ?o }"),
        vec!["\"x\""]
    );
    assert_eq!(
        one("SELECT (DATATYPE(?o) AS ?d) WHERE { <http://ex/a> <http://ex/v> ?o }"),
        vec![format!("<{XS}>")]
    );
}

#[test]
fn computed_strings_are_plain() {
    let rete = old_file();
    let (_, sols) = eval_sparql(
        &rete,
        &format!("SELECT ?a ?b WHERE {{ BIND(STRDT(\"z\", <{XS}>) AS ?a) BIND(<{XS}>(1) AS ?b) }}"),
    )
    .unwrap();
    assert_eq!(sols[0]["a"], "\"z\"");
    assert_eq!(sols[0]["b"], "\"1\"");
}
