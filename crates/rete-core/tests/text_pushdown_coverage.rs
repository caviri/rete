//! The `FILTER(CONTAINS(…))` pushdown on the shapes that used to scan (G18):
//! a variable predicate confined by the filter (`?p NOT IN (…)`, `?p IN (…)`,
//! `?p = <…>`, `?p != <…>`) on a predicate-scoped index (kind 11), and a
//! `UNION` with `GRAPH ?g` / `GRAPH <g>` on an index that covers the named
//! graphs (`--text-index-graphs all`, header bit 4).
//!
//! The rule is the one `text_index_scope.rs` states for every scope: every
//! query must return exactly the rows the same file without a text index
//! returns — here for both named-graph layouts — and the index may only make a
//! query faster. The second half checks that the index is actually used where
//! it covers the query, and left alone where it does not.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::{
    eval_query, Header, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader,
    TextIndexGraphs, TextIndexScope,
};

fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> RawQuad {
    (
        format!("<http://ex/{s}>"),
        format!("<http://ex/{p}>"),
        o.to_string(),
        g.map(|g| format!("<http://ex/{g}>")),
    )
}

/// Entities in the default graph, their provenance in per-entity graphs (the
/// OpenCitations shape), literals under predicates a scoped index leaves out,
/// an IRI object, a subject that appears in several graphs, a graph that holds
/// a candidate subject without a matching literal, and `fillers` graphs with
/// no match at all.
fn quads_with(fillers: usize) -> Vec<RawQuad> {
    let mut q = vec![
        quad("e1", "title", "\"Glucose metabolism\"", None),
        quad("e1", "desc", "\"The entity has been modified.\"", None),
        quad("e1", "note", "\"glucose again, in a note\"", None),
        quad("e2", "title", "\"Benzene ring\"", None),
        quad("e2", "query", "\"INSERT DATA { glucose }\"", None),
        quad("e3", "desc", "\"modified twice\"", None),
        quad("e3", "kind", "<http://ex/Thing>", None),
        quad("e3", "title", "\"A modified benzene\"@en", None),
        quad("p1", "desc", "\"The entity has been created.\"", Some("g1")),
        quad("p1", "note", "\"created by an agent\"", Some("g1")),
        quad("p1", "title", "\"Glucose provenance\"", Some("g1")),
        quad(
            "p2",
            "desc",
            "\"The entity has been modified.\"",
            Some("g2"),
        ),
        quad("p2", "query", "\"DELETE { benzene }\"", Some("g2")),
        // p1 again in another graph, with a literal that matches nothing.
        quad("p1", "note", "\"unrelated\"", Some("g3")),
        // e1 (a default-graph subject) inside a named graph.
        quad("e1", "note", "\"modified in a graph\"", Some("g4")),
    ];
    // Filler graphs with high-entropy names, so that the names alone span
    // many dictionary chunks (a walk over every graph reads each one's name).
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    for k in 0..fillers {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        q.push(quad(
            &format!("x{k}"),
            "note",
            "\"filler text\"",
            Some(&format!(
                "filler-graph/{k}/{seed:016x}{:016x}",
                seed.rotate_left(29)
            )),
        ));
    }
    q
}

fn build(layout: GraphLayout, scope: Option<TextIndexScope>) -> Vec<u8> {
    build_with(300, layout, scope)
}

fn build_with(fillers: usize, layout: GraphLayout, scope: Option<TextIndexScope>) -> Vec<u8> {
    assemble_dataset_with_layout(
        quads_with(fillers),
        false,
        scope,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    let q = format!("PREFIX ex: <http://ex/> {q}");
    match eval_query(rete, &q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn scope(graphs: TextIndexGraphs, include: &[&str], exclude: &[&str]) -> TextIndexScope {
    TextIndexScope::from_options(graphs, include.iter(), exclude.iter()).unwrap()
}

/// The scopes under test, by name.
fn scopes() -> Vec<(&'static str, TextIndexScope)> {
    use TextIndexGraphs::{All, Default};
    vec![
        ("default", TextIndexScope::default()),
        ("all graphs", scope(All, &[], &[])),
        (
            "exclude desc+query",
            scope(Default, &[], &["http://ex/desc", "http://ex/query"]),
        ),
        (
            "exclude desc+query, all graphs",
            scope(All, &[], &["http://ex/desc", "http://ex/query"]),
        ),
        (
            "include title+note, all graphs",
            scope(All, &["http://ex/title", "http://ex/note"], &[]),
        ),
    ]
}

const NOT_IN: &str = "FILTER(?p NOT IN (ex:desc, ex:query))";

/// Each query, with the scopes under which the pushdown must take it (by
/// name). The rows are checked under every scope.
fn queries() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for w in [
        "glucose", "modified", "benzene", "created", "Glucose", "agent",
    ] {
        let c = format!(
            "FILTER(isLiteral(?o)) FILTER(CONTAINS(LCASE(STR(?o)), \"{}\"))",
            w.to_lowercase()
        );
        let d = format!("FILTER(CONTAINS(?o, \"{w}\"))");
        // A variable predicate confined by the filter, every spelling.
        v.push(format!("SELECT ?s ?o WHERE {{ ?s ?p ?o {c} {NOT_IN} }}"));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o FILTER(CONTAINS(?o, \"{w}\") && ?p != ex:desc && ?p != ex:query) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(?p IN (ex:title, ex:note)) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(?p = ex:title) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(sameTerm(?p, ex:note)) }}"
        ));
        // ... and filters that do NOT confine it: the index must not prune.
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(?p NOT IN (ex:desc)) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(?p NOT IN (ex:desc, ex:query) || ?o != \"x\") }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ ?s ?p ?o {d} FILTER(?p IN (ex:title, ex:desc)) }}"
        ));
        v.push(format!("SELECT ?s ?o WHERE {{ ?s ?p ?o {d} }}"));
        // The default graph and every named graph, in one UNION.
        v.push(format!(
            "SELECT ?s ?o WHERE {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} {c} }}"
        ));
        v.push(format!(
            "SELECT ?s ?o ?g WHERE {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} {d} {NOT_IN} }}"
        ));
        v.push(format!(
            "SELECT ?s ?g WHERE {{ {{ ?s ex:title ?o }} UNION {{ GRAPH ?g {{ ?s ex:note ?o }} }} {d} }}"
        ));
        // GRAPH ?g / GRAPH <g> with the filter outside, and the filter using ?g.
        v.push(format!(
            "SELECT ?s ?o ?g WHERE {{ GRAPH ?g {{ ?s ?p ?o }} {d} }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ GRAPH ?g {{ ?s ?p ?o }} {d} FILTER(?g != ex:g2) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ GRAPH ex:g1 {{ ?s ?p ?o }} {d} }}"
        ));
        v.push(format!(
            "SELECT ?s ?o WHERE {{ {{ ?s ?p ?o }} UNION {{ GRAPH ex:g2 {{ ?s ?p ?o }} }} {d} }}"
        ));
        // EXISTS reads the active graph: GRAPH <g> must keep the filter outside.
        v.push(format!(
            "SELECT ?s ?o WHERE {{ GRAPH ex:g1 {{ ?s ?p ?o }} {d} FILTER(NOT EXISTS {{ ?s ex:title ?t }}) }}"
        ));
        v.push(format!(
            "SELECT ?s ?o ?g WHERE {{ GRAPH ?g {{ ?s ?p ?o }} {d} FILTER(EXISTS {{ ?s ?q ?z }}) }}"
        ));
        // FROM NAMED limits the graphs GRAPH may see.
        v.push(format!(
            "SELECT ?s ?o ?g FROM NAMED ex:g2 FROM NAMED ex:g4 WHERE {{ GRAPH ?g {{ ?s ?p ?o }} {d} }}"
        ));
        v.push(format!(
            "SELECT ?s ?o FROM NAMED ex:g2 WHERE {{ GRAPH ex:g1 {{ ?s ?p ?o }} {d} }}"
        ));
        // Joined, OPTIONAL and LIMITed forms around the union.
        v.push(format!(
            "SELECT ?s ?o ?t WHERE {{ {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} }} OPTIONAL {{ ?s ex:title ?t }} {d} }}"
        ));
        v.push(format!(
            "ASK {{ {{ ?s ?p ?o }} UNION {{ GRAPH ?g {{ ?s ?p ?o }} }} {d} }}"
        ));
    }
    v
}

#[test]
fn every_scope_and_layout_answers_like_a_file_without_an_index() {
    let queries = queries();
    for layout in [GraphLayout::PerGraph, GraphLayout::Quad] {
        let plain = build(layout, None);
        let plain = Rete::open(&plain).unwrap();
        let expected: Vec<Vec<String>> = queries.iter().map(|q| rows(&plain, q)).collect();
        assert!(
            expected.iter().filter(|r| !r.is_empty()).count() > queries.len() / 2,
            "the fixture should answer most queries"
        );
        for (name, scope) in scopes() {
            let bytes = build(layout, Some(scope));
            let resident = Rete::open(&bytes).unwrap();
            let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes.clone()), rete_core::HEADER_LEN as u64).unwrap();
            for (q, want) in queries.iter().zip(&expected) {
                assert_eq!(
                    &rows(&resident, q),
                    want,
                    "{layout:?}, scope {name}, resident: indexed and plain diverge for {q}"
                );
                assert_eq!(
                    &rows(&lazy, q),
                    want,
                    "{layout:?}, scope {name}, lazy: indexed and plain diverge for {q}"
                );
            }
            assert!(!lazy.index_incomplete());
        }
    }
}

// ---------------------------------------------------------------------------
// Is the index used? A lazy open over a recording reader: a pushdown reads
// posting lists, which a declined one never touches (the token table is read
// either way, to learn the scope).
// ---------------------------------------------------------------------------

/// Records every range a lazy open reads.
#[derive(Clone)]
struct Recorded {
    data: Arc<Vec<u8>>,
    reads: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl Recorded {
    fn new(data: Vec<u8>) -> Self {
        Recorded {
            data: Arc::new(data),
            reads: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn take(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl RangeReader for Recorded {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        SliceReader::new(&self.data).read_at(offset, len)
    }
}

/// The byte range of the text index's postings blob (after the token table).
fn postings(bytes: &[u8]) -> (u64, u64) {
    let h = Header::from_bytes(bytes).unwrap();
    let (off, len) = h.text_index_payload();
    let tt = Rete::open(bytes)
        .unwrap()
        .text_index_token_table_len()
        .unwrap();
    (off + tt, len - tt)
}

/// Run `q` on a fresh lazy open; did it read a posting list?
fn reads_postings(bytes: &[u8], q: &str) -> (Vec<String>, bool) {
    let rec = Recorded::new(bytes.to_vec());
    let rete = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();
    let got = rows(&rete, q);
    let (off, len) = postings(bytes);
    let touched = rec
        .take()
        .iter()
        .any(|&(o, l)| o < off + len && off < o + l);
    (got, touched)
}

#[test]
fn a_filter_that_confines_the_predicate_uses_a_scoped_index() {
    use TextIndexGraphs::Default;
    let bytes = build(
        GraphLayout::Quad,
        Some(scope(Default, &[], &["http://ex/desc", "http://ex/query"])),
    );
    let plain = Rete::open(&build(GraphLayout::Quad, None)).unwrap();
    for (q, uses) in [
        // Every excluded predicate ruled out: covered.
        (format!("SELECT ?s ?o WHERE {{ ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\")) {NOT_IN} }}"), true),
        (
            "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\") && ?p != ex:desc && ?p != ex:query) }".to_string(),
            true,
        ),
        (
            "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\")) FILTER(?p IN (ex:title, ex:note)) }".to_string(),
            true,
        ),
        // One excluded predicate still possible: not covered.
        (
            "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\")) FILTER(?p NOT IN (ex:desc)) }".to_string(),
            false,
        ),
        (
            "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\")) FILTER(?p IN (ex:title, ex:query)) }".to_string(),
            false,
        ),
        // A disjunction proves nothing.
        (
            "SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\") && (?p != ex:desc || ?p != ex:query)) }".to_string(),
            false,
        ),
        ("SELECT ?s ?o WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"Glucose\")) }".to_string(), false),
    ] {
        let (got, touched) = reads_postings(&bytes, &q);
        assert_eq!(got, rows(&plain, &q), "{q}");
        assert!(!got.is_empty(), "{q}");
        assert_eq!(touched, uses, "pushdown used = {uses}: {q}");
    }
}

#[test]
fn a_union_over_every_graph_uses_an_all_graphs_index() {
    use TextIndexGraphs::{All, Default};
    // "modified" is in default-graph and named-graph literals alike, so even a
    // default-scope index has a posting for it; "created" only in g1.
    let union =
        "SELECT ?s ?o WHERE { { ?s ?p ?o } UNION { GRAPH ?g { ?s ?p ?o } } FILTER(isLiteral(?o)) FILTER(CONTAINS(LCASE(STR(?o)), \"modified\")) }";
    let graph_iri =
        "SELECT ?s ?o WHERE { GRAPH ex:g1 { ?s ?p ?o } FILTER(CONTAINS(?o, \"created\")) }";
    for layout in [GraphLayout::Quad, GraphLayout::PerGraph] {
        let plain = Rete::open(&build(layout, None)).unwrap();
        for (idx, all_graphs) in [
            (scope(All, &[], &[]), true),
            (scope(Default, &[], &[]), false),
        ] {
            let bytes = build(layout, Some(idx));
            for q in [union, graph_iri] {
                let (got, touched) = reads_postings(&bytes, q);
                assert_eq!(got, rows(&plain, q), "{layout:?} all={all_graphs}: {q}");
                assert!(!got.is_empty(), "{q}");
                // The default-graph branch of the UNION takes any index; the
                // named graphs need one that covers them.
                let uses = q == union || all_graphs;
                assert_eq!(touched, uses, "{layout:?} all={all_graphs}: {q}");
            }
        }
    }
}

/// On a quad-index file, `GRAPH ?g` through an all-graphs index visits only the
/// graphs holding a candidate subject: one pass over the graph-leading SPO
/// tiles, not a view of every graph — and not the name of every graph, which
/// is what the whole walk reads the dictionary for.
#[test]
fn graph_var_walk_visits_only_graphs_with_candidates() {
    let idx = scope(TextIndexGraphs::All, &[], &[]);
    let bytes = build_with(20_000, GraphLayout::Quad, Some(idx));
    let h = Header::from_bytes(&bytes).unwrap();
    let (d0, d1) = (h.dictionary_offset, h.dictionary_offset + h.dictionary_len);
    let plain = Rete::open(&build_with(20_000, GraphLayout::Quad, None)).unwrap();
    let q = "SELECT ?s ?o ?g WHERE { GRAPH ?g { ?s ?p ?o } FILTER(CONTAINS(?o, \"created\")) }";
    let walk = "SELECT ?s ?o ?g WHERE { GRAPH ?g { ?s ?p ?o } FILTER(STRSTARTS(?o, \"created\")) }";
    // Dictionary bytes a fresh lazy open reads to answer `query`.
    let dict_bytes = |query: &str| {
        let rec = Recorded::new(bytes.clone());
        let rete = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
        rec.take();
        let got = rows(&rete, query);
        assert!(!rete.index_incomplete());
        let read: u64 = rec
            .take()
            .iter()
            .map(|&(o, l)| (o + l).min(d1).saturating_sub(o.max(d0)))
            .sum();
        (got, read)
    };
    let (got, pushed) = dict_bytes(q);
    assert_eq!(got, rows(&plain, q));
    assert_eq!(got.len(), 2, "p1's desc and note in g1: {got:?}");
    // A filter the index cannot take walks every graph.
    let (whole, walked) = dict_bytes(walk);
    assert_eq!(whole, rows(&plain, walk));
    assert_eq!(whole.len(), 1);
    assert!(
        pushed * 4 < walked,
        "the pushdown read {pushed} dictionary bytes, the whole walk {walked}"
    );
}
