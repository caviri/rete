//! **The exporter's report and the export driver's parser are one interface in
//! two files. This is the test that keeps them together.**
//!
//! `scripts/export_scholar_nquads.sh` decides whether a published dump may go
//! into the bucket by reading `rete export --sanitize-iris`'s stderr and filling
//! `state.tsv` from it. Nothing in the type system connects the two: the report
//! is Rust `eprintln!`, the parser is `awk`. Reword a line and the parser does
//! not error — it reads a **zero**, which is the answer that opens the gate.
//! That is exactly how a dump Oxigraph refuses came to be marked publishable.
//!
//! So the two are exercised against each other here, on real output: build a
//! fixture, run the real exporter, hand its real stderr to the real script
//! (`--parse-report`), and require the numbers to be the ones the report states.
//!
//! The cases are chosen to cover every branch of the report — nothing at all,
//! only repairable defects, only unrepairable ones, and a mixture — because a
//! parser can be right about one shape and wrong about the next.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::{fixture, rete, Fixture};

fn repo_root() -> PathBuf {
    // crates/rete-cli -> repo root
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("crate lives two levels under the repo root")
        .to_path_buf()
}

fn driver() -> PathBuf {
    repo_root().join("scripts/export_scholar_nquads.sh")
}

/// Build `nt`, export it with `--sanitize-iris`, and return the report stderr.
fn export_report(f: &Fixture, stem: &str, nt: &str) -> String {
    let src = f.write(&format!("{stem}.nt"), nt);
    let out = f.path(&format!("{stem}.rete"));
    rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let dump = rete()
        .args(["export"])
        .arg(&out)
        .arg("--sanitize-iris")
        .assert()
        .success();
    String::from_utf8_lossy(&dump.get_output().stderr).into_owned()
}

/// Feed a report to the driver's own parser and return what it made of it, as
/// `key=value` pairs.
fn parse_with_driver(f: &Fixture, stem: &str, report: &str) -> Vec<(String, String)> {
    let path = f.write(&format!("{stem}.iri.txt"), report);
    let out = Command::new("bash")
        .arg(driver())
        .arg("--parse-report")
        .arg(&path)
        .current_dir(repo_root())
        .output()
        .expect("bash is required to run the export driver's parser");
    assert!(
        out.status.success(),
        "the driver could not parse the exporter's own report.\n\
         --- report ---\n{report}\n--- stderr ---\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[track_caller]
fn field(parsed: &[(String, String)], key: &str) -> u64 {
    parsed
        .iter()
        .find(|(k, _)| k == key)
        .unwrap_or_else(|| panic!("the driver printed no `{key}`: {parsed:?}"))
        .1
        .parse()
        .expect("a count")
}

/// One statement per class, so every row of the report is present at once.
const EVERY_CLASS: &str = concat!(
    "<http://example.org/ok> <http://example.org/p> \"clean control\" .\n",
    "<http://example.org/a[b]> <http://example.org/p> \"bracket\" .\n",
    "<http://example.org/c#d#e> <http://example.org/p> \"second hash\" .\n",
    "<http://example.org/%x> <http://example.org/p> \"bad percent\" .\n",
    "<http://example.org/a b> <http://example.org/p> \"raw space\" .\n",
    "<noscheme/path> <http://example.org/p> \"relative\" .\n",
    "<https://::1> <http://example.org/p> \"unbracketed IPv6\" .\n",
);

/// Every branch of the report, parsed by the script that has to act on it.
#[test]
fn the_driver_parses_the_exporters_own_report() {
    let f = fixture();

    // (stem, input, invalid, repairable, unrepairable, unclassified)
    let cases: &[(&str, &str, u64, u64, u64, u64)] = &[
        (
            "clean",
            "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
            0,
            0,
            0,
            0,
        ),
        (
            "repairable_only",
            concat!(
                "<http://example.org/a[b]> <http://example.org/p> \"bracket\" .\n",
                "<http://example.org/c#d#e> <http://example.org/p> \"hash\" .\n",
            ),
            2,
            2,
            0,
            0,
        ),
        (
            "relative_only",
            "<noscheme/path> <http://example.org/p> \"relative\" .\n",
            1,
            0,
            1,
            0,
        ),
        (
            "unclassified_only",
            "<https://::1> <http://example.org/p> \"unbracketed IPv6\" .\n",
            1,
            0,
            1,
            1,
        ),
        ("every_class", EVERY_CLASS, 6, 4, 2, 1),
    ];

    for (stem, nt, invalid, repairable, unrepairable, unclassified) in cases {
        let report = export_report(&f, stem, nt);
        // The report must state the totals the driver is about to read.
        assert!(
            report.contains(&format!(
                "totals invalid={invalid} repairable={repairable} \
                 unrepairable={unrepairable} unclassified={unclassified}"
            )),
            "{stem}: the exporter's own totals line is not what this test expects.\n{report}"
        );
        let parsed = parse_with_driver(&f, stem, &report);
        assert_eq!(field(&parsed, "invalid"), *invalid, "{stem} invalid");
        assert_eq!(
            field(&parsed, "unrepairable"),
            *unrepairable,
            "{stem} unrepairable — THIS is the publication gate"
        );
        assert_eq!(
            field(&parsed, "unclassified"),
            *unclassified,
            "{stem} unclassified"
        );
    }
}

/// The per-class columns of `state.tsv` come from the indented rows, and those
/// are matched on fragments of `IriDefect::reason()`. A reason the script does
/// not recognise must not land in a class.
#[test]
fn every_class_row_reaches_its_own_column() {
    let f = fixture();
    let report = export_report(&f, "classes", EVERY_CLASS);
    let parsed = parse_with_driver(&f, "classes", &report);
    for (column, expected) in [
        ("bracket", 1),
        ("hash", 1),
        ("percent", 1),
        ("forbidden", 1),
        ("schemeless", 1),
    ] {
        assert_eq!(
            field(&parsed, column),
            expected,
            "{column} did not reach its column — a reason string moved.\n{report}"
        );
    }
    // The sum of the class columns must account for every invalid occurrence.
    // If it does not, a class row was dropped rather than counted, which the
    // script also warns about — this asserts it.
    let class_sum: u64 = ["bracket", "hash", "percent", "forbidden", "schemeless"]
        .iter()
        .map(|c| field(&parsed, c))
        .sum::<u64>()
        + field(&parsed, "unclassified");
    assert_eq!(
        class_sum,
        field(&parsed, "invalid"),
        "class rows do not account for every invalid occurrence.\n{report}"
    );
}

fn verdict(parsed: &[(String, String)]) -> String {
    parsed
        .iter()
        .find(|(k, _)| k == "verdict")
        .expect("the driver printed no verdict")
        .1
        .clone()
}

/// **The acceptance case.** A fixture whose only defect is an unbracketed IPv6
/// authority must end `refuse`, not publishable — even though no repair class
/// recognises it and the class the old gate read is zero.
///
/// The verdict comes from `gate_refuses`, the same shell function the sweep
/// calls, so this tests the decision rather than a restatement of it.
#[test]
fn an_unbracketed_ipv6_dump_is_refused_not_published() {
    let f = fixture();
    let report = export_report(
        &f,
        "gate",
        "<https://::1> <http://example.org/p> \"unbracketed IPv6\" .\n",
    );
    let parsed = parse_with_driver(&f, "gate", &report);

    // The old gate read this, and it is zero here. That is the incident.
    assert_eq!(
        field(&parsed, "schemeless"),
        0,
        "the old `schemeless` gate would have opened, which is the point"
    );
    assert!(field(&parsed, "unrepairable") > 0);
    assert_eq!(
        verdict(&parsed),
        "refuse",
        "the dump must not be published.\n{report}"
    );
}

/// The other direction, which costs just as much to get wrong: a dump whose
/// defects were all repaired is publishable. A gate that refuses everything is
/// not a gate.
#[test]
fn a_repaired_dump_is_publishable() {
    let f = fixture();
    for (stem, nt) in [
        (
            "clean_gate",
            "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
        ),
        (
            "repaired_gate",
            concat!(
                "<http://example.org/a[b]> <http://example.org/p> \"bracket\" .\n",
                "<http://example.org/c#d#e> <http://example.org/p> \"hash\" .\n",
            ),
        ),
        (
            // Every false-positive case from the design, in one dump.
            "legal_gate",
            concat!(
                "<http://user:pass@host/> <http://example.org/p> \"userinfo colon\" .\n",
                "<http://[::1]:8080/p> <http://example.org/p> \"bracketed IPv6\" .\n",
                "<urn:isbn:0451450523> <http://example.org/p> \"no authority\" .\n",
                "<mailto:a@b.com> <http://example.org/p> \"path at-sign\" .\n",
                "<http://host:8080/a:b> <http://example.org/p> \"path colons\" .\n",
                "<http://caf\u{e9}.example/> <http://example.org/p> \"ucschar host\" .\n",
            ),
        ),
    ] {
        let report = export_report(&f, stem, nt);
        let parsed = parse_with_driver(&f, stem, &report);
        assert_eq!(
            verdict(&parsed),
            "publishable",
            "{stem} was refused and should not have been.\n{report}"
        );
    }
}
