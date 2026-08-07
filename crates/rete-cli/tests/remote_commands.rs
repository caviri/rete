mod common;

use std::sync::atomic::Ordering;

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

    let ignores_range = common::serve(bytes, common::RangeMode::Ignore);
    common::rete()
        .arg("query-url")
        .arg(&ignores_range)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("ignored Range"));
}

#[test]
fn sparql_url_eager_fetches_an_eligible_object_once() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let len = bytes.len();
    let (url, stats) = common::serve_with_stats(bytes, common::RangeMode::Honor);
    let output = common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &url, SELECT, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
    assert_eq!(*stats.ranges.lock().unwrap(), vec![(0, len - 1)]);
}

#[test]
fn invalid_eager_configuration_fails_before_networking() {
    let fixture = common::fixture();
    let (url, stats) = common::serve_with_stats(
        std::fs::read(&fixture.rete).unwrap(),
        common::RangeMode::Honor,
    );
    common::rete()
        .env("RETE_EAGER_MAX_MB", "eight")
        .args(["sparql-url", &url, SELECT, "--json"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("RETE_EAGER_MAX_MB"));
    assert_eq!(stats.heads.load(Ordering::SeqCst), 0);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 0);
}

#[test]
fn sparql_url_zero_threshold_forces_the_lazy_path() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let len = bytes.len();
    let (url, stats) = common::serve_with_stats(bytes, common::RangeMode::Honor);
    let output = common::rete()
        .env("RETE_EAGER_MAX_MB", "0")
        .env("RETE_BLOCK_KB", "0")
        .args(["sparql-url", &url, SELECT, "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert!(stats.gets.load(Ordering::SeqCst) > 1);
    assert!(!stats.ranges.lock().unwrap().contains(&(0, len - 1)));
}

#[test]
fn sparql_url_eager_and_lazy_stdout_are_identical() {
    let fixture = common::fixture();
    let bytes = std::fs::read(&fixture.rete).unwrap();
    let eager_url = common::serve(bytes.clone(), common::RangeMode::Honor);
    let lazy_url = common::serve(bytes, common::RangeMode::Honor);

    let eager = common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &eager_url, SELECT, "--json"])
        .output()
        .unwrap();
    let lazy = common::rete()
        .env("RETE_EAGER_MAX_MB", "0")
        .env("RETE_BLOCK_KB", "0")
        .args(["sparql-url", &lazy_url, SELECT, "--json"])
        .output()
        .unwrap();

    assert!(eager.status.success());
    assert!(lazy.status.success());
    assert_eq!(eager.stdout, lazy.stdout);
}

#[test]
fn sparql_url_eager_rejects_a_truncated_response() {
    let fixture = common::fixture();
    let (url, stats) = common::serve_with_stats(
        std::fs::read(&fixture.rete).unwrap(),
        common::RangeMode::Truncate,
    );
    common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &url, SELECT, "--json"])
        .assert()
        .code(1)
        .stderr(
            predicate::str::contains("short range response")
                .or(predicate::str::contains("closed before")),
        );
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn sparql_url_eager_rejects_a_server_that_ignores_range() {
    let fixture = common::fixture();
    let (url, stats) = common::serve_with_stats(
        std::fs::read(&fixture.rete).unwrap(),
        common::RangeMode::Ignore,
    );
    common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &url, SELECT, "--json"])
        .assert()
        .code(1)
        .stderr(predicate::str::contains("ignored Range"));
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
}

#[test]
fn sparql_url_eager_rejects_malformed_eligible_bytes_without_panicking() {
    let bytes = b"not a rete file".to_vec();
    let len = bytes.len();
    let (url, stats) = common::serve_with_stats(bytes, common::RangeMode::Honor);
    let output = common::rete()
        .env("RETE_EAGER_MAX_MB", "8")
        .args(["sparql-url", &url, SELECT, "--json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("panicked"));
    assert_eq!(stats.heads.load(Ordering::SeqCst), 1);
    assert_eq!(stats.gets.load(Ordering::SeqCst), 1);
    assert_eq!(*stats.ranges.lock().unwrap(), vec![(0, len - 1)]);
}
