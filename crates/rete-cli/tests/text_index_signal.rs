//! `signals.text_index` — a `.rete` describing its own full-text index.
//!
//! A TEXT_INDEX section (kind 6) is opt-in at build time, and a file that has
//! one answers `FILTER(CONTAINS(…))` by word lookup while a file that does not
//! answers the *same query with the same rows* by full scan. The capability is
//! therefore invisible from the results — which is how the playground catalog
//! came to advertise an index two published files never carried (#189).
//!
//! The fix (#190) is a **measured** card signal: derived from the file's section
//! directory at read time, never authored and never stored. These tests pin the
//! three properties that makes it worth having — it is right in both directions,
//! it stays right when the file is rebuilt, and it costs a header read rather
//! than the index it describes.

mod common;

use predicates::prelude::*;

/// A source with enough distinct words that its text index is worth kilobytes —
/// so "the reader did not fetch the index" is an assertion with teeth.
fn wordy_source() -> String {
    let mut nt = String::new();
    for i in 0..400 {
        nt.push_str(&format!(
            "<http://example.test/e{i}> <http://example.test/label> \
             \"entity {i} alpha{i} beta{i} gamma{i} delta{i} epsilon{i}\" .\n"
        ));
    }
    nt
}

fn signals(card: &serde_json::Value) -> &serde_json::Value {
    &card["signals"]["text_index"]
}

/// `(offset, length)` of a section, read straight out of the 1 KiB header's
/// typed section directory (SPEC §4.1: `section_count` is a u16 at 44, entries
/// of 24 bytes from offset 64 — `kind`/`flags`/reserved, then `offset`/
/// `length`). Deliberately hand-rolled rather than going through `rete-core`:
/// these tests are about the BYTES the writer produced.
fn section(image: &[u8], kind: u16) -> Option<(usize, usize)> {
    let u64_at = |o: usize| u64::from_le_bytes(image[o..o + 8].try_into().unwrap()) as usize;
    let count = u16::from_le_bytes(image[44..46].try_into().unwrap()) as usize;
    (0..count).find_map(|i| {
        let p = 64 + i * 24;
        (u16::from_le_bytes(image[p..p + 2].try_into().unwrap()) == kind)
            .then(|| (u64_at(p + 8), u64_at(p + 16)))
    })
}

#[test]
fn the_card_reports_the_text_index_in_both_directions() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source());
    let plain = fixture.path("plain.rete");
    let indexed = fixture.path("indexed.rete");
    common::build(&source, &plain, &["--card", "--title", "Plain"]);
    common::build(
        &source,
        &indexed,
        &["--card", "--title", "Indexed", "--text-index"],
    );

    let no = common::json(common::rete().arg("card").arg(&plain).arg("--json"));
    assert_eq!(
        signals(&no)["present"],
        serde_json::json!(false),
        "a build without --text-index must say so, not stay silent: {}",
        signals(&no)
    );
    // `false` is measured, so nothing else is claimed alongside it.
    assert!(signals(&no)["bytes"].is_null());
    assert!(signals(&no)["token_table_bytes"].is_null());

    let yes = common::json(common::rete().arg("card").arg(&indexed).arg("--json"));
    assert_eq!(signals(&yes)["present"], serde_json::json!(true));
    let bytes = signals(&yes)["bytes"].as_u64().expect("section length");
    let table = signals(&yes)["token_table_bytes"]
        .as_u64()
        .expect("token-table length");
    assert!(bytes > 0, "a present index has a length");
    assert!(
        table > 0 && table <= bytes,
        "the token table is a prefix of the section: {table} of {bytes}"
    );
    // The whole point of quoting the table separately: it is the figure a first
    // search actually pays, and it is smaller than the section.
    assert!(
        table < bytes,
        "the postings blob is the bulk — {table} should be under {bytes}"
    );

    // The two builds differ ONLY by the index, so the counts must not move.
    assert_eq!(no["triple_count"], yes["triple_count"]);

    // …and the human view says it in both directions too.
    common::rete()
        .arg("card")
        .arg(&indexed)
        .assert()
        .success()
        .stdout(predicate::str::contains("full text  : TEXT_INDEX present"));
    common::rete()
        .arg("card")
        .arg(&plain)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "full text  : no TEXT_INDEX section",
        ));
}

#[test]
fn the_signal_is_measured_at_read_time_and_never_stored() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source());
    let indexed = fixture.path("indexed.rete");
    common::build(
        &source,
        &indexed,
        &["--card", "--title", "Indexed", "--text-index"],
    );

    // The stored metadata section must not contain the signal — otherwise it
    // could outlive the sections it describes. Read the raw bytes rather than
    // the CLI's rendering, which is where the projection is applied.
    let image = std::fs::read(&indexed).unwrap();
    let (start, len) = section(&image, 1).expect("the file has a metadata section");
    let stored = std::str::from_utf8(&image[start..start + len]).unwrap();
    assert!(
        stored.contains("\"signals\""),
        "sanity: the stored card has a signals block"
    );
    assert!(
        !stored.contains("text_index"),
        "the stored card must not carry the signal — it is measured, not written"
    );

    // `repyramid --text-index` ADDS an index to a file built without one. A
    // stamped signal would need the card re-authored to stay true; a measured
    // one simply becomes true, because it was never a claim in the first place.
    let plain = fixture.path("plain2.rete");
    common::build(&source, &plain, &["--card", "--title", "Plain"]);
    let before = common::json(common::rete().arg("card").arg(&plain).arg("--json"));
    assert_eq!(signals(&before)["present"], serde_json::json!(false));

    let repyramided = fixture.path("repyramided.rete");
    common::rete()
        .arg("repyramid")
        .arg(&plain)
        .arg("-o")
        .arg(&repyramided)
        .args(["--text-index", "--card", "--title", "Plain"])
        .assert()
        .success();
    let after = common::json(common::rete().arg("card").arg(&repyramided).arg("--json"));
    assert_eq!(
        signals(&after)["present"],
        serde_json::json!(true),
        "repyramid --text-index added the section; the signal must follow it"
    );
}

/// `rete repyramid` only writes a card when asked for one, so a repyramid
/// without `--card` produces an indexed file with **no card at all** — and the
/// question "can I search this?" still has to be answerable. It is, because the
/// answer never lived in the card: it lives in the section directory.
#[test]
fn a_cardless_file_still_answers_whether_it_can_be_searched() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source());
    let plain = fixture.path("plain.rete");
    common::build(&source, &plain, &["--card", "--title", "Plain"]);

    let stripped = fixture.path("stripped.rete");
    common::rete()
        .arg("repyramid")
        .arg(&plain)
        .arg("-o")
        .arg(&stripped)
        .arg("--text-index")
        .assert()
        .success();
    common::rete()
        .arg("card")
        .arg(&stripped)
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "(no dataset card — TEXT_INDEX present",
        ));
}

#[test]
fn a_ranged_read_answers_without_fetching_the_index() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source());
    let indexed = fixture.path("indexed.rete");
    common::build(
        &source,
        &indexed,
        &["--card", "--title", "Remote indexed", "--text-index"],
    );
    let image = std::fs::read(&indexed).unwrap();
    let url = common::serve(image.clone(), common::RangeMode::Honor);

    let output = common::rete()
        .arg("card-url")
        .arg(&url)
        .arg("--json")
        .output()
        .unwrap();
    assert!(output.status.success());
    let card: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(card["title"], "Remote indexed");
    assert_eq!(signals(&card)["present"], serde_json::json!(true));
    let (_, index_bytes) = section(&image, 6).expect("the file has a TEXT_INDEX section");
    assert_eq!(
        signals(&card)["bytes"].as_u64().unwrap(),
        index_bytes as u64
    );
    let table_bytes = signals(&card)["token_table_bytes"].as_u64().unwrap();

    // The CARD tier's budget, stated exactly: the 1 KiB header, the metadata +
    // build-info sections it points at, and the ≤10-byte varint that measures
    // the token table. Not one byte of the index, and not one byte of the table
    // either — which is the difference between describing an index and reading
    // it. (`fetched N of M bytes in K range request(s)`, on stderr.)
    let meta = section(&image, 1).map(|(_, l)| l).unwrap_or(0);
    let build = section(&image, 7).map(|(_, l)| l).unwrap_or(0);
    let budget = (1024 + meta + build + 10) as u64;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let fetched: u64 = stderr
        .split("fetched ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("no byte report in: {stderr}"));
    assert!(
        fetched <= budget,
        "the card tier fetched {fetched} B against a {budget} B budget \
         (header + card + build record + a 10-byte probe); the index is \
         {index_bytes} B with a {table_bytes} B token table — neither may be read"
    );
}

#[test]
fn card_audit_reports_the_index_and_calls_a_document_unknown() {
    let fixture = common::fixture();
    let source = fixture.write("wordy.nt", wordy_source());
    let indexed = fixture.path("indexed.rete");
    common::build(
        &source,
        &indexed,
        &["--card", "--title", "Audited", "--text-index"],
    );

    let audit = common::json(common::rete().arg("card-audit").arg(&indexed).arg("--json"));
    assert_eq!(audit["text_index"]["present"], serde_json::json!(true));
    assert!(audit["text_index"]["bytes"].as_u64().unwrap() > 0);
    assert!(
        audit["text_index"]["card_said"].is_null(),
        "a freshly built file has nothing to disagree with"
    );

    // A card DOCUMENT has no sections. The audit must say "unknown", never "no
    // index" — the distinction #190 asks for, so an unmeasured card can never be
    // mistaken for a measured negative.
    let saved = common::rete()
        .arg("card")
        .arg(&indexed)
        .arg("--json")
        .output()
        .unwrap();
    let doc = fixture.write("card.json", saved.stdout);
    let from_doc = common::json(common::rete().arg("card-audit").arg(&doc).arg("--json"));
    assert!(
        from_doc["text_index"].is_null(),
        "a card document measures nothing: {}",
        from_doc["text_index"]
    );
    common::rete()
        .arg("card-audit")
        .arg(&doc)
        .assert()
        .success()
        .stdout(predicate::str::contains("full text     unknown"));
}
