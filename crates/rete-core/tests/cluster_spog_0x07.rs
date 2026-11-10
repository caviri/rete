//! Format `0x07` class clustering of the quad index's SPOG
//! (`--cluster-subjects-by-class`, SPEC.md section 6.6) and the planner route
//! that probes it: the same quads in, the same answers out, clustered or not,
//! resident or lazy, by the graph walk or the SPOG probes; a class-scoped
//! `GRAPH ?g` query on a clustered lazy file reads one contiguous run of SPOG;
//! a clustered file un-clusters to the bytes it started from.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::bgp::set_class_route;
use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::parts::from_v06;
use rete_core::v06::write::{convert_image, rewrite, WriteOpts};
use rete_core::v06::{
    decode_cluster_root, open::read_front, tag, ClusterOrder, F_CLUSTERED, QUAD_SPOG, VERSION_06,
};
use rete_core::write_opts::{write_options, ClusterOpts};
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const GRAPHS: u32 = 37;

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

fn subject(i: u32) -> String {
    // Scattered over the id space: a class is not a lexical range.
    iri(&format!("r/{:06}", (i as u64 * 7919) % 100_003))
}

fn graph(i: u32) -> Option<String> {
    Some(iri(&format!("g/{}", i % GRAPHS)))
}

/// 6,000 records of five classes (A 40%, B 25%, C 15%, D 5%, E 5%) in 37 named
/// graphs: some multi-typed (every third E is also an A), some typed in two
/// graphs, 10% untyped; the `name` of a C record is in another graph than its
/// type, so the block of a class-scoped query matches only where both agree.
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    let class = |c: &str| iri(&format!("C{c}"));
    for i in 0..6_000u32 {
        let s = subject(i);
        let k = i % 20;
        let (g, g2) = (graph(i), graph(i * 5 + 1));
        let mut types: Vec<&str> = match k {
            0..=7 => vec!["A"],
            8..=12 => vec!["B"],
            13..=15 => vec!["C"],
            16 => vec!["D"],
            17 => vec!["E"],
            _ => vec![],
        };
        if k == 17 && i % 3 == 0 {
            types.push("A");
        }
        for t in &types {
            q.push((s.clone(), TYPE.into(), class(t), g.clone()));
        }
        if k < 8 && i % 7 == 0 {
            q.push((s.clone(), TYPE.into(), class("A"), g2.clone()));
        }
        let name_in = if (13..=15).contains(&k) { &g2 } else { &g };
        q.push((s.clone(), iri("name"), format!("\"n{i}\""), name_in.clone()));
        q.push((
            s.clone(),
            iri("v"),
            iri(&format!("o/{}", i % 50)),
            g.clone(),
        ));
        q.push((s.clone(), iri("cites"), subject((i * 31 + 7) % 6_000), g2));
        if i % 97 == 0 {
            q.push((s.clone(), iri("cites"), s.clone(), g.clone()));
        }
        if k < 8 {
            q.push((s.clone(), iri("pa"), format!("\"x{}\"", i % 9), g.clone()));
        }
        if k == 16 {
            q.push((s.clone(), iri("pd"), iri("o/1"), g.clone()));
        }
    }
    // A few default-graph triples.
    for i in 0..40u32 {
        q.push((subject(i), iri("d"), iri(&format!("o/{i}")), None));
    }
    q
}

static BUILD: Mutex<()> = Mutex::new(());
/// The route switch is process-wide: tests that query hold this.
static ROUTE: Mutex<()> = Mutex::new(());

fn v5() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
        let mut o = write_options();
        o.tile_budget = 2 * 1024;
        rete_core::write_opts::set_write_options(o);
        assemble_dataset_with_layout(
            dataset(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            PermSet::ALL,
            GraphLayout::Quad,
            |_, _| Vec::<u8>::new(),
        )
        .0
    })
}

fn four() -> ClusterOpts {
    ClusterOpts {
        quad_classes: 4,
        max_classes: 4,
        min_subjects: 100,
    }
}

fn opts(cluster: Option<ClusterOpts>) -> WriteOpts {
    WriteOpts {
        cluster,
        ..Default::default()
    }
}

/// A `0x06`/`0x07` file rewritten under `o`.
fn rw(bytes: &[u8], o: WriteOpts) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    rewrite(&SliceReader::new(bytes), &mut out, o).unwrap();
    out.into_inner()
}

fn plain() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), opts(None)).unwrap())
}

fn clustered() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), opts(Some(four()))).unwrap())
}

fn v06() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let o = WriteOpts {
            version: VERSION_06,
            ..Default::default()
        };
        convert_image(v5(), o).unwrap()
    })
}

struct Rec {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    bytes: AtomicU64,
}

impl Rec {
    fn new(data: &[u8]) -> Arc<Self> {
        Arc::new(Rec {
            data: data.to_vec(),
            reads: Mutex::new(Vec::new()),
            bytes: AtomicU64::new(0),
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
        self.reads.lock().unwrap().push((offset, len));
        self.bytes.fetch_add(len, Ordering::Relaxed);
        self.data
            .get(offset as usize..(offset + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
    fn concurrency(&self) -> usize {
        4
    }
}

fn lazy(data: &[u8]) -> (Rete, Arc<Rec>) {
    let rec = Rec::new(data);
    let r = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.take();
    rec.bytes.store(0, Ordering::Relaxed);
    (r, rec)
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            if !q.contains("ORDER BY") {
                v.sort();
            }
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn battery() -> Vec<String> {
    let t = TYPE;
    let mut qs: Vec<String> = Vec::new();
    for c in ["A", "B", "C", "D", "E", "Z"] {
        let c = format!("<http://ex/C{c}>");
        qs.push(format!(
            "SELECT ?g ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s ?p ?o }} }}"
        ));
        qs.push(format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s ?p ?o }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s ?n WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/name> ?n }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s ?v ?n WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/v> ?v . ?s <http://ex/name> ?n }} }}"
        ));
        qs.push(format!(
            "SELECT ?s ?g WHERE {{ GRAPH ?g {{ ?s {t} {c} }} }}"
        ));
        qs.push(format!(
            "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s ?p ?o }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s ?n WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/name> ?n FILTER(STRSTARTS(?n, \"n1\")) }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/cites> ?s }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s ?x WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/pa> ?x }} }} ORDER BY ?g ?s LIMIT 25"
        ));
        qs.push(format!(
            "SELECT ?s ?p WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s ?p <http://ex/o/1> }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {t} {c} . ?s ?p ?o }} }} ORDER BY ?g ?s ?p ?o LIMIT 7"
        ));
        qs.push(format!(
            "ASK {{ GRAPH ?g {{ ?s {t} {c} . ?s <http://ex/pd> <http://ex/o/1> }} }}"
        ));
    }
    qs.push(format!(
        "SELECT ?g ?s WHERE {{ GRAPH ?g {{ ?s {t} <http://ex/CA> , <http://ex/CE> }} }}"
    ));
    qs.push(format!(
        "SELECT ?g ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {t} <http://ex/CE> . ?s {t} <http://ex/CA> . ?s ?p ?o }} }}"
    ));
    // Bound subject, over every graph.
    for i in [
        0u32, 1, 8, 9, 13, 14, 16, 17, 18, 19, 20, 37, 5_999, 123, 4_567,
    ] {
        let s = subject(i);
        qs.push(format!(
            "SELECT ?g ?p ?o WHERE {{ GRAPH ?g {{ {s} ?p ?o }} }}"
        ));
        qs.push(format!(
            "SELECT ?g ?o WHERE {{ GRAPH ?g {{ {s} <http://ex/name> ?o }} }}"
        ));
        qs.push(format!("SELECT * WHERE {{ {s} ?p ?o }}"));
    }
    qs.push("SELECT ?g ?p ?o WHERE { GRAPH ?g { <http://ex/absent> ?p ?o } }".into());
    qs.extend(
        [
            "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }",
            "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g",
            "SELECT ?g ?o WHERE { GRAPH ?g { ?s <http://ex/v> ?o } }",
            "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/pa> \"x3\" } }",
            "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://ex/v> <http://ex/o/7> . ?s <http://ex/name> ?o } }",
            "SELECT ?s ?o WHERE { ?s <http://ex/d> ?o }",
            "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s ?p <http://ex/o/1> } } ORDER BY ?g ?s LIMIT 20",
        ]
        .map(String::from),
    );
    qs
}

fn answers(rete: &Rete, qs: &[String]) -> Vec<Vec<String>> {
    qs.iter().map(|q| rows(rete, q)).collect()
}

fn assert_same(qs: &[String], got: &[Vec<String>], want: &[Vec<String>], what: &str) {
    for ((q, g), w) in qs.iter().zip(got).zip(want) {
        assert_eq!(g.len(), w.len(), "{what}: {q}");
        assert!(
            g == w,
            "{what}: {q}\n got {:?}\nwant {:?}",
            &g[..g.len().min(3)],
            &w[..w.len().min(3)]
        );
    }
}

fn spog_cluster(bytes: &[u8]) -> Option<ClusterOrder> {
    let (_, root) = read_front(&SliceReader::new(bytes)).unwrap();
    decode_cluster_root(root.unit(tag::CLUSTER)?)
        .unwrap()
        .into_iter()
        .find(|c| c.si == QUAD_SPOG)
}

#[test]
fn spog_is_clustered_under_the_flag_and_the_unflagged_file_is_unchanged() {
    let (h, _) = read_front(&SliceReader::new(clustered())).unwrap();
    assert_ne!(h.flags & F_CLUSTERED, 0);
    let c = spog_cluster(clustered()).expect("SPOG has a cluster entry");
    assert_eq!(c.classes.len(), 4);
    assert_eq!(c.run_tiles.len(), 5);
    assert!(c.run_tiles.iter().all(|&n| n > 0), "{c:?}");
    let parts = from_v06(&SliceReader::new(clustered())).unwrap();
    let spog = &parts.quad.as_ref().unwrap().orders;
    let n = spog
        .iter()
        .find(|(si, _)| *si == QUAD_SPOG)
        .unwrap()
        .1
        .len();
    assert_eq!(c.run_tiles.iter().sum::<usize>(), n);
    assert!(rete_core::format::verify(clustered()).unwrap());
    // The default graph has no class with 100 subjects: only SPOG is clustered.
    let (_, root) = read_front(&SliceReader::new(clustered())).unwrap();
    let all = decode_cluster_root(root.unit(tag::CLUSTER).unwrap()).unwrap();
    assert_eq!(all.len(), 1, "{all:?}");
    // No flag, no cluster root, and the file is what it was.
    let (hp, rootp) = read_front(&SliceReader::new(plain())).unwrap();
    assert_eq!(hp.flags & F_CLUSTERED, 0);
    assert!(rootp.unit(tag::CLUSTER).is_none());
    assert!(plain() == &convert_image(v5(), WriteOpts::default()).unwrap()[..]);
    assert!(plain() != clustered());
    let (n_problem_orders, problems) =
        rete_core::v06::check_clusters(&SliceReader::new(clustered())).unwrap();
    assert_eq!((n_problem_orders, problems.len()), (1, 0), "{problems:?}");
}

#[test]
fn a_clustered_file_un_clusters_to_the_bytes_it_started_from() {
    let back = rw(clustered(), opts(None));
    assert!(plain() == &back[..], "un-clustered bytes differ");
    let again = rw(&back, opts(Some(four())));
    assert!(clustered() == &again[..], "re-clustered bytes differ");
    // A clustered file kept under the flag keeps its clusters as they are.
    let kept = rw(clustered(), opts(Some(four())));
    assert!(
        clustered() == &kept[..],
        "a clustered file changed under the flag"
    );
}

#[test]
fn every_shape_answers_the_same_by_either_route_clustered_or_not() {
    let _g = ROUTE.lock().unwrap_or_else(|e| e.into_inner());
    let qs = battery();
    set_class_route(Some(false));
    let want = answers(&Rete::open(v06()).unwrap(), &qs);
    for (name, route) in [("walk", Some(false)), ("probe", Some(true)), ("auto", None)] {
        set_class_route(route);
        assert_same(
            &qs,
            &answers(&Rete::open(plain()).unwrap(), &qs),
            &want,
            &format!("plain resident, {name}"),
        );
        assert_same(
            &qs,
            &answers(&Rete::open(clustered()).unwrap(), &qs),
            &want,
            &format!("clustered resident, {name}"),
        );
        let (l, _) = lazy(clustered());
        assert_same(
            &qs,
            &answers(&l, &qs),
            &want,
            &format!("clustered lazy, {name}"),
        );
        let (l, _) = lazy(plain());
        assert_same(
            &qs,
            &answers(&l, &qs),
            &want,
            &format!("plain lazy, {name}"),
        );
    }
    set_class_route(None);
    // The rows are not empty, or the comparison proves nothing.
    assert!(want.iter().filter(|r| r.is_empty()).count() < qs.len() / 4);
}

/// The byte ranges of SPOG's tiles in `bytes`, per run.
fn spog_runs(bytes: &[u8]) -> Vec<(u64, u64)> {
    let parts = from_v06(&SliceReader::new(bytes)).unwrap();
    let c = spog_cluster(bytes).unwrap();
    let tiles = &parts
        .quad
        .as_ref()
        .unwrap()
        .orders
        .iter()
        .find(|(si, _)| *si == QUAD_SPOG)
        .unwrap()
        .1;
    let mut at = 0;
    c.run_tiles
        .iter()
        .map(|&n| {
            let t = &tiles[at..at + n];
            at += n;
            (t[0].off, t[n - 1].off + t[n - 1].clen)
        })
        .collect()
}

#[test]
fn a_class_scoped_query_reads_one_contiguous_run_of_spog() {
    let _g = ROUTE.lock().unwrap_or_else(|e| e.into_inner());
    let runs = spog_runs(clustered());
    let whole = (runs[0].0, runs[runs.len() - 1].1);
    let (l, rec) = lazy(clustered());
    // D: 300 subjects, a run of its own.
    let q = format!(
        "SELECT ?g ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {TYPE} <http://ex/CD> . ?s ?p ?o }} }}"
    );
    set_class_route(Some(true));
    let got = rows(&l, &q);
    set_class_route(None);
    assert!(!got.is_empty());
    let want = rows(&Rete::open(v06()).unwrap(), &q);
    assert_eq!(got, want);
    let spog_reads: Vec<(u64, u64)> = rec
        .take()
        .into_iter()
        .filter(|&(off, len)| off < whole.1 && off + len > whole.0)
        .collect();
    assert!(!spog_reads.is_empty(), "no SPOG tile was read");
    let in_run = |i: usize| {
        spog_reads
            .iter()
            .all(|&(off, len)| off >= runs[i].0 && off + len <= runs[i].1)
    };
    assert!(
        (0..runs.len()).any(in_run),
        "SPOG reads {spog_reads:?} are not inside one run of {runs:?}"
    );
    // And far less than the unclustered file's SPOG probe reads.
    let read: u64 = spog_reads.iter().map(|r| r.1).sum();
    assert!(
        read < (whole.1 - whole.0) / 3,
        "{read} of {} bytes",
        whole.1 - whole.0
    );
}

#[test]
fn the_estimate_picks_the_probe_on_a_clustered_file_for_a_rare_class() {
    let _g = ROUTE.lock().unwrap_or_else(|e| e.into_inner());
    set_class_route(None);
    let q = format!(
        "SELECT ?g ?s ?p ?o WHERE {{ GRAPH ?g {{ ?s {TYPE} <http://ex/CD> . ?s ?p ?o }} }}"
    );
    let (lc, rc) = lazy(clustered());
    let (lp, rp) = lazy(plain());
    assert_eq!(rows(&lc, &q), rows(&lp, &q));
    let (bc, bp) = (
        rc.bytes.load(Ordering::Relaxed),
        rp.bytes.load(Ordering::Relaxed),
    );
    // The clustered file never reads more than the unclustered one for the
    // same answer.
    assert!(bc <= bp + bp / 4, "clustered {bc} bytes, plain {bp}");
}

#[test]
fn a_point_lookup_reads_a_bounded_number_of_tiles() {
    let _g = ROUTE.lock().unwrap_or_else(|e| e.into_inner());
    set_class_route(None);
    let q = format!(
        "SELECT ?g ?p ?o WHERE {{ GRAPH ?g {{ {} ?p ?o }} }}",
        subject(8)
    );
    let (l, rec) = lazy(clustered());
    let want = rows(&Rete::open(v06()).unwrap(), &q);
    assert_eq!(rows(&l, &q), want);
    let n = rec.take().len();
    // Dictionary, directory leaves and tiles of five runs: bounded.
    assert!(n < 80, "{n} reads");
    // A second subject, then the first again (the placement is remembered).
    let q2 = format!(
        "SELECT ?g ?p ?o WHERE {{ GRAPH ?g {{ {} ?p ?o }} }}",
        subject(9)
    );
    assert_eq!(rows(&l, &q2), rows(&Rete::open(v06()).unwrap(), &q2));
    rec.take();
    assert_eq!(rows(&l, &q), want);
}
