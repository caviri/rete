//! SPARQL `REGEX` / `REPLACE` with non-ASCII letters.
//!
//! `regex-lite` (the engine every build used) treats `\b`, `\w`, `\d` and
//! `(?i)` as ASCII-only, so `REGEX(?name, "\\bsvobodová\\b", "i")` matched
//! nothing: "á" is not an ASCII word character, so there is no boundary after
//! it. With the `unicode-regex` feature (default; every native build) the
//! engine is `regex` with the Unicode Perl classes and case folding. A build
//! without it (the browser engine, to keep its size) keeps the ASCII behavior,
//! which the second test pins down so the difference stays a documented one.

use rete_core::ingest::{assemble_dataset, parse_statements};
use rete_core::{eval_sparql, Rete};

fn rete() -> Rete {
    let nt = "<http://ex/a> <http://ex/name> \"Jitka Svobodová\" .\n\
              <http://ex/b> <http://ex/name> \"SVOBODOVÁ, J.\" .\n\
              <http://ex/c> <http://ex/name> \"Svobodováová\" .\n\
              <http://ex/d> <http://ex/name> \"Ludmila Dömeová\" .\n\
              <http://ex/e> <http://ex/name> \"plain ascii\" .\n";
    let (bytes, _) = assemble_dataset(parse_statements(nt, "nt").unwrap(), &[]);
    Rete::open(&bytes).unwrap()
}

fn subjects(rete: &Rete, filter: &str) -> Vec<String> {
    let q = format!("SELECT ?s WHERE {{ ?s <http://ex/name> ?n FILTER({filter}) }}");
    let (_, sols) = eval_sparql(rete, &q).unwrap();
    let mut v: Vec<String> = sols.iter().map(|b| b["s"].clone()).collect();
    v.sort();
    v
}

fn ex(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| format!("<http://ex/{n}>")).collect()
}

#[cfg(feature = "unicode-regex")]
#[test]
fn word_boundaries_classes_and_case_folding_are_unicode() {
    let rete = rete();
    // A boundary after "á", and "Á" folding to "á"; not inside a longer word.
    assert_eq!(
        subjects(&rete, r#"REGEX(?n, "\\bsvobodová\\b", "i")"#),
        ex(&["a", "b"])
    );
    // `\w` covers "ö" and "á": the whole surname is one word.
    assert_eq!(
        subjects(&rete, r#"REGEX(?n, "^\\w+ \\w+$")"#),
        ex(&["a", "d", "e"])
    );
    assert_eq!(
        subjects(&rete, r#"REGEX(?n, "\\bdömeová$", "i")"#),
        ex(&["d"])
    );
    // REPLACE uses the same engine.
    let (_, sols) = eval_sparql(
        &rete,
        r#"SELECT ?r WHERE { <http://ex/a> <http://ex/name> ?n BIND(REPLACE(?n, "\\bsvobodová\\b", "S.", "i") AS ?r) }"#,
    )
    .unwrap();
    assert_eq!(sols[0]["r"], "\"Jitka S.\"");
}

#[cfg(not(feature = "unicode-regex"))]
#[test]
fn without_the_feature_classes_stay_ascii() {
    let rete = rete();
    // ASCII `\b` sees no boundary after "á" (both sides non-word), so it
    // misses the two names — and sees one between "á" and "o" inside
    // "Svobodováová", a false hit. `(?i)` does not fold "Á" either.
    assert_eq!(
        subjects(&rete, r#"REGEX(?n, "\\bsvobodová\\b", "i")"#),
        ex(&["c"])
    );
    assert_eq!(subjects(&rete, r#"REGEX(?n, "^\\w+ \\w+$")"#), ex(&["e"]));
}

/// What does not depend on the engine: a literal pattern is a substring test
/// with full Unicode case folding either way.
#[test]
fn a_literal_pattern_folds_unicode_case_in_every_build() {
    let rete = rete();
    assert_eq!(
        subjects(&rete, r#"REGEX(?n, "svobodová", "i")"#),
        ex(&["a", "b", "c"])
    );
}
