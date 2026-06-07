//! Integer triple blocks: grouped, delta-coded adjacency with a zone map
//! (SPEC.md §6.1).
//!
//! A block stores triples `(a, b, c)` of dictionary IDs for one permutation,
//! sorted ascending. `a`/`b`/`c` are the permuted roles — for an SPO block
//! `a=subject, b=predicate, c=object`; for POS, `a=predicate, b=object,
//! c=subject`; and so on. The encoding is role-agnostic.

use crate::varint::{read_uvarint, write_uvarint};

/// A triple of dictionary IDs in some permutation's component order.
pub type Triple = (u32, u32, u32);

/// Per-block summary statistics enabling block-skipping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneMap {
    pub min_a: u32,
    pub max_a: u32,
    pub min_b: u32,
    pub max_b: u32,
    pub min_c: u32,
    pub max_c: u32,
    pub count: u32,
}

impl ZoneMap {
    /// Could a triple with the given bound components possibly live in this
    /// block? `None` means "unbound — don't constrain on this component".
    pub fn may_contain(&self, a: Option<u32>, b: Option<u32>, c: Option<u32>) -> bool {
        let in_range = |v: Option<u32>, lo: u32, hi: u32| v.is_none_or(|x| lo <= x && x <= hi);
        in_range(a, self.min_a, self.max_a)
            && in_range(b, self.min_b, self.max_b)
            && in_range(c, self.min_c, self.max_c)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum TripleError {
    #[error("malformed triple block: {0}")]
    Malformed(&'static str),
}

/// Accumulates triples and serializes a block.
#[derive(Default)]
pub struct TripleBlockBuilder {
    triples: Vec<Triple>,
}

impl TripleBlockBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, t: Triple) {
        self.triples.push(t);
    }

    pub fn len(&self) -> usize {
        self.triples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.triples.is_empty()
    }

    /// Sort, dedup, and encode. Returns an empty body for zero triples.
    pub fn build(mut self) -> Vec<u8> {
        self.triples.sort_unstable();
        self.triples.dedup();
        let t = &self.triples;

        let mut out = Vec::new();
        if t.is_empty() {
            // zone map of zeros + count 0, no body.
            for _ in 0..7 {
                write_uvarint(&mut out, 0);
            }
            write_uvarint(&mut out, 0); // num_a
            return out;
        }

        // Zone map.
        let (mut min_a, mut max_a) = (u32::MAX, 0u32);
        let (mut min_b, mut max_b) = (u32::MAX, 0u32);
        let (mut min_c, mut max_c) = (u32::MAX, 0u32);
        for &(a, b, c) in t {
            min_a = min_a.min(a);
            max_a = max_a.max(a);
            min_b = min_b.min(b);
            max_b = max_b.max(b);
            min_c = min_c.min(c);
            max_c = max_c.max(c);
        }
        for v in [min_a, max_a, min_b, max_b, min_c, max_c, t.len() as u32] {
            write_uvarint(&mut out, v as u64);
        }

        // Body: grouped delta adjacency.
        // First pass: collect a-groups -> b-groups -> c-list.
        // (b, c-list) for one b under an a; and (a, its b-groups).
        type BGroup = (u32, Vec<u32>);
        type AGroup = (u32, Vec<BGroup>);
        let mut i = 0;
        let mut a_groups: Vec<AGroup> = Vec::new();
        while i < t.len() {
            let a = t[i].0;
            let mut b_groups: Vec<(u32, Vec<u32>)> = Vec::new();
            while i < t.len() && t[i].0 == a {
                let b = t[i].1;
                let mut cs = Vec::new();
                while i < t.len() && t[i].0 == a && t[i].1 == b {
                    cs.push(t[i].2);
                    i += 1;
                }
                b_groups.push((b, cs));
            }
            a_groups.push((a, b_groups));
        }

        write_uvarint(&mut out, a_groups.len() as u64);
        let mut prev_a = 0u32;
        for (a, b_groups) in &a_groups {
            write_uvarint(&mut out, (a - prev_a) as u64);
            prev_a = *a;
            write_uvarint(&mut out, b_groups.len() as u64);
            let mut prev_b = 0u32;
            for (b, cs) in b_groups {
                write_uvarint(&mut out, (b - prev_b) as u64);
                prev_b = *b;
                write_uvarint(&mut out, cs.len() as u64);
                let mut prev_c = 0u32;
                for c in cs {
                    write_uvarint(&mut out, (c - prev_c) as u64);
                    prev_c = *c;
                }
            }
        }
        out
    }
}

/// A parsed triple block.
pub struct TripleBlock<'a> {
    bytes: &'a [u8],
    zone: ZoneMap,
    body_start: usize,
}

impl<'a> TripleBlock<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, TripleError> {
        let mut pos = 0;
        let take = |pos: &mut usize| -> Result<u32, TripleError> {
            let (v, n) = read_uvarint(&bytes[*pos..]).ok_or(TripleError::Malformed("truncated"))?;
            *pos += n;
            Ok(v as u32)
        };
        let zone = ZoneMap {
            min_a: take(&mut pos)?,
            max_a: take(&mut pos)?,
            min_b: take(&mut pos)?,
            max_b: take(&mut pos)?,
            min_c: take(&mut pos)?,
            max_c: take(&mut pos)?,
            count: take(&mut pos)?,
        };
        Ok(Self {
            bytes,
            zone,
            body_start: pos,
        })
    }

    pub fn zone(&self) -> &ZoneMap {
        &self.zone
    }

    /// Decode all triples in ascending order. The bytes may be corrupt (a block
    /// from an untrusted file), so decoding is bounds-safe and stops gracefully
    /// at the first malformed varint rather than panicking — returning whatever
    /// prefix decoded cleanly.
    pub fn triples(&self) -> Vec<Triple> {
        self.try_triples().unwrap_or_default()
    }

    fn try_triples(&self) -> Option<Vec<Triple>> {
        // `zone.count` is untrusted; each pushed triple consumes ≥1 byte, so the
        // buffer length is a safe capacity ceiling (avoids an OOM on a bogus count).
        let mut out = Vec::with_capacity((self.zone.count as usize).min(self.bytes.len()));
        let mut pos = self.body_start;
        let g = |pos: &mut usize| -> Option<u32> {
            let (v, n) = read_uvarint(self.bytes.get(*pos..)?)?;
            *pos += n;
            Some(v as u32)
        };
        let num_a = g(&mut pos)?;
        let mut a = 0u32;
        for _ in 0..num_a {
            // wrapping_add: corrupt deltas must not overflow-panic in debug builds.
            a = a.wrapping_add(g(&mut pos)?);
            let num_b = g(&mut pos)?;
            let mut b = 0u32;
            for _ in 0..num_b {
                b = b.wrapping_add(g(&mut pos)?);
                let num_c = g(&mut pos)?;
                let mut c = 0u32;
                for _ in 0..num_c {
                    c = c.wrapping_add(g(&mut pos)?);
                    out.push((a, b, c));
                }
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<Triple> {
        // Unsorted, with a duplicate, multiple b's per a and c's per b.
        vec![
            (5, 2, 9),
            (1, 1, 1),
            (1, 1, 4),
            (1, 3, 2),
            (5, 2, 7),
            (1, 1, 1), // dup
            (2, 9, 9),
        ]
    }

    #[test]
    fn round_trip_sorted_dedup() {
        let mut b = TripleBlockBuilder::new();
        for t in sample() {
            b.push(t);
        }
        let bytes = b.build();
        let blk = TripleBlock::parse(&bytes).unwrap();

        let mut expected = sample();
        expected.sort_unstable();
        expected.dedup();
        assert_eq!(blk.zone().count as usize, expected.len());
        assert_eq!(blk.triples(), expected);
    }

    #[test]
    fn zone_map_bounds_and_skipping() {
        let mut b = TripleBlockBuilder::new();
        for t in sample() {
            b.push(t);
        }
        let bytes = b.build();
        let blk = TripleBlock::parse(&bytes).unwrap();
        let z = blk.zone();
        assert_eq!((z.min_a, z.max_a), (1, 5));
        assert_eq!((z.min_b, z.max_b), (1, 9));
        assert_eq!((z.min_c, z.max_c), (1, 9));
        // a=3 is within [1,5] so "maybe"; a=99 is out so skippable.
        assert!(z.may_contain(Some(3), None, None));
        assert!(!z.may_contain(Some(99), None, None));
        assert!(z.may_contain(None, None, None)); // fully unbound
    }

    #[test]
    fn empty_block() {
        let blk_bytes = TripleBlockBuilder::new().build();
        let blk = TripleBlock::parse(&blk_bytes).unwrap();
        assert_eq!(blk.zone().count, 0);
        assert!(blk.triples().is_empty());
    }
}
