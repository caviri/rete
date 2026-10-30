//! `rete recard` — a new Dataset Card for a finished `.rete`, without a rebuild.
//!
//! The properties under test are the ones a published file depends on: every
//! section but the card and the build record comes through **byte for byte**,
//! the new content hash verifies, the publisher's curated fields survive
//! verbatim (or the command refuses), and the build record says where the file
//! came from.

mod common;

use predicates::prelude::*;

const CURATED: &str = r##"{
  "title": "Recard fixture",
  "license": "CC0-1.0",
  "description": ["# A fixture", "", "Prose the re-card must not touch — **stale figures included**: 12 statements."],
  "keywords": ["fixture", "recard"],
  "extra": {"examples": [{"query": "things", "words": ["thing", "number"]}]},
  "example_queries": ["SELECT ?s WHERE { ?s ?p ?o } LIMIT 3"]
}"##;

/// A carded file with a default graph, a named graph and a curated card.
fn carded() -> (common::Fixture, std::path::PathBuf) {
    let fixture = common::fixture();
    let mut nq = String::new();
    for i in 0..300 {
        nq.push_str(&format!(
            "<http://example.test/s{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
             <http://example.test/Thing> .\n\
             <http://example.test/s{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Thing \
             number {i}\"@en .\n\
             <http://example.test/s{i}> <http://example.test/next> <http://example.test/s{}> .\n\
             <http://example.test/s{i}> <http://example.test/seen> \"20{:02}-01-01\"^^\
             <http://www.w3.org/2001/XMLSchema#date> <http://example.test/g> .\n",
            (i + 1) % 300,
            i % 30
        ));
    }
    let source = fixture.write("graph.nq", nq);
    let card = fixture.write("card.json", CURATED);
    let out = fixture.path("graph.rete");
    common::build(&source, &out, &["--card-file", card.to_str().unwrap()]);
    (fixture, out)
}

/// `(kind, offset, length)` of every section, from the 1 KiB header.
fn sections(bytes: &[u8]) -> Vec<(u16, u64, u64)> {
    let n = u16::from_le_bytes([bytes[44], bytes[45]]) as usize;
    (0..n)
        .map(|i| {
            let at = 64 + i * 24;
            let kind = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
            let off = u64::from_le_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            let len = u64::from_le_bytes(bytes[at + 16..at + 24].try_into().unwrap());
            (kind, off, len)
        })
        .collect()
}

/// The file's content hash, hex, straight from header bytes 8..24 (`rete card
/// --json` carries no checksum).
fn content_hash(path: &std::path::Path) -> String {
    let bytes = std::fs::read(path).unwrap();
    bytes[8..24].iter().map(|b| format!("{b:02x}")).collect()
}

/// The data sections — everything but the card (1) and the build record (7) —
/// as `(kind, bytes)`, in directory order.
fn data_sections(bytes: &[u8]) -> Vec<(u16, Vec<u8>)> {
    sections(bytes)
        .into_iter()
        .filter(|(kind, _, len)| *kind != 1 && *kind != 7 && *len > 0)
        .map(|(kind, off, len)| (kind, bytes[off as usize..(off + len) as usize].to_vec()))
        .collect()
}

#[test]
fn a_recard_rewrites_only_the_card_and_keeps_the_prose() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--keep-curated", "--verify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("starter quer"));

    let before = std::fs::read(&src).unwrap();
    let after = std::fs::read(&out).unwrap();
    assert_eq!(
        data_sections(&before),
        data_sections(&after),
        "every section but the card and the build record is copied byte for byte"
    );
    common::rete().arg("verify").arg(&out).assert().success();

    let old = common::json(common::rete().arg("card").arg(&src).arg("--json"));
    let new = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    for field in [
        "title",
        "license",
        "description",
        "keywords",
        "extra",
        "example_queries",
    ] {
        assert_eq!(old[field], new[field], "{field} must travel verbatim");
    }
    assert_eq!(old["quad_count"], new["quad_count"]);
    assert_eq!(new["named_graph_count"], 1);
    assert!(!new["queries"].as_array().unwrap().is_empty());
    // The build record keeps the build's own provenance and says what
    // re-carded the file, from which content hash.
    let rec = &new["build"]["recarded"];
    assert_eq!(rec["from_content_hash"], content_hash(&src).as_str());
    assert_eq!(new["build"]["built_at"], old["build"]["built_at"]);
    assert!(new["build"]["query_costs"]["queries"].is_array());
    // And the data still answers, the same way.
    let q = "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }";
    let count = |f: &std::path::Path| {
        let out = common::rete().arg("sparql").arg(f).arg(q).output().unwrap();
        String::from_utf8(out.stdout).unwrap()
    };
    assert_eq!(count(&src), count(&out));
}

/// The card `rete recard` derives from the file is the card the build derived
/// from the input — on input with no duplicates and no multi-typed subjects —
/// so a re-card of a fresh build changes nothing but the build record.
#[test]
fn a_recard_of_a_fresh_build_derives_the_same_card() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("--keep-curated")
        .assert()
        .success();
    let strip = |mut v: serde_json::Value| {
        let o = v.as_object_mut().unwrap();
        o.remove("build");
        v
    };
    let old = strip(common::json(
        common::rete().arg("card").arg(&src).arg("--json"),
    ));
    let new = strip(common::json(
        common::rete().arg("card").arg(&out).arg("--json"),
    ));
    assert_eq!(old, new);
    // Same card bytes ⇒ same content hash: the card is the only hashed part
    // that could have changed.
    assert_eq!(content_hash(&src), content_hash(&out));
}

#[test]
fn a_recard_refuses_to_drop_curated_fields_or_overwrite() {
    let (fixture, src) = carded();
    let out = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .failure()
        .stderr(predicate::str::contains("curated fields"));
    assert!(!out.exists(), "nothing is written when the run is refused");

    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&src)
        .arg("--keep-curated")
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"));

    // `--card-file` replaces the curated half wholesale, as `build` takes it.
    let card = fixture.write("new-card.json", r#"{"title":"Replaced"}"#);
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .arg("--card-file")
        .arg(&card)
        .assert()
        .success();
    let new = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    assert_eq!(new["title"], "Replaced");
    assert!(new["description"].is_null());
}

/// The graph of [`carded`], as N-Quads, and its curated card document.
fn source() -> (common::Fixture, std::path::PathBuf, std::path::PathBuf) {
    let (fixture, _) = carded();
    let (src, card) = (fixture.path("graph.nq"), fixture.path("card.json"));
    (fixture, src, card)
}

/// The card section's raw bytes.
fn card_bytes(path: &std::path::Path) -> Vec<u8> {
    let bytes = std::fs::read(path).unwrap();
    let (_, off, len) = sections(&bytes)
        .into_iter()
        .find(|(kind, _, len)| *kind == 1 && *len > 0)
        .expect("a card");
    bytes[off as usize..(off + len) as usize].to_vec()
}

/// `(offset, length)` of a section kind, if present.
fn section(path: &std::path::Path, kind: u16) -> Option<(u64, u64)> {
    let bytes = std::fs::read(path).unwrap();
    sections(&bytes)
        .into_iter()
        .find(|(k, _, len)| *k == kind && *len > 0)
        .map(|(_, off, len)| (off, len))
}

/// The memory-bounded build writes the **full** card — profile and starter
/// queries — after the data, and it is the card `rete recard` derives from the
/// same file, byte for byte: both run one derivation. On input with no
/// duplicates and no multi-typed subjects it is also the in-RAM build's card,
/// so the two builds share their content hash.
#[test]
fn a_memory_bounded_build_writes_the_card_recard_writes() {
    let (fixture, src, card) = source();
    let ext = fixture.path("ext.rete");
    common::build(
        &src,
        &ext,
        &[
            "--memory-budget-mb",
            "64",
            "--card-file",
            card.to_str().unwrap(),
        ],
    );
    common::rete().arg("verify").arg(&ext).assert().success();
    let (card_off, card_len) = section(&ext, 1).unwrap();
    let (dict_off, _) = section(&ext, 2).unwrap();
    assert!(card_off > dict_off, "the card sits after the data");
    let (info_off, _) = section(&ext, 7).unwrap();
    assert_eq!(info_off, card_off + card_len, "the build record follows it");
    let full = common::json(common::rete().arg("card").arg(&ext).arg("--json"));
    assert!(!full["queries"].as_array().unwrap().is_empty(), "{full}");
    assert!(full["predicates"].as_array().is_some_and(|p| !p.is_empty()));
    assert_eq!(full["title"], "Recard fixture");
    let build = &full["build"];
    assert!(build["query_costs"]["queries"].is_array(), "{build}");
    assert_eq!(build["params"]["card_costs"], true);
    let phases = build["run"]["phases"].as_array().unwrap();
    let phase = |name: &str| phases.iter().find(|p| p["phase"] == name).cloned();
    for name in ["card_derive", "starter_queries", "content_hash"] {
        assert!(phase(name).is_some(), "{name} missing: {phases:?}");
    }
    if cfg!(target_os = "linux") {
        assert!(
            phase("card_derive").unwrap()["peak_rss_bytes"]
                .as_u64()
                .unwrap()
                > 0
        );
    }

    // `rete recard`, by copy and in place, from either curated source.
    let copied = fixture.path("copied.rete");
    common::rete()
        .arg("recard")
        .arg(&ext)
        .arg("-o")
        .arg(&copied)
        .arg("--keep-curated")
        .assert()
        .success();
    assert_eq!(card_bytes(&copied), card_bytes(&ext));
    assert_eq!(content_hash(&copied), content_hash(&ext));
    let in_place = fixture.path("in-place.rete");
    std::fs::copy(&ext, &in_place).unwrap();
    common::rete()
        .arg("recard")
        .arg(&in_place)
        .arg("--in-place")
        .arg("--card-file")
        .arg(&card)
        .assert()
        .success();
    assert_eq!(card_bytes(&in_place), card_bytes(&ext));
    assert_eq!(content_hash(&in_place), content_hash(&ext));

    // The in-RAM build of the same input: the same card, so the same hash —
    // wherever each file keeps it.
    let ram = fixture.path("ram.rete");
    common::build(
        &src,
        &ram,
        &["--no-pyramid", "--card-file", card.to_str().unwrap()],
    );
    assert_eq!(card_bytes(&ram), card_bytes(&ext));
    assert_eq!(content_hash(&ram), content_hash(&ext));
    assert_eq!(
        data_sections(&std::fs::read(&ram).unwrap()),
        data_sections(&std::fs::read(&ext).unwrap())
    );
}

/// `--no-card-costs` still works on the memory-bounded build: the card keeps
/// every generated starter query, unchecked, and the record has no costs.
#[test]
fn a_memory_bounded_build_honours_no_card_costs() {
    let (fixture, src, card) = source();
    let ext = fixture.path("ext.rete");
    common::rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&ext)
        .args(["--memory-budget-mb", "64", "--no-card-costs", "--card-file"])
        .arg(&card)
        .assert()
        .success()
        .stderr(predicate::str::contains("NOT run"));
    let full = common::json(common::rete().arg("card").arg(&ext).arg("--json"));
    assert!(!full["queries"].as_array().unwrap().is_empty());
    assert!(full["build"]["query_costs"].is_null());
    assert_eq!(full["build"]["params"]["card_costs"], false);
    common::rete().arg("verify").arg(&ext).assert().success();
}

/// `recard --in-place` leaves every byte of the old file where it was and
/// appends the new card, build record and footer; the file verifies, answers
/// the same, and its record says it was re-carded in place.
#[test]
fn an_in_place_recard_appends_and_moves_nothing() {
    let (fixture, src) = carded();
    let before = std::fs::read(&src).unwrap();
    common::rete()
        .arg("recard")
        .arg(&src)
        .args(["--in-place", "--keep-curated", "--verify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("dead byte"));
    let after = std::fs::read(&src).unwrap();
    assert_eq!(&after[1024..before.len()], &before[1024..], "nothing moved");
    assert_eq!(&after[after.len() - 4..], b"RETE");
    assert_eq!(data_sections(&before), data_sections(&after));
    common::rete().arg("verify").arg(&src).assert().success();
    let new = common::json(common::rete().arg("card").arg(&src).arg("--json"));
    assert_eq!(new["build"]["recarded"]["params"]["in_place"], true);
    assert_eq!(new["title"], "Recard fixture");

    // The copy of an in-place re-card drops the dead bytes again.
    let compact = fixture.path("compact.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&compact)
        .arg("--keep-curated")
        .assert()
        .success();
    assert!(std::fs::metadata(&compact).unwrap().len() < after.len() as u64);
    assert_eq!(card_bytes(&compact), card_bytes(&src));

    // Exactly one of -o and --in-place.
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("--keep-curated")
        .assert()
        .failure();
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(fixture.path("x.rete"))
        .args(["--in-place", "--keep-curated"])
        .assert()
        .failure();
}

/// Whether the card sits after every data section, with the build record
/// right behind it.
fn card_after_data(path: &std::path::Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    let all = sections(&bytes);
    let (_, card_off, card_len) = *all.iter().find(|(k, _, l)| *k == 1 && *l > 0).unwrap();
    let (_, info_off, _) = *all.iter().find(|(k, _, l)| *k == 7 && *l > 0).unwrap();
    let data_end = all
        .iter()
        .filter(|(k, _, l)| *k != 1 && *k != 7 && *l > 0)
        .map(|(_, off, len)| off + len)
        .max()
        .unwrap();
    card_off >= data_end && info_off == card_off + card_len
}

/// `rete merge` with a card writes the **full** card — the one `rete recard`
/// derives from the merged file, byte for byte — after the data, with a build
/// record that names the shards it folded; and the card changes nothing else:
/// the data sections are the cardless merge's, which are the memory-bounded
/// build's of the shards' union.
#[test]
fn a_merge_writes_the_card_recard_writes() {
    let (fixture, src, card) = source();
    // Three shards of the fixture graph, the lines dealt round-robin, and one
    // statement in two of them: the merge has a duplicate to drop.
    let text = std::fs::read_to_string(&src).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    let mut shards = vec![String::new(); 3];
    for (i, line) in lines.iter().enumerate() {
        shards[i % 3].push_str(line);
        shards[i % 3].push('\n');
    }
    shards[1].push_str(lines[0]);
    shards[1].push('\n');
    // The first shard has a curated card of its own, which a merge does not
    // carry.
    let shard_card = fixture.write("shard-card.json", r#"{"title":"Shard zero"}"#);
    let mut parts = Vec::new();
    for (i, nq) in shards.iter().enumerate() {
        let source = fixture.write(&format!("shard{i}.nq"), nq);
        let out = fixture.path(&format!("shard{i}.rete"));
        let carded = ["--card-file", shard_card.to_str().unwrap()];
        common::build(&source, &out, if i == 0 { &carded[..] } else { &[] });
        parts.push(out);
    }
    let merge = |out: &std::path::Path, extra: &[&str]| {
        common::rete()
            .arg("merge")
            .args(&parts)
            .arg("-o")
            .arg(out)
            .args(["--memory-budget-mb", "64"])
            .args(extra)
            .assert()
            .success()
    };

    let merged = fixture.path("merged.rete");
    merge(&merged, &["--card-file", card.to_str().unwrap()])
        .stderr(predicate::str::contains("shard0.rete carry curated fields"));
    common::rete().arg("verify").arg(&merged).assert().success();
    assert!(
        card_after_data(&merged),
        "the card and the record follow the data"
    );
    let full = common::json(common::rete().arg("card").arg(&merged).arg("--json"));
    assert!(!full["queries"].as_array().unwrap().is_empty(), "{full}");
    assert!(full["predicates"].as_array().is_some_and(|p| !p.is_empty()));
    assert_eq!(
        full["title"], "Recard fixture",
        "the curated half is --card-file's"
    );

    // The build record: the shards by name, size and BLAKE3, the flags, the
    // counts (the duplicate read twice, stored once) and every phase.
    let build = &full["build"];
    assert_eq!(build["params"]["command"], "merge");
    assert_eq!(build["params"]["card_costs"], true);
    assert_eq!(build["params"]["card_file"]["name"], "card.json");
    assert!(build["params"]["strict"].is_null(), "merge has no --strict");
    let inputs = build["inputs"].as_array().unwrap();
    assert_eq!(inputs.len(), 3);
    for (record, path) in inputs.iter().zip(&parts) {
        let bytes = std::fs::read(path).unwrap();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert_eq!(record["name"], name);
        assert_eq!(record["format"], "rete");
        assert_eq!(record["bytes"], bytes.len() as u64);
        assert_eq!(record["blake3"], blake3::hash(&bytes).to_hex().as_str());
    }
    let counts = &build["counts"];
    assert_eq!(counts["statements_read"], lines.len() as u64 + 1);
    assert_eq!(counts["statements_unique"], lines.len() as u64);
    assert_eq!(counts["duplicates_dropped"], 1);
    let phases: Vec<&str> = build["run"]["phases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases.first(), Some(&"read"), "{phases:?}");
    assert!(phases.contains(&"parse+chunk"), "{phases:?}");
    assert_eq!(
        &phases[phases.len() - 3..],
        ["card_derive", "starter_queries", "content_hash"],
        "{phases:?}"
    );
    assert!(build["query_costs"]["queries"].is_array(), "{build}");
    assert!(build["query_costs"]["context"]["transport"]
        .as_str()
        .unwrap()
        .starts_with("the file this merge wrote"));

    // `rete recard` of the merged file, by copy and in place: the same card,
    // so the same content hash.
    let copied = fixture.path("copied.rete");
    common::rete()
        .arg("recard")
        .arg(&merged)
        .arg("-o")
        .arg(&copied)
        .arg("--keep-curated")
        .assert()
        .success();
    assert_eq!(card_bytes(&copied), card_bytes(&merged));
    assert_eq!(content_hash(&copied), content_hash(&merged));
    let in_place = fixture.path("in-place.rete");
    std::fs::copy(&merged, &in_place).unwrap();
    common::rete()
        .arg("recard")
        .arg(&in_place)
        .arg("--in-place")
        .arg("--card-file")
        .arg(&card)
        .assert()
        .success();
    assert_eq!(card_bytes(&in_place), card_bytes(&merged));
    assert_eq!(content_hash(&in_place), content_hash(&merged));

    // The card changes nothing else: the data sections are the cardless
    // merge's, and those are the memory-bounded build's of the union — whose
    // card is the same bytes too.
    let data = |p: &std::path::Path| data_sections(&std::fs::read(p).unwrap());
    let cardless = fixture.path("cardless.rete");
    merge(&cardless, &[]).stderr(predicate::str::contains("has no card unless"));
    assert_eq!(section(&cardless, 1), None);
    assert_eq!(data(&cardless), data(&merged));
    let built = fixture.path("built.rete");
    common::build(
        &src,
        &built,
        &[
            "--memory-budget-mb",
            "64",
            "--card-file",
            card.to_str().unwrap(),
        ],
    );
    assert_eq!(data(&built), data(&merged));
    assert_eq!(card_bytes(&built), card_bytes(&merged));
    assert_eq!(content_hash(&built), content_hash(&merged));

    // `--no-card-costs`: the starter queries are kept unchecked, no costs.
    let unmeasured = fixture.path("unmeasured.rete");
    merge(&unmeasured, &["--no-card-costs", "--card"]).stderr(predicate::str::contains("NOT run"));
    let card = common::json(common::rete().arg("card").arg(&unmeasured).arg("--json"));
    assert!(!card["queries"].as_array().unwrap().is_empty());
    assert!(card["build"]["query_costs"].is_null());
    assert_eq!(card["build"]["params"]["card_costs"], false);
    assert!(
        card["title"].is_null(),
        "no curated fields: none carried from a shard"
    );
}
