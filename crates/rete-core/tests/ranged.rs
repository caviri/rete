//! Range-access invariants â€” the headline "give it a URL, fetch only what you
//! need" promise, asserted in code rather than only measured by the benchmark.
//!
//! A [`RecordingReader`] logs every byte range requested, so we can prove:
//!   * `SummaryView::open_ranged` reads only header + dictionary + summary and
//!     **never touches the triple-index byte range** (the "overview first" path);
//!   * `Rete::open_ranged` opens in a small bounded number of requests â€” never a
//!     linear scan proportional to the triple count.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rete_core::{
    build_pyramid_meta, eval_query, write_file, DictionaryBuilder, GraphIndexBuilder, QueryOutput,
    RangeReader, Rete, SliceReader, SummaryView, DEFAULT_TILE_BUDGET,
};

/// A `RangeReader` over an in-memory image that records each `(offset, len)`,
/// and can be switched to fail every read (simulating a network outage
/// mid-session). `Sync` so it can back a lazily-faulting open.
struct RecordingReader {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    fail: AtomicBool,
}

impl RecordingReader {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
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
    assert!(
        index_reads < 32,
        "full scan issued {index_reads} index-region reads over {spo_tiles} tiles — \
         expected the tile directories plus O(log n) coalesced batch reads"
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
    assert!(spo_tiles > 100, "expected a many-tile fixture, got {spo_tiles}");

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
        total_reads < 56,
        "lazy dump issued {total_reads} range reads — expected directories \
         plus coalesced chunk batches and O(log n) tile-prefetch batches"
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
