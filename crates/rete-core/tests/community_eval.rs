//! Community-split SPARQL evaluation: for subject-star queries, evaluating
//! per pyramid community and merging (modifiers applied globally) must give
//! **exactly** the whole-graph answer — and non-star shapes must be refused,
//! never answered wrongly.

use rete_core::{eval_query, eval_select_communities, ingest, QueryOutput, Rete, SparqlError};

/// A clustered scholarly-ish graph: three field communities of papers with
/// typed scores, titles, and intra-community citations (plus two bridges).
fn image() -> Vec<u8> {
    let mut nt = String::new();
    let mut add = |s: String| nt.push_str(&s);
    for c in 0..3u32 {
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
fn split_matches_whole_graph_for_star_queries() {
    let image = image();
    let rete = Rete::open(&image).unwrap();

    let queries = [
        // The user-facing shape: star + typed-double FILTER + global ORDER BY.
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
    ];
    for q in queries {
        let (vars, whole) = select_rows(&rete, q);
        let (svars, split, partials) = eval_select_communities(&rete, q, None).unwrap();
        assert_eq!(svars, vars, "vars for {q}");
        assert_eq!(split, whole, "rows for {q}");
        assert!(partials.len() > 1, "fixture should split into communities");
        let contributed: usize = partials.iter().map(|p| p.rows).sum();
        assert!(
            contributed >= whole.len(),
            "partials must cover the merged answer for {q}"
        );
    }
}

#[test]
fn non_star_shapes_are_refused_not_answered() {
    let image = image();
    let rete = Rete::open(&image).unwrap();
    // A 2-hop join crosses subjects (solutions can span communities):
    // splitting it would silently drop cross-community rows, so it must err.
    let err = eval_select_communities(
        &rete,
        "SELECT ?a ?c WHERE { ?a <http://ex/cites> ?b . ?b <http://ex/cites> ?c }",
        None,
    )
    .unwrap_err();
    assert!(matches!(err, SparqlError::Unsupported(_)), "{err}");

    // Non-SELECT forms are refused too.
    let err = eval_select_communities(&rete, "ASK { ?p <http://ex/score> ?s }", None).unwrap_err();
    assert!(matches!(err, SparqlError::Unsupported(_)), "{err}");
}
