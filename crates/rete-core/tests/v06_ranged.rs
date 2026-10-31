//! Range-access invariants on **format 0x06** files, asserted through a reader
//! that records every range: the lazy open stays lazy through the two-level
//! directories (a selective query faults a few leaves and tiles, not the
//! index), a failed fault is surfaced, result terms decode in one batch, the
//! text index is read only where the pushdown is proven, and `GRAPH <g>` finds
//! its tiles without reading the quad section.
//!
//! The fixtures are the `0x05` ones of `tests/ranged.rs`, converted — the
//! writer's own path (`rete build` assembles, then converts).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::{kind, open::read_front, write::convert_image, FIRST_READ};
use rete_core::{
    build_pyramid_meta, eval_query, write_file, DictionaryBuilder, GraphIndexBuilder, PermSet,
    PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader, SummaryView, TextIndexScope,
    DEFAULT_TILE_BUDGET,
};

/// Records every `(offset, len)` and every `read_many` batch; can be switched
/// to fail every read (a network outage mid-session).
struct Rec {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    batches: Mutex<Vec<Vec<(u64, u64)>>>,
    fail: AtomicBool,
    fan_out: usize,
}

impl Rec {
    fn new(data: Vec<u8>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            data,
            reads: Mutex::new(Vec::new()),
            batches: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
            fan_out: 4,
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
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        self.batches.lock().unwrap().push(ranges.to_vec());
        ranges.iter().map(|&(o, l)| self.read_at(o, l)).collect()
    }
    fn concurrency(&self) -> usize {
        self.fan_out
    }
}

fn v6(image: Vec<u8>) -> Vec<u8> {
    convert_image(&image, Default::default()).unwrap()
}

/// `[start, end)` of section `k` of a 0x06 image.
fn region(image: &[u8], k: u8) -> (u64, u64) {
    let (h, root) = read_front(&SliceReader::new(image)).unwrap();
    let r = root.section(k).expect("section present").abs(&h);
    (r.offset, r.offset + r.len)
}

/// Bytes of `reads` inside `region`.
fn inside(reads: &[(u64, u64)], region: (u64, u64)) -> u64 {
    reads
        .iter()
        .map(|&(o, l)| (o + l).min(region.1).saturating_sub(o.max(region.0)))
        .sum()
}

fn select(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        other => panic!("unexpected output {other:?}"),
    }
}

fn lazy(rec: &std::sync::Arc<Rec>) -> Rete {
    Rete::open_ranged_lazy(rec.clone()).unwrap()
}

fn mt_node(n: u32) -> String {
    format!(
        "<http://ex/n/{:08x}/{:08x}/{n:05}>",
        n.wrapping_mul(0x9E37_79B9),
        n.wrapping_mul(0x85EB_CA6B) ^ 0x5151_5151
    )
}

/// Many tiles (tiny tile budget) and many dictionary chunks (scrambled IRIs).
fn multi_tile() -> Vec<u8> {
    let knows = "<http://ex/knows>";
    let edges: Vec<(u32, u32)> = (0..24000u32)
        .flat_map(|i| [(i, (i * 7 + 1) % 24000), (i, (i * 13 + 5) % 24000)])
        .collect();
    let mut db = DictionaryBuilder::new();
    for &(s, o) in &edges {
        db.observe(&mt_node(s), knows, &mt_node(o));
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
    for &(s, o) in &edges {
        ib.push(dict.encode(&mt_node(s), knows, &mt_node(o)).unwrap());
    }
    v6(write_file(&dict, &ib.build(), false, &[], 0))
}

#[test]
fn a_selective_query_faults_a_few_tiles_and_chunks() {
    let image = multi_tile();
    let idx = region(&image, kind::INDEX_DATA);
    let dict = region(&image, kind::DICT_DATA);
    let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
    let want = select(&Rete::open(&image).unwrap(), &q);
    assert_eq!(want.len(), 2);

    let rec = Rec::new(image.clone());
    let rete = lazy(&rec);
    let opened = rec.take();
    assert_eq!(opened.len(), 1, "the open is one read: {opened:?}");
    assert_eq!(opened[0], (0, FIRST_READ));
    assert_eq!(select(&rete, &q), want);
    assert!(!rete.index_incomplete());
    let reads = rec.take();
    let (i, d) = (inside(&reads, idx), inside(&reads, dict));
    assert!(
        i < (idx.1 - idx.0) / 20,
        "{i} of {} index bytes for one bound subject",
        idx.1 - idx.0
    );
    assert!(
        d < (dict.1 - dict.0) / 4,
        "{d} of {} dictionary bytes for four terms",
        dict.1 - dict.0
    );
}

#[test]
fn the_routed_pattern_reads_only_its_tiles() {
    let image = multi_tile();
    let idx = region(&image, kind::INDEX_DATA);
    let n7 = mt_node(7);
    let want = Rete::open(&image).unwrap().query(Some(&n7), None, None);
    assert_eq!(want.len(), 2);
    let rec = Rec::new(image.clone());
    assert_eq!(
        Rete::query_ranged(&*rec, Some(&n7), None, None).unwrap(),
        want
    );
    let i = inside(&rec.take(), idx);
    assert!(
        i > 0 && i < (idx.1 - idx.0) / 20,
        "{i} of {} index bytes",
        idx.1 - idx.0
    );
    // An unknown term never reaches the index.
    let rec = Rec::new(image.clone());
    assert!(
        Rete::query_ranged(&*rec, Some("<http://ex/none>"), None, None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(inside(&rec.take(), idx), 0);
}

#[test]
fn limit_one_and_a_full_scan_stay_bounded() {
    let image = multi_tile();
    let idx = region(&image, kind::INDEX_DATA);
    let rec = Rec::new(image.clone());
    let rete = lazy(&rec);
    rec.take();
    assert_eq!(
        select(&rete, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 1").len(),
        1
    );
    let i = inside(&rec.take(), idx);
    assert!(i < (idx.1 - idx.0) / 4, "LIMIT 1 read {i} index bytes");

    // A full count: every SPO tile, in O(log n) coalesced batches.
    let rete = lazy(&rec);
    rec.take();
    let n = select(&rete, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
    assert!(n[0].contains("48000"), "{n:?}");
    let reads = rec.take();
    let index_reads = reads
        .iter()
        .filter(|&&(o, l)| o < idx.1 && idx.0 < o + l)
        .count();
    assert!(
        index_reads < 64,
        "{index_reads} index reads for a full scan"
    );
}

#[test]
fn a_failed_fault_is_flagged_and_retryable() {
    let image = multi_tile();
    let q = format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(7));
    let want = select(&Rete::open(&image).unwrap(), &q);
    let rec = Rec::new(image);
    let rete = lazy(&rec);
    rec.fail.store(true, Ordering::Relaxed);
    let _ = eval_query(&rete, &q).unwrap();
    assert!(rete.index_incomplete(), "an outage must flag the answer");
    rec.fail.store(false, Ordering::Relaxed);
    rete.reset_load_failures();
    assert!(!rete.index_incomplete());
    assert_eq!(select(&rete, &q), want, "the retry refetches what failed");
    assert!(!rete.index_incomplete());
}

/// Subject-only IRIs pointing at object-only literals: a row needs a chunk of
/// two dictionary sections.
fn two_sections() -> Vec<u8> {
    let p = "<http://ex/p>";
    let mix = |x: u64| {
        let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    };
    let t: Vec<(String, String)> = (0..20000u64)
        .map(|i| {
            (
                format!("<http://ex/subject/{:016x}{:016x}>", mix(i), mix(i + 1)),
                format!("\"value {:016x} {:016x}\"", mix(i + 7), mix(i + 9)),
            )
        })
        .collect();
    let mut db = DictionaryBuilder::new();
    for (s, o) in &t {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let mut ib = GraphIndexBuilder::new();
    for (s, o) in &t {
        ib.push(dict.encode(s, p, o).unwrap());
    }
    v6(write_file(&dict, &ib.build(), false, &[], 0))
}

#[test]
fn result_terms_decode_in_one_batch_across_sections() {
    let image = two_sections();
    let dict = region(&image, kind::DICT_DATA);
    let batches = |q: &str| -> (usize, u64) {
        let want = select(&Rete::open(&image).unwrap(), q);
        assert_eq!(want.len(), 20000);
        let rec = Rec::new(image.clone());
        let rete = lazy(&rec);
        let opened = rec.batches.lock().unwrap().len();
        assert_eq!(select(&rete, q), want);
        let b = rec.batches.lock().unwrap();
        let in_dict: Vec<&Vec<(u64, u64)>> = b[opened..]
            .iter()
            .filter(|b| b.iter().all(|&(o, l)| o < dict.1 && dict.0 < o + l))
            .collect();
        let bytes = in_dict
            .first()
            .map_or(0, |b| b.iter().map(|&(_, l)| l).sum());
        (in_dict.len(), bytes)
    };
    let (n_s, bytes_s) = batches("SELECT ?s WHERE { ?s <http://ex/p> ?o }");
    let (n_so, bytes_so) = batches("SELECT ?s ?o WHERE { ?s <http://ex/p> ?o }");
    assert_eq!(n_s, 1, "subject chunks in one batch");
    assert_eq!(n_so, 1, "subject and object chunks share one batch");
    assert!(bytes_so > bytes_s);
}

/// A clustered graph big enough that its index lies well past the first read.
fn pyramid_image() -> Vec<u8> {
    let node = mt_node;
    let knows = "<http://ex/knows>";
    let mut edges: Vec<(u32, u32)> = Vec::new();
    for c in 0..30u32 {
        for i in 0..60u32 {
            for j in 0..60u32 {
                if i != j {
                    edges.push((c * 60 + i, c * 60 + j));
                }
            }
        }
        edges.push((c * 60, ((c + 1) % 30) * 60));
    }
    let mut db = DictionaryBuilder::new();
    for &(s, o) in &edges {
        db.observe(&node(s), knows, &node(o));
    }
    let dict = db.build();
    let ids: Vec<_> = edges
        .iter()
        .map(|&(s, o)| dict.encode(&node(s), knows, &node(o)).unwrap())
        .collect();
    let mut ib = GraphIndexBuilder::new();
    for &t in &ids {
        ib.push(t);
    }
    let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
    v6(write_file(&dict, &ib.build(), false, &meta, levels))
}

#[test]
fn the_summary_view_never_reads_index_data() {
    let image = pyramid_image();
    let idx = region(&image, kind::INDEX_DATA);
    assert!(
        idx.1 > FIRST_READ,
        "the fixture's index must reach past the first read"
    );
    let rec = Rec::new(image.clone());
    let view = SummaryView::open_ranged(&*rec).unwrap().expect("a pyramid");
    let reads = rec.take();
    let past_first: Vec<(u64, u64)> = reads
        .iter()
        .copied()
        .filter(|&(o, l)| o + l > FIRST_READ)
        .collect();
    assert_eq!(
        inside(&past_first, (idx.0.max(FIRST_READ), idx.1)),
        0,
        "the summary read index data past the first read: {reads:?}"
    );
    assert!(reads.len() <= 6, "{reads:?}");
    let full = Rete::open(&image).unwrap();
    assert_eq!(
        view.predicate_total("<http://ex/knows>") as usize,
        full.query(None, Some("<http://ex/knows>"), None).len()
    );
    // The lazy open does not read the pyramid until a summary query asks.
    let pyr = region(&image, kind::PYRAMID);
    let rec = Rec::new(image.clone());
    let rete = lazy(&rec);
    select(
        &rete,
        &format!("SELECT ?o WHERE {{ {} <http://ex/knows> ?o }}", mt_node(3)),
    );
    let reads = rec.take();
    let past: Vec<(u64, u64)> = reads
        .iter()
        .copied()
        .filter(|&(o, _)| o >= FIRST_READ)
        .collect();
    assert_eq!(inside(&past, pyr), 0, "the pyramid was read: {reads:?}");
}

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

/// Words in literals for the text index, many entities so the postings lie
/// past the first read, and a quad index with many small graphs.
fn text_and_quads() -> Vec<u8> {
    let mut q: Vec<RawQuad> = Vec::new();
    let words = ["glucose", "benzene", "ring", "metabolism", "acid", "ester"];
    for e in 0..6000 {
        let ent = iri(&format!("e/{e}"));
        q.push((
            ent.clone(),
            iri("label"),
            format!(
                "\"{} {} {e}\"",
                words[e % words.len()],
                words[(e / 7) % words.len()]
            ),
            None,
        ));
        q.push((
            ent.clone(),
            iri("mixed"),
            iri(&format!("glucose/{e}")),
            None,
        ));
        let g = iri(&format!("e/{e}/prov/"));
        q.push((
            iri(&format!("e/{e}/prov/se/1")),
            iri("generatedAtTime"),
            format!("\"2020-01-{:02}\"", 1 + e % 28),
            Some(g.clone()),
        ));
        q.push((iri(&format!("e/{e}/prov/se/1")), iri("of"), ent, Some(g)));
    }
    assemble_dataset_with_layout(
        q,
        false,
        Some(TextIndexScope::default()),
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        GraphLayout::Quad,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

#[test]
fn the_text_index_is_read_only_where_the_pushdown_is_proven() {
    let v5 = text_and_quads();
    let image = v6(v5.clone());
    let postings = region(&image, kind::TEXT_DATA);
    assert!(postings.0 >= FIRST_READ, "postings past the first read");
    let plain = Rete::open(&v5).unwrap();
    for (q, uses) in [
        (
            "SELECT ?s WHERE { ?s <http://ex/label> ?o FILTER(CONTAINS(LCASE(STR(?o)), \"glucose\")) }",
            true,
        ),
        (
            "SELECT ?s WHERE { ?s ?p ?o FILTER(isLiteral(?o) && CONTAINS(LCASE(STR(?o)), \"glucose\")) }",
            true,
        ),
        (
            "SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(LCASE(STR(?o)), \"glucose\")) }",
            false,
        ),
    ] {
        let rec = Rec::new(image.clone());
        let rete = lazy(&rec);
        rec.take();
        assert_eq!(select(&rete, q), select(&plain, q), "{q}");
        let touched = inside(&rec.take(), postings) > 0;
        assert_eq!(touched, uses, "postings read = {uses}: {q}");
    }
}

#[test]
fn one_graph_is_found_without_reading_the_quad_section() {
    let image = v6(text_and_quads());
    let quads = region(&image, kind::QUAD_DATA);
    let q = "SELECT ?s ?t WHERE { GRAPH <http://ex/e/5998/prov/> { ?s <http://ex/generatedAtTime> ?t } }";
    let rec = Rec::new(image.clone());
    let rete = lazy(&rec);
    rec.take();
    let rows = select(&rete, q);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows, select(&Rete::open(&image).unwrap(), q));
    let pulled = inside(&rec.take(), quads);
    assert!(
        pulled < (quads.1 - quads.0) / 8,
        "GRAPH <g> pulled {pulled} B of a {} B quad section",
        quads.1 - quads.0
    );
    assert!(!rete.index_incomplete());
}

/// A walk over ascending ids faults the section's chunks in order: the reader
/// reads ahead (2, 4, 8, 16 chunks a batch) instead of one round trip per
/// chunk, and decodes the same terms.
#[test]
fn a_sequential_walk_reads_ahead() {
    let image = two_sections();
    let dict = region(&image, kind::DICT_DATA);
    let plain = Rete::open(&image).unwrap();
    let n = plain.dictionary().subject_only_count();
    let first = plain.dictionary().shared_count() + 1;
    let rec = Rec::new(image.clone());
    let rete = lazy(&rec);
    rec.take();
    let opened = rec.batches.lock().unwrap().len();
    for id in first..first + n {
        assert_eq!(
            rete.dictionary().subject_term(id),
            plain.dictionary().subject_term(id)
        );
    }
    let reads = rec.take();
    let batches: Vec<Vec<(u64, u64)>> = rec.batches.lock().unwrap()[opened..].to_vec();
    let in_dict = |&(o, l): &(u64, u64)| o < dict.1 && dict.0 < o + l;
    let fetched = reads.iter().filter(|r| in_dict(r)).count();
    let batched: usize = batches.iter().map(Vec::len).sum();
    // Round trips: each batch is one, each read outside a batch another.
    let rounds = batches.iter().filter(|b| b.iter().any(in_dict)).count() + fetched - batched;
    // Every chunk of the subject-only section, faulted in order.
    let (_, root) = read_front(&SliceReader::new(&image)).unwrap();
    let units =
        rete_core::v06::decode_dict_root(root.unit(rete_core::v06::tag::DICT).unwrap()).unwrap();
    let chunks = units[1].n_chunks as usize;
    assert!(chunks > 8, "the fixture needs many chunks ({chunks})");
    // One round trip per chunk without the read-ahead; with it, the
    // doubling windows (1, 2, 4, 8 …).
    assert!(
        rounds <= 5,
        "{rounds} round trips for {chunks} sequential chunks — expected a doubling read-ahead"
    );
}
