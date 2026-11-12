//! Format 0x08: the dictionary is in the SPARQL order, so `ORDER BY` over ids
//! sorts without decoding (SPEC.md section 5.2).
//!
//! The same quads are written twice, with a byte-ordered dictionary (what
//! `0x07` holds, so `ORDER BY` decodes every key) and with the SPARQL order
//! (`0x08`), and both are read resident and over a recording range reader.
//! Every query must return the same rows in the same order from all of them,
//! and the `0x08` file must read a fraction of the dictionary for a bounded
//! `ORDER BY`.

use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_opts, AssembleOptions, GraphLayout, RawQuad};
use rete_core::term_order::{with_builder_order, TermOrder};
use rete_core::v06::write::{convert_image, WriteOpts};
use rete_core::v06::{kind, open::read_front, VERSION, VERSION_06, VERSION_07};
use rete_core::{eval_query, QueryOutput, RangeReader, Rete, SliceReader};

const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
/// Subjects beyond the hand-written ones: enough that the dictionary is many
/// chunks and a lazy reader reads only some.
const BULK: usize = 4000;

fn iri(s: &str) -> String {
    format!("<{s}>")
}

fn typed(v: &str, dt: &str) -> String {
    format!("\"{v}\"^^<{dt}>")
}

/// A graph that has every kind of term the order has to place: IRIs that are
/// prefixes of each other, non-ASCII ones, blank nodes, text literals (plain,
/// tagged in two cases, with escapes and with characters below the closing
/// quote) and typed ones (numbers, a date, a boolean, an unknown datatype).
fn quads(bulk: usize) -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    let t = iri("http://ex/T");
    let ty = iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let p1 = iri("http://ex/p1");
    let p2 = iri("http://ex/p2");
    let num = iri("http://ex/num");
    let link = iri("http://ex/link");
    let mut subjects: Vec<String> = vec![
        iri("http://ex/s/Ng8"),
        iri("http://ex/s/Ng8130_provenance"),
        iri("http://ex/s/Ng8-x"),
        iri("http://ex/s/Ng8"),
        iri("http://ex/s/\u{e9}"),
        iri("http://ex/s/z"),
        iri("http://ex/s/\u{1f600}"),
        "_:b1".into(),
        "_:b2".into(),
    ];
    // Names that do not compress (a hash each), as nanopublication IRIs do, so
    // the dictionary is many chunks.
    let mut h = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..bulk {
        h = h
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let name = format!("{:016x}{:016x}", h, h.rotate_left(29) ^ 0xdead_beef);
        subjects.push(iri(&format!("http://ex/s/{name}")));
        subjects.push(iri(&format!("http://ex/s/{name}/sub")));
    }
    let objects: Vec<String> = vec![
        "\"a\"".into(),
        "\"a b\"".into(),
        "\"a!\"".into(),
        "\"B\"".into(),
        "\"a\\\"q\"".into(),
        "\"tab\\there\"".into(),
        "\"x\"@en".into(),
        "\"x\"@EN".into(),
        "\"x\"@fr".into(),
        "\"\u{e9}\"@fr".into(),
        "\"x\"".into(),
        typed("5", &format!("{XSD}integer")),
        typed("10", &format!("{XSD}integer")),
        typed("2.5", &format!("{XSD}decimal")),
        typed("1e3", &format!("{XSD}double")),
        typed("2020-01-01", &format!("{XSD}date")),
        typed("true", &format!("{XSD}boolean")),
        typed("custom", "http://ex/dt"),
        iri("http://ex/o/1"),
        iri("http://ex/o/Ng8"),
        iri("http://ex/o/Ng8130"),
        "_:o1".into(),
    ];
    for (i, s) in subjects.iter().enumerate() {
        q.push((s.clone(), ty.clone(), t.clone(), None));
        for k in 0..3 {
            let o = &objects[(i * 5 + k * 7) % objects.len()];
            q.push((s.clone(), p1.clone(), o.clone(), None));
        }
        q.push((
            s.clone(),
            p2.clone(),
            objects[i % objects.len()].clone(),
            None,
        ));
        q.push((
            s.clone(),
            num.clone(),
            typed(&format!("{}", (i * 37) % 101), &format!("{XSD}integer")),
            None,
        ));
        q.push((
            s.clone(),
            link.clone(),
            subjects[(i * 3 + 1) % subjects.len()].clone(),
            None,
        ));
        if i % 4 == 0 {
            q.push((
                s.clone(),
                p1.clone(),
                objects[(i * 3) % objects.len()].clone(),
                Some(iri(&format!("http://ex/g/{}", i % 7))),
            ));
        }
    }
    q
}

/// The `0x05` image of the quads, in `order`.
fn image(order: TermOrder) -> Vec<u8> {
    image_of(order, BULK)
}

fn image_of(order: TermOrder, bulk: usize) -> Vec<u8> {
    let mut o = AssembleOptions::sectioned();
    o.graph_layout = GraphLayout::Quad;
    o.with_pyramid = false;
    with_builder_order(order, || {
        assemble_dataset_opts(quads(bulk), &o, |_, _| Vec::new()).0
    })
}

fn v06() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let o = WriteOpts {
            version: VERSION_06,
            ..WriteOpts::default()
        };
        convert_image(&image(TermOrder::Bytewise), o).unwrap()
    })
}

fn v07() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let o = WriteOpts {
            version: VERSION_07,
            ..WriteOpts::default()
        };
        convert_image(&image(TermOrder::Bytewise), o).unwrap()
    })
}

fn v08() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| convert_image(&image(TermOrder::Sparql), WriteOpts::default()).unwrap())
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => rows.into_iter().map(|b| format!("{b:?}")).collect(),
        QueryOutput::Ask(b) => vec![b.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

/// `(query, rows come in this exact order)`: every ORDER BY here has a key the
/// rows cannot tie on, or projects only the key, so the order is exact.
fn ordered() -> Vec<String> {
    let q = |s: &str| s.to_string();
    vec![
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY DESC(?s) ?p ?o"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?o ?s ?p"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY DESC(?o) ?p DESC(?s)"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?p ?s ?o LIMIT 17"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 9 OFFSET 13"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY DESC(?s) ?p ?o LIMIT 25"),
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?o ?s ?p LIMIT 11 OFFSET 2"),
        q("SELECT ?s WHERE { ?s a <http://ex/T> } ORDER BY ?s"),
        q("SELECT ?s WHERE { ?s a <http://ex/T> } ORDER BY DESC(?s)"),
        q("SELECT ?s WHERE { ?s a <http://ex/T> } ORDER BY ?s LIMIT 5"),
        q("SELECT ?s WHERE { ?s a <http://ex/T> } ORDER BY ?s LIMIT 5 OFFSET 20"),
        q("SELECT ?o WHERE { ?s <http://ex/p1> ?o } ORDER BY ?o"),
        q("SELECT ?o WHERE { ?s <http://ex/p1> ?o } ORDER BY DESC(?o) LIMIT 30"),
        q("SELECT DISTINCT ?o WHERE { ?s ?p ?o } ORDER BY ?o"),
        q("SELECT DISTINCT ?p WHERE { ?s ?p ?o } ORDER BY ?p"),
        q("SELECT DISTINCT ?p WHERE { ?s ?p ?o } ORDER BY DESC(?p)"),
        // numbers sort by value, which ids cannot: the engine decodes them
        q("SELECT ?o WHERE { ?s <http://ex/num> ?o } ORDER BY ?o"),
        q("SELECT ?s ?o WHERE { ?s <http://ex/num> ?o } ORDER BY DESC(?o) ?s LIMIT 12"),
        // a column of IRIs, blank nodes and literals of every kind
        q("SELECT ?x WHERE { { ?x a <http://ex/T> } UNION { ?s <http://ex/p1> ?x } } ORDER BY ?x"),
        q("SELECT DISTINCT ?x WHERE { { ?x a <http://ex/T> } UNION { ?s ?p ?x } } ORDER BY DESC(?x)"),
        q("SELECT DISTINCT ?x WHERE { { ?s <http://ex/link> ?x } UNION { ?x <http://ex/link> ?s } } ORDER BY ?x"),
        // unbound sorts first
        q("SELECT ?s ?z WHERE { ?s <http://ex/p2> ?o OPTIONAL { ?s <http://ex/none> ?z } } ORDER BY ?z ?s"),
        q("SELECT ?s ?o ?z WHERE { ?s <http://ex/p2> ?o OPTIONAL { ?o <http://ex/none> ?z } } ORDER BY DESC(?z) ?s ?o"),
        // an expression key is decoded, as before
        q("SELECT ?s ?o WHERE { ?s <http://ex/p2> ?o } ORDER BY STR(?o) ?s"),
        q("SELECT ?s ?o WHERE { ?s <http://ex/p2> ?o } ORDER BY LCASE(STR(?s)) ?o"),
        // groups
        q("SELECT ?s (COUNT(?o) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?s ORDER BY DESC(?n) ?s"),
        q("SELECT ?o (COUNT(?s) AS ?n) WHERE { ?s <http://ex/p1> ?o } GROUP BY ?o ORDER BY ?o"),
        // named graphs
        q("SELECT ?g ?s ?p ?o WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY ?g ?s ?p ?o"),
        q("SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s ?p ?o } } ORDER BY DESC(?g) ?o ?s LIMIT 10"),
        // a join through a variable
        q("SELECT ?s ?t ?o WHERE { ?s <http://ex/link> ?t . ?t <http://ex/p2> ?o } ORDER BY ?t ?s ?o"),
        // a variable bound in two roles (subject and object): shared ids
        q("SELECT ?x WHERE { ?x <http://ex/link> ?y . ?z <http://ex/link> ?x } ORDER BY ?x"),
    ]
}

fn unordered() -> Vec<String> {
    let q = |s: &str| s.to_string();
    vec![
        q("SELECT ?s ?p ?o WHERE { ?s ?p ?o }"),
        q("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"),
        q("SELECT ?s WHERE { ?s <http://ex/p1> \"x\"@en }"),
        q("SELECT ?s WHERE { ?s <http://ex/p1> <http://ex/o/Ng8> }"),
        q("SELECT ?s WHERE { <http://ex/s/Ng8> <http://ex/link> ?s }"),
        q("SELECT ?s WHERE { <http://ex/s/Ng8130_provenance> <http://ex/link> ?s }"),
        q("SELECT ?s WHERE { ?s <http://ex/p2> ?o FILTER(STRSTARTS(STR(?s), \"http://ex/s/N\")) }"),
        q("SELECT ?o WHERE { ?s <http://ex/p1> ?o FILTER(CONTAINS(STR(?o), \"a\")) }"),
        q("SELECT ?o WHERE { ?s <http://ex/p1> ?o FILTER(isLiteral(?o)) }"),
        q("SELECT ?s WHERE { ?s <http://ex/num> ?n FILTER(?n > 50) }"),
        q("ASK { <http://ex/s/z> a <http://ex/T> }"),
    ]
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// A reader that remembers every range it is asked for.
struct Recording {
    data: &'static [u8],
    reads: Mutex<Vec<(u64, u64)>>,
}

impl Recording {
    fn new(data: &'static [u8]) -> Arc<Self> {
        Arc::new(Recording {
            data,
            reads: Mutex::new(Vec::new()),
        })
    }
    fn take(&self) -> Vec<(u64, u64)> {
        std::mem::take(&mut *self.reads.lock().unwrap())
    }
}

impl RangeReader for Recording {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.lock().unwrap().push((offset, len));
        self.data
            .get(offset as usize..(offset + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
}

#[test]
fn the_files_are_the_generations_they_say() {
    assert_eq!((v06()[4], v07()[4], v08()[4]), (6, 7, 8));
    assert_eq!(v08()[v08().len() - 8], 8);
    for f in [v06(), v07(), v08()] {
        assert!(rete_core::file::verify(f).unwrap());
        let r = SliceReader::new(f);
        let (n, bad) = rete_core::v06::check_dictionary(&r).unwrap();
        assert!(bad.is_empty(), "{bad:?}");
        assert!(n > 100);
    }
    let (h, _) = read_front(&SliceReader::new(v08())).unwrap();
    assert_eq!(h.version, VERSION);
}

#[test]
fn every_query_returns_the_same_rows_from_every_generation() {
    let want = Rete::open(v07()).unwrap();
    let readers: Vec<(&str, Rete)> = vec![
        ("0x06", Rete::open(v06()).unwrap()),
        ("0x08", Rete::open(v08()).unwrap()),
        (
            "0x08 lazy",
            Rete::open_ranged_lazy_with(Recording::new(v08()), 4096).unwrap(),
        ),
        (
            "0x07 lazy",
            Rete::open_ranged_lazy_with(Recording::new(v07()), 4096).unwrap(),
        ),
    ];
    for q in ordered() {
        let w = rows(&want, &q);
        assert!(!w.is_empty(), "{q}: no rows");
        for (name, r) in &readers {
            assert_eq!(rows(r, &q), w, "{name}: {q}");
        }
    }
    for q in unordered() {
        let w = sorted(rows(&want, &q));
        for (name, r) in &readers {
            assert_eq!(sorted(rows(r, &q)), w, "{name}: {q}");
        }
    }
}

/// The 0x05 images: the SPARQL-ordered one carries a header flag, the byte-
/// ordered one is a plain `0x05` image, and both answer alike.
#[test]
fn a_0x05_image_in_either_order_answers_alike() {
    let (a, b) = (image(TermOrder::Bytewise), image(TermOrder::Sparql));
    assert_eq!((a[4], b[4]), (5, 5));
    let (ra, rb) = (Rete::open(&a).unwrap(), Rete::open(&b).unwrap());
    assert_eq!(ra.dictionary().order(), TermOrder::Bytewise);
    assert_eq!(rb.dictionary().order(), TermOrder::Sparql);
    for q in ordered() {
        assert_eq!(rows(&ra, &q), rows(&rb, &q), "{q}");
    }
}

#[test]
fn a_dictionary_in_the_wrong_order_for_the_generation_is_refused() {
    // a byte-ordered dictionary cannot be written as 0x08, nor a SPARQL-ordered one as 0x07
    assert!(convert_image(&image(TermOrder::Bytewise), WriteOpts::default()).is_err());
    let o = WriteOpts {
        version: VERSION_07,
        ..WriteOpts::default()
    };
    assert!(convert_image(&image(TermOrder::Sparql), o).is_err());
}

#[test]
fn a_bounded_order_by_decodes_only_what_it_shows() {
    // A dictionary of many chunks: leaked on purpose, the readers need 'static data.
    let big = |order: TermOrder, version: u8| -> &'static [u8] {
        let o = WriteOpts {
            version,
            ..WriteOpts::default()
        };
        Box::leak(
            convert_image(&image_of(order, 30_000), o)
                .unwrap()
                .into_boxed_slice(),
        )
    };
    let (f7, f8) = (
        big(TermOrder::Bytewise, VERSION_07),
        big(TermOrder::Sparql, VERSION),
    );
    let q = "SELECT ?s ?p ?o WHERE { ?s <http://ex/link> ?o . ?s ?p ?o2 } ORDER BY ?s LIMIT 5";
    let dict_bytes = |image: &'static [u8]| -> (u64, Vec<String>) {
        let (h, root) = read_front(&SliceReader::new(image)).unwrap();
        let d = root.section(kind::DICT_DATA).unwrap().abs(&h).unwrap();
        let rec = Recording::new(image);
        let rete = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
        rec.take();
        let got = rows(&rete, q);
        let all = rec.take();
        eprintln!(
            "file {} B, dict section {}+{}, {} reads after open: {:?}",
            image.len(),
            d.offset,
            d.len,
            all.len(),
            all.iter()
                .filter(|&&(o, l)| o < d.offset + d.len && d.offset < o + l)
                .collect::<Vec<_>>()
        );
        let n: u64 = all
            .iter()
            .map(|&(o, l)| {
                (o + l)
                    .min(d.offset + d.len)
                    .saturating_sub(o.max(d.offset))
            })
            .sum();
        (n, got)
    };
    let (old, rows7) = dict_bytes(f7);
    let (new, rows8) = dict_bytes(f8);
    assert_eq!(rows7, rows8);
    assert_eq!(rows8.len(), 5);
    assert!(
        new * 3 < old,
        "ORDER BY ?s LIMIT 5 read {new} dictionary bytes on 0x08, {old} on 0x07"
    );
}

#[test]
fn only_exact_ids_have_an_order_slot() {
    let rete = Rete::open(v08()).unwrap();
    let d = rete.dictionary();
    let mut iris = 0;
    let mut typed_seen = 0;
    for node in 0..d.node_count() {
        let term = d.node_term(node).unwrap();
        let slot = d.order_slot(node);
        if term.starts_with('<') || term.starts_with("_:") {
            assert!(slot.is_some(), "{term}");
            iris += 1;
        } else if term.starts_with('"') && term.contains("^^<") && !term.ends_with("#string>") {
            assert!(slot.is_none(), "{term}: a typed literal sorts by value");
            typed_seen += 1;
        }
    }
    assert!(iris > 100 && typed_seen > 5);
    // within a section the local ids order the terms as the engine orders them
    let sections = [
        d.section_groups(0),
        d.section_groups(1),
        d.section_groups(2),
    ];
    for g in sections.into_iter().flatten() {
        assert!(g.total() > 0);
    }
    // the shared section's groups add up
    let g = d.section_groups(0).unwrap();
    assert_eq!(g.total(), d.shared_count() as u64);
    // and the old order has no slots
    let old = Rete::open(v07()).unwrap();
    assert!(old.dictionary().order_slot(0).is_none());
}

#[test]
fn the_literal_ranges_follow_the_groups() {
    for f in [v07(), v08()] {
        let rete = Rete::open(f).unwrap();
        let d = rete.dictionary();
        let lits = d.literal_objects().expect("ranges");
        let mut n = 0;
        for node in 0..d.node_count() {
            let Some(oid) = d.node_as_object_id(node) else {
                continue;
            };
            let is_lit = d.object_term(oid).unwrap().starts_with('"');
            assert_eq!(lits.contains(oid), is_lit, "object id {oid}");
            n += is_lit as u32;
        }
        assert!(n > 10);
    }
}
