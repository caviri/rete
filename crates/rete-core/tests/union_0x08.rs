//! The streaming union default graph (a pattern outside `GRAPH` also sees every
//! named graph) over a format `0x08` quad-index file whose default graph has a
//! derived IRI family and the SPARQL-order dictionary.
//!
//! The union scan merges the default graph's index with SPOG / POSG / OSPG, so
//! the same term must carry the same id on both sides, derived or stored. Every
//! answer is compared with the same quads built with `--no-derive-iris`, and,
//! where it applies, with the quads stored in the default graph of a file that
//! has no named graphs (the oracle of the union itself).

use std::sync::{Arc, Mutex};

use rete_core::derived::DeriveOpts;
use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::query::{eval_query_with, set_query_threads, sharded_queries};
use rete_core::v06::write::{convert_image, WriteOpts};
use rete_core::v06::{open::read_front, F_DERIVED};
use rete_core::write_opts::{set_write_options, write_options};
use rete_core::{PermSet, PyramidAlgo, QueryOpts, QueryOutput, RangeReader, Rete, SliceReader};

const CI: &str = "https://w3id.org/oc/index/ci/";
const BR: &str = "https://w3id.org/oc/meta/br/";
const CITO: &str = "http://purl.org/spar/cito/";

const UNION: QueryOpts = QueryOpts {
    reason: false,
    union_default_graph: true,
};
const PLAIN: QueryOpts = QueryOpts {
    reason: false,
    union_default_graph: false,
};

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn omid(i: u64) -> String {
    let digits = 7 + (i % 5) as usize;
    format!(
        "06{:0width$}",
        mix(i) % 10u64.pow(digits as u32 - 2),
        width = digits - 2
    )
}

fn p(s: &str) -> String {
    format!("<{CITO}{s}>")
}

/// Citations in the default graph (the family the builder derives), copies of
/// some of their triples in named graphs (overlap: the union must count them
/// once), extra triples about the same subjects in named graphs only, and
/// citations that exist in named graphs only.
fn quads(n: u64) -> Vec<RawQuad> {
    let mut out: Vec<RawQuad> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string();
    for i in 0..n {
        let (a, b) = (omid(mix(2 * i) % 10_000), omid(mix(2 * i + 1) % 10_000));
        let ci = format!("<{CI}{a}-{b}>");
        if !seen.insert(ci.clone()) {
            continue;
        }
        let triples = [
            (ty.clone(), p("Citation")),
            (p("hasCitingEntity"), format!("<{BR}{a}>")),
            (p("hasCitedEntity"), format!("<{BR}{b}>")),
            (
                p("hasCitationCreationDate"),
                format!("\"20{:02}-0{}-1{}\"", 10 + i % 12, 1 + i % 9, i % 9),
            ),
        ];
        for (pp, o) in &triples {
            out.push((ci.clone(), pp.clone(), o.clone(), None));
        }
        let h = mix(i ^ 0xABCD);
        // Overlap: the same triples again in one or two named graphs.
        if h.is_multiple_of(3) {
            for (pp, o) in &triples {
                out.push((
                    ci.clone(),
                    pp.clone(),
                    o.clone(),
                    Some(format!("<http://ex/g/{:02}>", h % 16)),
                ));
                if h.is_multiple_of(9) {
                    out.push((
                        ci.clone(),
                        pp.clone(),
                        o.clone(),
                        Some(format!("<http://ex/g/{:02}>", (h >> 8) % 16)),
                    ));
                }
            }
        }
        // A fact only a named graph has about a default-graph subject.
        if h.is_multiple_of(7) {
            out.push((
                ci.clone(),
                p("note"),
                format!("\"n{}\"", h % 50),
                Some(format!("<http://ex/g/{:02}>", (h >> 4) % 16)),
            ));
        }
    }
    // Subjects with a derivable name that live in named graphs only.
    for i in 0..12u64 {
        let (a, b) = (omid(900_000 + i), omid(900_500 + i));
        let ci = format!("<{CI}{a}-{b}>");
        let g = Some(format!("<http://ex/g/{:02}>", i % 16));
        out.push((ci.clone(), ty.clone(), p("Citation"), g.clone()));
        out.push((
            ci.clone(),
            p("hasCitingEntity"),
            format!("<{BR}{a}>"),
            g.clone(),
        ));
        out.push((ci, p("hasCitedEntity"), format!("<{BR}{b}>"), g));
    }
    out
}

static BUILD: Mutex<()> = Mutex::new(());

fn image(quads: Vec<RawQuad>, layout: GraphLayout) -> Vec<u8> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let mut o = write_options();
    o.tile_budget = 2 * 1024;
    o.dict_chunk_budget = 4 * 1024;
    set_write_options(o);
    assemble_dataset_with_layout(
        quads,
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        layout,
        |_, _| Vec::<u8>::new(),
    )
    .0
}

fn loose() -> WriteOpts {
    WriteOpts::default().with_derive(Some(DeriveOpts {
        min_permille: 0,
        min_bytes: 1,
    }))
}

fn off() -> WriteOpts {
    WriteOpts::default().with_derive(None)
}

fn v5() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| image(quads(20_000), GraphLayout::Quad))
}

/// 0x08 quad index with the derived family.
fn derived() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| convert_image(v5(), loose()).unwrap())
}

/// The same quads, `--no-derive-iris`.
fn stored() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| convert_image(v5(), off()).unwrap())
}

/// The union's oracle: every triple in the default graph of a file with no
/// named graphs, queried without the union switch.
fn flattened() -> &'static [u8] {
    static B: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let flat: Vec<RawQuad> = quads(20_000)
            .into_iter()
            .map(|(s, p, o, _)| (s, p, o, None))
            .collect();
        convert_image(&image(flat, GraphLayout::PerGraph), off()).unwrap()
    })
}

struct Reader(Vec<u8>);

impl RangeReader for Reader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.0
            .get(offset as usize..(offset + len) as usize)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"))
    }
    fn concurrency(&self) -> usize {
        4
    }
}

fn lazy(data: &[u8]) -> Rete {
    Rete::open_ranged_lazy_with(Arc::new(Reader(data.to_vec())), 4096).unwrap()
}

fn resident(data: &[u8]) -> Rete {
    Rete::open(data).unwrap()
}

fn serial() -> std::sync::MutexGuard<'static, ()> {
    static S: Mutex<()> = Mutex::new(());
    S.lock().unwrap_or_else(|e| e.into_inner())
}

/// The rows in the engine's order (header first).
fn run(rete: &Rete, q: &str, opts: QueryOpts) -> Vec<String> {
    match eval_query_with(rete, q, opts).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(vars, rows) => std::iter::once(format!("{vars:?}"))
            .chain(rows.into_iter().map(|b| format!("{b:?}")))
            .collect(),
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

fn shown(v: &[String]) -> &[String] {
    &v[..v.len().min(3)]
}

fn pre() -> String {
    format!("PREFIX cito: <{CITO}> ")
}

#[test]
fn the_fixture_is_a_derived_0x08_quad_index() {
    let (h, _) = read_front(&SliceReader::new(derived())).unwrap();
    assert_ne!(h.flags & F_DERIVED, 0, "nothing was derived");
    let (h, _) = read_front(&SliceReader::new(stored())).unwrap();
    assert_eq!(h.flags & F_DERIVED, 0);
    assert!(derived().len() < stored().len());
    assert!(rete_core::format::verify(derived()).unwrap());
}

/// Unordered queries: the union rows are the same set on the derived file, the
/// stored file and the flattened oracle, and the union differs from the plain
/// default graph (so the test would notice a union that did nothing).
#[test]
fn union_deduplicates_the_same_term_on_both_sides() {
    let _s = serial();
    set_query_threads(1);
    let c = pre();
    let probe = omid(7);
    let qs = [
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }".to_string(),
        "SELECT (COUNT(DISTINCT ?s) AS ?n) WHERE { ?s ?p ?o }".to_string(),
        "SELECT (COUNT(DISTINCT ?o) AS ?n) WHERE { ?s ?p ?o }".to_string(),
        format!("{c}SELECT ?s ?o WHERE {{ ?s cito:hasCitedEntity ?o }}"),
        format!("{c}SELECT ?s WHERE {{ ?s a cito:Citation }}"),
        format!("{c}SELECT ?s ?v WHERE {{ ?s cito:note ?v }}"),
        // A join of a default-graph triple with a named-graph-only one.
        format!("{c}SELECT ?s ?a ?v WHERE {{ ?s cito:hasCitingEntity ?a ; cito:note ?v }}"),
        format!("{c}SELECT ?s ?p ?o WHERE {{ ?s ?p ?o }}"),
        format!("{c}SELECT ?s ?p WHERE {{ ?s ?p <{BR}{probe}> }}"),
        format!(
            "{c}SELECT ?s ?p WHERE {{ ?s ?p ?o FILTER(?o = <{BR}{}>) }}",
            omid(55)
        ),
        // Named-only subjects with a derivable name.
        format!(
            "SELECT ?p ?o WHERE {{ <{CI}{}-{}> ?p ?o }}",
            omid(900_000),
            omid(900_500)
        ),
        format!("ASK {{ <{CI}{}-{}> ?p ?o }}", omid(900_001), omid(900_501)),
        // A name that is neither stored nor derivable.
        format!("ASK {{ <{CI}{}-{}> ?p ?o }}", omid(1), omid(2)),
        "SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p".to_string(),
        format!(
            "{c}SELECT ?o (COUNT(DISTINCT ?s) AS ?n) WHERE {{ ?s cito:hasCitedEntity ?o }} GROUP BY ?o"
        ),
    ];
    let (d, d_lazy, st, fl) = (
        resident(derived()),
        lazy(derived()),
        resident(stored()),
        resident(flattened()),
    );
    let mut nonempty = 0;
    for q in &qs {
        let want = sorted(run(&fl, q, PLAIN));
        for (what, got) in [
            ("derived", run(&d, q, UNION)),
            ("derived, range reader", run(&d_lazy, q, UNION)),
            ("stored", run(&st, q, UNION)),
        ] {
            let got = sorted(got);
            assert!(
                got == want,
                "{what} union vs flattened: {q}\n got {:?}\nwant {:?}",
                shown(&got),
                shown(&want)
            );
        }
        nonempty += usize::from(want.len() > 1 || want == ["true"]);
    }
    assert!(nonempty >= 12, "the battery should find rows ({nonempty})");
    // The union is not the default graph: more triples.
    let n = |r: &Rete, o| run(r, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }", o);
    assert_ne!(n(&d, UNION), n(&d, PLAIN));
    assert_eq!(n(&d, PLAIN), n(&st, PLAIN));
    // The overlap is real: the union has fewer triples than the quads.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    assert_ne!(run(&d, q, PLAIN), n(&d, UNION));
}

/// Without the union switch the derived file answers as the stored one.
#[test]
fn plain_mode_is_unchanged_by_the_union_code_on_0x08() {
    let _s = serial();
    set_query_threads(1);
    let c = pre();
    let (d, st) = (resident(derived()), resident(stored()));
    for q in [
        format!("{c}SELECT ?s ?o WHERE {{ ?s cito:hasCitedEntity ?o }} ORDER BY ?s ?o LIMIT 200"),
        format!("{c}SELECT ?g (COUNT(*) AS ?n) WHERE {{ GRAPH ?g {{ ?s a cito:Citation }} }} GROUP BY ?g ORDER BY ?g"),
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }".to_string(),
    ] {
        assert_eq!(run(&d, &q, PLAIN), run(&st, &q, PLAIN), "{q}");
    }
}

/// ORDER BY sorts on ids (no decoding) under the union: the order is the
/// SPARQL order of the terms, the same on derived and stored files.
#[test]
fn order_by_under_union_sorts_derived_and_stored_ids_alike() {
    let _s = serial();
    set_query_threads(1);
    let c = pre();
    let qs = [
        format!("{c}SELECT ?s ?o WHERE {{ ?s cito:hasCitedEntity ?o }} ORDER BY ?s ?o LIMIT 300"),
        format!("{c}SELECT ?s ?o WHERE {{ ?s cito:hasCitedEntity ?o }} ORDER BY DESC(?s) ?o LIMIT 40 OFFSET 700"),
        format!("{c}SELECT DISTINCT ?s WHERE {{ ?s ?p ?o }} ORDER BY ?s LIMIT 50 OFFSET 2500"),
        format!("{c}SELECT DISTINCT ?s WHERE {{ ?s ?p ?o }} ORDER BY DESC(?s) LIMIT 50"),
        format!("{c}SELECT ?s ?v WHERE {{ ?s cito:note ?v }} ORDER BY ?v DESC(?s)"),
        format!("{c}SELECT ?o ?s WHERE {{ ?s cito:hasCitingEntity ?o }} ORDER BY ?o ?s LIMIT 100"),
        "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o LIMIT 400".to_string(),
        format!("{c}SELECT ?s WHERE {{ ?s a cito:Citation FILTER(STR(?s) > \"{CI}0650000\") }} ORDER BY ?s"),
    ];
    let (d, d_lazy, st, fl) = (
        resident(derived()),
        lazy(derived()),
        resident(stored()),
        resident(flattened()),
    );
    for q in &qs {
        let want = run(&fl, q, PLAIN);
        assert!(want.len() > 1, "no rows: {q}");
        for (what, got) in [
            ("derived", run(&d, q, UNION)),
            ("derived, range reader", run(&d_lazy, q, UNION)),
            ("stored", run(&st, q, UNION)),
        ] {
            assert!(
                got == want,
                "{what} vs flattened: {q}\n got {:?}\nwant {:?}",
                shown(&got),
                shown(&want)
            );
        }
    }
}

/// COUNT and GROUP BY under the union: one thread and four give the same rows
/// in the same order, resident and through a range reader, equal to the
/// stored file's and the flattened oracle's.
#[test]
fn grouped_queries_under_union_agree_across_threads() {
    let _s = serial();
    let c = pre();
    let qs = [
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }".to_string(),
        "SELECT ?p (COUNT(*) AS ?n) (COUNT(DISTINCT ?s) AS ?d) WHERE { ?s ?p ?o } GROUP BY ?p ORDER BY ?p".to_string(),
        format!("{c}SELECT ?s (COUNT(*) AS ?n) (COUNT(DISTINCT ?p) AS ?np) WHERE {{ ?s ?p ?o }} GROUP BY ?s ORDER BY ?s"),
        format!("{c}SELECT ?o (COUNT(*) AS ?n) (SAMPLE(?s) AS ?x) (MIN(STR(?s)) AS ?lo) WHERE {{ ?s cito:hasCitedEntity ?o }} GROUP BY ?o ORDER BY ?o"),
        format!("{c}SELECT ?l (COUNT(*) AS ?n) WHERE {{ ?s a cito:Citation BIND(STRLEN(STR(?s)) AS ?l) }} GROUP BY ?l ORDER BY ?l"),
        format!("{c}SELECT (COUNT(DISTINCT ?s) AS ?n) (MAX(STR(?s)) AS ?hi) WHERE {{ ?s a cito:Citation }}"),
        format!("{c}SELECT ?d (COUNT(*) AS ?n) WHERE {{ ?s cito:hasCitationCreationDate ?d }} GROUP BY ?d ORDER BY ?d"),
        format!("{c}SELECT ?s (GROUP_CONCAT(?v; SEPARATOR=\"|\") AS ?vs) WHERE {{ ?s cito:note ?v }} GROUP BY ?s ORDER BY ?s LIMIT 300"),
    ];
    let (d, d_lazy, st, st_lazy, fl) = (
        resident(derived()),
        lazy(derived()),
        resident(stored()),
        lazy(stored()),
        resident(flattened()),
    );
    let before = sharded_queries();
    for q in &qs {
        set_query_threads(1);
        let one = run(&d, q, UNION);
        let one_lazy = run(&d_lazy, q, UNION);
        set_query_threads(4);
        let four = run(&d, q, UNION);
        let four_lazy = run(&d_lazy, q, UNION);
        let four_stored = run(&st, q, UNION);
        let four_stored_lazy = run(&st_lazy, q, UNION);
        set_query_threads(1);
        let want = run(&fl, q, PLAIN);
        assert!(want.len() > 1, "no rows: {q}");
        assert_eq!(one, four, "1 vs 4 threads: {q}");
        assert_eq!(one_lazy, four_lazy, "1 vs 4 threads, range reader: {q}");
        assert_eq!(four, four_stored, "derived vs stored: {q}");
        assert_eq!(
            four_lazy, four_stored_lazy,
            "derived vs stored, range reader: {q}"
        );
        let ordered = q.contains("ORDER BY");
        let norm = |v: &[String]| {
            if ordered {
                v.to_vec()
            } else {
                sorted(v.to_vec())
            }
        };
        assert!(
            norm(&four) == norm(&want),
            "union vs flattened: {q}\n got {:?}\nwant {:?}",
            shown(&four),
            shown(&want)
        );
    }
    set_query_threads(0);
    if cfg!(feature = "parallel") {
        assert!(
            sharded_queries() > before,
            "no union query was cut into slices"
        );
    }
}

/// REGEX, REPLACE and STR over derived IRIs, in both default-graph modes. The
/// regex fast paths (`^lit`, `lit$`, `[^SET]*$`, `^.*[SET]`, a literal) and the
/// engine must give the stored file's answer.
#[test]
fn regex_fast_paths_over_derived_iris_match_the_stored_file() {
    let _s = serial();
    let c = pre();
    let sel =
        |f: &str| format!("{c}SELECT ?s WHERE {{ ?s a cito:Citation FILTER({f}) }} ORDER BY ?s");
    let proj = |e: &str| {
        format!(
            "{c}SELECT ?s ?x WHERE {{ ?s a cito:Citation BIND({e} AS ?x) }} ORDER BY ?s LIMIT 400"
        )
    };
    let qs = [
        sel(r#"REGEX(STR(?s), "^https://w3id.org/oc/index/ci/06")"#),
        sel(r#"REGEX(STR(?s), "-0600000")"#),
        sel(r#"REGEX(STR(?s), "9$")"#),
        sel(r#"REGEX(STR(?s), "^.*[-]06")"#),
        sel(r#"REGEX(STR(?s), "[^/]*$")"#),
        sel(r#"REGEX(STR(?s), "ci/0612")"#),
        // The engine, not a fast path: a flag, a class.
        sel(r#"REGEX(STR(?s), "CI/0612", "i")"#),
        sel(r#"REGEX(STR(?s), "-06[0-4]\\d+$")"#),
        // A derived IRI through STR.
        sel(&format!("STR(?s) = \"{CI}{}-{}\"", omid(3), omid(4))),
        proj(r#"REPLACE(STR(?s), "https://w3id.org/oc/index/ci/", "")"#),
        proj(r#"REPLACE(STR(?s), "^.*[/]", "")"#),
        proj(r#"REPLACE(STR(?s), "[^-]*$", "")"#),
        proj(r#"REPLACE(STR(?s), "0", "o")"#),
        proj(r#"REPLACE(STR(?s), "-(06)(\\d+)$", "-$2")"#),
        proj("STR(?s)"),
        proj("STRLEN(STR(?s))"),
        proj("STRAFTER(STR(?s), \"ci/\")"),
        // Through the cited entities (stored IRIs) too.
        format!("{c}SELECT ?s ?x WHERE {{ ?s cito:hasCitedEntity ?o BIND(REPLACE(STR(?o), \"[^/]*$\", \"\") AS ?x) FILTER(REGEX(STR(?o), \"9$\")) }} ORDER BY ?s ?o LIMIT 300"),
    ];
    let (d, d_lazy, st, fl) = (
        resident(derived()),
        lazy(derived()),
        resident(stored()),
        resident(flattened()),
    );
    for threads in [1, 4] {
        set_query_threads(threads);
        let mut nonempty = 0;
        for q in &qs {
            for (opts, name) in [(PLAIN, "plain"), (UNION, "union")] {
                let want = run(&st, q, opts);
                assert_eq!(run(&d, q, opts), want, "{name}, {threads} threads: {q}");
                assert_eq!(
                    run(&d_lazy, q, opts),
                    want,
                    "{name}, range reader, {threads} threads: {q}"
                );
                if opts.union_default_graph {
                    let oracle = run(&fl, q, PLAIN);
                    assert_eq!(want, oracle, "union vs flattened, {threads} threads: {q}");
                }
                nonempty += usize::from(want.len() > 1);
            }
        }
        assert!(nonempty >= 20, "the battery should find rows ({nonempty})");
    }
    set_query_threads(0);
}
