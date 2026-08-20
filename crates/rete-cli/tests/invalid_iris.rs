//! Invalid IRIs: the build audit, `--strict`, and `export --sanitize-iris`.
//!
//! rete's N-Triples/N-Quads reader is deliberately tolerant — it stores whatever
//! sits between `<` and `>`. That is defensible for a container, but
//! `--format nq` names a grammar, and until #233 the exporter re-emitted IRIs
//! outside it while calling the result N-Quads. A user loading the published
//! `scholar/` dumps into Oxigraph paid for that: `epfl-infoscience` had 17,384
//! offending lines, and `openaire-2021-datasource` lost an entire ~102,000-line
//! chunk to a single IRI with no scheme, because a bulk loader rejects the
//! chunk, not the line.
//!
//! What these tests pin:
//!
//! * a graph with invalid IRIs **still builds** (published datasets must not
//!   stop building) and the build **says how many** it ingested;
//! * `--strict` refuses the same input, naming the IRI and the rule;
//! * `--sanitize-iris` percent-encodes the repairable classes, reports what it
//!   changed, and does **not** claim to have fixed an IRI with no scheme.
//!
//! The proof that the sanitized output is what a strict store accepts — and
//! that the unsanitized output is not — needs a real triple store, so it lives
//! in `tests/interop/oxigraph.sh` rather than here.

mod common;

use common::{fixture, rete};

/// One statement per repairable defect class, plus clean controls that must not
/// be flagged. Mirrors `tests/interop/fixtures/repairable.nt`.
const REPAIRABLE: &str = concat!(
    "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
    "<http://example.org/raw/caf\u{e9}> <http://example.org/p> \"a raw ucschar is a valid IRI\" .\n",
    "<http://example.org/a[b]> <http://example.org/p> \"bracket\" .\n",
    "<http://example.org/c#d#e> <http://example.org/p> \"second hash\" .\n",
    "<http://example.org/%x> <http://example.org/p> \"bad percent escape\" .\n",
    "<http://example.org/a b> <http://example.org/p> \"raw space\" .\n",
);

/// The class escaping cannot repair.
const UNREPAIRABLE: &str = concat!(
    "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
    "<noscheme/path> <http://example.org/p> \"no scheme\" .\n",
);

fn stderr_of(out: &std::process::Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// The default stays lenient — this is the whole reason the fix is a warning and
/// not a rejection. A file that builds today must still build.
#[test]
fn a_graph_with_invalid_iris_still_builds_and_says_how_many() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let out = f.path("bad.rete");
    let assert = rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();
    let err = stderr_of(assert.get_output());

    assert!(
        err.contains("4 statement(s) carry an invalid IRI (4 IRI occurrence(s))"),
        "no count in:\n{err}"
    );
    // Per class, so a publisher can tell an escaping problem from a data problem.
    assert!(
        err.contains("'[' or ']' outside an IP-literal host"),
        "{err}"
    );
    assert!(err.contains("more than one '#'"), "{err}");
    assert!(err.contains("'%' not followed by two hex digits"), "{err}");
    assert!(
        err.contains("a character the IRIREF grammar excludes"),
        "{err}"
    );
    // And it points at both flags that do something about it.
    assert!(err.contains("--sanitize-iris"), "{err}");
    assert!(err.contains("--strict"), "{err}");
    assert!(out.exists(), "the build must still produce a file");
}

/// A clean graph must stay silent: a warning that fires on good data is a
/// warning people learn to ignore.
#[test]
fn a_clean_graph_produces_no_warning() {
    let f = fixture();
    let src = f.write(
        "ok.nt",
        concat!(
            "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
            "<http://[::1]:7878/sparql> <http://example.org/p> \"an IP-literal host is legal\" .\n",
            "<urn:uuid:0000> <http://example.org/p> \"so is a URN\" .\n",
            "<http://example.org/a#frag> <http://example.org/p> \"one '#' is the fragment\" .\n",
        ),
    );
    let assert = rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(f.path("ok.rete"))
        .assert()
        .success();
    let err = stderr_of(assert.get_output());
    assert!(!err.contains("invalid IRI"), "false positive:\n{err}");
}

/// `--strict` is the opt-in refusal, and it has to name the IRI: a count alone
/// is not actionable in a 17,384-line dataset.
#[test]
fn strict_refuses_and_names_the_offending_iri() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let assert = rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(f.path("strict.rete"))
        .arg("--strict")
        .assert()
        .failure();
    let err = stderr_of(assert.get_output());
    assert!(
        err.contains("invalid IRI <http://example.org/a[b]>"),
        "{err}"
    );
    assert!(
        err.contains("'[' or ']' outside an IP-literal host"),
        "{err}"
    );
    assert!(err.contains("hint: `--strict` refuses input"), "{err}");
}

/// The memory-bounded external build takes a different code path through the
/// parser; the audit must reach it too, or the largest datasets — exactly the
/// ones built that way — would keep shipping unaudited.
#[test]
fn the_external_build_audits_too() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let assert = rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(f.path("ext.rete"))
        .args(["--memory-budget-mb", "64"])
        .assert()
        .success();
    let err = stderr_of(assert.get_output());
    assert!(err.contains("4 statement(s) carry an invalid IRI"), "{err}");
}

/// `rete validate` answers "does this parse". Now it also answers "is this
/// valid RDF", which is a different question and the one that decides whether a
/// dump will load anywhere else.
#[test]
fn validate_reports_the_audit_and_can_be_strict() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let assert = rete().args(["validate"]).arg(&src).assert().success();
    let err = stderr_of(assert.get_output());
    assert!(err.contains("carry an invalid IRI"), "{err}");

    rete()
        .args(["validate"])
        .arg(&src)
        .arg("--strict")
        .assert()
        .failure();
}

/// Without the flag the export is byte-for-byte what it always was. This is the
/// property that keeps a published dump's identity stable.
#[test]
fn export_is_unchanged_without_the_flag() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let out = f.path("bad.rete");
    rete()
        .args(["build"])
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .assert()
        .success();

    let dump = rete().args(["export"]).arg(&out).assert().success();
    let text = String::from_utf8_lossy(&dump.get_output().stdout).into_owned();
    assert!(text.contains("<http://example.org/a[b]>"), "{text}");
    assert!(text.contains("<http://example.org/c#d#e>"), "{text}");
}

/// The repair, and the honesty about what it cost.
#[test]
fn sanitize_iris_percent_encodes_and_reports() {
    let f = fixture();
    let src = f.write("bad.nt", REPAIRABLE);
    let out = f.path("bad.rete");
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
    let text = String::from_utf8_lossy(&dump.get_output().stdout).into_owned();
    let err = stderr_of(dump.get_output());

    assert!(text.contains("<http://example.org/a%5Bb%5D>"), "{text}");
    assert!(text.contains("<http://example.org/c#d%23e>"), "{text}");
    assert!(text.contains("<http://example.org/%25x>"), "{text}");
    assert!(text.contains("<http://example.org/a%20b>"), "{text}");
    // Valid IRIs are not touched — including the non-ASCII one.
    assert!(text.contains("<http://example.org/ok>"), "{text}");
    assert!(
        text.contains("<http://example.org/raw/caf\u{e9}>"),
        "{text}"
    );
    assert!(
        !text.contains("caf%"),
        "a valid ucschar was escaped:\n{text}"
    );

    assert!(err.contains("percent-encoded 4 IRI occurrence(s)"), "{err}");
    // The cost of the repair is stated, not buried.
    assert!(
        err.contains("no longer joins against the source graph"),
        "{err}"
    );
}

/// An IRI with no scheme is reported and left alone. The exporter must not imply
/// it produced a loadable dump when it did not — that is the difference between
/// a useful flag and a misleading one.
#[test]
fn sanitize_iris_admits_what_it_cannot_repair() {
    let f = fixture();
    let src = f.write("rel.nt", UNREPAIRABLE);
    let out = f.path("rel.rete");
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
    let text = String::from_utf8_lossy(&dump.get_output().stdout).into_owned();
    let err = stderr_of(dump.get_output());

    assert!(
        text.contains("<noscheme/path>"),
        "written verbatim:\n{text}"
    );
    assert!(err.contains("CANNOT be repaired"), "{err}");
    assert!(err.contains("still not valid N-Quads"), "{err}");
}

/// The flag says something even when it found nothing — it was asked for
/// explicitly, and silence would read as "sanitized" rather than "clean".
#[test]
fn sanitize_iris_says_so_when_there_was_nothing_to_do() {
    let f = fixture();
    let src = f.write(
        "ok.nt",
        "<http://example.org/ok> <http://example.org/p> \"fine\" .\n",
    );
    let out = f.path("ok.rete");
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
    assert!(
        stderr_of(dump.get_output()).contains("no invalid IRIs found"),
        "{}",
        stderr_of(dump.get_output())
    );
}

/// Named graphs go through a separate code path in the exporter (the graph term
/// labels every line of its slot), so the graph IRI is sanitized too.
#[test]
fn sanitize_iris_covers_the_graph_term() {
    let f = fixture();
    let src = f.write(
        "g.nq",
        "<http://example.org/s> <http://example.org/p> \"o\" <http://example.org/g[1]> .\n",
    );
    let out = f.path("g.rete");
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
    let text = String::from_utf8_lossy(&dump.get_output().stdout).into_owned();
    assert!(text.contains("<http://example.org/g%5B1%5D>"), "{text}");
}
