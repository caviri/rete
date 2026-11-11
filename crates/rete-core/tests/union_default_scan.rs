//! The union default graph on a `0x07` quad index is streamed: a pattern is
//! answered from SPOG / POSG / OSPG with the graph dropped, merged with the
//! file's own default graph, and nothing is built from every graph's triples.
//!
//! The oracle is the same quads with the graph dropped and stored in the
//! default graph of a plain file, which a standard query answers. Every query
//! must give the same rows on the streamed union, on the materializing merge a
//! per-graph file falls back to, on a lazy range-read open, and on four
//! threads.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::query::{eval_query_with, set_query_threads, sharded_queries};
use rete_core::v06::write::{convert_image, WriteOpts};
use rete_core::v06::{kind, open::read_front};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{
    eval_query_with_warnings, PermSet, PyramidAlgo, QueryOpts, QueryOutput, RangeReader, Rete,
    SliceReader,
};

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

const UNION: QueryOpts = QueryOpts {
    reason: false,
    union_default_graph: true,
};
const PLAIN: QueryOpts = QueryOpts {
    reason: false,
    union_default_graph: false,
};

/// Builds are serialised: the writer's tile size and the legacy switch are
/// process-wide.
static BUILD: Mutex<()> = Mutex::new(());

fn assemble(quads: Vec<RawQuad>, layout: GraphLayout, tile_budget: usize) -> Vec<u8> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let mut o = write_options();
    o.tile_budget = tile_budget;
    set_write_options(o);
    let bytes = assemble_dataset_with_layout(
        quads,
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0;
    bytes
}

/// The quads as a `0x07` quad-index image (graph-last orders).
fn quad07(quads: &[RawQuad], tile_budget: usize) -> Vec<u8> {
    let v5 = assemble(quads.to_vec(), GraphLayout::Quad, tile_budget);
    convert_image(&v5, WriteOpts::default()).unwrap()
}

/// The quads with a container per graph (the layout with no quad index: the
/// union is merged).
fn per_graph(quads: &[RawQuad], tile_budget: usize) -> Vec<u8> {
    assemble(quads.to_vec(), GraphLayout::PerGraph, tile_budget)
}

/// The oracle: the graph column dropped, every triple in the default graph.
fn flattened(quads: &[RawQuad], tile_budget: usize) -> Vec<u8> {
    let flat: Vec<RawQuad> = quads
        .iter()
        .map(|(s, p, o, _)| (s.clone(), p.clone(), o.clone(), None))
        .collect();
    assemble(flat, GraphLayout::PerGraph, tile_budget)
}

fn run(rete: &Rete, q: &str, opts: QueryOpts) -> Vec<String> {
    match eval_query_with(rete, q, opts).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(vars, rows) => std::iter::once(format!("{vars:?}"))
            .chain(rows.into_iter().map(|b| format!("{b:?}")))
            .collect(),
        QueryOutput::Ask(a) => vec![a.to_string()],
        QueryOutput::Construct(t) => t.into_iter().map(|t| format!("{t:?}")).collect(),
        other => panic!("unexpected output {other:?}"),
    }
}

/// Rows in the order a query with ORDER BY gives them, else sorted.
fn answer(rete: &Rete, q: &str, opts: QueryOpts) -> Vec<String> {
    let mut rows = run(rete, q, opts);
    if !q.contains("ORDER BY") {
        rows.sort();
    }
    rows
}

// ---------------------------------------------------------------------------
// A small dataset with known answers.

fn small() -> Vec<RawQuad> {
    let g = |n: &str| Some(iri(n));
    vec![
        // The default graph.
        (iri("a"), iri("p"), iri("b"), None),
        (iri("a"), iri("q"), iri("c"), None),
        // In the default graph and in two named graphs.
        (iri("a"), iri("p"), iri("b"), g("g1")),
        (iri("a"), iri("p"), iri("b"), g("g2")),
        // In two named graphs only.
        (iri("x"), iri("p"), iri("y"), g("g1")),
        (iri("x"), iri("p"), iri("y"), g("g2")),
        // In one.
        (iri("x"), iri("p"), iri("z"), g("g2")),
        (iri("m"), iri("r"), "\"lit\"".into(), g("g3")),
        (iri("m"), iri("r"), iri("a"), Some("_:bn".into())),
    ]
}

fn count(rete: &Rete, q: &str, opts: QueryOpts) -> String {
    match eval_query_with(rete, q, opts).unwrap() {
        QueryOutput::Select(_, rows) => {
            assert_eq!(rows.len(), 1, "{q}");
            let b = &rows[0];
            let v = b.iter().next().expect("one column").1.clone();
            // `"5"^^<...integer>`: the number.
            v.trim_matches('"').split('"').next().unwrap().to_string()
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_triple_in_several_graphs_counts_once() {
    let _serial = serial();
    let file = quad07(&small(), 4096);
    let rete = Rete::open(&file).unwrap();
    // a p b (default + g1 + g2), a q c, x p y (g1 + g2), x p z, m r "lit", m r a.
    assert_eq!(
        count(&rete, "SELECT (COUNT(*) AS ?n) { ?s ?p ?o }", UNION),
        "6"
    );
    assert_eq!(
        count(
            &rete,
            "SELECT (COUNT(*) AS ?n) { ?s <http://ex/p> ?o }",
            UNION
        ),
        "3"
    );
    assert_eq!(
        count(
            &rete,
            "SELECT (COUNT(DISTINCT ?s) AS ?n) { ?s ?p ?o }",
            UNION
        ),
        "3"
    );
    // The plain default graph is unchanged: two triples.
    assert_eq!(
        count(&rete, "SELECT (COUNT(*) AS ?n) { ?s ?p ?o }", PLAIN),
        "2"
    );
}

#[test]
fn the_default_graph_and_the_named_graphs_are_both_included() {
    let _serial = serial();
    let file = quad07(&small(), 4096);
    let rete = Rete::open(&file).unwrap();
    // `a q c` is only in the default graph, `m r "lit"` only in a named one.
    let rows = run(&rete, "SELECT ?o { <http://ex/a> <http://ex/q> ?o }", UNION);
    assert_eq!(rows.len(), 2, "{rows:?}");
    let rows = run(&rete, "SELECT ?o { <http://ex/m> <http://ex/r> ?o }", UNION);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(
        run(
            &rete,
            "ASK { <http://ex/x> <http://ex/p> <http://ex/z> }",
            UNION
        ),
        vec!["true"]
    );
    assert_eq!(
        run(
            &rete,
            "ASK { <http://ex/x> <http://ex/p> <http://ex/z> }",
            PLAIN
        ),
        vec!["false"]
    );
}

#[test]
fn from_still_wins() {
    let _serial = serial();
    let file = quad07(&small(), 4096);
    let rete = Rete::open(&file).unwrap();
    let q = "SELECT ?s ?o FROM <http://ex/g2> { ?s <http://ex/p> ?o }";
    assert_eq!(run(&rete, q, UNION), run(&rete, q, PLAIN));
    assert_eq!(run(&rete, q, UNION).len(), 1 + 3, "g2: a p b, x p y, x p z");
}

#[test]
fn graph_and_from_named_are_unaffected() {
    let _serial = serial();
    for file in [quad07(&small(), 4096), per_graph(&small(), 4096)] {
        let rete = Rete::open(&file).unwrap();
        for q in [
            "SELECT ?g ?s ?o { GRAPH ?g { ?s <http://ex/p> ?o } }",
            "SELECT ?s ?o { GRAPH <http://ex/g1> { ?s <http://ex/p> ?o } }",
            "SELECT ?g (COUNT(*) AS ?n) { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY ?g",
            "SELECT ?g ?s FROM NAMED <http://ex/g1> FROM NAMED <http://ex/g3> { GRAPH ?g { ?s ?p ?o } }",
            // One default-graph pattern next to GRAPH ?g.
            "SELECT ?s ?g { ?s <http://ex/q> ?c . GRAPH ?g { ?s <http://ex/p> ?o } }",
        ] {
            assert_eq!(answer(&rete, q, UNION), answer(&rete, q, PLAIN), "{q}");
        }
    }
}

#[test]
fn describe_and_construct_see_the_union() {
    let _serial = serial();
    let quads = small();
    let streamed = Rete::open(&quad07(&quads, 4096)).unwrap();
    let merged = Rete::open(&per_graph(&quads, 4096)).unwrap();
    let flat = Rete::open(&flattened(&quads, 4096)).unwrap();
    for q in [
        "DESCRIBE <http://ex/x>",
        "DESCRIBE <http://ex/a>",
        "CONSTRUCT { ?s <http://ex/seen> ?o } WHERE { ?s <http://ex/p> ?o }",
    ] {
        let want = answer(&flat, q, PLAIN);
        assert_eq!(answer(&streamed, q, UNION), want, "{q}");
        assert_eq!(answer(&merged, q, UNION), want, "{q}");
    }
}

#[test]
fn the_fallback_says_so() {
    let _serial = serial();
    let quads = small();
    let q = "SELECT * { ?s <http://ex/p> ?o }";
    let merged = Rete::open(&per_graph(&quads, 4096)).unwrap();
    let (_, w) = eval_query_with_warnings(&merged, q, UNION).unwrap();
    assert!(
        w.iter().any(|w| w.kind == "materialized-union"),
        "per-graph layout merges: {w:?}"
    );
    let streamed = Rete::open(&quad07(&quads, 4096)).unwrap();
    let (_, w) = eval_query_with_warnings(&streamed, q, UNION).unwrap();
    assert!(
        w.iter().all(|w| w.kind != "materialized-union"),
        "a quad index streams: {w:?}"
    );
    let (_, w) = eval_query_with_warnings(&merged, q, PLAIN).unwrap();
    assert!(w.is_empty());
}

// ---------------------------------------------------------------------------
// Random quads against the oracle.

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, n: u64) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 33) % n
    }
}

/// Random quads over few subjects, predicates and objects, so triples repeat
/// across graphs, with a default graph that overlaps the named ones.
fn random_quads(seed: u64) -> Vec<RawQuad> {
    let mut r = Lcg(seed);
    let graphs = 3 + r.next(40);
    let n = 150 + r.next(450);
    let mut q: Vec<RawQuad> = Vec::new();
    for _ in 0..n {
        let s = iri(&format!("s{}", r.next(14)));
        let p = iri(&format!("p{}", r.next(5)));
        let o = if r.next(4) == 0 {
            format!("\"v{}\"", r.next(9))
        } else {
            iri(&format!("s{}", r.next(14)))
        };
        let g = (r.next(9) != 0).then(|| iri(&format!("g{}", r.next(graphs))));
        q.push((s.clone(), p.clone(), o.clone(), g));
        // The same triple again elsewhere, often.
        if r.next(2) == 0 {
            q.push((s, p, o, Some(iri(&format!("g{}", r.next(graphs))))));
        }
    }
    q
}

fn terms(r: &mut Lcg, kind: char) -> String {
    match (kind, r.next(3)) {
        ('s', _) => iri(&format!("s{}", r.next(16))),
        ('p', _) => iri(&format!("p{}", r.next(6))),
        (_, 0) => format!("\"v{}\"", r.next(10)),
        _ => iri(&format!("s{}", r.next(16))),
    }
}

/// Every shape of one pattern, plus aggregates and joins over them.
fn queries(seed: u64) -> Vec<String> {
    let mut r = Lcg(seed ^ 0x9e37);
    let mut qs: Vec<String> = Vec::new();
    for mask in 0..8u32 {
        for _ in 0..2 {
            let pat = format!(
                "{} {} {}",
                if mask & 1 != 0 {
                    terms(&mut r, 's')
                } else {
                    "?s".into()
                },
                if mask & 2 != 0 {
                    terms(&mut r, 'p')
                } else {
                    "?p".into()
                },
                if mask & 4 != 0 {
                    terms(&mut r, 'o')
                } else {
                    "?o".into()
                },
            );
            qs.push(format!("SELECT * {{ {pat} }}"));
            qs.push(format!("SELECT (COUNT(*) AS ?n) {{ {pat} }}"));
            qs.push(format!("ASK {{ {pat} }}"));
        }
    }
    let (s1, p1, p2, o1) = (
        terms(&mut r, 's'),
        terms(&mut r, 'p'),
        terms(&mut r, 'p'),
        terms(&mut r, 'o'),
    );
    qs.extend([
        "SELECT (COUNT(*) AS ?n) { ?s ?p ?o }".to_string(),
        "SELECT (COUNT(DISTINCT ?s) AS ?n) (COUNT(DISTINCT ?o) AS ?m) { ?s ?p ?o }".to_string(),
        "SELECT ?p (COUNT(*) AS ?n) { ?s ?p ?o } GROUP BY ?p ORDER BY ?p".to_string(),
        "SELECT ?s (COUNT(*) AS ?n) { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) ?s".to_string(),
        "SELECT ?o (COUNT(DISTINCT ?s) AS ?n) { ?s ?p ?o } GROUP BY ?o ORDER BY ?o".to_string(),
        "SELECT DISTINCT ?p { ?s ?p ?o }".to_string(),
        "SELECT ?s ?p ?o { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 7".to_string(),
        "SELECT ?s ?p ?o { ?s ?p ?o } ORDER BY DESC(?o) ?s ?p LIMIT 5 OFFSET 3".to_string(),
        format!("SELECT ?s ?o ?z {{ ?s {p1} ?o . ?s {p2} ?z }}"),
        format!("SELECT ?s ?o ?z {{ ?s {p1} ?o . ?o {p2} ?z }}"),
        format!("SELECT ?s ?o {{ ?s {p1} ?o . ?s {p2} {o1} }}"),
        format!("SELECT ?s ?z {{ {s1} {p1} ?o . ?o ?q ?z }}"),
        format!("SELECT ?s ?o ?z {{ ?s {p1} ?o OPTIONAL {{ ?s {p2} ?z }} }}"),
        format!("SELECT ?s ?o {{ ?s {p1} ?o FILTER NOT EXISTS {{ ?s {p2} ?o }} }}"),
        format!("SELECT (COUNT(*) AS ?n) {{ ?s {p1} ?o . ?s {p2} ?z }}"),
        format!("SELECT ?p (COUNT(*) AS ?n) {{ ?s ?p ?o . ?s {p1} ?z }} GROUP BY ?p ORDER BY ?p"),
        format!("SELECT ?s ?n {{ {{ SELECT ?s (COUNT(*) AS ?n) {{ ?s ?p ?o }} GROUP BY ?s }} ?s {p1} ?x }}"),
        format!("SELECT ?s {{ {{ ?s {p1} ?o }} UNION {{ ?s {p2} ?o }} }}"),
        "SELECT ?s ?o { ?s <http://ex/p0>+ ?o }".to_string(),
        "SELECT ?s (COUNT(*) AS ?n) { ?s ?p ?o FILTER(isIRI(?o)) } GROUP BY ?s ORDER BY ?s".to_string(),
        format!("CONSTRUCT {{ ?s <http://ex/seen> ?o }} WHERE {{ ?s {p1} ?o }}"),
        format!("DESCRIBE {s1}"),
        // GRAPH ?g next to a default-graph pattern.
        format!("SELECT ?s ?g {{ ?s {p1} ?o . GRAPH ?g {{ ?s {p2} ?z }} }}"),
    ]);
    qs
}

/// A query on a plain default-graph file is the oracle for the union query,
/// except for what names a graph (checked against the same file's plain mode).
fn oracle_applies(q: &str) -> bool {
    !q.contains("GRAPH")
}

#[test]
fn the_streamed_union_equals_the_merge_for_random_quads() {
    let _serial = serial();
    for seed in 1..=14u64 {
        let quads = random_quads(seed);
        // Small tiles: every order is many tiles, a triple's quads straddle them.
        let budget = [128, 256, 1024][(seed % 3) as usize];
        let streamed = Rete::open(&quad07(&quads, budget)).unwrap();
        let merged = Rete::open(&per_graph(&quads, budget)).unwrap();
        let flat = Rete::open(&flattened(&quads, budget)).unwrap();
        for q in queries(seed) {
            let from_merge = answer(&merged, &q, UNION);
            let from_stream = answer(&streamed, &q, UNION);
            assert_eq!(from_stream, from_merge, "seed {seed}: {q}");
            if oracle_applies(&q) {
                assert_eq!(from_stream, answer(&flat, &q, PLAIN), "seed {seed}: {q}");
            }
            // Plain mode stays the standard one.
            assert_eq!(
                answer(&streamed, &q, PLAIN),
                answer(&merged, &q, PLAIN),
                "seed {seed} plain: {q}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Threads.

/// Many graphs, a hot triple in all of them (so its quads cross tile
/// boundaries), and a default graph that overlaps.
fn big() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..2400 {
        let g = Some(iri(&format!("g/{i}")));
        q.push((iri("hot"), iri("p"), iri("o"), g.clone()));
        q.push((
            iri(&format!("s/{}", i % 300)),
            iri("p"),
            iri(&format!("o/{}", i % 17)),
            g.clone(),
        ));
        q.push((
            iri(&format!("s/{}", (i * 7) % 300)),
            iri("q"),
            format!("\"{}\"", i % 23),
            g,
        ));
        if i % 40 == 0 {
            q.push((
                iri(&format!("s/{}", i % 300)),
                iri("p"),
                iri(&format!("o/{}", i % 17)),
                None,
            ));
            q.push((iri(&format!("d/{i}")), iri("p"), iri("o"), None));
        }
    }
    q
}

#[test]
fn four_threads_answer_as_one_does() {
    let _serial = serial();
    let quads = big();
    let streamed = Rete::open(&quad07(&quads, 512)).unwrap();
    let flat = Rete::open(&flattened(&quads, 512)).unwrap();
    let parallel = cfg!(feature = "parallel");
    let mut split_any = false;
    for q in [
        "SELECT ?p (COUNT(*) AS ?n) { ?s ?p ?o } GROUP BY ?p ORDER BY ?p",
        "SELECT ?o (COUNT(*) AS ?n) { ?s <http://ex/p> ?o } GROUP BY ?o ORDER BY ?o",
        "SELECT ?s (COUNT(*) AS ?n) { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) ?s LIMIT 20",
        "SELECT (COUNT(*) AS ?n) (COUNT(DISTINCT ?o) AS ?m) { ?s ?p ?o }",
        "SELECT ?v (COUNT(*) AS ?n) { ?s <http://ex/p> ?o . ?s <http://ex/q> ?v } GROUP BY ?v ORDER BY ?v",
        "SELECT ?s (COUNT(*) AS ?n) { ?s ?p <http://ex/o> } GROUP BY ?s ORDER BY ?s",
        "SELECT (COUNT(*) AS ?n) { <http://ex/hot> ?p ?o }",
    ] {
        set_query_threads(1);
        let before = sharded_queries();
        let one = answer(&streamed, q, UNION);
        assert_eq!(sharded_queries(), before, "one thread never splits: {q}");
        set_query_threads(4);
        let before = sharded_queries();
        let four = answer(&streamed, q, UNION);
        split_any |= sharded_queries() > before;
        assert_eq!(four, one, "{q}");
        set_query_threads(1);
        assert_eq!(one, answer(&flat, q, PLAIN), "{q}");
    }
    set_query_threads(0);
    if parallel {
        assert!(split_any, "no union query ran on several threads");
    }
}

// ---------------------------------------------------------------------------
// Bytes read.

struct Rec {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
}

impl RangeReader for Rec {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        let (s, e) = (offset as usize, (offset + len) as usize);
        self.data
            .get(s..e)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
    fn concurrency(&self) -> usize {
        4
    }
}

#[test]
fn a_selective_union_query_reads_a_few_tiles() {
    let _serial = serial();
    let mut quads: Vec<RawQuad> = Vec::new();
    for i in 0..20_000 {
        let g = Some(iri(&format!("g/{i}")));
        quads.push((
            iri(&format!("s/{i}")),
            iri("p"),
            iri(&format!("o/{}", i % 4000)),
            g.clone(),
        ));
        quads.push((
            iri(&format!("s/{i}")),
            iri("q"),
            format!("\"{}\"", i % 311),
            g.clone(),
        ));
        quads.push((iri("hot"), iri("r"), iri(&format!("o/{}", i % 50)), g));
    }
    let image = quad07(&quads, 4096);
    let (qd_start, quad_data) = {
        let (h, root) = read_front(&SliceReader::new(&image)).unwrap();
        let r = root.section(kind::QUAD_DATA).unwrap().abs(&h).unwrap();
        (r.offset, r.len)
    };
    let rec = Arc::new(Rec {
        data: image,
        reads: Mutex::new(Vec::new()),
    });
    let rete = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    // Rows, and the bytes of the quad tiles the query read (the dictionary and
    // the directories are other regions).
    let reads = |q: &str| -> (usize, u64) {
        rec.reads.lock().unwrap().clear();
        let rows = run(&rete, q, UNION).len() - 1;
        let tiles: u64 = rec
            .reads
            .lock()
            .unwrap()
            .iter()
            .map(|&(o, l)| {
                (o + l)
                    .min(qd_start + quad_data)
                    .saturating_sub(o.max(qd_start))
            })
            .sum();
        (rows, tiles)
    };
    // One subject, one predicate and object, one object: a tile or a few.
    for q in [
        "SELECT * { <http://ex/s/777> ?p ?o }",
        "SELECT * { ?s <http://ex/p> <http://ex/o/1234> }",
        "SELECT * { ?s ?p <http://ex/o/1234> }",
        "SELECT (COUNT(*) AS ?n) { <http://ex/s/12345> <http://ex/q> ?o }",
    ] {
        let (rows, bytes) = reads(q);
        assert!(rows >= 1, "{q}");
        assert!(
            bytes * 20 < quad_data,
            "{q}: read {bytes} B of a {quad_data} B quad index"
        );
    }
    // Same answers as the resident open of the same file.
    let resident = Rete::open(&rec.data).unwrap();
    for q in [
        "SELECT * { <http://ex/s/777> ?p ?o }",
        "SELECT ?s { ?s <http://ex/p> <http://ex/o/1234> } ORDER BY ?s",
        "SELECT (COUNT(*) AS ?n) { <http://ex/hot> <http://ex/r> ?o }",
        "SELECT (COUNT(DISTINCT ?o) AS ?n) { <http://ex/hot> ?p ?o }",
    ] {
        assert_eq!(answer(&rete, q, UNION), answer(&resident, q, UNION), "{q}");
    }
    // The hot triples are in 20 000 graphs and count once each.
    assert_eq!(
        count(
            &rete,
            "SELECT (COUNT(*) AS ?n) { <http://ex/hot> <http://ex/r> ?o }",
            UNION
        ),
        "50"
    );
    assert!(!rete.index_incomplete());
}

/// Tests share the process-wide thread count and sharded-query counter.
fn serial() -> std::sync::MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}
