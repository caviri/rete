//! `rete export --format ttl|trig` at the CLI surface.
//!
//! The unit tests in `commands::turtle` cover the writer in isolation — QName
//! formation, escape-aware literal scanning, the grouping shapes. What they
//! cannot cover is the *wiring*: which graphs a format is handed, whether those
//! get wrapped in `GRAPH` blocks, and whether the result survives a rebuild.
//! Those are the bugs this file exists for, and the first test here is one that
//! actually escaped the unit tests — Turtle emitting a TriG `GRAPH { }` block
//! when the graph-selection ladder handed it a named graph.
//!
//! The round-trip tests deliberately compare through **N-Quads of both sides**
//! rather than against hand-written expected Turtle. Turtle output is allowed to
//! change shape (a new prefix, different grouping) without changing meaning, so
//! an expected-bytes test would fail on improvements and pass on real losses.

mod common;

/// Default graph plus two named graphs, with the shapes that break serializers:
/// a literal containing an escaped quote, a typed literal, a language tag, a
/// blank node, an IRI whose local part cannot be a QName, and one that can.
const QUADS: &str = concat!(
    "<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/T> .\n",
    "<http://ex/a> <http://ex/p> <http://ex/1> .\n",
    "<http://ex/a> <http://ex/p> <http://ex/2> .\n",
    "<http://ex/a> <http://ex/q> \"he said \\\"no\\\"\" .\n",
    "<http://ex/a> <http://ex/n> \"30\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
    "<http://ex/a> <http://ex/l> \"h\u{e9}llo\"@en .\n",
    "<http://ex/a> <http://ex/b> _:b0 .\n",
    "<http://ex/needs%20escaping/x> <http://ex/p> <http://ex/has(paren)> .\n",
    "<http://ex/c> <http://ex/p> <http://ex/3> <http://ex/g1> .\n",
    "<http://ex/c> <http://ex/p> <http://ex/4> <http://ex/g1> .\n",
    "<http://ex/d> <http://ex/p> <http://ex/5> <http://ex/g2> .\n",
);

/// Only a named graph — no default-graph statements at all. This is the shape a
/// TriG dump of most public datasets has, and the one the selection ladder has
/// to reason about.
const NAMED_ONLY: &str = concat!(
    "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/only> .\n",
    "<http://ex/s> <http://ex/q> \"x\" <http://ex/only> .\n",
);

fn build(nq: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, nq).unwrap();
    common::build(&src, &out, &["--no-pyramid"]);
    (dir, out)
}

fn export(file: &std::path::Path, args: &[&str]) -> String {
    let out = common::rete()
        .arg("export")
        .arg(file)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "export {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Sorted N-Quads of a file — the order-independent identity of its graph.
fn quads(file: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = export(file, &["--format", "nq"])
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect();
    v.sort();
    v
}

/// Serialize as `fmt`, build the result back into a `.rete`, and return its
/// quads — so the caller can compare against the source's.
fn rebuild(dir: &tempfile::TempDir, text: &str, fmt: &str) -> std::path::PathBuf {
    let src = dir.path().join(format!("round.{fmt}"));
    let out = dir.path().join(format!("round-{fmt}.rete"));
    std::fs::write(&src, text).unwrap();
    common::build(&src, &out, &["--no-pyramid", "--format", fmt]);
    out
}

// --- the wiring bug ---------------------------------------------------------

#[test]
fn turtle_never_emits_a_trig_graph_block() {
    // When the default graph is empty and exactly one named graph exists, the
    // ladder hands Turtle that named graph. Turtle has no graph term, so the
    // statements must be written bare: wrapping them would put TriG syntax in a
    // file called `.ttl`, which no Turtle parser accepts.
    let (_dir, file) = build(NAMED_ONLY);
    let ttl = export(&file, &["--format", "ttl"]);
    assert!(
        !ttl.contains("GRAPH"),
        "Turtle must not wrap a graph:\n{ttl}"
    );
    assert!(!ttl.contains('{'), "nor open a block:\n{ttl}");
    assert!(
        ttl.contains("<http://ex/s>"),
        "…but must write the data:\n{ttl}"
    );

    // TriG, given the same file, must do the opposite.
    let trig = export(&file, &["--format", "trig"]);
    assert!(
        trig.contains("GRAPH <http://ex/only> {"),
        "TriG must wrap it:\n{trig}"
    );
}

// --- the graph-selection ladder --------------------------------------------

#[test]
fn a_single_graph_format_says_which_graph_it_chose() {
    let (_dir, file) = build(QUADS);
    let out = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "ttl"])
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("default graph"), "stderr: {err}");
    assert!(
        err.contains("NOT included") && err.contains("--format trig"),
        "it must say the named graphs were dropped, and how to keep them: {err}"
    );
}

#[test]
fn an_empty_default_graph_with_one_named_graph_exports_that_one() {
    let (_dir, file) = build(NAMED_ONLY);
    let out = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "ttl"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("<http://ex/only>"), "stderr: {err}");
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("<http://ex/o>"),
        "the named graph's data must be written"
    );
}

#[test]
fn an_empty_default_graph_with_several_named_graphs_is_an_error_that_lists_them() {
    // The genuinely ambiguous case: fail rather than pick, merge, or write
    // nothing. Two graphs, no default-graph statements.
    let (_dir, file) = build(concat!(
        "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g1> .\n",
        "<http://ex/s> <http://ex/p> <http://ex/o> <http://ex/g2> .\n",
    ));
    let out = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "ttl"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "must not exit 0");
    let err = String::from_utf8_lossy(&out.stderr);
    for want in [
        "<http://ex/g1>",
        "<http://ex/g2>",
        "--graph",
        "--format trig",
    ] {
        assert!(err.contains(want), "error must mention {want}: {err}");
    }
    assert!(
        String::from_utf8_lossy(&out.stdout).is_empty(),
        "and must write nothing to stdout"
    );
}

#[test]
fn an_unknown_graph_names_the_graphs_that_exist() {
    let (_dir, file) = build(QUADS);
    let out = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "ttl", "--graph", "http://ex/nope"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("<http://ex/g1>"), "{err}");
}

// --- round-trips ------------------------------------------------------------

#[test]
fn trig_round_trips_every_graph() {
    let (dir, file) = build(QUADS);
    let trig = export(&file, &["--format", "trig"]);
    let back = rebuild(&dir, &trig, "trig");
    assert_eq!(quads(&file), quads(&back), "TriG must be lossless:\n{trig}");
}

#[test]
fn turtle_round_trips_the_graph_it_wrote() {
    let (dir, file) = build(QUADS);
    let ttl = export(&file, &["--format", "ttl"]);
    let back = rebuild(&dir, &ttl, "ttl");
    // Turtle carries one graph, so compare against the default graph alone.
    let want = {
        let mut v: Vec<String> = export(&file, &["--format", "nq", "--graph", ""])
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        v.sort();
        v
    };
    assert_eq!(want, quads(&back), "Turtle must round-trip:\n{ttl}");
}

#[test]
fn prefix_compression_does_not_change_the_graph() {
    // `--no-prefixes` and the default must denote the same thing. Comparing the
    // two rebuilt files is the only check that catches a QName which is legal
    // Turtle but resolves to the wrong IRI — the failure mode a bytes-comparison
    // test would sail past.
    let (dir, file) = build(QUADS);
    let with = rebuild(&dir, &export(&file, &["--format", "trig"]), "trig");
    let plain = {
        let text = export(&file, &["--format", "trig", "--no-prefixes"]);
        let src = dir.path().join("plain.trig");
        let out = dir.path().join("plain.rete");
        std::fs::write(&src, text).unwrap();
        common::build(&src, &out, &["--no-pyramid", "--format", "trig"]);
        out
    };
    assert_eq!(quads(&with), quads(&plain));
    assert_eq!(quads(&file), quads(&with));
}

#[test]
fn an_iri_whose_local_part_cannot_be_a_qname_is_written_in_full() {
    let (_dir, file) = build(QUADS);
    let trig = export(&file, &["--format", "trig"]);
    // `has(paren)` is not PN_LOCAL, and we never emit PN_LOCAL_ESC.
    assert!(
        trig.contains("<http://ex/has(paren)>"),
        "must stay a full IRI:\n{trig}"
    );
    assert!(!trig.contains(":has(paren)"), "never as a QName:\n{trig}");
}

#[test]
fn a_literal_with_an_escaped_quote_survives_verbatim() {
    let (_dir, file) = build(QUADS);
    for fmt in ["ttl", "trig"] {
        let out = export(&file, &["--format", fmt]);
        assert!(
            out.contains(r#""he said \"no\"""#),
            "{fmt} mangled the literal:\n{out}"
        );
    }
}

// --- streaming contract -----------------------------------------------------

#[test]
fn the_dump_is_identical_at_every_memory_budget() {
    // The bounded-export guarantee (#245-#248) that Turtle and TriG inherit: the
    // budget changes residency, never the bytes.
    let (_dir, file) = build(QUADS);
    for fmt in ["ttl", "trig"] {
        let base = export(&file, &["--format", fmt, "--memory-budget-mb", "4096"]);
        for mb in ["1", "16", "0"] {
            assert_eq!(
                base,
                export(&file, &["--format", fmt, "--memory-budget-mb", mb]),
                "{fmt} differs at --memory-budget-mb {mb}"
            );
        }
        assert_eq!(
            base,
            export(&file, &["--format", fmt, "--in-memory"]),
            "{fmt} differs under --in-memory"
        );
    }
}

#[test]
fn a_bound_predicate_still_produces_parseable_output() {
    // A bound predicate routes to POS, which is ordered by object then subject —
    // NOT grouped by subject. The writer must notice and fall back to one
    // statement per line rather than emit blocks that split a subject.
    let (dir, file) = build(QUADS);
    let trig = export(&file, &["--format", "trig", "--predicate", "http://ex/p"]);
    let back = rebuild(&dir, &trig, "trig");
    let want = {
        let mut v: Vec<String> = export(&file, &["--format", "nq", "--predicate", "http://ex/p"])
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect();
        v.sort();
        v
    };
    assert_eq!(want, quads(&back), "filtered TriG must round-trip:\n{trig}");
}

#[test]
fn only_used_prefixes_are_declared() {
    let (_dir, file) = build(QUADS);
    let trig = export(&file, &["--format", "trig"]);
    for line in trig.lines().filter(|l| l.starts_with("@prefix ")) {
        let name = line
            .trim_start_matches("@prefix ")
            .split(':')
            .next()
            .unwrap();
        let uses = trig.matches(&format!("{name}:")).count();
        assert!(
            uses > 1,
            "prefix {name} is declared but never used:\n{trig}"
        );
    }
}
