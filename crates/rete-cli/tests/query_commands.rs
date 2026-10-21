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

/// `COUNT(DISTINCT …)` spills past an eighth of the budget and stays exact:
/// the tiniest budget and an unlimited one count the same.
#[test]
fn count_distinct_is_exact_at_any_budget() {
    let fixture = common::fixture();
    let q = "SELECT (COUNT(DISTINCT ?s) AS ?a) (COUNT(DISTINCT ?o) AS ?b) WHERE { ?s ?p ?o }";
    let run = |budget: &str| -> String {
        let out = common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .arg(q)
            .args(["--memory-budget-mb", budget])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    let unlimited = run("0");
    assert!(unlimited.contains("?a="), "{unlimited}");
    assert_eq!(run("1"), unlimited);
}

/// `--query-file <path>` (`-` = standard input) feeds every query command, so a
/// query too long for the command line still runs: Linux caps one argument at
/// 128 KiB and Windows the whole command line at 32,767 characters, and a
/// VALUES block of a few thousand IRIs crosses both (OpenAIRE QA, E5).
#[test]
fn query_file_and_stdin_feed_every_query_command() {
    let fixture = common::fixture();
    let inline = common::json(
        common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .arg(SELECT)
            .arg("--json"),
    );
    let qfile = fixture.write("select.rq", SELECT);
    let from_file = common::json(
        common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .arg("--query-file")
            .arg(&qfile)
            .arg("--json"),
    );
    assert_eq!(from_file, inline);
    let from_stdin = common::json(
        common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .args(["--query-file", "-", "--json"])
            .write_stdin(SELECT),
    );
    assert_eq!(from_stdin, inline);

    // A 20,000-IRI VALUES block, ~0.6 MB: far past one argument's limit.
    let values: Vec<String> = (0..20_000)
        .map(|i| format!("<http://example.test/p{i}>"))
        .chain(["<http://example.test/alice>".to_string()])
        .collect();
    let big = format!(
        "SELECT ?s ?o WHERE {{ VALUES ?s {{ {} }} ?s <http://example.test/knows> ?o }}",
        values.join(" ")
    );
    assert!(big.len() > 128 * 1024);
    let bigfile = fixture.write("big.rq", &big);
    let rows = common::json(
        common::rete()
            .arg("sparql")
            .arg(&fixture.rete)
            .arg("--query-file")
            .arg(&bigfile)
            .arg("--json"),
    );
    let bindings = rows["results"]["bindings"].as_array().unwrap();
    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0]["o"]["value"], "http://example.test/bob");

    // The other query commands take it too.
    let federated = common::json(
        common::rete()
            .arg("federate")
            .arg(&fixture.rete)
            .arg("--query-file")
            .arg(&qfile)
            .arg("--json"),
    );
    assert_eq!(federated["results"], inline["results"]);
    let cost = common::json(
        common::rete()
            .arg("cost")
            .arg(&fixture.rete)
            .arg("--query-file")
            .arg(&qfile)
            .arg("--json"),
    );
    assert_eq!(cost["schemaVersion"], 1);
    let with_pyramid = fixture.path("pyramid.rete");
    common::build(&fixture.source, &with_pyramid, &[]);
    let count = fixture.write("count.rq", "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }");
    let progressive = common::json(
        common::rete()
            .arg("progressive")
            .arg(&with_pyramid)
            .arg("--query-file")
            .arg(&count)
            .arg("--json"),
    );
    assert_eq!(progressive["schemaVersion"], 1);
    let cypher = fixture.write("q.cypher", "MATCH (s) RETURN s LIMIT 1");
    let cypher = common::json(
        common::rete()
            .arg("cypher")
            .arg(&fixture.rete)
            .arg("--query-file")
            .arg(&cypher)
            .arg("--json"),
    );
    assert!(cypher["results"]["bindings"].is_array());
}

#[test]
fn query_file_is_exclusive_and_required_and_reports_bad_files() {
    let fixture = common::fixture();
    let qfile = fixture.write("select.rq", SELECT);
    // Both the argument and the file: a usage error (exit 2).
    common::rete()
        .arg("sparql")
        .arg(&fixture.rete)
        .arg(SELECT)
        .arg("--query-file")
        .arg(&qfile)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
    common::rete()
        .arg("federate")
        .arg(&fixture.rete)
        .args(["--query", SELECT])
        .arg("--query-file")
        .arg(&qfile)
        .assert()
        .code(2);
    // Neither: a usage error naming the missing argument.
    common::rete()
        .arg("sparql")
        .arg(&fixture.rete)
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--query-file"));
    common::rete()
        .arg("federate")
        .arg(&fixture.rete)
        .assert()
        .code(2);
    // `cost --dump` previews a dump: a query file makes no sense there.
    common::rete()
        .arg("cost")
        .arg(&fixture.rete)
        .arg("--dump")
        .arg("--query-file")
        .arg(&qfile)
        .assert()
        .code(2);
    // A file that cannot be read, or holds no query: a runtime error (exit 1).
    common::rete()
        .arg("sparql")
        .arg(&fixture.rete)
        .arg("--query-file")
        .arg(fixture.path("missing.rq"))
        .assert()
        .code(1)
        .stderr(predicate::str::contains("cannot read --query-file"));
    let empty = fixture.write("empty.rq", "\n");
    common::rete()
        .arg("sparql")
        .arg(&fixture.rete)
        .arg("--query-file")
        .arg(&empty)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("is empty"));
    common::rete()
        .arg("sparql")
        .arg(&fixture.rete)
        .args(["--query-file", "-"])
        .write_stdin("")
        .assert()
        .code(1)
        .stderr(predicate::str::contains("standard input is empty"));
}
