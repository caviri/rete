//! G48: a graph variable over many tiny named graphs (the shape of a nanopub
//! dump: several small graphs per publication) must neither read the whole
//! file to answer a point question nor hold anything per graph while it
//! streams.
//!
//! The fixture is synthetic: 30 000 publications, three small graphs each
//! (about 240 000 quads), cut into 4 KiB tiles so that every order has a few
//! hundred of them.

use std::sync::Arc;

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{eval_query, CountingReader, PermSet, PyramidAlgo, QueryOutput, Rete, SliceReader};

const N: usize = 30_000;
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
        let prov = iri(&format!("http://ex/np/{i}#provenance"));
        q.push((
            np.clone(),
            iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            iri(&format!("{NP}Nanopublication")),
            head.clone(),
        ));
        q.push((
            np.clone(),
            iri(&format!("{NP}hasAssertion")),
            assertion.clone(),
            head.clone(),
        ));
        q.push((
            np.clone(),
            iri(&format!("{NP}hasProvenance")),
            prov.clone(),
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
        let gp = Some(prov);
        q.push((
            assertion.clone(),
            iri("http://www.w3.org/ns/prov#wasAttributedTo"),
            iri(&format!("http://ex/agent/{}", i % 50)),
            gp.clone(),
        ));
        q.push((
            assertion,
            iri("http://www.w3.org/ns/prov#generatedAtTime"),
            format!("\"2018-04-{:02}\"", 1 + i % 28),
            gp,
        ));
    }
    q
}

fn bytes() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let mut o = write_options();
        o.tile_budget = 4 * 1024;
        set_write_options(o);
        // The image these reads are measured against keeps the three graph-leading
        // orders (the legacy `0x05` layout); an in-RAM build for `0x07` carries GSPO
        // alone and derives the rest when it is written, so its quad section is a
        // third of the size the thresholds below were set against.
        std::env::set_var("RETE_WRITE_FORMAT", "5");
        let image = assemble_dataset_with_layout(
            fixture(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            PermSet::CORE,
            GraphLayout::Quad,
            |_, _| Vec::<u8>::new(),
        )
        .0;
        std::env::remove_var("RETE_WRITE_FORMAT");
        image
    })
}

/// A cold lazy open whose reads are counted, and the quad section's length.
fn lazy() -> (Rete, Arc<CountingReader<SliceReader<'static>>>, u64) {
    let section = Rete::open(bytes())
        .unwrap()
        .header()
        .quad_index_section()
        .unwrap()
        .length;
    let reader = Arc::new(CountingReader::new(SliceReader::new(bytes())));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    (rete, reader, section)
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => rows.into_iter().map(|b| format!("{b:?}")).collect(),
        other => panic!("unexpected output {other:?}"),
    }
}

/// The integer a single-row COUNT query answers.
fn count(rete: &Rete, q: &str) -> usize {
    let r = rows(rete, q);
    assert_eq!(r.len(), 1, "{q}");
    // The binding prints as `{"n": "\"30000\"^^<…integer>"}`: the digits after
    // the first escaped quote.
    let at = r[0]
        .find("\\\"")
        .unwrap_or_else(|| panic!("no literal in {}", r[0]));
    let digits: String = r[0][at + 2..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().unwrap_or_else(|_| panic!("{}", r[0]))
}

const PFX: &str = "PREFIX np: <http://ex/np#> ";

/// `GRAPH <head> { <np> np:hasAssertion ?a } GRAPH ?a { ?s ?p ?o }`: the graph
/// variable is bound on the left, so the right side is one graph's lookup. It
/// used to evaluate `GRAPH ?a` over every graph first.
#[test]
fn a_graph_variable_bound_on_the_left_reads_one_graph() {
    let (rete, reader, section) = lazy();
    let before = reader.bytes_read();
    let got = rows(
        &rete,
        &format!(
            "{PFX} SELECT ?s ?p ?o WHERE {{ GRAPH <http://ex/np/12345#Head> \
             {{ <http://ex/np/12345> np:hasAssertion ?a }} GRAPH ?a {{ ?s ?p ?o }} }}"
        ),
    );
    assert_eq!(got.len(), 3, "{got:?}");
    let direct = rows(
        &rete,
        "SELECT ?s ?p ?o WHERE { GRAPH <http://ex/np/12345#assertion> { ?s ?p ?o } }",
    );
    let mut a = got;
    let mut b = direct;
    a.sort();
    b.sort();
    assert_eq!(a, b);
    let pulled = reader.bytes_read() - before;
    assert!(
        pulled < section / 20,
        "the join pulled {pulled} B of a {section} B quad section"
    );
    assert!(!rete.index_incomplete());
}

/// Several left rows each look their own graph up; none walks the graphs.
#[test]
fn a_join_binds_the_graph_for_each_left_row() {
    let (rete, reader, section) = lazy();
    let before = reader.bytes_read();
    let got = rows(
        &rete,
        &format!(
            "{PFX} SELECT ?np ?s WHERE {{ VALUES ?np {{ <http://ex/np/1> <http://ex/np/2> <http://ex/np/29999> }} \
             GRAPH <http://ex/np/1#Head> {{ <http://ex/np/1> np:hasAssertion ?a1 }} \
             BIND(IRI(CONCAT(STR(?np), \"#assertion\")) AS ?g) GRAPH ?g {{ ?s <http://ex/rel1> ?o }} }}"
        ),
    );
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(
        reader.bytes_read() - before < section / 10,
        "the join read {} B of a {section} B section",
        reader.bytes_read() - before
    );
}

/// A scan with a LIMIT stops once it has its rows: it does not read every tile
/// of the order it walks (the unlimited query does).
#[test]
fn a_limit_stops_a_graph_variable_scan_early() {
    let q =
        format!("{PFX} SELECT ?h WHERE {{ GRAPH ?h {{ <http://ex/np/100> np:hasAssertion ?a }} }}");
    let (full, full_reader, _) = lazy();
    let before = full_reader.bytes_read();
    assert_eq!(rows(&full, &q).len(), 1);
    let whole = full_reader.bytes_read() - before;

    let (rete, reader, _) = lazy();
    let before = reader.bytes_read();
    assert_eq!(rows(&rete, &format!("{q} LIMIT 1")).len(), 1);
    let limited = reader.bytes_read() - before;
    // A LIMIT that read on after the first match would pull what the full
    // query pulls.
    assert!(
        limited < whole,
        "LIMIT 1 pulled {limited} B, the full scan {whole} B"
    );
}

/// A count over a graph variable holds nothing per graph: the answer is right
/// and the named-graph bookkeeping stays small, where a name and a view cell
/// per graph would grow with the 90 000 graphs.
#[test]
fn counting_over_every_graph_keeps_nothing_per_graph() {
    let (rete, _, _) = lazy();
    let n = count(
        &rete,
        &format!(
            "{PFX} SELECT (COUNT(?np) AS ?n) WHERE {{ GRAPH ?h {{ ?np a <http://ex/np#Nanopublication> }} }}"
        ),
    );
    assert_eq!(n, N);
    let held = rete.fixed_overhead().named_graphs;
    // The tile directory of the order it scanned, not a byte per graph.
    assert!(held < 256 * 1024, "the scan holds {held} B of graph state");
    assert!(!rete.index_incomplete());

    // A FILTER keeps the pattern off the scan: every graph is visited, one
    // view at a time. (A bare `?s ?p ?o` streams from the tiles instead.)
    let (rete, _, _) = lazy();
    let quads = rete.named_quad_count().unwrap() as usize;
    let n = count(
        &rete,
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o FILTER(isIRI(?s)) } }",
    );
    assert_eq!(n, quads);
    let graphs = rete.named_graph_count();
    assert_eq!(graphs, 3 * N);
    let held = rete.fixed_overhead().named_graphs as usize;
    // 16 B per graph for the directory blocks; a kept name and cell would be
    // several times that.
    assert!(
        held < 24 * graphs,
        "the walk holds {held} B for {graphs} graphs"
    );
}

/// A walk over graphs with several patterns picks its graphs by the pattern
/// with a constant, and answers what the plain walk answers.
#[test]
fn a_constant_in_one_pattern_picks_the_graphs_of_a_multi_pattern_walk() {
    let (rete, reader, section) = lazy();
    let q = "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/rel0> <http://ex/o/13> . ?s <http://ex/rel1> ?x } }";
    let before = reader.bytes_read();
    let got = rows(&rete, q);
    assert!(!got.is_empty());
    assert!(reader.bytes_read() - before < section);
    // The same question with no constant to scan on, which walks every graph.
    let walked = rows(
        &rete,
        "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/rel0> ?o . ?s <http://ex/rel1> ?x FILTER(?o = <http://ex/o/13>) } }",
    );
    let (mut a, mut b) = (got, walked);
    a.sort();
    b.sort();
    assert_eq!(a, b);
}
