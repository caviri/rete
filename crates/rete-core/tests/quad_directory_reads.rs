//! G4: the first touch of one named graph in a big `0x06` quad index reads the
//! tile directory of the order the query routes to, not of all six.
//!
//! An `0x06` order's quad tile directory is one compressed leaf (about 0.9 MB
//! on the 43 M-graph nanopub dump); a view of one graph used to read every
//! order's leaf, one after the other, before it touched a tile: 5.2 MB in six
//! dependent round trips for a lookup that needs one order. The fixture is the
//! nanopub shape of `graph_variable_scale.rs`, converted to `0x06`, read through
//! a recording reader, and every answer is checked against the resident open.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::{kind, open::read_front, write::convert_image, write::WriteOpts, VERSION_06};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

const N: usize = 80_000;
const NP: &str = "http://ex/np#";

fn iri(s: &str) -> String {
    format!("<{s}>")
}

fn fixture() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..N {
        let np = iri(&format!("http://ex/np/{i}"));
        let head = Some(iri(&format!("http://ex/np/{i}#Head")));
        let assertion = iri(&format!("http://ex/np/{i}#assertion"));
        q.push((
            np.clone(),
            iri(&format!("{NP}hasAssertion")),
            assertion.clone(),
            head.clone(),
        ));
        q.push((
            np.clone(),
            iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            iri(&format!("{NP}Nanopublication")),
            head,
        ));
        let ga = Some(assertion.clone());
        for k in 0..3 {
            q.push((
                iri(&format!("http://ex/e/{}", (i * 7) % 500)),
                iri(&format!("http://ex/rel{k}")),
                iri(&format!("http://ex/o/{}", (i * 13 + k) % 997)),
                ga.clone(),
            ));
        }
    }
    q
}

/// The fixture as a `0x06` image with all six orders.
fn image() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let mut o = write_options();
        o.tile_budget = 4 * 1024;
        set_write_options(o);
        // The legacy image keeps all six graph-leading orders (an in-RAM build for
        // `0x07` carries GSPO alone), which is what this file reads.
        std::env::set_var("RETE_WRITE_FORMAT", "5");
        let v5 = assemble_dataset_with_layout(
            fixture(),
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
        // This file tests the `0x06` directories (six graph-leading orders); the
        // `0x07` ones are in `quad_orders_0x07.rs`.
        let o = WriteOpts {
            version: VERSION_06,
            ..Default::default()
        };
        convert_image(&v5, o).unwrap()
    })
}

/// Records every range and every `read_many` batch.
struct Rec {
    data: &'static [u8],
    reads: Mutex<Vec<(u64, u64)>>,
    fail: AtomicBool,
}

impl Rec {
    fn new() -> Arc<Self> {
        Arc::new(Rec {
            data: image(),
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }
    fn take(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl RangeReader for Rec {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("simulated network failure"));
        }
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

/// `[start, end)` of section `k` of the image.
fn region(k: u8) -> (u64, u64) {
    let (h, root) = read_front(&SliceReader::new(image())).unwrap();
    let r = root.section(k).expect("section present").abs(&h).unwrap();
    (r.offset, r.offset + r.len)
}

fn inside(reads: &[(u64, u64)], region: (u64, u64)) -> u64 {
    reads
        .iter()
        .map(|&(o, l)| (o + l).min(region.1).saturating_sub(o.max(region.0)))
        .sum()
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        other => panic!("unexpected output {other:?}"),
    }
}

const G: &str = "<http://ex/np/12345#assertion>";

/// Leaves per order in the fixture's quad root.
fn leaves_per_order() -> Vec<usize> {
    let (_, root) = read_front(&SliceReader::new(image())).unwrap();
    let (_, payload) = root
        .units
        .iter()
        .find(|(t, _)| *t == rete_core::v06::tag::QUAD)
        .expect("a quad unit");
    rete_core::v06::decode_quad_root(payload)
        .unwrap()
        .orders
        .iter()
        .map(|o| o.leaves.len())
        .collect()
}

/// The fixture must hold several leaves per order, or the tests below prove
/// nothing about choosing among them.
#[test]
fn the_fixture_has_several_leaves_per_order() {
    let n = leaves_per_order();
    assert_eq!(n.len(), 6);
    assert!(n.iter().all(|&l| l >= 2), "leaves per order: {n:?}");
}

/// The first touch of one graph reads the graph directory block and one
/// order's tile directory, not all six orders'.
#[test]
fn one_graph_reads_one_orders_directory() {
    let leaves = region(kind::QUAD_LEAVES);
    let rec = Rec::new();
    let rete = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.take();
    let got = rows(
        &rete,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}"),
    );
    assert_eq!(got.len(), 3, "{got:?}");
    let reads = rec.take();
    let in_leaves: Vec<(u64, u64)> = reads
        .iter()
        .copied()
        .filter(|&(o, l)| inside(&[(o, l)], leaves) > 0)
        .collect();
    // The graph directory block and one leaf of one order; it was seven reads
    // (the block, then every leaf of each of the six orders).
    assert!(
        in_leaves.len() <= 2,
        "GRAPH <g> made {} reads in the quad leaves: {in_leaves:?}",
        in_leaves.len()
    );
    // One leaf, not the order: far less than a whole order's directory.
    let widest = leaves_per_order().into_iter().max().unwrap() as u64;
    let bytes: u64 = in_leaves.iter().map(|r| r.1).sum();
    let all = leaves.1 - leaves.0;
    assert!(
        bytes < all / (3 * widest.min(4)),
        "GRAPH <g> read {bytes} B of {all} B of quad leaves"
    );
    assert!(!rete.index_incomplete());
}

/// Every pattern shape inside one graph, and the graph-variable shapes, give the
/// resident open's rows: an order built on first use is the same directory.
#[test]
fn deferred_directories_answer_like_the_resident_open() {
    let resident = Rete::open(image()).unwrap();
    let rec = Rec::new();
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    let queries = [
        format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}"),
        format!("SELECT ?p ?o WHERE {{ GRAPH {G} {{ <http://ex/e/345> ?p ?o }} }}"),
        format!("SELECT ?s ?o WHERE {{ GRAPH {G} {{ ?s <http://ex/rel1> ?o }} }}"),
        format!("SELECT ?s ?p WHERE {{ GRAPH {G} {{ ?s ?p <http://ex/o/{}> }} }}", (12345 * 13) % 997),
        format!("SELECT ?s ?o WHERE {{ GRAPH {G} {{ ?s <http://ex/rel0> ?o . ?s <http://ex/rel2> ?x }} }}"),
        format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} GRAPH <http://ex/np/777#assertion> {{ ?s ?p ?o }} }}"),
        format!("SELECT ?g WHERE {{ GRAPH ?g {{ <http://ex/np/100> <{NP}hasAssertion> ?a }} }}"),
        format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <http://ex/np/9#Head> {{ <http://ex/np/9> <{NP}hasAssertion> ?a }} \
             GRAPH ?a {{ ?s ?p ?o }} }}"
        ),
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <http://ex/np/5#assertion> { ?s ?p ?o } }".to_string(),
        "SELECT ?s WHERE { GRAPH <http://ex/np/none#assertion> { ?s ?p ?o } }".to_string(),
    ];
    for q in &queries {
        assert_eq!(rows(&lazy, q), rows(&resident, q), "{q}");
    }
    assert!(!lazy.index_incomplete());
}

/// A failed directory read is an error for the query, never a silently smaller
/// answer, and a later query retries.
#[test]
fn a_failed_directory_read_is_surfaced_and_retried() {
    let rec = Rec::new();
    let rete = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.fail.store(true, Ordering::Relaxed);
    let q = format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}");
    let _ = eval_query(&rete, &q);
    assert!(rete.index_incomplete(), "the failure must be recorded");
    rec.fail.store(false, Ordering::Relaxed);
    rete.reset_load_failures();
    assert_eq!(rows(&rete, &q).len(), 3);
    assert!(!rete.index_incomplete());
}

/// Graphs all along the dictionary (so in every leaf of every order, and at
/// leaf boundaries) answer as the resident open does, in every routed order.
#[test]
fn graphs_in_every_leaf_answer_like_the_resident_open() {
    let resident = Rete::open(image()).unwrap();
    let rec = Rec::new();
    let lazy = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    let mut ids: Vec<usize> = (0..N).step_by(1_999).collect();
    ids.extend([0, 1, N - 2, N - 1]);
    for i in ids {
        for (g, o) in [("assertion", i * 13 % 997), ("Head", 0)] {
            let graph = format!("<http://ex/np/{i}#{g}>");
            for q in [
                format!("SELECT ?s ?p ?o WHERE {{ GRAPH {graph} {{ ?s ?p ?o }} }}"),
                format!("SELECT ?s ?p WHERE {{ GRAPH {graph} {{ ?s ?p <http://ex/o/{o}> }} }}"),
                format!("SELECT ?s ?o WHERE {{ GRAPH {graph} {{ ?s <http://ex/rel1> ?o }} }}"),
            ] {
                assert_eq!(rows(&lazy, &q), rows(&resident, &q), "{q}");
            }
        }
    }
    assert!(!lazy.index_incomplete());
}
