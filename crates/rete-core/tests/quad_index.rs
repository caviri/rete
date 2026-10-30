//! The quad index (SPEC.md §6.5, `GraphLayout::Quad`) against the per-graph
//! layout it replaces: same quads in, the same answers out — through every open
//! path, every graph-scoped API and SPARQL — with a fraction of the bytes, a
//! directory instead of a walk, and an old reader refusing the file.

use std::collections::BTreeSet;

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::{eval_query, CountingReader, PermSet, PyramidAlgo, QueryOutput, Rete, SliceReader};

const EX: &str = "http://ex/";

fn iri(s: &str) -> String {
    format!("<{EX}{s}>")
}

/// A dataset shaped like OpenCitations provenance: a default graph of entities,
/// many small per-entity graphs, one large graph that spans tiles, a graph
/// named by a blank node, and a graph whose name is also a default-graph term.
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for e in 0..400 {
        let ent = iri(&format!("br/{e}"));
        q.push((ent.clone(), iri("title"), format!("\"work {e}\""), None));
        q.push((
            ent.clone(),
            iri("cites"),
            iri(&format!("br/{}", (e * 7) % 400)),
            None,
        ));
        let g = iri(&format!("br/{e}/prov/"));
        for s in 0..(1 + e % 3) {
            let snap = iri(&format!("br/{e}/prov/se/{s}"));
            q.push((
                snap.clone(),
                iri("specializationOf"),
                ent.clone(),
                Some(g.clone()),
            ));
            q.push((
                snap.clone(),
                iri("generatedAtTime"),
                format!("\"2020-01-{:02}\"", 1 + (e + s) % 28),
                Some(g.clone()),
            ));
            if s > 0 {
                q.push((
                    snap.clone(),
                    iri("wasDerivedFrom"),
                    iri(&format!("br/{e}/prov/se/{}", s - 1)),
                    Some(g.clone()),
                ));
            }
        }
    }
    // A large graph: far over one tile, with a mega-group (one predicate).
    for k in 0..30_000 {
        q.push((
            iri(&format!("big/{k}")),
            iri("p"),
            format!("\"v{}\"", k % 977),
            Some(iri("big")),
        ));
    }
    // A blank-node graph name and a graph that is also a subject/object.
    q.push((iri("a"), iri("p"), iri("b"), Some("_:g1".into())));
    q.push((iri("meta"), iri("describes"), iri("br/3/prov/"), None));
    q.push((iri("br/3/prov/"), iri("generatedBy"), iri("tool"), None));
    // A duplicate, which both layouts must drop.
    q.push((iri("a"), iri("p"), iri("b"), Some("_:g1".into())));
    q
}

fn build(layout: GraphLayout, perms: PermSet) -> Vec<u8> {
    assemble_dataset_with_layout(
        dataset(),
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        perms,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn sorted<T: Ord>(v: impl IntoIterator<Item = T>) -> Vec<T> {
    let mut v: Vec<T> = v.into_iter().collect();
    v.sort();
    v
}

fn select(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => sorted(rows.into_iter().map(|b| format!("{b:?}"))),
        QueryOutput::Ask(a) => vec![a.to_string()],
        QueryOutput::Construct(t) => sorted(t.into_iter().map(|t| format!("{t:?}"))),
        other => panic!("unexpected output {other:?}"),
    }
}

const QUERIES: &[&str] = &[
    // GRAPH <g>: the c08 shape.
    "SELECT ?s ?t WHERE { GRAPH <http://ex/br/7/prov/> { ?s <http://ex/specializationOf> <http://ex/br/7> ; <http://ex/generatedAtTime> ?t } }",
    "SELECT ?s ?p ?o WHERE { GRAPH <http://ex/br/399/prov/> { ?s ?p ?o } }",
    // A graph that does not exist, and one whose name is a default-graph term.
    "SELECT * WHERE { GRAPH <http://ex/nope/> { ?s ?p ?o } }",
    "SELECT * WHERE { GRAPH <http://ex/br/3/prov/> { ?s ?p ?o } }",
    "SELECT * WHERE { GRAPH <http://ex/meta> { ?s ?p ?o } }",
    // GRAPH ?g: every graph, bound inner patterns, aggregates.
    "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g",
    "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/wasDerivedFrom> ?prev } }",
    "SELECT ?g WHERE { GRAPH ?g { <http://ex/big/17> ?p ?o } }",
    "SELECT ?g ?o WHERE { GRAPH ?g { ?s <http://ex/p> ?o FILTER(?o = \"v5\") } } LIMIT 5",
    // A join between the default graph and a named one (c10's shape).
    "SELECT ?e ?t WHERE { ?e <http://ex/cites> <http://ex/br/14> . GRAPH ?g { ?s <http://ex/specializationOf> ?e ; <http://ex/generatedAtTime> ?t } }",
    // FROM / FROM NAMED.
    "SELECT ?s ?p ?o FROM <http://ex/br/5/prov/> WHERE { ?s ?p ?o }",
    "SELECT ?s ?p ?o FROM <http://ex/br/5/prov/> FROM <http://ex/br/6/prov/> WHERE { ?s ?p ?o }",
    "SELECT ?g ?s FROM NAMED <http://ex/br/1/prov/> FROM NAMED <http://ex/big> WHERE { GRAPH ?g { ?s <http://ex/specializationOf> ?e } }",
    "ASK { GRAPH <http://ex/big> { <http://ex/big/29999> <http://ex/p> ?o } }",
    // Blank-node graph name.
    "SELECT ?g ?o WHERE { GRAPH ?g { <http://ex/a> <http://ex/p> ?o } }",
];

#[test]
fn quad_layout_answers_exactly_like_the_per_graph_layout() {
    for perms in [PermSet::ALL, PermSet::CORE] {
        let legacy_bytes = build(GraphLayout::PerGraph, perms);
        let quad_bytes = build(GraphLayout::Quad, perms);
        let legacy = Rete::open(&legacy_bytes).unwrap();
        let quad = Rete::open(&quad_bytes).unwrap();

        let h = quad.header();
        assert!(h.has_quad_index() && h.has_quads());
        assert_eq!(h.named_graphs_len, 0, "no kind-5 section");
        assert!(h.quad_index_section().is_some(), "a kind-10 section");
        assert_eq!(h.quad_count, legacy.header().quad_count);
        assert_eq!(quad.named_graph_count(), legacy.named_graph_count());
        assert!(rete_core::verify(&quad_bytes).unwrap());

        // Same graphs, same quads, graph by graph and all together.
        let names: BTreeSet<&str> = quad.graph_names().into_iter().collect();
        let legacy_names: BTreeSet<&str> = legacy.graph_names().into_iter().collect();
        assert_eq!(names, legacy_names);
        for g in &names {
            assert_eq!(
                sorted(quad.dump(Some(g))),
                sorted(legacy.dump(Some(g))),
                "graph {g}"
            );
            assert_eq!(
                sorted(quad.query_in_graph(Some(g), None, Some(&iri("p")), None)),
                sorted(legacy.query_in_graph(Some(g), None, Some(&iri("p")), None)),
            );
        }
        assert_eq!(
            sorted(quad.query_quads(None, None, None)),
            sorted(legacy.query_quads(None, None, None))
        );
        assert!(quad.graph_index(&iri("nope/")).is_none());
        assert!(
            quad.graph_index(&iri("meta")).is_none(),
            "a term, not a graph"
        );

        for q in QUERIES {
            assert_eq!(select(&quad, q), select(&legacy, q), "{q}");
        }
        assert!(!quad.index_incomplete());
    }
}

#[test]
fn the_union_default_graph_merges_every_view() {
    let opts = rete_core::QueryOpts {
        union_default_graph: true,
        ..Default::default()
    };
    let legacy_bytes = build(GraphLayout::PerGraph, PermSet::ALL);
    let quad_bytes = build(GraphLayout::Quad, PermSet::ALL);
    let legacy = Rete::open(&legacy_bytes).unwrap();
    let quad = Rete::open(&quad_bytes).unwrap();
    for q in [
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        "SELECT ?s WHERE { ?s <http://ex/specializationOf> <http://ex/br/9> }",
        "SELECT ?o WHERE { <http://ex/a> <http://ex/p> ?o }",
    ] {
        let run = |r: &Rete| match rete_core::eval_query_with(r, q, opts).unwrap() {
            QueryOutput::Select(_, rows) => sorted(rows.into_iter().map(|b| format!("{b:?}"))),
            other => panic!("unexpected {other:?}"),
        };
        assert_eq!(run(&quad), run(&legacy), "{q}");
    }
}

#[test]
fn the_quad_index_is_far_smaller_than_one_container_per_graph() {
    // The small provenance graphs only: that is where a container per graph
    // costs ~600 B of framing, frames and directories (a large graph costs
    // about the same either way).
    let small: Vec<RawQuad> = dataset()
        .into_iter()
        .filter(|q| q.3.as_deref() != Some(iri("big").as_str()))
        .collect();
    let mk = |layout| {
        assemble_dataset_with_layout(
            small.clone(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            PermSet::ALL,
            layout,
            |_, _| Vec::<u8>::new(),
        )
        .0
    };
    let (legacy, quad) = (mk(GraphLayout::PerGraph), mk(GraphLayout::Quad));
    let lh = Rete::open(&legacy).unwrap().header().clone();
    let qh = Rete::open(&quad).unwrap().header().clone();
    let q = qh.quad_index_section().unwrap();
    // Most of the per-graph overhead is one zstd frame per tiny tile; without
    // compression (`--no-default-features`) the gap is smaller.
    let factor = if cfg!(feature = "compression") { 5 } else { 1 };
    assert!(
        q.length * factor < lh.named_graphs_len,
        "quad index {} B vs per-graph {} B",
        q.length,
        lh.named_graphs_len
    );
    let file_factor = if cfg!(feature = "compression") { 2 } else { 1 };
    assert!(
        quad.len() * file_factor < legacy.len(),
        "{} vs {}",
        quad.len(),
        legacy.len()
    );
}

#[test]
fn every_open_path_reads_the_quad_index() {
    let bytes = build(GraphLayout::Quad, PermSet::ALL);
    let eager = Rete::open(&bytes).unwrap();
    let ranged = Rete::open_ranged(&SliceReader::new(&bytes)).unwrap();
    let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
    let lazy = Rete::open_ranged_lazy(SliceReader::new(leaked)).unwrap();
    for r in [&ranged, &lazy] {
        assert_eq!(r.named_graph_count(), eager.named_graph_count());
        for q in QUERIES {
            assert_eq!(select(r, q), select(&eager, q), "{q}");
        }
        assert!(!r.index_incomplete());
    }
    assert!(rete_core::verify_ranged(&SliceReader::new(&bytes)).unwrap());
}

#[test]
fn one_graph_is_found_without_walking_the_section() {
    let bytes = build(GraphLayout::Quad, PermSet::ALL);
    let section = Rete::open(&bytes)
        .unwrap()
        .header()
        .quad_index_section()
        .unwrap();
    let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
    let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
    let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let before = reader.bytes_read();
    let rows = select(
        &lazy,
        "SELECT ?s ?t WHERE { GRAPH <http://ex/br/398/prov/> { ?s <http://ex/generatedAtTime> ?t } }",
    );
    assert_eq!(rows.len(), 3);
    let pulled = reader.bytes_read() - before;
    // The graph sorts near the end of the section, which the per-graph layout
    // would have walked up to.
    assert!(
        pulled < section.length / 2,
        "GRAPH <g> pulled {pulled} B of a {} B quad section",
        section.length
    );
    assert!(!lazy.index_incomplete());
}

#[test]
fn tampering_with_the_quad_index_fails_verify() {
    let bytes = build(GraphLayout::Quad, PermSet::ALL);
    let q = Rete::open(&bytes)
        .unwrap()
        .header()
        .quad_index_section()
        .unwrap();
    let mut bad = bytes.clone();
    bad[(q.offset + q.length / 2) as usize] ^= 0x01;
    assert!(!rete_core::verify(&bad).unwrap());
    assert!(!rete_core::verify_ranged(&SliceReader::new(&bad)).unwrap());
}

#[test]
fn inconsistent_headers_are_refused() {
    let bytes = build(GraphLayout::Quad, PermSet::ALL);
    // Clear the flag: the kind-10 section is then unannounced.
    let mut no_flag = bytes.clone();
    no_flag[5] &= !rete_core::header::FLAG_QUAD_INDEX;
    assert!(Rete::open(&no_flag).is_err());
    // Set the flag on a per-graph file: no kind-10 section to honour it.
    let mut legacy = build(GraphLayout::PerGraph, PermSet::ALL);
    legacy[5] |= rete_core::header::FLAG_QUAD_INDEX;
    assert!(Rete::open(&legacy).is_err());
    let leaked: &'static [u8] = Box::leak(legacy.into_boxed_slice());
    assert!(Rete::open_ranged_lazy(SliceReader::new(leaked)).is_err());
}

#[test]
fn a_dump_batch_walk_of_a_view_ends_quickly() {
    // A view's tile shared with other graphs has the leading range
    // 0..=u32::MAX; the batched dump must jump over absent ids, not step.
    let bytes = build(GraphLayout::Quad, PermSet::ALL);
    let rete = Rete::open(&bytes).unwrap();
    let g = iri("br/10/prov/");
    let mut cursor = 0;
    let mut got = Vec::new();
    for calls in 0.. {
        assert!(calls < 20, "dump_batch did not finish");
        let (rows, next, done) = rete.dump_batch(Some(&g), cursor, 2);
        got.extend(rows);
        cursor = next;
        if done {
            break;
        }
    }
    assert_eq!(sorted(got), sorted(rete.dump(Some(&g))));
}

#[test]
fn no_named_graphs_is_byte_identical_whatever_the_layout() {
    let triples: Vec<RawQuad> = dataset().into_iter().filter(|q| q.3.is_none()).collect();
    let mk = |layout| {
        assemble_dataset_with_layout(
            triples.clone(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            PermSet::ALL,
            layout,
            |_, _| Vec::<u8>::new(),
        )
        .0
    };
    assert_eq!(mk(GraphLayout::Quad), mk(GraphLayout::PerGraph));
}
