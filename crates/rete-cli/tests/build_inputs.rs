//! Build inputs: gzip, Turtle/TriG streaming, and the external (memory-bounded)
//! build's acceptance of them.
//!
//! The public RDF dumps that actually need `--memory-budget-mb` do not ship as
//! plain `.nt` — they ship as `dump.ttl.gz` / `dump.trig.gz`. Expanding one to
//! N-Triples first costs an order of magnitude more disk than the source (146.8
//! bytes/triple measured on SemOpenAlex), which is exactly the resource the
//! external build exists to avoid spending. These tests pin the contract that
//! makes that unnecessary: every accepted syntax streams, compressed or not, and
//! the bytes it produces do not depend on which spelling of the same graph the
//! input used.

mod common;

use std::io::Write as _;

use common::{fixture, rete};

const TTL: &str = concat!(
    "@prefix ex: <http://example.test/> .\n",
    "ex:alice ex:knows ex:bob ;\n",
    "         ex:name \"Alice\"@en .\n",
    "ex:bob ex:name \"Bob\"@en .\n",
);

/// The same three statements as `TTL`, but inside a named graph.
const TRIG: &str = concat!(
    "@prefix ex: <http://example.test/> .\n",
    "ex:people {\n",
    "  ex:alice ex:knows ex:bob ;\n",
    "           ex:name \"Alice\"@en .\n",
    "  ex:bob ex:name \"Bob\"@en .\n",
    "}\n",
);

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(bytes).unwrap();
    enc.finish().unwrap()
}

fn assert_external_matches_resident(external: &[u8], resident: &[u8]) {
    let external_header = rete_core::Header::from_bytes(external).unwrap();
    let resident_header = rete_core::Header::from_bytes(resident).unwrap();
    assert_eq!(external_header.version, 0x05);
    assert_eq!(resident_header.version, 0x06);
    assert_eq!(external_header.quad_count, resident_header.quad_count);
    assert_eq!(external_header.term_count, resident_header.term_count);

    let external = rete_core::Rete::open(external).unwrap();
    let resident = rete_core::Rete::open(resident).unwrap();
    assert_eq!(external.dump(None), resident.dump(None));
    assert_eq!(external.graph_names(), resident.graph_names());
    for graph in external.graph_names() {
        assert_eq!(external.dump(Some(graph)), resident.dump(Some(graph)));
    }
}

/// `.ttl.gz` builds, and to the same bytes as the uncompressed `.ttl`.
/// Compression is a transport detail; it must not reach the output.
#[test]
fn gzipped_turtle_builds_identically() {
    let f = fixture();
    let plain = f.write("g.ttl", TTL);
    let gz = f.write("g.ttl.gz", gzip(TTL.as_bytes()));
    let out_plain = f.path("plain.rete");
    let out_gz = f.path("gz.rete");

    for (src, out) in [(&plain, &out_plain), (&gz, &out_gz)] {
        rete()
            .args(["build"])
            .arg(src)
            .arg("-o")
            .arg(out)
            .assert()
            .success();
    }
    assert_eq!(
        std::fs::read(&out_plain).unwrap(),
        std::fs::read(&out_gz).unwrap(),
        "gzip must not change the built file"
    );
}

/// A gzip file may be a CONCATENATION of members — what `cat a.gz b.gz` writes,
/// and how several public dumps are assembled. `GzDecoder` stops silently after
/// the first member and would drop the rest of the graph on the floor; the
/// decoder must be the multi-member one.
#[test]
fn concatenated_gzip_members_all_read() {
    let f = fixture();
    let first = "<http://example.test/a> <http://example.test/p> \"1\" .\n";
    let second = "<http://example.test/b> <http://example.test/p> \"2\" .\n";
    let mut both = gzip(first.as_bytes());
    both.extend_from_slice(&gzip(second.as_bytes()));
    let src = f.write("both.nt.gz", both);
    let out = f.path("both.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicates::str::contains("2 triples"));
}

/// The external build accepts Turtle — the point of the exercise. Its output is
/// remains query-equivalent to a standard `--no-pyramid` build while the
/// transitional external writer stays on 0x05 and the resident writer emits
/// paired generation 0x06.
#[test]
fn external_build_from_gzipped_turtle_matches_in_ram() {
    let f = fixture();
    let gz = f.write("g.ttl.gz", gzip(TTL.as_bytes()));
    let plain = f.write("g.ttl", TTL);
    let out_ext = f.path("ext.rete");
    let out_ram = f.path("ram.rete");

    rete()
        .args(["build"])
        .arg(&gz)
        .args(["--memory-budget-mb", "64", "-o"])
        .arg(&out_ext)
        .assert()
        .success();
    rete()
        .args(["build"])
        .arg(&plain)
        .args(["--no-pyramid", "-o"])
        .arg(&out_ram)
        .assert()
        .success();

    assert_external_matches_resident(
        &std::fs::read(&out_ext).unwrap(),
        &std::fs::read(&out_ram).unwrap(),
    );
}

#[test]
fn external_card_reports_the_legacy_generation_it_writes() {
    let f = fixture();
    let src = f.write("card.ttl", TTL);
    let out = f.path("card.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--memory-budget-mb", "64", "--card", "-o"])
        .arg(&out)
        .assert()
        .success();

    let bytes = std::fs::read(&out).unwrap();
    let header = rete_core::Header::from_bytes(&bytes).unwrap();
    let card = rete_core::card::load_card(&bytes)
        .unwrap()
        .expect("external card is embedded");
    assert_eq!(header.version, 0x05);
    assert_eq!(card.format_version, header.version);
}

#[test]
fn ordinary_cards_report_the_selected_physical_generation() {
    let f = fixture();
    let src = f.write("ordinary-card.ttl", TTL);
    for (permutations, expected) in [("3", 0x05), ("6", 0x06)] {
        let out = f.path(&format!("ordinary-card-{permutations}.rete"));
        rete()
            .args(["build"])
            .arg(&src)
            .args(["--permutations", permutations, "--card", "-o"])
            .arg(&out)
            .assert()
            .success();
        let bytes = std::fs::read(&out).unwrap();
        let header = rete_core::Header::from_bytes(&bytes).unwrap();
        let card = rete_core::card::load_card(&bytes)
            .unwrap()
            .expect("ordinary card is embedded");
        assert_eq!(header.version, expected);
        assert_eq!(card.format_version, expected);
    }
}

/// TriG parses, and its named graph survives into the file.
#[test]
fn trig_keeps_its_named_graph() {
    let f = fixture();
    let src = f.write("g.trig", TRIG);
    let out = f.path("g.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicates::str::contains("named graph"));
    rete()
        .args(["graphs"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicates::str::contains("http://example.test/people"));
}

/// `--collapse-graphs` folds those statements into the default graph — which is
/// what makes a named-graph dump answer `?s ?p ?o` at all. The result must equal
/// the same data written as plain Turtle.
#[test]
fn collapse_graphs_matches_the_default_graph_build() {
    let f = fixture();
    let trig = f.write("g.trig", TRIG);
    let ttl = f.write("g.ttl", TTL);
    let out_collapsed = f.path("collapsed.rete");
    let out_ttl = f.path("ttl.rete");

    rete()
        .args(["build"])
        .arg(&trig)
        .args(["--collapse-graphs", "-o"])
        .arg(&out_collapsed)
        .assert()
        .success();
    rete()
        .args(["build"])
        .arg(&ttl)
        .arg("-o")
        .arg(&out_ttl)
        .assert()
        .success();

    assert_eq!(
        std::fs::read(&out_collapsed).unwrap(),
        std::fs::read(&out_ttl).unwrap(),
        "a collapsed TriG must build to the same file as the equivalent Turtle"
    );
}

/// The external build carries named graphs through the same spill the default
/// graph uses. During the 0x05/0x06 transition its physical bytes differ from
/// the in-RAM build, but all default/named graph content must remain identical.
#[test]
fn external_build_keeps_named_graphs_identically() {
    let f = fixture();
    let src = f.write("g.trig", TRIG);
    let out_ext = f.path("ext.rete");
    let out_ram = f.path("ram.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--memory-budget-mb", "64", "-o"])
        .arg(&out_ext)
        .assert()
        .success()
        .stdout(predicates::str::contains("named graph"));
    rete()
        .args(["build"])
        .arg(&src)
        .args(["--no-pyramid", "-o"])
        .arg(&out_ram)
        .assert()
        .success();

    assert_external_matches_resident(
        &std::fs::read(&out_ext).unwrap(),
        &std::fs::read(&out_ram).unwrap(),
    );
    rete()
        .args(["graphs"])
        .arg(&out_ext)
        .assert()
        .success()
        .stdout(predicates::str::contains("http://example.test/people"));

    // …and --collapse-graphs still folds them into the default graph.
    let out_flat = f.path("flat.rete");
    rete()
        .args(["build"])
        .arg(&src)
        .args(["--collapse-graphs", "--memory-budget-mb", "64", "-o"])
        .arg(&out_flat)
        .assert()
        .success();
}

/// `rete merge` fed the external builder, so it refused named-graph shards up
/// front. With #139 it folds them: two shards carrying *different* named graphs
/// merge into one file that has both, and the union of their statements.
#[test]
fn merge_folds_named_graph_shards() {
    let f = fixture();
    let a = f.write(
        "a.nq",
        concat!(
            "<http://ex/s1> <http://ex/p> <http://ex/o1> <http://ex/gA> .\n",
            "<http://ex/d> <http://ex/p> <http://ex/o> .\n",
        ),
    );
    let b = f.write(
        "b.nq",
        concat!(
            "<http://ex/s2> <http://ex/p> <http://ex/o2> <http://ex/gB> .\n",
            "<http://ex/s1> <http://ex/p> <http://ex/o1> <http://ex/gA> .\n",
        ),
    );
    let (ra, rb, out) = (f.path("a.rete"), f.path("b.rete"), f.path("all.rete"));
    for (src, dst) in [(&a, &ra), (&b, &rb)] {
        rete()
            .args(["build"])
            .arg(src)
            .arg("-o")
            .arg(dst)
            .assert()
            .success();
    }
    rete()
        .args(["merge"])
        .arg(&ra)
        .arg(&rb)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();

    let graphs = rete().args(["graphs"]).arg(&out).assert().success();
    let listed = String::from_utf8(graphs.get_output().stdout.clone()).unwrap();
    assert!(listed.contains("http://ex/gA"), "gA missing: {listed}");
    assert!(listed.contains("http://ex/gB"), "gB missing: {listed}");
    // the duplicate quad in gA is written once: 2 named + 1 default
    rete()
        .args(["info"])
        .arg(&out)
        .assert()
        .success()
        .stdout(predicates::str::contains("quad_count: 3"));
}

/// `--text-index` stays refused on the memory-bounded path, with an error that
/// says why rather than a bare "not supported yet".
#[test]
fn external_build_rejects_text_index_with_a_reason() {
    let f = fixture();
    let src = f.write("g.nt", "<http://ex/s> <http://ex/p> \"hello world\" .\n");
    let out = f.path("ext.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--text-index", "--memory-budget-mb", "64", "-o"])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("separate external sort"));
}

/// RDF/XML is the one syntax the external build still refuses; the error has to
/// say so rather than fall over inside the parser.
#[test]
fn external_build_rejects_rdfxml_with_a_clear_error() {
    let f = fixture();
    let src = f.write("o.rdf", "<rdf:RDF/>");
    let out = f.path("ext.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--memory-budget-mb", "64", "-o"])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("rdfxml"));
}

/// Turtle with **anonymous blank nodes** — `[ … ]` and collections — must build.
///
/// This is the trap the streaming work has to stay clear of: oxttl labels an
/// anonymous blank node with a fresh random id on every parse, so any path that
/// parses the input more than once presents the encoder terms the observing pass
/// never saw. The two-pass assembler does exactly that, which is why `.ttl` files
/// keep taking the whole-text path; the external build reads once and is safe.
/// Both must produce the graph, and the same number of statements.
#[test]
fn turtle_with_anonymous_blank_nodes_builds_on_both_paths() {
    let f = fixture();
    let ttl = concat!(
        "@prefix ex: <http://ex/> .\n",
        "ex:a ex:p [ ex:q \"1\" ] .\n",
        "ex:b ex:list ( \"x\" \"y\" ) .\n",
    );
    let src = f.write("bn.ttl", ttl);
    let out_plain = f.path("bn.rete");
    let out_ext = f.path("bn-ext.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--no-pyramid", "-o"])
        .arg(&out_plain)
        .assert()
        .success()
        .stdout(predicates::str::contains("7 triples"));
    rete()
        .args(["build"])
        .arg(&src)
        .args(["--memory-budget-mb", "64", "-o"])
        .arg(&out_ext)
        .assert()
        .success()
        .stdout(predicates::str::contains("7 triples"));
}

/// `estimate` reads the same inputs the build does — otherwise the "will this
/// fit?" question cannot be asked about the dumps that need asking.
#[test]
fn estimate_reads_gzipped_turtle() {
    let f = fixture();
    let gz = f.write("g.ttl.gz", gzip(TTL.as_bytes()));

    rete()
        .args(["estimate"])
        .arg(&gz)
        .assert()
        .success()
        .stdout(predicates::str::contains("statements"));
}
