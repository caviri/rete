//! Transitive reachability over a single relation — **always available**.
//!
//! This module is intentionally *not* feature-gated: it has no thread or rayon
//! dependency, so it compiles to native, `--no-default-features`, and `wasm32`
//! alike. The optional data-parallel batch variant lives in `crate::parallel`
//! (feature `parallel`) and reuses the shared BFS here, so serial and parallel
//! results are bit-identical by construction.
//!
//! Workload: given N seed nodes, compute each seed's transitive reach over a
//! predicate via BFS on `predicate_pairs`. Build the adjacency once with
//! [`build_adjacency`] (forward), then run [`batch_reach_serial`] (or the
//! parallel sibling). Each per-seed result is the deterministic [`reach_one`].

use std::collections::{BTreeSet, HashMap, VecDeque};

use crate::file::Rete;
use crate::terms::NodeId;

/// Forward adjacency in unified node space for one predicate: `node -> [succ]`.
/// Built once and shared (read-only) across all seeds. For reverse reachability
/// ("who reaches the seed?"), build the map yourself from
/// [`Rete::predicate_pairs`] swapping `(s, o) -> (o, s)`.
pub fn build_adjacency(rete: &Rete, pred: &str) -> HashMap<NodeId, Vec<NodeId>> {
    let mut adj: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (s, o) in rete.predicate_pairs(pred) {
        adj.entry(s).or_default().push(o);
    }
    adj
}

/// Transitive reach of `seed` over the adjacency (excludes the seed itself).
/// Plain BFS; deterministic `BTreeSet` result. This is the single shared BFS
/// used by both the serial and parallel batch drivers.
pub fn reach_one(adj: &HashMap<NodeId, Vec<NodeId>>, seed: NodeId) -> BTreeSet<NodeId> {
    let mut visited = BTreeSet::new();
    let mut queue = VecDeque::new();
    if let Some(succ) = adj.get(&seed) {
        for &n in succ {
            if visited.insert(n) {
                queue.push_back(n);
            }
        }
    }
    while let Some(n) = queue.pop_front() {
        if let Some(succ) = adj.get(&n) {
            for &m in succ {
                if visited.insert(m) {
                    queue.push_back(m);
                }
            }
        }
    }
    visited
}

/// Per-seed transitive reach, serial loop (reference). Results are returned in
/// seed order. The parallel sibling `crate::parallel::batch_reach_parallel`
/// produces an identical result.
pub fn batch_reach_serial(
    adj: &HashMap<NodeId, Vec<NodeId>>,
    seeds: &[NodeId],
) -> Vec<BTreeSet<NodeId>> {
    seeds.iter().map(|&s| reach_one(adj, s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::file::{build_pyramid_meta, write_dataset, Rete, DEFAULT_TILE_BUDGET};
    use crate::index::GraphIndexBuilder;

    /// Two clusters of `knows` edges joined by a bridge.
    fn fixture() -> Rete {
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
        }
        let dict = db.build();
        let mut triples: Vec<(u32, u32, u32)> = edges
            .iter()
            .map(|(s, o)| dict.encode(s, "knows", o).unwrap())
            .collect();
        triples.sort_unstable();
        triples.dedup();

        let mut def = GraphIndexBuilder::new();
        for &t in &triples {
            def.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &triples, DEFAULT_TILE_BUDGET);
        let bytes = write_dataset(&dict, &def.build(), &[], false, &meta, levels);
        Rete::open(&bytes).unwrap()
    }

    #[test]
    fn build_adjacency_and_reach() {
        let rete = fixture();
        let adj = build_adjacency(&rete, "knows");
        let dict = rete.dictionary();
        let a = dict.node_of_term("A").unwrap();
        let reached = reach_one(&adj, a);
        // A reaches the whole graph through the C->D bridge; the C->A cycle pulls A
        // back into the visited set, so all 6 nodes (A,B,C,D,E,F) are reached.
        assert_eq!(reached.len(), 6);
    }

    #[test]
    fn batch_serial_matches_single() {
        let rete = fixture();
        let adj = build_adjacency(&rete, "knows");
        let seeds: Vec<u32> = adj.keys().copied().collect();
        let batch = batch_reach_serial(&adj, &seeds);
        for (i, &s) in seeds.iter().enumerate() {
            assert_eq!(batch[i], reach_one(&adj, s));
        }
        assert!(batch.iter().any(|r| r.len() >= 4));
    }
}
