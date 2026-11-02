//! `rete convert` says so in the build record: the source's own record is kept
//! as it was, and a `conversions` entry names the engine, the command and the
//! file it started from.

mod common;

use common::rete;
use serde_json::Value;
use std::path::Path;

fn nt() -> String {
    (0..30)
        .map(|i| {
            format!(
                "<http://example.test/e{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"E {i}\" .\n\
                 <http://example.test/e{i}> <http://example.test/next> <http://example.test/e{}> .\n",
                (i + 1) % 30
            )
        })
        .collect()
}

/// A `0x05` file (the test hook), reproducible record, with or without a card.
fn legacy(dir: &Path, card: bool) -> std::path::PathBuf {
    let src = dir.join("in.nt");
    std::fs::write(&src, nt()).unwrap();
    let out = dir.join(if card { "card.rete" } else { "bare.rete" });
    let mut c = rete();
    c.env("RETE_WRITE_FORMAT", "5")
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("--no-pyramid");
    if card {
        c.args(["--card", "--no-card-costs"]);
    }
    assert!(c.output().unwrap().status.success());
    out
}

fn convert(old: &Path, out: &Path) {
    let o = rete()
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000100")
        .arg("convert")
        .arg(old)
        .arg("-o")
        .arg(out)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn card_json(file: &Path) -> Value {
    common::json(rete().arg("card").arg("--json").arg(file))
}

fn hash_of(file: &Path) -> String {
    let b = std::fs::read(file).unwrap();
    b[8..24].iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn a_converted_file_keeps_the_original_record_and_adds_the_conversion() {
    let dir = tempfile::tempdir().unwrap();
    let old = legacy(dir.path(), true);
    let a = dir.path().join("a.rete");
    let b = dir.path().join("b.rete");
    convert(&old, &a);
    convert(&old, &b);
    assert_eq!(std::fs::read(&a).unwrap()[4], 6);

    let before = card_json(&old)["build"].clone();
    let after = card_json(&a)["build"].clone();
    // The original record, intact.
    for k in ["engine", "inputs", "counts", "params", "built_at"] {
        assert_eq!(before[k], after[k], "{k}");
        assert!(!before[k].is_null(), "{k} is in the source record");
    }
    // One conversion, naming the file it started from.
    let c = after["conversions"].as_array().unwrap();
    assert_eq!(c.len(), 1, "{after}");
    assert_eq!(c[0]["source_format"], "0x05");
    assert_eq!(c[0]["source_content_hash"], hash_of(&old).as_str());
    assert_eq!(
        c[0]["source_bytes"].as_u64().unwrap(),
        std::fs::metadata(&old).unwrap().len()
    );
    assert_eq!(c[0]["command"], "rete convert");
    assert_eq!(c[0]["engine"]["name"], "rete-cli");
    assert_eq!(c[0]["at"], "2023-11-14T22:15:00Z");
    assert!(before.get("conversions").is_none());

    // The text card shows it, and the file verifies (the record is outside the hash).
    let text = rete().arg("card").arg(&a).output().unwrap();
    let text = String::from_utf8_lossy(&text.stdout);
    assert!(
        text.contains("converted") && text.contains(&hash_of(&old)),
        "{text}"
    );
    rete().arg("verify").arg(&a).assert().success();

    // Two reproducible converts are byte-identical.
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    // A re-card keeps them.
    let r = dir.path().join("r.rete");
    let o = rete()
        .env("RETE_BUILD_RECORD", "reproducible")
        .args(["recard"])
        .arg(&a)
        .args(["--keep-curated", "-o"])
        .arg(&r)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        card_json(&r)["build"]["conversions"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_file_without_a_record_gets_one_that_holds_the_conversion() {
    let dir = tempfile::tempdir().unwrap();
    // Every 0.5 writer leaves a record, so take it off, as a file from before has none.
    let full = legacy(dir.path(), false);
    let old = dir.path().join("old.rete");
    std::fs::write(
        &old,
        rete_core::attach_build_info(&std::fs::read(&full).unwrap(), &[]).unwrap(),
    )
    .unwrap();
    assert!(rete_core::read_build_info(&std::fs::read(&old).unwrap())
        .unwrap()
        .is_none_or(|r| r.is_empty()));
    let out = dir.path().join("out.rete");
    convert(&old, &out);
    // No card, so `rete card` has nothing to print: read the record itself.
    let raw = rete_core::read_build_info(&std::fs::read(&out).unwrap())
        .unwrap()
        .expect("a record");
    let build: Value = serde_json::from_slice(raw.trim_ascii_end()).unwrap();
    assert!(
        build.get("engine").is_none() && build.get("counts").is_none(),
        "{build}"
    );
    assert_eq!(build["conversions"].as_array().unwrap().len(), 1, "{build}");
    assert_eq!(
        build["conversions"][0]["source_content_hash"],
        hash_of(&old).as_str()
    );
    rete().arg("verify").arg(&out).assert().success();
}

fn raw_record(file: &Path) -> String {
    let raw = rete_core::read_build_info(&std::fs::read(file).unwrap())
        .unwrap()
        .expect("a record");
    String::from_utf8(raw).unwrap().trim_end().to_string()
}

/// "Kept as it was" is literal: the source's record text, up to its closing
/// brace, is the start of the converted record.
#[test]
fn the_source_record_is_kept_byte_for_byte() {
    let dir = tempfile::tempdir().unwrap();
    let old = legacy(dir.path(), true);
    let out = dir.path().join("out.rete");
    convert(&old, &out);
    let (before, after) = (raw_record(&old), raw_record(&out));
    let prefix = before.strip_suffix('}').expect("a JSON object");
    assert!(
        after.starts_with(prefix),
        "{before}
{after}"
    );
    assert!(after.ends_with("]}"), "{after}");
}

/// A per-graph (kind 5) source is re-encoded rather than copied, and still
/// keeps its own record: engine, inputs, counts, start time.
#[test]
fn a_per_graph_source_keeps_its_record_too() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let mut nq = nt();
    nq.push_str("<http://example.test/e1> <http://example.test/in> <http://example.test/e2> <http://example.test/g1> .
");
    std::fs::write(&src, nq).unwrap();
    let old = dir.path().join("pg.rete");
    let o = rete()
        .env("RETE_WRITE_FORMAT", "5")
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000")
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&old)
        .args([
            "--no-pyramid",
            "--named-graphs",
            "per-graph",
            "--card",
            "--no-card-costs",
        ])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = dir.path().join("out.rete");
    let o = rete()
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000100")
        .arg("convert")
        .arg(&old)
        .arg("-o")
        .arg(&out)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("kind 5"),
        "the re-encode path ran"
    );
    let (before, after) = (raw_record(&old), raw_record(&out));
    let prefix = before.strip_suffix('}').unwrap();
    assert!(
        after.starts_with(prefix),
        "{before}
{after}"
    );
    let b = card_json(&out)["build"].clone();
    assert!(b["inputs"].is_array() && !b["started_at"].is_null(), "{b}");
    assert_eq!(b["conversions"].as_array().unwrap().len(), 1);
    assert_eq!(
        b["conversions"][0]["source_content_hash"],
        hash_of(&old).as_str()
    );
    rete().arg("verify").arg(&out).assert().success();
}
