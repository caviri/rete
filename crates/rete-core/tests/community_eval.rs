//! Community-split SPARQL evaluation: split where the partition is sound,
//! evaluate globally where it is not — the merged answer must be **exactly**
//! the whole-graph answer for every supported shape (including multi-hop
//! joins whose solutions cross communities), and a query with nothing to
//! split must be refused, never answered redundantly.

use rete_core::{eval_query, eval_select_communities, ingest, QueryOutput, Rete, SparqlError};

/// A clustered scholarly-ish graph: three field communities of papers with
/// typed scores, titles, labelled fields, and intra-community citations plus
/// two cross-community bridge citations (0→12→24).
fn image() -> Vec<u8> {
    let mut nt = String::new();
    let mut add = |s: String| nt.push_str(&s);
    for c in 0..3u32 {
        add(format!(
            "<http://ex/field/{c}> <http://ex/label> \"Field {c}\" .\n"
        ));
        for i in 0..12u32 {
            let p = c * 12 + i;
            add(format!(
                "<http://ex/paper/{p}> <http://ex/score> \"{}.5\"^^<http://www.w3.org/2001/XMLSchema#double> .\n",
                (p % 7)
            ));
            add(format!(
                "<http://ex/paper/{p}> <http://ex/title> \"Paper {p}\" .\n"
            ));
            add(format!(
                "<http://ex/paper/{p}> <http://ex/field> <http://ex/field/{c}> .\n"
            ));
            // Odd papers get an optional award; even ones do not.
            if p % 2 == 1 {
                add(format!(
                    "<http://ex/paper/{p}> <http://ex/award> \"award-{p}\" .\n"
                ));
            }
            // Dense intra-community citations give Louvain real communities.
            for j in 0..12u32 {
                if i != j {
                    add(format!(
                        "<http://ex/paper/{p}> <http://ex/cites> <http://ex/paper/{}> .\n",
                        c * 12 + j
                    ));
                }
            }
        }
    }
    add("<http://ex/paper/0> <http://ex/cites> <http://ex/paper/12> .\n".into());
    add("<http://ex/paper/12> <http://ex/cites> <http://ex/paper/24> .\n".into());
    let quads = ingest::parse_statements(&nt, "nt").unwrap();
    ingest::assemble_dataset(&quads, &[]).0
}

fn select_rows(rete: &Rete, q: &str) -> (Vec<String>, Vec<rete_core::Binding>) {
    match eval_query(rete, q).unwrap() {
        QueryOutput::Select(vars, rows) => (vars, rows),
        other => panic!("expected select, got {other:?}"),
    }
}

#[test]
fn split_matches_whole_graph_across_shapes() {
    let image = image();
    let rete = Rete::open(&image).unwrap();

    let queries = [
        // The original prompting shape: star + typed-double FILTER + ORDER BY.
        "SELECT ?title ?score WHERE { \
           ?p <http://ex/score> ?score ; <http://ex/title> ?title . \
           FILTER(?score > 3.0) } ORDER BY DESC(?score) ?title",
        // Global aggregation over per-community partials.
        "SELECT ?f (COUNT(?p) AS ?n) WHERE { \
           ?p <http://ex/field> ?f ; <http://ex/score> ?s } \
         GROUP BY ?f ORDER BY ?f",
        // ORDER BY + LIMIT: the global top-k must survive the merge.
        "SELECT ?p ?s WHERE { ?p <http://ex/score> ?s } ORDER BY DESC(?s) ?p LIMIT 5",
        // DISTINCT over a star.
        "SELECT DISTINCT ?f WHERE { ?p <http://ex/field> ?f } ORDER BY ?f",
        // TWO stars joined on a non-subject variable (?p's star ⋈ ?f's star).
        "SELECT ?title ?l WHERE { \
           ?p <http://ex/title> ?title ; <http://ex/field> ?f . \
           ?f <http://ex/label> ?l } ORDER BY ?title",
        // Multi-hop join: solutions crossing communities (the 0→12→24
        // bridges) must survive the star decomposition + global join.
        "SELECT ?a ?c WHERE { ?a <http://ex/cites> ?b . ?b <http://ex/cites> ?c } \
         ORDER BY ?a ?c LIMIT 40",
        // OPTIONAL: unmatched left rows (even papers have no award) survive.
        "SELECT ?p ?award WHERE { ?p <http://ex/score> ?s . \
           OPTIONAL { ?p <http://ex/award> ?award } } ORDER BY ?p ?award",
        // UNION of two stars.
        "SELECT ?p WHERE { \
           { ?p <http://ex/field> <http://ex/field/0> } UNION \
           { ?p <http://ex/field> <http://ex/field/1> } } ORDER BY ?p",
        // MINUS over a star.
        "SELECT ?p WHERE { ?p <http://ex/score> ?s . \
           MINUS { ?p <http://ex/field> <http://ex/field/0> } } ORDER BY ?p",
        // A star joined with a property path (the path evaluates globally
        // inside the split — exact either way).
        "SELECT DISTINCT ?b WHERE { <http://ex/paper/0> <http://ex/cites>+ ?b . \
           ?b <http://ex/field> <http://ex/field/2> } ORDER BY ?b",
    ];
    for q in queries {
        let (vars, whole) = select_rows(&rete, q);
        assert!(!whole.is_empty(), "fixture should produce rows for {q}");
        let (svars, split, partials) = eval_select_communities(&rete, q, None).unwrap();
        assert_eq!(svars, vars, "vars for {q}");
        assert_eq!(split, whole, "rows for {q}");
        assert!(partials.len() > 1, "fixture should split into communities");
    }
}

#[test]
fn cross_community_bridges_survive_the_split() {
    // The explicit regression for the old star-only limitation: the 2-hop
    // chain 0→12→24 spans all three communities and must be in the answer.
    let image = image();
    let rete = Rete::open(&image).unwrap();
    let (_, rows, _) = eval_select_communities(
        &rete,
        "SELECT ?a ?c WHERE { ?a <http://ex/cites> ?b . ?b <http://ex/cites> ?c }",
        None,
    )
    .unwrap();
    let has = |a: &str, c: &str| {
        rows.iter().any(|r| {
            r.get("a").map(String::as_str) == Some(a) && r.get("c").map(String::as_str) == Some(c)
        })
    };
    assert!(
        has("<http://ex/paper/0>", "<http://ex/paper/24>"),
        "the community-crossing 0→12→24 chain must survive the split"
    );
}

#[test]
fn unsplittable_queries_are_refused() {
    let image = image();
    let rete = Rete::open(&image).unwrap();
    // A pure property path has no BGP to split — the strategy adds nothing.
    let err = eval_select_communities(
        &rete,
        "SELECT ?b WHERE { <http://ex/paper/0> <http://ex/cites>+ ?b }",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, SparqlError::Unsupported(_)), "{err}");

    // Non-SELECT forms are refused too.
    let err = eval_select_communities(&rete, "ASK { ?p <http://ex/score> ?s }", None).unwrap_err();
    assert!(matches!(err, SparqlError::Unsupported(_)), "{err}");
}
