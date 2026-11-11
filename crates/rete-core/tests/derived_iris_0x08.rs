//! Derived IRI families (format `0x08`, SPEC.md section 5.3): a file that drops
//! the bodies of the subject-only chunks its index spells out answers every
//! query the plain file does, row for row, resident or lazy, and says so in its
//! header.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::derived::Family;
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

fn omid(i: u64) -> String {
    // Lengths vary (7 to 11 digits), as OMIDs do.
    let digits = 7 + (i % 5) as usize;
    format!("06{:0width$}", i * 7919 % 99_991, width = digits - 2)
}

/// 24,000 citations among 2,000 works, and a few IRIs under the same namespace
/// that are no function of their triples.
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    let p = |s: &str| format!("<http://purl.org/spar/cito/{s}>");
    let mut seen = std::collections::BTreeSet::new();
    for i in 0..24_000u64 {
        let (a, b) = (omid(i * 13 % 2_000), omid((i * 29 + 5) % 2_000));
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

fn v5() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
        let mut o = write_options();
        o.tile_budget = 2 * 1024;
        o.dict_chunk_budget = 4 * 1024;
        rete_core::write_opts::set_write_options(o);
        assemble_dataset_with_layout(
            dataset(),
            false,
            false,
            None,
            PyramidAlgo::Louvain,
            PermSet::ALL,
            GraphLayout::Quad,
            |_, _| Vec::<u8>::new(),
        )
        .0
    })
}

fn derive(spec: &str) -> WriteOpts {
    WriteOpts {
        derive: vec![Family::parse_spec(spec).unwrap()],
        ..WriteOpts::default()
    }
}

fn plain() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        convert_image(
            v5(),
            WriteOpts {
                derive: Vec::new(),
                ..WriteOpts::default()
            },
        )
        .unwrap()
    })
}

fn derived() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), derive(&spec())).unwrap())
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
fn the_header_says_so_and_the_file_is_smaller() {
    let (h, root) = read_front(&SliceReader::new(derived())).unwrap();
    assert_ne!(h.flags & F_DERIVED, 0);
    let unit = root.unit(tag::DERIVED).unwrap();
    let fams = rete_core::derived::decode_families(unit, 1 << 20).unwrap();
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
fn a_family_the_file_does_not_spell_derives_nothing() {
    // Inputs swapped: no term is the function of its triples any more.
    let swapped = format!("{CI}{{{CITED}|{BR}}}-{{{CITING}|{BR}}}");
    let f = convert_image(v5(), derive(&swapped)).unwrap();
    let (h, _) = read_front(&SliceReader::new(&f)).unwrap();
    assert_eq!(h.flags & F_DERIVED, 0);
    assert!(f == plain());
    // A predicate the file lacks.
    let none = format!("{CI}{{http://nope/p|{BR}}}-{{{CITED}|{BR}}}");
    assert!(convert_image(v5(), derive(&none)).unwrap() == plain());
}

#[test]
fn a_derived_file_rewrites_to_the_same_bytes() {
    let mut out = std::io::Cursor::new(Vec::new());
    rete_core::v06::write::rewrite(
        &SliceReader::new(derived()),
        &mut out,
        WriteOpts {
            derive: Vec::new(),
            ..WriteOpts::default()
        },
    )
    .unwrap();
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
            for step in [1usize, 25, 1000] {
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
