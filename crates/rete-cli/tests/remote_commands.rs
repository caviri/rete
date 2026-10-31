mod common;

use predicates::prelude::*;

const SELECT: &str =
    "SELECT ?o WHERE { <http://example.test/alice> <http://example.test/knows> ?o }";

#[test]
fn remote_commands_use_a_local_range_server() {
    let fixture = common::fixture();
    let remote_file = fixture.path("remote.rete");
    common::build(
        &fixture.source,
        &remote_file,
        &["--card", "--title", "Remote fixture"],
    );
    let url = common::serve(
        std::fs::read(&remote_file).unwrap(),
        common::RangeMode::Honor,
    );

    common::rete()
        .arg("summary-url")
        .arg(&url)
        .assert()
        .success()
        .stdout(predicate::str::contains("round"));
    common::rete()
        .arg("query-url")
        .arg(&url)
        .args(["--predicate", "<http://example.test/knows>"])
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/bob"));

    let why = common::json(common::rete().arg("why-url").arg(&url).args([
        "--predicate",
        "<http://example.test/knows>",
        "--json",
    ]));
    assert_eq!(why["schemaVersion"], 1);
    assert_eq!(why["result_count"], 1);

    let card = common::json(common::rete().arg("card-url").arg(&url).arg("--json"));
    assert_eq!(card["schemaVersion"], 1);
    assert_eq!(card["title"], "Remote fixture");

    let sparql = common::json(
        common::rete()
            .arg("sparql-url")
            .arg(&url)
            .arg(SELECT)
            .arg("--json"),
    );
    assert!(sparql["results"]["bindings"].is_array());
    // The same query from a file and from standard input.
    let qfile = fixture.write("select.rq", SELECT);
    let from_file = common::json(
        common::rete()
            .arg("sparql-url")
            .arg(&url)
            .arg("--query-file")
            .arg(&qfile)
            .arg("--json"),
    );
    assert_eq!(from_file, sparql);
    let from_stdin = common::json(
        common::rete()
            .arg("sparql-url")
            .arg(&url)
            .args(["--query-file", "-", "--json"])
            .write_stdin(SELECT),
    );
    assert_eq!(from_stdin, sparql);

    let cost = common::json(
        common::rete()
            .arg("cost")
            .arg(&url)
            .arg(SELECT)
            .arg("--json"),
    );
    assert_eq!(cost["schemaVersion"], 1);
    let progressive = common::json(
        common::rete()
            .arg("progressive")
            .arg(&url)
            .arg("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")
            .arg("--json"),
    );
    assert_eq!(progressive["schemaVersion"], 1);

    let shapes = fixture.write("empty.ttl", "@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    let shacl = common::json(
        common::rete()
            .arg("shacl-url")
            .arg(&url)
            .arg("--shapes")
            .arg(&shapes)
            .args(["--format", "json"]),
    );
    assert_eq!(shacl["schemaVersion"], 1);
    assert_eq!(shacl["conforms"], true);

    common::rete()
        .arg("reason")
        .arg("--url")
        .arg(&url)
        .arg("--check")
        .assert()
        .success()
        .stdout(predicate::str::contains("coherent"));
}

#[test]
fn remote_http_failures_exit_one_without_panicking() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let missing = common::serve(bytes.clone(), common::RangeMode::NotFound);
    common::rete()
        .arg("summary-url")
        .arg(&missing)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("404"));
}

/// A host that ignores `Range` answers `200` with the whole file. Under the
/// 256 MiB cap the reader keeps that body and serves every read from it (G6):
/// the same answer as from a range host, never a mis-sliced one. (The refusal
/// above the cap is pinned in rete-core's `http` tests and against a real nginx
/// in `tests/interop/range_ignoring_host.sh`.)
#[test]
fn a_host_that_ignores_range_is_served_from_its_whole_body() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let answer = |url: &str| {
        let out = common::rete()
            .arg("sparql-url")
            .arg(url)
            .arg(SELECT)
            .arg("--json")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        json["results"]["bindings"].clone()
    };
    let ranged = answer(&common::serve(bytes.clone(), common::RangeMode::Honor));
    let whole = answer(&common::serve(bytes, common::RangeMode::Ignore));
    assert_eq!(whole, ranged);
    assert!(!whole.as_array().unwrap().is_empty());
}
