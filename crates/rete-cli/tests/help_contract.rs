mod common;

use predicates::prelude::*;

#[test]
fn top_level_help_and_version_are_stable() {
    let output = common::rete().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    let commands = [
        "build",
        "validate",
        "info",
        "stats",
        "verify",
        "search",
        "card",
        "card-url",
        "graphs",
        "export",
        "repyramid",
        "query",
        "why",
        "summary",
        "communities",
        "predicates",
        "schema",
        "reach",
        "bgp",
        "reason",
        "shacl",
        "shacl-url",
        "cost",
        "progressive",
        "sparql",
        "serve",
        "cypher",
        "summary-url",
        "query-url",
        "federate",
        "sparql-url",
        "why-url",
        "manifest",
    ];
    let mut previous = 0;
    for command in commands {
        let needle = format!("\n  {command}");
        let position = help
            .find(&needle)
            .unwrap_or_else(|| panic!("missing command `{command}` in:\n{help}"));
        assert!(position >= previous, "`{command}` is out of enum order");
        previous = position;
    }

    common::rete()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("rete {}\n", env!("CARGO_PKG_VERSION")));
}

#[test]
fn release_critical_subcommand_help_keeps_required_flags() {
    for (command, required) in [
        ("build", "--output"),
        ("sparql", "--json"),
        ("serve", "--bind"),
        ("shacl", "--shapes"),
        ("reason", "--url"),
        ("federate", "--query"),
    ] {
        common::rete()
            .args([command, "--help"])
            .assert()
            .success()
            .stdout(predicate::str::contains(required));
    }
}

#[test]
fn sparql_url_help_distinguishes_full_transfer_from_lazy_parsing() {
    common::rete()
        .args(["sparql-url", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("one full-file transfer"))
        .stdout(predicate::str::contains("lazy ranged opener"))
        .stdout(predicate::str::contains("same adaptive transfer policy"));
}
