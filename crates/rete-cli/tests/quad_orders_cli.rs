//! Format 0x07 quad orders through the CLI: a build (in RAM and bounded) and a
//! convert write GSPO, SPOG, POSG and OSPG whatever `--permutations` says,
//! `info` lists them, `verify` checks them tile by tile, and the same query
//! gives the same rows from every build.

mod common;

use std::path::Path;

use common::{json, rete};

/// Named-graph quads sharing subjects and objects across graphs.
fn nquads() -> String {
    let mut s = String::new();
    for i in 0..600 {
        let g = format!("<http://ex/g/{i}>");
        s.push_str(&format!(
            "<http://ex/e/{}> <http://ex/rel0> <http://ex/o/{}> {g} .\n",
            i % 40,
            (i * 13) % 97
        ));
        s.push_str(&format!(
            "<http://ex/s/{i}> <http://ex/rel1> <http://ex/e/{}> {g} .\n",
            i % 40
        ));
        s.push_str(&format!(
            "<http://ex/s/{i}> <http://ex/label> \"s {}\" {g} .\n",
            i % 11
        ));
    }
    s.push_str("<http://ex/e/1> <http://ex/rel0> <http://ex/o/1> <http://ex/g/7> .\n");
    s.push_str("<http://ex/e/1> <http://ex/title> \"one\" .\n");
    s
}

fn rows(f: &Path, q: &str) -> Vec<String> {
    let v = json(rete().arg("sparql").arg(f).arg(q).arg("--json"));
    let mut r: Vec<String> = v["results"]["bindings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b.to_string())
        .collect();
    r.sort();
    r
}

const QUERIES: &[&str] = &[
    "SELECT ?g ?p ?o WHERE { GRAPH ?g { <http://ex/e/1> ?p ?o } }",
    "SELECT ?g ?s WHERE { GRAPH ?g { ?s <http://ex/rel0> <http://ex/o/1> } }",
    "SELECT ?g ?s ?p WHERE { GRAPH ?g { ?s ?p <http://ex/e/3> } }",
    "SELECT ?g ?s WHERE { GRAPH <http://ex/g/7> { ?s <http://ex/label> ?l } }",
    "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s <http://ex/rel1> ?o } }",
];

fn quad_order_lines(f: &Path) -> Vec<String> {
    let out = rete().arg("info").arg(f).output().unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .skip_while(|l| !l.contains("quad orders"))
        .skip(1)
        .take_while(|l| l.starts_with("    "))
        .map(|l| l.trim().to_string())
        .collect()
}

#[test]
fn every_build_writes_the_four_orders_and_answers_alike() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("q.nq");
    std::fs::write(&src, nquads()).unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();

    let a = dir.path().join("ram6.rete");
    common::build(&src, &a, &["--no-pyramid"]);
    let b = dir.path().join("ram3.rete");
    common::build(&src, &b, &["--no-pyramid", "--permutations", "3"]);
    let c = dir.path().join("bounded.rete");
    common::build(
        &src,
        &c,
        &[
            "--no-pyramid",
            "--memory-budget-mb",
            "64",
            "--tmp-dir",
            tmp.to_str().unwrap(),
        ],
    );
    // A convert of a 0x07 file is a rewrite; a tiny sort run forces the spill.
    let d = dir.path().join("converted.rete");
    rete()
        .arg("convert")
        .arg(&a)
        .arg("-o")
        .arg(&d)
        .arg("--no-card-costs")
        .args(["--tmp-dir", tmp.to_str().unwrap()])
        .env("RETE_QUAD_RUN_ROWS", "200")
        .assert()
        .success();

    for f in [&a, &b, &c, &d] {
        assert_eq!(std::fs::read(f).unwrap()[4], 7, "{f:?}");
        rete().arg("verify").arg(f).assert().success();
        let lines = quad_order_lines(f);
        let names: Vec<&str> = lines
            .iter()
            .map(|l| l.split_whitespace().next().unwrap())
            .collect();
        assert_eq!(
            names,
            ["G+SPO", "SPO+G", "POS+G", "OSP+G"],
            "{f:?}: {lines:?}"
        );
    }
    // --permutations is the default graph's: the quad orders are the same.
    assert_eq!(quad_order_lines(&a), quad_order_lines(&b));
    // The bounded build left no spill behind.
    assert_eq!(
        std::fs::read_dir(&tmp).unwrap().count(),
        0,
        "spill left in tmp"
    );
    for q in QUERIES {
        let want = rows(&a, q);
        assert!(!want.is_empty(), "{q}");
        for f in [&b, &c, &d] {
            assert_eq!(rows(f, q), want, "{q} on {f:?}");
        }
    }
}

#[test]
fn verify_checks_the_orders_and_catches_a_damaged_one() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("q.nq");
    std::fs::write(&src, nquads()).unwrap();
    let f = dir.path().join("a.rete");
    common::build(&src, &f, &["--no-pyramid"]);
    rete().arg("verify").arg(&f).assert().success();
    // Damage a byte inside the quad payload: the digest catches it (and says
    // which section), and the section check names it too.
    let mut bytes = std::fs::read(&f).unwrap();
    let (h, root) = rete_core::v06::open::read_front(&rete_core::SliceReader::new(&bytes)).unwrap();
    let q = root
        .section(rete_core::v06::kind::QUAD_DATA)
        .unwrap()
        .abs(&h)
        .unwrap();
    let at = (q.offset + q.len - 40) as usize;
    bytes[at] ^= 0x55;
    let g = dir.path().join("b.rete");
    std::fs::write(&g, &bytes).unwrap();
    let out = rete().arg("verify").arg(&g).output().unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("quad"), "{err}");
}
