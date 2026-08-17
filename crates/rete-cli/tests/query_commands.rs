mod common;

use predicates::prelude::*;
use rete_core::format::{Header, HEADER_LEN};

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

fn take_uvarint(bytes: &[u8], pos: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = bytes[*pos];
        *pos += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

fn corrupt_unused_ops_tile(image: &mut [u8]) {
    let header = Header::from_bytes(&image[..HEADER_LEN]).unwrap();
    let mut pos = usize::try_from(header.root_dir_offset).unwrap();
    assert_eq!(take_uvarint(image, &mut pos), 6);

    let ops = (0..6)
        .find_map(|section| {
            let len = take_uvarint(image, &mut pos);
            let start = pos;
            pos += len;
            (section == 5).then_some((start, len))
        })
        .unwrap();
    let mut dir = ops.0;
    let tiles = take_uvarint(image, &mut dir);
    assert!(tiles > 0);
    let mut compressed_lens = Vec::with_capacity(tiles);
    for _ in 0..tiles {
        let _min_delta = take_uvarint(image, &mut dir);
        let _leading_span = take_uvarint(image, &mut dir);
        compressed_lens.push(take_uvarint(image, &mut dir));
    }
    let first_tile_end = dir + compressed_lens[0];
    assert!(first_tile_end <= ops.0 + ops.1);
    image[dir..first_tile_end].fill(0xff);
}

#[test]
fn local_sparql_skips_an_unused_permutation_but_export_stays_eager() {
    let fixture = common::fixture();
    let mut image = std::fs::read(&fixture.rete).unwrap();
    corrupt_unused_ops_tile(&mut image);
    let corrupt = fixture.write("unused-ops-corrupt.rete", image);

    common::rete()
        .arg("sparql")
        .arg(&corrupt)
        .arg(SELECT)
        .arg("--json")
        .assert()
        .success()
        .stdout(predicate::str::contains("example.test/bob"));

    common::rete()
        .arg("export")
        .arg(&corrupt)
        .assert()
        .failure();
}
