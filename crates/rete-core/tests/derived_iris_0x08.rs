//! Derived IRI families (format `0x08`, SPEC.md section 5.3): a file that drops
//! the bodies of the subject-only chunks its index spells out answers every
//! query the plain file does, row for row, resident or lazy, and says so in its
//! header.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::derived::{DeriveOpts, Family};
use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::write::{convert_image, WriteOpts};
use rete_core::v06::{open::read_front, tag, F_DERIVED};
use rete_core::write_opts::write_options;
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

const CITING: &str = "http://purl.org/spar/cito/hasCitingEntity";
const CITED: &str = "http://purl.org/spar/cito/hasCitedEntity";
const BR: &str = "https://w3id.org/oc/meta/br/";
const CI: &str = "https://w3id.org/oc/index/ci/";

fn spec() -> String {
    format!("{CI}{{{CITING}|{BR}}}-{{{CITED}|{BR}}}")
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn omid(i: u64) -> String {
    // Lengths vary (7 to 11 digits), as OMIDs do, and the digits are not a
    // counter: the stored dictionary is not trivially compressible.
    let digits = 7 + (i % 5) as usize;
    format!(
        "06{:0width$}",
        mix(i) % 10u64.pow(digits as u32 - 2),
        width = digits - 2
    )
}

/// Up to `n` citations among 20,000 works, and a few IRIs under the same namespace
/// that are no function of their triples.
fn dataset(n: u64) -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    let p = |s: &str| format!("<http://purl.org/spar/cito/{s}>");
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..n {
        let (a, b) = (omid(mix(2 * i) % 20_000), omid(mix(2 * i + 1) % 20_000));
        let ci = format!("<{CI}{a}-{b}>");
        if !seen.insert(ci.clone()) {
            continue;
        }
        q.push((
            ci.clone(),
            "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
            p("Citation"),
            None,
        ));
        q.push((ci.clone(), p("hasCitingEntity"), format!("<{BR}{a}>"), None));
        q.push((ci.clone(), p("hasCitedEntity"), format!("<{BR}{b}>"), None));
        q.push((
            ci,
            p("hasCitationCreationDate"),
            format!("\"20{:02}-0{}-1{}\"", 10 + i % 12, 1 + i % 9, i % 9),
            None,
        ));
    }
    // Exceptions: a name the inputs do not spell, and one with two cited works.
    let odd = format!("<{CI}not-derived>");
    q.push((
        odd.clone(),
        p("hasCitingEntity"),
        format!("<{BR}{}>", omid(1)),
        None,
    ));
    q.push((odd, p("hasCitedEntity"), format!("<{BR}{}>", omid(2)), None));
    let two = format!("<{CI}0000001-0000002>");
    q.push((
        two.clone(),
        p("hasCitingEntity"),
        format!("<{BR}0000001>"),
        None,
    ));
    q.push((
        two.clone(),
        p("hasCitedEntity"),
        format!("<{BR}0000002>"),
        None,
    ));
    q.push((two, p("hasCitedEntity"), format!("<{BR}0000003>"), None));
    q
}

static BUILD: Mutex<()> = Mutex::new(());

/// A `0x05` image of `quads` with small chunks and tiles.
fn image(quads: Vec<RawQuad>) -> Vec<u8> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let mut o = write_options();
    o.tile_budget = 2 * 1024;
    o.dict_chunk_budget = 4 * 1024;
    rete_core::write_opts::set_write_options(o);
    assemble_dataset_with_layout(
        quads,
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        GraphLayout::Quad,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn v5() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| image(dataset(60_000)))
}

/// Detection with whatever size a family removes: for the small shapes.
fn loose() -> WriteOpts {
    WriteOpts::default().with_derive(Some(DeriveOpts {
        min_permille: 0,
        min_bytes: 1,
    }))
}

fn off() -> WriteOpts {
    WriteOpts::default().with_derive(None)
}

fn plain() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), off()).unwrap())
}

/// No flag, no template: the defaults of `rete build` and `rete convert`.
fn derived() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), WriteOpts::default()).unwrap())
}

/// The families a file stores, if any.
fn families(data: &[u8]) -> Vec<Family> {
    let (_, root) = read_front(&SliceReader::new(data)).unwrap();
    match root.unit(tag::DERIVED) {
        Some(unit) => rete_core::derived::decode_families(unit, 1 << 20).unwrap(),
        None => Vec::new(),
    }
}

struct Rec {
    data: Vec<u8>,
    reads: AtomicU64,
}

impl RangeReader for Rec {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.data
            .get(offset as usize..(offset + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
    fn concurrency(&self) -> usize {
        4
    }
}

fn lazy(data: &[u8]) -> Rete {
    let rec = Arc::new(Rec {
        data: data.to_vec(),
        reads: AtomicU64::new(0),
    });
    Rete::open_ranged_lazy_with(rec, 4096).unwrap()
}

fn resident(data: &[u8]) -> Rete {
    Rete::open(data).unwrap()
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            if !q.contains("ORDER BY") {
                v.sort();
            }
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn battery() -> Vec<String> {
    let c = "PREFIX cito: <http://purl.org/spar/cito/>";
    let mut qs: Vec<String> = Vec::new();
    for i in [0u64, 7, 55, 123, 399, 1_500] {
        let w = omid(i);
        qs.push(format!(
            "{c} SELECT ?ci ?cited WHERE {{ ?ci cito:hasCitingEntity <{BR}{w}> ; cito:hasCitedEntity ?cited }}"
        ));
        qs.push(format!(
            "{c} SELECT ?ci ?citing WHERE {{ ?ci cito:hasCitedEntity <{BR}{w}> ; cito:hasCitingEntity ?citing }}"
        ));
        qs.push(format!(
            "{c} SELECT ?ci WHERE {{ ?ci cito:hasCitedEntity <{BR}{w}> }} ORDER BY ?ci"
        ));
        qs.push(format!(
            "{c} SELECT ?ci WHERE {{ ?ci cito:hasCitedEntity <{BR}{w}> }} ORDER BY DESC(?ci) LIMIT 5"
        ));
    }
    // One citation by name: present, absent, malformed.
    for (a, b) in [
        (omid(0), omid(5)),
        (omid(13), omid(34)),
        (omid(1), omid(2)),
        ("0000001".to_string(), "0000002".to_string()),
        (omid(1), omid(1)),
    ] {
        qs.push(format!("SELECT ?p ?o WHERE {{ <{CI}{a}-{b}> ?p ?o }}"));
        qs.push(format!(
            "{c} ASK {{ <{CI}{a}-{b}> cito:hasCitedEntity <{BR}{b}> }}"
        ));
    }
    for name in ["not-derived", "00-00", "x-", "-", "1-2-3", ""] {
        qs.push(format!("SELECT ?p ?o WHERE {{ <{CI}{name}> ?p ?o }}"));
    }
    qs.extend(
        [
            "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
            "SELECT ?s ?p ?o WHERE { ?s ?p ?o }",
            "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o",
            "SELECT ?s ?o WHERE { ?s ?p ?o } ORDER BY ?s ?o LIMIT 50",
            "SELECT ?s ?o WHERE { ?s ?p ?o } ORDER BY DESC(?s) LIMIT 50",
            "SELECT DISTINCT ?s WHERE { ?s ?p ?o } ORDER BY ?s",
            "SELECT ?s WHERE { ?s a <http://purl.org/spar/cito/Citation> FILTER(STRENDS(STR(?s), \"-0600000\")) }",
            "SELECT ?s WHERE { ?s a <http://purl.org/spar/cito/Citation> FILTER(CONTAINS(STR(?s), \"ci/0612\")) }",
            "SELECT ?s WHERE { ?s a <http://purl.org/spar/cito/Citation> FILTER(?s > <https://w3id.org/oc/index/ci/0640000>) } ORDER BY ?s",
            "SELECT (SAMPLE(?s) AS ?x) (COUNT(?s) AS ?n) WHERE { ?s a <http://purl.org/spar/cito/Citation> } GROUP BY (STRLEN(STR(?s)) AS ?l)",
            "SELECT ?s (STRLEN(STR(?s)) AS ?l) WHERE { ?s <http://purl.org/spar/cito/hasCitationCreationDate> ?d } ORDER BY ?d ?s LIMIT 40",
        ]
        .map(String::from),
    );
    qs
}

#[test]
fn it_is_found_with_no_flag_and_the_file_is_smaller() {
    let (h, _) = read_front(&SliceReader::new(derived())).unwrap();
    assert_ne!(h.flags & F_DERIVED, 0, "nothing was derived");
    let fams = families(derived());
    assert_eq!(fams.len(), 1);
    assert_eq!(fams[0].spec(), spec());
    assert!(fams[0].chunk_count() > 3, "{:?}", fams[0].runs);
    assert!(
        derived().len() < plain().len(),
        "{} vs {}",
        derived().len(),
        plain().len()
    );
    let (hp, rootp) = read_front(&SliceReader::new(plain())).unwrap();
    assert_eq!(hp.flags & F_DERIVED, 0);
    assert!(rootp.unit(tag::DERIVED).is_none());
}

#[test]
fn the_exceptions_stay_stored_and_verify_passes() {
    assert!(rete_core::format::verify(derived()).unwrap());
    let r = SliceReader::new(derived());
    let (n, bad) = rete_core::v06::check_dictionary(&r).unwrap();
    assert!(bad.is_empty(), "{bad:?}");
    let (np, badp) = rete_core::v06::check_dictionary(&SliceReader::new(plain())).unwrap();
    assert!(badp.is_empty());
    assert_eq!(n, np);
}

#[test]
fn every_answer_is_the_same_resident_and_lazy() {
    let qs = battery();
    let want_r: Vec<_> = qs.iter().map(|q| rows(&resident(plain()), q)).collect();
    let want_l: Vec<_> = qs.iter().map(|q| rows(&lazy(plain()), q)).collect();
    assert_eq!(want_r, want_l);
    let (dr, dl) = (resident(derived()), lazy(derived()));
    let mut nonempty = 0;
    for ((q, w), _) in qs.iter().zip(&want_r).zip(0..) {
        let (a, b) = (rows(&dr, q), rows(&dl, q));
        assert!(
            a == *w,
            "resident: {q}\n got {:?}\nwant {:?}",
            &a[..a.len().min(3)],
            &w[..w.len().min(3)]
        );
        assert!(
            b == *w,
            "lazy: {q}\n got {:?}\nwant {:?}",
            &b[..b.len().min(3)],
            &w[..w.len().min(3)]
        );
        nonempty += usize::from(!w.is_empty());
    }
    assert!(
        nonempty > qs.len() / 2,
        "the battery should mostly find rows"
    );
}

#[test]
fn a_dump_is_the_same() {
    let mut a = resident(plain()).dump(None);
    let mut b = resident(derived()).dump(None);
    a.sort();
    b.sort();
    assert_eq!(a, b);
    let mut c = lazy(derived()).dump(None);
    c.sort();
    assert_eq!(a, c);
}

#[test]
fn a_derived_file_rewrites_to_the_same_bytes() {
    let mut out = std::io::Cursor::new(Vec::new());
    rete_core::v06::write::rewrite(&SliceReader::new(derived()), &mut out, off()).unwrap();
    assert!(out.into_inner() == derived());
}

/// The rows of a result handle, `step` at a time: a page is the same slice of
/// the full result, whether the file holds the names or derives them.
fn pages_of(rete: &Rete, q: &str, step: usize) -> Vec<String> {
    let h = rete_core::eval_select_rows(rete, q, rete_core::QueryOpts::default())
        .unwrap_or_else(|e| panic!("{q}: {e}"));
    let mut out = Vec::new();
    while out.len() < h.len() {
        let page = h.page(rete, out.len(), step);
        assert!(!page.is_empty());
        out.extend(page.into_iter().map(|b| format!("{b:?}")));
    }
    out
}

#[test]
fn a_page_decodes_derived_names_as_the_full_result_does() {
    let c = "PREFIX cito: <http://purl.org/spar/cito/>";
    let qs = [
        "SELECT ?s WHERE { ?s a <http://purl.org/spar/cito/Citation> } ORDER BY ?s".to_string(),
        "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY DESC(?s) ?p".to_string(),
        format!("{c} SELECT ?ci ?cited WHERE {{ ?ci cito:hasCitingEntity <{BR}{}> ; cito:hasCitedEntity ?cited }}", omid(7)),
        "SELECT DISTINCT ?s WHERE { ?s ?p ?o } ORDER BY ?s".to_string(),
    ];
    for q in &qs {
        let want = rows(&resident(plain()), q);
        let full = |r: &Rete| match eval_query(r, q).unwrap() {
            QueryOutput::Select(_, rows) => rows
                .into_iter()
                .map(|b| format!("{b:?}"))
                .collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        for r in [resident(derived()), lazy(derived())] {
            let all = full(&r);
            if q.contains("ORDER BY") {
                assert_eq!(all, want, "{q}");
            }
            for step in [3_000usize, 50_000] {
                let got = pages_of(&r, q, step);
                assert_eq!(got, all, "{q} paged by {step}");
            }
            let h = rete_core::eval_select_rows(&r, q, rete_core::QueryOpts::default()).unwrap();
            let seven: Vec<String> = h.page(&r, 10, 7).iter().map(|b| format!("{b:?}")).collect();
            assert_eq!(
                seven,
                all.iter().skip(10).take(7).cloned().collect::<Vec<_>>()
            );
            assert!(h.page(&r, h.len() + 5, 3).is_empty());
        }
    }
}

// ---------------------------------------------------------------------------
// Detection on other shapes, and what it must not take.
// ---------------------------------------------------------------------------

const A: &str = "http://ex/hasA";
const B: &str = "http://ex/hasB";

fn q(s: &str, p: &str, o: &str) -> RawQuad {
    (s.to_string(), format!("<{p}>"), o.to_string(), None)
}

/// The statements, resident and lazy, in file order and sorted: the same.
fn same_rows(a: &[u8], b: &[u8]) {
    let order = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o";
    for (x, y) in [(resident(a), resident(b)), (lazy(a), lazy(b))] {
        let (mut d1, mut d2) = (x.dump(None), y.dump(None));
        d1.sort();
        d2.sort();
        assert_eq!(d1, d2);
        assert_eq!(rows(&x, order), rows(&y, order));
        assert_eq!(pages_of(&x, order, 37), pages_of(&y, order, 37));
    }
}

/// The record is outside the content hash: a file with and without detection
/// that found nothing holds the same data.
fn content_hash(data: &[u8]) -> [u8; 16] {
    read_front(&SliceReader::new(data)).unwrap().0.content_hash
}

fn chunks(data: &[u8]) -> usize {
    let (h, root) = read_front(&SliceReader::new(data)).unwrap();
    let dict = rete_core::v06::decode_dict_root(root.unit(tag::DICT).unwrap(), h.version).unwrap();
    dict[rete_core::derived::SECTION].n_chunks
}

fn decimal(i: u64, width: u32) -> String {
    format!("{:0w$}", mix(i) % 10u64.pow(width), w = width as usize)
}

#[test]
fn random_iris_and_a_shared_prefix_give_no_family() {
    let mut quads = Vec::new();
    for i in 0..6_000u64 {
        // The subject and its objects share a namespace and nothing else.
        let s = format!("<http://ex/obs/{:016x}>", mix(i));
        quads.push(q(
            &s,
            A,
            &format!("<http://ex/obs/{:016x}>", mix(i + (1 << 20))),
        ));
        quads.push(q(
            &s,
            B,
            &format!("<http://ex/other/{:016x}>", mix(i + (2 << 20))),
        ));
        quads.push(q(&s, "http://ex/label", &format!("\"n{i}\"")));
    }
    let v5 = image(quads);
    let auto = convert_image(&v5, loose()).unwrap();
    let none = convert_image(&v5, off()).unwrap();
    assert!(families(&auto).is_empty());
    let (h, _) = read_front(&SliceReader::new(&auto)).unwrap();
    assert_eq!(h.flags & F_DERIVED, 0);
    assert_eq!(content_hash(&auto), content_hash(&none));
    same_rows(&auto, &none);
}

#[test]
fn a_family_that_fits_all_but_a_few_stores_the_misfits() {
    let mut quads = Vec::new();
    for i in 0..6_000u64 {
        let (a, b) = (decimal(i, 10), decimal(i + 7, 9));
        let s = format!("<http://ex/ci/{a}-{b}>");
        quads.push(q(&s, A, &format!("<http://ex/w/{a}>")));
        quads.push(q(&s, B, &format!("<http://ex/w/{b}>")));
    }
    // Six names the objects do not spell; they sort after every digit.
    for k in 0..6 {
        let s = format!("<http://ex/ci/zz-odd-{k}>");
        quads.push(q(&s, A, "<http://ex/w/1111111111>"));
        quads.push(q(&s, B, "<http://ex/w/222222222>"));
    }
    let v5 = image(quads);
    let auto = convert_image(&v5, loose()).unwrap();
    let none = convert_image(&v5, off()).unwrap();
    let fams = families(&auto);
    assert_eq!(fams.len(), 1, "no family found");
    assert_eq!(
        fams[0].spec(),
        format!("http://ex/ci/{{{A}|http://ex/w/}}-{{{B}|http://ex/w/}}")
    );
    let (derived, all) = (fams[0].chunk_count(), chunks(&auto));
    assert!(derived >= 1 && derived < all, "{derived} of {all} chunks");
    same_rows(&auto, &none);
    // The misfits are still there, as text.
    for k in 0..6 {
        let sparql = format!("SELECT ?p ?o WHERE {{ <http://ex/ci/zz-odd-{k}> ?p ?o }}");
        let want = rows(&resident(&none), &sparql);
        assert_eq!(want.len(), 2);
        assert_eq!(rows(&resident(&auto), &sparql), want);
        assert_eq!(rows(&lazy(&auto), &sparql), want);
    }
    let r = SliceReader::new(&auto);
    assert!(rete_core::v06::check_dictionary(&r).unwrap().1.is_empty());
}

#[test]
fn a_predicate_with_two_objects_is_never_a_hole() {
    let mut quads = Vec::new();
    for i in 0..6_000u64 {
        let (a, b) = (decimal(i, 10), decimal(i + 7, 9));
        let s = format!("<http://ex/m/{a}-{b}>");
        quads.push(q(&s, A, &format!("<http://ex/w/{a}>")));
        // `B` has the spelled object and another one.
        quads.push(q(&s, B, &format!("<http://ex/w/{b}>")));
        quads.push(q(&s, B, &format!("<http://ex/v/{i}>")));
    }
    let v5 = image(quads);
    let auto = convert_image(&v5, loose()).unwrap();
    let none = convert_image(&v5, off()).unwrap();
    for f in families(&auto) {
        assert!(f.inputs.iter().all(|i| i.predicate != B), "{}", f.spec());
    }
    // The name needs `B`'s text, which no single object gives: nothing derives.
    assert!(families(&auto).is_empty());
    same_rows(&auto, &none);
}

#[test]
fn a_shape_that_is_not_oc_like_is_found_too() {
    const STATION: &str = "http://ex/hasStation";
    const DAY: &str = "http://ex/hasDay";
    let mut quads = Vec::new();
    for i in 0..9_000u64 {
        let station = format!("S{}", i % 450);
        let day = format!("2020-{:02}-{:02}", 1 + i / 450 % 12, 1 + i / 5_400);
        let s = format!("<http://ex/obs/{station}/{day}>");
        quads.push(q(&s, STATION, &format!("<http://ex/station/{station}>")));
        quads.push(q(&s, DAY, &format!("<http://ex/day/{day}>")));
        quads.push(q(&s, "http://ex/value", &format!("\"{}\"", i % 97)));
    }
    let v5 = image(quads);
    let auto = convert_image(&v5, loose()).unwrap();
    let none = convert_image(&v5, off()).unwrap();
    let fams = families(&auto);
    assert_eq!(fams.len(), 1, "no family found");
    assert_eq!(
        fams[0].spec(),
        format!("http://ex/obs/{{{STATION}|http://ex/station/}}/{{{DAY}|http://ex/day/}}")
    );
    assert!(fams[0].chunk_count() >= 1);
    assert!(auto.len() < none.len());
    same_rows(&auto, &none);
    let r = SliceReader::new(&auto);
    assert!(rete_core::v06::check_dictionary(&r).unwrap().1.is_empty());
}

#[test]
fn the_build_record_says_what_was_found() {
    let rec = br#"{"schema":1,"builder":"test"}"#;
    let on =
        rete_core::build_record::finish_in_ram_with(v5().to_vec(), Some(rec), WriteOpts::default())
            .unwrap();
    let bytes = rete_core::file::read_build_info(&on).unwrap().unwrap();
    let text = rete_core::build_record::derive_report_text(&bytes).expect("a report");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["taken"][0]["template"], spec());
    assert!(v["taken"][0]["bytes_removed"].as_u64().unwrap() > 0);
    assert!(v["sampled_subjects"].as_u64().unwrap() >= 16);
    assert!(v["rule"].as_str().unwrap().contains("5.0 %"));
    let all: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(all["builder"], "test");
    // Off: no report, no family.
    let o = rete_core::build_record::finish_in_ram_with(v5().to_vec(), Some(rec), off()).unwrap();
    let bytes = rete_core::file::read_build_info(&o).unwrap().unwrap();
    assert!(rete_core::build_record::derive_report_text(&bytes).is_none());
    assert!(families(&o).is_empty());
}

// ---------------------------------------------------------------------------
// Threaded and sliced evaluation (grouped folds, shard scans, GRAPH ?g) over
// derived names and the SPARQL-order dictionary.
// ---------------------------------------------------------------------------

/// The citations of [`dataset`] in the default graph (where the builder looks
/// for derived names), and a copy of each subject's triples in one of 64 named
/// graphs, so that `GRAPH ?g` has graphs to walk and meets the same names.
fn graphed(n: u64) -> Vec<RawQuad> {
    let base = dataset(n);
    let named: Vec<RawQuad> = base
        .iter()
        .map(|(s, p, o, _)| {
            let g = mix(s.bytes().fold(7u64, |h, b| mix(h ^ u64::from(b)))) % 64;
            (
                s.clone(),
                p.clone(),
                o.clone(),
                Some(format!("<http://ex/g/{g:02}>")),
            )
        })
        .collect();
    base.into_iter().chain(named).collect()
}

fn v5g() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| image(graphed(30_000)))
}

fn plain_g() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5g(), off()).unwrap())
}

fn derived_g() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5g(), loose()).unwrap())
}

/// The rows as the engine returns them, in its order.
fn raw_rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(vars, rows) => std::iter::once(format!("{vars:?}"))
            .chain(rows.into_iter().map(|b| format!("{b:?}")))
            .collect(),
        QueryOutput::Construct(mut t) => {
            t.sort();
            t.into_iter().map(|t| format!("{t:?}")).collect()
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

/// `(query, ordered)`: an ordered result is compared in the engine's order
/// everywhere; an unordered one is compared in the engine's order between 1
/// and several threads, and sorted against the file with no derived family.
fn threaded_battery() -> Vec<(String, bool)> {
    let c = "PREFIX cito: <http://purl.org/spar/cito/> ";
    let g = |s: &str| (format!("{c}{s}"), false);
    let o = |s: &str| (format!("{c}{s}"), true);
    vec![
        // GROUP BY on a derived name: every aggregate that merges, and the
        // ones that send their rows to one fold.
        o("SELECT ?ci (COUNT(*) AS ?n) (SAMPLE(?x) AS ?any) WHERE { GRAPH ?g { ?ci ?p ?x } } \
           GROUP BY ?ci ORDER BY ?ci"),
        o("SELECT ?ci (COUNT(DISTINCT ?p) AS ?n) (COUNT(DISTINCT ?x) AS ?m) \
           (GROUP_CONCAT(DISTINCT STR(?p); SEPARATOR=\"|\") AS ?ps) WHERE { GRAPH ?g { ?ci ?p ?x } } \
           GROUP BY ?ci ORDER BY DESC(?ci)"),
        o("SELECT ?w (COUNT(?ci) AS ?n) (COUNT(DISTINCT ?ci) AS ?d) (SAMPLE(?ci) AS ?one) \
           (GROUP_CONCAT(STR(?ci); SEPARATOR=\" \") AS ?all) \
           WHERE { GRAPH ?g { ?ci cito:hasCitedEntity ?w } } GROUP BY ?w ORDER BY ?w"),
        o("SELECT (COUNT(DISTINCT ?ci) AS ?n) (COUNT(*) AS ?rows) (MIN(STR(?ci)) AS ?lo) \
           (MAX(STR(?ci)) AS ?hi) WHERE { GRAPH ?g { ?ci a cito:Citation } }"),
        o("SELECT ?l (COUNT(*) AS ?n) (SAMPLE(?ci) AS ?any) WHERE { GRAPH ?g { ?ci a cito:Citation } \
           BIND(STRLEN(STR(?ci)) AS ?l) } GROUP BY ?l ORDER BY ?l"),
        // A GRAPH block with no constant to find graphs by, and one that is
        // not a plain pattern.
        o("SELECT ?g (COUNT(DISTINCT ?ci) AS ?n) (MIN(STR(?ci)) AS ?lo) WHERE { GRAPH ?g { ?ci ?p ?x } } \
           GROUP BY ?g ORDER BY ?g"),
        o("SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?ci cito:hasCitingEntity ?a \
           OPTIONAL { ?ci cito:hasCitationCreationDate ?d } FILTER(STRENDS(STR(?ci), \"9\")) } } \
           GROUP BY ?g ORDER BY ?g"),
        // Joins on a derived name (two scans of one subject) and on the
        // entity both sides name (a hash join).
        o("SELECT ?b (COUNT(*) AS ?n) (SAMPLE(?ci) AS ?one) WHERE { \
           GRAPH ?g { ?ci cito:hasCitingEntity ?a } GRAPH ?h { ?ci cito:hasCitedEntity ?b } } \
           GROUP BY ?b ORDER BY ?b"),
        o("SELECT ?w (COUNT(*) AS ?n) (MIN(STR(?out)) AS ?lo) (MAX(STR(?inn)) AS ?hi) WHERE { \
           GRAPH ?g { ?out cito:hasCitingEntity ?w } GRAPH ?h { ?inn cito:hasCitedEntity ?w } } \
           GROUP BY ?w ORDER BY ?w"),
        o("SELECT ?ci (COUNT(?d) AS ?n) WHERE { GRAPH ?g { ?ci a cito:Citation \
           OPTIONAL { ?ci cito:hasCitationCreationDate ?d } } } GROUP BY ?ci ORDER BY ?ci LIMIT 500"),
        // Not grouped: ORDER BY on ids, LIMIT and OFFSET.
        o("SELECT ?ci ?w WHERE { GRAPH ?g { ?ci cito:hasCitedEntity ?w } } ORDER BY ?ci LIMIT 40"),
        o("SELECT ?ci ?w WHERE { GRAPH ?g { ?ci cito:hasCitedEntity ?w } } \
           ORDER BY DESC(?ci) LIMIT 25 OFFSET 100"),
        o("SELECT ?ci ?g WHERE { GRAPH ?g { ?ci cito:hasCitingEntity ?a } } \
           ORDER BY ?g DESC(?ci) LIMIT 60"),
        o("SELECT ?ci ?a ?b WHERE { ?ci cito:hasCitingEntity ?a ; cito:hasCitedEntity ?b } \
           ORDER BY ?b ?ci LIMIT 50"),
        o("SELECT DISTINCT ?ci WHERE { GRAPH ?g { ?ci ?p ?x } } ORDER BY ?ci LIMIT 30 OFFSET 3000"),
        // No ORDER BY: only the set of rows is the file's.
        g("SELECT ?ci ?w WHERE { GRAPH ?g { ?ci cito:hasCitedEntity ?w } }"),
        g("SELECT ?a (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?ci cito:hasCitingEntity ?a } } GROUP BY ?a"),
        // CONSTRUCT, which decodes derived terms into triples.
        g(&format!(
            "CONSTRUCT {{ ?ci cito:cites ?w }} WHERE {{ GRAPH ?g {{ ?ci cito:hasCitedEntity ?w }} \
             FILTER(?w = <{BR}{}>) }}",
            omid(55)
        )),
        g("CONSTRUCT { ?ci cito:made ?d } WHERE { GRAPH ?g { ?ci cito:hasCitationCreationDate ?d } \
           FILTER(?d = \"2011-02-11\") }"),
    ]
}

/// Every answer over a file with a derived family and the SPARQL-order
/// dictionary is the same on one thread and on several, resident and read
/// through a range reader (which cuts the work into fewer, larger slices), and
/// the same as the answer over the file built with `--no-derive-iris`.
#[test]
fn threads_and_slices_decode_derived_names_and_sort_on_ids() {
    let (h, _) = read_front(&SliceReader::new(derived_g())).unwrap();
    assert_ne!(h.flags & F_DERIVED, 0, "the fixture has no derived family");
    assert_eq!(families(derived_g()).len(), 1);
    assert!(rete_core::format::verify(derived_g()).unwrap());
    let (plain_r, derived_r, derived_l, plain_l) = (
        resident(plain_g()),
        resident(derived_g()),
        lazy(derived_g()),
        lazy(plain_g()),
    );
    let before = rete_core::query::sharded_queries();
    let mut nonempty = 0;
    for (q, ordered) in threaded_battery() {
        rete_core::query::set_query_threads(1);
        let want = raw_rows(&plain_r, &q);
        let one = raw_rows(&derived_r, &q);
        let one_lazy = raw_rows(&derived_l, &q);
        rete_core::query::set_query_threads(4);
        let four = raw_rows(&derived_r, &q);
        let four_lazy = raw_rows(&derived_l, &q);
        let four_plain_lazy = raw_rows(&plain_l, &q);
        let same = |a: &[String], b: &[String], what: &str| {
            let (mut a, mut b) = (a.to_vec(), b.to_vec());
            if !ordered {
                a.sort();
                b.sort();
            }
            assert!(
                a == b,
                "{what}: {q}\n got {:?}\nwant {:?}",
                &a[..a.len().min(3)],
                &b[..b.len().min(3)]
            );
        };
        // One thread against several: the same rows in the same order.
        assert_eq!(one, four, "1 vs 4 threads: {q}");
        assert_eq!(one_lazy, four_lazy, "1 vs 4 threads, range reader: {q}");
        assert_eq!(
            four_lazy, four_plain_lazy,
            "derived vs plain, range reader: {q}"
        );
        // Against the file with the names stored.
        same(&one, &want, "derived resident vs plain");
        same(&four, &want, "derived resident (4 threads) vs plain");
        same(&four_lazy, &want, "derived range reader vs plain");
        nonempty += usize::from(want.len() > 1);
    }
    assert!(nonempty >= 15, "the battery should find rows ({nonempty})");
    if cfg!(feature = "parallel") {
        assert!(
            rete_core::query::sharded_queries() > before,
            "no query was cut into slices"
        );
    }
    rete_core::query::set_query_threads(0);
}

/// A result handle (`SelectRows`) over the same files: a page is the slice of
/// the full result, under several threads too.
#[test]
fn a_result_handle_pages_derived_names_under_threads() {
    let q = "PREFIX cito: <http://purl.org/spar/cito/> SELECT ?ci ?w WHERE { GRAPH ?g { \
             ?ci cito:hasCitedEntity ?w } } ORDER BY DESC(?ci) LIMIT 2000";
    rete_core::query::set_query_threads(4);
    let want = raw_rows(&resident(plain_g()), q);
    for r in [resident(derived_g()), lazy(derived_g())] {
        let got: Vec<String> = pages_of(&r, q, 300);
        assert_eq!(&want[1..], &got[..], "paged");
    }
    rete_core::query::set_query_threads(0);
}
