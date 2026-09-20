#![no_main]
//! Fuzz the IRI classifier and sanitizer on arbitrary bytes.
//!
//! The classifier runs over every term of every ingest, so it sees whatever a
//! harvested dump contains — truncated UTF-8 sequences, half-written escapes,
//! IRIs that are nothing but punctuation. None of it may panic, and the
//! sanitizer's contract must hold on all of it:
//!
//! * **a repair lands valid** — if `sanitize_iri_content` returns something, the
//!   result must itself be defect-free, or the exporter would be claiming a fix
//!   that leaves the dump unloadable;
//! * **a repair is idempotent** — sanitizing the result must be a no-op;
//! * **a valid IRI is never rewritten** — the one failure mode a sanitizer must
//!   not have;
//! * **`sanitize` agrees with `term_defect`** on whether a term was clean.
//!
//! `cargo +nightly fuzz run iri`.

use libfuzzer_sys::fuzz_target;
use rete_core::iri::{
    iri_content_defect, sanitize_iri_content, sanitize_term, term_defect, term_defects,
};

fuzz_target!(|data: &[u8]| {
    let Ok(s) = std::str::from_utf8(data) else {
        return;
    };

    // --- IRIREF content -----------------------------------------------------
    let defect = iri_content_defect(s);
    match sanitize_iri_content(s) {
        None => {
            // Nothing was rewritten. Either it was already fine, or the defect
            // is one escaping cannot repair — never "repairable but skipped".
            if let Some(d) = defect {
                assert!(!d.repairable(), "repairable defect {d:?} left unrepaired: {s:?}");
            }
        }
        Some(fixed) => {
            assert!(defect.is_some(), "rewrote an IRI with no defect: {s:?}");
            assert_eq!(
                iri_content_defect(&fixed),
                None,
                "repair did not land valid: {s:?} -> {fixed:?}"
            );
            assert_eq!(
                sanitize_iri_content(&fixed),
                None,
                "repair is not idempotent: {s:?} -> {fixed:?}"
            );
        }
    }

    // --- whole term token ---------------------------------------------------
    // `<iri>`, `"lit"^^<dt>`, `<<s p o>>` — every IRI a term can hide.
    let all = term_defects(s);
    // `term_defect` is the cheap "is it broken at all" check; the two must agree
    // on that question even though one stops at the first reason.
    assert_eq!(
        term_defect(s).is_some(),
        !all.is_empty(),
        "term_defect and term_defects disagree: {s:?}"
    );
    match sanitize_term(s) {
        None => {
            // Nothing claimed. Legitimate only when the term was already clean,
            // or when SOMETHING in it cannot be repaired — a quoted triple is
            // all-or-nothing, so one bad component holds back the rest.
            assert!(
                all.is_empty() || all.iter().any(|d| !d.repairable()),
                "every defect was repairable yet nothing was repaired: {s:?} {all:?}"
            );
        }
        Some(fixed) => {
            assert!(!all.is_empty(), "rewrote a clean term: {s:?}");
            assert!(
                all.iter().all(|d| d.repairable()),
                "claimed a repair over an unrepairable defect: {s:?} {all:?}"
            );
            assert_eq!(
                term_defect(&fixed),
                None,
                "term repair did not land valid: {s:?} -> {fixed:?}"
            );
            assert_eq!(
                sanitize_term(&fixed),
                None,
                "term repair is not idempotent: {s:?} -> {fixed:?}"
            );
        }
    }

    // The same bytes wrapped as an IRIREF: `<` and `>` are forbidden inside, so
    // this mostly exercises the token path's bracket handling.
    let wrapped = format!("<{}>", s.replace(['<', '>'], ""));
    let _ = term_defect(&wrapped);
    let _ = sanitize_term(&wrapped);
});
