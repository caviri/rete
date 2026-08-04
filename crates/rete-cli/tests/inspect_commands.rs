mod common;

use predicates::prelude::*;

/// The curated discovery fields (`keywords`, `theme`) round-trip
/// `build --card-file` → every read surface, canonicalized
/// (trimmed/sorted/deduped) on the way in, and projected under their
/// standard terms — `schema:keywords` + `dcat:keyword` and `@id`-typed
/// `dcat:theme` in JSON-LD; keywords in Croissant's schema.org header. The
/// official-field contrast to the `extra` bag's opaque `rete:extra/<key>`
/// treatment below.
#[test]
fn curated_discovery_fields_round_trip_across_formats() {
    let fixture = common::fixture();
    let card_json = fixture.write(
        "kw-card.json",
        r#"{"title":"Keyword fixture",
            "keywords":["open data"," catalog ","open data"],
            "theme":["http://publications.europa.eu/resource/authority/data-theme/GOVE"]}"#,
    );
    let out = fixture.path("kw.rete");
    common::build(
        &fixture.source,
        &out,
        &[
            "--no-pyramid",
            "--no-card-costs",
            "--card-file",
            card_json.to_str().unwrap(),
        ],
    );

    // --json: the canonical (trimmed, sorted, deduped) lists.
    let card = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    assert_eq!(
        card["keywords"],
        serde_json::json!(["catalog", "open data"])
    );
    assert_eq!(
        card["theme"],
        serde_json::json!(["http://publications.europa.eu/resource/authority/data-theme/GOVE"])
    );

    // The text catalog renders them.
    common::rete()
        .arg("card")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "keywords     : catalog, open data",
        ))
        .stdout(predicate::str::contains(
            "theme        : http://publications.europa.eu/resource/authority/data-theme/GOVE",
        ));

    // JSON-LD: the standard terms, not rete:-invented ones.
    let ld = common::json(
        common::rete()
            .arg("card")
            .arg(&out)
            .args(["--format", "jsonld"]),
    );
    assert_eq!(ld["keywords"], serde_json::json!(["catalog", "open data"]));
    assert_eq!(
        ld["dcat:keyword"],
        serde_json::json!(["catalog", "open data"])
    );
    assert_eq!(
        ld["dcat:theme"],
        serde_json::json!(["http://publications.europa.eu/resource/authority/data-theme/GOVE"])
    );
    assert!(ld.get("rete:extra/keywords").is_none());

    // Croissant: keywords belong to its schema.org header (the bag and the
    // DCAT-only theme do not).
    let cr = common::json(
        common::rete()
            .arg("card")
            .arg(&out)
            .args(["--format", "croissant"]),
    );
    assert_eq!(cr["keywords"], serde_json::json!(["catalog", "open data"]));
    assert!(cr.get("dcat:theme").is_none());

    // A keyword that is empty after trimming fails the build loudly, and so
    // does a free-text theme (which belongs in `keywords`).
    let bad = fixture.write("kw-bad.json", r#"{"keywords":["ok","  "]}"#);
    common::rete()
        .arg("build")
        .arg(&fixture.source)
        .arg("-o")
        .arg(fixture.path("kw-bad.rete"))
        .args(["--card-file", bad.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("keywords"));
    let bad_theme = fixture.write("theme-bad.json", r#"{"theme":["government"]}"#);
    common::rete()
        .arg("build")
        .arg(&fixture.source)
        .arg("-o")
        .arg(fixture.path("theme-bad.rete"))
        .args(["--card-file", bad_theme.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not an IRI"));
}

/// Custom fields (`extra`) round-trip `build --card-file` → `card --json`,
/// and every projection applies its stated policy: the text view lists them,
/// JSON-LD emits per-key `rete:extra/<key>` opaque values, Croissant omits
/// them.
#[test]
fn custom_extra_fields_round_trip_across_formats() {
    let fixture = common::fixture();
    let card_json = fixture.write(
        "card.json",
        r#"{"title":"Extra fixture","extra":{"zeta":1,"alpha":{"nested":true},"owner":"dg"}}"#,
    );
    let out = fixture.path("extra.rete");
    common::build(
        &fixture.source,
        &out,
        &[
            "--no-pyramid",
            "--no-card-costs",
            "--card-file",
            card_json.to_str().unwrap(),
        ],
    );

    // --json carries the bag verbatim.
    let card = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    assert_eq!(card["extra"]["owner"], "dg");
    assert_eq!(card["extra"]["alpha"]["nested"], true);
    assert_eq!(card["extra"]["zeta"], 1);

    // The text catalog lists them.
    common::rete()
        .arg("card")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("custom fields (3):"))
        .stdout(predicate::str::contains("owner = \"dg\""))
        .stdout(predicate::str::contains("zeta = 1"));

    // JSON-LD: per-key opaque values — a scalar plainly, a container as an
    // @json-typed JSON literal.
    let ld = common::json(
        common::rete()
            .arg("card")
            .arg(&out)
            .args(["--format", "jsonld"]),
    );
    assert_eq!(ld["rete:extra/owner"], "dg");
    assert_eq!(ld["rete:extra/zeta"], 1);
    assert_eq!(ld["rete:extra/alpha"]["nested"], true);
    assert_eq!(ld["@context"]["rete:extra/alpha"]["@type"], "@json");
    assert!(ld.get("owner").is_none(), "no bare top-level property");

    // Croissant: custom fields are omitted entirely.
    let cr = common::json(
        common::rete()
            .arg("card")
            .arg(&out)
            .args(["--format", "croissant"]),
    );
    assert!(cr.get("rete:extra/owner").is_none());
    assert!(cr.get("extra").is_none());
}

/// Overflowing the `extra` byte cap rejects the build loudly (one byte over),
/// and a publisher key at the card file's top level is rejected with a
/// pointer to the bag — never silently dropped.
#[test]
fn oversized_or_top_level_custom_fields_fail_the_build_loudly() {
    let fixture = common::fixture();
    // `{"pad":"…"}` serializes to 10 + n bytes; 8,183 x's = 8,193 B — one over.
    let over = format!(r#"{{"extra":{{"pad":"{}"}}}}"#, "x".repeat(8183));
    let card_json = fixture.write("over.json", over);
    common::rete()
        .arg("build")
        .arg(&fixture.source)
        .arg("-o")
        .arg(fixture.path("no.rete"))
        .args(["--card-file", card_json.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("8193 bytes"));

    let stray = fixture.write("stray.json", r#"{"title":"T","my_field":1}"#);
    common::rete()
        .arg("build")
        .arg(&fixture.source)
        .arg("-o")
        .arg(fixture.path("no2.rete"))
        .args(["--card-file", stray.to_str().unwrap()])
        .assert()
        .failure()
        .stderr(predicate::str::contains("my_field"))
        .stderr(predicate::str::contains("\"extra\""));
}

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
