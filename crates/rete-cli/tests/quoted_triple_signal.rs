//! `signals.quoted_triples` — a `.rete` saying whether a consumer will meet
//! **triple terms** in it, and in which surface an export will spell them.
//!
//! The question matters before anyone downloads the file. A dump's quoted
//! triples can be written two ways and the two are not interchangeable
//! downstream: an RDF 1.2 parser rejects `<<s p o>>` in N-Quads and silently
//! reads it as a *reifier* in Turtle/TriG, and an RDF-star parser rejects
//! `<<( s p o )>>` everywhere. Which spelling a dump carries decides whether it
//! loads at all — so "does this dataset have them, and what will I get" belongs
//! in the card rather than in the reader's memory of a CLI flag.
//!
//! The property these tests exist for is the second one: the signal is
//! **derived from the header at read time**, exactly like `signals.permutations`.
//! `FLAG_HAS_QUOTED_TRIPLES` has been written by every build since quoted
//! triples were supported, so a file built long before this signal existed
//! answers it correctly today, with no re-card and no rebuild. A stored field
//! would read `null` on every published dataset, which is worse than none: it
//! would assert "unknown" about a file whose own header knows.

mod common;

use predicates::prelude::*;

const PLAIN: &str = concat!(
    "<http://ex/a> <http://ex/p> <http://ex/b> .\n",
    "<http://ex/b> <http://ex/p> <http://ex/c> .\n",
);

/// Object-position quoted triples — the shape RDF 1.2 can express, so the card
/// can honestly offer both surfaces.
const QUOTED: &str = concat!(
    "<http://ex/a> <http://ex/p> <http://ex/b> .\n",
    "<http://ex/claim> <http://ex/states> << <http://ex/a> <http://ex/p> <http://ex/b> >> .\n",
);

fn build(nq: &str, extra: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, nq).unwrap();
    let mut args = vec!["--no-pyramid"];
    args.extend_from_slice(extra);
    common::build(&src, &out, &args);
    (dir, out)
}

fn card_json(file: &std::path::Path) -> serde_json::Value {
    common::json(common::rete().arg("card").arg(file).arg("--json"))
}

fn card_text(file: &std::path::Path) -> String {
    let out = common::rete().arg("card").arg(file).output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap()
}

fn jsonld(file: &std::path::Path) -> serde_json::Value {
    common::json(
        common::rete()
            .arg("card")
            .arg(file)
            .args(["--format", "jsonld"]),
    )
}

// --- right in both directions ----------------------------------------------

#[test]
fn a_file_with_quoted_triples_says_so_and_names_both_surfaces() {
    let (_dir, file) = build(QUOTED, &["--card", "--title", "Annotated"]);
    let q = &card_json(&file)["signals"]["quoted_triples"];
    assert_eq!(q["present"], true);
    assert_eq!(
        q["export_surfaces"],
        serde_json::json!(["rdf12", "rdf-star"])
    );
    assert_eq!(q["export_default"], "rdf12");
}

#[test]
fn a_file_without_them_says_false_rather_than_nothing() {
    // `false` is a measurement, and a consumer branching on "will I meet triple
    // terms" needs it stated. It is the field's ABSENCE that means unknown.
    let (_dir, file) = build(PLAIN, &["--card", "--title", "Plain"]);
    let q = &card_json(&file)["signals"]["quoted_triples"];
    assert_eq!(q["present"], false);
    assert!(q["export_surfaces"].is_null(), "nothing to offer: {q}");
    assert!(q["export_default"].is_null(), "nothing to offer: {q}");
}

// --- the human card stays quiet about what a dataset does not have ----------

#[test]
fn the_human_card_names_it_only_when_there_is_something_to_say() {
    let (_dir, quoted) = build(QUOTED, &["--card", "--title", "Annotated"]);
    let text = card_text(&quoted);
    assert!(
        text.contains("quoted trip:") && text.contains("<<( s p o )>>"),
        "the card should say what an export will write:\n{text}"
    );
    assert!(
        text.contains("--quoted-triple-syntax rdf-star"),
        "…and how to ask for the other surface:\n{text}"
    );

    // Most datasets have none, and a line per affordance they do not use is
    // noise. The JSON and the JSON-LD still carry the explicit boolean.
    let (_dir2, plain) = build(PLAIN, &["--card", "--title", "Plain"]);
    let text = card_text(&plain);
    assert!(
        !text.contains("quoted trip"),
        "a card with no quoted triples should not grow a line:\n{text}"
    );
}

#[test]
fn a_cardless_file_still_answers_from_its_header() {
    // The #189 lesson, applied: a file with no card can still state what its
    // own header decides.
    let (_dir, file) = build(QUOTED, &[]);
    let text = card_text(&file);
    assert!(text.contains("no dataset card"), "{text}");
    assert!(
        text.contains("quoted triples present"),
        "a cardless file must still answer:\n{text}"
    );
}

// --- the JSON-LD projection -------------------------------------------------

#[test]
fn the_jsonld_projection_carries_the_boolean_and_the_surfaces() {
    let (_dir, file) = build(QUOTED, &["--card", "--title", "Annotated"]);
    let v = jsonld(&file);
    assert_eq!(v["rete:quotedTriples"], true);
    assert_eq!(
        v["rete:quotedTripleExportSurfaces"],
        serde_json::json!(["rdf12", "rdf-star"])
    );
    assert_eq!(v["rete:quotedTripleExportDefault"], "rdf12");

    // The boolean is written either way — a machine reading the projection has
    // to be able to see the negative without inferring it from a missing key.
    let (_dir2, plain) = build(PLAIN, &["--card", "--title", "Plain"]);
    let v = jsonld(&plain);
    assert_eq!(v["rete:quotedTriples"], false);
    assert!(v["rete:quotedTripleExportSurfaces"].is_null());
    assert!(v["rete:quotedTripleExportDefault"].is_null());
}

// --- derived, never stored --------------------------------------------------

#[test]
fn the_signal_is_not_in_the_files_own_bytes() {
    // The card's metadata section is hashed into the file's content hash. A
    // stored copy of this signal would be a claim about the file's own header,
    // which the header answers for free — and it would be `null` on every file
    // built before the signal existed, which is the failure this design avoids.
    let (_dir, file) = build(QUOTED, &["--card", "--title", "Annotated"]);
    let image = std::fs::read(&file).unwrap();
    let text = String::from_utf8_lossy(&image);
    assert!(
        !text.contains("quoted_triples"),
        "the signal reached the metadata section"
    );
    // …and it is still reported, because it is measured on the way out.
    assert_eq!(
        card_json(&file)["signals"]["quoted_triples"]["present"],
        true
    );
}

#[test]
fn a_card_file_cannot_author_it() {
    let (dir, _file) = build(PLAIN, &[]);
    let card_file = dir.path().join("card.json");
    std::fs::write(
        &card_file,
        r#"{"title":"Faked","signals":{"quoted_triples":{"present":true}}}"#,
    )
    .unwrap();
    let src = dir.path().join("in2.nq");
    std::fs::write(&src, PLAIN).unwrap();
    common::rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.path().join("faked.rete"))
        .args(["--no-pyramid", "--card-file"])
        .arg(&card_file)
        .assert()
        .failure()
        .stderr(predicate::str::contains("unknown field"));
}

#[test]
fn the_signal_survives_a_rebuild_of_the_same_graph() {
    // `rete export | rete build` keeps the quoted triples, so the flag — and
    // therefore the signal — has to come back. This is the drift case: a card
    // that said "yes" on a file that no longer has any would be exactly the
    // #189 shape.
    let (dir, file) = build(QUOTED, &["--card", "--title", "Annotated"]);
    let dump = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "nq"])
        .output()
        .unwrap();
    assert!(dump.status.success());
    let src = dir.path().join("round.nq");
    std::fs::write(&src, &dump.stdout).unwrap();
    let out = dir.path().join("round.rete");
    common::build(&src, &out, &["--no-pyramid", "--card", "--title", "Again"]);
    assert_eq!(
        card_json(&out)["signals"]["quoted_triples"]["present"],
        true
    );
}
