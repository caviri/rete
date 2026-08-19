mod common;

use predicates::prelude::*;

#[test]
fn ordinary_and_external_builds_report_common_phases() {
    let fixture = common::fixture();
    let ordinary = fixture.path("ordinary.rete");
    let external = fixture.path("external.rete");
    let external_source = fixture.write(
        "external.nt",
        "<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .\n",
    );

    for (source, output, args) in [
        (&fixture.source, &ordinary, vec!["--no-pyramid"]),
        (
            &external_source,
            &external,
            vec!["--memory-budget-mb", "16", "--no-pyramid"],
        ),
    ] {
        common::rete()
            .env("RETE_BUILD_TIMING", "1")
            .arg("build")
            .arg(source)
            .arg("-o")
            .arg(output)
            .args(args)
            .assert()
            .success()
            .stderr(
                predicate::str::contains("  [build] parse+ingest:")
                    .and(predicate::str::contains("  [build] canonicalize:"))
                    .and(predicate::str::contains("  [build] index families:"))
                    .and(predicate::str::contains("  [build] final write:"))
                    .and(predicate::str::contains("  [build] total:")),
            );
    }
}
