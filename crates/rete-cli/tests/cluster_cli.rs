//! `--cluster-subjects-by-class` through the CLI: `build` (in RAM and bounded)
//! and `convert` write the same clustered permutations, `info` lists the runs,
//! `verify` checks every row against its class, a clustered file converted
//! without the flag is the unclustered file again, and the answers do not
//! change.

mod common;

use std::path::Path;

use common::{json, rete};

/// 8,000 records of A (40%), B (25%), C (15%), D (5%, too few to get a run),
/// untyped, scattered by IRI.
fn ntriples() -> String {
    let mut s = String::new();
    for i in 0..8_000u64 {
        let subj = format!("<http://ex/r/{:06}>", (i * 7919) % 100_003);
        let class = match i % 20 {
            0..=7 => Some("A"),
            8..=12 => Some("B"),
            13..=15 => Some("C"),
            16 => Some("D"),
            _ => None,
        };
        if let Some(c) = class {
            s.push_str(&format!(
                "{subj} <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/C{c}> .\n"
            ));
        }
        s.push_str(&format!("{subj} <http://ex/name> \"n{i}\" .\n"));
        s.push_str(&format!(
            "{subj} <http://ex/v> <http://ex/o/{}> .\n",
            i % 50
        ));
    }
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

/// The `info` lines that say how the index is laid out: the section digests of
/// the index and its leaves, and the cluster runs.
fn index_lines(f: &Path) -> Vec<String> {
    let out = rete().arg("info").arg(f).output().unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with("index ")
                || t.starts_with("index-leaves")
                || t.starts_with("clusters")
                || t.starts_with("class ")
                || t.starts_with("rest ")
        })
        .map(|l| {
            // A section line: its name and digest (offsets move with the card).
            let t: Vec<&str> = l.split_whitespace().collect();
            if t[0].starts_with("index") && t.len() > 3 && t[0] != "index" || t.contains(&"digest")
            {
                format!("{} {}", t[0], t[t.len() - 1])
            } else {
                t.join(" ")
            }
        })
        .collect()
}

#[test]
fn builds_and_converts_agree_and_the_flag_is_reversible() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("t.nt");
    std::fs::write(&src, ntriples()).unwrap();
    let tmp = dir.path().join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    // Several tiles per run.
    let tiles = ("RETE_TILE_KIB", "4");

    let build = |name: &str, extra: &[&str]| {
        let out = dir.path().join(name);
        let mut c = rete();
        c.env(tiles.0, tiles.1)
            .arg("build")
            .arg(&src)
            .arg("-o")
            .arg(&out)
            .arg("--no-pyramid")
            .args(extra);
        c.assert().success();
        out
    };
    let plain = build("plain.rete", &[]);
    let ram = build("ram.rete", &["--cluster-subjects-by-class"]);
    let bounded = build(
        "bounded.rete",
        &[
            "--cluster-subjects-by-class",
            "--memory-budget-mb",
            "64",
            "--tmp-dir",
            tmp.to_str().unwrap(),
        ],
    );
    let convert = |from: &Path, name: &str, flag: bool| {
        let out = dir.path().join(name);
        let mut c = rete();
        c.env(tiles.0, tiles.1)
            .arg("convert")
            .arg(from)
            .arg("-o")
            .arg(&out)
            .arg("--no-card-costs");
        if flag {
            c.arg("--cluster-subjects-by-class");
        }
        c.assert().success();
        out
    };
    let clustered = convert(&plain, "clustered.rete", true);
    let back = convert(&clustered, "back.rete", false);
    let again = convert(&back, "again.rete", true);

    // The unclustered file has no runs; the clustered ones list three classes.
    let p = index_lines(&plain);
    assert!(!p.iter().any(|l| l.starts_with("clusters")), "{p:?}");
    let r = index_lines(&ram);
    assert!(r.iter().any(|l| l.starts_with("clusters")), "{r:?}");
    assert_eq!(
        r.iter().filter(|l| l.starts_with("class ")).count(),
        6,
        "three classes in each of SPO and SOP: {r:?}"
    );
    // The same layout from every writer.
    assert_eq!(index_lines(&bounded), r, "bounded against in RAM");
    assert_eq!(index_lines(&clustered), r, "convert against build");
    assert_eq!(index_lines(&again), r);
    // Without the flag a clustered file is the unclustered one again.
    assert_eq!(index_lines(&back), p);
    assert_eq!(
        std::fs::read(&back).unwrap()[5] & 0b1_0000,
        0,
        "flag bit cleared"
    );
    assert_ne!(
        std::fs::read(&ram).unwrap()[5] & 0b1_0000,
        0,
        "flag bit set"
    );

    for f in [&ram, &bounded, &clustered, &back, &again] {
        rete().arg("verify").arg(f).assert().success();
    }
    let out = rete().arg("verify").arg(&ram).output().unwrap();
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("clustered permutations"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );

    let qs = [
        "SELECT ?s ?n WHERE { ?s a <http://ex/CB> . ?s <http://ex/name> ?n }",
        "SELECT ?s WHERE { ?s a <http://ex/CD> }",
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        "SELECT ?p ?o WHERE { <http://ex/r/000000> ?p ?o }",
        "SELECT ?p ?o WHERE { <http://ex/r/007919> ?p ?o }",
        "SELECT ?c (COUNT(*) AS ?n) WHERE { ?s a ?c } GROUP BY ?c",
    ];
    for q in qs {
        let want = rows(&plain, q);
        assert!(!want.is_empty(), "{q}");
        for f in [&ram, &bounded, &clustered] {
            assert_eq!(rows(f, q), want, "{q} on {f:?}");
        }
    }
}
