//! Every shortcut the evaluator takes must answer over the query's ACTIVE
//! graph — the default graph, a `GRAPH <g>` block, the `FROM` merge, or the
//! union-default toggle — never the file's default graph by accident.
//!
//! Regression net for `FILTER(CONTAINS(…))` inside `GRAPH <g> { … }` returning
//! no rows (the TEXT_INDEX pushdown seeded the scan from the default graph's
//! words), and for the same mistake in sub-SELECTs and DESCRIBE. Each query is
//! checked on a file WITH the text index against the same file WITHOUT it, and
//! against the answer written out by hand.

use rete_core::ingest::assemble_dataset_with_opts;
use rete_core::{eval_query, eval_query_with, eval_sparql, QueryOpts, QueryOutput, Rete};

const P: &str = "PREFIX ex: <http://ex/> ";

/// Default graph: two labelled resources. Named graph `<http://ex/g>`: three,
/// one of whose words ("observatory") the default graph also uses, so a
/// text-index seed taken from the wrong graph is non-empty and wrong.
fn quads() -> Vec<(String, String, String, Option<String>)> {
    let q = |s: &str, p: &str, o: &str, g: Option<&str>| {
        (
            format!("<http://ex/{s}>"),
            format!("<http://ex/{p}>"),
            o.to_string(),
            g.map(|g| format!("<http://ex/{g}>")),
        )
    };
    vec![
        q("d1", "label", "\"Royal Observatory\"", None),
        q("d2", "label", "\"plain building\"", None),
        q("n1", "label", "\"The entity has been created.\"", Some("g")),
        q("n2", "label", "\"an observatory in a graph\"", Some("g")),
        q(
            "n3",
            "label",
            "\"The entity has been modified.\"",
            Some("g"),
        ),
        q("n1", "kind", "<http://ex/Snapshot>", Some("g")),
    ]
}

fn files() -> (Rete, Rete) {
    let build = |text: bool| {
        let (bytes, _) = assemble_dataset_with_opts(quads(), false, text, None, |_, _| Vec::new());
        Rete::open(&bytes).unwrap()
    };
    (build(false), build(true))
}

fn col(rete: &Rete, q: &str, var: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, &format!("{P}{q}")).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get(var).cloned()).collect();
    v.sort();
    v
}

fn ex(names: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = names.iter().map(|n| format!("<http://ex/{n}>")).collect();
    v.sort();
    v
}

#[test]
fn contains_inside_graph_sees_the_graphs_literals() {
    let (plain, indexed) = files();
    assert!(indexed.has_text_index() && !plain.has_text_index());
    let cases: [(&str, &[&str]); 6] = [
        (
            "SELECT ?s WHERE { GRAPH <http://ex/g> { ?s ex:label ?l FILTER(CONTAINS(?l, \"created\")) } }",
            &["n1"],
        ),
        // A word the default graph ALSO has: the wrong seed would be {d1}.
        (
            "SELECT ?s WHERE { GRAPH <http://ex/g> { ?s ex:label ?l FILTER(CONTAINS(LCASE(?l), \"observatory\")) } }",
            &["n2"],
        ),
        (
            "SELECT ?s WHERE { GRAPH ?g { ?s ex:label ?l FILTER(CONTAINS(?l, \"entity\")) } }",
            &["n1", "n3"],
        ),
        // A FILTER over a join inside the graph.
        (
            "SELECT ?s WHERE { GRAPH <http://ex/g> { ?s ex:label ?l ; ex:kind ?k FILTER(CONTAINS(?l, \"created\")) } }",
            &["n1"],
        ),
        // FROM <g> makes <g> the default graph.
        (
            "SELECT ?s FROM <http://ex/g> WHERE { ?s ex:label ?l FILTER(CONTAINS(?l, \"modified\")) }",
            &["n3"],
        ),
        // The default graph keeps using the index.
        (
            "SELECT ?s WHERE { ?s ex:label ?l FILTER(CONTAINS(LCASE(?l), \"observatory\")) }",
            &["d1"],
        ),
    ];
    for (q, want) in cases {
        assert_eq!(col(&plain, q, "s"), ex(want), "no text index: {q}");
        assert_eq!(col(&indexed, q, "s"), ex(want), "text index: {q}");
    }
}

#[test]
fn contains_over_the_union_default_graph() {
    let (plain, indexed) = files();
    let q = format!(
        "{P}SELECT ?s WHERE {{ ?s ex:label ?l FILTER(CONTAINS(LCASE(?l), \"observatory\")) }}"
    );
    let opts = QueryOpts {
        union_default_graph: true,
        ..Default::default()
    };
    for rete in [&plain, &indexed] {
        let QueryOutput::Select(_, rows) = eval_query_with(rete, &q, opts).unwrap() else {
            panic!("select");
        };
        let mut got: Vec<String> = rows.iter().map(|b| b["s"].clone()).collect();
        got.sort();
        assert_eq!(got, ex(&["d1", "n2"]));
    }
}

#[test]
fn subselect_inside_graph_reads_that_graph() {
    let (plain, indexed) = files();
    for rete in [&plain, &indexed] {
        // The sub-SELECT inherits the GRAPH block's active graph.
        assert_eq!(
            col(
                rete,
                "SELECT ?s WHERE { GRAPH <http://ex/g> { { SELECT DISTINCT ?s WHERE { ?s ex:label ?l } } } }",
                "s"
            ),
            ex(&["n1", "n2", "n3"])
        );
        // ... and the FROM merge.
        assert_eq!(
            col(
                rete,
                "SELECT ?s FROM <http://ex/g> WHERE { { SELECT ?s WHERE { ?s ex:kind ?k } } }",
                "s"
            ),
            ex(&["n1"])
        );
        // ... and GRAPH ?g, which binds ?g outside.
        let (_, sols) = eval_sparql(
            rete,
            &format!("{P}SELECT ?g ?s WHERE {{ GRAPH ?g {{ {{ SELECT ?s WHERE {{ ?s ex:kind ?k }} }} }} }}"),
        )
        .unwrap();
        assert_eq!(sols.len(), 1);
        assert_eq!(sols[0]["g"], "<http://ex/g>");
        assert_eq!(sols[0]["s"], "<http://ex/n1>");
        // A sub-SELECT outside any GRAPH still reads the default graph.
        assert_eq!(
            col(
                rete,
                "SELECT ?s WHERE { { SELECT ?s WHERE { ?s ex:label ?l } } }",
                "s"
            ),
            ex(&["d1", "d2"])
        );
    }
}

#[test]
fn describe_reads_the_active_graph() {
    let (_, rete) = files();
    let triples = |q: &str| match eval_query(&rete, q).unwrap() {
        QueryOutput::Construct(t) => t,
        other => panic!("describe: {other:?}"),
    };
    // n1 lives only in <g>: nothing in the default graph, two triples under FROM.
    assert!(triples("DESCRIBE <http://ex/n1>").is_empty());
    assert_eq!(
        triples("DESCRIBE <http://ex/n1> FROM <http://ex/g>").len(),
        2
    );
    assert_eq!(triples("DESCRIBE <http://ex/d1>").len(), 1);
}
