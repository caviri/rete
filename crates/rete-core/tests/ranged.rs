//! Range-access invariants â€” the headline "give it a URL, fetch only what you
//! need" promise, asserted in code rather than only measured by the benchmark.
//!
//! A [`RecordingReader`] logs every byte range requested, so we can prove:
//!   * `SummaryView::open_ranged` reads only header + dictionary + summary and
//!     **never touches the triple-index byte range** (the "overview first" path);
//!   * `Rete::open_ranged` opens in a small bounded number of requests â€” never a
//!     linear scan proportional to the triple count.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use rete_core::{
    build_pyramid_meta, eval_query, validate_shacl, write_dataset, write_file,
    AdaptiveReadController, BlockCacheReader, ByteRange, CountingReader, DataGraph,
    DictionaryBuilder, GraphIndexBuilder, Header, QueryOutput, RangeReader, ReadIntent, Rete,
    ReteGraph, ShaclShapes, SliceReader, SummaryView, DEFAULT_TILE_BUDGET, HEADER_LEN,
};

/// A `RangeReader` over an in-memory image that records each `(offset, len)`,
/// and can be switched to fail every read (simulating a network outage
/// mid-session). `Sync` so it can back a lazily-faulting open.
struct RecordingReader {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    fail: AtomicBool,
    short: AtomicBool,
}

impl RecordingReader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            short: AtomicBool::new(false),
        }
    }
    fn reads(&self) -> Vec<(u64, u64)> {
        self.reads.lock().unwrap().clone()
    }
    fn bytes_read(&self) -> u64 {
        self.reads.lock().unwrap().iter().map(|&(_, l)| l).sum()
    }
    /// Make every subsequent `read_at` fail.
    fn fail_from_now(&self) {
        self.fail.store(true, Ordering::Relaxed);
    }
    /// End either simulated transport disruption; reads succeed exactly again.
    fn recover(&self) {
        self.fail.store(false, Ordering::Relaxed);
        self.short.store(false, Ordering::Relaxed);
    }
    /// Return one byte fewer than requested while still reporting success.
    fn short_from_now(&self) {
        self.short.store(true, Ordering::Relaxed);
    }
}

impl RangeReader for RecordingReader {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("simulated network failure"));
        }
        self.reads.lock().unwrap().push((offset, len));
        let start = offset as usize;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
        let returned_end = if self.short.load(Ordering::Relaxed) && len > 0 {
            end - 1
        } else {
            end
        };
        Ok(self.data[start..returned_end].to_vec())
    }
}

struct IntentReader<R> {
    inner: R,
    intents: Mutex<Vec<ReadIntent>>,
}

impl<R> IntentReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            intents: Mutex::new(Vec::new()),
        }
    }

    fn intents(&self) -> Vec<ReadIntent> {
        self.intents.lock().unwrap().clone()
    }
}

impl<R: RangeReader> RangeReader for IntentReader<R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_at(offset, len)
    }

    fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_at_precise(offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        self.inner.read_many(ranges)
    }

    fn read_many_with_intent(
        &self,
        ranges: &[(u64, u64)],
        intent: ReadIntent,
    ) -> std::io::Result<Vec<Vec<u8>>> {
        self.intents.lock().unwrap().push(intent);
        self.inner.read_many_with_intent(ranges, intent)
    }

    fn adaptive_controller(&self) -> Option<std::sync::Arc<AdaptiveReadController>> {
        self.inner.adaptive_controller()
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// A clustered graph that yields a real multi-community pyramid.
fn image_with_pyramid() -> Vec<u8> {
    let node = |n: u32| format!("<http://ex/n{n}>");
    let knows = "<http://ex/knows>".to_string();
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for c in 0..3u32 {
        let base = c * 10;
        for i in 0..10u32 {
            for j in 0..10u32 {
                if i != j {
                    edges.push((base + i, base + j));
                }
            }
        }
    }
    edges.push((0, 10)); // bridges
    edges.push((10, 20));

    let mut db = DictionaryBuilder::new();
    for &(s, o) in &edges {
        db.observe(&node(s), &knows, &node(o));
    }
    let dict = db.build();
    let ids: Vec<_> = edges
        .iter()
        .map(|&(s, o)| dict.encode(&node(s), &knows, &node(o)).unwrap())
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    write_file(&dict, &ib.build(), false, &meta, levels)
}

/// The index occupies `[root_dir_offset, root_dir_offset + root_dir_len)`.
fn index_region(image: &[u8]) -> (u64, u64) {
    let h = Rete::open(image).unwrap().header().clone();
    (h.root_dir_offset, h.root_dir_offset + h.root_dir_len)
}

fn overlaps(r: (u64, u64), region: (u64, u64)) -> bool {
    let (start, len) = r;
    let end = start + len;
    start < region.1 && region.0 < end
}

fn test_uvarint(bytes: &[u8], pos: &mut usize) -> u64 {
    let mut value = 0u64;
    for shift in (0..70).step_by(7) {
        let byte = bytes[*pos];
        *pos += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("test fixture contains an invalid varint")
}

/// Absolute compressed-payload ranges for the Subject family's SPO and SOP
/// siblings in a generation-0x06 image.
type PhysicalRanges = Vec<(u64, u64)>;

fn subject_family_payload_ranges(image: &[u8]) -> (PhysicalRanges, PhysicalRanges) {
    let header = Header::from_bytes(image).unwrap();
    assert_eq!(header.version, 0x06);
    let root_start = usize::try_from(header.root_dir_offset).unwrap();
    let root_end = usize::try_from(header.root_dir_offset + header.root_dir_len).unwrap();
    let root = &image[root_start..root_end];
    let mut root_pos = 0usize;
    assert_eq!(test_uvarint(root, &mut root_pos), 3);
    let subject_len = usize::try_from(test_uvarint(root, &mut root_pos)).unwrap();
    let subject_file_start = root_start + root_pos;
    let family = &root[root_pos..root_pos + subject_len];

    let mut pos = 0usize;
    let count = usize::try_from(test_uvarint(family, &mut pos)).unwrap();
    for _ in 0..count {
        let _ = test_uvarint(family, &mut pos); // leading minimum delta
        let _ = test_uvarint(family, &mut pos); // leading span
    }
    let mut first = Vec::with_capacity(count);
    let mut second = Vec::with_capacity(count);
    for _ in 0..count {
        let _ = test_uvarint(family, &mut pos); // flags
        let len = test_uvarint(family, &mut pos);
        let prefix = test_uvarint(family, &mut pos);
        first.push((prefix, len));
    }
    for _ in 0..count {
        let _ = test_uvarint(family, &mut pos); // flags
        let len = test_uvarint(family, &mut pos);
        let prefix = test_uvarint(family, &mut pos);
        second.push((prefix, len));
    }
    let records_start = pos;
    let physical = |records: &[(u64, u64)], cursor: &mut usize| {
        records
            .iter()
            .map(|&(prefix, len)| {
                let prefix = usize::try_from(prefix).unwrap();
                let len = usize::try_from(len).unwrap();
                let start = subject_file_start + *cursor + prefix;
                *cursor += prefix + len;
                (start as u64, len as u64)
            })
            .collect::<Vec<_>>()
    };
    let mut cursor = records_start;
    let first_ranges = physical(&first, &mut cursor);
    let second_ranges = physical(&second, &mut cursor);
    (first_ranges, second_ranges)
}

fn paired_ranged_image() -> Vec<u8> {
    let knows = "<http://ex/knows>";
    let triples: Vec<(String, String, String)> = (0..80)
        .map(|i| {
            (
                format!("<http://ex/person/{i:03}>"),
                knows.to_string(),
                format!(
                    "<http://ex/person/{:03}>",
                    if i == 2 { 1 } else { (i + 1) % 80 }
                ),
            )
        })
        .collect();
    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut index = GraphIndexBuilder::new().with_tile_budget(64);
    for (s, p, o) in &triples {
        index.push(dict.encode(s, p, o).unwrap());
    }
    write_file(&dict, &index.build(), false, &[], 0)
}

#[test]
fn paired_lazy_spo_query_never_reads_the_sop_payload() {
    let image = paired_ranged_image();
    let (_, sop_payloads) = subject_family_payload_ranges(&image);
    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let before = reader.reads().len();
    let rows = rete.query(Some("<http://ex/person/000>"), None, None);
    assert_eq!(rows.len(), 1);
    assert!(!rete.index_incomplete());
    for read in &reader.reads()[before..] {
        for &(offset, len) in &sop_payloads {
            assert!(
                !overlaps(*read, (offset, offset + len)),
                "SPO-only query read SOP payload {offset}..{} via {read:?}",
                offset + len
            );
        }
    }
}

/// Generation 0x06 frames each family's directory and synopsis trailer so a
/// lazy open can fetch those two metadata blobs directly.  Opening must stay
/// well below the old lengthless-varint walk's ~100 index requests while still
/// transferring no tile payload bytes.
#[test]
fn paired_lazy_open_batches_family_metadata() {
    let image = multi_tile_image();
    let (idx_start, idx_end) = index_region(&image);
    let reader = std::sync::Arc::new(RecordingReader::new(image));

    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert_eq!(rete.header().version, 0x06);

    let index_reads = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (idx_start, idx_end)))
        .count();
    assert!(
        index_reads <= 28,
        "paired lazy open issued {index_reads} index metadata reads; expected three framed \
         family directories and trailers"
    );
}

#[test]
fn summary_view_never_reads_the_index() {
    let image = image_with_pyramid();
    let (idx_start, idx_end) = index_region(&image);
    assert!(
        idx_end > idx_start,
        "test graph should have a non-empty index"
    );

    let reader = RecordingReader::new(image.clone());
    let view = SummaryView::open_ranged(&reader)
        .unwrap()
        .expect("has pyramid");

    let reads = reader.reads();
    // Header + dictionary + pyramid-meta: exactly three bounded requests.
    assert_eq!(
        reads.len(),
        3,
        "summary path should be 3 range reads, got {reads:?}"
    );
    // None of them may touch the triple index.
    for r in &reads {
        assert!(
            !overlaps(*r, (idx_start, idx_end)),
            "summary read {r:?} overlapped the index region [{idx_start}, {idx_end})"
        );
    }
    // And it pulls far less than the whole file (the index dominates size).
    assert!(
        reader.bytes_read() < image.len() as u64,
        "summary read {} of {} bytes â€” expected a strict subset",
        reader.bytes_read(),
        image.len()
    );

    // The cheap path is also correct: its per-predicate totals match a full read.
    let full = Rete::open(&image).unwrap();
    let knows_full = full.query(None, Some("<http://ex/knows>"), None).len() as u32;
    assert_eq!(view.predicate_total("<http://ex/knows>"), knows_full);
}

#[test]
fn summary_works_with_index_zeroed() {
    // The progressive browser path assembles a buffer with only header + dict +
    // summary populated and the index region absent. Prove the overview computes
    // correctly even when that region is overwritten with zeros â€” i.e. the
    // summary truly does not depend on the index bytes.
    let mut image = image_with_pyramid();
    let (idx_start, idx_end) = index_region(&image);
    for b in &mut image[idx_start as usize..idx_end as usize] {
        *b = 0;
    }

    let view = SummaryView::open_ranged(&SliceReader::new(&image))
        .unwrap()
        .expect("has pyramid");

    // Per-predicate totals from the summary must still match the intact file.
    let intact = Rete::open(&image_with_pyramid()).unwrap();
    let knows_intact = intact.query(None, Some("<http://ex/knows>"), None).len() as u32;
    assert_eq!(view.predicate_total("<http://ex/knows>"), knows_intact);
    assert!(knows_intact > 0);
}

#[test]
fn full_open_is_bounded_not_a_scan() {
    let image = image_with_pyramid();
    let reader = RecordingReader::new(image.clone());
    let rete = Rete::open_ranged(&reader).unwrap();

    // header, dictionary, index, pyramid-meta â€” at most these four (no named
    // graphs in this file). Crucially constant, not proportional to triples.
    let reads = reader.reads();
    assert!(
        reads.len() <= 4,
        "full ranged open should be â‰¤4 reads, got {} ({reads:?})",
        reads.len()
    );

    // Sanity: the ranged open reproduces the same data as a plain open.
    let plain = Rete::open(&image).unwrap();
    assert_eq!(rete.dump(None).len(), plain.dump(None).len());
}

#[test]
fn routed_pattern_query_fetches_only_the_selected_permutation() {
    let image = image_with_pyramid();
    let (idx_start, idx_end) = index_region(&image);
    let plain = Rete::open(&image).unwrap();
    let expected = plain.query(Some("<http://ex/n0>"), Some("<http://ex/knows>"), None);
    assert!(!expected.is_empty());

    let full_reader = RecordingReader::new(image.clone());
    let _ = Rete::open_ranged(&full_reader).unwrap();

    let reader = RecordingReader::new(image.clone());
    let got = Rete::query_ranged(
        &reader,
        Some("<http://ex/n0>"),
        Some("<http://ex/knows>"),
        None,
    )
    .unwrap();

    assert_eq!(got, expected);
    assert!(
        reader.bytes_read() < full_reader.bytes_read(),
        "routed pattern query read {} bytes; full ranged open read {} bytes",
        reader.bytes_read(),
        full_reader.bytes_read()
    );
    assert!(
        reader
            .reads()
            .iter()
            .any(|r| overlaps(*r, (idx_start, idx_end))),
        "the routed query should fetch the selected index permutation"
    );
    assert!(
        !reader.reads().contains(&(idx_start, idx_end - idx_start)),
        "routed query must not fetch the whole index container"
    );
}

/// A graph whose index sections *and* dictionary dwarf the 4 KiB directory
/// prefetches: many tiles (tiny tile budget) and long IRIs (multi-chunk dict
/// sections). ``QUERIED_NODE`` has exactly two
/// `knows` edges.
/// Node IRIs for [`multi_tile_image`]. Scrambled hex segments keep the
/// dictionary from front-coding/zstd-ing into a few KB — the lazy-dict
/// assertion needs real-sized sections.
fn mt_node(n: u32) -> String {
    format!(
        "<http://ex/n/{:08x}/{:08x}/{n:05}>",
        n.wrapping_mul(0x9E37_79B9),
        n.wrapping_mul(0x85EB_CA6B) ^ 0x5151_5151
    )
}

fn multi_tile_image() -> Vec<u8> {
    let node = mt_node;
    let knows = "<http://ex/knows>".to_string();
    let mut db = DictionaryBuilder::new();
    // Enough scrambled terms that each dict section spans many ~64 KiB chunks,
    // so "a few faulted chunks" is measurably smaller than the section.
    let edges: Vec<(u32, u32)> = (0..24000u32)
        .flat_map(|i| [(i, (i * 7 + 1) % 24000), (i, (i * 13 + 5) % 24000)])
        .collect();
    for &(s, o) in &edges {
        db.observe(&node(s), &knows, &node(o));
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
    for &(s, o) in &edges {
        ib.push(dict.encode(&node(s), &knows, &node(o)).unwrap());
    }
    write_file(&dict, &ib.build(), false, &[], 0)
}

#[derive(Debug)]
struct NamedGraphLayout {
    name: String,
    sections: [ByteRange; 6],
    tiles: [Vec<ByteRange>; 6],
}

/// Independent, test-only unsigned LEB128 walker. It deliberately does not use
/// rete's container/directory decoders: overlap assertions need an oracle whose
/// bounds calculations cannot share the production bug under test.
fn oracle_uvarint(image: &[u8], pos: &mut usize, end: usize) -> u64 {
    let mut value = 0u64;
    for shift in (0..=63).step_by(7) {
        let byte = *image
            .get(*pos)
            .filter(|_| *pos < end)
            .expect("fixture varint stays inside its declared section");
        *pos += 1;
        if shift == 63 {
            assert!(byte <= 1, "fixture varint overflows u64");
        }
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
    }
    panic!("fixture varint exceeds ten bytes");
}

fn oracle_bound(pos: usize, len: u64, end: usize) -> usize {
    pos.checked_add(usize::try_from(len).expect("fixture length fits usize"))
        .filter(|&next| next <= end)
        .expect("fixture field stays inside its declared section")
}

fn named_graph_layout(image: &[u8]) -> Vec<NamedGraphLayout> {
    let header = Header::from_bytes(&image[..HEADER_LEN]).unwrap();
    let mut pos = usize::try_from(header.named_graphs_offset).unwrap();
    let end = oracle_bound(pos, header.named_graphs_len, image.len());
    let graph_count = usize::try_from(oracle_uvarint(image, &mut pos, end)).unwrap();
    let mut graphs = Vec::with_capacity(graph_count);

    for _ in 0..graph_count {
        let iri_len = oracle_uvarint(image, &mut pos, end);
        let iri_end = oracle_bound(pos, iri_len, end);
        let name = String::from_utf8_lossy(&image[pos..iri_end]).into_owned();
        pos = iri_end;

        let container_len = oracle_uvarint(image, &mut pos, end);
        let container_end = oracle_bound(pos, container_len, end);
        let section_count = oracle_uvarint(image, &mut pos, container_end);
        let mut sections = [ByteRange { offset: 0, len: 0 }; 6];
        let mut tiles: [Vec<ByteRange>; 6] = Default::default();
        if header.version == 0x05 {
            assert_eq!(section_count, 6, "legacy fixture index has six sections");
            for section in &mut sections {
                let len = oracle_uvarint(image, &mut pos, container_end);
                let section_end = oracle_bound(pos, len, container_end);
                *section = ByteRange {
                    offset: pos as u64,
                    len,
                };
                pos = section_end;
            }
            tiles = std::array::from_fn(|section_index| {
                let section = sections[section_index];
                let mut dir_pos = usize::try_from(section.offset).unwrap();
                let section_end = oracle_bound(dir_pos, section.len, image.len());
                let tile_count =
                    usize::try_from(oracle_uvarint(image, &mut dir_pos, section_end)).unwrap();
                let mut lengths = Vec::with_capacity(tile_count);
                for _ in 0..tile_count {
                    let _delta_min = oracle_uvarint(image, &mut dir_pos, section_end);
                    let _span = oracle_uvarint(image, &mut dir_pos, section_end);
                    lengths.push(oracle_uvarint(image, &mut dir_pos, section_end));
                }
                lengths
                    .into_iter()
                    .map(|len| {
                        let tile_end = oracle_bound(dir_pos, len, section_end);
                        let range = ByteRange {
                            offset: dir_pos as u64,
                            len,
                        };
                        dir_pos = tile_end;
                        range
                    })
                    .collect()
            });
        } else {
            assert_eq!(section_count, 3, "paired fixture index has three families");
            for family in 0..3usize {
                let len = oracle_uvarint(image, &mut pos, container_end);
                let family_end = oracle_bound(pos, len, container_end);
                let physical = ByteRange {
                    offset: pos as u64,
                    len,
                };
                sections[family] = physical;
                sections[family + 3] = physical;

                let mut family_pos = pos;
                let tile_count =
                    usize::try_from(oracle_uvarint(image, &mut family_pos, family_end)).unwrap();
                let directory_len = oracle_uvarint(image, &mut family_pos, family_end);
                let trailer_len = oracle_uvarint(image, &mut family_pos, family_end);
                let directory_end = oracle_bound(family_pos, directory_len, family_end);
                for _ in 0..tile_count {
                    let _ = oracle_uvarint(image, &mut family_pos, directory_end);
                    let _ = oracle_uvarint(image, &mut family_pos, directory_end);
                }
                let mut first = Vec::with_capacity(tile_count);
                let mut second = Vec::with_capacity(tile_count);
                for target in [&mut first, &mut second] {
                    for _ in 0..tile_count {
                        let _flags = oracle_uvarint(image, &mut family_pos, directory_end);
                        let payload = oracle_uvarint(image, &mut family_pos, directory_end);
                        let prefix = oracle_uvarint(image, &mut family_pos, directory_end);
                        target.push(prefix + payload);
                    }
                }
                assert_eq!(family_pos, directory_end, "family directory is exact");
                for (slot, lengths) in [(family, first), (family + 3, second)] {
                    tiles[slot] = lengths
                        .into_iter()
                        .map(|record_len| {
                            let record_end = oracle_bound(family_pos, record_len, family_end);
                            let range = ByteRange {
                                offset: family_pos as u64,
                                len: record_len,
                            };
                            family_pos = record_end;
                            range
                        })
                        .collect();
                }
                assert_eq!(
                    oracle_bound(family_pos, trailer_len, family_end),
                    family_end,
                    "family trailer is exact"
                );
                pos = family_end;
            }
        }
        assert_eq!(
            pos, container_end,
            "fixture container has no trailing bytes"
        );
        graphs.push(NamedGraphLayout {
            name,
            sections,
            tiles,
        });
        pos = container_end;
    }
    graphs
}

fn named_graph_image() -> Vec<u8> {
    let p = "<http://ex/p>".to_string();
    let make = |graph: &str, count: u32| {
        (0..count)
            .map(|i| {
                (
                    format!("<http://ex/{graph}/s{i:04}>"),
                    p.clone(),
                    format!("<http://ex/{graph}/o{i:04}>"),
                )
            })
            .collect::<Vec<_>>()
    };
    let default = make("default", 96);
    let g1 = make("g1", 256);
    let g2 = make("g2", 256);

    let mut db = DictionaryBuilder::new();
    for triples in [&default, &g1, &g2] {
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
    }
    let dict = db.build();
    let build_index = |triples: &[(String, String, String)]| {
        let mut builder = GraphIndexBuilder::new().with_tile_budget(64);
        for (s, p, o) in triples {
            builder.push(dict.encode(s, p, o).unwrap());
        }
        builder.build()
    };
    let default_index = build_index(&default);
    let named = vec![
        ("<http://ex/g1>".to_string(), build_index(&g1)),
        ("<http://ex/g2>".to_string(), build_index(&g2)),
    ];
    assert!(
        named.iter().all(|(_, index)| index
            .tile_sections()
            .iter()
            .all(|section| section.len() > 1)),
        "fixture must be multi-tile in every named permutation"
    );
    write_dataset(&dict, &default_index, &named, true, &[], 0)
}

fn single_tile_named_graph_image() -> Vec<u8> {
    let triple = (
        "<http://ex/tiny-s>",
        "<http://ex/tiny-p>",
        "<http://ex/tiny-o>",
    );
    let mut db = DictionaryBuilder::new();
    db.observe(triple.0, triple.1, triple.2);
    let dict = db.build();
    let mut graph = GraphIndexBuilder::new();
    graph.push(dict.encode(triple.0, triple.1, triple.2).unwrap());
    write_dataset(
        &dict,
        &GraphIndexBuilder::new().build(),
        &[("<http://ex/tiny>".to_string(), graph.build())],
        true,
        &[],
        0,
    )
}

fn adaptive_shared_source_image() -> Vec<u8> {
    let p = "<http://ex/p>".to_string();
    let node = |scope: &str, n: u32| {
        format!(
            "<http://ex/{scope}/{:08x}/{:08x}/{n:05}>",
            n.wrapping_mul(0x9E37_79B9),
            n.wrapping_mul(0x85EB_CA6B) ^ 0x5151_5151
        )
    };
    let default: Vec<_> = (0..8000u32)
        .flat_map(|i| {
            [
                (
                    node("default", i),
                    p.clone(),
                    node("default", (i + 1) % 8000),
                ),
                (
                    node("default", i),
                    p.clone(),
                    node("default", (i + 17) % 8000),
                ),
            ]
        })
        .collect();
    let named: Vec<_> = (0..512u32)
        .map(|i| (node("named", i), p.clone(), node("named", (i + 1) % 512)))
        .collect();

    let mut db = DictionaryBuilder::new();
    for (s, p, o) in default.iter().chain(&named) {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let build_index = |triples: &[(String, String, String)]| {
        let mut builder = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in triples {
            builder.push(dict.encode(s, p, o).unwrap());
        }
        builder.build()
    };
    write_dataset(
        &dict,
        &build_index(&default),
        &[("<http://ex/named>".to_string(), build_index(&named))],
        true,
        &[],
        0,
    )
}

#[test]
fn lazy_open_reads_no_named_tile_payloads_before_named_catalog_access() {
    let image = named_graph_image();
    let layout = named_graph_layout(&image);
    assert!(
        layout
            .iter()
            .flat_map(|graph| graph.tiles.iter().flatten())
            .count()
            > 12,
        "fixture must expose many named tile payloads"
    );

    let eager_reader = RecordingReader::new(image.clone());
    let eager = Rete::open_ranged(&eager_reader).unwrap();
    assert_eq!(
        eager.graph_names(),
        vec!["<http://ex/g1>", "<http://ex/g2>"]
    );
    let eager_reads = eager_reader.reads();
    assert!(
        layout
            .iter()
            .flat_map(|graph| graph.tiles.iter().flatten())
            .all(|tile| {
                eager_reads
                    .iter()
                    .any(|&read| overlaps(read, (tile.offset, tile.end())))
            }),
        "eager ranged open must retain resident named payload behavior: {eager_reads:?}"
    );

    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let reads = reader.reads();
    for graph in &layout {
        for tile in graph.tiles.iter().flatten() {
            assert!(
                !reads
                    .iter()
                    .any(|&read| overlaps(read, (tile.offset, tile.end()))),
                "lazy open read named graph {} tile {tile:?}: {reads:?}",
                graph.name
            );
        }
    }
    assert_eq!(rete.graph_names(), vec!["<http://ex/g1>", "<http://ex/g2>"]);
}

#[test]
fn lazy_open_through_block_cache_physically_skips_named_tile_payloads() {
    let image = named_graph_image();
    let layout = named_graph_layout(&image);
    let physical = std::sync::Arc::new(RecordingReader::new(image));
    let cached = std::sync::Arc::new(BlockCacheReader::new(physical.clone(), 4096));

    let rete = Rete::open_ranged_lazy(cached).unwrap();
    let reads = physical.reads();
    for graph in &layout {
        for tile in graph.tiles.iter().flatten() {
            assert!(
                !reads
                    .iter()
                    .any(|&read| overlaps(read, (tile.offset, tile.end()))),
                "physical cache fetch read named graph {} tile {tile:?}: {reads:?}",
                graph.name
            );
        }
    }
    assert_eq!(rete.graph_names(), vec!["<http://ex/g1>", "<http://ex/g2>"]);
}

#[test]
fn adaptive_default_named_and_dictionary_reads_train_one_source_controller() {
    let image = adaptive_shared_source_image();
    let layout = named_graph_layout(&image);
    let physical = std::sync::Arc::new(RecordingReader::new(image));
    let now = std::sync::Arc::new(AtomicU64::new(0));
    let tick = now.clone();
    let cache = BlockCacheReader::new(physical.clone(), 4096)
        .with_adaptive_clock(move || Some(tick.fetch_add(100_000, Ordering::SeqCst)));
    let controller = cache.adaptive_controller().unwrap();
    let cached = std::sync::Arc::new(cache);

    let rete = Rete::open_ranged_lazy(cached).unwrap();
    assert_eq!(controller.successful_samples(), 0);
    for tile in layout.iter().flat_map(|graph| graph.tiles.iter().flatten()) {
        assert!(
            !physical
                .reads()
                .iter()
                .any(|&read| overlaps(read, (tile.offset, tile.end()))),
            "adaptive lazy open crossed named tile payload {tile:?}"
        );
    }

    assert_eq!(rete.default_index().triple_count(), 16_000);
    let after_default = controller.successful_samples();
    assert!(
        after_default > 0,
        "default tiles did not train the controller"
    );

    assert_eq!(
        rete.graph_index("<http://ex/named>")
            .unwrap()
            .triple_count(),
        512
    );
    let after_named = controller.successful_samples();
    assert!(
        after_named > after_default,
        "named tiles did not advance the shared controller"
    );

    rete.dictionary().prefetch_all();
    assert!(
        controller.successful_samples() > after_named,
        "dictionary batches did not advance the shared controller"
    );
    assert!(!rete.index_incomplete());
}

#[test]
fn lazy_open_does_not_probe_past_tiny_named_graph_framing() {
    let image = single_tile_named_graph_image();
    let layout = named_graph_layout(&image);
    assert_eq!(layout.len(), 1);
    assert!(
        layout[0].tiles.iter().all(|tiles| tiles.len() == 1),
        "tiny fixture must have exactly one tile per permutation"
    );

    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let reads = reader.reads();
    for tile in layout[0].tiles.iter().flatten() {
        assert!(
            !reads
                .iter()
                .any(|&read| overlaps(read, (tile.offset, tile.end()))),
            "framing probe crossed tiny tile payload {tile:?}: {reads:?}"
        );
    }
    assert_eq!(rete.graph_names(), vec!["<http://ex/tiny>"]);
}

#[test]
fn lazy_tiny_open_through_block_cache_physically_skips_named_tile_payloads() {
    let image = single_tile_named_graph_image();
    let layout = named_graph_layout(&image);
    let physical = std::sync::Arc::new(RecordingReader::new(image));
    let cached = std::sync::Arc::new(BlockCacheReader::new(physical.clone(), 4096));

    let rete = Rete::open_ranged_lazy(cached).unwrap();
    let reads = physical.reads();
    for tile in layout[0].tiles.iter().flatten() {
        assert!(
            !reads
                .iter()
                .any(|&read| overlaps(read, (tile.offset, tile.end()))),
            "physical opening read crossed header-adjacent tiny tile {tile:?}: {reads:?}"
        );
    }
    assert_eq!(rete.graph_names(), vec!["<http://ex/tiny>"]);
}

#[test]
fn lazy_named_graph_queries_match_eager_for_graph_from_and_graph_variable() {
    let image = named_graph_image();
    let eager = Rete::open(&image).unwrap();
    let lazy = Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(image))).unwrap();
    let queries = [
        "SELECT ?s ?o WHERE { GRAPH <http://ex/g1> { ?s <http://ex/p> ?o } } ORDER BY ?s ?o",
        "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/p> ?o } } ORDER BY ?g ?s",
        "SELECT ?s ?o FROM <http://ex/g1> WHERE { ?s <http://ex/p> ?o } ORDER BY ?s ?o",
    ];

    for query in queries {
        match (
            eval_query(&eager, query).unwrap(),
            eval_query(&lazy, query).unwrap(),
        ) {
            (QueryOutput::Select(want_vars, want), QueryOutput::Select(got_vars, got)) => {
                assert_eq!(got_vars, want_vars, "variables differ for {query}");
                assert_eq!(got, want, "rows differ for {query}");
            }
            (want, got) => panic!("unexpected outputs for {query}: {want:?} vs {got:?}"),
        }
        assert!(!lazy.index_incomplete(), "lazy query failed: {query}");
    }
}

#[test]
fn lazy_named_graph_query_fetches_only_the_routed_tiles() {
    // The production lazy named-graph path intentionally keeps containers up
    // to 1 MiB resident. Inflate this graph beyond that threshold so this test
    // exercises tile routing rather than the small-container fast path.
    let image = dataset_with_named_graphs_budget(&[30_000], Some(64));
    let layout = named_graph_layout(&image);
    let g1 = layout
        .iter()
        .find(|graph| graph.name == "<http://ex/g000>")
        .unwrap();
    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let before = reader.reads().len();

    let query = format!(
        "SELECT ?o WHERE {{ GRAPH <http://ex/g000> {{ {} <http://ex/p> ?o }} }}",
        mt_node(100)
    );
    let rows = match eval_query(&rete, &query).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(rows.len(), 1);
    assert!(!rete.index_incomplete());

    let query_reads = &reader.reads()[before..];
    let touched_g1 = g1
        .tiles
        .iter()
        .flatten()
        .filter(|tile| {
            query_reads
                .iter()
                .any(|&read| overlaps(read, (tile.offset, tile.end())))
        })
        .count();
    let g1_tiles = g1.tiles.iter().flatten().count();
    assert!(touched_g1 > 0, "query must fetch a routed g1 tile");
    assert!(
        touched_g1 < g1_tiles,
        "bound query fetched all {g1_tiles} g1 tiles"
    );
}

#[test]
fn lazy_named_graph_tile_failure_sets_incomplete_and_retries_after_reset() {
    let image = dataset_with_named_graphs_budget(&[30_000], Some(64));
    let query = format!(
        "SELECT ?o WHERE {{ GRAPH <http://ex/g000> {{ {} <http://ex/p> ?o }} }}",
        mt_node(100)
    );
    let expected = match eval_query(&Rete::open(&image).unwrap(), &query).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(expected.len(), 1);

    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert!(rete.dictionary().subject_id(&mt_node(100)).is_some());
    assert!(rete.dictionary().predicate_id("<http://ex/p>").is_some());
    reader.fail_from_now();
    let _ = eval_query(&rete, &query).unwrap();
    assert!(rete.index_incomplete());

    reader.recover();
    rete.reset_load_failures();
    assert!(!rete.index_incomplete());
    let got = match eval_query(&rete, &query).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(got, expected);
    assert!(!rete.index_incomplete());
}

#[test]
fn lazy_named_graph_short_tile_read_sets_incomplete() {
    let image = dataset_with_named_graphs_budget(&[30_000], Some(64));
    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let subject = rete.dictionary().subject_id(&mt_node(100)).unwrap();
    let predicate = rete.dictionary().predicate_id("<http://ex/p>").unwrap();
    let graph = rete.graph_index("<http://ex/g000>").unwrap();

    reader.short_from_now();
    let first = graph
        .scan_iter((Some(subject), Some(predicate), None))
        .collect::<Vec<_>>();

    assert!(first.is_empty());
    assert!(
        graph.load_incomplete(),
        "a short successful tile response must be a failed lazy load"
    );
    assert!(rete.index_incomplete());

    reader.recover();
    rete.reset_load_failures();
    let recovered = graph
        .scan_iter((Some(subject), Some(predicate), None))
        .collect::<Vec<_>>();
    assert_eq!(recovered.len(), 1);
    assert!(!rete.index_incomplete());
}

fn assert_corrupt_named_tile_is_lazy(image: Vec<u8>) {
    let rete = Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(image)))
        .expect("tile payload corruption must remain lazy at open");
    let query = "SELECT ?s ?p ?o WHERE { GRAPH <http://ex/g000> { ?s ?p ?o } } ORDER BY ?s ?p ?o";
    let _ = eval_query(&rete, query).unwrap();
    assert!(
        rete.graph_index("<http://ex/g000>")
            .unwrap()
            .load_incomplete(),
        "first scan of the corrupt named tile must mark the graph incomplete"
    );
    assert!(rete.index_incomplete());
}

#[cfg(feature = "compression")]
#[test]
fn corrupt_named_compressed_tile_fails_on_first_scan_not_open() {
    let mut image = dataset_with_named_graphs_budget(&[30_000], Some(64));
    let layout = named_graph_layout(&image);
    let corrupt = layout
        .iter()
        .find(|graph| graph.name == "<http://ex/g000>")
        .unwrap()
        .tiles[0][0];
    assert!(corrupt.len > 0);
    image[corrupt.offset as usize] ^= 0xff;

    assert_corrupt_named_tile_is_lazy(image);
}

#[cfg(not(feature = "compression"))]
#[test]
fn corrupt_named_raw_tile_fails_on_first_scan_not_open() {
    let mut image = dataset_with_named_graphs_budget(&[30_000], Some(64));
    let layout = named_graph_layout(&image);
    let corrupt = layout
        .iter()
        .find(|graph| graph.name == "<http://ex/g000>")
        .unwrap()
        .tiles[0][0];
    assert!(corrupt.len > 0);
    let corrupt_end = usize::try_from(corrupt.end()).unwrap();
    image[corrupt.offset as usize..corrupt_end].fill(0xff);

    assert_corrupt_named_tile_is_lazy(image);
}

#[test]
fn malformed_named_graph_framing_or_directory_fails_on_first_named_access() {
    let image = named_graph_image();
    let layout = named_graph_layout(&image);

    let mut malformed_framing = image.clone();
    let mut header = Rete::open(&image).unwrap().header().clone();
    header.named_graphs_len = 1;
    malformed_framing[..HEADER_LEN].copy_from_slice(&header.to_bytes());
    let malformed =
        Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(malformed_framing)))
            .expect("lazy open must defer named-graph framing");
    assert!(malformed.graph_names().is_empty());
    assert!(malformed.index_incomplete());

    let mut malformed_directory = image;
    let directory = layout[0].sections[0];
    assert!(directory.len >= 10);
    malformed_directory[directory.offset as usize..directory.offset as usize + 10].fill(0x80);
    let malformed = Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(
        malformed_directory,
    )))
    .expect("lazy open must defer named tile directories");
    assert!(malformed.graph_index("<http://ex/g1>").is_none());
    assert!(malformed.index_incomplete());
}

/// On a tiled (v0.2) multi-tile file, a bound-subject routed query must fetch
/// only the tile **directory** plus the one matching tile â€” a small fraction
/// of the selected permutation section, not the whole section.
#[test]
fn routed_pattern_query_fetches_only_matching_tiles() {
    let image = multi_tile_image();
    let (idx_start, idx_end) = index_region(&image);
    let index_len = idx_end - idx_start;

    let plain = Rete::open(&image).unwrap();
    let n7 = mt_node(7);
    let expected = plain.query(Some(&n7), None, None);
    assert_eq!(expected.len(), 2);

    let reader = RecordingReader::new(image.clone());
    let got = Rete::query_ranged(&reader, Some(&n7), None, None).unwrap();
    assert_eq!(got, expected);

    // Bytes fetched from inside the index region: directory prefix + 1 tile.
    let index_bytes_read: u64 = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (idx_start, idx_end)))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        index_bytes_read < index_len / 6,
        "tile-routed query read {index_bytes_read} of {index_len} index bytes â€” \
         expected directory + one tile, a small fraction of one section"
    );
}

/// Full SPARQL over a lazily-faulting ranged open: a selective query must
/// fault in only the tiles its scans touch â€” directory prefixes + a couple of
/// tiles, a small fraction of the index â€” while returning exactly the same
/// rows as an in-memory open.
#[test]
fn lazy_sparql_open_fetches_only_touched_tiles() {
    let image = multi_tile_image();
    let (idx_start, idx_end) = index_region(&image);
    let index_len = idx_end - idx_start;

    let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
    let plain = Rete::open(&image).unwrap();
    let expected = match eval_query(&plain, &q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(expected.len(), 2);

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let got = match eval_query(&rete, &q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(got, expected);
    assert!(!rete.index_incomplete());

    let index_bytes_read: u64 = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (idx_start, idx_end)))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        index_bytes_read < index_len / 4,
        "lazy SPARQL read {index_bytes_read} of {index_len} index bytes â€” \
         expected tile directories plus the touched tiles only"
    );

    // The dictionary is also lazy (chunked): the query resolves a couple of
    // constant terms and a couple of output terms, so it must fetch the
    // section headers + chunk directories plus a few chunks â€” not the whole
    // dictionary container.
    let h = plain.header().clone();
    let (dict_start, dict_end) = (h.dictionary_offset, h.dictionary_offset + h.dictionary_len);
    let dict_bytes_read: u64 = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (dict_start, dict_end)))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        dict_bytes_read < h.dictionary_len / 2,
        "lazy SPARQL read {dict_bytes_read} of {} dictionary bytes â€” \
         expected directories plus a few chunks only",
        h.dictionary_len
    );
}

/// The pyramid meta (community structure) is large on real graphs and SPARQL
/// never reads it, so a lazy open must NOT fetch it — neither on open nor while
/// evaluating a query. It must still fault in when `pyramid()` is actually
/// called (community / pyramid_tree / inspect queries).
#[test]
fn lazy_open_defers_the_pyramid_until_needed() {
    let image = image_with_pyramid();
    let h = Rete::open(&image).unwrap().header().clone();
    let pyr = (
        h.pyramid_meta_offset,
        h.pyramid_meta_offset + h.pyramid_meta_len,
    );
    assert!(pyr.1 > pyr.0, "fixture should carry a pyramid");

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();

    // Open + a bound SPARQL query: the pyramid region must stay untouched.
    let q = "SELECT ?o WHERE { <http://ex/n0> <http://ex/knows> ?o }";
    let _ = eval_query(&rete, q).unwrap();
    assert!(
        !reader.reads().iter().any(|r| overlaps(*r, pyr)),
        "lazy SPARQL open/eval fetched the pyramid region {pyr:?}: {:?}",
        reader.reads()
    );

    // Asking for the pyramid faults it in (and matches a full open).
    let got = rete.pyramid().expect("pyramid faults in on demand");
    let want = Rete::open(&image).unwrap();
    assert_eq!(got.summary.len(), want.pyramid().unwrap().summary.len());
    assert!(!got.summary.is_empty(), "fixture pyramid has super-edges");
    assert!(
        reader.reads().iter().any(|r| overlaps(*r, pyr)),
        "pyramid() should have fetched the pyramid region {pyr:?}"
    );
}

/// A full unbound scan over a lazily-opened multi-tile file must coalesce its
/// tile fetches: adjacent tile ranges batch into single range reads, so the
/// request count stays a small constant instead of one per tile.
#[test]
fn full_scan_coalesces_tile_fetches() {
    let image = multi_tile_image();
    let (idx_start, idx_end) = index_region(&image);

    let plain = Rete::open(&image).unwrap();
    let spo_tiles = plain.default_index().tile_sections()[0].len();
    assert!(
        spo_tiles > 100,
        "expected a many-tile fixture, got {spo_tiles}"
    );
    // Fully unbound: routes to SPO and must visit every one of its tiles.
    let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o }";
    let expected = match eval_query(&plain, q).unwrap() {
        QueryOutput::Select(_, rows) => rows.len(),
        other => panic!("unexpected output {other:?}"),
    };
    assert!(expected > 40_000);

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let got = match eval_query(&rete, q).unwrap() {
        QueryOutput::Select(_, rows) => rows.len(),
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(got, expected);
    assert!(!rete.index_incomplete());

    let index_reads = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (idx_start, idx_end)))
        .count();
    // The scan ramps its prefetch window geometrically (4, 8, 16, … tiles),
    // so a full sweep costs O(log tiles) coalesced batch reads plus the tile
    // directories — a small constant, NOT one read per tile (which over 700+
    // tiles would be in the hundreds).
    // The three length-framed paired-family directories and synopsis trailers
    // are fetched exactly at open; the remaining reads are the SPO scan's
    // O(log n) coalesced tile batches.  The measured transition cost is 34,
    // and this bound leaves modest transport variation without regressing to
    // the old lengthless metadata walk.
    assert!(
        index_reads < 48,
        "full scan issued {index_reads} index-region reads over {spo_tiles} tiles — \
         expected three framed family directories plus O(log n) coalesced batches"
    );
}

/// A small `LIMIT` on an unbound scan must not fault the whole index: the
/// geometric prefetch ramp stops as soon as the pipeline stops pulling rows,
/// so `LIMIT 1` fetches only the first window of tiles, not every tile.
#[test]
fn small_limit_does_not_fetch_the_whole_index() {
    let image = multi_tile_image();
    let (idx_start, idx_end) = index_region(&image);
    let index_len = idx_end - idx_start;
    let spo_tiles = Rete::open(&image).unwrap().default_index().tile_sections()[0].len();
    assert!(
        spo_tiles > 100,
        "expected a many-tile fixture, got {spo_tiles}"
    );

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1";
    let rows = match eval_query(&rete, q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(rows.len(), 1);
    assert!(!rete.index_incomplete());

    let index_bytes_read: u64 = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, (idx_start, idx_end)))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        index_bytes_read < index_len / 4,
        "LIMIT 1 read {index_bytes_read} of {index_len} index bytes over \
         {spo_tiles} tiles — expected only the first prefetch window"
    );
}

#[test]
fn streaming_limit_is_bounded_but_order_and_aggregate_are_full_scans() {
    let image = multi_tile_image();
    let run = |query: &str| {
        let physical = std::sync::Arc::new(RecordingReader::new(image.clone()));
        let now = std::sync::Arc::new(AtomicU64::new(0));
        let tick = now.clone();
        let cached = BlockCacheReader::new(physical, 4096)
            .with_adaptive_clock(move || Some(tick.fetch_add(100_000, Ordering::SeqCst)));
        let intent_reader = std::sync::Arc::new(IntentReader::new(cached));
        let rete = Rete::open_ranged_lazy(intent_reader.clone()).unwrap();
        let _ = eval_query(&rete, query).unwrap();
        assert!(!rete.index_incomplete());
        intent_reader.intents()
    };

    let bounded = run("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1");
    assert!(
        bounded.contains(&ReadIntent::BoundedScan),
        "streaming LIMIT intents: {bounded:?}"
    );

    let ordered = run("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s LIMIT 1");
    assert!(
        ordered.contains(&ReadIntent::FullScan),
        "ORDER BY must consume the scan before LIMIT: {ordered:?}"
    );
    assert!(
        !ordered.contains(&ReadIntent::BoundedScan),
        "ORDER BY was mislabeled as streaming: {ordered:?}"
    );

    let aggregate = run("SELECT (COUNT(*) AS ?count) WHERE { ?s ?p ?o }");
    assert!(
        aggregate.contains(&ReadIntent::FullScan),
        "aggregate must consume the complete scan: {aggregate:?}"
    );
    assert!(
        !aggregate.contains(&ReadIntent::BoundedScan),
        "aggregate was mislabeled as bounded: {aggregate:?}"
    );
}

/// Resolving a many-row result must coalesce its dictionary faults: the engine
/// gathers the bounded page's term IDs and batch-faults their chunks in a few
/// coalesced range reads, instead of one fetch per distinct term (which over a
/// remote file is hundreds of sequential requests).
#[test]
fn multi_term_output_coalesces_dictionary_faults() {
    let image = multi_tile_image();
    let h = Rete::open(&image).unwrap().header().clone();
    let dict = (h.dictionary_offset, h.dictionary_offset + h.dictionary_len);

    // 400 rows, each a distinct (subject, object) of scrambled IRIs — their
    // terms land in many chunks spread across the dictionary sections.
    let q = "SELECT ?s ?o WHERE { ?s <http://ex/knows> ?o } LIMIT 400";
    let plain = Rete::open(&image).unwrap();
    let expected = match eval_query(&plain, q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(expected.len(), 400);

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let got = match eval_query(&rete, q).unwrap() {
        QueryOutput::Select(_, rows) => rows,
        other => panic!("unexpected output {other:?}"),
    };
    assert_eq!(got, expected);
    assert!(!rete.index_incomplete());

    // 800 cells (400 subjects + 400 objects) resolve in far fewer than 800
    // dictionary reads — the per-section chunk batches coalesce adjacent runs.
    let dict_reads = reader
        .reads()
        .iter()
        .filter(|r| overlaps(**r, dict))
        .count();
    assert!(
        dict_reads < 24,
        "resolving 800 output terms issued {dict_reads} dictionary reads — \
         expected a few coalesced chunk batches, not one per term/chunk"
    );
}

/// `dump` resolves every triple and every term: over a lazy open it must
/// batch-fault the dictionary chunks and the scanned tiles in coalesced
/// reads — a bounded number of requests, never one per chunk/tile.
#[test]
fn dump_over_lazy_open_coalesces_fetches() {
    let image = multi_tile_image();
    let plain = Rete::open(&image).unwrap();
    let expected = plain.dump(None);

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let got = rete.dump(None);
    assert_eq!(got, expected);
    assert!(!rete.index_incomplete());

    let total_reads = reader.reads().len();
    assert!(
        total_reads < 80,
        "lazy dump issued {total_reads} range reads — expected three exact paired-family \
         directories plus coalesced chunk batches and O(log n) tile-prefetch batches"
    );
}

/// A range failure during lazy tile faulting must be detectable: the engine's
/// scans stay infallible (they see an empty tile), but the sticky
/// `index_incomplete` flag turns the partial answer into an error upstream.
#[test]
fn lazy_sparql_open_surfaces_failed_tile_fetches() {
    let image = multi_tile_image();
    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert!(!rete.index_incomplete());

    // The network dies after the open; the next query's chunk/tile faults fail.
    reader.fail_from_now();
    let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
    let _ = eval_query(&rete, &q).unwrap(); // evaluation itself must not panic
    assert!(
        rete.index_incomplete(),
        "a failed tile fetch must set the incomplete flag"
    );
}

/// The incompleteness verdict is PER QUERY for a resident session: after the
/// network recovers, `reset_load_failures` clears the sticky flag and the same
/// query RE-FETCHES what failed (failed tiles/chunks are never cached as
/// empty), returning the complete answer — one transient blip must not poison
/// every later query on a long-lived handle (the browser worker's session).
#[test]
fn reset_load_failures_makes_failed_fetches_retryable() {
    let image = multi_tile_image();
    let expected = {
        // The complete answer, computed over a healthy session.
        let healthy =
            Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(image.clone())))
                .unwrap();
        let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
        match eval_query(&healthy, &q).unwrap() {
            QueryOutput::Select(_, rows) => rows.len(),
            _ => unreachable!(),
        }
    };
    assert!(expected > 0, "fixture must produce rows");

    let reader = std::sync::Arc::new(RecordingReader::new(image));
    let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    reader.fail_from_now();
    let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
    let _ = eval_query(&rete, &q).unwrap();
    assert!(rete.index_incomplete(), "outage query must be flagged");

    reader.recover();
    rete.reset_load_failures();
    assert!(!rete.index_incomplete(), "reset must clear the verdict");
    let rows = match eval_query(&rete, &q).unwrap() {
        QueryOutput::Select(_, rows) => rows.len(),
        _ => unreachable!(),
    };
    assert!(
        !rete.index_incomplete(),
        "recovered query must not re-flag — its fetches succeeded"
    );
    assert_eq!(
        rows, expected,
        "the retried query must return the COMPLETE answer, not a poisoned empty tile"
    );
}

#[test]
fn routed_pattern_query_with_unknown_term_skips_the_index() {
    let image = image_with_pyramid();
    let (idx_start, idx_end) = index_region(&image);
    let reader = RecordingReader::new(image);

    let got = Rete::query_ranged(
        &reader,
        Some("<http://ex/missing>"),
        Some("<http://ex/knows>"),
        None,
    )
    .unwrap();

    assert!(got.is_empty());
    for r in reader.reads() {
        assert!(
            !overlaps(r, (idx_start, idx_end)),
            "unknown-term query read index range {r:?}"
        );
    }
}

/// SHACL validation over a **lazy** open ([`ReteGraph`]) must (1) produce the
/// same report as the eager in-memory [`DataGraph`] path and (2) fetch only the
/// shapes' targets — a few type/email tiles — not the whole graph. A few `Person`
/// targets (one with a bad email) are buried in thousands of unrelated triples.
#[test]
fn shacl_over_lazy_open_matches_eager_and_fetches_only_targets() {
    const TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    const PERSON: &str = "<http://ex/Person>";
    const EMAIL: &str = "<http://ex/email>";

    let mut triples: Vec<(String, String, String)> = Vec::new();
    for i in 0..6 {
        let p = format!("<http://ex/p{i}>");
        triples.push((p.clone(), TYPE.to_string(), PERSON.to_string()));
        let email = if i == 3 {
            "\"bad\"".to_string() // fails the pattern → a violation on both paths
        } else {
            format!("\"p{i}@ex.org\"")
        };
        triples.push((p, EMAIL.to_string(), email));
    }
    // Filler so the index is many-tiled and a full read would be expensive.
    for i in 0..3000u32 {
        triples.push((
            format!("<http://ex/n{i:05}>"),
            "<http://ex/knows>".to_string(),
            format!("<http://ex/n{:05}>", (i + 1) % 3000),
        ));
    }

    let mut db = DictionaryBuilder::new();
    for (s, p, o) in &triples {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let ids: Vec<(u32, u32, u32)> = triples
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
        .collect();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(2048);
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    let image = write_file(&dict, &ib.build(), false, &meta, levels);

    let shapes = ShaclShapes::parse_turtle(
        r#"
        @prefix ex: <http://ex/> .
        @prefix sh: <http://www.w3.org/ns/shacl#> .
        ex:PersonShape a sh:NodeShape ;
          sh:targetClass ex:Person ;
          sh:property [ sh:path ex:email ; sh:minCount 1 ; sh:pattern "^[^@]+@[^@]+$" ] .
        "#,
    )
    .unwrap();

    // Eager: whole graph in memory.
    let eager_rete = Rete::open(&image).unwrap();
    let eager = validate_shacl(&DataGraph::from_rete(&eager_rete, None), &shapes);

    // Lazy: validate over a range reader, measuring the bytes the validation pulls.
    let leaked: &'static [u8] = Box::leak(image.clone().into_boxed_slice());
    let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
    let lazy_rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
    let before = reader.bytes_read();
    let lazy = validate_shacl(&ReteGraph::new(&lazy_rete), &shapes);
    let pulled = reader.bytes_read() - before;
    assert!(!lazy_rete.index_incomplete(), "no lazy fetch failed");

    // Same verdict, same violations.
    assert!(!eager.conforms, "the bad email must fail the pattern");
    assert_eq!(eager.conforms, lazy.conforms);
    assert_eq!(eager.results.len(), lazy.results.len());

    // The targeted validation pulled far less than the whole file.
    assert!(
        (pulled as usize) < image.len() / 3,
        "lazy SHACL pulled {pulled} B of a {} B file — expected only the type/email tiles",
        image.len()
    );
}

/// A split mega-group (one predicate far over the tile budget) must stay fully
/// queryable through the LAZY open: a bound (p, o) probe whose object sits in a
/// LATE slice of the split run must find its subjects. (Regression: the first
/// split file returned 0 for exactly this shape while plain opens passed.)
#[test]
fn lazy_bound_object_lookup_reaches_late_slices_of_a_split_group() {
    let node = mt_node;
    let cites = "<http://ex/cites>".to_string();
    let mut db = DictionaryBuilder::new();
    let edges: Vec<(u32, u32)> = (0..8000u32).map(|i| (i % 400, 10_000 + i)).collect();
    for &(s, o) in &edges {
        db.observe(&node(s), &cites, &node(o));
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
    for &(s, o) in &edges {
        ib.push(dict.encode(&node(s), &cites, &node(o)).unwrap());
    }
    let image = write_file(&dict, &ib.build(), false, &[], 0);

    let plain = Rete::open(&image).unwrap();
    let lazy =
        Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(image.clone()))).unwrap();
    for i in [0u32, 4_000, 7_999] {
        let q = format!(
            "SELECT ?s WHERE {{ ?s <http://ex/cites> {} }}",
            node(10_000 + i)
        );
        let want = match eval_query(&plain, &q).unwrap() {
            QueryOutput::Select(_, r) => r.len(),
            _ => unreachable!(),
        };
        assert_eq!(want, 1, "plain open must find o=10{i}");
        let got = match eval_query(&lazy, &q).unwrap() {
            QueryOutput::Select(_, r) => r.len(),
            _ => unreachable!(),
        };
        assert!(!lazy.index_incomplete());
        assert_eq!(got, want, "LAZY open must find o offset {i} in its slice");
    }
}

/// A dataset image with one small default graph and `n` named graphs; graph
/// `k` holds `sizes[k]` triples. All graphs share one dictionary.
fn dataset_with_named_graphs(sizes: &[usize]) -> Vec<u8> {
    dataset_with_named_graphs_budget(sizes, None)
}

/// [`dataset_with_named_graphs`] with a per-graph tile budget override — a
/// tiny budget inflates each graph's container (many tiles + directories),
/// the cheap way to build multi-MB tile-lazy containers in a test.
fn dataset_with_named_graphs_budget(sizes: &[usize], tile_budget: Option<usize>) -> Vec<u8> {
    let node = mt_node;
    let p = "<http://ex/p>".to_string();
    let mut db = DictionaryBuilder::new();
    db.observe(&node(0), &p, &node(1));
    let mut all: Vec<(usize, u32, u32)> = Vec::new(); // (graph, s, o)
    let mut next = 100u32;
    for (g, &sz) in sizes.iter().enumerate() {
        for _ in 0..sz {
            db.observe(&node(next), &p, &node(next + 1));
            all.push((g, next, next + 1));
            next += 2;
        }
    }
    let dict = db.build();
    let mut def = GraphIndexBuilder::new();
    def.push(dict.encode(&node(0), &p, &node(1)).unwrap());
    let mut named_builders: Vec<GraphIndexBuilder> = sizes
        .iter()
        .map(|_| match tile_budget {
            Some(b) => GraphIndexBuilder::new().with_tile_budget(b),
            None => GraphIndexBuilder::new(),
        })
        .collect();
    for &(g, s, o) in &all {
        named_builders[g].push(dict.encode(&node(s), &p, &node(o)).unwrap());
    }
    let named: Vec<(String, _)> = named_builders
        .into_iter()
        .enumerate()
        .map(|(g, b)| (format!("<http://ex/g{g:03}>"), b.build()))
        .collect();
    rete_core::write_dataset(&dict, &def.build(), &named, true, &[], 0)
}

/// The layout override on both fetch strategies: over a file of BIG
/// (tile-lazy, >1 MiB) containers, a header walk must keep hopping payloads —
/// the geometric ramp resets on every oversized container, so listing names
/// costs O(headers), never O(section). And the exhaustive hint must not
/// change that verdict for more than its first seeded chunk: the COUNT still
/// answers exactly, with the big payloads arriving through the tile reads
/// that actually need them, not through runaway walk read-ahead.
#[test]
fn big_containers_keep_the_walk_on_headers() {
    // 4 graphs, each ~30k triples over a tiny tile budget → multi-MB
    // containers (premise asserted below via the section size).
    let image = dataset_with_named_graphs_budget(&[30_000; 4], Some(64));
    let h = Rete::open(&image).unwrap().header().clone();
    let named = (
        h.named_graphs_offset,
        h.named_graphs_offset + h.named_graphs_len,
    );
    assert!(
        h.named_graphs_len / 4 > 1200 * 1024,
        "fixture containers too small ({} B section / 4 graphs) — the tile-lazy \
         (>1 MiB) premise does not hold",
        h.named_graphs_len
    );

    // A names-only walk: headers, never payloads. Without the ramp reset the
    // doubling would grow the read size while hopping — fetching the multi-MB
    // payloads a name listing has no use for.
    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert_eq!(lazy.graph_names().len(), 4);
    let walked: u64 = reader
        .reads()
        .iter()
        .filter(|&&r| overlaps(r, named))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        walked < 1024 * 1024,
        "listing 4 names over a {} B section read {walked} B — \
         the walk is fetching payloads it exists to hop over",
        h.named_graphs_len
    );

    // Exhaustive demand over the same layout: exact answer, and the section
    // is not fetched more than once by the walk on top of the tile reads that
    // legitimately need it (≤ 2× section + slack overall).
    let plain = Rete::open(&image).unwrap();
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let count_of = |rete: &Rete| -> String {
        match eval_query(rete, q).unwrap() {
            QueryOutput::Select(_, rows) => rows[0].get("n").cloned().unwrap(),
            _ => unreachable!(),
        }
    };
    let reader2 = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let lazy2 = Rete::open_ranged_lazy(reader2.clone()).unwrap();
    assert_eq!(count_of(&lazy2), count_of(&plain));
    assert!(!lazy2.index_incomplete());
    let fetched: u64 = reader2
        .reads()
        .iter()
        .filter(|&&r| overlaps(r, named))
        .map(|&(_, l)| l)
        .sum();
    assert!(
        fetched <= 2 * h.named_graphs_len + NAMED_WALK_CHUNK_MAX_TEST,
        "exhaustive COUNT fetched {fetched} B of a {} B section — runaway read-ahead",
        h.named_graphs_len
    );
}

/// Mirror of the engine's `NAMED_WALK_CHUNK_MAX` (not public — the test only
/// needs a slack constant of the same magnitude).
const NAMED_WALK_CHUNK_MAX_TEST: u64 = 8 * 1024 * 1024;

/// The lazy ranged open must not touch the NAMED_GRAPHS section at all — that
/// section used to be fetched and fully decoded up front, which on a
/// many-graph remote file (nkod: 67 MB, ~32k graphs) defeated remote laziness.
/// Queries must then walk/decode only what they touch, and stay CORRECT:
/// `GRAPH ?g` totals must match the resident open exactly.
#[test]
fn named_graphs_are_lazy_and_graph_queries_stay_correct() {
    // A couple of small graphs first, one much larger one after, then more
    // small ones. The big graph makes the section dwarf the walk's 64 KiB
    // read chunk, so a LIMITed query provably reads a PREFIX of the section
    // and never the big payload.
    let sizes: Vec<usize> = {
        let mut v = vec![3usize; 8];
        v.push(60_000); // the big graph, ninth in stored order
        v.extend([3usize; 8]);
        v
    };
    let total: usize = sizes.iter().sum();
    let image = dataset_with_named_graphs(&sizes);

    let h = Rete::open(&image).unwrap().header().clone();
    assert!(h.named_graphs_len > 0);
    let named = (
        h.named_graphs_offset,
        h.named_graphs_offset + h.named_graphs_len,
    );
    let named_bytes = |reader: &RecordingReader| -> u64 {
        reader
            .reads()
            .iter()
            .filter(|&&r| overlaps(r, named))
            .map(|&(off, len)| {
                // Count only the part of the read inside the section.
                let end = (off + len).min(named.1);
                end - off.max(named.0)
            })
            .sum()
    };

    // 1. The OPEN reads nothing of the named-graphs section.
    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert_eq!(
        named_bytes(&reader),
        0,
        "open_ranged_lazy fetched the named-graphs section eagerly"
    );

    // 2. The count is the section's leading varint — a few bytes, not a walk.
    assert_eq!(lazy.named_graph_count(), sizes.len());
    assert!(
        named_bytes(&reader) <= 16,
        "named_graph_count read {} bytes of the section",
        named_bytes(&reader)
    );

    // 3. A LIMITed GRAPH ?g query touches a PREFIX of the section: the big
    //    ninth graph's container is never fetched.
    let q_limit = "SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 2";
    match eval_query(&lazy, q_limit).unwrap() {
        QueryOutput::Select(_, rows) => assert_eq!(rows.len(), 2),
        _ => unreachable!(),
    }
    assert!(!lazy.index_incomplete());
    let after_limit = named_bytes(&reader);
    assert!(
        after_limit < h.named_graphs_len / 2,
        "LIMIT 2 over GRAPH ?g read {after_limit} of {} section bytes",
        h.named_graphs_len
    );

    // 4. The full count is exact — and equals the resident open's answer.
    let plain = Rete::open(&image).unwrap();
    let q_count = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let count_of = |rete: &Rete| -> String {
        match eval_query(rete, q_count).unwrap() {
            QueryOutput::Select(_, rows) => rows[0].get("n").cloned().unwrap(),
            _ => unreachable!(),
        }
    };
    let want = count_of(&plain);
    assert!(
        want.starts_with(&format!("\"{total}\"")),
        "resident count: {want}"
    );
    assert_eq!(count_of(&lazy), want);
    assert!(!lazy.index_incomplete());

    // 5. Point lookups agree with the resident open; a miss walks headers only.
    assert_eq!(lazy.graph_names(), plain.graph_names());
    assert_eq!(
        lazy.graph_index("<http://ex/g008>").unwrap().triple_count(),
        plain
            .graph_index("<http://ex/g008>")
            .unwrap()
            .triple_count(),
    );
    assert!(lazy.graph_index("<http://ex/missing>").is_none());
    assert!(!lazy.index_incomplete());
}

/// A fixture whose named-graphs section is big enough (hundreds of mid-size
/// graphs, multi-MB section) that bulk reads and the 64 KiB incremental walk
/// are unmistakably different in the read log.
fn many_named_graphs_image() -> Vec<u8> {
    dataset_with_named_graphs(&vec![300usize; 600])
}

/// The named-graphs section byte range, and the walk reads inside it (the
/// leading count-varint probe, ≤16 bytes, filtered out).
fn section_walk_reads(image: &[u8], reader: &RecordingReader) -> (u64, Vec<u64>) {
    let h = Rete::open(image).unwrap().header().clone();
    let named = (
        h.named_graphs_offset,
        h.named_graphs_offset + h.named_graphs_len,
    );
    let walk_reads: Vec<u64> = reader
        .reads()
        .iter()
        .filter(|&&r| overlaps(r, named) && r.1 > 16)
        .map(|&(_, len)| len)
        .collect();
    (h.named_graphs_len, walk_reads)
}

/// An exhaustive `GRAPH ?g` (unbound, no LIMIT — here behind an aggregate,
/// which consumes everything) must fetch the section in BULK: the walk knows
/// at evaluation time that every graph will be visited and opened, so it
/// reads full-size chunks from the first request instead of paying one 64 KiB
/// round trip per ~30 graphs (the regression this exists to fix: 262 requests
/// / ~16.5 s vs ~8.5 s eager on the 32k-graph nkod file). Same answer as the
/// resident open, in a handful of section reads with no per-container reads
/// (small containers decode straight out of the bulk chunk).
#[test]
fn exhaustive_graph_walk_bulk_fetches_the_section() {
    let image = many_named_graphs_image();
    let plain = Rete::open(&image).unwrap();
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let count_of = |rete: &Rete| -> String {
        match eval_query(rete, q).unwrap() {
            QueryOutput::Select(_, rows) => rows[0].get("n").cloned().unwrap(),
            _ => unreachable!(),
        }
    };
    let want = count_of(&plain);
    assert!(want.starts_with("\"180000\""), "resident count: {want}");

    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
    assert_eq!(count_of(&lazy), want);
    assert!(!lazy.index_incomplete());

    let (section_len, walk_reads) = section_walk_reads(&image, &reader);
    assert!(
        section_len > 1024 * 1024,
        "fixture section too small ({section_len} B) to discriminate bulk from incremental"
    );
    // Bulk from the FIRST read: the whole remainder (capped at the 8 MiB
    // chunk ceiling) in one request, not a 64 KiB probe.
    let first_bulk = section_len.min(8 * 1024 * 1024);
    assert!(
        walk_reads[0] >= first_bulk - 64 * 1024,
        "exhaustive walk started with a {} B read over a {section_len} B section — \
         the demand hint did not engage bulk fetching",
        walk_reads[0]
    );
    // And a handful of section requests in total — count varint + bulk
    // chunk(s) — never one per graph or per 64 KiB.
    assert!(
        walk_reads.len() <= 6,
        "exhaustive walk over 600 graphs issued {} section reads ({:?}...) — \
         expected the section in a few bulk chunks",
        walk_reads.len(),
        &walk_reads[..walk_reads.len().min(8)]
    );
}

/// The other side of the bargain — the reason the bulk path is gated on
/// PROVABLE exhaustive demand. Query shapes that stop early or restrict the
/// walk must keep today's small incremental reads: `LIMIT` (bounded demand),
/// `GRAPH <iri>` (a point lookup via the by-name walk), and `FROM NAMED`
/// (the restriction filters graphs out before opening). Each returns
/// identical results to the resident open, and none may issue a bulk
/// section read.
#[test]
fn non_exhaustive_graph_shapes_stay_incremental() {
    let image = many_named_graphs_image();
    let plain = Rete::open(&image).unwrap();
    let run = |rete: &Rete, q: &str| -> Vec<rete_core::Binding> {
        match eval_query(rete, q).unwrap() {
            QueryOutput::Select(_, rows) => rows,
            _ => unreachable!(),
        }
    };

    // (query, cap on TOTAL section bytes read, or None where the walk
    // legitimately spans the directory). A LIMIT stops inside the first
    // 64 KiB chunk; the point lookup stops at its (early) target; FROM NAMED
    // must walk every header to filter by name — that full walk is correct,
    // it just must never turn into a from-the-start bulk grab.
    let cases: [(&str, Option<u64>); 3] = [
        (
            "SELECT ?g WHERE { GRAPH ?g { ?s ?p ?o } } LIMIT 3",
            Some(256 * 1024),
        ),
        (
            "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <http://ex/g007> { ?s ?p ?o } }",
            Some(256 * 1024),
        ),
        (
            "SELECT (COUNT(*) AS ?n) FROM NAMED <http://ex/g010> WHERE { GRAPH ?g { ?s ?p ?o } }",
            None,
        ),
    ];
    for (q, byte_cap) in cases {
        let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
        let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
        assert_eq!(run(&lazy, q), run(&plain, q), "lazy != resident for {q}");
        assert!(!lazy.index_incomplete());

        let (section_len, walk_reads) = section_walk_reads(&image, &reader);
        assert!(
            !walk_reads.is_empty(),
            "expected the walk to touch the section for {q}"
        );
        // The first walk read is the discriminator: incremental starts at the
        // 64 KiB floor; a bulk engagement would grab ~the whole section.
        assert!(
            walk_reads[0] <= 64 * 1024,
            "{q}: first section read was {} B — bulk fetching engaged for a \
             targeted/early-stopping shape (section: {section_len} B)",
            walk_reads[0]
        );
        if let Some(cap) = byte_cap {
            let total: u64 = walk_reads.iter().sum();
            assert!(
                total <= cap,
                "{q}: read {total} B of a {section_len} B section — \
                 expected an early stop under {cap} B"
            );
        }
    }
}

/// A mid-walk outage must surface as `index_incomplete`, never as a silently
/// smaller answer — and recovery must retry (failures are not memoised).
#[test]
fn named_graph_walk_failure_is_sticky_and_retryable() {
    let image = dataset_with_named_graphs(&[3, 3, 3, 3]);
    let reader = std::sync::Arc::new(RecordingReader::new(image.clone()));
    let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();

    reader.fail_from_now();
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let _ = eval_query(&lazy, q); // whatever it returns, the flag must be set
    assert!(
        lazy.index_incomplete(),
        "a failed named-graph walk went unrecorded"
    );

    reader.recover();
    lazy.reset_load_failures();
    match eval_query(&lazy, q).unwrap() {
        QueryOutput::Select(_, rows) => {
            let n = rows[0].get("n").cloned().unwrap();
            assert!(n.starts_with("\"12\""), "recovered count: {n}");
        }
        _ => unreachable!(),
    }
    assert!(!lazy.index_incomplete());
}

/// A hostile leading count varint (claiming ~2^60 graphs) must not balloon
/// memory or panic: the section is refused, queries come back empty, and the
/// incompleteness flag is raised.
#[test]
fn hostile_named_graph_count_is_refused() {
    let image = dataset_with_named_graphs(&[2, 2]);
    let h = Rete::open(&image).unwrap().header().clone();
    let mut bad = image.clone();
    let off = h.named_graphs_offset as usize;
    // A 9-byte maximal varint where the real count was one byte.
    for b in bad.iter_mut().skip(off).take(8) {
        *b = 0xff;
    }
    bad[off + 8] = 0x01;
    let lazy = Rete::open_ranged_lazy(std::sync::Arc::new(RecordingReader::new(bad))).unwrap();
    assert_eq!(lazy.named_graph_count(), 0);
    assert!(lazy.index_incomplete());
}
