//! `rete convert` to format `0x08` is a re-encode (SPEC.md section 4.11): the
//! dictionary is sorted in the SPARQL order, every id changes, and every
//! structure that holds ids is written again.
//!
//! The inputs are made in-process in each older generation (`0x05`, `0x06`,
//! `0x07`: a byte-ordered dictionary), converted by the CLI, and compared with
//! what they were: the rows of every query (the exact order of every
//! `ORDER BY`), the content hash across sources, and the bytes of a second
//! conversion.

mod common;

use std::path::Path;

use common::rete;
use rete_core::ingest::{assemble_dataset_opts, AssembleOptions, GraphLayout, RawQuad};
use rete_core::term_order::{with_builder_order, TermOrder};
use rete_core::v06::write::{convert_image, WriteOpts};
use rete_core::v06::{VERSION_06, VERSION_07};

fn iri(s: &str) -> String {
    format!("<{s}>")
}

/// Labels, a type, text and typed literals, IRIs that prefix each other, a
/// blank node and two named graphs.
fn quads() -> Vec<RawQuad> {
    let label = iri("http://www.w3.org/2000/01/rdf-schema#label");
    let ty = iri("http://www.w3.org/1999/02/22-rdf-syntax-ns#type");
    let mut q: Vec<RawQuad> = Vec::new();
    for i in 0..300 {
        let s = iri(&format!("http://ex/e/{i}"));
        q.push((
            s.clone(),
            ty.clone(),
            iri(&format!("http://ex/T{}", i % 3)),
            None,
        ));
        q.push((
            s.clone(),
            label.clone(),
            format!("\"Entity number {i}\"@en"),
            None,
        ));
        q.push((
            s.clone(),
            iri("http://ex/note"),
            format!("\"the quick brown fox {i} jumps\""),
            None,
        ));
        q.push((
            s.clone(),
            iri("http://ex/n"),
            format!(
                "\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                (i * 37) % 101
            ),
            None,
        ));
        q.push((
            s.clone(),
            iri("http://ex/next"),
            iri(&format!("http://ex/e/{}", (i + 1) % 300)),
            None,
        ));
        if i % 5 == 0 {
            q.push((
                s,
                iri("http://ex/in"),
                iri(&format!("http://ex/e/{i}/provenance")),
                Some(iri(&format!("http://ex/g/{}", i % 4))),
            ));
        }
    }
    q.push((
        iri("http://ex/e/Ng8"),
        iri("http://ex/note"),
        "\"a\"".into(),
        None,
    ));
    q.push((
        iri("http://ex/e/Ng8130_provenance"),
        iri("http://ex/note"),
        "\"a b\"".into(),
        None,
    ));
    q.push(("_:b0".into(), iri("http://ex/note"), "\"a!\"".into(), None));
    q
}

/// The `0x05` image of `quads()` in the byte order, and its `0x06` / `0x07`
/// files.
fn inputs(dir: &Path) -> Vec<(&'static str, std::path::PathBuf)> {
    let mut o = AssembleOptions::sectioned();
    o.graph_layout = GraphLayout::Quad;
    o.with_pyramid = false;
    o.with_text_index = true;
    o.label_index = true;
    let image = with_builder_order(TermOrder::Bytewise, || {
        assemble_dataset_opts(quads(), &o, |_, _| Vec::new()).0
    });
    assert_eq!(image[4], 5);
    let mut out = Vec::new();
    let p5 = dir.join("in05.rete");
    std::fs::write(&p5, &image).unwrap();
    out.push(("0x05", p5));
    for (name, version) in [("0x06", VERSION_06), ("0x07", VERSION_07)] {
        let opts = WriteOpts {
            version,
            ..WriteOpts::default()
        };
        let bytes = convert_image(&image, opts).unwrap();
        assert_eq!(bytes[4], version);
        let p = dir.join(format!("in{name}.rete"));
        std::fs::write(&p, bytes).unwrap();
        out.push((name, p));
    }
    out
}

fn convert(input: &Path, output: &Path, extra: &[&str]) {
    let o = rete()
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .arg("convert")
        .arg(input)
        .arg("-o")
        .arg(output)
        .args(["--no-card-costs", "--memory-budget-mb", "256"])
        .args(extra)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn hash(path: &Path) -> String {
    let b = std::fs::read(path).unwrap();
    b[8..24].iter().map(|x| format!("{x:02x}")).collect()
}

/// `(ordered, query)`; an ordered query is compared row by row in order.
const QUERIES: &[(bool, &str)] = &[
    (true, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o"),
    (true, "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY DESC(?o) ?p ?s LIMIT 40 OFFSET 7"),
    (true, "SELECT ?s WHERE { ?s a <http://ex/T1> } ORDER BY ?s LIMIT 12"),
    (true, "SELECT ?s WHERE { ?s a <http://ex/T1> } ORDER BY DESC(?s)"),
    (true, "SELECT DISTINCT ?o WHERE { ?s <http://ex/note> ?o } ORDER BY ?o"),
    (true, "SELECT ?o WHERE { ?s <http://ex/n> ?o } ORDER BY ?o"),
    (true, "SELECT ?x WHERE { { ?x a <http://ex/T0> } UNION { ?s <http://ex/note> ?x } } ORDER BY ?x"),
    (true, "SELECT ?g ?s ?o WHERE { GRAPH ?g { ?s <http://ex/in> ?o } } ORDER BY ?g ?s ?o"),
    (false, "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"),
    (false, "SELECT ?l WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l FILTER(CONTAINS(?l, \"number 1\")) }"),
    (false, "SELECT ?s WHERE { ?s <http://ex/note> ?o FILTER(CONTAINS(?o, \"fox\")) }"),
    (false, "SELECT ?s WHERE { <http://ex/e/Ng8> <http://ex/note> ?s }"),
    (false, "SELECT ?s WHERE { <http://ex/e/Ng8130_provenance> <http://ex/note> ?s }"),
];

fn rows(file: &Path, query: &str, ordered: bool) -> Vec<String> {
    let out = rete()
        .arg("sparql")
        .arg(file)
        .arg(query)
        .arg("--json")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{query}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut r: Vec<String> = match v["results"]["bindings"].as_array() {
        Some(b) => b.iter().map(|x| x.to_string()).collect(),
        None => vec![v.to_string()],
    };
    if !ordered {
        r.sort();
    }
    r
}

#[test]
fn every_older_generation_converts_to_0x08_with_the_same_answers() {
    let dir = tempfile::tempdir().unwrap();
    let sources = inputs(dir.path());
    let mut hashes = Vec::new();
    for (name, src) in &sources {
        let out = dir.path().join(format!("out-{name}.rete"));
        convert(src, &out, &[]);
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(bytes[4], 8, "{name}");
        assert_eq!(&bytes[bytes.len() - 8..], b"\x08\x00\x00\x00RETE", "{name}");
        for (ordered, q) in QUERIES {
            let (before, after) = (rows(src, q, *ordered), rows(&out, q, *ordered));
            assert!(!before.is_empty(), "{name}: {q}");
            assert_eq!(before, after, "{name}: {q}");
        }
        // verify checks the dictionary order and says so
        let v = rete().arg("verify").arg(&out).output().unwrap();
        let text = String::from_utf8_lossy(&v.stdout).to_string();
        assert!(
            v.status.success() && text.contains("SPARQL order"),
            "{name}: {text}"
        );
        let info = rete().arg("info").arg(&out).output().unwrap();
        assert!(String::from_utf8_lossy(&info.stdout).contains("term order   : SPARQL"));
        hashes.push(hash(&out));
    }
    // the same data from any generation is the same file (the build record
    // says where it came from and is outside the hash)
    assert!(hashes.iter().all(|h| *h == hashes[0]), "{hashes:?}");
}

#[test]
fn converting_twice_gives_the_same_bytes_and_a_0x08_input_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let sources = inputs(dir.path());
    let src = &sources[1].1; // 0x06
    let (a, b, c) = (
        dir.path().join("a.rete"),
        dir.path().join("b.rete"),
        dir.path().join("c.rete"),
    );
    convert(src, &a, &[]);
    // a different budget and spill directory: the bytes do not depend on them
    let spill = dir.path().join("spill");
    std::fs::create_dir(&spill).unwrap();
    convert(src, &b, &["--tmp-dir", spill.to_str().unwrap()]);
    assert!(std::fs::read(&a).unwrap() == std::fs::read(&b).unwrap());
    // a 0x08 file converted again is written again under the leaf policy: same bytes
    convert(&a, &c, &[]);
    assert!(std::fs::read(&a).unwrap() == std::fs::read(&c).unwrap());
}

#[test]
fn an_older_engine_refuses_the_new_generation_by_name() {
    let dir = tempfile::tempdir().unwrap();
    let sources = inputs(dir.path());
    let out = dir.path().join("out.rete");
    convert(&sources[2].1, &out, &[]);
    // 0x09 is refused by this engine the way 0x08 is by 0.6.0: by name, with
    // the range it reads
    let mut b = std::fs::read(&out).unwrap();
    b[4] = 9;
    let future = dir.path().join("future.rete");
    std::fs::write(&future, b).unwrap();
    let o = rete().arg("info").arg(&future).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(
        !o.status.success()
            && err.contains("unsupported .rete format 0x09")
            && err.contains("0x05..=0x08"),
        "{err}"
    );
}
