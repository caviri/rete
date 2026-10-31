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
    let words = ["alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta"];
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
        .map(|q| format!("{:?}", crate::sparql::eval_query(r, q).map_err(|e| e.to_string())))
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
    let mut o = WriteOpts::default();
    o.quad = write::QuadClustering::OrderMajor;
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
    assert!(v6.len() as u64 <= FIRST_READ, "fixture must fit the first read");
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
    let report = crate::file::verify_ranged_v06(&crate::reader::SliceReader::new(&bad), &bad, None)
        .unwrap();
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
    assert_eq!(h0.root_digest, h1.root_digest, "no section moved in content");
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
    assert_eq!(Header06::from_bytes(&c3).unwrap().content_hash, h0.content_hash);
}
