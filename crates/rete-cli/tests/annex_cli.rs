//! Format 0x07 through the CLI: a card with a field over the hot cap goes to
//! the annex, `card --full` brings it back, `verify` covers it, a re-card and
//! a convert keep it, and `info` shows the root and the annex.

mod common;

use common::{json, rete};
use std::path::Path;

fn big_card() -> String {
    // 114 source checksums, the OpenCitations Index card's shape: 13 KB of `extra`.
    let md5s: Vec<String> = (0..114)
        .map(|i| format!("{:032x}", (i as u128) * 0x9e37_79b9_7f4a_7c15))
        .collect();
    serde_json::json!({
        "title": "annexed",
        "extra": { "sources": md5s.join(", "), "n": 114, "note": "x".repeat(8000) }
    })
    .to_string()
}

fn build_with_card(dir: &common::Fixture, name: &str) -> std::path::PathBuf {
    let card = dir.write("card.json", big_card());
    let out = dir.path(name);
    common::build(
        &dir.source,
        &out,
        &["--no-pyramid", "--card-file", card.to_str().unwrap()],
    );
    out
}

fn verify(f: &Path) {
    rete().arg("verify").arg(f).assert().success();
}

fn full_extra_n(f: &Path) -> serde_json::Value {
    let v = json(rete().arg("card").arg(f).args(["--json", "--full"]));
    v["extra"]["n"].clone()
}

#[test]
fn a_big_card_goes_to_the_annex_and_full_brings_it_back() {
    let fx = common::fixture();
    let f = build_with_card(&fx, "a.rete");
    assert_eq!(std::fs::read(&f).unwrap()[4], 8);
    verify(&f);
    // Hot: no `extra`, a marker. Full: the field is back and the marker gone.
    let hot = json(rete().arg("card").arg(&f).arg("--json"));
    assert!(hot.get("extra").is_none(), "{hot}");
    assert_eq!(hot["annex"]["fields"], serde_json::json!(["extra"]));
    let full = json(rete().arg("card").arg(&f).args(["--json", "--full"]));
    assert_eq!(full["extra"]["n"], 114);
    assert!(full.get("annex").is_none());
    // The text view says there is more; info shows the section and the root.
    let text = rete().arg("card").arg(&f).output().unwrap();
    assert!(String::from_utf8_lossy(&text.stdout).contains("card annex"));
    let info = rete().arg("info").arg(&f).output().unwrap();
    let info = String::from_utf8_lossy(&info.stdout).into_owned();
    assert!(info.contains("format 0x08"), "{info}");
    assert!(info.contains("card-annex"), "{info}");
    assert!(info.contains("root units"), "{info}");
}

#[test]
fn recard_keep_curated_and_convert_keep_the_annex() {
    let fx = common::fixture();
    let f = build_with_card(&fx, "a.rete");
    rete()
        .arg("recard")
        .arg(&f)
        .args(["--in-place", "--keep-curated", "--no-card-costs"])
        .assert()
        .success();
    verify(&f);
    assert_eq!(full_extra_n(&f), 114);
    let g = fx.path("converted.rete");
    rete()
        .arg("convert")
        .arg(&f)
        .arg("-o")
        .arg(&g)
        .arg("--no-card-costs")
        .assert()
        .success();
    verify(&g);
    assert_eq!(full_extra_n(&g), 114);
    let info = rete().arg("info").arg(&g).output().unwrap();
    assert!(String::from_utf8_lossy(&info.stdout).contains("card-annex"));
}

#[test]
fn convert_from_0x05_writes_0x07_with_no_annex() {
    let fx = common::fixture();
    let src = fx.write(
        "labels.nq",
        "<http://e/a> <http://www.w3.org/2000/01/rdf-schema#label> \"alpha one\" .
<http://e/b> <http://www.w3.org/2000/01/rdf-schema#label> \"beta two\" .
",
    );
    let old = fx.path("old.rete");
    let mut c = rete();
    c.env("RETE_WRITE_FORMAT", "5");
    c.arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&old)
        .args(["--no-pyramid", "--label-index", "--card"])
        .assert()
        .success();
    assert_eq!(std::fs::read(&old).unwrap()[4], 5);
    let new = fx.path("new.rete");
    rete()
        .arg("convert")
        .arg(&old)
        .arg("-o")
        .arg(&new)
        .arg("--no-card-costs")
        .assert()
        .success();
    assert_eq!(std::fs::read(&new).unwrap()[4], 8);
    verify(&new);
    let info = rete().arg("info").arg(&new).output().unwrap();
    let info = String::from_utf8_lossy(&info.stdout).into_owned();
    assert!(info.contains("label-root"), "{info}");
    assert!(!info.contains("card-annex"), "{info}");
    let hits = rete().arg("search").arg(&new).arg("alp").output().unwrap();
    assert!(hits.status.success());
    assert!(String::from_utf8_lossy(&hits.stdout).contains("alpha one"));
}

#[test]
fn a_corrupted_annex_fails_verify_by_name() {
    let fx = common::fixture();
    let f = build_with_card(&fx, "a.rete");
    let mut bytes = std::fs::read(&f).unwrap();
    let n = bytes.len();
    bytes[n - 20] ^= 0x10; // inside the annex: the last section, before the footer
    let bad = fx.path("bad.rete");
    std::fs::write(&bad, bytes).unwrap();
    let o = rete().arg("verify").arg(&bad).output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("card-annex"));
}
