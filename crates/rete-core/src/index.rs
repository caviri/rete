//! Permutation index set (SPO / POS / OSP) over integer triples (SPEC.md §6).
//!
//! The three orders together answer every triple-pattern shape: for any subset
//! of bound `(s, p, o)` components, at least one permutation sorts those bound
//! components into a leading prefix, turning the lookup into a range scan.
//!
//! v0 keeps one block per permutation (no tiling yet) and scans with zone-map
//! pruning. Repeated point lookups go through [`GraphIndex::probe_iter`],
//! which binary-searches a lazily-built in-memory a-group directory; on-disk
//! tiling comes with the pyramid layer.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use crate::triples::{GroupDirectory, Triple, TripleBlock, TripleBlockBuilder};

/// A triple pattern: `None` is an unbound variable, `Some(id)` a bound term.
pub type Pattern = (Option<u32>, Option<u32>, Option<u32>);

/// Which stored permutation to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPermutation {
    Spo,
    Pos,
    Osp,
}

impl IndexPermutation {
    /// Stable display name used in CLI diagnostics and provenance records.
    pub fn name(self) -> &'static str {
        match self {
            IndexPermutation::Spo => "SPO",
            IndexPermutation::Pos => "POS",
            IndexPermutation::Osp => "OSP",
        }
    }

    /// Section number inside the stored permutation container.
    pub fn section_index(self) -> usize {
        match self {
            IndexPermutation::Spo => 0,
            IndexPermutation::Pos => 1,
            IndexPermutation::Osp => 2,
        }
    }

    /// Map a canonical `(s, p, o)` triple into this permutation's `(a, b, c)`.
    fn forward(self, (s, p, o): Triple) -> Triple {
        match self {
            IndexPermutation::Spo => (s, p, o),
            IndexPermutation::Pos => (p, o, s),
            IndexPermutation::Osp => (o, s, p),
        }
    }

    /// Map a stored `(a, b, c)` back to canonical `(s, p, o)`.
    fn back(self, (a, b, c): Triple) -> Triple {
        match self {
            IndexPermutation::Spo => (a, b, c),
            IndexPermutation::Pos => (c, a, b), // a=p, b=o, c=s
            IndexPermutation::Osp => (b, c, a), // a=o, b=s, c=p
        }
    }

    /// The pattern's bound components in this permutation's component order.
    fn order_pattern(self, (s, p, o): Pattern) -> [Option<u32>; 3] {
        match self {
            IndexPermutation::Spo => [s, p, o],
            IndexPermutation::Pos => [p, o, s],
            IndexPermutation::Osp => [o, s, p],
        }
    }

    /// Length of the leading run of bound components — higher is more selective.
    fn leading_bound(self, pat: Pattern) -> usize {
        self.order_pattern(pat)
            .iter()
            .take_while(|c| c.is_some())
            .count()
    }
}

/// Build a [`GraphIndex`] from canonical `(s, p, o)` integer triples.
#[derive(Default)]
pub struct GraphIndexBuilder {
    triples: Vec<Triple>,
}

impl GraphIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, t: Triple) {
        self.triples.push(t);
    }

    pub fn build(self) -> GraphIndex {
        let mut spo = TripleBlockBuilder::new();
        let mut pos = TripleBlockBuilder::new();
        let mut osp = TripleBlockBuilder::new();
        for &t in &self.triples {
            spo.push(IndexPermutation::Spo.forward(t));
            pos.push(IndexPermutation::Pos.forward(t));
            osp.push(IndexPermutation::Osp.forward(t));
        }
        GraphIndex::from_blocks([spo.build(), pos.build(), osp.build()])
    }
}

/// The three permutation blocks, queryable by triple pattern.
pub struct GraphIndex {
    spo: Vec<u8>,
    pos: Vec<u8>,
    osp: Vec<u8>,
    /// Lazily-built a-group directories (SPO, POS, OSP), enabling
    /// binary-search jumps for leading-bound scans. Built at most once per
    /// permutation — and only from the *second* such scan (`scan_counts`), so
    /// a one-shot cold query never pays the full directory walk.
    dirs: [OnceLock<GroupDirectory>; 3],
    scan_counts: [AtomicU32; 3],
}

impl GraphIndex {
    /// Rebuild from three serialized permutation blocks (SPO, POS, OSP), e.g.
    /// when reading a `.rete` file.
    pub fn from_blocks(blocks: [Vec<u8>; 3]) -> Self {
        let [spo, pos, osp] = blocks;
        GraphIndex {
            spo,
            pos,
            osp,
            dirs: [OnceLock::new(), OnceLock::new(), OnceLock::new()],
            scan_counts: [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)],
        }
    }

    /// Total triple count (from the SPO block's zone map).
    pub fn triple_count(&self) -> u32 {
        TripleBlock::parse(&self.spo)
            .map(|b| b.zone().count)
            .unwrap_or(0)
    }

    /// Serialized permutation blocks, for the file writer (SPO, POS, OSP).
    pub fn blocks(&self) -> [&[u8]; 3] {
        [&self.spo, &self.pos, &self.osp]
    }

    /// The permutation selected for a pattern: the one with the longest bound
    /// prefix. Ties keep the canonical SPO order (then POS, then OSP), which
    /// makes provenance stable for unbound or equally selective shapes and routes
    /// a fully unbound pattern to the SPO block rather than fetching all three.
    pub fn best_permutation(pattern: Pattern) -> IndexPermutation {
        let mut best = IndexPermutation::Spo;
        let mut best_score = best.leading_bound(pattern);
        for perm in [IndexPermutation::Pos, IndexPermutation::Osp] {
            let score = perm.leading_bound(pattern);
            if score > best_score {
                best = perm;
                best_score = score;
            }
        }
        best
    }

    fn block(&self, perm: IndexPermutation) -> &[u8] {
        match perm {
            IndexPermutation::Spo => &self.spo,
            IndexPermutation::Pos => &self.pos,
            IndexPermutation::Osp => &self.osp,
        }
    }

    /// Match one already-decoded serialized permutation block. This is the core
    /// primitive for range-routed readers: the caller fetches only the selected
    /// block payload, then this scans it as if it came from a full [`GraphIndex`].
    pub fn match_serialized_block(
        bytes: &[u8],
        permutation: IndexPermutation,
        pattern: Pattern,
    ) -> Vec<Triple> {
        let [pa, pb, pc] = permutation.order_pattern(pattern);
        let mut out: Vec<Triple> = TripleBlock::parse(bytes)
            .ok()
            .filter(|b| b.zone().may_contain(pa, pb, pc))
            .map(|b| b.scan(pa, pb, pc))
            .into_iter()
            .flatten()
            .map(move |abc| permutation.back(abc))
            .collect();
        out.sort_unstable();
        out
    }

    /// All triples matching `pattern`, returned in canonical `(s, p, o)` order.
    ///
    /// Thin eager wrapper over [`scan_iter`](Self::scan_iter): collect the lazy
    /// stream and restore the canonical sort (the stream is sorted in the chosen
    /// permutation's order, which differs from canonical once `perm.back`
    /// permutes the free components).
    pub fn match_pattern(&self, pattern: Pattern) -> Vec<Triple> {
        let mut out: Vec<Triple> = self.scan_iter(pattern).collect();
        out.sort_unstable();
        out
    }

    /// Lazily stream the triples matching `pattern` in canonical `(s, p, o)`
    /// order *within the chosen permutation* — the streaming entry point for
    /// callers that can stop early (ASK, `LIMIT`, BGP probes) or that don't need
    /// the canonical re-sort. Decodes only the matching groups (see
    /// [`TripleBlock::scan`]); a malformed/absent block yields nothing rather
    /// than panicking.
    pub fn scan_iter(&self, pattern: Pattern) -> impl Iterator<Item = Triple> + '_ {
        // Pick the permutation with the longest leading-bound prefix.
        let perm = Self::best_permutation(pattern);
        let bytes = self.block(perm);
        let [pa, pb, pc] = perm.order_pattern(pattern);
        // Parse (untrusted bytes ⇒ `None` on malformed) and zone-prune up front,
        // then stream the matching groups, mapping each back to canonical order.
        let cursor = TripleBlock::parse(bytes)
            .ok()
            .filter(|b| b.zone().may_contain(pa, pb, pc))
            .map(|b| match (pa, self.directory(perm)) {
                // A leading-bound scan with the directory available jumps
                // straight to its a-group (binary search) instead of walking
                // and decoding every preceding group.
                (Some(a), Some(dir)) => b.scan_from(dir, a, pb, pc),
                _ => b.scan(pa, pb, pc),
            });
        cursor.into_iter().flatten().map(move |abc| perm.back(abc))
    }

    /// The permutation's a-group directory, if this scan should use one: the
    /// first leading-bound scan walks linearly (a cold one-shot query never
    /// pays the full directory build), every later one gets — and if needed
    /// builds — the directory, after which probes are point lookups. The
    /// directory itself is built at most once (`OnceLock`).
    fn directory(&self, perm: IndexPermutation) -> Option<&GroupDirectory> {
        let i = perm.section_index();
        if let Some(dir) = self.dirs[i].get() {
            return Some(dir);
        }
        if self.scan_counts[i].fetch_add(1, Ordering::Relaxed) == 0 {
            return None;
        }
        let block = TripleBlock::parse(self.block(perm)).ok()?;
        Some(self.dirs[i].get_or_init(|| block.group_directory()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (GraphIndex, Vec<Triple>) {
        let data = vec![
            (1, 10, 100),
            (1, 10, 101),
            (1, 11, 100),
            (2, 10, 100),
            (2, 12, 200),
            (3, 11, 300),
        ];
        let mut b = GraphIndexBuilder::new();
        for &t in &data {
            b.push(t);
        }
        (b.build(), data)
    }

    /// Brute-force reference for a pattern.
    fn reference(data: &[Triple], (s, p, o): Pattern) -> Vec<Triple> {
        let mut v: Vec<Triple> = data
            .iter()
            .copied()
            .filter(|&(a, b, c)| {
                s.is_none_or(|x| x == a) && p.is_none_or(|x| x == b) && o.is_none_or(|x| x == c)
            })
            .collect();
        v.sort_unstable();
        v
    }

    #[test]
    fn every_pattern_shape_matches_reference() {
        let (idx, data) = graph();
        let vals = |opts: &[u32]| {
            let mut v: Vec<Option<u32>> = opts.iter().map(|&x| Some(x)).collect();
            v.push(None);
            v
        };
        // Exercise all 8 bound/unbound shapes across representative values.
        for s in vals(&[1, 2, 9]) {
            for p in vals(&[10, 11, 99]) {
                for o in vals(&[100, 300, 999]) {
                    let pat = (s, p, o);
                    assert_eq!(
                        idx.match_pattern(pat),
                        reference(&data, pat),
                        "pattern {pat:?}"
                    );
                    // The lazy stream must match the eager result once sorted —
                    // same triples, just without the up-front canonical re-sort.
                    let mut streamed: Vec<Triple> = idx.scan_iter(pat).collect();
                    streamed.sort_unstable();
                    assert_eq!(streamed, reference(&data, pat), "scan_iter {pat:?}");
                }
            }
        }
    }

    #[test]
    fn unbound_returns_everything_sorted() {
        let (idx, data) = graph();
        let mut sorted = data.clone();
        sorted.sort_unstable();
        assert_eq!(idx.match_pattern((None, None, None)), sorted);
    }

    /// Scans must yield identical results across the directory lifecycle: the
    /// first leading-bound scan walks linearly, the second builds the
    /// directory, and later ones jump through it — including absent ids
    /// (between, below, and above the stored groups).
    #[test]
    fn directory_backed_scans_match_reference_every_shape() {
        let (idx, data) = graph();
        let vals = |opts: &[u32]| {
            let mut v: Vec<Option<u32>> = opts.iter().map(|&x| Some(x)).collect();
            v.push(None);
            v
        };
        for round in 0..3 {
            for s in vals(&[1, 2, 3, 0, 9]) {
                for p in vals(&[10, 11, 12, 99]) {
                    for o in vals(&[100, 200, 300, 999]) {
                        let pat = (s, p, o);
                        let mut got: Vec<Triple> = idx.scan_iter(pat).collect();
                        got.sort_unstable();
                        assert_eq!(got, reference(&data, pat), "round {round} scan {pat:?}");
                    }
                }
            }
        }
    }
}
