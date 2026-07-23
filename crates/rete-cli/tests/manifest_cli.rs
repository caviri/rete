//! `rete manifest` end-to-end: init/add/status, **pattern-level joins across
//! segments** (the property `federate`'s per-source UNION cannot provide),
//! tombstone deletes + re-adds (ordered fold), journal seal, and compact.

mod common;

use std::path::PathBuf;

use common::{build, json, rete, Fixture};

/// Build a `--no-pyramid` segment from inline N-Quads text.
fn segment(f: &Fixture, name: &str, nq: &str) -> PathBuf {
    let src = f.write(&format!("{name}.nq"), nq);
    let out = f.path(&format!("{name}.rete"));
    build(&src, &out, &["--no-pyramid"]);
    out
}

fn init(manifest: &PathBuf, base: &PathBuf) {
    let mut cmd = rete();
    cmd.args(["manifest", "init"])
        .arg(manifest)
        .arg(base)
        .assert()
        .success();
}

fn add(manifest: &PathBuf, adds: Option<&PathBuf>, dels: Option<&PathBuf>) {
    let mut cmd = rete();
    cmd.args(["manifest", "add"]).arg(manifest);
    if let Some(a) = adds {
        cmd.arg("--adds").arg(a);
    }
    if let Some(d) = dels {
        cmd.arg("--dels").arg(d);
    }
    cmd.assert().success();
}

fn select_values(manifest: &PathBuf, query: &str, var: &str) -> Vec<String> {
    let mut cmd = rete();
    cmd.args(["manifest", "query"])
        .arg(manifest)
        .arg(query)
        .arg("--json");
    let out = json(&mut cmd);
    out["results"]["bindings"]
        .as_array()
        .expect("bindings array")
        .iter()
        .map(|b| b[var]["value"].as_str().expect("bound value").to_string())
        .collect()
}

fn status_json(manifest: &PathBuf, count: bool) -> serde_json::Value {
    let mut cmd = rete();
    cmd.args(["manifest", "status"]).arg(manifest).arg("--json");
    if count {
        cmd.arg("--count");
    }
    json(&mut cmd)
}

/// A join whose two patterns live in DIFFERENT segments must resolve: the
/// manifest is one logical graph with one merged dictionary, not a UNION of
/// independently-queried sources.
#[test]
fn query_joins_across_segments() {
    let f = common::fixture();
    let base = segment(
        &f,
        "base",
        "<http://ex/a> <http://ex/knows> <http://ex/b> .\n",
    );
    let delta = segment(&f, "delta", "<http://ex/b> <http://ex/name> \"Bob\" .\n");
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &base);
    add(&manifest, Some(&delta), None);

    let names = select_values(
        &manifest,
        "SELECT ?n WHERE { <http://ex/a> <http://ex/knows> ?x . ?x <http://ex/name> ?n }",
        "n",
    );
    assert_eq!(names, ["Bob"]);

    let status = status_json(&manifest, true);
    assert_eq!(status["generation"], 2);
    assert_eq!(status["entries"], 2);
    assert_eq!(status["verified"], true);
    assert_eq!(status["visible_quads"], 2);
}

/// The ordered fold: an update is delete+insert in one entry; a LATER entry
/// re-adding a tombstoned quad wins over the older tombstone.
#[test]
fn tombstone_update_then_readd() {
    let f = common::fixture();
    let base = segment(
        &f,
        "base",
        "<http://ex/a> <http://ex/name> \"Old\" .\n\
         <http://ex/a> <http://ex/kind> <http://ex/T> .\n",
    );
    let old_name = segment(&f, "oldname", "<http://ex/a> <http://ex/name> \"Old\" .\n");
    let new_name = segment(&f, "newname", "<http://ex/a> <http://ex/name> \"New\" .\n");
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &base);

    // Update = one entry that deletes the old value and adds the new one.
    add(&manifest, Some(&new_name), Some(&old_name));
    let q = "SELECT ?n WHERE { <http://ex/a> <http://ex/name> ?n } ORDER BY ?n";
    assert_eq!(select_values(&manifest, q, "n"), ["New"]);
    // The untouched quad is still visible.
    assert_eq!(status_json(&manifest, true)["visible_quads"], 2);

    // Re-add after delete: the same file that served as the tombstone now
    // serves as adds — a later log entry wins over the older tombstone.
    add(&manifest, Some(&old_name), None);
    assert_eq!(select_values(&manifest, q, "n"), ["New", "Old"]);
}

/// Named-graph quads survive the fold (RawQuad carries the graph).
#[test]
fn named_graphs_survive_the_fold() {
    let f = common::fixture();
    // The stock fixture holds 2 default-graph quads + 1 in a named graph.
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &f.rete);
    assert_eq!(status_json(&manifest, true)["visible_quads"], 3);
    let names = select_values(
        &manifest,
        "SELECT ?n WHERE { GRAPH <http://example.test/people> { ?s <http://example.test/name> ?n } }",
        "n",
    );
    assert_eq!(names, ["Alice"]);
}

/// Seal nets the journal per quad (last op wins) into an adds segment and a
/// tombstone segment, appends ONE log entry, and truncates the journal.
#[test]
fn seal_nets_the_journal_into_segments() {
    let f = common::fixture();
    let base = segment(
        &f,
        "base",
        "<http://ex/a> <http://ex/name> \"Alice\" .\n\
         <http://ex/b> <http://ex/name> \"Bob\" .\n",
    );
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &base);

    // The journal `rete serve` would have written: an insert, a delete, and
    // an insert that is deleted again within the same chunk (nets to a
    // harmless tombstone).
    let journal = f.write(
        "g.rete-manifest.json.changes",
        "+ <http://ex/c> <http://ex/name> \"Carol\" .\n\
         - <http://ex/a> <http://ex/name> \"Alice\" .\n\
         + <http://ex/d> <http://ex/name> \"Dave\" .\n\
         - <http://ex/d> <http://ex/name> \"Dave\" .\n",
    );

    let mut cmd = rete();
    cmd.args(["manifest", "seal"])
        .arg(&manifest)
        .assert()
        .success();

    // The journal was checkpointed: truncated, not deleted.
    assert_eq!(std::fs::read_to_string(&journal).unwrap(), "");
    let status = status_json(&manifest, true);
    assert_eq!(status["generation"], 2);
    assert_eq!(status["entries"], 2);
    assert_eq!(status["verified"], true);
    assert_eq!(status["visible_quads"], 2);

    let q = "SELECT ?n WHERE { ?s <http://ex/name> ?n } ORDER BY ?n";
    assert_eq!(select_values(&manifest, q, "n"), ["Bob", "Carol"]);

    // Sealing an empty journal is a no-op: no new generation.
    let mut cmd = rete();
    cmd.args(["manifest", "seal"])
        .arg(&manifest)
        .assert()
        .success();
    assert_eq!(status_json(&manifest, false)["generation"], 2);
}

/// Compact folds the whole log into one fresh `.rete`, resets the manifest to
/// a single entry, and is deterministic: compacting the same visible graph
/// again pins an identical content hash.
#[test]
fn compact_folds_to_one_deterministic_segment() {
    let f = common::fixture();
    let base = segment(
        &f,
        "base",
        "<http://ex/a> <http://ex/name> \"Alice\" .\n\
         <http://ex/b> <http://ex/name> \"Bob\" .\n",
    );
    let tomb = segment(&f, "tomb", "<http://ex/a> <http://ex/name> \"Alice\" .\n");
    let delta = segment(&f, "delta", "<http://ex/c> <http://ex/name> \"Carol\" .\n");
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &base);
    add(&manifest, Some(&delta), Some(&tomb));

    let q = "SELECT ?n WHERE { ?s <http://ex/name> ?n } ORDER BY ?n";
    let before = select_values(&manifest, q, "n");
    assert_eq!(before, ["Bob", "Carol"]);

    let mut cmd = rete();
    cmd.args(["manifest", "compact"])
        .arg(&manifest)
        .assert()
        .success();

    let status = status_json(&manifest, true);
    assert_eq!(status["entries"], 1);
    assert_eq!(status["generation"], 3);
    assert_eq!(status["visible_quads"], 2);
    assert_eq!(select_values(&manifest, q, "n"), before);
    let pin1 = status["segments"][0]["blake3_16"]
        .as_str()
        .unwrap()
        .to_string();

    // The compacted artifact is a full, verifiable .rete.
    let compacted = f.path(status["segments"][0]["url"].as_str().unwrap());
    let mut cmd = rete();
    cmd.arg("verify").arg(&compacted).assert().success();

    // Same visible graph → byte-identical rebuild → identical pin.
    let mut cmd = rete();
    cmd.args(["manifest", "compact"])
        .arg(&manifest)
        .assert()
        .success();
    let status = status_json(&manifest, false);
    assert_eq!(status["segments"][0]["blake3_16"].as_str().unwrap(), pin1);
}

/// A segment that changed on disk after being pinned must fail loudly — a
/// silent partial/wrong answer is the one unacceptable outcome.
#[test]
fn drifted_segment_fails_status_and_query() {
    let f = common::fixture();
    let base = segment(&f, "base", "<http://ex/a> <http://ex/name> \"Alice\" .\n");
    let delta = segment(&f, "delta", "<http://ex/b> <http://ex/name> \"Bob\" .\n");
    let manifest = f.path("g.rete-manifest.json");
    init(&manifest, &base);
    add(&manifest, Some(&delta), None);

    // The segment is replaced AFTER being pinned: different content, same path.
    segment(
        &f,
        "delta",
        "<http://ex/z> <http://ex/name> \"Mallory\" .\n",
    );

    let mut cmd = rete();
    cmd.args(["manifest", "status"])
        .arg(&manifest)
        .assert()
        .failure();
    let mut cmd = rete();
    cmd.args(["manifest", "query"])
        .arg(&manifest)
        .arg("SELECT ?s WHERE { ?s ?p ?o }")
        .assert()
        .failure();
}
