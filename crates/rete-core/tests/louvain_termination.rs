//! Louvain must terminate: graphs on which `build` never returned in rete 0.3.3
//! and earlier (see `pyramid::louvain_one_level`). Every level of every
//! fixture must converge on its own, well inside `MAX_LOUVAIN_PASSES`, and
//! the whole build must return. Each check runs on a worker thread under a
//! time limit, so a regression fails here instead of hanging the suite.
//!
//! Fixtures (`tests/fixtures/louvain-hangs/`):
//! - `min-15-level0.nt`, `min-18-level1.nt`: the smallest hangs found by the
//!   random-graph harness, one in the base graph and one in the aggregated
//!   level above it;
//! - `jev-*.nt`: reported by the Jev Games consumer (a 145-triple Commander
//!   position and two random graphs of 30 and 63 triples).

use std::sync::mpsc;
use std::time::Duration;

use rete_core::ingest::{assemble_dataset, parse_statements};
use rete_core::{
    build_dendrogram, louvain_one_level_stats, project_graph, DictionaryBuilder, MAX_LOUVAIN_PASSES,
};

const FIXTURES: &[(&str, &str)] = &[
    (
        "min-15-level0",
        include_str!("fixtures/louvain-hangs/min-15-level0.nt"),
    ),
    (
        "min-18-level1",
        include_str!("fixtures/louvain-hangs/min-18-level1.nt"),
    ),
    (
        "jev-commander-145",
        include_str!("fixtures/louvain-hangs/jev-commander-145.nt"),
    ),
    (
        "jev-random30-30",
        include_str!("fixtures/louvain-hangs/jev-random30-30.nt"),
    ),
    (
        "jev-random63-63",
        include_str!("fixtures/louvain-hangs/jev-random63-63.nt"),
    ),
];

/// Run `f` on a worker thread; fail if it has not returned within `secs`.
fn within<T: Send + 'static>(what: &str, secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        tx.send(f()).ok();
    });
    rx.recv_timeout(Duration::from_secs(secs))
        .unwrap_or_else(|_| panic!("{what}: did not return within {secs} s"))
}

/// Per level: (passes, capped, communities), the base graph first.
fn levels(text: &str) -> Vec<(usize, bool, usize)> {
    let quads = parse_statements(text, "nt").expect("fixture parses");
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let triples: Vec<_> = quads
        .iter()
        .map(|(s, p, o, _)| dict.encode(s, p, o).unwrap())
        .collect();
    // The loop of `build_dendrogram`, keeping each level's stats.
    let mut g = project_graph(&dict, &triples);
    let mut out = Vec::new();
    loop {
        let (p, st) = louvain_one_level_stats(&g);
        out.push((st.passes, st.capped, p.count));
        if p.count >= g.node_count() {
            break;
        }
        let next = g.quotient(&p.comm, p.count);
        if next.node_count() <= 1 {
            break;
        }
        g = next;
    }
    out
}

#[test]
fn every_level_converges_without_the_cap() {
    for &(name, text) in FIXTURES {
        let lv = within(name, 30, move || levels(text));
        assert!(
            lv.len() >= 2,
            "{name}: expected at least two levels, got {lv:?}"
        );
        for (i, &(passes, capped, _)) in lv.iter().enumerate() {
            assert!(!capped, "{name}: level {i} stopped at the pass cap: {lv:?}");
            // Converged levels need a handful of passes; the cap is a backstop.
            assert!(
                passes <= 50,
                "{name}: level {i} took {passes} passes: {lv:?}"
            );
            assert!(passes < MAX_LOUVAIN_PASSES);
        }
    }
}

#[test]
fn the_whole_build_returns() {
    for &(name, text) in FIXTURES {
        let n = within(name, 60, move || {
            let quads = parse_statements(text, "nt").unwrap();
            let n = quads.len();
            let (bytes, stats) = assemble_dataset(quads, &[]);
            assert!(stats.pyramid_levels > 0, "a pyramid was built");
            assert!(!bytes.is_empty());
            n
        });
        assert!(n >= 15, "{name}: {n} statements");
    }
}

/// The partition is a pure function of the input: two builds of the same text
/// are byte-identical (the pyramid included), and the smallest fixture's
/// levels are pinned, so a change in a tie decision shows up here. The wasm
/// test (`crates/rete-wasm/tests/louvain_termination.rs`) pins the same file.
#[test]
fn the_fixtures_build_reproducibly() {
    for &(name, text) in FIXTURES {
        let build = move || assemble_dataset(parse_statements(text, "nt").unwrap(), &[]).0;
        let a = within(name, 60, build);
        let b = within(name, 60, build);
        assert_eq!(a, b, "{name}: two builds differ");
    }
    assert_eq!(
        dendrogram(FIXTURES[0].1),
        PINNED_MIN_15,
        "min-15-level0: the dendrogram (community per node, per level)"
    );
}

fn dendrogram(text: &str) -> Vec<Vec<usize>> {
    let quads = parse_statements(text, "nt").unwrap();
    let mut db = DictionaryBuilder::new();
    for (s, p, o, _) in &quads {
        db.observe(s, p, o);
    }
    let dict = db.build();
    let triples: Vec<_> = quads
        .iter()
        .map(|(s, p, o, _)| dict.encode(s, p, o).unwrap())
        .collect();
    let d = build_dendrogram(&project_graph(&dict, &triples));
    d.levels.into_iter().map(|p| p.comm).collect()
}

/// `dendrogram(min-15-level0.nt)` with the exact move test.
const PINNED_MIN_15: &[&[usize]] = &[&[0, 0, 1, 0, 0, 1, 1]];
