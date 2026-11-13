//! `rete convert` writes the source's community pyramid as it is: the same
//! communities, superedges and descriptors, whatever algorithm made them, and
//! through the memory-bounded builder (a source with a pyramid, or with
//! per-graph named graphs, is not rebuilt in memory).

mod common;

use std::path::Path;

use common::rete;

/// Three classes of entities in a ring, with labels, a literal, and two named
/// graphs.
fn nquads() -> String {
    let mut s = String::new();
    for i in 0..300 {
        s.push_str(&format!(
            "<http://example.test/e{i}> <http://www.w3.org/2000/01/rdf-schema#label> \"Entity number {i}\"@en .\n\
             <http://example.test/e{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/T{}> .\n\
             <http://example.test/e{i}> <http://example.test/note> \"the quick brown fox {i} jumps\" .\n\
             <http://example.test/e{i}> <http://example.test/next> <http://example.test/e{}> .\n\
             <http://example.test/e{i}> <http://example.test/far> <http://example.test/e{}> .\n",
            i % 3,
            (i + 1) % 300,
            (i * 7 + 11) % 300
        ));
    }
    for i in 0..20 {
        s.push_str(&format!(
            "<http://example.test/e{i}> <http://example.test/in> <http://example.test/e{}> <http://example.test/g{}> .\n",
            i + 1,
            i % 2
        ));
    }
    s
}

fn run(args: &[&str], envs: &[(&str, &str)]) -> (String, String) {
    let mut c = rete();
    c.env_remove("RETE_WRITE_FORMAT")
        .env("RETE_BUILD_RECORD", "reproducible")
        .env("SOURCE_DATE_EPOCH", "1700000000");
    for (k, v) in envs {
        c.env(k, v);
    }
    let o = c.args(args).output().unwrap();
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    assert!(o.status.success(), "{args:?}: {err}");
    (String::from_utf8_lossy(&o.stdout).to_string(), err)
}

/// The lines of `rete summary`, sorted: the order of the superedges follows
/// the predicate ids, which a conversion changes.
fn summary(file: &Path) -> Vec<String> {
    let (out, _) = run(&["summary", file.to_str().unwrap()], &[]);
    assert!(out.starts_with("pyramid round"), "{out}");
    let mut lines: Vec<String> = out.lines().map(str::to_string).collect();
    lines.sort();
    lines
}

fn answer(file: &Path, query: &str) -> String {
    run(&["sparql", file.to_str().unwrap(), query, "--json"], &[]).0
}

/// The header's pyramid rounds, as `rete info` prints them.
fn levels(file: &Path) -> String {
    let (info, _) = run(&["info", file.to_str().unwrap()], &[]);
    info.lines()
        .find(|l| l.trim_start().starts_with("pyramid") && l.contains("level"))
        .map(|l| l.split(':').nth(1).unwrap().trim().to_string())
        .unwrap_or_else(|| panic!("no pyramid line in {info}"))
}

fn check(name: &str, build: &[&str]) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nq");
    std::fs::write(&src, nquads()).unwrap();
    let old = dir.path().join("old.rete");
    let out = dir.path().join("out.rete");
    let mut args = vec!["build", src.to_str().unwrap(), "-o", old.to_str().unwrap()];
    args.extend_from_slice(build);
    run(&args, &[("RETE_WRITE_FORMAT", "5")]);
    assert_eq!(std::fs::read(&old).unwrap()[4], 5, "{name}");
    let (_, err) = run(
        &[
            "convert",
            old.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--no-card-costs",
            "--memory-budget-mb",
            "64",
        ],
        &[],
    );
    assert!(err.contains("memory-bounded builder"), "{name}: {err}");
    assert!(err.contains("pyramid carried over"), "{name}: {err}");
    assert!(!err.contains("in memory"), "{name}: {err}");
    assert_eq!(std::fs::read(&out).unwrap()[4], 8, "{name}");

    let (before, after) = (summary(&old), summary(&out));
    assert!(before.len() > 3, "{name}: {before:?}");
    assert_eq!(before, after, "{name}: the summary changed");
    assert_eq!(levels(&old), levels(&out), "{name}");
    for q in [
        "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }",
        "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }",
        "SELECT ?g (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } } GROUP BY ?g ORDER BY ?g",
        "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> \"Entity number 7\"@en }",
    ] {
        assert_eq!(answer(&old, q), answer(&out, q), "{name}: {q}");
    }
    let v = run(&["verify", out.to_str().unwrap()], &[]).0;
    assert!(v.contains("OK"), "{name}: {v}");
}

#[test]
fn a_type_partition_pyramid_is_carried_over() {
    check(
        "types",
        &["--pyramid-algo", "types", "--card", "--no-card-costs"],
    );
}

#[test]
fn a_louvain_pyramid_is_carried_over() {
    check("louvain", &["--card", "--no-card-costs", "--text-index"]);
}

#[test]
fn a_pyramid_beside_per_graph_named_graphs_is_carried_over() {
    check(
        "per-graph",
        &[
            "--pyramid-algo",
            "types",
            "--named-graphs",
            "per-graph",
            "--card",
            "--no-card-costs",
        ],
    );
}

/// A file as 0.3.x wrote it has its labels in the pyramid's legacy block
/// only. It gets a label index, and `--no-label-index` leaves it without one.
#[test]
fn the_legacy_label_block_becomes_a_label_index() {
    use rete_core::ingest::{assemble_dataset_opts, AssembleOptions, RawQuad};
    let quads: Vec<RawQuad> = (0..200)
        .flat_map(|i| {
            let s = format!("<http://example.test/e{i}>");
            [
                (
                    s.clone(),
                    "<http://www.w3.org/2000/01/rdf-schema#label>".to_string(),
                    format!("\"Entity number {i}\""),
                    None,
                ),
                (
                    s,
                    "<http://example.test/next>".to_string(),
                    format!("<http://example.test/e{}>", (i + 1) % 200),
                    None,
                ),
            ]
        })
        .collect();
    let image = assemble_dataset_opts(quads, &AssembleOptions::legacy(), |_, _| Vec::new()).0;
    assert_eq!(image[4], 5);
    let legacy = rete_core::Rete::open(&image).unwrap();
    assert!(!legacy.label_index().is_empty() && !legacy.has_label_index());
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.rete");
    std::fs::write(&old, &image).unwrap();
    for (flag, want) in [(None, true), (Some("--no-label-index"), false)] {
        let out = dir.path().join(format!("out-{want}.rete"));
        let mut args = vec![
            "convert",
            old.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--no-card-costs",
        ];
        args.extend(flag);
        run(&args, &[]);
        let bytes = std::fs::read(&out).unwrap();
        let converted = rete_core::Rete::open(&bytes).unwrap();
        assert_eq!(converted.has_label_index(), want, "{flag:?}");
        assert!(
            converted.label_index().is_empty(),
            "the block is not carried"
        );
        assert_eq!(summary(&old), summary(&out), "{flag:?}");
    }
}

/// A source whose schema was typed with another predicate than `rdf:type`
/// (`build --type-predicate`) keeps that typing: a pyramid-meta does not name
/// the predicate, so `convert` finds it from the classes of the rollup.
#[test]
fn the_type_predicate_of_the_source_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("in.nt");
    let mut nt = String::new();
    for i in 0..120 {
        nt.push_str(&format!(
            "<http://example.test/e{i}> <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <http://example.test/Item> .\n\
             <http://example.test/e{i}> <http://example.test/kind> <http://example.test/K{}> .\n\
             <http://example.test/e{i}> <http://example.test/next> <http://example.test/e{}> .\n",
            i % 4,
            (i + 1) % 120
        ));
    }
    std::fs::write(&src, nt).unwrap();
    let old = dir.path().join("old.rete");
    let out = dir.path().join("out.rete");
    run(
        &[
            "build",
            src.to_str().unwrap(),
            "-o",
            old.to_str().unwrap(),
            "--type-predicate",
            "<http://example.test/kind>",
        ],
        &[("RETE_WRITE_FORMAT", "5")],
    );
    let (_, err) = run(
        &[
            "convert",
            old.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--no-card-costs",
        ],
        &[],
    );
    assert!(
        err.contains("schema typed by <http://example.test/kind>"),
        "{err}"
    );
    let rollup = |f: &Path| {
        let (s, _) = run(&["summary", f.to_str().unwrap()], &[]);
        s.lines()
            .find(|l| l.trim_start().starts_with("level 0"))
            .map(|l| l.split("):").nth(1).unwrap().trim().to_string())
            .unwrap_or_else(|| panic!("no rollup in {s}"))
    };
    assert!(
        rollup(&old).contains("<http://example.test/K0>×30"),
        "{}",
        rollup(&old)
    );
    assert_eq!(rollup(&old), rollup(&out));
}

/// The same for a file as 0.3.x wrote it, whose pyramid-meta holds the schema
/// and does not name the predicate: the build found `kind` (there is no
/// `rdf:type`), and `convert` finds it again from the rollup's classes.
#[test]
fn the_type_predicate_is_found_for_a_pyramid_meta_schema() {
    use rete_core::ingest::{assemble_dataset_opts, AssembleOptions, RawQuad};
    let quads: Vec<RawQuad> = (0..200)
        .flat_map(|i| {
            let s = format!("<http://example.test/e{i}>");
            [
                (
                    s.clone(),
                    "<http://example.test/kind>".to_string(),
                    format!("<http://example.test/K{}>", i % 4),
                    None,
                ),
                (
                    s,
                    "<http://example.test/next>".to_string(),
                    format!("<http://example.test/e{}>", (i + 1) % 200),
                    None,
                ),
            ]
        })
        .collect();
    let image = assemble_dataset_opts(quads, &AssembleOptions::legacy(), |_, _| Vec::new()).0;
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.rete");
    let out = dir.path().join("out.rete");
    std::fs::write(&old, &image).unwrap();
    let rollup = |f: &Path| {
        let (s, _) = run(&["summary", f.to_str().unwrap()], &[]);
        s.lines()
            .find(|l| l.trim_start().starts_with("level 0"))
            .map(|l| l.split("):").nth(1).unwrap().trim().to_string())
            .unwrap_or_else(|| panic!("no rollup in {s}"))
    };
    assert!(
        rollup(&old).contains("<http://example.test/K0>×50"),
        "{}",
        rollup(&old)
    );
    let (_, err) = run(
        &[
            "convert",
            old.to_str().unwrap(),
            "-o",
            out.to_str().unwrap(),
            "--no-card-costs",
        ],
        &[],
    );
    assert!(
        err.contains("schema typed by <http://example.test/kind>"),
        "{err}"
    );
    assert_eq!(rollup(&old), rollup(&out));
}
