//! SPARQL 1.2 triple terms against data written in RDF 1.2 and in RDF-star.
//!
//! The data side has read `<<( s p o )>>` for a while and stores it as the same
//! `<<s p o>>` token as an RDF-star quoted triple; the query side did not parse
//! it, so a SPARQL 1.2 query over rete's own files failed. Now `<<( … )>>`
//! parses as a triple-term pattern everywhere, and `<< … >>` keeps its
//! SPARQL-star meaning unless the query declares `VERSION "1.2"`, under which
//! it is a reified triple: `[ rdf:reifies <<( … )>> ]`.

use rete_core::ingest::{assemble_dataset, parse_statements};
use rete_core::{eval_query, eval_sparql, QueryOutput, Rete, SparqlError};

const DATA: &str = r#"<http://ex/snap> <http://ex/inserted> <<( <http://ex/w> <http://ex/title> "T" )>> .
<http://ex/snap> <http://ex/inserted> <<( <http://ex/w> <http://ex/page> "7" )>> .
_:r <http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies> <<( <http://ex/a> <http://ex/b> <http://ex/c> )>> .
_:r <http://ex/source> <http://ex/wiki> .
<< <http://ex/a> <http://ex/b> <http://ex/c> >> <http://ex/certainty> "0.9" .
<http://ex/a> <http://ex/b> <http://ex/c> .
"#;

fn rete() -> Rete {
    let quads = parse_statements(DATA, "nt").unwrap();
    let (bytes, _) = assemble_dataset(quads, &[]);
    Rete::open(&bytes).unwrap()
}

fn col(rete: &Rete, q: &str, var: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, q).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get(var).cloned()).collect();
    v.sort();
    v
}

#[test]
fn a_triple_term_pattern_matches_both_spellings_of_the_data() {
    let rete = rete();
    for (open, close) in [("<<(", ")>>"), ("<<", ">>")] {
        let q = format!(
            "SELECT ?p ?o WHERE {{ <http://ex/snap> <http://ex/inserted> \
             {open} <http://ex/w> ?p ?o {close} }}"
        );
        assert_eq!(
            col(&rete, &q, "o"),
            vec!["\"7\"".to_string(), "\"T\"".to_string()],
            "{open}"
        );
    }
    // Ground, nested in a VERSION "1.2" query, in VALUES, BIND and CONSTRUCT.
    let ground = "VERSION \"1.2\" SELECT ?s WHERE { ?s <http://ex/inserted> \
                  <<( <http://ex/w> <http://ex/title> \"T\" )>> }";
    assert_eq!(col(&rete, ground, "s"), vec!["<http://ex/snap>"]);
    let values = "SELECT ?s WHERE { VALUES ?t { <<( <http://ex/w> <http://ex/page> \"7\" )>> } \
                  ?s <http://ex/inserted> ?t }";
    assert_eq!(col(&rete, values, "s"), vec!["<http://ex/snap>"]);
    let bind = "SELECT ?s WHERE { BIND(<<( <http://ex/w> <http://ex/page> \"7\" )>> AS ?t) \
                ?s <http://ex/inserted> ?t }";
    assert_eq!(col(&rete, bind, "s"), vec!["<http://ex/snap>"]);
    let construct = "CONSTRUCT { ?w ?p ?o } WHERE { <http://ex/snap> <http://ex/inserted> \
                     <<( ?w ?p ?o )>> }";
    match eval_query(&rete, construct).unwrap() {
        QueryOutput::Construct(t) => assert_eq!(t.len(), 2),
        other => panic!("{other:?}"),
    }
}

#[test]
fn version_12_reads_a_quoted_triple_as_a_reifier() {
    let rete = rete();
    // SPARQL 1.2: `<< a b c >> :source ?src` is `?r rdf:reifies <<( a b c )>>
    // . ?r :source ?src` — the reifier `_:r` carries the source.
    let q = "VERSION \"1.2\" SELECT ?src WHERE { << <http://ex/a> <http://ex/b> <http://ex/c> >> \
             <http://ex/source> ?src }";
    assert_eq!(col(&rete, q, "src"), vec!["<http://ex/wiki>"]);
    // Without the declaration it is the RDF-star quoted triple as a subject.
    let star = "SELECT ?c WHERE { << <http://ex/a> <http://ex/b> <http://ex/c> >> \
                <http://ex/certainty> ?c }";
    assert_eq!(col(&rete, star, "c"), vec!["\"0.9\""]);
    // ... which the 1.2 reading does not see, and the other way round.
    let q12_certainty = format!("VERSION \"1.2\" {star}");
    assert!(col(&rete, &q12_certainty, "c").is_empty());
    let star_source = "SELECT ?src WHERE { << <http://ex/a> <http://ex/b> <http://ex/c> >> \
                       <http://ex/source> ?src }";
    assert!(col(&rete, star_source, "src").is_empty());
    // Inner variables, and a reifier in object position.
    let inner = "VERSION \"1.2\" SELECT ?o ?src WHERE { << <http://ex/a> <http://ex/b> ?o >> \
                 <http://ex/source> ?src }";
    assert_eq!(col(&rete, inner, "o"), vec!["<http://ex/c>"]);
    // SELECT * does not show the reifier (a blank node, not a variable).
    let star12 = "VERSION \"1.2\" SELECT * WHERE { << <http://ex/a> <http://ex/b> ?o >> \n                  <http://ex/source> ?src }";
    let (vars, sols) = eval_sparql(&rete, star12).unwrap();
    assert_eq!(sols.len(), 1);
    assert!(vars.iter().all(|v| v == "o" || v == "src"), "{vars:?}");
    assert!(
        sols[0].keys().all(|k| k == "o" || k == "src"),
        "{:?}",
        sols[0]
    );
}

#[test]
fn what_sparql12_forbids_is_refused() {
    let rete = rete();
    // A reified triple is not an expression in SPARQL 1.2.
    let expr = "VERSION \"1.2\" SELECT ?t WHERE { BIND(<< <http://ex/a> <http://ex/b> <http://ex/c> >> AS ?t) }";
    assert!(matches!(
        eval_sparql(&rete, expr),
        Err(SparqlError::Parse(_))
    ));
    // Named reifiers and annotations are refused by name, not misread.
    let named = "VERSION \"1.2\" SELECT ?r WHERE { << <http://ex/a> <http://ex/b> <http://ex/c> ~ ?r >> <http://ex/source> ?s }";
    assert!(matches!(
        eval_sparql(&rete, named),
        Err(SparqlError::Unsupported(_))
    ));
    let annotation = "VERSION \"1.2\" SELECT ?s WHERE { <http://ex/a> <http://ex/b> <http://ex/c> {| <http://ex/source> ?s |} }";
    assert!(matches!(
        eval_sparql(&rete, annotation),
        Err(SparqlError::Unsupported(_))
    ));
}
