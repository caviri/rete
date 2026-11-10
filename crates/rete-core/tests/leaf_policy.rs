//! A1: the writer keeps directory leaves small. On a big file the old loop grew
//! every directory to one leaf (the root never got under its budget, because the
//! graph directory block index alone is over it), so one cold lookup read a
//! whole directory: 0.73 to 0.99 MB per quad order and a 1.27 MB dictionary
//! directory on the 43 M-graph nanopub dump. The fixture is that dump shape,
//! written with the leaves the writer now keeps and with one leaf per directory
//! (where the old loop ended), read through a recording reader.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::write::{convert_image, LeafPolicy, WriteOpts};
use rete_core::v06::{decode_dict_root, decode_quad_root, kind, open::read_front, tag};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

const N: usize = 80_000;

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
            iri("http://ex/np#hasAssertion"),
            assertion.clone(),
            head.clone(),
        ));
        q.push((
            np,
            iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type"),
            iri("http://ex/np#Nanopublication"),
            head,
        ));
        for k in 0..3 {
            q.push((
                iri(&format!("http://ex/e/{}", (i * 7) % 500)),
                iri(&format!("http://ex/rel{k}")),
                iri(&format!("http://ex/o/{}", (i * 13 + k) % 997)),
                Some(assertion.clone()),
            ));
        }
    }
    q
}

/// The fixture as a `0x05` image, once.
fn v5() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let mut o = write_options();
        o.tile_budget = 4 * 1024;
        set_write_options(o);
        assemble_dataset_with_layout(
            fixture(),
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

fn written(leaves: LeafPolicy) -> Vec<u8> {
    convert_image(
        v5(),
        WriteOpts {
            leaves,
            ..WriteOpts::default()
        },
    )
    .unwrap()
}

/// A big file: the root budget cannot be met whatever the leaf size, so the
/// leaves stay at their starting size (1 KiB here, the writer 16 KiB scaled to
/// a fixture whose whole quad directory is about 10 KB).
fn small() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        written(LeafPolicy {
            start: 1024,
            root_budget: 1,
            ..LeafPolicy::default()
        })
    })
}

/// One leaf per directory: what the old loop ended with on a big file.
fn single() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        written(LeafPolicy {
            start: usize::MAX / 4,
            ..LeafPolicy::default()
        })
    })
}

struct Rec {
    data: &'static [u8],
    reads: Mutex<Vec<(u64, u64)>>,
}

impl Rec {
    fn new(data: &'static [u8]) -> Arc<Self> {
        Arc::new(Rec {
            data,
            reads: Mutex::new(Vec::new()),
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

fn region(image: &[u8], k: u8) -> (u64, u64) {
    let (h, root) = read_front(&SliceReader::new(image)).unwrap();
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

fn leaf_counts(image: &[u8]) -> (Vec<usize>, Vec<usize>) {
    let (_, root) = read_front(&SliceReader::new(image)).unwrap();
    let dict = decode_dict_root(root.unit(tag::DICT).unwrap())
        .unwrap()
        .iter()
        .map(|s| s.leaves.len())
        .collect();
    let quad = decode_quad_root(root.unit(tag::QUAD).unwrap())
        .unwrap()
        .orders
        .iter()
        .map(|o| o.leaves.len())
        .collect();
    (dict, quad)
}

const G: &str = "<http://ex/np/12345#assertion>";

/// Bytes one cold `GRAPH <g> { ?s ?p ?o }` reads from the dictionary's and the
/// quad index's directory leaves.
fn cold_directory_bytes(image: &'static [u8]) -> (u64, u64) {
    let rec = Rec::new(image);
    let rete = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.take();
    let got = rows(
        &rete,
        &format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}"),
    );
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(!rete.index_incomplete());
    let reads = rec.take();
    // The quad leaves section opens with the graph directory's blocks: a block is
    // what the lookup reads to name the graph, not a tile directory, so only the
    // bytes after it count here (a block of 8,192 graphs is a few KB compressed,
    // and 16 to 30 KB without a codec, which would drown the leaves compared).
    let (qstart, qend) = region(image, kind::QUAD_LEAVES);
    let (_, root) = read_front(&SliceReader::new(image)).unwrap();
    let gdir = decode_quad_root(root.unit(tag::QUAD).unwrap())
        .unwrap()
        .gdir_len;
    (
        inside(&reads, region(image, kind::DICT_LEAVES)),
        inside(&reads, (qstart + gdir, qend)),
    )
}

#[test]
fn the_fixture_has_several_leaves_where_the_old_loop_left_one() {
    let (_, quad) = leaf_counts(small());
    assert!(quad.iter().all(|&n| n >= 3), "quad leaves: {quad:?}");
    let (dict, quad) = leaf_counts(single());
    assert!(
        dict.iter().chain(&quad).all(|&n| n <= 1),
        "{dict:?} {quad:?}"
    );
}

/// One cold lookup reads a bounded number of directory bytes with small
/// leaves, and a multiple of that with one leaf per directory.
#[test]
fn a_cold_lookup_reads_less_directory_than_a_whole_one() {
    let (dict, quad) = cold_directory_bytes(small());
    let (sdict, squad) = cold_directory_bytes(single());
    eprintln!("small: dict {dict} quad {quad}; single: dict {sdict} quad {squad}");
    // A few small leaves against the whole directory: the bound is relative
    // because the fixture size moves with the process-wide tile budget.
    let (small, whole) = (dict + quad, sdict + squad);
    assert!(whole > 4 * 1024, "one leaf per directory: {whole} B");
    assert!(small * 3 < whole, "small leaves {small} B vs {whole} B");
}

/// The same rows from both files and from the resident open, every pattern
/// shape inside one graph and across graphs.
#[test]
fn the_rows_are_the_same_whatever_the_leaf_size() {
    let resident = Rete::open(single()).unwrap();
    let lazy = Rete::open_ranged_lazy_with(Rec::new(small()), 4096).unwrap();
    let np = "http://ex/np#hasAssertion";
    let queries = [
        format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}"),
        format!("SELECT ?p ?o WHERE {{ GRAPH {G} {{ <http://ex/e/345> ?p ?o }} }}"),
        format!("SELECT ?s ?o WHERE {{ GRAPH {G} {{ ?s <http://ex/rel1> ?o }} }}"),
        format!(
            "SELECT ?s ?p WHERE {{ GRAPH {G} {{ ?s ?p <http://ex/o/{}> }} }}",
            (12345 * 13) % 997
        ),
        format!("SELECT ?g WHERE {{ GRAPH ?g {{ <http://ex/np/100> <{np}> ?a }} }}"),
        format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <http://ex/np/9#Head> {{ <http://ex/np/9> <{np}> ?a }} \
             GRAPH ?a {{ ?s ?p ?o }} }}"
        ),
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <http://ex/np/5#assertion> { ?s ?p ?o } }".into(),
        "SELECT ?s WHERE { GRAPH <http://ex/np/none#assertion> { ?s ?p ?o } }".into(),
        "SELECT ?g WHERE { GRAPH ?g { <http://ex/e/7> <http://ex/rel0> <http://ex/o/91> } } LIMIT 5"
            .into(),
    ];
    for q in &queries {
        assert_eq!(rows(&lazy, q), rows(&resident, q), "{q}");
    }
    for i in (0..N).step_by(1_999).chain([0, 1, N - 1]) {
        let q = format!(
            "SELECT ?s ?p ?o WHERE {{ GRAPH <http://ex/np/{i}#assertion> {{ ?s ?p ?o }} }}"
        );
        assert_eq!(rows(&lazy, &q), rows(&resident, &q), "{q}");
    }
    assert!(!lazy.index_incomplete());
    assert!(rete_core::file::verify(small()).unwrap(), "verify");
}

/// Counts the dependent round trips of a cold query: each `read_at` and each
/// `read_many` is one round (a batch goes out together), whatever it holds.
struct Rounds {
    data: &'static [u8],
    rounds: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
}

impl Rounds {
    fn new(data: &'static [u8]) -> Arc<Self> {
        Arc::new(Rounds {
            data,
            rounds: Default::default(),
            bytes: Default::default(),
        })
    }
    /// `(rounds, bytes)` since the last call.
    fn take(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::SeqCst;
        (self.rounds.swap(0, SeqCst), self.bytes.swap(0, SeqCst))
    }
    fn slice(&self, o: u64, l: u64) -> std::io::Result<Vec<u8>> {
        use std::sync::atomic::Ordering::SeqCst;
        self.bytes.fetch_add(l, SeqCst);
        self.data
            .get(o as usize..(o + l) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
}

impl RangeReader for Rounds {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.rounds
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.slice(offset, len)
    }
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        self.rounds
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        ranges.iter().map(|&(o, l)| self.slice(o, l)).collect()
    }
    fn concurrency(&self) -> usize {
        4
    }
}

/// Dependent round trips of one cold query over `image`, and its rows. The first
/// read is the header and the root, exactly: a leaf that happens to sit inside a
/// bigger first read costs no round trip, and which one does depends on how big
/// the root is (a smaller block index moves the front sections nearer), not on
/// the layout being compared.
fn cold_rounds(image: &'static [u8], q: &str) -> (u64, Vec<String>) {
    let rec = Rounds::new(image);
    let (h, _) = read_front(&SliceReader::new(image)).unwrap();
    let first = rete_core::HEADER_LEN as u64 + h.root_len as u64;
    let rete = Rete::open_ranged_lazy_with(rec.clone(), first).unwrap();
    rec.take();
    let got = rows(&rete, q);
    assert!(!rete.index_incomplete());
    (rec.take().0, got)
}

/// Cold shapes over small leaves take no more dependent round trips than over
/// one leaf per directory (the old loop's file, read by the same reader): the
/// join, a graph's view and a bound subject in a graph. Before, each distinct
/// dictionary or directory leaf was a read of its own: every predicate a
/// `GRAPH ?a { ?s ?p ?o }` probe met was canonicalized by itself (a leaf read
/// and a chunk read each time), a result's terms were decoded after the
/// predicates were, and a graph's directory block was read before the tile
/// directory leaf that does not depend on it. Rows are the resident reader's.
#[test]
fn cold_shapes_over_small_leaves_are_no_deeper_than_over_one_leaf() {
    let np = "http://ex/np#hasAssertion";
    let queries = [
        (
            "join",
            format!(
                "SELECT ?s ?p ?o WHERE {{ GRAPH <http://ex/np/9#Head> {{ <http://ex/np/9> <{np}> ?a }}                  GRAPH ?a {{ ?s ?p ?o }} }}"
            ),
        ),
        (
            "view",
            format!("SELECT ?s ?p ?o WHERE {{ GRAPH {G} {{ ?s ?p ?o }} }}"),
        ),
        (
            "a graph's rows, decoded",
            "SELECT ?s ?p ?o WHERE { GRAPH <http://ex/np/777#Head> { ?s ?p ?o } }".into(),
        ),
        (
            "bound subject in a graph",
            "SELECT ?p ?o WHERE { GRAPH <http://ex/np/777#assertion> { <http://ex/e/439> ?p ?o } }"
                .into(),
        ),
        (
            "bound subject",
            "SELECT ?p ?o WHERE { GRAPH <http://ex/np/9#Head> { <http://ex/np/9> ?p ?o } }".into(),
        ),
    ];
    let resident = Rete::open(single()).unwrap();
    for (name, q) in &queries {
        let want = rows(&resident, q);
        assert!(!want.is_empty(), "{name}");
        let (small_rounds, got) = cold_rounds(small(), q);
        assert_eq!(&got, &want, "{name}: rows differ from the resident reader");
        let (single_rounds, got) = cold_rounds(single(), q);
        assert_eq!(&got, &want, "{name}: rows differ on one leaf");
        eprintln!("{name}: small {small_rounds} rounds, single {single_rounds} rounds");
        assert!(
            small_rounds <= single_rounds,
            "{name}: {small_rounds} round trips on small leaves, {single_rounds} on one leaf"
        );
    }
}
