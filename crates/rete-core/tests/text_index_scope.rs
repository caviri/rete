//! The full-text index's **scope** (`rete build --text-index-graphs`,
//! `--text-index-include`, `--text-index-exclude`) against SPARQL: the
//! `FILTER(CONTAINS(…))` pushdown prunes subjects with the index only where the
//! index covers the active graph and the pattern's predicate. Every query here
//! must return exactly the rows a file without any text index returns, for every
//! scope — the index may only ever make a query faster.

use rete_core::ingest::{assemble_dataset_with_opts, RawQuad};
use rete_core::{eval_sparql, Rete, TextIndexGraphs, TextIndexScope};

fn quad(s: &str, p: &str, o: &str, g: Option<&str>) -> RawQuad {
    (
        format!("<http://ex/{s}>"),
        format!("<http://ex/{p}>"),
        o.to_string(),
        g.map(|g| format!("<http://ex/{g}>")),
    )
}

/// Data in the default graph, provenance in two named graphs: the shape of an
/// OpenCitations entity and its `…/prov/` graph.
fn quads() -> Vec<RawQuad> {
    vec![
        quad("e1", "title", "\"Glucose metabolism\"", None),
        quad("e1", "desc", "\"The entity has been modified.\"", None),
        quad("e2", "title", "\"Benzene ring\"", None),
        quad("e3", "desc", "\"modified twice\"", None),
        quad("e3", "kind", "<http://ex/Thing>", None),
        quad("p1", "desc", "\"The entity has been created.\"", Some("g1")),
        quad("p1", "note", "\"created by an agent\"", Some("g1")),
        quad(
            "p2",
            "desc",
            "\"The entity has been modified.\"",
            Some("g2"),
        ),
    ]
}

fn build(scope: Option<TextIndexScope>) -> Vec<u8> {
    let (bytes, _) = assemble_dataset_with_opts(quads(), false, scope, None, |_, _| Vec::new());
    bytes
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    let (_, sols) = eval_sparql(rete, &format!("PREFIX ex: <http://ex/> {q}")).unwrap();
    let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("s").cloned()).collect();
    v.sort();
    v
}

fn scopes() -> Vec<(&'static str, TextIndexScope)> {
    let none: [&str; 0] = [];
    vec![
        ("default", TextIndexScope::default()),
        (
            "all graphs",
            TextIndexScope::from_options(TextIndexGraphs::All, none, none).unwrap(),
        ),
        (
            "exclude desc",
            TextIndexScope::from_options(TextIndexGraphs::Default, none, ["http://ex/desc"])
                .unwrap(),
        ),
        (
            "include desc, all graphs",
            TextIndexScope::from_options(TextIndexGraphs::All, ["http://ex/desc"], none).unwrap(),
        ),
        (
            "include title",
            TextIndexScope::from_options(TextIndexGraphs::Default, ["<http://ex/title>"], none)
                .unwrap(),
        ),
    ]
}

const QUERIES: &[&str] = &[
    // The default graph, a covered or an excluded predicate.
    "SELECT ?s WHERE { ?s ex:desc ?o FILTER(CONTAINS(?o, \"modified\")) }",
    "SELECT ?s WHERE { ?s ex:title ?o FILTER(CONTAINS(?o, \"Glucose\")) }",
    // A variable predicate: only an every-predicate index may prune it.
    "SELECT ?s WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"modified\")) }",
    // Inside GRAPH: a default-graph index never saw these literals (the
    // "CONTAINS inside GRAPH returns 0 rows" defect).
    "SELECT ?s WHERE { GRAPH ex:g1 { ?s ?p ?o FILTER(CONTAINS(?o, \"created\")) } }",
    "SELECT ?s WHERE { GRAPH ?g { ?s ex:desc ?o FILTER(CONTAINS(?o, \"modified\")) } }",
    // FROM makes a named graph the active default graph.
    "SELECT ?s FROM ex:g1 WHERE { ?s ?p ?o FILTER(CONTAINS(?o, \"created\")) }",
];

#[test]
fn every_scope_answers_contains_like_a_file_without_an_index() {
    let plain = build(None);
    let rete_plain = Rete::open(&plain).unwrap();
    for q in QUERIES {
        assert!(
            !rows(&rete_plain, q).is_empty(),
            "fixture answers nothing: {q}"
        );
    }
    for (name, scope) in scopes() {
        let bytes = build(Some(scope.clone()));
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(
            rete.header().text_index_partial,
            scope.restricts_predicates()
        );
        for q in QUERIES {
            assert_eq!(
                rows(&rete, q),
                rows(&rete_plain, q),
                "scope {name}: indexed and plain diverge for {q}"
            );
        }
    }
}

/// `rete search` reads the same scope: named-graph text only under `all`,
/// excluded predicates never.
#[test]
fn text_search_follows_the_scope() {
    let search = |scope: TextIndexScope, word: &str| {
        let bytes = build(Some(scope));
        Rete::open(&bytes).unwrap().text_search(&[word], None, 0)
    };
    let all = scopes();
    assert_eq!(search(all[0].1.clone(), "created"), Vec::<String>::new());
    assert_eq!(search(all[1].1.clone(), "created"), ["<http://ex/p1>"]);
    assert_eq!(
        search(all[0].1.clone(), "modified"),
        ["<http://ex/e1>", "<http://ex/e3>"]
    );
    assert_eq!(
        search(all[1].1.clone(), "modified"),
        ["<http://ex/e1>", "<http://ex/e3>", "<http://ex/p2>"]
    );
    assert_eq!(search(all[2].1.clone(), "modified"), Vec::<String>::new());
    assert_eq!(search(all[2].1.clone(), "glucose"), ["<http://ex/e1>"]);
    assert_eq!(search(all[4].1.clone(), "modified"), Vec::<String>::new());
}

/// The default scope is the pre-0.4.0 index, byte for byte: no scope block,
/// section kind 6.
#[test]
fn default_scope_is_the_unscoped_index() {
    let unscoped = {
        let (b, _) = assemble_dataset_with_opts(quads(), false, true, None, |_, _| Vec::new());
        b
    };
    assert_eq!(build(Some(TextIndexScope::default())), unscoped);
    let h = rete_core::Header::from_bytes(&unscoped).unwrap();
    assert!(h.text_index_len > 0 && !h.text_index_partial);
}

// ---------------------------------------------------------------------------
// Kind 11 outside the content hash; the named-graphs bit decides coverage
// ---------------------------------------------------------------------------

use rete_core::format::content_hash_parts;
use rete_core::{verify, verify_ranged, Header, RangeReader, SectionKind, SliceReader};
use std::sync::{Arc, Mutex};

fn exclude_desc() -> TextIndexScope {
    let none: [&str; 0] = [];
    TextIndexScope::from_options(TextIndexGraphs::Default, none, ["http://ex/desc"]).unwrap()
}

/// A predicate-restricted index (kind 11) is outside the content hash, like
/// kinds 8 and 9, and carries its own leading digest: a reader older than 0.4.0
/// cannot see kind 11, and a hashed one would make its `verify` call the file
/// corrupt. The hash is that of the same build without a text index; the new
/// `verify` still catches a flipped byte in the section.
#[test]
fn a_restricted_text_index_is_self_digested_not_hashed() {
    let bytes = build(Some(exclude_desc()));
    let h = Header::from_bytes(&bytes).unwrap();
    assert!(h.text_index_partial);
    let k11 = h.section(SectionKind::TextIndexPartial).unwrap();
    assert!(h
        .section(SectionKind::TextIndex)
        .is_none_or(|s| s.length == 0));
    assert!(
        content_hash_parts(&h)
            .iter()
            .all(|&(off, _)| off != k11.offset),
        "kind 11 is not folded into the content hash"
    );
    let plain = Header::from_bytes(&build(None)).unwrap();
    assert_eq!(h.content_hash, plain.content_hash);
    // The section is `digest | kind-6 payload`.
    let (p_off, p_len) = h.text_index_payload();
    assert_eq!((p_off, p_len), (k11.offset + 16, k11.length - 16));
    let section = &bytes[k11.offset as usize..(k11.offset + k11.length) as usize];
    assert_eq!(
        &section[..16],
        &blake3::hash(&section[16..]).as_bytes()[..16]
    );
    assert!(verify(&bytes).unwrap());
    assert!(verify_ranged(&SliceReader::new(&bytes)).unwrap());
    for at in [k11.offset + 3, k11.offset + 20, k11.offset + k11.length - 1] {
        let mut bad = bytes.clone();
        bad[at as usize] ^= 0x40;
        assert!(!verify(&bad).unwrap(), "a flipped byte at {at} in kind 11");
        assert!(!verify_ranged(&SliceReader::new(&bad)).unwrap());
    }
    // ... and it still answers: the digest is skipped on every open path.
    let rete = Rete::open(&bytes).unwrap();
    assert_eq!(
        rows(
            &rete,
            r#"SELECT ?s WHERE { ?s ex:title ?t FILTER(CONTAINS(?t, "Glucose")) }"#
        ),
        ["<http://ex/e1>"]
    );
    let lazy = Rete::open_ranged_lazy_with(Recorded::new(bytes.clone()), rete_core::HEADER_LEN as u64).unwrap();
    assert!(lazy.text_index_scope().unwrap().restricts_predicates());
    assert_eq!(
        rows(
            &lazy,
            r#"SELECT ?s WHERE { ?s ex:title ?t FILTER(CONTAINS(?t, "Glucose")) }"#
        ),
        ["<http://ex/e1>"]
    );
}

/// The header says whether the index covers the named graphs (flag bit 4), so
/// coverage off the default graph is decided without the token table.
#[test]
fn the_named_graphs_bit_follows_the_scope() {
    for (name, scope) in scopes() {
        let h = Header::from_bytes(&build(Some(scope.clone()))).unwrap();
        assert_eq!(
            h.text_index_covers_named_graphs(),
            scope.covers_named_graphs(),
            "{name}"
        );
        assert_eq!(h.text_index_partial, scope.restricts_predicates(), "{name}");
    }
    assert!(!Header::from_bytes(&build(None))
        .unwrap()
        .text_index_covers_named_graphs());
}

/// Records every range a lazy open reads.
#[derive(Clone)]
struct Recorded {
    data: Arc<Vec<u8>>,
    reads: Arc<Mutex<Vec<(u64, u64)>>>,
}

impl Recorded {
    fn new(data: Vec<u8>) -> Self {
        Recorded {
            data: Arc::new(data),
            reads: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn take(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl RangeReader for Recorded {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        SliceReader::new(&self.data).read_at(offset, len)
    }
}

fn touches(reads: &[(u64, u64)], (off, len): (u64, u64)) -> bool {
    reads.iter().any(|&(o, l)| o < off + len && off < o + l)
}

/// `FILTER(CONTAINS(…))` inside `GRAPH` on a lazily opened default-scope file
/// scans, and reads **no byte** of the text index to find that out: the
/// header's named-graphs bit is absent. With `--text-index-graphs all` the
/// same query uses the index, and both answer the same rows.
#[test]
fn contains_inside_graph_reads_the_text_index_only_when_it_covers_the_graphs() {
    let q = r#"SELECT ?s WHERE { GRAPH ex:g1 { ?s ex:desc ?d FILTER(CONTAINS(?d, "created")) } }"#;
    for (scope, uses_index) in [
        (TextIndexScope::default(), false),
        (scopes()[1].1.clone(), true),
    ] {
        assert_eq!(scope.covers_named_graphs(), uses_index);
        let bytes = build(Some(scope));
        let h = Header::from_bytes(&bytes).unwrap();
        let text = (h.text_index_offset, h.text_index_len);
        let rec = Recorded::new(bytes);
        let rete = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
        rec.take();
        assert_eq!(rows(&rete, q), ["<http://ex/p1>"]);
        assert_eq!(
            touches(&rec.take(), text),
            uses_index,
            "GRAPH + CONTAINS, uses_index = {uses_index}"
        );
    }
    // On the default graph a default-scope index is used, as before.
    let bytes = build(Some(TextIndexScope::default()));
    let h = Header::from_bytes(&bytes).unwrap();
    let rec = Recorded::new(bytes);
    let rete = Rete::open_ranged_lazy_with(rec.clone(), rete_core::HEADER_LEN as u64).unwrap();
    rec.take();
    assert_eq!(
        rows(
            &rete,
            r#"SELECT ?s WHERE { ?s ex:title ?t FILTER(CONTAINS(?t, "Glucose")) }"#
        ),
        ["<http://ex/e1>"]
    );
    assert!(touches(
        &rec.take(),
        (h.text_index_offset, h.text_index_len)
    ));
}
