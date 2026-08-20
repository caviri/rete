//! How the CLI *says* it found invalid IRIs — the build-time warning and the
//! export-time sanitize summary.
//!
//! Both read the same [`IriReport`] (`rete_core::iri`), and both go to **stderr**:
//! `rete export` writes the dump to stdout, so a report on stdout would corrupt
//! the very file it is describing.
//!
//! The wording is deliberate on two points the issue this fixes turned on:
//!
//! * **A build warning is not a build failure.** Datasets that build today —
//!   including published ones — must keep building, so the default records the
//!   damage and continues. `--strict` is the flag for the other choice.
//! * **Sanitizing is lossy, and says so.** Percent-encoding changes the IRI, so
//!   a sanitized dump no longer joins against the graph it came from. The
//!   summary states that every time it does anything, rather than reading as a
//!   clean bill of health.

use rete_core::iri::IriReport;

/// One indented `count  reason` row per non-empty class, with an example.
fn class_rows(report: &IriReport, indent: &str, with_examples: bool) {
    for (defect, count, sample) in report.classes() {
        let tail = if defect.repairable() {
            ""
        } else {
            "   [NOT repairable by escaping]"
        };
        eprintln!("{indent}{count:>7}  {}{tail}", defect.reason());
        if with_examples {
            if let Some(s) = sample {
                eprintln!("{indent}         e.g. {}", elide(s));
            }
        }
    }
}

/// Keep a sample short enough to stay on one line — some real offenders are
/// 400-character harvested PDF URLs.
fn elide(token: &str) -> String {
    const MAX: usize = 96;
    if token.chars().count() <= MAX {
        return token.to_string();
    }
    let head: String = token.chars().take(MAX - 1).collect();
    format!("{head}…")
}

/// The build-time warning: what was ingested, what it costs downstream, and the
/// two flags that do something about it. Silent when the input was clean.
pub(crate) fn warn_after_build(report: &IriReport) {
    if report.is_empty() {
        return;
    }
    eprintln!(
        "warning: {} statement(s) carry an invalid IRI ({} IRI occurrence(s)).",
        report.statements(),
        report.occurrences()
    );
    eprintln!(
        "         They are stored verbatim, so `rete export --format nq` emits N-Quads that a"
    );
    eprintln!(
        "         strict parser (Oxigraph, Jena, rapper) rejects — and a bulk loader rejects the"
    );
    eprintln!("         whole chunk, not the line.");
    class_rows(report, "         ", true);
    eprintln!("         `rete export --sanitize-iris` percent-encodes the repairable ones (which");
    eprintln!("         changes the IRI); `rete build --strict` refuses such input outright.");
}

/// The export-time summary for `--sanitize-iris`. Unlike the build warning this
/// prints even when it found nothing — the flag was asked for explicitly, and
/// "nothing needed changing" is the answer to that question.
pub(crate) fn report_sanitized(report: &IriReport) {
    if report.is_empty() {
        eprintln!(
            "--sanitize-iris: no invalid IRIs found; the dump is byte-identical to a plain export."
        );
        return;
    }
    if report.repaired() > 0 {
        eprintln!(
            "--sanitize-iris: percent-encoded {} IRI occurrence(s). The dump's IRIs are NOT the",
            report.repaired()
        );
        eprintln!("                 file's IRIs: it no longer joins against the source graph, and");
        eprintln!("                 rete → store → rete is no longer the identity.");
        for (defect, count, _) in report.classes().filter(|(d, _, _)| d.repairable()) {
            eprintln!("                 {count:>7}  {}", defect.reason());
        }
    }
    if report.unrepairable() > 0 {
        eprintln!(
            "--sanitize-iris: {} occurrence(s) CANNOT be repaired by escaping and were written",
            report.unrepairable()
        );
        eprintln!(
            "                 verbatim — this dump is still not valid N-Quads. Fix them at the"
        );
        eprintln!(
            "                 source; a relative IRI needs a base IRI the file never recorded."
        );
        for (defect, count, sample) in report.classes().filter(|(d, _, _)| !d.repairable()) {
            eprintln!("                 {count:>7}  {}", defect.reason());
            if let Some(s) = sample {
                eprintln!("                          e.g. {}", elide(s));
            }
        }
    }
}
