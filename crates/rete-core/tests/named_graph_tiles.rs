//! The lazy named-graph path (NAMED_GRAPHS, section kind 5) under failed,
//! short and corrupt reads: every one must surface as `index_incomplete` on
//! the first scan that needs the bytes — never as a smaller answer, never at
//! open (which fetches no tile), never as a panic — and a reset must retry.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::{
    eval_query, write_dataset, Binding, ByteRange, DictionaryBuilder, GraphIndexBuilder, Header,
    QueryOutput, RangeReader, Rete, HEADER_LEN,
};

/// An in-memory reader that can be told to fail every read, or to return
/// fewer bytes than asked while still reporting success (a truncated response
/// a careless transport passes on): one byte fewer, or at most `cap` bytes.
struct FlakyReader {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    fail: AtomicBool,
    short: AtomicBool,
    cap: AtomicUsize,
}

impl FlakyReader {
    fn new(data: Vec<u8>) -> Arc<Self> {
        Arc::new(Self {
            data,
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            short: AtomicBool::new(false),
            cap: AtomicUsize::new(usize::MAX),
        })
    }
    fn fail_from_now(&self) {
        self.fail.store(true, Ordering::Relaxed);
    }
    fn short_from_now(&self) {
        self.short.store(true, Ordering::Relaxed);
    }
    fn truncate_to(&self, cap: usize) {
        self.cap.store(cap, Ordering::Relaxed);
    }
    fn recover(&self) {
        self.fail.store(false, Ordering::Relaxed);
        self.short.store(false, Ordering::Relaxed);
        self.cap.store(usize::MAX, Ordering::Relaxed);
    }
}

impl RangeReader for FlakyReader {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("simulated network failure"));
        }
        self.reads.lock().unwrap().push((offset, len));
        let start = usize::try_from(offset).unwrap();
        let end = start
            .checked_add(usize::try_from(len).unwrap())
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
        let end = if self.short.load(Ordering::Relaxed) && len > 0 {
            end - 1
        } else {
            end
        };
        let end = end.min(start.saturating_add(self.cap.load(Ordering::Relaxed)));
        Ok(self.data[start..end].to_vec())
    }
}

fn node(n: u32) -> String {
    format!("<http://ex/n{n}>")
}

/// One small default graph and named graphs `g000`, `g001`, … holding
/// `sizes[k]` triples each, at a tiny tile budget: 30,000 triples make a
/// multi-MB container, which the lazy path opens tile by tile (the resident
/// cut-off is 1 MiB).
fn dataset(sizes: &[usize]) -> Vec<u8> {
    let p = "<http://ex/p>".to_string();
    let mut db = DictionaryBuilder::new();
    db.observe(&node(0), &p, &node(1));
    let mut all = Vec::new();
    let mut next = 100u32;
    for (g, &n) in sizes.iter().enumerate() {
        for _ in 0..n {
            db.observe(&node(next), &p, &node(next + 1));
            all.push((g, next));
            next += 2;
        }
    }
    let dict = db.build();
    let mut def = GraphIndexBuilder::new();
    def.push(dict.encode(&node(0), &p, &node(1)).unwrap());
    let mut builders: Vec<_> = sizes
        .iter()
        .map(|_| GraphIndexBuilder::new().with_tile_budget(64))
        .collect();
    for &(g, s) in &all {
        builders[g].push(dict.encode(&node(s), &p, &node(s + 1)).unwrap());
    }
    let named: Vec<(String, _)> = builders
        .into_iter()
        .enumerate()
        .map(|(g, b)| (format!("<http://ex/g{g:03}>"), b.build()))
        .collect();
    write_dataset(&dict, &def.build(), &named, true, &[], 0)
}

/// Two small (resident-sized) named graphs, each multi-tile.
fn small_named_image() -> Vec<u8> {
    dataset(&[256, 256])
}

/// Where each named graph's six sections and their tiles sit in the image.
struct NamedGraphLayout {
    name: String,
    sections: Vec<ByteRange>,
    tiles: Vec<Vec<ByteRange>>,
}

/// A test-only LEB128 walker, independent of rete's decoders on purpose: the
/// layout it finds must not share a bug with the code under test.
fn oracle_uvarint(image: &[u8], pos: &mut usize, end: usize) -> u64 {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        assert!(*pos < end, "fixture varint stays inside its section");
        let byte = image[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("fixture varint is longer than ten bytes");
}

fn oracle_end(pos: usize, len: u64, end: usize) -> usize {
    let next = pos + usize::try_from(len).unwrap();
    assert!(next <= end, "fixture field stays inside its section");
    next
}

fn named_graph_layout(image: &[u8]) -> Vec<NamedGraphLayout> {
    let header = Header::from_bytes(&image[..HEADER_LEN]).unwrap();
    let mut pos = usize::try_from(header.named_graphs_offset).unwrap();
    let end = oracle_end(pos, header.named_graphs_len, image.len());
    let count = oracle_uvarint(image, &mut pos, end);
    let mut graphs = Vec::new();
    for _ in 0..count {
        let iri_len = oracle_uvarint(image, &mut pos, end);
        let iri_end = oracle_end(pos, iri_len, end);
        let name = String::from_utf8_lossy(&image[pos..iri_end]).into_owned();
        pos = iri_end;
        let container_len = oracle_uvarint(image, &mut pos, end);
        let container_end = oracle_end(pos, container_len, end);
        let sections_n = oracle_uvarint(image, &mut pos, container_end);
        let mut sections = Vec::new();
        for _ in 0..sections_n {
            let len = oracle_uvarint(image, &mut pos, container_end);
            sections.push(ByteRange {
                offset: pos as u64,
                len,
            });
            pos = oracle_end(pos, len, container_end);
        }
        assert_eq!(pos, container_end);
        let tiles = sections
            .iter()
            .map(|s| {
                let mut p = usize::try_from(s.offset).unwrap();
                let s_end = oracle_end(p, s.len, image.len());
                let n = oracle_uvarint(image, &mut p, s_end);
                let lens: Vec<u64> = (0..n)
                    .map(|_| {
                        oracle_uvarint(image, &mut p, s_end); // delta min
                        oracle_uvarint(image, &mut p, s_end); // span
                        oracle_uvarint(image, &mut p, s_end)
                    })
                    .collect();
                lens.into_iter()
                    .map(|len| {
                        let r = ByteRange {
                            offset: p as u64,
                            len,
                        };
                        p = oracle_end(p, len, s_end);
                        r
                    })
                    .collect()
            })
            .collect();
        graphs.push(NamedGraphLayout {
            name,
            sections,
            tiles,
        });
    }
    graphs
}

fn rows(rete: &Rete, query: &str) -> Vec<Binding> {
    match eval_query(rete, query).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    }
}

fn bound_query() -> String {
    format!(
        "SELECT ?o WHERE {{ GRAPH <http://ex/g000> {{ {} <http://ex/p> ?o }} }}",
        node(100)
    )
}

#[test]
fn lazy_named_graph_tile_failure_sets_incomplete_and_retries_after_reset() {
    let image = dataset(&[30_000]);
    let expected = rows(&Rete::open(&image).unwrap(), &bound_query());
    assert_eq!(expected.len(), 1);

    let reader = FlakyReader::new(image);
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    // Resolve the terms first, so only the tile fetch meets the outage.
    assert!(rete.dictionary().subject_id(&node(100)).is_some());
    assert!(rete.dictionary().predicate_id("<http://ex/p>").is_some());
    reader.fail_from_now();
    let _ = eval_query(&rete, &bound_query()).unwrap();
    assert!(rete.index_incomplete());

    reader.recover();
    rete.reset_load_failures();
    assert!(!rete.index_incomplete());
    assert_eq!(rows(&rete, &bound_query()), expected);
    assert!(!rete.index_incomplete());
}

#[test]
fn lazy_named_graph_short_tile_read_sets_incomplete() {
    let image = dataset(&[30_000]);
    let reader = FlakyReader::new(image);
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let s = rete.dictionary().subject_id(&node(100)).unwrap();
    let p = rete.dictionary().predicate_id("<http://ex/p>").unwrap();
    let graph = rete.graph_index("<http://ex/g000>").unwrap();
    assert!(graph.is_lazy(), "the fixture must exercise the tile loader");

    reader.short_from_now();
    let first: Vec<_> = graph.scan_iter((Some(s), Some(p), None)).collect();
    assert!(first.is_empty());
    assert!(
        graph.load_incomplete(),
        "a short successful tile response is a failed load, not a shorter tile"
    );
    assert!(rete.index_incomplete());

    reader.recover();
    rete.reset_load_failures();
    let again: Vec<_> = graph.scan_iter((Some(s), Some(p), None)).collect();
    assert_eq!(again.len(), 1);
    assert!(!rete.index_incomplete());
}

/// Open succeeds (it reads no tile), and the first scan of the corrupt tile
/// marks the graph incomplete instead of reading it as an empty one.
fn assert_corrupt_named_tile_fails_on_first_scan(image: Vec<u8>) {
    let rete = Rete::open_ranged_lazy(FlakyReader::new(image))
        .expect("tile payload corruption must stay lazy at open");
    let q = "SELECT ?s ?p ?o WHERE { GRAPH <http://ex/g000> { ?s ?p ?o } } ORDER BY ?s ?p ?o";
    let _ = eval_query(&rete, q).unwrap();
    assert!(
        rete.graph_index("<http://ex/g000>")
            .unwrap()
            .load_incomplete(),
        "the first scan of the corrupt named tile must mark the graph incomplete"
    );
    assert!(rete.index_incomplete());
}

fn first_tile_of_g000(image: &[u8]) -> ByteRange {
    let layout = named_graph_layout(image);
    let g = layout
        .iter()
        .find(|g| g.name == "<http://ex/g000>")
        .unwrap();
    assert_eq!(g.sections.len(), g.tiles.len());
    let tile = g.tiles[0][0];
    assert!(tile.len > 0);
    tile
}

#[cfg(feature = "compression")]
#[test]
fn corrupt_named_compressed_tile_fails_on_first_scan_not_open() {
    let mut image = dataset(&[30_000]);
    let tile = first_tile_of_g000(&image);
    image[tile.offset as usize] ^= 0xff; // breaks the zstd frame magic
    assert_corrupt_named_tile_fails_on_first_scan(image);
}

#[cfg(not(feature = "compression"))]
#[test]
fn corrupt_named_raw_tile_fails_on_first_scan_not_open() {
    let mut image = dataset(&[30_000]);
    let tile = first_tile_of_g000(&image);
    let (start, end) = (tile.offset as usize, (tile.offset + tile.len) as usize);
    // A raw tile "decompresses" whatever its bytes are; only the zone-map
    // check in the loader can tell this one is not a tile.
    image[start..end].fill(0xff);
    assert_corrupt_named_tile_fails_on_first_scan(image);
}

#[test]
fn malformed_named_graph_framing_or_directory_fails_on_first_named_access() {
    let image = small_named_image();
    let layout = named_graph_layout(&image);

    // Framing: a NAMED_GRAPHS section one byte long cannot hold its records.
    let mut framing = image.clone();
    let mut header = Rete::open(&image).unwrap().header().clone();
    header.named_graphs_len = 1;
    framing[..HEADER_LEN].copy_from_slice(&header.to_bytes());
    let rete = Rete::open_ranged_lazy(FlakyReader::new(framing))
        .expect("the lazy open defers named-graph framing");
    assert!(rete.graph_names().is_empty());
    assert!(rete.index_incomplete());

    // Directory: ten continuation bytes where g000's first tile directory
    // starts make an unterminated varint.
    let mut directory = image;
    let dir = layout[0].sections[0];
    assert!(dir.len >= 10);
    directory[dir.offset as usize..dir.offset as usize + 10].fill(0x80);
    let rete = Rete::open_ranged_lazy(FlakyReader::new(directory))
        .expect("the lazy open defers named tile directories");
    assert!(rete.graph_index("<http://ex/g000>").is_none());
    assert!(rete.index_incomplete());
}

/// A truncated response on the walk of the NAMED_GRAPHS records themselves:
/// before, the walk sliced its buffer by the IRI length it had just parsed
/// and panicked when the read came back with only a couple of bytes.
#[test]
fn short_read_on_the_named_graph_walk_is_a_failed_load_not_a_panic() {
    let image = small_named_image();
    let reader = FlakyReader::new(image.clone());
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    // Two bytes: the count is already read, so these hold the first IRI's
    // length varint and the IRI's first byte.
    reader.truncate_to(2);
    let got = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rete.graph_names().len()));
    assert!(got.is_ok(), "a short walk read panicked");
    assert!(rete.index_incomplete());

    reader.recover();
    rete.reset_load_failures();
    let want = Rete::open(&image).unwrap();
    assert_eq!(rete.graph_names(), want.graph_names());
}
