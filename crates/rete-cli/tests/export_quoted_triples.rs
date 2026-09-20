//! `rete export --quoted-triple-syntax` at the CLI surface: which spelling a
//! quoted triple leaves the tool in, and what happens where RDF 1.2 has none.
//!
//! WHY THIS FILE EXISTS
//!
//! rete stores one canonical token for a quoted triple, the RDF-star surface
//! `<<s p o>>`, and the writers used to emit it verbatim. Current RDF 1.2
//! parsers do not read that: `oxigraph convert --from-format nq` **rejects** it
//! outright, and in Turtle/TriG reads `<< s p o >>` as a *reifier*, turning one
//! statement into two with a blank node where the triple term was. A dump with
//! a quoted triple in it was therefore refused by the scholar export driver's
//! required parse check — recorded as a latent limitation in #257 and fixed
//! here by writing the RDF 1.2 triple term `<<( s p o )>>` instead.
//!
//! The real referee is a different codebase, and it runs in
//! `tests/scholar/parse_check.sh` and `tests/interop/oxigraph.sh`. What is
//! checkable *here*, with no third-party image, is the wiring: that the default
//! is RDF 1.2, that the legacy surface is still reachable and correct, that
//! both survive a rebuild, and that the shapes RDF 1.2 cannot express are
//! refused by name instead of written as something no parser accepts.

mod common;

/// Quoted triples in **object** position only — the shape RDF 1.2 can express.
/// Includes a nested one (`ttObject` admits another triple term), a typed
/// literal, a blank node on both sides, a language-tagged literal carrying the
/// characters that break a naive rewriter (`>` and `<<`), and a named graph.
const OBJECT_QUOTED: &str = concat!(
    "<http://ex/occ1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> .\n",
    "<http://ex/jsmith> <http://ex/recorded> << <http://ex/occ1> \
     <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> >> .\n",
    "<http://ex/claim1> <http://ex/states> << <http://ex/occ1> <http://ex/count> \
     \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> >> .\n",
    "<http://ex/claim2> <http://ex/states> << <http://ex/a> <http://ex/b> \
     << <http://ex/x> <http://ex/y> <http://ex/z> >> >> <http://ex/g1> .\n",
    "_:b0 <http://ex/notes> << _:b1 <http://ex/b> \"a > b << c\"@en >> <http://ex/g1> .\n",
);

/// The same graph written in the **RDF 1.2** surface on input. `take_term`
/// canonicalises both to the same stored token, so building this must produce
/// the same graph as building `OBJECT_QUOTED` — the property that makes the
/// round trip safe in either direction, pinned so it cannot rot.
const OBJECT_QUOTED_RDF12: &str = concat!(
    "<http://ex/occ1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> .\n",
    "<http://ex/jsmith> <http://ex/recorded> <<( <http://ex/occ1> \
     <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> )>> .\n",
    "<http://ex/claim1> <http://ex/states> <<( <http://ex/occ1> <http://ex/count> \
     \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> )>> .\n",
    "<http://ex/claim2> <http://ex/states> <<( <http://ex/a> <http://ex/b> \
     <<( <http://ex/x> <http://ex/y> <http://ex/z> )>> )>> <http://ex/g1> .\n",
    "_:b0 <http://ex/notes> <<( _:b1 <http://ex/b> \"a > b << c\"@en )>> <http://ex/g1> .\n",
);

/// A quoted triple in **subject** position. Legal RDF-star, legal rete, and
/// *unwritable* in RDF 1.2: `ttSubject ::= iri | BlankNode` and a statement's
/// subject is an IRI or a blank node, so there is no spelling for it at all.
const SUBJECT_QUOTED: &str = concat!(
    "<http://ex/occ1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> .\n",
    "<< <http://ex/occ1> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/Swallow> >> \
     <http://ex/recordedBy> <http://ex/jsmith> .\n",
);

/// A quoted triple nested in another one's subject: the same defect one level
/// down, which only the recursive check catches.
const NESTED_SUBJECT_QUOTED: &str = "<http://ex/claim> <http://ex/states> \
     << << <http://ex/x> <http://ex/y> <http://ex/z> >> <http://ex/p> <http://ex/o> >> .\n";

/// No quoted triples at all — the byte-identity control.
const PLAIN: &str = concat!(
    "<http://ex/a> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://ex/T> .\n",
    "<http://ex/a> <http://ex/p> \"x\"@en .\n",
    "<http://ex/a> <http://ex/q> _:b0 .\n",
    "<http://ex/c> <http://ex/p> <http://ex/3> <http://ex/g1> .\n",
);

fn build_from(nq: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    let out = dir.path().join("out.rete");
    std::fs::write(&src, nq).unwrap();
    common::build(&src, &out, &["--no-pyramid"]);
    (dir, out)
}

fn run(file: &std::path::Path, args: &[&str]) -> std::process::Output {
    common::rete()
        .arg("export")
        .arg(file)
        .args(args)
        .output()
        .unwrap()
}

fn export(file: &std::path::Path, args: &[&str]) -> String {
    let out = run(file, args);
    assert!(
        out.status.success(),
        "export {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Sorted N-Quads in the **RDF-star** surface — i.e. the stored tokens. Used as
/// the order-independent identity of a graph, because it is the one spelling
/// every fixture here can be written in.
fn identity(file: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = export(
        file,
        &["--format", "nq", "--quoted-triple-syntax", "rdf-star"],
    )
    .lines()
    .filter(|l| !l.trim().is_empty())
    .map(str::to_string)
    .collect();
    v.sort();
    v
}

/// Build `text` (in format `fmt`) back into a `.rete` and return its path.
fn rebuild(dir: &tempfile::TempDir, text: &str, fmt: &str) -> std::path::PathBuf {
    let src = dir.path().join(format!("round.{fmt}"));
    let out = dir.path().join(format!("round-{fmt}.rete"));
    std::fs::write(&src, text).unwrap();
    common::build(&src, &out, &["--no-pyramid", "--format", fmt]);
    out
}

// --- the default -----------------------------------------------------------

#[test]
fn the_default_surface_is_the_rdf12_triple_term() {
    let (_dir, file) = build_from(OBJECT_QUOTED);
    for fmt in ["nq", "trig"] {
        let out = export(&file, &["--format", fmt]);
        assert!(
            out.contains(
                "<<( <http://ex/occ1> <http://ex/count> \
                 \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> )>>"
            ),
            "{fmt} should carry RDF 1.2 triple terms:\n{out}"
        );
        // The legacy surface must be gone entirely, not merely accompanied.
        // `<<<` is how the stored token starts when its subject is an IRI.
        assert!(
            !out.contains("<<<"),
            "{fmt} still carries the RDF-star surface:\n{out}"
        );
    }
}

#[test]
fn nesting_is_rewritten_at_every_depth() {
    let (_dir, file) = build_from(OBJECT_QUOTED);
    let nq = export(&file, &["--format", "nq"]);
    assert!(
        nq.contains(
            "<<( <http://ex/a> <http://ex/b> \
             <<( <http://ex/x> <http://ex/y> <http://ex/z> )>> )>>"
        ),
        "the inner triple term must be rewritten too:\n{nq}"
    );
}

#[test]
fn a_literal_inside_a_triple_term_is_not_disturbed() {
    // Term boundaries come from the shared `take_term` scanner, so a literal
    // carrying `>` and `<<` does not derail the rewrite. A writer that split on
    // spaces or counted angle brackets would corrupt exactly this line.
    let (_dir, file) = build_from(OBJECT_QUOTED);
    let nq = export(&file, &["--format", "nq"]);
    assert!(
        nq.contains("<<( _:b1 <http://ex/b> \"a > b << c\"@en )>>"),
        "the literal must survive verbatim:\n{nq}"
    );
}

// --- the legacy surface, to the same standard -------------------------------

#[test]
fn the_rdf_star_surface_is_still_available_and_exact() {
    let (_dir, file) = build_from(OBJECT_QUOTED);
    for fmt in ["nq", "trig"] {
        let out = export(
            &file,
            &["--format", fmt, "--quoted-triple-syntax", "rdf-star"],
        );
        assert!(
            out.contains(
                "<<<http://ex/occ1> <http://ex/count> \
                 \"5\"^^<http://www.w3.org/2001/XMLSchema#integer>>>"
            ),
            "{fmt} --quoted-triple-syntax rdf-star should carry the stored token:\n{out}"
        );
        assert!(
            !out.contains("<<("),
            "{fmt} must not mix in RDF 1.2 triple terms:\n{out}"
        );
    }
}

#[test]
fn an_unknown_surface_is_rejected_by_name() {
    let (_dir, file) = build_from(PLAIN);
    let out = run(
        &file,
        &["--format", "nq", "--quoted-triple-syntax", "rdf-1.2"],
    );
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("rdf12") && err.contains("rdf-star"),
        "the error should name both surfaces: {err}"
    );
}

// --- the round trip, in both surfaces ---------------------------------------

#[test]
fn both_surfaces_rebuild_into_the_same_graph() {
    let (dir, file) = build_from(OBJECT_QUOTED);
    let want = identity(&file);

    for (label, args) in [
        ("rdf12", &["--format", "nq"][..]),
        (
            "rdf-star",
            &["--format", "nq", "--quoted-triple-syntax", "rdf-star"][..],
        ),
    ] {
        let dump = export(&file, args);
        let back = rebuild(&dir, &dump, "nq");
        assert_eq!(
            identity(&back),
            want,
            "the {label} surface did not round-trip"
        );
    }
}

#[test]
fn trig_round_trips_through_rete_in_the_rdf_star_surface() {
    let (dir, file) = build_from(OBJECT_QUOTED);
    let dump = export(
        &file,
        &["--format", "trig", "--quoted-triple-syntax", "rdf-star"],
    );
    let back = rebuild(&dir, &dump, "trig");
    assert_eq!(identity(&back), identity(&file));
}

#[test]
fn a_rdf12_trig_dump_is_not_readable_by_retes_own_trig_parser_yet() {
    // A limit worth pinning rather than discovering. rete's N-Triples/N-Quads
    // tokenizer (`take_term`) accepts BOTH surfaces, but Turtle and TriG go
    // through `oxttl` 0.1, which is RDF-star era and rejects `<<( … )>>`. So a
    // default (RDF 1.2) TriG dump is readable by current third-party parsers
    // and NOT by `rete build` — the reverse of the bug this PR fixes, in the
    // one format pair rete does not parse itself.
    //
    // The export says so on stderr when it writes one, and `--format nq`
    // round-trips in either surface. Closing it means the RDF-1.2-era oxttl,
    // which docs/compatibility.md defers on purpose.
    let (dir, file) = build_from(OBJECT_QUOTED);

    let out = common::rete()
        .arg("export")
        .arg(&file)
        .args(["--format", "trig"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let note = String::from_utf8_lossy(&out.stderr);
    assert!(
        note.contains("RDF 1.2 triple term") && note.contains("--quoted-triple-syntax rdf-star"),
        "the export must warn that rete cannot read this back: {note}"
    );

    let src = dir.path().join("rdf12.trig");
    std::fs::write(&src, String::from_utf8(out.stdout).unwrap()).unwrap();
    let build = common::rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(dir.path().join("rdf12-trig.rete"))
        .args(["--no-pyramid", "--format", "trig"])
        .output()
        .unwrap();
    assert!(
        !build.status.success(),
        "if this now passes, oxttl was upgraded — delete this test and the note in `export`"
    );
}

#[test]
fn ingest_accepts_both_input_surfaces_as_the_same_graph() {
    // Existing behaviour (`take_term` canonicalises both), pinned here so the
    // writer change cannot quietly become a storage change.
    let (_a, star) = build_from(OBJECT_QUOTED);
    let (_b, rdf12) = build_from(OBJECT_QUOTED_RDF12);
    assert_eq!(
        identity(&star),
        identity(&rdf12),
        "an RDF-star file and an RDF 1.2 file must build into the same graph"
    );
}

// --- what RDF 1.2 cannot express --------------------------------------------

#[test]
fn a_subject_position_quoted_triple_is_refused_by_name() {
    // RDF 1.2 puts a triple term in object position only. Writing this one in
    // the `<<( … )>>` spelling anyway would produce a dump the parse check
    // rejects, which is the bug this flag exists to fix — so the export fails,
    // says which slot, and names the flag that can write it.
    let (_dir, file) = build_from(SUBJECT_QUOTED);
    for fmt in ["nq", "trig", "ttl"] {
        let out = run(&file, &["--format", fmt]);
        assert!(
            !out.status.success(),
            "{fmt} should refuse a subject-position quoted triple"
        );
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("subject position") && err.contains("OBJECT position only"),
            "{fmt}: the refusal should name the slot and the rule: {err}"
        );
        assert!(
            err.contains("--quoted-triple-syntax rdf-star"),
            "{fmt}: the refusal should name the way out: {err}"
        );
    }
}

#[test]
fn the_same_graph_still_exports_in_the_rdf_star_surface() {
    // The refusal above is about RDF 1.2's grammar, not about rete's data: the
    // other surface writes it, and it rebuilds.
    let (dir, file) = build_from(SUBJECT_QUOTED);
    let dump = export(
        &file,
        &["--format", "nq", "--quoted-triple-syntax", "rdf-star"],
    );
    assert!(dump.contains("<<<http://ex/occ1>"), "{dump}");
    let back = rebuild(&dir, &dump, "nq");
    assert_eq!(identity(&back), identity(&file));
}

#[test]
fn a_quoted_triple_nested_in_a_subject_is_refused_too() {
    let (_dir, file) = build_from(NESTED_SUBJECT_QUOTED);
    let out = run(&file, &["--format", "nq"]);
    assert!(!out.status.success(), "the nested case must be caught too");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("nested in the SUBJECT of another quoted triple"),
        "the refusal should name the nested case: {err}"
    );
}

// --- formats with no term kind for a quoted triple at all -------------------

#[test]
fn hdt_refuses_a_file_with_quoted_triples() {
    // Before this, HDT interned the whole token as an IRI — brackets stripped
    // by the same rule that strips a real one's — and wrote a file that loads
    // cleanly and means something else. The refusal reads the header flag, so
    // it lands before any of the in-memory build.
    let (_dir, file) = build_from(OBJECT_QUOTED);
    let out = run(&file, &["--format", "hdt"]);
    assert!(!out.status.success(), "HDT should refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no term kind for one") && err.contains("--format trig"),
        "the refusal should say why and where to go: {err}"
    );
    assert!(
        out.stdout.is_empty(),
        "nothing should have been written to stdout"
    );
}

#[test]
fn jsonld_refuses_a_file_with_quoted_triples() {
    // Same defect, same shape: expanded JSON-LD wrote the token as an `@id`.
    let (_dir, file) = build_from(OBJECT_QUOTED);
    let out = run(&file, &["--format", "jsonld"]);
    assert!(!out.status.success(), "JSON-LD should refuse");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("no term kind for one"), "{err}");
}

#[test]
fn hdt_and_jsonld_are_untouched_without_quoted_triples() {
    let (_dir, file) = build_from(PLAIN);
    assert!(run(&file, &["--format", "hdt"]).status.success());
    assert!(run(&file, &["--format", "jsonld"]).status.success());
}

// --- the standing guarantee -------------------------------------------------

#[test]
fn a_file_with_no_quoted_triples_is_byte_identical_in_either_surface() {
    // The export writers were rewritten across #245–#252 with byte-identity as
    // the standing guarantee. The header carries one bit saying whether the
    // file holds a quoted triple at all, and when it does not the surface check
    // is skipped entirely — so this is a property of the code path, not a
    // coincidence of the fixture.
    let (_dir, file) = build_from(PLAIN);
    for fmt in ["nq", "trig", "ttl"] {
        let default = export(&file, &["--format", fmt]);
        let star = export(
            &file,
            &["--format", fmt, "--quoted-triple-syntax", "rdf-star"],
        );
        assert_eq!(default, star, "{fmt} differs between the two surfaces");
        assert!(!default.is_empty(), "{fmt} wrote nothing");
    }
}
