//! `rete recard` — a new Dataset Card for a finished `.rete`, without a rebuild.
//!
//! The properties under test are the ones a published file depends on: every
//! section but the card and the build record comes through **byte for byte**,
//! the new content hash verifies, the publisher's curated fields survive
//! verbatim (or the command refuses), and the build record says where the file
//! came from.

mod common;

use predicates::prelude::*;

const CURATED: &str = r##"{
  "title": "Recard fixture",
  "license": "CC0-1.0",
  "description": ["# A fixture", "", "Prose the re-card must not touch — **stale figures included**: 12 statements."],
  "keywords": ["fixture", "recard"],
  "extra": {"examples": [{"query": "things", "words": ["thing", "number"]}]},
  "example_queries": ["SELECT ?s WHERE { ?s ?p ?o } LIMIT 3"]
}"##;

/// A carded file with a default graph, a named graph and a curated card.
fn carded() -> (common::Fixture, std::path::PathBuf) {
    let fixture = common::fixture();
    let mut nq = String::new();
    for i in 0..300 {
        nq.push_str(&format!(
            "<http://example.test/s{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.test/Thing> .\n\
             <http://example.test/s{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Thing \
             number {i}\"@en .\n\
             <http://example.test/s{i}> <http://example.test/next> <http://example.test/s{}> .\n\
             <http://example.test/s{i}> <http://example.test/seen> \"20{:02}-01-01\"^^\
             <http://www.w3.org/2001/XMLSchema#date> <http://example.test/g> .\n",
            (i + 1) % 300,
            i % 30
        ));
    }
    let source = fixture.write("graph.nq", nq);
    let card = fixture.write("card.json", CURATED);
    let out = fixture.path("graph.rete");
    common::build(&source, &out, &["--card-file", card.to_str().unwrap()]);
    (fixture, out)
}

/// `(kind, offset, length)` of every section, from the 1 KiB header.
fn sections(bytes: &[u8]) -> Vec<(u16, u64, u64)> {
    let n = u16::from_le_bytes([bytes[44], bytes[45]]) as usize;
    (0..n)
        .map(|i| {
            let at = 64 + i * 24;
            let kind = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            let off = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            let len = u64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap());
            (kind, off, len)
        })
        .collect()
}

/// The file's content hash, hex, straight from header bytes 8..24 (`rete card
/// --json` carries no checksum).
fn content_hash(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    bytes[8..24].iter().map(|b| format!("{b:02x}")).collect()
}

/// The data sections — everything but the card (1) and the build record (7) —
/// as `(kind, bytes)`, in directory order.
fn data_sections(bytes: &[u8]) -> Vec<(u16, Vec<u8>)> {
    sections(bytes)
        .into_iter()
        .filter(|(kind, _, len)| *kind != 1 && *kind != 7 && *len > 0)
        .map(|(kind, off, len)| (kind, bytes[off as usize..(off + len) as usize].to_vec()))
        .collect()
}

#[test]
fn a_recard_rewrites_only_the_card_and_keeps_the_prose() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--keep-curated", "--verify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("starter quer"));

    let before = std::fs::read(&src).unwrap();
    let after = std::fs::read(&out).unwrap();
    assert_eq!(
        data_sections(&before),
        data_sections(&after),
        "every section but the card and the build record is copied byte for byte"
    );
    common::rete().arg("verify").arg(&out).assert().success();

    let old = common::json(common::rete().arg("card").arg(&src).arg("--json"));
    let new = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    for field in [
        "title",
        "license",
        "description",
        "keywords",
        "extra",
        "example_queries",
    ] {
        assert_eq!(old[field], new[field], "{field} must travel verbatim");
    }
    assert_eq!(old["quad_count"], new["quad_count"]);
    assert_eq!(new["named_graph_count"], 1);
    assert!(!new["queries"].as_array().unwrap().is_empty());
    // The build record keeps the build's own provenance and says what
    // re-carded the file, from which content hash.
    let rec = &new["build"]["recarded"];
    assert_eq!(rec["from_content_hash"], content_hash(&src).as_str());
    assert_eq!(new["build"]["built_at"], old["build"]["built_at"]);
    assert!(new["build"]["query_costs"]["queries"].is_array());
    // And the data still answers, the same way.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let count = |f: &std::path::Path| {
        let out = common::rete().arg("sparql").arg(f).arg(q).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(count(&src), count(&out));
}

/// The card `rete recard` derives from the file is the card the build derived
/// from the input — on input with no duplicates and no multi-typed subjects —
/// so a re-card of a fresh build changes nothing but the build record.
#[test]
fn a_recard_of_a_fresh_build_derives_the_same_card() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("--keep-curated")
        .assert()
        .success();
    let strip = |mut v: serde_json::Value| {
        let o = v.as_object_mut().unwrap();
        o.remove("build");
        v
    };
    let old = strip(common::json(
        common::rete().arg("card").arg(&src).arg("--json"),
    ));
    let new = strip(common::json(
        common::rete().arg("card").arg(&out).arg("--json"),
    ));
    assert_eq!(old, new);
    // Same card bytes ⇒ same content hash: the card is the only hashed part
    // that could have changed.
    assert_eq!(content_hash(&src), content_hash(&out));
}

#[test]
fn a_recard_refuses_to_drop_curated_fields_or_overwrite() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("curated fields"));
    assert!(!out.exists(), "nothing is written when the run is refused");

    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&src)
        .arg("--keep-curated")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    // `--card-file` replaces the curated half wholesale, as `build` takes it.
    let card = fixture.write("new-card.json", r#"{"title":"Replaced"}"#);
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("--card-file")
        .arg(&card)
        .assert()
        .success();
    let new = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    assert_eq!(new["title"], "Replaced");
    assert!(new["description"].is_null());
}
