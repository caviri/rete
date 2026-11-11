//! `rete convert` of a `0x05` build is byte-identical to the direct `0x06`
//! build of the same input, card costs off (the costs in a build record are
//! measured on the file the build wrote, so a 0x05 build's differ by design)
//! (pyramid, quad index, label and text indexes, and
//! the `--no-pyramid` shape), and the card of each says the format it got.

mod common;

use std::path::Path;

use common::{rete, stored_card};

/// A small graph: labels, a type, a literal to text-index, two named graphs.
fn nquads() -> String {
    let mut s = String::new();
    for i in 0..40 {
        s.push_str(&format!(
            "<http://example.test/e{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Entity number {i}\"@en .\n\
             <http://example.test/e{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/T{}> .\n\
             <http://example.test/e{i}> <http://example.test/note> \"the quick brown fox {i} jumps\" .\n\
             <http://example.test/e{i}> <http://example.test/next> <http://example.test/e{}> .\n",
            i % 3,
            (i + 1) % 40
        ));
    }
    s.push_str("<http://example.test/e1> <http://example.test/in> <http://example.test/e2> <http://example.test/g1> .\n");
    s.push_str("<http://example.test/e3> <http://example.test/in> <http://example.test/e4> <http://example.test/g2> .\n");
    s
}

fn build(src: &Path, out: &Path, legacy: bool, extra: &[&str]) {
    let mut c = rete();
    c.env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000");
    if legacy {
        c.env("RETE_WRITE_FORMAT", "5");
    } else {
        c.env_remove("RETE_WRITE_FORMAT");
    }
    let o = c
        .arg("build")
        .arg(src)
        .arg("-o")
        .arg(out)
        .args(extra)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
}

fn card_version(file: &Path) -> u64 {
    let bytes = std::fs::read(file).unwrap();
    let card = stored_card(&bytes).expect("a card");
    let v: serde_json::Value = serde_json::from_slice(&card).unwrap();
    v["format_version"].as_u64().unwrap()
}

fn check(name: &str, extra: &[&str]) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    std::fs::write(&src, nquads()).unwrap();
    let old = dir.path().join("old.rete");
    let new = dir.path().join("new.rete");
    let conv = dir.path().join("conv.rete");
    build(&src, &old, true, extra);
    build(&src, &new, false, extra);
    assert_eq!(std::fs::read(&old).unwrap()[4], 5, "{name}: legacy build");
    assert_eq!(std::fs::read(&new).unwrap()[4], 8, "{name}: 0x08 build");
    assert_eq!(card_version(&old), 5, "{name}: 0x05 card");
    assert_eq!(card_version(&new), 8, "{name}: 0x08 card");
    let o = rete()
        .arg("convert")
        .arg(&old)
        .arg("-o")
        .arg(&conv)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(card_version(&conv), 8, "{name}: converted card");
    // The record says the file was converted, so the files differ there and
    // nowhere else: compare them without it.
    let plain = |p: &Path| rete_core::attach_build_info(&std::fs::read(p).unwrap(), &[]).unwrap();
    assert!(
        plain(&conv) == plain(&new),
        "{name}: convert of the 0x05 build differs from the direct 0x06 build"
    );
}

#[test]
fn convert_equals_direct_build_with_indexes() {
    check(
        "full",
        &["--card", "--no-card-costs", "--label-index", "--text-index"],
    );
}

#[test]
fn convert_equals_direct_build_without_pyramid() {
    check("no-pyramid", &["--card", "--no-card-costs", "--no-pyramid"]);
}
