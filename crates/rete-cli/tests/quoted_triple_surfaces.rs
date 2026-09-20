//! `rete build --quoted-triple-syntax` — reading BOTH quoted-triple surfaces,
//! and the round-trip matrix that says the two directions actually meet.
//!
//! WHY THIS FILE EXISTS
//!
//! #262 gave `rete export` a choice of surface and defaulted it to `rdf12`,
//! because that is what current parsers read. It left `rete build` able to read
//! only one of them in Turtle/TriG: the reader was `oxttl` 0.1, which knows
//! `<< s p o >>` and rejects `<<( s p o )>>`. rete could therefore not re-ingest
//! its own default TriG output — an interchange format that fails at its own
//! round trip.
//!
//! The obstacle was never the store. rete keeps one canonical token, `<<s p o>>`,
//! and RDF 1.2 reification is *ordinary RDF*: `_:r rdf:reifies <<( s p o )>>` is
//! a blank node, an IRI and a term, all of which rete stored before any of this.
//! The whole difference lives at the parse boundary, and it is genuinely
//! ambiguous there: in Turtle, `<< s p o >>` is a quoted triple under RDF-star
//! and a reifier under RDF 1.2. Same bytes, two graphs. Nothing can detect it,
//! so it is a flag.
//!
//! WHAT IS PINNED HERE
//!
//! 1. The matrix: every (input surface × output surface) pair round-trips to one
//!    identical graph. This is the acceptance bar.
//! 2. The ambiguity, from both sides: one Turtle file read under the two
//!    surfaces produces the two DIFFERENT graphs it should, asserted as graphs
//!    and not as parser trivia.
//! 3. rete's own `--format trig` default output, re-ingested by `rete build` —
//!    the bug that started this.
//! 4. The default is `rdf-star`, and the mistake it leaves possible is LOUD:
//!    an RDF 1.2 file read as RDF-star fails with an error that names the flag.
//! 5. Files with no quoted triples are untouched: same graph, same bytes, either
//!    surface, either direction.

mod common;

const RDF_REIFIES: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#reifies>";

/// The ambiguous file: one `<< s p o >>` used as an object, one as a subject.
/// Every term is an IRI, so the RDF-star reading has no generated blank node
/// and can be compared token for token.
const AMBIGUOUS_TTL: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "ex:jsmith ex:recorded << ex:occ1 ex:species ex:Swallow >> .\n",
    "<< ex:occ1 ex:species ex:Swallow >> ex:certainty ex:high .\n",
);

/// A Turtle file in the **RDF 1.2** surface, using only what RDF 1.2 can say:
/// a triple term in object position, a reifier with an explicit label, and a
/// `{| … |}` annotation. Nothing here parses as RDF-star at all.
const RDF12_TTL: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n",
    "ex:jsmith ex:recorded <<( ex:occ1 ex:species ex:Swallow )>> .\n",
    "_:r rdf:reifies <<( ex:occ2 ex:species ex:Robin )>> .\n",
    "_:r ex:source ex:fieldbook .\n",
    "ex:occ3 ex:species ex:Wren {| ex:certainty ex:low |} .\n",
);

/// The RDF-star Turtle every rete release before this flag read, and must go on
/// reading identically under the default.
const STAR_TTL: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "ex:jsmith ex:recorded << ex:occ1 ex:species ex:Swallow >> .\n",
    "ex:claim ex:states << ex:occ1 ex:count \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> >> .\n",
    "ex:note ex:about << ex:a ex:b \"a > b << c\"@en >> .\n",
);

/// [`STAR_TTL`] transcribed by hand into the RDF 1.2 surface — the same graph,
/// the other spelling.
///
/// By hand, and not by `replace("<< ", "<<( ")`, because the third statement's
/// literal *contains* `<<` and a textual rewrite corrupts it. That is not a
/// contrived detail: it is why this translation belongs in a parser and not in
/// sed, and the first draft of this test made exactly that mistake.
const STAR_TTL_AS_RDF12: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "ex:jsmith ex:recorded <<( ex:occ1 ex:species ex:Swallow )>> .\n",
    "ex:claim ex:states <<( ex:occ1 ex:count \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> )>> .\n",
    "ex:note ex:about <<( ex:a ex:b \"a > b << c\"@en )>> .\n",
);

/// No quoted triples anywhere — the control for \"nothing else changed\".
const PLAIN_TTL: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n",
    "ex:a rdf:type ex:T ;\n",
    "  ex:p \"x\"@en ;\n",
    "  ex:n \"5\"^^<http://www.w3.org/2001/XMLSchema#integer> .\n",
    "ex:c ex:p ex:d .\n",
);

/// The same control as TriG, so the named-graph reader is covered too.
const PLAIN_TRIG: &str = concat!(
    "@prefix ex: <http://ex/> .\n",
    "ex:a ex:p \"x\"@en .\n",
    "ex:g { ex:c ex:p ex:d . ex:c ex:q \"y\" . }\n",
);

// --- plumbing --------------------------------------------------------------

fn build_ttl(dir: &std::path::Path, name: &str, text: &str, surface: &str) -> std::path::PathBuf {
    let src = dir.join(name);
    let out = dir.join(format!("{name}.rete"));
    std::fs::write(&src, text).unwrap();
    common::build(
        &src,
        &out,
        &["--no-pyramid", "--quoted-triple-syntax", surface],
    );
    out
}

fn try_build(dir: &std::path::Path, name: &str, text: &str, surface: &str) -> std::process::Output {
    let src = dir.join(name);
    let out = dir.join(format!("{name}.rete"));
    std::fs::write(&src, text).unwrap();
    common::rete()
        .arg("build")
        .arg(&src)
        .arg("-o")
        .arg(&out)
        .args(["--no-pyramid", "--quoted-triple-syntax", surface])
        .output()
        .unwrap()
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

/// A graph's order-independent identity: sorted N-Quads in the RDF-star
/// surface, which is the stored token, so two files compare term for term.
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

/// [`identity`] with every blank-node label replaced by `_:B`.
///
/// oxttl labels an *anonymous* blank node — `[ … ]`, a collection, and now an
/// RDF 1.2 reifier written without a label — with a fresh random id on every
/// parse. That predates this flag (it is why the two-pass streaming build
/// refuses Turtle) and it means a graph containing one has no stable token
/// identity. Structure is still comparable, and structure is the claim.
fn identity_modulo_bnodes(file: &std::path::Path) -> Vec<String> {
    let mut v: Vec<String> = identity(file)
        .into_iter()
        .map(|l| {
            let mut out = String::with_capacity(l.len());
            let mut rest = l.as_str();
            while let Some(i) = rest.find("_:") {
                out.push_str(&rest[..i]);
                out.push_str("_:B");
                rest = &rest[i + 2..];
                let end = rest
                    .find(|c: char| !c.is_ascii_alphanumeric())
                    .unwrap_or(rest.len());
                rest = &rest[end..];
            }
            out.push_str(rest);
            out
        })
        .collect();
    v.sort();
    v
}

// --- 1. the round-trip matrix ----------------------------------------------

/// **The acceptance bar.** Take one graph, write it in each output surface,
/// read each dump back under the matching input surface, and require all four
/// combinations to be the same graph as the original.
///
/// The matrix is (input surface × output surface) and it has to be exactly
/// that: a dump is written in one surface and must be read in that surface.
/// Reading it in the other is not a cell of this matrix, it is the ambiguity
/// test below — and for the `rdf12` dump it is a hard error, which is the
/// point.
#[test]
fn round_trip_matrix_closes() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    // The source graph, read in each input surface from a file written in that
    // surface. Both must land the same graph — the superset claim, stated as a
    // test: `<<( s p o )>>` and `<< s p o >>` denote the same term, differently
    // spelled.
    let from_star = build_ttl(dir, "src-star.ttl", STAR_TTL, "rdf-star");
    let from_12 = build_ttl(dir, "src-12.ttl", STAR_TTL_AS_RDF12, "rdf12");
    assert_eq!(
        identity(&from_star),
        identity(&from_12),
        "the same graph written in the two input surfaces must land one graph"
    );

    let want = identity(&from_star);

    let mut seen = Vec::new();
    for in_surface in ["rdf-star", "rdf12"] {
        let source = if in_surface == "rdf-star" {
            &from_star
        } else {
            &from_12
        };
        for out_surface in ["rdf-star", "rdf12"] {
            for fmt in ["nq", "ttl", "trig"] {
                let dump = export(
                    source,
                    &["--format", fmt, "--quoted-triple-syntax", out_surface],
                );
                // The dump is read back in the surface it was WRITTEN in. That
                // pairing is the whole contract.
                let back = build_ttl(
                    dir,
                    &format!("rt-{in_surface}-{out_surface}-{fmt}.{fmt}"),
                    &dump,
                    out_surface,
                );
                assert_eq!(
                    identity(&back),
                    want,
                    "round trip in={in_surface} out={out_surface} fmt={fmt} changed the graph"
                );
                seen.push((in_surface, out_surface, fmt));
            }
        }
    }
    assert_eq!(seen.len(), 12, "every cell of the matrix must have run");
}

/// The bug that started this: `rete export --format trig` writes `rdf12` by
/// default, and `rete build` could not read it back at all.
///
/// Both halves are asserted, because both are the feature: with the flag it
/// round-trips, and *without* it the failure names the flag rather than
/// complaining about a stray `(`.
#[test]
fn retes_own_default_trig_output_is_re_ingestible() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let file = build_ttl(dir, "own.ttl", STAR_TTL, "rdf-star");

    // Default export: RDF 1.2 triple terms.
    let dump = export(&file, &["--format", "trig"]);
    assert!(
        dump.contains("<<("),
        "the default TriG export should carry RDF 1.2 triple terms:\n{dump}"
    );

    let back = build_ttl(dir, "own-back.trig", &dump, "rdf12");
    assert_eq!(
        identity(&back),
        identity(&file),
        "rete's own default TriG output must re-ingest to the same graph"
    );

    // And the default input surface refuses it, loudly and by name.
    let out = try_build(dir, "own-default.trig", &dump, "rdf-star");
    assert!(!out.status.success(), "reading it as RDF-star must fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--quoted-triple-syntax rdf12"),
        "the refusal must name the flag that fixes it:\n{err}"
    );
    assert!(
        err.contains("<<("),
        "the refusal must say what it saw:\n{err}"
    );
}

/// The same round trip with **no flag at all** on either side, which is the
/// path a user who never reads this documentation takes: export the RDF-star
/// surface and `rete build` reads it back with no argument.
#[test]
fn rdf_star_export_re_ingests_with_no_flag() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let file = build_ttl(dir, "noflag.ttl", STAR_TTL, "rdf-star");
    for fmt in ["nq", "ttl", "trig"] {
        let dump = export(
            &file,
            &["--format", fmt, "--quoted-triple-syntax", "rdf-star"],
        );
        let src = dir.join(format!("noflag-back.{fmt}"));
        let out = dir.join(format!("noflag-back-{fmt}.rete"));
        std::fs::write(&src, &dump).unwrap();
        common::build(&src, &out, &["--no-pyramid"]);
        assert_eq!(
            identity(&out),
            identity(&file),
            "{fmt}: an rdf-star dump must re-ingest with no flag"
        );
    }
}

// --- 2. the ambiguity, asserted from both sides ----------------------------

/// One Turtle file, two surfaces, two DIFFERENT graphs — asserted, because this
/// is the ambiguity the whole feature exists to resolve, and a test that only
/// checked \"both parse\" would pass while quietly reading everything wrong.
#[test]
fn the_same_turtle_parses_into_two_different_graphs() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();

    let star = build_ttl(dir, "amb-star.ttl", AMBIGUOUS_TTL, "rdf-star");
    let twelve = build_ttl(dir, "amb-12.ttl", AMBIGUOUS_TTL, "rdf12");

    let star_id = identity(&star);
    let twelve_id = identity_modulo_bnodes(&twelve);
    assert_ne!(
        star_id, twelve_id,
        "the two surfaces must disagree about this file — that is the point"
    );

    // Under RDF-star: two statements, each carrying the quoted triple as a
    // term. No blank node, no `rdf:reifies`, nothing invented.
    assert_eq!(star_id.len(), 2, "rdf-star reading:\n{star_id:#?}");
    assert!(
        star_id.iter().all(|l| l.contains("<<<http://ex/occ1>")),
        "rdf-star must keep the quoted triple as a term:\n{star_id:#?}"
    );
    assert!(
        !star_id.iter().any(|l| l.contains(RDF_REIFIES)),
        "rdf-star must not invent a reification:\n{star_id:#?}"
    );
    assert!(
        !star_id.iter().any(|l| l.contains("_:")),
        "rdf-star must not invent a blank node:\n{star_id:#?}"
    );

    // Under RDF 1.2: each `<< … >>` is a reifier, so each input line becomes
    // TWO statements — the `rdf:reifies` fact and the statement about the
    // reifier — and a blank node appears that the file never wrote.
    assert_eq!(twelve_id.len(), 4, "rdf12 reading:\n{twelve_id:#?}");
    assert_eq!(
        twelve_id.iter().filter(|l| l.contains(RDF_REIFIES)).count(),
        2,
        "rdf12 must reify both occurrences:\n{twelve_id:#?}"
    );
    assert!(
        twelve_id.iter().all(|l| l.contains("_:B")),
        "every rdf12 statement here should involve the reifier blank node:\n{twelve_id:#?}"
    );
    // The reified triple itself is stored as rete's canonical quoted-triple
    // token — the superset claim, visible: a triple term IS a term here.
    assert!(
        twelve_id.iter().any(|l| l.contains(&format!(
            "_:B {RDF_REIFIES} <<<http://ex/occ1> <http://ex/species> <http://ex/Swallow>>>"
        ))),
        "the triple term must land as rete's stored token:\n{twelve_id:#?}"
    );
}

/// The RDF 1.2 surface reads what only RDF 1.2 can say — triple terms, labelled
/// reifiers, and `{| … |}` annotations — into ordinary rete statements, with no
/// storage change of any kind.
#[test]
fn rdf12_constructs_land_as_ordinary_statements() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let file = build_ttl(dir, "r12.ttl", RDF12_TTL, "rdf12");
    let id = identity_modulo_bnodes(&file);

    // The triple term, in object position, as a term.
    assert!(
        id.iter().any(|l| l
            == "<http://ex/jsmith> <http://ex/recorded> \
                <<<http://ex/occ1> <http://ex/species> <http://ex/Swallow>>> ."),
        "triple term:\n{id:#?}"
    );
    // The explicitly labelled reifier keeps its label — `_:r` is written in the
    // file, so it is not one of the invented ones.
    assert!(
        identity(&file).iter().any(|l| l.starts_with("_:r ")),
        "an explicit reifier label must survive:\n{:#?}",
        identity(&file)
    );
    // The annotation: the annotated statement itself, plus a reifier carrying
    // the annotation's own property.
    assert!(
        id.iter()
            .any(|l| l == "<http://ex/occ3> <http://ex/species> <http://ex/Wren> ."),
        "an annotated statement is still asserted:\n{id:#?}"
    );
    assert!(
        id.iter()
            .any(|l| l.contains("<http://ex/certainty> <http://ex/low>")),
        "the annotation's own statement must be there:\n{id:#?}"
    );
    assert_eq!(
        id.iter().filter(|l| l.contains(RDF_REIFIES)).count(),
        2,
        "one reifier for `_:r`, one for the annotation:\n{id:#?}"
    );
}

/// `rete validate` takes the same flag with the same meaning — including the
/// count it reports, which differs between the surfaces for the same file
/// because the graphs differ.
#[test]
fn validate_takes_the_same_flag() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("amb.ttl");
    std::fs::write(&src, AMBIGUOUS_TTL).unwrap();

    let run = |surface: &str| {
        let out = common::rete()
            .arg("validate")
            .arg(&src)
            .args(["--quoted-triple-syntax", surface])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "validate --quoted-triple-syntax {surface} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };

    assert!(
        run("rdf-star").contains("valid: 2 statement(s)"),
        "{}",
        run("rdf-star")
    );
    assert!(
        run("rdf12").contains("valid: 4 statement(s)"),
        "{}",
        run("rdf12")
    );
}

// --- 3. the default, and how loud it is ------------------------------------

/// The default input surface is `rdf-star`: existing Turtle sources keep
/// producing the graph they produce today, with no flag and no migration.
#[test]
fn the_default_input_surface_is_rdf_star() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let src = dir.join("default.ttl");
    std::fs::write(&src, AMBIGUOUS_TTL).unwrap();
    let out = dir.join("default.rete");
    common::build(&src, &out, &["--no-pyramid"]);

    let explicit = build_ttl(dir, "explicit.ttl", AMBIGUOUS_TTL, "rdf-star");
    assert_eq!(
        identity(&out),
        identity(&explicit),
        "no flag must mean --quoted-triple-syntax rdf-star"
    );
}

/// The mistake the default leaves possible is a **hard error**, not a wrong
/// graph. That asymmetry is the entire reason the input default is not `rdf12`
/// while the output default is: `<<(` belongs to RDF 1.2 alone, so an RDF 1.2
/// file read as RDF-star cannot parse — whereas an RDF-star file read as
/// RDF 1.2 parses perfectly into the wrong thing.
#[test]
fn an_rdf12_file_read_as_rdf_star_fails_naming_the_flag() {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in [("r12.ttl", RDF12_TTL), ("r12.trig", RDF12_TTL)] {
        let out = try_build(dir.path(), name, text, "rdf-star");
        assert!(!out.status.success(), "{name} should not parse as RDF-star");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(
            err.contains("--quoted-triple-syntax rdf12"),
            "{name}: the error must name the flag:\n{err}"
        );
    }
    // `rete validate` says the same thing — it is the cheap way to ask.
    let src = dir.path().join("v.ttl");
    std::fs::write(&src, RDF12_TTL).unwrap();
    let out = common::rete().arg("validate").arg(&src).output().unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--quoted-triple-syntax rdf12"),
        "validate must give the same hint"
    );
}

/// An unknown value is rejected by clap before anything is read.
#[test]
fn an_unknown_surface_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let out = try_build(dir.path(), "x.ttl", PLAIN_TTL, "rdf-1.2");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("rdf12") && err.contains("rdf-star"),
        "the refusal should list the values:\n{err}"
    );
}

// --- 4. nothing else moved -------------------------------------------------

/// A file with no quoted triples is the same graph and the same BYTES under
/// either input surface, in Turtle and in TriG. The standing guarantee: this
/// flag changes what `<< … >>` means and nothing else.
#[test]
fn files_without_quoted_triples_are_byte_identical_either_way() {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    for (name, text, fmt) in [
        ("plain.ttl", PLAIN_TTL, "ttl"),
        ("plain.trig", PLAIN_TRIG, "trig"),
    ] {
        let star = build_ttl(dir, &format!("star-{name}"), text, "rdf-star");
        let twelve = build_ttl(dir, &format!("12-{name}"), text, "rdf12");
        assert_eq!(
            identity(&star),
            identity(&twelve),
            "{fmt}: the two surfaces must agree on a file with no quoted triples"
        );
        // And the exports are byte-for-byte equal, in every output surface.
        for out_surface in ["rdf-star", "rdf12"] {
            for out_fmt in ["nq", "ttl", "trig"] {
                assert_eq!(
                    export(
                        &star,
                        &["--format", out_fmt, "--quoted-triple-syntax", out_surface]
                    ),
                    export(
                        &twelve,
                        &["--format", out_fmt, "--quoted-triple-syntax", out_surface]
                    ),
                    "{name} -> {out_fmt}/{out_surface} differed between input surfaces"
                );
            }
        }
        // The built files themselves, byte for byte — the strongest form of the
        // claim, and stronger than comparing digests of them.
        assert_eq!(
            std::fs::read(&star).unwrap(),
            std::fs::read(&twelve).unwrap(),
            "{name}: the built .rete must be byte-identical either way"
        );
    }
}

/// N-Triples and N-Quads ignore the flag entirely: their reader is rete's own
/// and has taken both spellings since #262, so neither value may change what
/// they produce.
#[test]
fn line_based_formats_ignore_the_flag() {
    const NQ_STAR: &str =
        "<http://ex/a> <http://ex/b> << <http://ex/s> <http://ex/p> <http://ex/o> >> .\n";
    const NQ_12: &str =
        "<http://ex/a> <http://ex/b> <<( <http://ex/s> <http://ex/p> <http://ex/o> )>> .\n";
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    let mut built = Vec::new();
    for (i, text) in [NQ_STAR, NQ_12].iter().enumerate() {
        for surface in ["rdf-star", "rdf12"] {
            built.push(build_ttl(
                dir,
                &format!("nq{i}-{surface}.nq"),
                text,
                surface,
            ));
        }
    }
    let first = identity(&built[0]);
    for f in &built[1..] {
        assert_eq!(
            identity(f),
            first,
            "N-Quads must read the same under either surface and either spelling"
        );
    }
}
