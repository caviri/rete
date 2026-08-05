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
/// documented to be byte-identical to a standard `--no-pyramid` build, and that
/// promise must survive the new input path.
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

    assert_eq!(
        std::fs::read(&out_ext).unwrap(),
        std::fs::read(&out_ram).unwrap(),
        "external build of .ttl.gz must equal the in-RAM --no-pyramid build"
    );
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
/// what makes a named-graph dump answer `?s ?p ?o` at all, and what makes it
/// eligible for the default-graph-only external build. The result must equal the
/// same data written as plain Turtle.
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

/// The external build writes default-graph files only. A TriG input with named
/// graphs must therefore fail with an error that names the fix, not with a
/// half-written file.
#[test]
fn external_build_rejects_named_graphs_and_names_the_fix() {
    let f = fixture();
    let src = f.write("g.trig", TRIG);
    let out = f.path("ext.rete");

    rete()
        .args(["build"])
        .arg(&src)
        .args(["--memory-budget-mb", "64", "-o"])
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicates::str::contains("named graph"));

    // …and with the fix applied it goes through.
    rete()
        .args(["build"])
        .arg(&src)
        .args(["--collapse-graphs", "--memory-budget-mb", "64", "-o"])
        .arg(&out)
        .assert()
        .success();
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
