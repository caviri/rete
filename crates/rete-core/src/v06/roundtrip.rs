//! `0x06` round trips: a `0x05` image converted to `0x06` answers every query
//! the same, on the resident and the lazy open; verify catches tampering; a
//! re-card keeps every section.

use super::write::{convert_image, replace_card, WriteOpts};
use super::*;
use crate::file::Rete;
use crate::ingest::{assemble_dataset_opts, AssembleOptions, GraphLayout, RawQuad};
use crate::reader::{BytesReader, CountingReader};

const RDFS_LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

fn quads(n: usize, graphs: bool) -> Vec<RawQuad> {
    let words = [
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ];
    let mut q = Vec::new();
    for i in 0..n {
        let s = format!("<http://ex.org/s/{i:05}>");
        q.push((
            s.clone(),
            RDF_TYPE.to_string(),
            format!("<http://ex.org/C{}>", i % 7),
            None,
        ));
        q.push((
            s.clone(),
            RDFS_LABEL.to_string(),
            format!(
                "\"{} {} item {i}\"",
                words[i % words.len()],
                words[(i / 3) % words.len()]
            ),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex.org/p/link>".to_string(),
            format!("<http://ex.org/s/{:05}>", (i * 7 + 3) % n),
            None,
        ));
        q.push((
            s.clone(),
            "<http://ex.org/p/num>".to_string(),
            format!("\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>", i % 97),
            None,
        ));
        if graphs && i % 3 == 0 {
            let g = format!("<http://ex.org/g/{}>", i / 3);
            q.push((
                format!("<http://ex.org/snap/{i}>"),
                "<http://ex.org/p/of>".to_string(),
                s.clone(),
                Some(g.clone()),
            ));
            q.push((
                format!("<http://ex.org/snap/{i}>"),
                "<http://ex.org/p/note>".to_string(),
                format!("\"created {i}\""),
                Some(g),
            ));
        }
    }
    q
}

fn image(n: usize, graphs: bool, text: bool, labels: bool, pyramid: bool) -> Vec<u8> {
    let mut o = AssembleOptions::sectioned();
    o.with_text_index = text;
    o.label_index = labels;
    o.with_pyramid = pyramid;
    o.graph_layout = GraphLayout::Quad;
    let (bytes, _) = assemble_dataset_opts(quads(n, graphs), &o, |_, _| Vec::new());
    bytes
}

const QUERIES: &[&str] = &[
    "SELECT * WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 50",
    "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
    "SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s a ?c } GROUP BY ?c ORDER BY ?c",
    "SELECT ?l WHERE { <http://ex.org/s/00042> <http://www.w3.org/2000/01/rdf-schema#label> ?l }",
    "SELECT ?s WHERE { ?s <http://ex.org/p/link> <http://ex.org/s/00003> }",
    "SELECT ?s ?l WHERE { ?s <http://ex.org/p/link> ?o . ?o <http://www.w3.org/2000/01/rdf-schema#label> ?l } ORDER BY ?s LIMIT 20",
    "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l FILTER(CONTAINS(?l, \"gamma\")) } ORDER BY ?s LIMIT 30",
    "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY ?g LIMIT 15",
    "SELECT ?s ?o WHERE { GRAPH <http://ex.org/g/7> { ?s ?p ?o } } ORDER BY ?s ?o",
    "ASK { ?s <http://ex.org/p/num> 5 }",
    "SELECT ?x WHERE { <http://ex.org/s/00010> <http://ex.org/p/link>+ ?x } LIMIT 10",
];

fn answers(r: &Rete) -> Vec<String> {
    QUERIES
        .iter()
        .map(|q| {
            format!(
                "{:?}",
                crate::sparql::eval_query(r, q).map_err(|e| e.to_string())
            )
        })
        .collect()
}

fn lazy(v6: &[u8]) -> Rete {
    Rete::open_ranged_lazy(BytesReader(std::sync::Arc::from(v6))).unwrap()
}

#[test]
fn converted_file_answers_the_same() {
    for &(graphs, text, labels, pyramid) in &[
        (true, true, true, true),
        (false, false, false, false),
        (true, false, true, false),
    ] {
        let v5 = image(3000, graphs, text, labels, pyramid);
        let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
        assert!(open::is_v06(&v6));
        assert!(crate::file::verify(&v6).unwrap(), "verify");
        let r5 = Rete::open(&v5).unwrap();
        let want = answers(&r5);
        let r6 = Rete::open(&v6).unwrap();
        assert_eq!(answers(&r6), want, "resident open");
        let l6 = lazy(&v6);
        assert_eq!(answers(&l6), want, "lazy open");
        assert!(!l6.index_incomplete());
        if text {
            for w in [&["gamma"][..], &["alpha", "beta"]] {
                assert_eq!(r5.text_search(w, None, 0), l6.text_search(w, None, 0));
            }
            assert_eq!(
                r5.text_search(&[], Some("eps"), 0),
                l6.text_search(&[], Some("eps"), 0)
            );
        }
        if labels {
            assert_eq!(r5.prefix_search("gamma", 40), l6.prefix_search("gamma", 40));
        }
        assert_eq!(r5.graph_names(), l6.graph_names());
    }
}

#[test]
fn conversion_is_deterministic_and_idempotent_on_layout() {
    let v5 = image(500, true, true, true, true);
    let a = convert_image(&v5, WriteOpts::default()).unwrap();
    let b = convert_image(&v5, WriteOpts::default()).unwrap();
    assert_eq!(a, b);
    // `0x07` is order-major whatever the option says.
    let mut seven = WriteOpts {
        quad: write::QuadClustering::GraphMajor,
        ..WriteOpts::default()
    };
    assert_eq!(convert_image(&v5, seven.clone()).unwrap(), a);
    seven.quad = write::QuadClustering::OrderMajor;
    assert_eq!(convert_image(&v5, seven).unwrap(), a);
    // `0x06`: order-major and graph-major hold the same content.
    let mut o = WriteOpts {
        quad: write::QuadClustering::OrderMajor,
        ..v06_opts()
    };
    let om = convert_image(&v5, o.clone()).unwrap();
    o.quad = write::QuadClustering::GraphMajor;
    let gm = convert_image(&v5, o).unwrap();
    let want = answers(&Rete::open(&om).unwrap());
    assert_eq!(answers(&Rete::open(&gm).unwrap()), want);
    // The clustering is in the flags (hashed): a different file identity.
    assert_ne!(
        Header06::from_bytes(&om).unwrap().content_hash,
        Header06::from_bytes(&gm).unwrap().content_hash
    );
}

#[test]
fn small_file_opens_in_one_read() {
    let v5 = image(300, false, false, false, false);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    assert!(
        v6.len() as u64 <= FIRST_READ,
        "fixture must fit the first read"
    );
    let counting = std::sync::Arc::new(CountingReader::new(BytesReader(std::sync::Arc::from(
        &v6[..],
    ))));
    let r = Rete::open_ranged_lazy(counting.clone()).unwrap();
    let _ = crate::sparql::eval_query(&r, QUERIES[4]).unwrap();
    assert_eq!(counting.requests(), 1, "the open and the query read once");
}

#[test]
fn tampering_is_caught_and_named() {
    let v5 = image(800, true, true, true, false);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    let h = Header06::from_bytes(&v6).unwrap();
    // Flip a byte in the middle of the data region.
    let mut bad = v6.clone();
    let mid = (h.data_offset + (h.file_len - h.data_offset) / 2) as usize;
    bad[mid] ^= 0x55;
    assert!(!crate::file::verify(&bad).unwrap());
    let report =
        crate::file::verify_ranged_v06(&crate::reader::SliceReader::new(&bad), &bad, None).unwrap();
    assert!(!report.is_empty());
    // A flipped header count changes the content hash.
    let mut bad = v6.clone();
    bad[48] ^= 1;
    assert!(!crate::file::verify(&bad).unwrap());
    // A truncated file is refused at open.
    assert!(Rete::open(&v6[..v6.len() - 10]).is_err());
}

#[test]
fn recard_rewrites_only_the_card() {
    let v5 = image(400, false, true, true, false);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    let card = br#"{"title":"t"}"#;
    let c1 = replace_card(&v6, Some(card), Some(b"{\"r\":1}")).unwrap();
    assert!(crate::file::verify(&c1).unwrap());
    let h0 = Header06::from_bytes(&v6).unwrap();
    let h1 = Header06::from_bytes(&c1).unwrap();
    assert_eq!(
        h0.root_digest, h1.root_digest,
        "no section moved in content"
    );
    assert_ne!(h0.content_hash, h1.content_hash, "the card is in the hash");
    let r = Rete::open(&c1).unwrap();
    assert_eq!(r.metadata(), Some(&card[..]));
    assert_eq!(answers(&r), answers(&Rete::open(&v6).unwrap()));
    // A different build record: same content hash.
    let c2 = replace_card(&v6, Some(card), Some(b"{\"r\":2, \"x\": 3}")).unwrap();
    assert_eq!(
        Header06::from_bytes(&c2).unwrap().content_hash,
        h1.content_hash
    );
    // Removing the card again restores the cardless identity.
    let c3 = replace_card(&c1, None, None).unwrap();
    assert_eq!(
        Header06::from_bytes(&c3).unwrap().content_hash,
        h0.content_hash
    );
}

/// The streaming re-card writes what the in-memory one does, and an in-place
/// re-card into the region's slack lands on the same content hash with no
/// data byte moved; a pair too big for the slack is refused, not squeezed.
#[test]
fn recard_streams_and_fits_in_place() {
    use super::write::{commit_header, recard_in_place, rewrite_card_stream, NewCard};
    let v5 = image(400, false, true, true, false);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    let card = br#"{"title":"first"}"#;
    let c1 = replace_card(&v6, Some(card), Some(b"{\"r\":1}")).unwrap();

    // Streamed copy == in-memory replace.
    let card2 = br#"{"title":"second, a little longer"}"#;
    let want = replace_card(&c1, Some(card2), Some(b"{\"r\":2}")).unwrap();
    let mut out = Vec::new();
    let mut seen = 0u64;
    let src = crate::reader::SliceReader::new(&c1);
    rewrite_card_stream(
        &src,
        &mut out,
        NewCard::Set(Some(card2)),
        Some(b"{\"r\":2}"),
        &mut |n| seen = n,
    )
    .unwrap();
    assert_eq!(out, want);
    let h1 = Header06::from_bytes(&c1).unwrap();
    assert_eq!(seen, h1.file_len - h1.data_offset);

    // In place: the new pair beside the old one, then the header.
    let mut f = std::io::Cursor::new(c1.clone());
    let nh = recard_in_place(&mut f, NewCard::Set(Some(card2)), Some(b"{\"r\":2}"))
        .unwrap()
        .expect("fits the slack");
    assert_eq!(
        &f.get_ref()[..HEADER_LEN],
        &c1[..HEADER_LEN],
        "header untouched before commit"
    );
    commit_header(&mut f, &nh).unwrap();
    let c2 = f.into_inner();
    assert_eq!(c2.len(), c1.len(), "nothing appended");
    assert_eq!(
        &c2[nh.data_offset as usize..],
        &c1[h1.data_offset as usize..]
    );
    assert_eq!(
        nh.content_hash,
        Header06::from_bytes(&want).unwrap().content_hash
    );
    assert!(crate::file::verify(&c2).unwrap());
    let r = Rete::open(&c2).unwrap();
    assert_eq!(r.metadata(), Some(&card2[..]));

    // A second in-place re-card reuses the gap the first one freed.
    let mut f = std::io::Cursor::new(c2);
    let nh3 = recard_in_place(&mut f, NewCard::Set(Some(card)), Some(b"{\"r\":3}"))
        .unwrap()
        .expect("fits the freed gap");
    commit_header(&mut f, &nh3).unwrap();
    let c3 = f.into_inner();
    assert!(crate::file::verify(&c3).unwrap());
    assert_eq!(nh3.content_hash, h1.content_hash, "same card, same hash");

    // Only the record changes: the card stays put, the hash too.
    let mut f = std::io::Cursor::new(c3);
    let nh4 = recard_in_place(&mut f, NewCard::Keep, Some(b"{\"r\":4, \"costs\":[1,2,3]}"))
        .unwrap()
        .expect("a record fits");
    assert_eq!(nh4.card_off, nh3.card_off);
    assert_eq!(nh4.content_hash, h1.content_hash);

    // Too big for the slack: refused, the file untouched.
    let huge = vec![b' '; 64 << 10];
    let mut f = std::io::Cursor::new(c1.clone());
    assert!(
        recard_in_place(&mut f, NewCard::Set(Some(card)), Some(&huge))
            .unwrap()
            .is_none()
    );
    assert_eq!(f.into_inner(), c1);
}

/// `rete convert` gives a file without a LABEL_INDEX section the one
/// `rete build --label-index` would have written: converting the unlabeled
/// build with the computed section is byte-identical to converting the
/// labeled build.
#[test]
fn convert_builds_the_label_index_a_build_would() {
    use super::write::{convert_with_labels, label_section};
    let bare = image(2000, true, true, false, true);
    let labeled = image(2000, true, true, true, true);
    let sec = label_section(&Rete::open(&bare).unwrap());
    assert!(!sec.is_empty());
    let mut out = std::io::Cursor::new(Vec::new());
    convert_with_labels(
        &crate::reader::SliceReader::new(&bare),
        Some(&sec),
        &mut out,
        WriteOpts::default(),
    )
    .unwrap();
    let got = out.into_inner();
    let want = convert_image(&labeled, WriteOpts::default()).unwrap();
    assert_eq!(got, want);
    let r = lazy(&got);
    assert!(!r.prefix_search("gamma", 10).is_empty());
}

/// Records every range read.
struct Logged(std::sync::Arc<[u8]>, std::sync::Mutex<Vec<(u64, u64)>>);

impl crate::reader::RangeReader for Logged {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, o: u64, l: u64) -> std::io::Result<Vec<u8>> {
        self.1.lock().unwrap().push((o, l));
        self.0
            .get(o as usize..(o + l) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::other("oob"))
    }
}

/// A join that probes thousands of subjects routed to the same leaf fetches
/// that leaf once, not once per subject (it used to send one batch of 8,572
/// copies of a 256-byte leaf here, and 2.7 GB for 460 k subjects on the
/// OpenCitations sample).
#[test]
fn a_probe_join_faults_each_leaf_once() {
    let v5 = image(60000, true, true, true, false);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    let q = "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s a <http://ex.org/C3> ; ?p ?o } GROUP BY ?p";
    let want = format!(
        "{:?}",
        crate::sparql::eval_query(&Rete::open(&v5).unwrap(), q).unwrap()
    );
    let r = std::sync::Arc::new(Logged(std::sync::Arc::from(&v6[..]), Default::default()));
    let rete = Rete::open_ranged_lazy_with(r.clone(), 1024).unwrap();
    r.1.lock().unwrap().clear();
    let got = format!("{:?}", crate::sparql::eval_query(&rete, q).unwrap());
    assert_eq!(got, want);
    let (h, root) = open::read_front(&crate::reader::SliceReader::new(&v6)).unwrap();
    let leaves = root.section(kind::INDEX_LEAVES).unwrap().abs(&h).unwrap();
    let reads = r.1.lock().unwrap().clone();
    let leaf_reads = reads
        .iter()
        .filter(|&&(o, _)| o >= leaves.offset && o < leaves.offset + leaves.len)
        .count();
    assert!(leaf_reads <= 6, "{leaf_reads} index-leaf reads");
    assert!(reads.len() < 40, "{} reads for the query", reads.len());
}

/// The writer records each section's largest block, every block fits its
/// section's bound, and a bound one byte short is refused.
#[test]
fn every_section_records_its_largest_block() {
    let v5 = image(3000, true, true, true, true);
    let v6 = convert_image(&v5, WriteOpts::default()).unwrap();
    let r = crate::reader::SliceReader::new(&v6);
    let (h, root) = open::read_front(&r).unwrap();
    for k in [
        kind::SCHEMA,
        kind::DICT_LEAVES,
        kind::INDEX_LEAVES,
        kind::QUAD_LEAVES,
        kind::TEXT_LEAVES,
        kind::DICT_DATA,
        kind::INDEX_DATA,
        kind::QUAD_DATA,
    ] {
        let s = root
            .section(k)
            .unwrap_or_else(|| panic!("{}", kind::name(k)));
        assert!(s.max_block > 0, "{} records no maximum", kind::name(k));
    }
    // Every tile of the index fits its bound exactly: the largest one is it.
    let f = open::V06File::open(
        std::sync::Arc::new(BytesReader(std::sync::Arc::from(&v6[..]))),
        &v6,
    )
    .unwrap();
    let max = f.bound(kind::INDEX_DATA);
    let index = f.index(1).unwrap();
    let mut largest = 0u64;
    for (si, list) in index.tile_sections().iter().enumerate() {
        list.ensure_span(0, list.len());
        for ti in 0..list.len() {
            let (off, len) = index.tile_range(si, ti).unwrap();
            let stored = &v6[off as usize..(off + len) as usize];
            let raw = crate::file::decompress_bounded(h.block_codec, stored, max).unwrap();
            largest = largest.max(raw.len() as u64);
            if raw.len() as u64 == max {
                assert!(crate::file::decompress_bounded(h.block_codec, stored, max - 1).is_err());
            }
        }
    }
    assert_eq!(largest, max, "the recorded maximum is the largest tile");
}

/// Leaves per group of a `0x06` image: dictionary sections, index orders, quad
/// orders and the text index, from the root alone.
fn leaf_counts(v6: &[u8]) -> (Vec<usize>, Vec<usize>, Vec<usize>, usize) {
    let (_, root) = open::read_front(&crate::reader::SliceReader::new(v6)).unwrap();
    let dict = root
        .unit(tag::DICT)
        .map(|u| {
            decode_dict_root(u)
                .unwrap()
                .iter()
                .map(|s| s.leaves.len())
                .collect()
        })
        .unwrap_or_default();
    let index = root
        .unit(tag::INDEX)
        .map(|u| {
            decode_index_root(u)
                .unwrap()
                .iter()
                .map(|p| p.leaves.len())
                .collect()
        })
        .unwrap_or_default();
    let quad = root
        .unit(tag::QUAD)
        .map(|u| {
            decode_quad_root(u)
                .unwrap()
                .orders
                .iter()
                .map(|o| o.leaves.len())
                .collect()
        })
        .unwrap_or_default();
    let text = root
        .unit(tag::TEXT)
        .map(|u| decode_text_root(u).unwrap().leaves.len())
        .unwrap_or(0);
    (dict, index, quad, text)
}

/// Leaves of a few hundred bytes, so a small fixture has several per directory.
fn small_leaves() -> write::LeafPolicy {
    write::LeafPolicy {
        start: 256,
        max: 1024,
        root_budget: 1,
        list_cap: 1 << 20,
    }
}

/// One leaf per directory: where the writer used to end on a big file.
fn one_leaf() -> write::LeafPolicy {
    write::LeafPolicy {
        start: usize::MAX / 4,
        ..write::LeafPolicy::default()
    }
}

/// The writer cuts a directory larger than the leaf size into several leaves,
/// and keeps them when the root is over budget for another reason: the
/// graph-directory block index (or anything else that is not a leaf list)
/// cannot be shrunk by bigger leaves.
#[test]
fn a_directory_larger_than_the_target_gets_several_leaves() {
    let v5 = image(3000, true, true, true, false);
    let opts = |leaves| WriteOpts {
        leaves,
        ..WriteOpts::default()
    };
    let single = convert_image(&v5, opts(one_leaf())).unwrap();
    let (d, i, q, t) = leaf_counts(&single);
    assert!(d.iter().chain(&i).chain(&q).all(|&n| n <= 1));
    assert!(t <= 1);
    // A 1-byte root budget cannot be met even by one leaf per directory: the
    // old loop grew every directory to one leaf anyway; the leaves stay small.
    let kept = convert_image(&v5, opts(small_leaves())).unwrap();
    let (_, _, _, t) = leaf_counts(&kept);
    assert!(t >= 2, "text leaves: {t}");
}

/// The leaf lists add at most `list_cap` to the root of one leaf per
/// directory: past it, leaves grow.
#[test]
fn the_leaf_lists_are_capped() {
    let v5 = image(3000, true, true, true, false);
    let root_len = |leaves| {
        let v6 = convert_image(
            &v5,
            WriteOpts {
                leaves,
                ..WriteOpts::default()
            },
        )
        .unwrap();
        Header06::from_bytes(&v6).unwrap().root_len as usize
    };
    let floor = root_len(one_leaf());
    let free = root_len(small_leaves());
    assert!(
        free > floor + 256,
        "the fixture's leaf lists: {free} vs {floor}"
    );
    let capped = root_len(write::LeafPolicy {
        list_cap: 256,
        ..small_leaves()
    });
    assert!(capped < free, "{capped} < {free}");
    assert!(capped <= floor + 256 + 64, "{capped} vs {floor} + 256");
}

/// A file written with small leaves answers every pattern shape as one written
/// with one leaf per directory, on the resident and the lazy open, verifies,
/// and is what a rewrite of the other produces byte for byte.
#[test]
fn leaf_sizes_do_not_change_answers() {
    for &(graphs, text, labels, pyramid) in &[
        (true, true, true, true),
        (false, false, false, false),
        (true, false, true, false),
    ] {
        let v5 = image(3000, graphs, text, labels, pyramid);
        let opts = |leaves| WriteOpts {
            leaves,
            ..WriteOpts::default()
        };
        let want = answers(&Rete::open(&v5).unwrap());
        let small = convert_image(&v5, opts(small_leaves())).unwrap();
        let single = convert_image(&v5, opts(one_leaf())).unwrap();
        assert_eq!(
            small != single,
            text,
            "small leaves differ where a directory has entries to cut"
        );
        for v6 in [&small, &single] {
            assert!(crate::file::verify(v6).unwrap(), "verify");
            let rep =
                crate::file::verify_ranged_v06(&crate::reader::SliceReader::new(v6), v6, None)
                    .unwrap();
            assert!(rep.is_empty(), "{rep:?}");
            assert_eq!(answers(&Rete::open(v6).unwrap()), want, "resident");
            assert_eq!(answers(&lazy(v6)), want, "lazy");
        }
        // Rewriting changes the directories and nothing else.
        for (from, to, leaves) in [
            (&single, &small, small_leaves()),
            (&small, &single, one_leaf()),
        ] {
            let mut out = std::io::Cursor::new(Vec::new());
            write::rewrite(
                &crate::reader::SliceReader::new(from),
                &mut out,
                opts(leaves),
            )
            .unwrap();
            assert_eq!(&out.into_inner(), to, "rewrite == convert");
        }
    }
}

// ---------------------------------------------------------------------------
// 0x07: the label root's own section, the card annex, the generation byte
// ---------------------------------------------------------------------------

fn v06_opts() -> WriteOpts {
    WriteOpts {
        version: VERSION_06,
        ..WriteOpts::default()
    }
}

/// Records every range read (offset, length).
struct Recording {
    inner: BytesReader,
    log: std::sync::Mutex<Vec<(u64, u64)>>,
}

impl crate::reader::RangeReader for Recording {
    fn len(&self) -> u64 {
        self.inner.len()
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.log.lock().unwrap().push((offset, len));
        self.inner.read_at(offset, len)
    }
}

fn big_extra_card() -> Vec<u8> {
    // 114 checksums, the OpenCitations Index card's shape: ~13 KB of `extra`.
    let md5s: Vec<String> = (0..114)
        .map(|i| format!("{:032x}", (i as u128) * 0x9e37_79b9_7f4a_7c15))
        .collect();
    let extra =
        serde_json::json!({ "sources": md5s.join(", "), "n": 114, "note": "x".repeat(8000) });
    serde_json::to_vec(&serde_json::json!({"title": "annexed", "extra": extra})).unwrap()
}

fn front_of(bytes: &[u8]) -> (Header06, Root06) {
    open::read_front(&crate::reader::SliceReader::new(bytes)).unwrap()
}

#[test]
fn writers_write_0x07_and_the_label_root_has_its_own_section() {
    let v5 = image(1500, true, true, true, false);
    let v7 = convert_image(&v5, WriteOpts::default()).unwrap();
    assert_eq!(v7[4], 7);
    assert_eq!(&v7[v7.len() - 8..], &footer(7));
    let (h, root) = front_of(&v7);
    assert_eq!(h.version, VERSION);
    assert!(root.section(kind::LABEL_ROOT).is_some());
    let unit = decode_label_root(root.unit(tag::LABELS).unwrap()).unwrap();
    assert_eq!(unit.head.len(), 32, "the root keeps the preamble only");
    assert!(crate::file::verify(&v7).unwrap());
    // The same file as 0x06 carries the root node in the root.
    let v6 = convert_image(&v5, v06_opts()).unwrap();
    assert_eq!(v6[4], 6);
    let (h6, root6) = front_of(&v6);
    assert_eq!(h6.version, VERSION_06);
    assert!(root6.section(kind::LABEL_ROOT).is_none());
    assert!(crate::file::verify(&v6).unwrap());
    let want = unit.head.len() as u64 + root.section(kind::LABEL_ROOT).unwrap().len;
    let got6 = decode_label_root(root6.unit(tag::LABELS).unwrap())
        .unwrap()
        .head
        .len() as u64;
    assert_eq!(got6, want);
    // The root node (a few bytes on this small file) is out of the root; the
    // price is one 38-byte table entry. (A file without named graphs: the quad
    // orders of 0x07 are a different set from 0x06's and change the root too.)
    let plain = image(1500, false, true, true, false);
    let (h7p, _) = front_of(&convert_image(&plain, WriteOpts::default()).unwrap());
    let (h6p, _) = front_of(&convert_image(&plain, v06_opts()).unwrap());
    assert!(h7p.root_len as usize <= h6p.root_len as usize + SEC_ENTRY_LEN);
}

#[test]
fn convert_0x06_to_0x07_keeps_every_answer() {
    for &(graphs, text, labels, pyramid) in &[
        (true, true, true, true),
        (false, false, false, false),
        (true, false, true, false),
        (false, true, true, false),
    ] {
        let v5 = image(1200, graphs, text, labels, pyramid);
        let v6 = convert_image(&v5, v06_opts()).unwrap();
        let src = crate::reader::SliceReader::new(&v6);
        let mut out = std::io::Cursor::new(Vec::new());
        write::rewrite(&src, &mut out, WriteOpts::default()).unwrap();
        let v7 = out.into_inner();
        assert_eq!(v7[4], 7);
        assert!(crate::file::verify(&v7).unwrap());
        // Identical to converting the 0x05 source straight to 0x07.
        assert_eq!(v7, convert_image(&v5, WriteOpts::default()).unwrap());
        let want = answers(&Rete::open(&v6).unwrap());
        assert_eq!(answers(&Rete::open(&v7).unwrap()), want, "resident");
        assert_eq!(answers(&lazy(&v7)), want, "lazy");
        // The card and the build record are kept.
        assert_eq!(
            Rete::open(&v7).unwrap().metadata(),
            Rete::open(&v6).unwrap().metadata()
        );
    }
}

#[test]
fn a_label_search_reads_the_label_root_only_when_it_runs() {
    let v5 = image(4000, false, false, true, false);
    let v7 = convert_image(&v5, WriteOpts::default()).unwrap();
    let (h, root) = front_of(&v7);
    let lr = root.section(kind::LABEL_ROOT).unwrap().abs(&h).unwrap();
    let rec = std::sync::Arc::new(Recording {
        inner: BytesReader(std::sync::Arc::from(&v7[..])),
        log: Default::default(),
    });
    let touches = |rec: &Recording| {
        rec.log
            .lock()
            .unwrap()
            .iter()
            .any(|&(o, l)| o < lr.offset + lr.len && lr.offset < o + l)
    };
    let r = Rete::open_ranged_lazy_with(rec.clone(), 1024).unwrap();
    let _ = crate::sparql::eval_query(&r, QUERIES[4]).unwrap();
    assert!(!touches(&rec), "a query does not read the label root");
    let hits = r.prefix_search("gamma", 10);
    assert!(!hits.is_empty());
    assert!(touches(&rec), "a label search does");
    let want = Rete::open(&v5).unwrap().prefix_search("gamma", 10);
    assert_eq!(hits, want);
}

#[test]
fn verify_catches_a_corrupted_label_root() {
    let v5 = image(1500, false, false, true, false);
    let v7 = convert_image(&v5, WriteOpts::default()).unwrap();
    let (h, root) = front_of(&v7);
    let a = root.section(kind::LABEL_ROOT).unwrap().abs(&h).unwrap();
    let mut bad = v7.clone();
    bad[(a.offset + a.len / 2) as usize] ^= 0x40;
    let rep =
        crate::file::verify_ranged_v06(&crate::reader::SliceReader::new(&bad), &bad, None).unwrap();
    assert_eq!(rep, vec!["label-root".to_string()]);
    assert!(!crate::file::verify(&bad).unwrap());
}

#[test]
fn a_card_over_the_hot_cap_goes_to_the_annex_and_comes_back() {
    let v5 = image(400, false, false, true, false);
    let v7 = convert_image(&v5, WriteOpts::default()).unwrap();
    let card = big_extra_card();
    assert!(card.len() > annex::HOT_FIELD_MAX);
    let c1 = replace_card(&v7, Some(&card), Some(b"{\"r\":1}")).unwrap();
    assert!(crate::file::verify(&c1).unwrap());
    let r = crate::reader::SliceReader::new(&c1);
    let (h, root) = open::read_front(&r).unwrap();
    let a = root.section(kind::CARD_ANNEX).expect("an annex section");
    // The annex is the last section, right before the footer.
    assert_eq!(a.abs(&h).unwrap().offset + a.len + 8, h.file_len);
    // The hot card is small and says so; the full card is the one written.
    let hot = open::read_card_build(&r, &h).unwrap().0.unwrap();
    assert!(hot.len() < annex::HOT_FIELD_MAX && annex::has_marker(&hot));
    let (full, merged) = open::read_card_full(&r).unwrap();
    assert!(merged);
    let full: serde_json::Value = serde_json::from_slice(&full.unwrap()).unwrap();
    let want: serde_json::Value = serde_json::from_slice(&card).unwrap();
    assert_eq!(full, want);
    assert!(crate::file::read_card_full_ranged(&r).unwrap().is_some());
    // Convert (0x07 to 0x07) carries the annex and the card.
    let mut out = std::io::Cursor::new(Vec::new());
    write::rewrite(&r, &mut out, WriteOpts::default()).unwrap();
    assert_eq!(out.into_inner(), c1, "the same file written again");
    // Answers are untouched.
    assert_eq!(
        answers(&Rete::open(&c1).unwrap()),
        answers(&Rete::open(&v7).unwrap())
    );
}

#[test]
fn verify_catches_a_corrupted_annex() {
    let v7 = convert_image(
        &image(300, false, false, false, false),
        WriteOpts::default(),
    )
    .unwrap();
    let c1 = replace_card(&v7, Some(&big_extra_card()), None).unwrap();
    let (h, root) = front_of(&c1);
    let a = root.section(kind::CARD_ANNEX).unwrap().abs(&h).unwrap();
    let mut bad = c1.clone();
    bad[(a.offset + a.len / 2) as usize] ^= 1;
    let rep =
        crate::file::verify_ranged_v06(&crate::reader::SliceReader::new(&bad), &bad, None).unwrap();
    assert_eq!(rep, vec!["card-annex".to_string()]);
    assert!(open::read_card_full(&crate::reader::SliceReader::new(&bad)).is_err());
}

#[test]
fn recard_with_an_annex_streams_equal_and_in_place_only_when_it_is_unchanged() {
    use super::write::{recard_in_place, rewrite_card_stream, NewCard};
    let v7 = convert_image(
        &image(300, false, false, false, false),
        WriteOpts::default(),
    )
    .unwrap();
    let card = big_extra_card();
    let stream = |src: &[u8], c: NewCard<'_>| {
        let mut out = Vec::new();
        rewrite_card_stream(
            &crate::reader::SliceReader::new(src),
            &mut out,
            c,
            Some(b"{\"r\":9}"),
            &mut |_| {},
        )
        .unwrap();
        out
    };
    let c1 = stream(&v7, NewCard::Set(Some(&card)));
    assert_eq!(
        c1,
        replace_card(&v7, Some(&card), Some(b"{\"r\":9}")).unwrap()
    );
    assert!(crate::file::verify(&c1).unwrap());
    // Same card again: unchanged annex, so an in-place re-card fits.
    let mut f = std::io::Cursor::new(c1.clone());
    let nh = recard_in_place(&mut f, NewCard::Set(Some(&card)), Some(b"{\"r\":10}"))
        .unwrap()
        .expect("fits in the card region");
    write::commit_header(&mut f, &nh).unwrap();
    assert!(crate::file::verify(f.get_ref()).unwrap());
    assert_eq!(
        nh.content_hash,
        Header06::from_bytes(&c1).unwrap().content_hash
    );
    // A different big field: the annex changes, so it is a copy.
    let mut other = serde_json::from_slice::<serde_json::Value>(&card).unwrap();
    other["extra"]["n"] = serde_json::json!(115);
    let other = serde_json::to_vec(&other).unwrap();
    let mut f = std::io::Cursor::new(c1.clone());
    assert!(recard_in_place(&mut f, NewCard::Set(Some(&other)), None)
        .unwrap()
        .is_none());
    let c2 = stream(&c1, NewCard::Set(Some(&other)));
    assert!(crate::file::verify(&c2).unwrap());
    // Dropping the big field drops the annex.
    let c3 = stream(&c2, NewCard::Set(Some(br#"{"title":"small"}"#)));
    assert!(crate::file::verify(&c3).unwrap());
    assert!(front_of(&c3).1.section(kind::CARD_ANNEX).is_none());
    assert!(c3.len() < c2.len());
    // Keep leaves the annex alone.
    let c4 = stream(&c2, NewCard::Keep);
    assert!(front_of(&c4).1.section(kind::CARD_ANNEX).is_some());
    assert!(crate::file::verify(&c4).unwrap());
}

#[test]
fn a_0x06_file_has_no_annex() {
    let v6 = convert_image(&image(300, false, false, false, false), v06_opts()).unwrap();
    let e = replace_card(&v6, Some(&big_extra_card()), None).unwrap_err();
    assert!(e.to_string().contains("rete convert"), "{e}");
}

#[test]
fn a_reader_refuses_a_generation_it_does_not_know_by_name() {
    let v7 = convert_image(
        &image(100, false, false, false, false),
        WriteOpts::default(),
    )
    .unwrap();
    let mut future = v7.clone();
    future[4] = 8;
    let e = Rete::open(&future).err().expect("refused").to_string();
    assert!(e.contains("0x08") && e.contains("0x05"), "{e}");
}
