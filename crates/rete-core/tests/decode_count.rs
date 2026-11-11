//! A query decodes what its output needs, not what it passes through (the
//! `evidence` shape of the nanopubs explorer: a graph variable joined twice,
//! 18 rows out of ten million graphs). Counted by `terms_decoded`, the number
//! of dictionary terms decoded from the sections, so this file holds ONE test
//! (the counter is process-wide).

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::{eval_query, terms_decoded, PermSet, PyramidAlgo, QueryOutput, Rete};

const N: usize = 3000;

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

/// `N` nanopub-like graphs: each holds one `wi:evidence` triple over a grade
/// (6 grades in all, 3 of them labelled).
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for k in 0..N {
        let g = iri(&format!("np/{k:05}/pubinfo"));
        let grade = iri(&format!("grade/{}", k % 6));
        q.push((
            iri(&format!("np/{k:05}")),
            iri("evidence"),
            grade.clone(),
            Some(g.clone()),
        ));
        if k % 6 < 3 {
            q.push((grade, iri("label"), format!("\"grade {}\"", k % 6), Some(g)));
        }
    }
    q
}

fn file() -> Vec<u8> {
    std::env::set_var("RETE_WRITE_FORMAT", "5");
    let bytes = assemble_dataset_with_layout(
        dataset(),
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        GraphLayout::Quad,
        |_, _| Vec::<u8>::new(),
    )
    .0;
    std::env::remove_var("RETE_WRITE_FORMAT");
    bytes
}

fn run(rete: &Rete, q: &str) -> (Vec<String>, u64) {
    let before = terms_decoded();
    let out = eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}"));
    let used = terms_decoded() - before;
    match out {
        QueryOutput::Select(_, rows) => {
            (rows.into_iter().map(|b| format!("{b:?}")).collect(), used)
        }
        other => panic!("unexpected output {other:?}"),
    }
}

#[test]
fn a_bound_graph_and_a_group_by_decode_only_the_output() {
    let bytes = file();
    let rete = Rete::open(&bytes).unwrap();

    // The graph variable is bound by the left side and looked up on the right
    // for each of the N rows: the lookup goes by id, not by the graph's name.
    let q = "SELECT ?e ?l (COUNT(*) AS ?n) WHERE { \
             GRAPH ?g { ?x <http://ex/evidence> ?e } \
             OPTIONAL { GRAPH ?g { ?e <http://ex/label> ?l } } \
             } GROUP BY ?e ?l ORDER BY ?e";
    let (rows, used) = run(&rete, q);
    assert_eq!(rows.len(), 6);
    assert!(
        rows.iter().filter(|r| r.contains("\"l\"")).count() == 3,
        "3 grades have a label: {rows:?}"
    );
    // 6 grades and 3 labels are the output (the ORDER BY key and the
    // projection may each decode a group once): a few dozen, never one per graph.
    assert!(used < 100, "decoded {used} terms for {N} graphs");

    // The same through a FILTER on the graph variable.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?x <http://ex/evidence> ?e } \
             OPTIONAL { GRAPH ?g { ?e <http://ex/label> ?l } } }";
    let (rows, used) = run(&rete, q);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains(&N.to_string()), "{rows:?}");
    assert!(used < 20, "decoded {used} terms for a count");

    // A graph variable over several patterns walks the graphs that can match;
    // the walk binds each graph as an id, it does not decode its name.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?x <http://ex/evidence> ?e .              ?e <http://ex/label> ?l } }";
    let (rows, used) = run(&rete, q);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].contains(&(N / 2).to_string()), "{rows:?}");
    assert!(used < 20, "decoded {used} terms for a count over a walk");
}
