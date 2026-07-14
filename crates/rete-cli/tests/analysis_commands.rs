mod common;

use predicates::prelude::*;

#[test]
fn communities_reach_reason_and_shacl_cover_analysis_paths() {
    let fixture = common::fixture();
    let with_pyramid = fixture.path("pyramid.rete");
    common::build(&fixture.source, &with_pyramid, &[]);

    let communities = common::json(
        common::rete()
            .arg("communities")
            .arg(&with_pyramid)
            .arg("--json"),
    );
    assert_eq!(communities["schemaVersion"], 1);
    assert!(communities["communities"].is_array());

    common::rete()
        .arg("reach")
        .arg(&fixture.rete)
        .args([
            "--predicate",
            "<http://example.test/knows>",
            "--seed",
            "<http://example.test/alice>",
            "--count",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("1 node"));

    common::rete()
        .arg("reason")
        .arg(&fixture.rete)
        .arg("--check")
        .assert()
        .success()
        .stdout(predicate::str::contains("coherent"));

    let shapes = fixture.write("empty.ttl", "@prefix sh: <http://www.w3.org/ns/shacl#> .\n");
    let shacl = common::json(
        common::rete()
            .arg("shacl")
            .arg(&fixture.rete)
            .arg("--shapes")
            .arg(&shapes)
            .args(["--format", "json"]),
    );
    assert_eq!(shacl["schemaVersion"], 1);
    assert_eq!(shacl["conforms"], true);
    assert!(shacl["results"].is_array());

    common::rete()
        .arg("communities")
        .arg(&with_pyramid)
        .args(["--profile", "--predicate", "<http://example.test/knows>"])
        .assert()
        .success()
        .stdout(predicate::str::contains("community"));

    let seeds = fixture.write("seeds.txt", "<http://example.test/bob>\n");
    common::rete()
        .arg("reach")
        .arg(&fixture.rete)
        .arg("--predicate")
        .arg("<http://example.test/knows>")
        .arg("--seeds-file")
        .arg(&seeds)
        .args(["--reverse", "--parallel"])
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/alice"));

    common::rete()
        .arg("reason")
        .arg(&fixture.rete)
        .args(["--materialize", "--format", "ttl"])
        .assert()
        .success()
        .stdout(predicate::str::contains("<http://example.test/alice>"));

    let stamped = fixture.path("stamped.rete");
    common::build(&fixture.source, &stamped, &["--no-pyramid", "--reason"]);
    common::rete()
        .arg("reason")
        .arg(&stamped)
        .arg("--verify-card")
        .assert()
        .success()
        .stdout(predicate::str::contains("verified"));
}
