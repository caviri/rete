//! Permutation index set (SPO / POS / OSP) over integer triples (SPEC.md §6).
//!
//! The three orders together answer every triple-pattern shape: for any subset
//! of bound `(s, p, o)` components, at least one permutation sorts those bound
//! components into a leading prefix, turning the lookup into a range scan.
//!
//! v0 keeps one block per permutation (no tiling yet) and scans with zone-map
//! pruning; tiling and intra-block binary search come with the pyramid layer.

use crate::triples::{Triple, TripleBlock, TripleBlockBuilder};

/// A triple pattern: `None` is an unbound variable, `Some(id)` a bound term.
pub type Pattern = (Option<u32>, Option<u32>, Option<u32>);

/// Which stored permutation to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Perm {
    Spo,
    Pos,
    Osp,
}

impl Perm {
    /// Map a canonical `(s, p, o)` triple into this permutation's `(a, b, c)`.
    fn forward(self, (s, p, o): Triple) -> Triple {
        match self {
            Perm::Spo => (s, p, o),
            Perm::Pos => (p, o, s),
            Perm::Osp => (o, s, p),
        }
    }

    /// Map a stored `(a, b, c)` back to canonical `(s, p, o)`.
    fn back(self, (a, b, c): Triple) -> Triple {
        match self {
            Perm::Spo => (a, b, c),
            Perm::Pos => (c, a, b), // a=p, b=o, c=s
            Perm::Osp => (b, c, a), // a=o, b=s, c=p
        }
    }

    /// The pattern's bound components in this permutation's component order.
    fn order_pattern(self, (s, p, o): Pattern) -> [Option<u32>; 3] {
        match self {
            Perm::Spo => [s, p, o],
            Perm::Pos => [p, o, s],
            Perm::Osp => [o, s, p],
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
            spo.push(Perm::Spo.forward(t));
            pos.push(Perm::Pos.forward(t));
            osp.push(Perm::Osp.forward(t));
        }
        GraphIndex {
            spo: spo.build(),
            pos: pos.build(),
            osp: osp.build(),
        }
    }
}

/// The three permutation blocks, queryable by triple pattern.
pub struct GraphIndex {
    spo: Vec<u8>,
    pos: Vec<u8>,
    osp: Vec<u8>,
}

impl GraphIndex {
    /// Rebuild from three serialized permutation blocks (SPO, POS, OSP), e.g.
    /// when reading a `.rete` file.
    pub fn from_blocks(blocks: [Vec<u8>; 3]) -> Self {
        let [spo, pos, osp] = blocks;
        GraphIndex { spo, pos, osp }
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

    /// All triples matching `pattern`, returned in canonical `(s, p, o)` order.
    pub fn match_pattern(&self, pattern: Pattern) -> Vec<Triple> {
        // Pick the permutation with the longest leading-bound prefix.
        let perm = [Perm::Spo, Perm::Pos, Perm::Osp]
            .into_iter()
            .max_by_key(|p| p.leading_bound(pattern))
            .unwrap();
        let bytes = match perm {
            Perm::Spo => &self.spo,
            Perm::Pos => &self.pos,
            Perm::Osp => &self.osp,
        };
        // The block bytes may come from an untrusted file; a malformed block
        // yields no matches rather than panicking.
        let Ok(block) = TripleBlock::parse(bytes) else {
            return Vec::new();
        };

        // Zone-map prune: translate the pattern into stored-order bounds.
        let [pa, pb, pc] = perm.order_pattern(pattern);
        if !block.zone().may_contain(pa, pb, pc) {
            return Vec::new();
        }

        let mut out: Vec<Triple> = block
            .triples()
            .into_iter()
            .filter(|&abc| {
                let [wa, wb, wc] = [pa, pb, pc];
                wa.is_none_or(|x| x == abc.0)
                    && wb.is_none_or(|x| x == abc.1)
                    && wc.is_none_or(|x| x == abc.2)
            })
            .map(|abc| perm.back(abc))
            .collect();
        out.sort_unstable();
        out
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
}
