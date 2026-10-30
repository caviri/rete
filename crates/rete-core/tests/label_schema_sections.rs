//! The LABEL_INDEX (kind 8) and SCHEMA (kind 9) sections, end to end: what a
//! sectioned build writes, that every open path reads the same answers, what a
//! remote prefix search costs, and the compatibility rules — the sections sit
//! outside the content hash, survive every header rewrite, and a file built
//! before them keeps working through the pyramid-meta fallback.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use rete_core::format::{content_hash_parts, rewrite_card};
use rete_core::ingest::{assemble_dataset_opts, AssembleOptions, RawQuad};
use rete_core::{
    append_sections, attach_build_info, read_schema_coherence_ranged, read_schema_summary_ranged,
    replace_metadata, verify, verify_ranged, Header, RangeReader, Rete, SearchView, SectionKind,
    SliceReader, SummaryView,
};

const T: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
const SUB: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
const FAM: &str = "<http://xmlns.com/foaf/0.1/familyName>";
const GIV: &str = "<http://xmlns.com/foaf/0.1/givenName>";

fn quads(n: usize) -> Vec<RawQuad> {
    let q = |s: String, p: &str, o: String| (s, p.to_string(), o, None);
    let mut v = vec![q(
        "<http://ex/Journal>".into(),
        SUB,
        "<http://ex/Venue>".into(),
    )];
    for i in 0..n {
        let s = format!("<http://ex/j{i}>");
        v.push(q(s.clone(), T, "<http://ex/Journal>".into()));
        v.push(q(s.clone(), LABEL, format!("\"Acta Universitatis {i}\"")));
        v.push(q(
            s,
            "<http://ex/next>",
            format!("<http://ex/j{}>", (i + 1) % n),
        ));
        let p = format!("<http://ex/p{i}>");
        v.push(q(p.clone(), FAM, format!("\"Svobodová{i}\"")));
        v.push(q(p, GIV, "\"Jana\"".into()));
    }
    v
}

fn build(quads: Vec<RawQuad>, opts: &AssembleOptions<'_>) -> Vec<u8> {
    assemble_dataset_opts(quads, opts, |_, _| Vec::new()).0
}

fn sectioned(with_pyramid: bool) -> AssembleOptions<'static> {
    AssembleOptions {
        with_pyramid,
        label_index: true,
        ..AssembleOptions::sectioned()
    }
}

/// A shareable in-memory reader that counts requests and bytes.
#[derive(Clone)]
struct Counted {
    data: Arc<Vec<u8>>,
    requests: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
}

impl Counted {
    fn new(data: Vec<u8>) -> Self {
        Counted {
            data: Arc::new(data),
            requests: Arc::new(AtomicU64::new(0)),
            bytes: Arc::new(AtomicU64::new(0)),
        }
    }
    fn reset(&self) {
        self.requests.store(0, Ordering::SeqCst);
        self.bytes.store(0, Ordering::SeqCst);
    }
    fn requests(&self) -> u64 {
        self.requests.load(Ordering::SeqCst)
    }
}

impl RangeReader for Counted {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.bytes.fetch_add(len, Ordering::SeqCst);
        SliceReader::new(&self.data).read_at(offset, len)
    }
}

#[test]
fn a_sectioned_build_writes_both_sections_and_keeps_them_out_of_the_pyramid() {
    let bytes = build(quads(200), &sectioned(true));
    let h = Header::from_bytes(&bytes).unwrap();
    assert!(h.label_index_section().is_some());
    assert!(h.schema_section().is_some());
    assert!(
        h.extra_sections
            .iter()
            .all(|s| matches!(s.kind, SectionKind::LabelIndex | SectionKind::Schema)),
        "kinds 8 and 9 live in the extra directory entries"
    );
    let rete = Rete::open(&bytes).unwrap();
    let pyr = rete.pyramid().expect("a pyramid was built");
    assert!(pyr.label_index.is_empty(), "no label block in the pyramid");
    assert!(pyr.predicate_stats.is_empty() && pyr.char_sets.is_empty());
    assert!(pyr.class_hierarchy.is_empty() && pyr.level_rollups.is_empty());
    assert!(!pyr.summary.is_empty(), "the community summary stays");
    // …and the answers come from the sections.
    assert_eq!(rete.prefix_search("acta universitatis 1", 1000).len(), 111);
    assert!(!rete.predicate_stats().is_empty());
    assert!(rete
        .schema()
        .unwrap()
        .class_hierarchy
        .iter()
        .any(|c| c.class == "<http://ex/Journal>"));
    assert!(verify(&bytes).unwrap());
}

#[test]
fn the_sections_do_not_change_the_content_hash_so_older_readers_still_verify() {
    // Old readers' `verify` hashes the kinds they know; kinds 8 and 9 are
    // outside that set, so a file with and without them has ONE identity.
    let with = build(quads(100), &sectioned(false));
    let without = build(
        quads(100),
        &AssembleOptions {
            with_pyramid: false,
            label_index: false,
            schema: false,
            ..AssembleOptions::sectioned()
        },
    );
    let (hw, ho) = (
        Header::from_bytes(&with).unwrap(),
        Header::from_bytes(&without).unwrap(),
    );
    assert_eq!(hw.content_hash, ho.content_hash);
    assert!(with.len() > without.len());
    // The prefix of the file up to the old footer is byte-identical: the
    // sections are appended after everything an old reader knows.
    let body_end = without.len() - 4;
    assert_eq!(with[1024..body_end], without[1024..body_end]);
    // The new verify still covers them, by their own digests.
    assert!(verify(&with).unwrap());
    assert!(verify_ranged(&SliceReader::new(&with)).unwrap());
    let s = hw.label_index_section().unwrap();
    let mut bad = with.clone();
    bad[(s.offset + s.length - 1) as usize] ^= 0x55;
    assert!(!verify(&bad).unwrap(), "a flipped byte in kind 8 is caught");
    assert!(!verify_ranged(&SliceReader::new(&bad)).unwrap());
    // A reader that does not know the kinds (an older one) keeps them as
    // unknown entries; renumbering ours to an unused kind models exactly that.
    let mut old_view = with.clone();
    for i in 0..u16::from_le_bytes([old_view[44], old_view[45]]) as usize {
        let p = 64 + i * 24;
        let kind = u16::from_le_bytes([old_view[p], old_view[p + 1]]);
        if kind == 8 || kind == 9 {
            old_view[p..p + 2].copy_from_slice(&(kind + 900).to_le_bytes());
        }
    }
    let oh = Header::from_bytes(&old_view).unwrap();
    assert_eq!(oh.extra_sections.len(), 2);
    assert!(oh.label_index_section().is_none());
    assert!(verify(&old_view).unwrap(), "verify ignores unknown kinds");
    let old = Rete::open(&old_view).unwrap();
    assert!(old.prefix_search("acta", 5).is_empty());
    assert_eq!(
        old.dump(None).len(),
        Rete::open(&with).unwrap().dump(None).len(),
        "same rows without the sections"
    );
}

#[test]
fn every_open_path_answers_the_same_prefix_search() {
    let bytes = build(quads(300), &sectioned(false));
    let want = Rete::open(&bytes).unwrap().prefix_search("svobodová2", 500);
    assert_eq!(want.len(), 111, "Svobodová2, Svobodová20…299: {want:?}");
    assert!(want.iter().all(|(l, _)| l.ends_with(", Jana")));
    let ranged = Rete::open_ranged(&SliceReader::new(&bytes)).unwrap();
    assert_eq!(ranged.prefix_search("svobodová2", 500), want);
    let lazy = Rete::open_ranged_lazy(Counted::new(bytes.clone())).unwrap();
    assert_eq!(lazy.prefix_search("SVOBODOVÁ2", 500), want);
    let view = SearchView::open_ranged(Counted::new(bytes)).unwrap();
    assert!(view.has_label_index() && view.has_labels());
    assert_eq!(view.prefix_search("svobodová2", 500), want);
}

#[test]
fn a_remote_prefix_search_reads_the_header_the_section_head_and_one_block() {
    let reader = Counted::new(build(quads(2_000), &sectioned(false)));
    let view = SearchView::open_ranged(reader.clone()).unwrap();
    assert_eq!(reader.requests(), 1, "the open reads the header only");
    reader.reset();
    let hits = view.prefix_search("acta universitatis 1999", 5);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, "<http://ex/j1999>");
    // head (preamble + root) + one data block: no dictionary read at all
    assert_eq!(reader.requests(), 2);
    reader.reset();
    let _ = view.prefix_search("acta universitatis 5", 5);
    assert_eq!(reader.requests(), 1, "the head is resident now");
}

#[test]
fn a_file_without_a_pyramid_has_schema_planner_stats_and_tier0_reads() {
    let bytes = build(quads(150), &sectioned(false));
    let rete = Rete::open(&bytes).unwrap();
    assert!(rete.pyramid().is_none());
    assert!(
        !rete.predicate_stats().is_empty(),
        "query_stats without a pyramid"
    );
    let r = SliceReader::new(&bytes);
    let (classes, _rel) = read_schema_summary_ranged(&r)
        .unwrap()
        .expect("schema summary");
    assert!(classes
        .iter()
        .any(|(c, n)| c == "<http://ex/Journal>" && *n == 150));
    assert!(read_schema_coherence_ranged(&r).unwrap().is_some());
    assert!(
        SummaryView::open_ranged(&r).unwrap().is_none(),
        "no summary"
    );
    let schema_only = SummaryView::open_schema_ranged(&r)
        .unwrap()
        .expect("schema view");
    assert!(schema_only.level_count() > 0);
    // Lazy: the schema is faulted on first use, one range.
    let c = Counted::new(bytes);
    let lazy = Rete::open_ranged_lazy(c.clone()).unwrap();
    c.reset();
    assert!(!lazy.predicate_stats().is_empty());
    assert_eq!(c.requests(), 1);
}

#[test]
fn header_rewrites_keep_the_sections_reachable() {
    let bytes = build(quads(80), &sectioned(true));
    let want = Rete::open(&bytes).unwrap().prefix_search("acta", 10);
    // build info goes in after the metadata: every later section shifts
    let with_info = attach_build_info(&bytes, br#"{"built_at":"x"}"#).unwrap();
    assert!(verify(&with_info).unwrap());
    assert_eq!(
        Rete::open(&with_info).unwrap().prefix_search("acta", 10),
        want
    );
    // a new card: metadata replaced, hash recomputed over the same parts
    let recarded = replace_metadata(&with_info, br#"{"title":"t"}"#).unwrap();
    assert!(verify(&recarded).unwrap());
    let rr = Rete::open(&recarded).unwrap();
    assert_eq!(rr.prefix_search("acta", 10), want);
    assert!(rr.schema().is_some());
    // appending a kind the file already has is refused
    assert!(append_sections(&bytes, &[(SectionKind::LabelIndex, b"x".as_slice())]).is_err());
}

#[test]
fn a_legacy_build_keeps_its_combined_pyramid_meta_and_answers_from_it() {
    // The library's legacy entry points are byte-for-byte what they were: the
    // labels and the schema inside the pyramid-meta, no kinds 8/9.
    let bytes = build(quads(60), &AssembleOptions::legacy());
    let h = Header::from_bytes(&bytes).unwrap();
    assert!(h.extra_sections.is_empty());
    let rete = Rete::open(&bytes).unwrap();
    assert!(rete.schema().is_none());
    assert!(!rete.pyramid().unwrap().label_index.is_empty());
    assert_eq!(rete.prefix_search("acta universitatis 5", 20).len(), 11);
    // composed person names are a LABEL_INDEX rule only
    assert!(rete.prefix_search("svobodová", 5).is_empty());
    assert!(!rete.predicate_stats().is_empty(), "from the pyramid-meta");
    assert!(rete.schema_or_pyramid().is_some());
}

/// Which section kinds the header content hash covers, pinned, in the order
/// `content_hash_parts` folds them: metadata (1), dictionary (2), index (3),
/// pyramid-meta (4), text index (6), named graphs (5). Build info (7) and the
/// self-digested LABEL_INDEX (8) and SCHEMA (9) are outside it. A kind that
/// joins the hash joins `content_hash_parts` and this list in the same change.
#[test]
fn the_content_hash_covers_exactly_kinds_1_to_6() {
    let mut q = quads(40);
    q.push((
        "<http://ex/g1>".into(),
        LABEL.into(),
        "\"in a named graph\"".into(),
        Some("<http://ex/graph1>".into()),
    ));
    let opts = AssembleOptions {
        with_text_index: true,
        ..sectioned(true)
    };
    let bytes = build(q, &opts);
    let bytes = replace_metadata(&bytes, br#"{"title":"t"}"#).unwrap();
    let bytes = attach_build_info(&bytes, br#"{"built_at":"x"}"#).unwrap();
    let h = Header::from_bytes(&bytes).unwrap();
    let all = [
        SectionKind::Metadata,
        SectionKind::Dictionary,
        SectionKind::Index,
        SectionKind::PyramidMeta,
        SectionKind::NamedGraphs,
        SectionKind::TextIndex,
        SectionKind::BuildInfo,
        SectionKind::LabelIndex,
        SectionKind::Schema,
    ];
    for k in all {
        assert!(h.section(k).is_some(), "the fixture carries {k:?}");
    }
    let hashed: Vec<u16> = content_hash_parts(&h)
        .into_iter()
        .map(|(off, len)| {
            // `all` is in kind order, so a position is its kind number minus one.
            let i = all
                .iter()
                .position(|k| {
                    h.section(*k)
                        .is_some_and(|s| s.offset == off && s.length == len)
                })
                .unwrap_or_else(|| panic!("hashed range {off}+{len} is no section"));
            i as u16 + 1
        })
        .collect();
    assert_eq!(hashed, [1, 2, 3, 4, 6, 5]);
    assert!(verify(&bytes).unwrap());
}

/// `rete recard`'s rewrite (only the card and the build record change) keeps
/// kinds 8 and 9 byte for byte and reachable: the header offsets shift, their
/// own digests still hold, and searches and the schema answer as before.
#[test]
fn a_card_rewrite_carries_the_label_and_schema_sections() {
    let bytes = build(quads(80), &sectioned(true));
    let bytes = attach_build_info(&bytes, br#"{"built_at":"x"}"#).unwrap();
    let before = Rete::open(&bytes).unwrap();
    let want = before.prefix_search("acta", 10);
    let want_people = before.prefix_search("svobodová", 5);
    assert!(!want_people.is_empty());
    let section_bytes = |b: &[u8], s: rete_core::Section| {
        b[s.offset as usize..(s.offset + s.length) as usize].to_vec()
    };
    let h0 = Header::from_bytes(&bytes).unwrap();
    let card = br#"{"title":"a much longer title than before, so every later section moves"}"#;
    let mut out = std::io::Cursor::new(Vec::new());
    rewrite_card(
        &SliceReader::new(&bytes),
        &mut out,
        card,
        br#"{"built_at":"y"}"#,
        &mut |_| {},
    )
    .unwrap();
    let out = out.into_inner();
    let h1 = Header::from_bytes(&out).unwrap();
    for k in [SectionKind::LabelIndex, SectionKind::Schema] {
        let (s0, s1) = (h0.section(k).unwrap(), h1.section(k).unwrap());
        assert_ne!(s0.offset, s1.offset, "{k:?} moved with the longer card");
        assert_eq!(section_bytes(&bytes, s0), section_bytes(&out, s1), "{k:?}");
    }
    assert!(verify(&out).unwrap());
    assert!(verify_ranged(&SliceReader::new(&out)).unwrap());
    let after = Rete::open(&out).unwrap();
    assert_eq!(after.prefix_search("acta", 10), want);
    assert_eq!(after.prefix_search("svobodová", 5), want_people);
    assert!(after.schema().is_some());
}
