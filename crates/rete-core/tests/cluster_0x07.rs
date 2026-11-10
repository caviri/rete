//! Format `0x07` class clustering of the subject-leading permutations
//! (`--cluster-subjects-by-class`, SPEC.md §6.6): the same triples in, the same
//! answers out, clustered or not, resident or lazy; a type-scoped join reads one
//! contiguous run; a clustered file un-clusters to the bytes it started from.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::write::{convert_image, rewrite, WriteOpts};
use rete_core::v06::{
    decode_cluster_root, decode_index_root, open::read_front, tag, F_CLUSTERED, VERSION_06,
};
use rete_core::write_opts::{write_options, ClusterOpts};
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

fn subject(i: u32) -> String {
    // Scattered over the id space: a class is not a lexical range.
    iri(&format!("r/{:06}", (i as u64 * 7919) % 100_003))
}

/// 6,000 records of five classes (A 40%, B 25%, C 15%, D 5%, E 5%), some
/// multi-typed (every third E is also an A), and 10% untyped.
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    let class = |c: &str| iri(&format!("C{c}"));
    for i in 0..6_000u32 {
        let s = subject(i);
        let k = i % 20;
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
        for t in types {
            q.push((s.clone(), TYPE.into(), class(t), None));
        }
        q.push((s.clone(), iri("name"), format!("\"n{i}\""), None));
        q.push((s.clone(), iri("v"), iri(&format!("o/{}", i % 50)), None));
        q.push((s.clone(), iri("cites"), subject((i * 31 + 7) % 6_000), None));
        if k < 8 {
            q.push((s.clone(), iri("pa"), format!("\"x{}\"", i % 9), None));
        }
        if k == 16 {
            q.push((s.clone(), iri("pd"), iri("o/1"), None));
        }
    }
    // Some named graphs, which the clustering must leave alone.
    for g in 0..20 {
        q.push((
            subject(g),
            iri("p"),
            iri(&format!("o/{g}")),
            Some(iri(&format!("g/{g}"))),
        ));
    }
    q
}

static BUILD: Mutex<()> = Mutex::new(());

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

fn opts(cluster: Option<ClusterOpts>) -> WriteOpts {
    WriteOpts {
        cluster,
        ..Default::default()
    }
}

fn four() -> ClusterOpts {
    ClusterOpts {
        max_classes: 4,
        min_subjects: 100,
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
            // ORDER BY queries are compared in the order they came.
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
    // Bound subject: every class, multi-typed, untyped, absent.
    for i in [
        0u32, 1, 8, 9, 13, 14, 16, 17, 18, 19, 20, 37, 5_999, 123, 4_567,
    ] {
        let s = subject(i);
        qs.push(format!("SELECT * WHERE {{ {s} ?p ?o }}"));
        qs.push(format!("SELECT * WHERE {{ {s} <http://ex/name> ?o }}"));
        qs.push(format!("SELECT * WHERE {{ {s} {t} ?c }}"));
        qs.push(format!("ASK {{ {s} <http://ex/cites> ?o }}"));
        qs.push(format!("SELECT * WHERE {{ ?x <http://ex/cites> {s} }}"));
    }
    qs.push("SELECT * WHERE { <http://ex/absent> ?p ?o }".into());
    for c in ["A", "B", "C", "D", "E", "Z"] {
        let c = format!("<http://ex/C{c}>");
        qs.push(format!("SELECT ?s WHERE {{ ?s {t} {c} }}"));
        qs.push(format!("SELECT (COUNT(*) AS ?n) WHERE {{ ?s {t} {c} }}"));
        qs.push(format!(
            "SELECT ?s ?n WHERE {{ ?s {t} {c} . ?s <http://ex/name> ?n }}"
        ));
        qs.push(format!("SELECT ?s ?p ?o WHERE {{ ?s {t} {c} . ?s ?p ?o }}"));
        qs.push(format!(
            "SELECT ?s ?v WHERE {{ ?s {t} {c} . ?s <http://ex/v> ?v . ?s <http://ex/name> ?n }} ORDER BY ?v ?s"
        ));
        qs.push(format!(
            "SELECT DISTINCT ?v WHERE {{ ?s {t} {c} . ?s <http://ex/v> ?v }}"
        ));
        qs.push(format!(
            "SELECT ?s ?x WHERE {{ ?s {t} {c} . OPTIONAL {{ ?s <http://ex/pa> ?x }} }}"
        ));
        qs.push(format!(
            "SELECT ?s ?n WHERE {{ ?s {t} {c} . ?s <http://ex/name> ?n FILTER(STRSTARTS(?n, \"n1\")) }}"
        ));
        qs.push(format!(
            "SELECT ?a ?s WHERE {{ ?a <http://ex/cites> ?s . ?s {t} {c} }}"
        ));
    }
    qs.extend(
        [
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
            "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ?p ?o }",
            "SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
            "SELECT ?s (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s",
            "SELECT ?c (COUNT(*) AS ?n) WHERE { ?s a ?c } GROUP BY ?c",
            "SELECT ?s WHERE { ?s <http://ex/pa> ?x }",
            "SELECT ?s WHERE { ?s <http://ex/pa> \"x3\" }",
            "SELECT ?s ?o WHERE { ?s <http://ex/v> <http://ex/o/7> . ?s <http://ex/name> ?o }",
            "SELECT ?s ?t WHERE { ?s <http://ex/pa> ?x . ?s a ?t }",
            "SELECT ?s ?n ?t WHERE { ?s <http://ex/name> ?n . OPTIONAL { ?s a ?t } }",
            "SELECT ?s ?o ?o2 WHERE { ?s <http://ex/cites> ?o . ?o <http://ex/cites> ?o2 }",
            "SELECT ?s WHERE { ?s ?p <http://ex/o/1> } ORDER BY ?s LIMIT 20",
            "SELECT ?s ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 30",
            "SELECT ?g ?o WHERE { GRAPH ?g { ?s <http://ex/p> ?o } }",
            "SELECT ?s WHERE { ?s a <http://ex/CA> FILTER NOT EXISTS { ?s a <http://ex/CE> } }",
            "SELECT ?s WHERE { ?s a <http://ex/CA> , <http://ex/CE> }",
            "ASK { ?s a <http://ex/CD> . ?s <http://ex/pd> <http://ex/o/1> }",
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

#[test]
fn the_flag_is_in_the_header_and_the_unflagged_file_is_unchanged() {
    let (h, root) = read_front(&SliceReader::new(clustered())).unwrap();
    assert_ne!(h.flags & F_CLUSTERED, 0);
    let c = decode_cluster_root(root.unit(tag::CLUSTER).unwrap()).unwrap();
    assert_eq!(c.len(), 2, "SPO and SOP");
    for o in &c {
        assert_eq!(o.classes.len(), 4);
        assert_eq!(o.run_tiles.len(), 5);
        assert!(o.run_tiles.iter().all(|&n| n > 0), "{o:?}");
    }
    let p = decode_index_root(root.unit(tag::INDEX).unwrap()).unwrap();
    for o in &c {
        let n = p.iter().find(|p| p.si == o.si).unwrap().n_tiles;
        assert_eq!(o.run_tiles.iter().sum::<usize>(), n);
    }
    assert!(rete_core::format::verify(clustered()).unwrap());
    // No flag, no cluster root: the file is what it was.
    let (hp, rootp) = read_front(&SliceReader::new(plain())).unwrap();
    assert_eq!(hp.flags & F_CLUSTERED, 0);
    assert!(rootp.unit(tag::CLUSTER).is_none());
    assert!(plain() == &convert_image(v5(), WriteOpts::default()).unwrap()[..]);
}

#[test]
fn a_clustered_file_un_clusters_to_the_bytes_it_started_from() {
    let back = rw(clustered(), opts(None));
    assert!(plain() == &back[..], "un-clustered bytes differ");
    // And clustering is deterministic, from either start.
    let again = rw(&back, opts(Some(four())));
    assert!(clustered() == &again[..], "re-clustered bytes differ");
}

#[test]
fn every_shape_answers_the_same_clustered_or_not() {
    let qs = battery();
    let want = answers(&Rete::open(v06()).unwrap(), &qs);
    assert_same(
        &qs,
        &answers(&Rete::open(plain()).unwrap(), &qs),
        &want,
        "plain",
    );
    assert_same(
        &qs,
        &answers(&Rete::open(clustered()).unwrap(), &qs),
        &want,
        "clustered resident",
    );
    let (l, _) = lazy(clustered());
    assert_same(&qs, &answers(&l, &qs), &want, "clustered lazy");
    let (l, _) = lazy(plain());
    assert_same(&qs, &answers(&l, &qs), &want, "plain lazy");
}

#[test]
fn a_point_lookup_reads_at_most_a_tile_a_run() {
    let q = format!("SELECT * WHERE {{ {} ?p ?o }}", subject(8));
    let (l, rec) = lazy(clustered());
    let want = rows(&Rete::open(plain()).unwrap(), &q);
    assert_eq!(rows(&l, &q), want);
    let n = rec.take().len();
    // Dictionary, directory leaves and tiles for the five runs: bounded.
    assert!(n < 60, "{n} reads");
}
