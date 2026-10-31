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

/// The 0x06 dictionary data section, as [`sections`] numbers it.
const DICT_DATA: u16 = 100 + rete_core::v06::kind::DICT_DATA as u16;

/// `(kind, offset, length)` of every section. A 0x05 file: from its 1 KiB
/// header. A 0x06 file: the card (1) and the build record (7) in the card
/// region, then every section of the root as `100 + its kind`.
fn sections(bytes: &[u8]) -> Vec<(u16, u64, u64)> {
    if rete_core::v06::open::is_v06(bytes) {
        let h = rete_core::v06::Header06::from_bytes(bytes).unwrap();
        let (_, root) =
            rete_core::v06::open::read_front(&rete_core::SliceReader::new(bytes)).unwrap();
        let (c, b) = (h.card_range(), h.build_range());
        let mut all = vec![(1, c.offset, c.len), (7, b.offset, b.len)];
        all.extend(root.sections.iter().map(|x| {
            let r = x.abs(&h);
            (100 + x.kind as u16, r.offset, r.len)
        }));
        return all;
    }
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
/// queries — in the card region, and it is the card `rete recard` derives from the
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
    let (dict_off, _) = section(&ext, DICT_DATA).unwrap();
    assert!(
        card_off < dict_off,
        "the card region sits in front of the data"
    );
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

/// `recard --in-place` writes the new card and build record into the free
/// part of the card region, beside the old pair, then switches the 128-byte
/// header: nothing is appended and no byte outside the card region moves. The
/// file verifies, answers the same, and its record says it was re-carded in
/// place.
#[test]
fn an_in_place_recard_writes_the_card_region_and_moves_nothing() {
    let (fixture, src) = carded();
    let before = std::fs::read(&src).unwrap();
    common::rete()
        .arg("recard")
        .arg(&src)
        .args(["--in-place", "--keep-curated", "--verify"])
        .assert()
        .success()
        .stdout(predicate::str::contains("no data byte moved"));
    let after = std::fs::read(&src).unwrap();
    assert_eq!(after.len(), before.len(), "nothing appended");
    let h = rete_core::v06::Header06::from_bytes(&after).unwrap();
    let region = h.card_region_offset() as usize;
    let front = region + h.card_region_len as usize;
    assert_eq!(&after[128..region], &before[128..region], "the root stays");
    assert_eq!(
        &after[front..],
        &before[front..],
        "nothing outside the card region moved"
    );
    assert_eq!(&after[after.len() - 4..], b"RETE");
    assert_eq!(data_sections(&before), data_sections(&after));
    common::rete().arg("verify").arg(&src).assert().success();
    let new = common::json(common::rete().arg("card").arg(&src).arg("--json"));
    assert_eq!(new["build"]["recarded"]["params"]["in_place"], true);
    assert_eq!(new["title"], "Recard fixture");

    // A copy re-lays the card region out around the new pair.
    let compact = fixture.path("compact.rete");
    common::rete()
        .arg("recard")
        .arg(&src)
        .arg("-o")
        .arg(&compact)
        .arg("--keep-curated")
        .assert()
        .success();
    assert_eq!(card_bytes(&compact), card_bytes(&src));
    assert_eq!(content_hash(&compact), content_hash(&src));

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

/// Whether the card sits in front of every section (the 0x06 card region),
/// with the build record right behind it.
fn card_in_front(path: &std::path::Path) -> bool {
    let bytes = std::fs::read(path).unwrap();
    let all = sections(&bytes);
    let (_, card_off, card_len) = *all.iter().find(|(k, _, l)| *k == 1 && *l > 0).unwrap();
    let (_, info_off, info_len) = *all.iter().find(|(k, _, l)| *k == 7 && *l > 0).unwrap();
    let data_start = all
        .iter()
        .filter(|(k, _, l)| *k != 1 && *k != 7 && *l > 0)
        .map(|(_, off, _)| *off)
        .min()
        .unwrap();
    info_off + info_len <= data_start && info_off == card_off + card_len
}

/// `rete merge` with a card writes the **full** card — the one `rete recard`
/// derives from the merged file, byte for byte — in the card region, with a build
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
        card_in_front(&merged),
        "the card and the record sit in the card region, in front of the data"
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

/// A graph built to make the card derivations disagree if they could (G17):
/// every subject is typed, a third of them twice — the type written LAST is
/// the lexicographically smaller class, so "last type in input order" and
/// "highest dictionary id" pick different classes — some statements appear
/// twice, the labels feed a text index, two named graphs carry quads, and the
/// links fan out into more than `CARD_TOP_N` (100) class links, most of them
/// tied on their count, so the top-100 cut is decided by the tie-break.
fn g17_graph() -> String {
    const SUBJECTS: usize = 600;
    const CLASSES: usize = 150;
    let ex = "http://example.test";
    let mut nq = String::new();
    for i in 0..SUBJECTS {
        let s = format!("<{ex}/s{i:04}>");
        let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let class = |c: usize| format!("<{ex}/C{:03}>", c % CLASSES);
        if i % 3 == 0 {
            // The larger class first, the smaller one last.
            nq.push_str(&format!("{s} {ty} {} .\n", class(i + 1)));
        }
        nq.push_str(&format!("{s} {ty} {} .\n", class(i)));
        nq.push_str(&format!(
            "{s} <http://www.w3.org/2000/01/rdf-schema#label> \"subject {i} about topic{} and theme{}\"@en .\n",
            i % 40,
            i % 7
        ));
        for k in 0..3 {
            let line = format!(
                "{s} <{ex}/p{k}> <{ex}/s{:04}> .\n",
                (i * 37 + k * 101 + 1) % SUBJECTS
            );
            nq.push_str(&line);
            if (i + k) % 11 == 0 {
                nq.push_str(&line); // a duplicate statement
            }
        }
        if i % 5 == 0 {
            nq.push_str(&format!(
                "{s} <{ex}/seen> \"20{:02}-01-01\"^^<http://www.w3.org/2001/XMLSchema#date> <{ex}/g{}> .\n",
                i % 30,
                i % 2
            ));
        }
    }
    nq
}

/// The memory-bounded build's card and `rete recard`'s are one derivation
/// (#356), so they must be the same bytes on the graph above, where the
/// in-RAM derivation is not: the in-RAM card counts the input multiset and
/// classifies a multi-typed subject by its last type in input order, while
/// both streaming paths read the finished file (deduplicated, index order).
/// That difference is documented (docs/dataset-cards.md, "Maintaining a
/// published card"), and this pins all three facts: the two streaming cards
/// agree byte for byte; the in-RAM card differs in its class links; and
/// re-carding the in-RAM file gives the streaming card, because the data
/// sections — the text index included (G16) — are identical, so the
/// measured text-index signal (`token_table_bytes`) agrees everywhere.
#[test]
fn a_bounded_build_and_a_recard_write_the_same_card_where_in_ram_differs() {
    let fixture = common::fixture();
    let src = fixture.write("g17.nq", g17_graph());
    let card = fixture.write("card.json", CURATED);
    let card_arg = card.to_str().unwrap();

    let ext = fixture.path("ext.rete");
    common::build(
        &src,
        &ext,
        &[
            "--memory-budget-mb",
            "64",
            "--text-index",
            "--card-file",
            card_arg,
        ],
    );
    let recarded = fixture.path("recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&ext)
        .arg("-o")
        .arg(&recarded)
        .arg("--keep-curated")
        .assert()
        .success();
    assert_eq!(
        card_bytes(&recarded),
        card_bytes(&ext),
        "recard = bounded build"
    );
    assert_eq!(content_hash(&recarded), content_hash(&ext));

    let without_build = |p: &std::path::Path| {
        let mut v = common::json(common::rete().arg("card").arg(p).arg("--json"));
        v.as_object_mut().unwrap().remove("build");
        v
    };
    let streamed = without_build(&ext);
    assert_eq!(streamed, without_build(&recarded));
    let links = streamed["class_links"].as_array().unwrap();
    assert_eq!(links.len(), 100, "the top-100 cut is exercised");
    assert_eq!(streamed["truncated"], true);
    let token_table =
        |c: &serde_json::Value| c["signals"]["text_index"]["token_table_bytes"].clone();
    assert!(
        token_table(&streamed).as_u64().is_some_and(|n| n > 0),
        "{streamed}"
    );

    // The in-RAM build of the same input: the same data, a different card.
    let ram = fixture.path("ram.rete");
    common::build(
        &src,
        &ram,
        &["--no-pyramid", "--text-index", "--card-file", card_arg],
    );
    assert_eq!(
        data_sections(&std::fs::read(&ram).unwrap()),
        data_sections(&std::fs::read(&ext).unwrap()),
        "the in-RAM and memory-bounded builds write the same data sections"
    );
    let in_ram = without_build(&ram);
    assert_eq!(token_table(&in_ram), token_table(&streamed));
    assert_ne!(
        in_ram["class_links"], streamed["class_links"],
        "the in-RAM card no longer differs: update docs/dataset-cards.md, which says it does"
    );
    // …and only because of what it derived from: re-carding the in-RAM file
    // gives the streaming card, byte for byte.
    let ram_recarded = fixture.path("ram-recarded.rete");
    common::rete()
        .arg("recard")
        .arg(&ram)
        .arg("-o")
        .arg(&ram_recarded)
        .arg("--keep-curated")
        .assert()
        .success();
    assert_eq!(card_bytes(&ram_recarded), card_bytes(&ext));
}
