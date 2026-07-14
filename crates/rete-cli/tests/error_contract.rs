mod common;

use predicates::prelude::*;

#[test]
fn clap_usage_errors_exit_two() {
    common::rete()
        .arg("build")
        .assert()
        .code(2)
        .stderr(predicate::str::contains("Usage:"));
}

#[test]
fn malformed_or_incompatible_data_exits_one() {
    let fixture = common::fixture();
    let malformed = fixture.write("malformed.nt", "not RDF\n");
    common::rete()
        .arg("validate")
        .arg(&malformed)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Error"));

    let corrupt = fixture.write("corrupt.rete", b"RETE");
    common::rete()
        .arg("info")
        .arg(&corrupt)
        .assert()
        .code(1)
        .stderr(predicate::str::contains("Error"));

    let mut old = std::fs::read(&fixture.rete).unwrap();
    old[4] = 0x04;
    let old = fixture.write("pre-v1.rete", old);
    common::rete()
        .arg("info")
        .arg(&old)
        .assert()
        .code(1)
        .stderr(
            predicate::str::contains("unsupported .rete format")
                .and(predicate::str::contains("rebuilt")),
        );
}

#[test]
fn completed_validation_or_reasoning_nonconformance_exits_three() {
    let fixture = common::fixture();
    let shapes = fixture.write(
        "missing-name.ttl",
        r#"@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.test/> .
[] a sh:NodeShape ;
   sh:targetNode ex:alice ;
   sh:property [ sh:path ex:missing ; sh:minCount 1 ] .
"#,
    );
    common::rete()
        .arg("shacl")
        .arg(&fixture.rete)
        .arg("--shapes")
        .arg(&shapes)
        .args(["--format", "json"])
        .assert()
        .code(3)
        .stdout(predicate::str::contains("\"conforms\": false"));

    let incoherent_source = fixture.write(
        "incoherent.nt",
        concat!(
            "<urn:A> <http://www.w3.org/2002/07/owl#disjointWith> <urn:B> .\n",
            "<urn:x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:A> .\n",
            "<urn:x> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:B> .\n",
        ),
    );
    let incoherent = fixture.path("incoherent.rete");
    common::build(&incoherent_source, &incoherent, &["--no-pyramid"]);
    common::rete()
        .arg("reason")
        .arg(&incoherent)
        .arg("--check")
        .assert()
        .code(3)
        .stdout(predicate::str::contains("incoherent"));
}
