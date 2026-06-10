//! Range-access invariants — the headline "give it a URL, fetch only what you
//! need" promise, asserted in code rather than only measured by the benchmark.
//!
//! A [`RecordingReader`] logs every byte range requested, so we can prove:
//!   * `SummaryView::open_ranged` reads only header + dictionary + summary and
//!     **never touches the triple-index byte range** (the "overview first" path);
//!   * `Rete::open_ranged` opens in a small bounded number of requests — never a
//!     linear scan proportional to the triple count.

use std::cell::RefCell;

use rete_core::{
    build_pyramid_meta, write_file, DictionaryBuilder, GraphIndexBuilder, RangeReader, Rete,
    SliceReader, SummaryView, DEFAULT_TILE_BUDGET,
};

/// A `RangeReader` over an in-memory image that records each `(offset, len)`.
struct RecordingReader {
    data: Vec<u8>,
    reads: RefCell<Vec<(u64, u64)>>,
}

impl RecordingReader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            reads: RefCell::new(Vec::new()),
        }
    }
    fn reads(&self) -> Vec<(u64, u64)> {
        self.reads.borrow().clone()
    }
    fn bytes_read(&self) -> u64 {
        self.reads.borrow().iter().map(|&(_, l)| l).sum()
    }
}

impl RangeReader for RecordingReader {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.borrow_mut().push((offset, len));
        let start = offset as usize;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))?;
        Ok(self.data[start..end].to_vec())
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
        "summary read {} of {} bytes — expected a strict subset",
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
    // correctly even when that region is overwritten with zeros — i.e. the
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

    // header, dictionary, index, pyramid-meta — at most these four (no named
    // graphs in this file). Crucially constant, not proportional to triples.
    let reads = reader.reads();
    assert!(
        reads.len() <= 4,
        "full ranged open should be ≤4 reads, got {} ({reads:?})",
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

/// On a tiled (v0.2) multi-tile file, a bound-subject routed query must fetch
/// only the tile **directory** plus the one matching tile — a small fraction
/// of the selected permutation section, not the whole section.
#[test]
fn routed_pattern_query_fetches_only_matching_tiles() {
    // A graph whose sections dwarf the 4 KiB directory prefetch, split into
    // many tiles.
    let node = |n: u32| format!("<http://ex/n{n}>");
    let knows = "<http://ex/knows>".to_string();
    let mut db = DictionaryBuilder::new();
    let edges: Vec<(u32, u32)> = (0..4000u32)
        .flat_map(|i| [(i, (i * 7 + 1) % 4000), (i, (i * 13 + 5) % 4000)])
        .collect();
    for &(s, o) in &edges {
        db.observe(&node(s), &knows, &node(o));
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
    for &(s, o) in &edges {
        ib.push(dict.encode(&node(s), &knows, &node(o)).unwrap());
    }
    let image = write_file(&dict, &ib.build(), false, &[], 0);
    let (idx_start, idx_end) = index_region(&image);
    let index_len = idx_end - idx_start;

    let plain = Rete::open(&image).unwrap();
    let expected = plain.query(Some("<http://ex/n7>"), None, None);
    assert_eq!(expected.len(), 2);

    let reader = RecordingReader::new(image.clone());
    let got = Rete::query_ranged(&reader, Some("<http://ex/n7>"), None, None).unwrap();
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
        "tile-routed query read {index_bytes_read} of {index_len} index bytes — \
         expected directory + one tile, a small fraction of one section"
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
