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
    // Order-major and graph-major hold the same content.
    let mut o = WriteOpts {
        quad: write::QuadClustering::OrderMajor,
        ..WriteOpts::default()
    };
    let om = convert_image(&v5, o).unwrap();
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
    let leaves = root.section(kind::INDEX_LEAVES).unwrap().abs(&h);
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
