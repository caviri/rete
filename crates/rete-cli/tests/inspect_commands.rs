mod common;

use predicates::prelude::*;

#[test]
fn inspect_commands_report_the_fixture() {
    let fixture = common::fixture();
    common::rete()
        .arg("info")
        .arg(&fixture.rete)
        .assert()
        .success()
        .stdout(predicate::str::contains("version"));
    common::rete()
        .arg("stats")
        .arg(&fixture.rete)
        .assert()
        .success()
        .stdout(predicate::str::contains("triples"));
    common::rete()
        .arg("graphs")
        .arg(&fixture.rete)
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/people"));
    common::rete()
        .arg("verify")
        .arg(&fixture.rete)
        .assert()
        .success()
        .stdout(predicate::str::contains("content hash matches"));
}

#[test]
fn rete_specific_inspection_json_has_a_versioned_envelope() {
    let fixture = common::fixture();
    let card_file = fixture.path("card.rete");
    common::build(
        &fixture.source,
        &card_file,
        &["--no-pyramid", "--card", "--title", "Release fixture"],
    );

    let card = common::json(common::rete().arg("card").arg(&card_file).arg("--json"));
    assert_eq!(card["schemaVersion"], 1);
    assert_eq!(card["title"], "Release fixture");
    assert!(card["triple_count"].is_number());

    let why = common::json(common::rete().arg("why").arg(&fixture.rete).args([
        "--predicate",
        "<http://example.test/knows>",
        "--json",
    ]));
    assert_eq!(why["schemaVersion"], 1);
    assert_eq!(why["result_count"], 1);
    assert!(why["results"].is_array());

    let search = common::json(
        common::rete()
            .arg("search")
            .arg(&fixture.rete)
            .arg("--json"),
    );
    assert_eq!(search["schemaVersion"], 1);
    assert!(search["matches"].is_array());
}

#[test]
fn summary_schema_search_and_repyramid_paths_are_covered() {
    let fixture = common::fixture();
    let typed_source = fixture.write(
        "typed.nt",
        concat!(
            "<urn:alice> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .\n",
            "<urn:alice> <http://www.w3.org/2000/01/rdf-schema#label> \"Alice Example\" .\n",
            "<urn:alice> <urn:note> \"glucose metabolism\" .\n",
            "<urn:bob> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:Person> .\n",
            "<urn:alice> <urn:knows> <urn:bob> .\n",
        ),
    );
    let typed = fixture.path("typed.rete");
    common::build(
        &typed_source,
        &typed,
        &["--text-index", "--card", "--title", "Typed fixture"],
    );

    for (command, expected) in [
        ("summary", "pyramid round"),
        ("predicates", "urn:knows"),
        ("schema", "urn:Person"),
    ] {
        common::rete()
            .arg(command)
            .arg(&typed)
            .assert()
            .success()
            .stdout(predicate::str::contains(expected));
    }
    common::rete()
        .arg("summary")
        .arg(&typed)
        .args(["--level", "0"])
        .assert()
        .success()
        .stdout(predicate::str::contains("urn:Person"));
    common::rete()
        .arg("card")
        .arg(&typed)
        .assert()
        .success()
        .stdout(predicate::str::contains("Typed fixture"));

    let labels = common::json(
        common::rete()
            .arg("search")
            .arg(&typed)
            .arg("ali")
            .arg("--json"),
    );
    assert_eq!(labels["matches"].as_array().unwrap().len(), 1);
    let text = common::json(common::rete().arg("search").arg(&typed).args([
        "--contains",
        "glucose",
        "--json",
    ]));
    assert_eq!(text["matches"].as_array().unwrap().len(), 1);

    let repyramid = fixture.path("repyramid.rete");
    common::rete()
        .arg("repyramid")
        .arg(&fixture.rete)
        .arg("-o")
        .arg(&repyramid)
        .assert()
        .success();
    common::rete()
        .arg("summary")
        .arg(&repyramid)
        .assert()
        .success();
}
