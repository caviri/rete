mod common;

use predicates::prelude::*;

const SELECT: &str =
    "SELECT ?o WHERE { <http://example.test/alice> <http://example.test/knows> ?o }";

#[test]
fn local_query_commands_return_expected_results() {
    let fixture = common::fixture();
    common::rete()
        .arg("query")
        .arg(&fixture.rete)
        .args(["--predicate", "<http://example.test/knows>"])
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/bob"));
    common::rete()
        .arg("bgp")
        .arg(&fixture.rete)
        .arg("?s <http://example.test/knows> ?o")
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/bob"));

    let sparql = common::json(
        common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .arg(SELECT)
            .arg("--json"),
    );
    assert!(sparql["head"]["vars"].is_array());
    assert!(sparql["results"]["bindings"].is_array());
    assert!(sparql.get("schemaVersion").is_none());
}

#[test]
fn planner_and_progressive_json_are_schema_versioned() {
    let fixture = common::fixture();
    let cost = common::json(
        common::rete()
            .arg("cost")
            .arg(&fixture.rete)
            .arg(SELECT)
            .args(["--json", "--explain"]),
    );
    assert_eq!(cost["schemaVersion"], 1);
    assert!(cost["sections"].is_object());
    assert!(cost["explain"].is_object());
    common::rete()
        .arg("cost")
        .arg(&fixture.rete)
        .arg("ASK { ?s ?p ?o }")
        .assert()
        .success()
        .stdout(predicate::str::contains("query cost preview"));

    let with_pyramid = fixture.path("pyramid.rete");
    common::build(&fixture.source, &with_pyramid, &[]);
    let progressive = common::json(
        common::rete()
            .arg("progressive")
            .arg(&with_pyramid)
            .arg("SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }")
            .arg("--json"),
    );
    assert_eq!(progressive["schemaVersion"], 1);
    assert_eq!(progressive["progressive"]["reads_index"], false);
}

#[test]
fn cypher_and_federation_keep_standard_sparql_results_json() {
    let fixture = common::fixture();
    let cypher = common::json(
        common::rete()
            .arg("cypher")
            .arg(&fixture.rete)
            .arg("MATCH (s) RETURN s LIMIT 1")
            .arg("--json"),
    );
    assert!(cypher["results"]["bindings"].is_array());
    assert!(cypher.get("schemaVersion").is_none());

    let federated = common::json(
        common::rete()
            .arg("federate")
            .arg(&fixture.rete)
            .arg("--query")
            .arg(SELECT)
            .arg("--json"),
    );
    assert!(federated["results"]["bindings"].is_array());
    assert!(federated.get("schemaVersion").is_none());
}

/// `--memory-budget-mb` changes how much the reader may keep resident, never
/// the answer: the same query over the eager open, a lazy open at the default
/// budget, a lazy open under a tiny budget and an unlimited one all agree.
#[test]
fn sparql_memory_budget_never_changes_the_answer() {
    let fixture = common::fixture();
    let q = "SELECT ?s ?p ?o WHERE { ?s ?p ?o } ORDER BY ?s ?p ?o";
    let run = |lazy: bool, budget: Option<&str>| -> String {
        let mut cmd = common::rete();
        cmd.arg("sparql").arg(&fixture.rete).arg(q);
        if let Some(b) = budget {
            cmd.args(["--memory-budget-mb", b]);
        }
        if lazy {
            cmd.env("RETE_LOCAL_LAZY_ABOVE_MB", "0");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let eager = run(false, None);
    assert!(eager.contains("example.test/bob"), "{eager}");
    for (lazy, budget) in [
        (true, None),
        (true, Some("1")),
        (true, Some("0")),
        (false, Some("1")),
    ] {
        assert_eq!(run(lazy, budget), eager, "lazy={lazy} budget={budget:?}");
    }
}
