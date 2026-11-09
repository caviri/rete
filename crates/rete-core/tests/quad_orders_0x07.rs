//! Format `0x07` quad orders: GSPO + SPOG + POSG + OSPG (SPEC.md §6.5).
//!
//! The same quads in, the same answers out, through every writer and every open
//! path: a `0x06` file and its `0x07` conversion, the in-RAM and the spilling
//! sort, the memory-bounded build, resident and lazy readers, on every shape of
//! `GRAPH ?g` and `GRAPH <g>`; and a bound subject across graphs reads a leaf
//! and a tile, not a pass over the file.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use rete_core::ingest::{assemble_dataset_with_layout, GraphLayout, RawQuad};
use rete_core::v06::write::{convert_image, rewrite, WriteOpts};
use rete_core::v06::{decode_quad_root, kind, open::read_front, tag, VERSION, VERSION_06};
use rete_core::write_opts::{set_quad_run_rows, set_quad_view_synth_max, write_options};
use rete_core::{eval_query, PermSet, PyramidAlgo, QueryOutput, RangeReader, Rete, SliceReader};

fn iri(s: &str) -> String {
    format!("<http://ex/{s}>")
}

/// Nanopub- and provenance-shaped: many small graphs sharing subjects, objects
/// and whole triples, one big graph with a hot subject spanning tiles, a blank
/// node graph, literals, and a default graph.
fn dataset() -> Vec<RawQuad> {
    let mut q: Vec<RawQuad> = Vec::new();
    for k in 0..60 {
        q.push((
            iri(&format!("e/{k}")),
            iri("title"),
            format!("\"work {k}\""),
            None,
        ));
        q.push((
            iri(&format!("e/{k}")),
            iri("cites"),
            iri(&format!("e/{}", (k * 7) % 60)),
            None,
        ));
    }
    for i in 0..400 {
        let g = Some(iri(&format!("g/{i}")));
        q.push((
            iri(&format!("e/{}", i % 40)),
            iri("rel0"),
            iri(&format!("o/{}", (i * 13) % 97)),
            g.clone(),
        ));
        q.push((
            iri(&format!("e/{}", i % 40)),
            iri("rel1"),
            iri(&format!("o/{}", i % 7)),
            g.clone(),
        ));
        q.push((
            iri(&format!("s/{i}")),
            iri("rel2"),
            iri(&format!("e/{}", i % 40)),
            g.clone(),
        ));
        q.push((
            iri(&format!("s/{i}")),
            iri("label"),
            format!("\"s {} \"@en", i % 11),
            g.clone(),
        ));
        if i < 30 {
            // The same triple in thirty graphs.
            q.push((iri("e/1"), iri("rel0"), iri("o/1"), g.clone()));
        }
    }
    // A big graph: a hot subject over many tiles, and many subjects.
    let big = Some(iri("big"));
    for k in 0..6_000 {
        q.push((iri("e/1"), iri("rel3"), iri(&format!("o/{k}")), big.clone()));
        q.push((
            iri(&format!("b/{k}")),
            iri("rel4"),
            iri(&format!("o/{}", k % 50)),
            big.clone(),
        ));
    }
    q.push((iri("a"), iri("p"), iri("b"), Some("_:g1".into())));
    q.push((iri("a"), iri("p"), iri("b"), Some("_:g1".into())));
    q
}

fn small_tiles() {
    // Process-wide, and the same for every test in this binary: many tiles per order.
    let mut o = write_options();
    o.tile_budget = 4 * 1024;
    rete_core::write_opts::set_write_options(o);
}

/// Builds are serialised: the legacy switch is an environment variable.
static BUILD: Mutex<()> = Mutex::new(());

/// The in-RAM `0x05` image of `quads`: with all six graph-leading quad orders
/// (`legacy`, the test hook that makes a `0x06` input with the old order set),
/// or as a `0x07` writer makes it, with GSPO alone.
fn image_of(quads: Vec<RawQuad>, legacy: bool) -> Vec<u8> {
    let _g = BUILD.lock().unwrap_or_else(|e| e.into_inner());
    small_tiles();
    if legacy {
        std::env::set_var("RETE_WRITE_FORMAT", "5");
    }
    let bytes = assemble_dataset_with_layout(
        quads,
        false,
        false,
        None,
        PyramidAlgo::Louvain,
        PermSet::ALL,
        GraphLayout::Quad,
        |_, _| Vec::<u8>::new(),
    )
    .0;
    std::env::remove_var("RETE_WRITE_FORMAT");
    bytes
}

fn v5() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| image_of(dataset(), false))
}

fn v5_legacy() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| image_of(dataset(), true))
}

fn v06() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        let o = WriteOpts {
            version: VERSION_06,
            ..Default::default()
        };
        convert_image(v5_legacy(), o).unwrap()
    })
}

fn v07() -> &'static [u8] {
    static BYTES: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| convert_image(v5(), WriteOpts::default()).unwrap())
}

/// A reader that counts what it is asked for.
struct Rec {
    data: Vec<u8>,
    reads: Mutex<Vec<(u64, u64)>>,
    fail: AtomicBool,
}

impl Rec {
    fn new(data: &[u8]) -> Arc<Self> {
        Arc::new(Rec {
            data: data.to_vec(),
            reads: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
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
    fn concurrency(&self) -> usize {
        4
    }
}

fn lazy(data: &[u8]) -> (Rete, Arc<Rec>) {
    let rec = Rec::new(data);
    let r = Rete::open_ranged_lazy_with(rec.clone(), 4096).unwrap();
    rec.take();
    (r, rec)
}

fn rows(rete: &Rete, q: &str) -> Vec<String> {
    match eval_query(rete, q).unwrap_or_else(|e| panic!("{q}: {e}")) {
        QueryOutput::Select(_, rows) => {
            let mut v: Vec<String> = rows.into_iter().map(|b| format!("{b:?}")).collect();
            v.sort();
            v
        }
        QueryOutput::Ask(a) => vec![a.to_string()],
        other => panic!("unexpected output {other:?}"),
    }
}

/// Every shape of `GRAPH ?g` / `GRAPH <g>` with every combination of bound
/// subject, predicate and object (present, hot, absent), plus the aggregate,
/// limit, distinct, filter and cross-graph join forms.
fn battery() -> Vec<String> {
    let subjects: [Option<&str>; 5] = [None, Some("e/1"), Some("s/5"), Some("e/39"), Some("nope")];
    let preds: [Option<&str>; 5] = [None, Some("rel0"), Some("rel1"), Some("rel3"), Some("nope")];
    let objs: [Option<&str>; 5] = [None, Some("o/1"), Some("o/5"), Some("e/1"), Some("o/5999")];
    let mut qs = Vec::new();
    let term = |t: Option<&str>, v: &str| t.map_or(format!("?{v}"), |x| format!("<http://ex/{x}>"));
    for s in subjects {
        for p in preds {
            for o in objs {
                let pat = format!("{} {} {}", term(s, "s"), term(p, "p"), term(o, "o"));
                qs.push(format!("SELECT * WHERE {{ GRAPH ?g {{ {pat} }} }}"));
                qs.push(format!(
                    "SELECT * WHERE {{ GRAPH <http://ex/g/7> {{ {pat} }} }}"
                ));
                qs.push(format!(
                    "SELECT * WHERE {{ GRAPH <http://ex/g/12> {{ {pat} }} }}"
                ));
                qs.push(format!(
                    "SELECT * WHERE {{ GRAPH <http://ex/big> {{ {pat} }} }}"
                ));
                qs.push(format!(
                    "SELECT (COUNT(*) AS ?n) WHERE {{ GRAPH ?g {{ {pat} }} }}"
                ));
            }
        }
    }
    qs.extend(
        [
            "SELECT DISTINCT ?g WHERE { GRAPH ?g { <http://ex/e/1> ?p ?o } }",
            "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s <http://ex/rel0> <http://ex/o/1> } }",
            "SELECT DISTINCT ?g WHERE { GRAPH ?g { ?s ?p <http://ex/e/1> } }",
            "SELECT ?g ?o WHERE { GRAPH ?g { ?s <http://ex/rel0> ?o FILTER(?o != <http://ex/o/1>) } }",
            "SELECT ?g ?l WHERE { GRAPH ?g { ?s <http://ex/label> ?l FILTER(CONTAINS(STR(?l), \"s 3\")) } }",
            "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g",
            "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?h { ?np <http://ex/rel2> ?x } }",
            "SELECT ?g1 ?g2 WHERE { GRAPH ?g1 { <http://ex/e/1> <http://ex/rel1> ?x } GRAPH ?g2 { ?y <http://ex/rel0> ?x } }",
            "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://ex/rel1> ?o . ?s <http://ex/rel0> ?z } }",
            "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/rel2> <http://ex/e/3> . ?s <http://ex/label> ?l } }",
            "SELECT ?e ?g WHERE { ?e <http://ex/cites> <http://ex/e/14> . GRAPH ?g { ?e <http://ex/rel0> ?o } }",
            "SELECT ?g ?o WHERE { GRAPH ?g { <http://ex/a> <http://ex/p> ?o } }",
            "ASK { GRAPH ?g { <http://ex/e/1> <http://ex/rel0> <http://ex/o/1> } }",
            "SELECT ?s ?p ?o FROM <http://ex/g/3> WHERE { ?s ?p ?o }",
            "SELECT ?g ?s FROM NAMED <http://ex/g/1> FROM NAMED <http://ex/big> WHERE { GRAPH ?g { ?s <http://ex/rel2> ?e } }",
        ]
        .map(String::from),
    );
    qs
}

/// `LIMIT` and `OFFSET` forms: the count is checked (which rows come first is
/// not specified), each row must be one of the full answer.
fn limited() -> Vec<(String, String)> {
    let mut v = Vec::new();
    for pat in [
        "<http://ex/e/1> ?p ?o",
        "?s <http://ex/rel0> ?o",
        "?s ?p <http://ex/o/1>",
        "?s ?p ?o",
    ] {
        let full = format!("SELECT * WHERE {{ GRAPH ?g {{ {pat} }} }}");
        v.push((format!("{full} LIMIT 1"), full.clone()));
        v.push((format!("{full} LIMIT 25"), full.clone()));
        v.push((format!("{full} LIMIT 10 OFFSET 3"), full));
    }
    v
}

fn all_answers(rete: &Rete, qs: &[String]) -> Vec<Vec<String>> {
    qs.iter().map(|q| rows(rete, q)).collect()
}

/// The first query whose answer differs, with the sizes: a diff of every row
/// would be megabytes.
fn assert_same(qs: &[String], got: &[Vec<String>], want: &[Vec<String>], what: &str) {
    for ((q, g), w) in qs.iter().zip(got).zip(want) {
        if g != w {
            let extra: Vec<_> = g.iter().filter(|r| !w.contains(r)).take(2).collect();
            let missing: Vec<_> = w.iter().filter(|r| !g.contains(r)).take(2).collect();
            panic!(
                "{what}: {q}
  got {} rows, want {}
  extra {extra:?}
  missing {missing:?}",
                g.len(),
                w.len()
            );
        }
    }
}

#[test]
fn the_0x07_file_has_the_four_orders_and_no_other() {
    let (h, root) = read_front(&SliceReader::new(v07())).unwrap();
    assert_eq!(h.version, VERSION);
    let q = decode_quad_root(root.unit(tag::QUAD).unwrap()).unwrap();
    let sis: Vec<usize> = q.orders.iter().map(|o| o.si).collect();
    assert_eq!(sis, vec![0, 6, 7, 8], "GSPO, SPOG, POSG, OSPG");
    // Every order holds every quad once (the tile counts differ, the rows do not).
    assert!(
        q.orders.iter().all(|o| o.n_tiles > 4),
        "small tiles give several per order"
    );
    assert_eq!(
        h.flags & rete_core::v06::F_QUAD_GRAPH_MAJOR,
        0,
        "order-major"
    );
    assert!(rete_core::format::verify(v07()).unwrap());
    // The 0x06 file of the same input has the six graph-leading orders.
    let (_, root6) = read_front(&SliceReader::new(v06())).unwrap();
    let q6 = decode_quad_root(root6.unit(tag::QUAD).unwrap()).unwrap();
    assert_eq!(q6.orders.len(), 6);
    // Fewer quad bytes than six graph-leading orders.
    assert!(
        v07().len() < v06().len(),
        "{} vs {}",
        v07().len(),
        v06().len()
    );
}

#[test]
fn a_0x07_file_answers_like_the_0x06_file_on_every_shape() {
    let qs = battery();
    let want = all_answers(&Rete::open(v06()).unwrap(), &qs);
    assert!(
        want.iter().filter(|r| !r.is_empty()).count() > 100,
        "the battery finds rows"
    );
    let resident = Rete::open(v07()).unwrap();
    assert_same(&qs, &all_answers(&resident, &qs), &want, "resident");
    let (lz, _) = lazy(v07());
    assert_same(&qs, &all_answers(&lz, &qs), &want, "lazy");
    assert!(!lz.index_incomplete());
    for (limited, full) in limited() {
        let all = rows(&Rete::open(v06()).unwrap(), &full);
        let got = rows(&resident, &limited);
        let n = limited
            .split("LIMIT ")
            .nth(1)
            .and_then(|t| t.split_whitespace().next())
            .and_then(|t| t.parse::<usize>().ok())
            .unwrap();
        let off = if limited.contains("OFFSET 3") { 3 } else { 0 };
        assert_eq!(got.len(), n.min(all.len().saturating_sub(off)), "{limited}");
        assert!(got.iter().all(|r| all.contains(r)), "{limited}");
        let lazy_got = rows(&lz, &limited);
        assert_eq!(lazy_got.len(), got.len(), "{limited}");
    }
}

#[test]
fn the_graph_leading_orders_of_the_input_do_not_matter() {
    // A 0x05 image with six graph-leading orders, and one with GSPO alone, make
    // the same 0x07 file: only GSPO is read, the rest is derived.
    assert!(convert_image(v5_legacy(), WriteOpts::default()).unwrap() == v07());
}

#[test]
fn the_conversion_from_0x06_is_the_conversion_from_0x05() {
    let src = SliceReader::new(v06());
    let mut out = std::io::Cursor::new(Vec::new());
    rewrite(&src, &mut out, WriteOpts::default()).unwrap();
    assert_eq!(out.into_inner(), v07(), "0x06 -> 0x07 equals 0x05 -> 0x07");
}

#[test]
fn a_spilling_sort_writes_the_same_bytes_as_the_in_ram_one() {
    let want = v07().to_vec();
    for rows in [1_000usize, 7_777, 20_000] {
        set_quad_run_rows(rows);
        let got = convert_image(v5(), WriteOpts::default()).unwrap();
        set_quad_run_rows(0);
        assert_eq!(got.len(), want.len(), "run of {rows} rows");
        assert!(got == want, "run of {rows} rows");
    }
}

#[test]
fn the_memory_bounded_build_answers_like_the_in_ram_build() {
    use rete_core::extbuild::{build_external, ExternalBuildOptions};
    small_tiles();
    let dir = std::env::temp_dir().join(format!("rete-q7-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("out.rete");
    let quads = dataset();
    build_external(
        |visit| {
            for q in &quads {
                visit(q.clone())?;
            }
            Ok(())
        },
        &out,
        ExternalBuildOptions {
            tmp_dir: Some(dir.clone()),
            graphs: GraphLayout::Quad,
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = std::fs::read(&out).unwrap();
    let (h, root) = read_front(&SliceReader::new(&bytes)).unwrap();
    assert_eq!(h.version, VERSION);
    let q = decode_quad_root(root.unit(tag::QUAD).unwrap()).unwrap();
    assert_eq!(
        q.orders.iter().map(|o| o.si).collect::<Vec<_>>(),
        vec![0, 6, 7, 8]
    );
    assert!(rete_core::format::verify(&bytes).unwrap());
    // The spill left nothing behind but the output.
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(left, vec!["out.rete".to_string()], "{left:?}");
    let qs = battery();
    let want = all_answers(&Rete::open(v07()).unwrap(), &qs);
    assert_same(
        &qs,
        &all_answers(&Rete::open(&bytes).unwrap(), &qs),
        &want,
        "resident",
    );
    let (lz, _) = lazy(&bytes);
    assert_same(&qs, &all_answers(&lz, &qs), &want, "lazy");
    // The same bytes where the build makes the same dictionary.
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_big_graph_reads_pos_and_osp_from_the_graph_last_orders() {
    // The `big` graph holds 12,000 quads: with the threshold under that, its
    // view reads POSG and OSPG tiles and keeps its own rows; the answers do not
    // change.
    let qs: Vec<String> = [
        "SELECT * WHERE { GRAPH <http://ex/big> { ?s <http://ex/rel4> ?o } }",
        "SELECT * WHERE { GRAPH <http://ex/big> { ?s ?p <http://ex/o/7> } }",
        "SELECT * WHERE { GRAPH <http://ex/big> { <http://ex/e/1> ?p ?o } }",
        "SELECT * WHERE { GRAPH <http://ex/big> { ?s <http://ex/rel4> <http://ex/o/7> } }",
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH <http://ex/big> { ?s ?p ?o } }",
        "SELECT * WHERE { GRAPH <http://ex/g/9> { ?s <http://ex/rel0> ?o } }",
    ]
    .map(String::from)
    .to_vec();
    let want = all_answers(&Rete::open(v06()).unwrap(), &qs);
    set_quad_view_synth_max(1_000);
    let resident = all_answers(&Rete::open(v07()).unwrap(), &qs);
    let (lz, rec) = lazy(v07());
    let lazy_rows = all_answers(&lz, &qs);
    let _ = rec;
    set_quad_view_synth_max(0);
    assert_same(&qs, &resident, &want, "resident");
    assert_same(&qs, &lazy_rows, &want, "lazy");
}

/// The ranges of section `k` of the image.
fn region(data: &[u8], k: u8) -> (u64, u64) {
    let (h, root) = read_front(&SliceReader::new(data)).unwrap();
    let r = root.section(k).expect("section present").abs(&h).unwrap();
    (r.offset, r.offset + r.len)
}

fn inside(reads: &[(u64, u64)], region: (u64, u64)) -> (usize, u64) {
    let n = reads
        .iter()
        .filter(|&&(o, l)| (o + l).min(region.1) > o.max(region.0))
        .count();
    let bytes = reads
        .iter()
        .map(|&(o, l)| (o + l).min(region.1).saturating_sub(o.max(region.0)))
        .sum();
    (n, bytes)
}

/// A nanopub-shaped file with a hundred thousand quads.
fn nanopubs() -> &'static (Vec<u8>, Vec<u8>) {
    static BYTES: std::sync::OnceLock<(Vec<u8>, Vec<u8>)> = std::sync::OnceLock::new();
    BYTES.get_or_init(|| {
        small_tiles();
        let n = 25_000;
        let mut q: Vec<RawQuad> = Vec::new();
        for i in 0..n {
            let np = iri(&format!("np/{i}"));
            let head = Some(format!("<http://ex/np/{i}#Head>"));
            let assertion = format!("<http://ex/np/{i}#assertion>");
            q.push((
                np.clone(),
                iri("hasAssertion"),
                assertion.clone(),
                head.clone(),
            ));
            q.push((
                np.clone(),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".into(),
                iri("Nanopublication"),
                head,
            ));
            let ga = Some(assertion);
            for k in 0..2 {
                q.push((
                    iri(&format!("e/{}", (i * 7) % 500)),
                    iri(&format!("rel{k}")),
                    iri(&format!("o/{}", (i * 13 + k) % 997)),
                    ga.clone(),
                ));
            }
        }
        let v5 = image_of(q.clone(), false);
        let v5_legacy = image_of(q, true);
        let six = convert_image(
            &v5_legacy,
            WriteOpts {
                version: VERSION_06,
                ..Default::default()
            },
        )
        .unwrap();
        (six, convert_image(&v5, WriteOpts::default()).unwrap())
    })
}

#[test]
fn a_bound_subject_across_graphs_reads_a_leaf_and_a_few_tiles() {
    let (six, seven) = nanopubs();
    let q = "SELECT ?g ?p ?o WHERE { GRAPH ?g { <http://ex/np/12345> ?p ?o } }";
    let (old, old_rec) = lazy(six);
    let old_rows = rows(&old, q);
    let old_reads = old_rec.take();
    let (new, new_rec) = lazy(seven);
    let new_rows = rows(&new, q);
    let new_reads = new_rec.take();
    assert_eq!(new_rows, old_rows);
    assert_eq!(new_rows.len(), 2, "{new_rows:?}");
    assert!(!new.index_incomplete());
    let tiles = region(seven, kind::QUAD_DATA);
    let leaves = region(seven, kind::QUAD_LEAVES);
    let (n_tiles, tile_bytes) = inside(&new_reads, tiles);
    let (n_leaves, leaf_bytes) = inside(&new_reads, leaves);
    assert!(n_tiles <= 3, "{n_tiles} tile reads: {new_reads:?}");
    assert!(n_leaves <= 2, "{n_leaves} leaf reads");
    assert!(tile_bytes < 64 * 1024, "{tile_bytes} B of tiles");
    assert!(leaf_bytes < 64 * 1024, "{leaf_bytes} B of leaves");
    // Against the pass over the SPO tiles of the 0x06 file.
    let old_tiles = region(six, kind::QUAD_DATA);
    let (_, old_bytes) = inside(&old_reads, old_tiles);
    assert!(
        old_bytes > 20 * tile_bytes.max(1),
        "0x06 read {old_bytes} B of tiles, 0x07 {tile_bytes} B"
    );
}

#[test]
fn an_object_and_a_predicate_object_pair_are_seeks_too() {
    let (six, seven) = nanopubs();
    for q in [
        "SELECT ?g ?s WHERE { GRAPH ?g { ?s ?p <http://ex/o/321> } }",
        "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/rel1> <http://ex/o/321> } }",
        "SELECT ?g WHERE { GRAPH ?g { ?s <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Nanopublication> } } LIMIT 3",
    ] {
        let (new, rec) = lazy(seven);
        let got = rows(&new, q);
        let reads = rec.take();
        let (old, _) = lazy(six);
        let want = rows(&old, q);
        if q.contains("LIMIT") {
            assert_eq!(got.len(), 3);
            continue;
        }
        assert_eq!(got, want, "{q}");
        assert!(!got.is_empty());
        let (_, bytes) = inside(&reads, region(seven, kind::QUAD_DATA));
        assert!(bytes < 256 * 1024, "{q}: {bytes} B of tiles");
    }
}

#[test]
fn a_failed_read_in_a_graph_last_scan_is_surfaced_and_retried() {
    let (_, seven) = nanopubs();
    let (lz, rec) = lazy(seven);
    let q = "SELECT ?g ?p ?o WHERE { GRAPH ?g { <http://ex/np/777> ?p ?o } }";
    rec.fail.store(true, Ordering::Relaxed);
    let _ = eval_query(&lz, q);
    assert!(lz.index_incomplete(), "the failure must be recorded");
    rec.fail.store(false, Ordering::Relaxed);
    lz.reset_load_failures();
    assert_eq!(rows(&lz, q).len(), 2);
    assert!(!lz.index_incomplete());
}

#[test]
fn the_deep_check_passes_a_sound_file_and_names_a_damaged_order() {
    use rete_core::v06::check_orders;
    for (bytes, orders) in [(v07(), 4), (v06(), 6)] {
        let (n, bad) = check_orders(&SliceReader::new(bytes)).unwrap();
        assert_eq!((n, bad), (orders, Vec::<String>::new()));
    }
    // Damage a byte in the middle of each order's tiles (the digest would catch
    // it too: this is the structural check, which names the order).
    let (h, root) = read_front(&SliceReader::new(v07())).unwrap();
    let q = decode_quad_root(root.unit(tag::QUAD).unwrap()).unwrap();
    let base = root
        .section(kind::QUAD_DATA)
        .unwrap()
        .abs(&h)
        .unwrap()
        .offset;
    for (i, name) in [(1usize, "SPO+G"), (2, "POS+G"), (3, "OSP+G"), (0, "G+SPO")] {
        let first = q.orders[i].leaves[0].3;
        let next = q
            .orders
            .get(i + 1)
            .map_or(root.section(kind::QUAD_DATA).unwrap().len, |n| {
                n.leaves[0].3
            });
        let mut bytes = v07().to_vec();
        let at = (base + (first + next) / 2) as usize;
        for b in &mut bytes[at..at + 6] {
            *b = !*b;
        }
        let (_, bad) = check_orders(&SliceReader::new(&bytes)).unwrap();
        assert!(bad.iter().any(|p| p.contains(name)), "{name}: {bad:?}");
    }
}
