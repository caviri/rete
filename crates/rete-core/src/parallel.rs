//! Prototype **data-parallel** query evaluator (feature `parallel`).
//!
//! This module sits *alongside* the serial engine — it never replaces it. Every
//! parallel workload here ships with a serial reference that produces a
//! bit-identical result, so a benchmark (or a test) can assert
//! `serial == parallel` and only then trust the timing.
//!
//! Two honest, correctness-checkable workloads (see `docs/BENCHMARK.md`):
//!
//! 1. **Intra-query split across communities + harmonize.** The graph's triples
//!    are partitioned into per-community [`Tile`]s (`tile_by_community` at the
//!    `choose_round_for_budget` round). A decomposable aggregation runs per
//!    tile in parallel and the partials are *harmonized* (summed / merged):
//!    - [`count_predicate_serial`] / [`count_predicate_parallel`] — per-tile
//!      predicate count, summed.
//!    - [`out_degree_serial`] / [`out_degree_parallel`] — per-subject
//!      out-degree, computed per tile then merged into one `BTreeMap`.
//!
//! 2. **Batch of independent reachability queries** (embarrassingly parallel).
//!    Given N seed nodes, compute each seed's transitive reach over a predicate
//!    via BFS on `predicate_pairs`. The serial reference and the BFS itself live
//!    in the always-available [`crate::reach`] module
//!    ([`crate::reach::batch_reach_serial`]); [`batch_reach_parallel`] here fans
//!    one rayon task per seed and returns an identical per-seed reach set.
//!
//! CPU-side only: the HTTP/range path is I/O-bound and wasm has no threads, so
//! `parallel` is intentionally off by default and absent from those builds.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rayon::prelude::*;

use crate::file::Rete;
use crate::reach::reach_one;
use crate::terms::NodeId;
use crate::tiling::Tile;

// ---------------------------------------------------------------------------
// Workload 1a: per-community predicate count, harmonized by sum.
// ---------------------------------------------------------------------------

/// Count, over the whole graph, the triples whose predicate is `pred` — the
/// serial reference. Uses the engine's index fast path (single pattern scan).
pub fn count_predicate_serial(rete: &Rete, pred: &str) -> u64 {
    match rete.dictionary().predicate_id(pred) {
        Some(pid) => rete.match_ids((None, Some(pid), None)).len() as u64,
        None => 0,
    }
}

/// Same count, computed per community tile in parallel and summed (harmonize).
/// `tiles` come from [`crate::tiling::tile_by_community`]; `pid` is the
/// predicate's dictionary ID (resolve once with `dictionary().predicate_id`).
pub fn count_predicate_parallel(tiles: &[Tile], pid: u32) -> u64 {
    tiles
        .par_iter()
        .map(|t| t.triples.iter().filter(|&&(_, p, _)| p == pid).count() as u64)
        .sum()
}

/// Serial sum over tiles, so a test can compare the *same* tiled view both ways
/// (the index-based [`count_predicate_serial`] is a stronger cross-check).
pub fn count_predicate_tiles_serial(tiles: &[Tile], pid: u32) -> u64 {
    tiles
        .iter()
        .map(|t| t.triples.iter().filter(|&&(_, p, _)| p == pid).count() as u64)
        .sum()
}

// ---------------------------------------------------------------------------
// Workload 1b: per-subject out-degree, harmonized by BTreeMap merge.
// ---------------------------------------------------------------------------

/// Per-subject out-degree (number of triples with that subject) over all tiles,
/// serial reference. Deterministic key order via `BTreeMap`.
pub fn out_degree_serial(tiles: &[Tile]) -> BTreeMap<u32, u64> {
    let mut acc: BTreeMap<u32, u64> = BTreeMap::new();
    for t in tiles {
        for &(s, _, _) in &t.triples {
            *acc.entry(s).or_default() += 1;
        }
    }
    acc
}

/// Same out-degree distribution, computed per tile in parallel then merged.
/// The merge (harmonize) sums partials key-by-key; result is identical to the
/// serial map regardless of tile order or thread scheduling.
pub fn out_degree_parallel(tiles: &[Tile]) -> BTreeMap<u32, u64> {
    tiles
        .par_iter()
        .map(|t| {
            let mut local: BTreeMap<u32, u64> = BTreeMap::new();
            for &(s, _, _) in &t.triples {
                *local.entry(s).or_default() += 1;
            }
            local
        })
        .reduce(BTreeMap::new, |mut a, b| {
            for (k, v) in b {
                *a.entry(k).or_default() += v;
            }
            a
        })
}

// ---------------------------------------------------------------------------
// Workload 2: batch of independent single-source reachability queries.
// ---------------------------------------------------------------------------

/// Per-seed transitive reach, one rayon task per seed. Results are returned in
/// seed order and are identical to [`crate::reach::batch_reach_serial`]. The BFS
/// itself ([`reach_one`]) is shared with the serial module.
pub fn batch_reach_parallel(
    adj: &HashMap<NodeId, Vec<NodeId>>,
    seeds: &[NodeId],
) -> Vec<BTreeSet<NodeId>> {
    seeds.par_iter().map(|&s| reach_one(adj, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{build_pyramid_meta, write_dataset, Rete, DEFAULT_TILE_BUDGET};
    use crate::index::GraphIndexBuilder;
    use crate::pyramid::{build_dendrogram, project_graph};
    use crate::tiling::{choose_round_for_budget, tile_by_community};

    /// Two clusters of `knows` edges joined by a bridge, plus `age` literals so
    /// there is a second predicate. Returns the opened `.rete` and the tiles.
    fn fixture() -> (Rete, Vec<crate::tiling::Tile>) {
        let edges = [
            ("A", "B"),
            ("B", "C"),
            ("A", "C"),
            ("C", "A"),
            ("D", "E"),
            ("E", "F"),
            ("D", "F"),
            ("F", "D"),
            ("C", "D"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, o) in edges {
            db.observe(s, "knows", o);
            db.observe(s, "age", "30");
        }
        let dict = db.build();
        let mut triples: Vec<(u32, u32, u32)> = edges
            .iter()
            .map(|(s, o)| dict.encode(s, "knows", o).unwrap())
            .collect();
        for (s, _) in edges {
            triples.push(dict.encode(s, "age", "30").unwrap());
        }
        triples.sort_unstable();
        triples.dedup();

        let mut def = GraphIndexBuilder::new();
        for &t in &triples {
            def.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &triples, DEFAULT_TILE_BUDGET);
        let bytes = write_dataset(&dict, &def.build(), &[], false, &meta, levels);
        let rete = Rete::open(&bytes).unwrap();

        let g = project_graph(&dict, &triples);
        let dend = build_dendrogram(&g);
        let round = choose_round_for_budget(&dict, &triples, &dend, DEFAULT_TILE_BUDGET);
        let tiles = tile_by_community(&dict, &triples, &dend, round);
        (rete, tiles)
    }

    #[test]
    fn predicate_count_serial_eq_parallel() {
        let (rete, tiles) = fixture();
        let pid = rete.dictionary().predicate_id("knows").unwrap();
        let serial = count_predicate_serial(&rete, "knows");
        let par = count_predicate_parallel(&tiles, pid);
        assert_eq!(serial, par);
        assert_eq!(serial, count_predicate_tiles_serial(&tiles, pid));
        assert!(serial > 0);
    }

    #[test]
    fn out_degree_serial_eq_parallel() {
        let (_rete, tiles) = fixture();
        let serial = out_degree_serial(&tiles);
        let par = out_degree_parallel(&tiles);
        assert_eq!(serial, par);
        assert!(!serial.is_empty());
    }

    #[test]
    fn batch_reach_serial_eq_parallel() {
        use crate::reach::{batch_reach_serial, build_adjacency};
        let (rete, _tiles) = fixture();
        let adj = build_adjacency(&rete, "knows");
        let seeds: Vec<u32> = adj.keys().copied().collect();
        let serial = batch_reach_serial(&adj, &seeds);
        let par = batch_reach_parallel(&adj, &seeds);
        assert_eq!(serial, par);
        // The bridge means at least one seed reaches the other cluster.
        assert!(serial.iter().any(|r| r.len() >= 4));
    }
}
