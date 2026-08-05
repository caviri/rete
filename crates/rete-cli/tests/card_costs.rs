//! `rete card-audit --measure` — what the starter queries a published card
//! ships actually cost, and what happens when those figures are written back.
//!
//! The property under test throughout is that a **re-measurement reproduces the
//! build record**. Both go through the same `measure_query`, so `bytes` and
//! `requests` are a function of file layout and query text and nothing else —
//! which is exactly what makes a cost figure worth publishing, and what a
//! second implementation would quietly destroy.

mod common;

use predicates::prelude::*;

/// A fixture with a full derived card. `closed` decides whether every
/// non-literal object is also a subject: when it is, nothing dangles, and
/// `top-dangling` — the template the static audit calls **undecidable**,
/// because a card does not record which objects are also subjects — comes back
/// empty. That one query is the whole argument for measuring: no card-only
/// method can reach it, and a single run settles it.
///
/// Note what `closed` implies about the build. A measuring build now *drops* a
/// starter query it measured at zero, so `closed` + a plain `--card` build
/// yields a card that no longer ships `top-dangling` at all. Tests that need a
/// file which **does** ship an empty starter query — i.e. one shaped like the
/// published corpus this command was written for — pass `--no-card-costs` in
/// `extra`, which skips the run and therefore the drop.
fn carded(closed: bool, extra: &[&str]) -> (common::Fixture, std::path::PathBuf) {
    let fixture = common::fixture();
    let mut nq = String::new();
    for i in 0..2000 {
        if !closed {
            nq.push_str(&format!(
                "<http://example.test/s{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> \
                 <http://example.test/Thing> .\n"
            ));
        }
        nq.push_str(&format!(
            "<http://example.test/s{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Thing \
             number {i}\"@en .\n<http://example.test/s{i}> <http://example.test/next> \
             <http://example.test/s{}> .\n",
            (i + 1) % 2000
        ));
    }
    let source = fixture.write("graph.nq", nq);
    let out = fixture.path("graph.rete");
    let mut args = vec!["--card"];
    args.extend_from_slice(extra);
    common::build(&source, &out, &args);
    (fixture, out)
}

/// The check the whole command exists to make: re-measuring a file that
/// already carries a build record reproduces that record, query for query.
///
/// `bytes` and `requests` are claimed to be portable properties of layout +
/// query. This is what that claim cashes out to — and the command reports the
/// comparison itself (`recorded.agrees`) rather than leaving a human to diff
/// two tables.
#[test]
fn a_re_measurement_reproduces_the_build_record() {
    let (_fixture, out) = carded(false, &[]);
    let doc = common::json(
        common::rete()
            .arg("card-audit")
            .arg(&out)
            .args(["--measure", "--json"]),
    );
    let findings = doc["findings"].as_array().unwrap();
    assert!(!findings.is_empty(), "the fixture card ships queries");
    let mut checked = 0;
    for f in findings {
        let o = &f["observed"];
        assert!(!o.is_null(), "{} was not measured", f["id"]);
        let Some(rec) = o.get("recorded").filter(|r| !r.is_null()) else {
            continue;
        };
        checked += 1;
        assert_eq!(
            rec["agrees"],
            true,
            "{}: measured {} B / {} req / {} rows, record says {} B / {} req / {} rows",
            f["id"],
            o["bytes"],
            o["requests"],
            o["rows"],
            rec["bytes"],
            rec["requests"],
            rec["rows"]
        );
    }
    assert!(checked > 0, "the build record carried no query costs");
    // The transport is part of the answer, not a footnote.
    let m = &doc["measurement"];
    assert!(m["transport"].as_str().unwrap().starts_with("local file "));
    assert_eq!(m["queries_run"], findings.len());
    assert!(m["total_bytes"].as_u64().unwrap() > 0);
}

/// Local and remote measure the **same quantity**, and the output says which
/// one produced the number anyway.
///
/// This is the claim that makes a free local measurement usable as a stand-in
/// for the paid remote one. It holds because no block cache is in the stack:
/// the reader sequence a query issues is a function of layout and query, so a
/// file handle and an HTTP client fetch the same ranges. The one thing that
/// differs is the reader's fan-out hint — 1 for a file handle, 16 for the HTTP
/// client — which the planner is allowed to use, so the equality is asserted
/// here across every starter query rather than assumed from the design.
#[test]
fn local_and_remote_measure_the_same_thing_and_both_say_so() {
    let (_fixture, out) = carded(false, &[]);
    let local = common::json(
        common::rete()
            .arg("card-audit")
            .arg(&out)
            .args(["--measure", "--json"]),
    );
    let url = common::serve(std::fs::read(&out).unwrap(), common::RangeMode::Honor);
    let remote = common::json(
        common::rete()
            .arg("card-audit")
            .arg(&url)
            .args(["--measure", "--json"]),
    );

    let transport = remote["measurement"]["transport"].as_str().unwrap();
    assert!(
        transport.starts_with("HTTP range requests to http://"),
        "{transport}"
    );
    assert!(transport.contains("no block cache"), "{transport}");
    assert!(transport.contains("reader fan-out"), "{transport}");

    let costs = |doc: &serde_json::Value| -> Vec<(String, u64, u64, u64)> {
        doc["findings"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|f| !f["observed"].is_null())
            .map(|f| {
                let o = &f["observed"];
                (
                    f["id"].as_str().unwrap().to_string(),
                    o["bytes"].as_u64().unwrap(),
                    o["requests"].as_u64().unwrap(),
                    o["rows"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(
        costs(&local),
        costs(&remote),
        "bytes/requests/rows must not depend on the transport"
    );
    assert_eq!(
        remote["measurement"]["total_bytes"],
        local["measurement"]["total_bytes"]
    );
}

/// `--only` is the difference between spending 3 MB and spending 8 GB, so it
/// has to actually restrict the run.
#[test]
fn only_measures_the_queries_it_was_given() {
    let (_fixture, out) = carded(false, &[]);
    let doc = common::json(common::rete().arg("card-audit").arg(&out).args([
        "--measure",
        "--json",
        "--only",
        "ov-triples",
    ]));
    assert_eq!(doc["measurement"]["queries_run"], 1);
    let measured: Vec<_> = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| !f["observed"].is_null())
        .map(|f| f["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(measured, vec!["ov-triples"]);

    common::rete()
        .arg("card-audit")
        .arg(&out)
        .args(["--measure", "--only", "not-a-query"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no starter query matches --only"));
}

/// A byte budget stops a query rather than letting it download the file, and
/// the row still reports what it spent getting that far — "costs more than
/// N MB" is itself a usable answer about a remote file.
#[test]
fn a_byte_budget_stops_a_query_and_still_reports_the_spend() {
    let (_fixture, out) = carded(false, &[]);
    // 20 KB: under what any of this fixture's queries needs, over what the
    // header + card cost, so the budget bites mid-query rather than at open.
    let doc = common::json(common::rete().arg("card-audit").arg(&out).args([
        "--measure",
        "--json",
        "--max-mb",
        "0.02",
    ]));
    let stopped: Vec<_> = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["observed"]["outcome"] == "error")
        .collect();
    assert!(
        !stopped.is_empty(),
        "a 20 KB budget should stop at least one query on this fixture"
    );
    for f in stopped {
        assert!(f["observed"]["error"]
            .as_str()
            .unwrap()
            .contains("byte budget exhausted"));
        assert!(f["observed"]["bytes"].as_u64().unwrap() <= 20 * 1024);
    }
    // A run that did not finish is not a cost record.
    common::rete()
        .arg("card-audit")
        .arg(&out)
        .args([
            "--measure",
            "--write-costs",
            "--allow-empty",
            "--max-mb",
            "0.02",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("did not finish"));
}

/// Writing costs back into a file keeps its **identity** (the content hash is
/// not over the build-info section) and its **data** (byte-identical N-Quads),
/// while changing its bytes (the section is near the front, so everything
/// behind it shifts). All four of those have to be true at once or the feature
/// is not safe to point at a published file.
#[test]
fn writing_costs_preserves_the_hash_and_the_data() {
    // Built WITHOUT costs: the state 109 of the 110 published cards are in.
    let (_fixture, out) = carded(false, &["--no-card-costs"]);
    let before_len = std::fs::metadata(&out).unwrap().len();
    let before_hash = common::json(common::rete().arg("card").arg(&out).arg("--json"))["checksum"]
        .as_str()
        .map(str::to_string);
    let before_nq = common::rete()
        .arg("export")
        .arg(&out)
        .args(["--format", "nq"])
        .output()
        .unwrap()
        .stdout;

    common::rete()
        .arg("card-audit")
        .arg(&out)
        .args(["--measure", "--write-costs"])
        .assert()
        .success()
        .stderr(predicate::str::contains("content hash unchanged"));

    // Identity: same content hash, still verifies.
    let after = common::json(common::rete().arg("card").arg(&out).arg("--json"));
    assert_eq!(after["checksum"].as_str().map(str::to_string), before_hash);
    common::rete().arg("verify").arg(&out).assert().success();

    // Data: byte-identical N-Quads.
    let after_nq = common::rete()
        .arg("export")
        .arg(&out)
        .args(["--format", "nq"])
        .output()
        .unwrap()
        .stdout;
    assert_eq!(before_nq, after_nq, "the data must not move");

    // Bytes: the file really was rewritten — that is the cost being paid.
    assert!(
        std::fs::metadata(&out).unwrap().len() > before_len,
        "the build-info section has to come from somewhere"
    );

    // And the costs are now readable from the CARD tier, which is the point:
    // the next reader gets them in two range requests instead of re-measuring.
    common::rete()
        .arg("card")
        .arg(&out)
        .assert()
        .success()
        .stdout(predicate::str::contains("query costs"));

    // Re-measuring now finds a record to check itself against, and agrees with
    // it — the write round-trips through the same measurement.
    let again = common::json(
        common::rete()
            .arg("card-audit")
            .arg(&out)
            .args(["--measure", "--json"]),
    );
    for f in again["findings"].as_array().unwrap() {
        let rec = &f["observed"]["recorded"];
        assert!(!rec.is_null(), "{} lost its record", f["id"]);
        assert_eq!(rec["agrees"], true, "{}", f["id"]);
    }
}

/// The ways `--write-costs` refuses, each because writing would make the file's
/// self-description worse than saying nothing.
#[test]
fn writing_costs_refuses_what_would_mislead() {
    let (_fixture, out) = carded(false, &["--no-card-costs"]);

    // A partial run would be stored as if it were the whole card.
    common::rete()
        .arg("card-audit")
        .arg(&out)
        .args(["--measure", "--write-costs", "--only", "ov-triples"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("refuses a partial run"));

    // A URL cannot be written to.
    let url = common::serve(std::fs::read(&out).unwrap(), common::RangeMode::Honor);
    common::rete()
        .arg("card-audit")
        .arg(&url)
        .args(["--write-costs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("needs a local file"));

    // The interesting one: on a fully-described graph `top-dangling` really is
    // empty, which the card cannot tell and the run can. A cost for a query
    // that answers nothing is not worth rewriting a file for — say so, and let
    // `--allow-empty` be the deliberate override for the template whose
    // emptiness IS the answer.
    let (_closed_fixture, closed) = carded(true, &["--no-card-costs"]);
    common::rete()
        .arg("card-audit")
        .arg(&closed)
        .args(["--measure", "--write-costs"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("returned zero rows"))
        .stderr(predicate::str::contains("top-dangling"))
        .stderr(predicate::str::contains("needs a re-card"));
    common::rete()
        .arg("card-audit")
        .arg(&closed)
        .args(["--measure", "--write-costs", "--allow-empty"])
        .assert()
        .success();
}

/// The headline: a run settles what a card cannot. `top-dangling` is
/// `undecidable` from any card — nothing in one records which objects are also
/// subjects — and on a fully-described graph it comes back empty. #175 left
/// that template undecided on 79 of 96 published files; one measurement each
/// would have closed it.
///
/// The fixture is built `--no-card-costs` **on purpose**, and the second half
/// of the test is why. A build that measures now drops a starter query it
/// measured at zero (#176), so a current `rete build --card` cannot produce a
/// file that ships one. The files this command exists for can: everything
/// published before that change, every external/memory-bounded build and
/// `rete merge` (no costs measured, so no drop), and every build that opted out
/// with `--no-card-costs`. Auditing those is the whole use case — of 110
/// published cards surveyed, exactly one carries a build record — so the
/// fixture reproduces one rather than testing a shape the build now prevents.
#[test]
fn a_run_settles_a_verdict_no_card_can_reach() {
    let (_fixture, closed) = carded(true, &["--no-card-costs"]);
    let doc = common::json(common::rete().arg("card-audit").arg(&closed).args([
        "--measure",
        "--json",
        "--only",
        "top-dangling",
    ]));
    let f = doc["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["id"] == "top-dangling")
        .expect("an unmeasured build still ships top-dangling");
    assert_eq!(f["verdict"], "undecidable", "the card cannot decide it");
    assert_eq!(f["observed"]["outcome"], "empty", "the run can");
    assert_eq!(f["observed"]["rows"], 0);

    // The other half of the same fact: when the build DOES measure, it settles
    // this before anyone can audit it. Same graph, same query, no
    // `--no-card-costs` — the card ships without `top-dangling` and the build
    // record says why. Asserted here, next to the audit, so that the two
    // commands' shared oracle cannot regress on one side without the other
    // noticing.
    let (_measured_fixture, measured) = carded(true, &[]);
    let card = common::json(common::rete().arg("card").arg(&measured).arg("--json"));
    assert!(
        !card["queries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|q| q["id"] == "top-dangling"),
        "a measured build must not ship the query it measured at zero rows"
    );
    let dropped = card["build"]["dropped_queries"].as_array().unwrap();
    let d = dropped
        .iter()
        .find(|d| d["id"] == "top-dangling")
        .expect("the drop is recorded in the build record");
    assert!(d["why"].as_str().unwrap().contains("0 rows"), "{d}");
    assert!(
        d["contradicts_claim"].is_null(),
        "top-dangling declares itself undecidable — a measured zero is news, not a defect"
    );
}

/// `--measure` needs the file. A card document — what a survey saves, and what
/// the static audit is happy to take — has no data behind it, and the error has
/// to say so rather than reporting zeros.
#[test]
fn measuring_a_card_document_is_refused() {
    let (fixture, out) = carded(false, &[]);
    let card = common::rete()
        .arg("card")
        .arg(&out)
        .arg("--json")
        .output()
        .unwrap()
        .stdout;
    let doc = fixture.write("card.json", card);
    common::rete()
        .arg("card-audit")
        .arg(&doc)
        .assert()
        .success(); // the static audit is fine with a card document
    common::rete()
        .arg("card-audit")
        .arg(&doc)
        .arg("--measure")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--measure needs the .rete file"));
}
