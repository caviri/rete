//! `rete build` of Jelly input: by extension, `--format jelly`, gzipped, on
//! stdin, in RAM and memory-bounded, plus `validate` and the refusals. The
//! file is 3 quads in Jelly 1.1 row layout, written by hand and checked against
//! `jelly-cli`; the big checks (the conformance suite, the nanopub registry
//! stream against `jelly-cli`) are described in the PR.

mod common;

use common::rete;
use std::io::Write as _;

const TINY: &str = "bb010a0e0a0c1002481050085808700278020a1752151213687474703a2f2f6578616d706c652e6f72672f0a094a071205616c6963650a094a0712056b6e6f77730a074a051203626f620a084a0612046e616d650a064a04120267310a2c5a2a1228687474703a2f2f7777772e77332e6f72672f323030312f584d4c536368656d6123696e74656765720a0c1a0a0a0208012a004a007a000a171a152a0210045a0b0a05416c6963651202656e6a0210050a0a1a085a060a0234321801";

const EXPECTED: [&str; 3] = [
    "<http://example.org/alice> <http://example.org/knows> <http://example.org/bob> .",
    "<http://example.org/alice> <http://example.org/name> \"42\"^^<http://www.w3.org/2001/XMLSchema#integer> <http://example.org/g1> .",
    "<http://example.org/alice> <http://example.org/name> \"Alice\"@en <http://example.org/g1> .",
];

fn tiny() -> Vec<u8> {
    (0..TINY.len() / 2)
        .map(|i| u8::from_str_radix(&TINY[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

/// The statements of a .rete as sorted N-Quads lines.
fn exported(file: &std::path::Path) -> Vec<String> {
    let out = rete()
        .arg("export")
        .arg(file)
        .args(["--format", "nq"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

fn expected() -> Vec<String> {
    EXPECTED.iter().map(|s| s.to_string()).collect()
}

#[test]
fn builds_from_a_jelly_file_every_way() {
    let dir = tempfile::tempdir().unwrap();
    let jelly = dir.path().join("tiny.jelly");
    std::fs::write(&jelly, tiny()).unwrap();

    // in RAM, two passes, by extension
    let ram = dir.path().join("ram.rete");
    rete()
        .arg("build")
        .arg(&jelly)
        .arg("-o")
        .arg(&ram)
        .assert()
        .success();
    assert_eq!(exported(&ram), expected());

    // memory-bounded
    let bounded = dir.path().join("bounded.rete");
    let spill = dir.path().join("spill");
    std::fs::create_dir(&spill).unwrap();
    rete()
        .arg("build")
        .arg(&jelly)
        .arg("-o")
        .arg(&bounded)
        .args(["--memory-budget-mb", "64", "--tmp-dir"])
        .arg(&spill)
        .assert()
        .success();
    assert_eq!(exported(&bounded), expected());

    // gzipped, detected from the bytes; the name still says jelly
    let gz = dir.path().join("tiny.jelly.gz");
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    enc.write_all(&tiny()).unwrap();
    std::fs::write(&gz, enc.finish().unwrap()).unwrap();
    let from_gz = dir.path().join("gz.rete");
    rete()
        .arg("build")
        .arg(&gz)
        .arg("-o")
        .arg(&from_gz)
        .assert()
        .success();
    assert_eq!(exported(&from_gz), expected());

    // stdin needs the format named: Jelly has no magic number to guess from
    let piped = dir.path().join("stdin.rete");
    rete()
        .args(["build", "-", "--format", "jelly", "-o"])
        .arg(&piped)
        .write_stdin(tiny())
        .assert()
        .success();
    assert_eq!(exported(&piped), expected());
}

#[test]
fn validate_counts_a_jelly_file() {
    let dir = tempfile::tempdir().unwrap();
    let jelly = dir.path().join("tiny.jelly");
    std::fs::write(&jelly, tiny()).unwrap();
    let out = rete().arg("validate").arg(&jelly).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("valid: 3 statement(s)"), "{stdout}");
    assert!(
        stdout.contains("1 in the default graph, 1 named graph(s)"),
        "{stdout}"
    );
}

#[test]
fn a_damaged_jelly_file_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    let cut = dir.path().join("cut.jelly");
    let bytes = tiny();
    std::fs::write(&cut, &bytes[..bytes.len() - 4]).unwrap();
    let out = rete()
        .arg("build")
        .arg(&cut)
        .arg("-o")
        .arg(dir.path().join("cut.rete"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("jelly") && err.contains("truncated"), "{err}");

    // the same bytes read as N-Triples are not a Jelly file, and say so
    let as_nt = dir.path().join("tiny.nt");
    std::fs::write(&as_nt, bytes).unwrap();
    rete()
        .arg("build")
        .arg(&as_nt)
        .arg("-o")
        .arg(dir.path().join("nt.rete"))
        .assert()
        .failure();
}
