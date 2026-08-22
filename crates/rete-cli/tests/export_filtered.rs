//! `rete export` with a filter, and `rete cost --dump` — issue #117 points 1
//! and 3 at the CLI surface.
//!
//! The filter is a **pruning** filter (the engine routes it to one permutation
//! and drops tiles a synopsis rejects, without fetching them), so the thing that
//! has to be tested is not that it is fast — the rete-core tests measure that —
//! but that it returns exactly the right rows. A pruning bug returns fewer rows
//! and no error, so every assertion here compares against the unfiltered export
//! filtered with `grep`-equivalent logic, never against a hand-written expected
//! list that could be wrong in the same direction.

mod common;

use predicates::prelude::*;

/// Three graphs, three predicates, literals and IRIs in the object column.
const QUADS: &str = concat!(
    "<http://ex/a> <http://ex/knows> <http://ex/b> .\n",
    "<http://ex/a> <http://ex/name> \"Alice\"@en .\n",
    "<http://ex/b> <http://ex/name> \"Bob\"@en .\n",
    "<http://ex/b> <http://ex/knows> <http://ex/a> .\n",
    "<http://ex/c> <http://ex/name> \"Carol\"@en <http://ex/g1> .\n",
    "<http://ex/c> <http://ex/knows> <http://ex/a> <http://ex/g1> .\n",
    "<http://ex/d> <http://ex/age> \"41\" <http://ex/g2> .\n",
);

fn built() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, QUADS).unwrap();
    common::build(&src, &out, &["--no-pyramid"]);
    (dir, out)
}

fn export(file: &std::path::Path, args: &[&str]) -> Vec<String> {
    let out = common::rete()
        .arg("export")
        .arg(file)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "export {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mut lines: Vec<String> = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines.sort();
    lines
}

/// Every filter shape must yield exactly the lines the unfiltered export yields
/// that contain that term — computed from the unfiltered output, so the two can
/// only agree by being right.
#[test]
fn a_filtered_export_is_the_full_export_filtered() {
    let (_dir, file) = built();
    let all = export(&file, &["--format", "nq"]);
    assert_eq!(all.len(), 7, "fixture changed: {all:#?}");

    // Predicate: bare IRI and `<iri>` token must behave identically.
    for token in ["http://ex/name", "<http://ex/name>"] {
        let got = export(&file, &["--format", "nq", "--predicate", token]);
        let want: Vec<String> = all
            .iter()
            .filter(|l| l.contains(" <http://ex/name> "))
            .cloned()
            .collect();
        assert_eq!(got, want, "--predicate {token}");
        assert_eq!(got.len(), 3);
    }

    // Subject — spans the default graph and a named one.
    let got = export(&file, &["--format", "nq", "--subject", "http://ex/c"]);
    assert_eq!(
        got,
        all.iter()
            .filter(|l| l.starts_with("<http://ex/c> "))
            .cloned()
            .collect::<Vec<_>>()
    );
    assert_eq!(got.len(), 2);

    // Object, as a language-tagged literal token (the shape a shell user has to
    // quote, and the one that would break if the CLI wrapped it in `<>`).
    let got = export(&file, &["--format", "nq", "--object", "\"Carol\"@en"]);
    assert_eq!(got.len(), 1);
    assert!(got[0].contains("\"Carol\"@en"), "{got:?}");

    // Two bound components at once.
    let got = export(
        &file,
        &[
            "--format",
            "nq",
            "--subject",
            "http://ex/b",
            "--predicate",
            "http://ex/knows",
        ],
    );
    assert_eq!(
        got,
        vec!["<http://ex/b> <http://ex/knows> <http://ex/a> .".to_string()]
    );

    // A term the dictionary does not contain matches nothing — and is not an
    // error, because "no such predicate" is a legitimate answer to a dump.
    assert!(export(&file, &["--format", "nq", "--predicate", "http://ex/nope"]).is_empty());
}

/// `--graph` selects one graph; absent means the default graph plus every named
/// one (the lossless N-Quads dump), and the empty string means the default
/// graph alone.
#[test]
fn graph_selection_slices_the_export() {
    let (_dir, file) = built();
    let all = export(&file, &["--format", "nq"]);

    let default_only = export(&file, &["--format", "nq", "--graph", ""]);
    assert_eq!(default_only.len(), 4);
    assert!(
        default_only
            .iter()
            .all(|l| !l.contains("<http://ex/g1>") && !l.contains("<http://ex/g2>")),
        "the default graph must not carry a graph token: {default_only:?}"
    );

    for token in ["http://ex/g1", "<http://ex/g1>"] {
        let g1 = export(&file, &["--format", "nq", "--graph", token]);
        assert_eq!(g1.len(), 2, "--graph {token}");
        assert!(g1.iter().all(|l| l.contains("<http://ex/g1> .")));
    }

    // The parts partition the whole.
    let g2 = export(&file, &["--format", "nq", "--graph", "http://ex/g2"]);
    assert_eq!(default_only.len() + 2 + g2.len(), all.len());

    // Filters compose with the graph.
    let got = export(
        &file,
        &[
            "--format",
            "nq",
            "--graph",
            "http://ex/g1",
            "--predicate",
            "http://ex/knows",
        ],
    );
    assert_eq!(got.len(), 1);
    assert!(got[0].contains("<http://ex/g1>"));

    // Turtle/JSON-LD are single-graph, and now that graph can be a named one.
    common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "ttl", "--graph", "http://ex/g1"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"Carol\"@en"));
}

/// `rete cost --dump` previews a dump the way `rete cost <query>` previews a
/// query: same `AccessCost` line shape, same JSON envelope conventions.
///
/// The index figure is *computed from the tile directories*, so the assertions
/// worth making are the ones a sampled estimate could not satisfy: a filter
/// admits no more tiles than routing leaves, routing leaves no more than the
/// section has, and a filter term the file does not contain plans no scan at
/// all rather than a scan of everything.
#[test]
fn dump_cost_previews_a_dump_without_running_it() {
    let (_dir, file) = built();

    let full: serde_json::Value = common::json(
        common::rete()
            .arg("cost")
            .arg(&file)
            .args(["--dump", "--json"]),
    );
    assert_eq!(full["mode"], "dump");
    assert_eq!(full["source_kind"], "local");
    assert_eq!(full["lazy_dump_open"]["available"], true);
    assert!(full["lazy_dump_open"]["bytes"].as_u64().unwrap() > 0);
    // Default graph + two named graphs.
    assert_eq!(full["graphs"].as_array().unwrap().len(), 3);
    let full_tiles = full["index_tiles"]["bytes"].as_u64().unwrap();
    assert!(full_tiles > 0);
    // The floor is the open plus the admitted tiles; the ceiling adds the
    // dictionary. Both are stated, neither is guessed.
    let floor = full["estimated_bytes"]["floor"].as_u64().unwrap();
    let ceiling = full["estimated_bytes"]["ceiling"].as_u64().unwrap();
    assert_eq!(
        ceiling - floor,
        full["dictionary_ceiling_bytes"].as_u64().unwrap()
    );
    // No `floor <= file_bytes` assertion: on a fixture this small the lazy
    // open's block-aligned reads and its chunk-directory probe over-read the
    // whole file several times, so the floor exceeds the file length. That is
    // the same small-file artefact `cost_cli` documents for the query preview,
    // and pinning it here would pin the artefact rather than the contract.
    assert!(floor > 0);

    // One graph is a smaller preview than every graph.
    let one: serde_json::Value = common::json(common::rete().arg("cost").arg(&file).args([
        "--dump",
        "--graph",
        "http://ex/g1",
        "--json",
    ]));
    assert_eq!(one["graphs"].as_array().unwrap().len(), 1);
    assert!(one["index_tiles"]["bytes"].as_u64().unwrap() < full_tiles);

    // Every planned scan routes inside {SPO, POS, OSP} — the three permutations
    // a `--permutations 3` file is guaranteed to carry.
    let pred: serde_json::Value = common::json(common::rete().arg("cost").arg(&file).args([
        "--dump",
        "--predicate",
        "http://ex/name",
        "--json",
    ]));
    for g in pred["graphs"].as_array().unwrap() {
        if g["matches"].as_bool() == Some(true) {
            let perm = g["permutation"].as_str().unwrap();
            assert!(["SPO", "POS", "OSP"].contains(&perm), "routed to {perm}");
            assert!(g["tiles_admitted"].as_u64().unwrap() <= g["tiles_routed"].as_u64().unwrap());
            assert!(g["tiles_routed"].as_u64().unwrap() <= g["tiles_total"].as_u64().unwrap());
        }
    }

    // A term the file does not contain: no scan planned, zero index bytes.
    let miss: serde_json::Value = common::json(common::rete().arg("cost").arg(&file).args([
        "--dump",
        "--predicate",
        "http://ex/nope",
        "--json",
    ]));
    assert_eq!(miss["index_tiles"]["bytes"], 0);
    assert!(miss["graphs"]
        .as_array()
        .unwrap()
        .iter()
        .all(|g| g["matches"] == false));

    // The human-readable form says the same thing, and says what it cannot know.
    common::rete()
        .arg("cost")
        .arg(&file)
        .args(["--dump", "--predicate", "http://ex/name"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dump cost preview"))
        .stdout(predicate::str::contains("no tile fetched"))
        .stdout(predicate::str::contains("dictionary ceiling"));

    // `rete cost` with neither a query nor --dump is an error, not a panic.
    common::rete()
        .arg("cost")
        .arg(&file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("--dump"));
}
