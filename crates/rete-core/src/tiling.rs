//! Size-targeted tiling and graph summarization (SPEC.md §7).
//!
//! Given the community [`Dendrogram`], this layer turns triples into the
//! pyramid's two products:
//!
//! - **Tiles** — each triple is assigned to the tile of its *subject's*
//!   community (so every triple lives in exactly one tile, and a tile's triples
//!   reconstruct losslessly). Tiles are sized by their encoded triple block, and
//!   [`choose_round_for_budget`] picks the coarsest dendrogram round whose tiles
//!   all fit the byte budget `T`.
//! - **Summary** — aggregated [`SuperEdge`]s `(s_community, predicate,
//!   o_community, count)`: the quotient graph a client fetches first for an
//!   overview before zooming into any tile.

use std::collections::BTreeMap;

use crate::dictionary::Dictionary;
use crate::pyramid::Dendrogram;
use crate::triples::TripleBlockBuilder;

/// Community of a subject term at the given dendrogram round (round is ignored
/// when the graph had no community structure — everything is community 0).
fn subject_comm(dict: &Dictionary, dend: &Dendrogram, round: usize, sid: u32) -> usize {
    if dend.rounds() == 0 {
        0
    } else {
        dend.base_community(dict.subject_node(sid) as usize, round)
    }
}

/// Community of an object term at the given dendrogram round.
fn object_comm(dict: &Dictionary, dend: &Dendrogram, round: usize, oid: u32) -> usize {
    if dend.rounds() == 0 {
        0
    } else {
        dend.base_community(dict.object_node(oid) as usize, round)
    }
}

/// One tile: a community's triples and their encoded SPO block.
#[derive(Debug, Clone)]
pub struct Tile {
    pub community: usize,
    pub triples: Vec<(u32, u32, u32)>,
    pub encoded: Vec<u8>,
}

/// Partition triples into per-community tiles at `round`, ordered by community.
pub fn tile_by_community(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    round: usize,
) -> Vec<Tile> {
    let mut groups: BTreeMap<usize, Vec<(u32, u32, u32)>> = BTreeMap::new();
    for &t in triples {
        let c = subject_comm(dict, dend, round, t.0);
        groups.entry(c).or_default().push(t);
    }
    groups
        .into_iter()
        .map(|(community, ts)| {
            let mut b = TripleBlockBuilder::new();
            for &t in &ts {
                b.push(t);
            }
            Tile {
                community,
                encoded: b.build(),
                triples: ts,
            }
        })
        .collect()
}

/// The coarsest round whose every tile fits `budget_bytes`, else round 0.
/// Coarser rounds mean fewer, larger tiles; we want the fewest tiles that still
/// respect the per-tile budget (PMTiles-style).
pub fn choose_round_for_budget(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    budget_bytes: usize,
) -> usize {
    if dend.rounds() == 0 {
        return 0;
    }
    for round in (0..dend.rounds()).rev() {
        let max_tile = tile_by_community(dict, triples, dend, round)
            .iter()
            .map(|t| t.encoded.len())
            .max()
            .unwrap_or(0);
        if max_tile <= budget_bytes {
            return round;
        }
    }
    0
}

/// An aggregated relation between two communities in the summary graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SuperEdge {
    pub s_comm: usize,
    pub predicate: u32,
    pub o_comm: usize,
    pub count: u32,
}

/// Aggregate triples into the quotient (summary) graph at `round`.
pub fn summarize(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    round: usize,
) -> Vec<SuperEdge> {
    let mut counts: BTreeMap<(usize, u32, usize), u32> = BTreeMap::new();
    for &(s, p, o) in triples {
        let sc = subject_comm(dict, dend, round, s);
        let oc = object_comm(dict, dend, round, o);
        *counts.entry((sc, p, oc)).or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((s_comm, predicate, o_comm), count)| SuperEdge {
            s_comm,
            predicate,
            o_comm,
            count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::pyramid::{build_dendrogram, project_graph};

    /// Two clusters of `knows` edges joined by a bridge, plus some intra-cluster
    /// fan-out so tiles have real size.
    fn fixture() -> (Dictionary, Vec<(u32, u32, u32)>, Dendrogram) {
        let edges = [
            ("A", "B"),
            ("B", "C"),
            ("A", "C"),
            ("C", "A"),
            ("D", "E"),
            ("E", "F"),
            ("D", "F"),
            ("F", "D"),
            ("C", "D"), // bridge
        ];
        let mut db = DictionaryBuilder::new();
        for (s, o) in edges {
            db.observe(s, "knows", o);
        }
        let dict = db.build();
        let triples: Vec<_> = edges
            .iter()
            .map(|(s, o)| dict.encode(s, "knows", o).unwrap())
            .collect();
        let g = project_graph(&dict, &triples);
        let dend = build_dendrogram(&g);
        (dict, triples, dend)
    }

    #[test]
    fn tiles_reconstruct_all_triples_losslessly() {
        let (dict, triples, dend) = fixture();
        let round = dend.rounds().saturating_sub(1);
        let tiles = tile_by_community(&dict, &triples, &dend, round);

        let mut from_tiles: Vec<_> = tiles.iter().flat_map(|t| t.triples.clone()).collect();
        from_tiles.sort_unstable();
        let mut expected = triples.clone();
        expected.sort_unstable();
        assert_eq!(
            from_tiles, expected,
            "tiles must cover every triple exactly"
        );
        // More than one tile means the summary actually partitioned the graph.
        assert!(tiles.len() >= 2);
    }

    #[test]
    fn summary_preserves_total_count() {
        let (dict, triples, dend) = fixture();
        let round = dend.rounds().saturating_sub(1);
        let summary = summarize(&dict, &triples, &dend, round);
        let total: u32 = summary.iter().map(|e| e.count).sum();
        assert_eq!(total as usize, triples.len());
        // The bridge C->D shows up as a cross-community superedge.
        assert!(summary.iter().any(|e| e.s_comm != e.o_comm));
    }

    #[test]
    fn budget_selects_finer_round_when_tight() {
        let (dict, triples, dend) = fixture();
        // Huge budget => coarsest round (fewest tiles).
        let coarse = choose_round_for_budget(&dict, &triples, &dend, 1_000_000);
        // Zero budget => finest round 0.
        let fine = choose_round_for_budget(&dict, &triples, &dend, 0);
        assert!(coarse >= fine);
        assert_eq!(fine, 0);
    }
}
