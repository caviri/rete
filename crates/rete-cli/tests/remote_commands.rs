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
