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

    /// Build the byte-offset directory of this block's a-groups (one header
    /// walk), enabling binary-search probes via [`scan_from`](Self::scan_from).
    /// On corrupt bytes the walk stops early — the directory is a prefix, and
    /// the bounds-checked cursor degrades gracefully like every other reader.
    pub fn group_directory(&self) -> GroupDirectory {
        let bytes = self.bytes;
        let mut entries = Vec::new();
        let mut p = self.body_start;
        let mut walk = || -> Option<()> {
            let num_a = rd(bytes, &mut p)?;
            // `num_a` is untrusted; each group consumes ≥2 bytes, so the buffer
            // length caps the allocation.
            entries.reserve((num_a as usize).min(bytes.len()));
            let mut a = 0u32;
            for i in 0..num_a {
                a = a.wrapping_add(rd(bytes, &mut p)?);
                let num_b = rd(bytes, &mut p)?;
                entries.push(DirEntry {
                    a,
                    pos: p,
                    num_b,
                    a_rem_after: num_a - 1 - i,
                });
                for _ in 0..num_b {
                    rd(bytes, &mut p)?; // delta_b
                    let nc = rd(bytes, &mut p)?;
                    for _ in 0..nc {
                        rd(bytes, &mut p)?;
                    }
                }
            }
            Some(())
        };
        let _ = walk();
        GroupDirectory { entries }
    }

    /// Probe the block for a **bound leading component** `pa`, jumping straight
    /// to its a-group through the directory (binary search) instead of walking
    /// every preceding group header. Yields exactly what
    /// `scan(Some(pa), pb, pc)` would.
    pub fn scan_from(
        &self,
        dir: &GroupDirectory,
        pa: u32,
        pb: Option<u32>,
        pc: Option<u32>,
    ) -> BlockCursor<'a> {
        let mut cursor = BlockCursor {
            bytes: self.bytes,
            pos: self.body_start,
            a: 0,
            b: 0,
            c: 0,
            a_rem: 0,
            b_rem: 0,
            c_rem: 0,
            started: true, // a dead cursor unless the probe below arms it
            pa: Some(pa),
            pb,
            pc,
        };
        if let Ok(i) = dir.entries.binary_search_by_key(&pa, |e| e.a) {
            let e = &dir.entries[i];
            // State as if the main cursor had just consumed this group's
            // delta_a + num_b header: positioned at the first b-group.
            cursor.pos = e.pos;
            cursor.a = e.a;
            cursor.a_rem = e.a_rem_after;
            cursor.b_rem = e.num_b;
        }
        cursor
    }

    /// Stream the triples matching a (permuted) pattern, *without* decoding the
    /// whole block. `pa`/`pb`/`pc` are the bound components in this block's stored
    /// order (`None` = wildcard). The cursor walks the grouped body and:
    ///
    /// * **range-stops** once the leading component `a` exceeds a bound `pa` — the
    ///   a-groups are stored ascending, so nothing later can match (the early-out
    ///   that makes a leading-bound lookup `O(matches + preceding groups)` instead
    ///   of `O(whole block)`);
    /// * **group-skips** a/b groups that can't match (decoding their headers to
    ///   advance, but never building or emitting their triples);
    /// * **equality-filters** `pb`/`pc` without ever early-breaking inside a
    ///   c-list, so on a *valid* block the yielded set equals what
    ///   [`triples`](Self::triples) would yield filtered — even on corrupt bytes
    ///   it only ever yields fewer, never panics (every read is bounds-checked).
    ///
    /// Yields triples in this block's stored `(a, b, c)` order; callers map back
    /// to canonical `(s, p, o)` themselves.
    pub fn scan(&self, pa: Option<u32>, pb: Option<u32>, pc: Option<u32>) -> BlockCursor<'a> {
        BlockCursor {
            bytes: self.bytes,
            pos: self.body_start,
            a: 0,
            b: 0,
            c: 0,
            a_rem: 0,
            b_rem: 0,
            c_rem: 0,
            started: false,
            pa,
            pb,
            pc,
        }
    }
}

/// Read one uvarint at `*pos`, advancing it; `None` if truncated. Panic-free,
/// mirroring the decoder inside [`TripleBlock::try_triples`].
#[inline]
fn rd(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let (v, n) = read_uvarint(bytes.get(*pos..)?)?;
    *pos += n;
    Some(v as u32)
}

/// A byte-offset directory of a block's a-groups: one entry per group, sorted
/// by leading id (the storage order). Built once per block with
/// [`TripleBlock::group_directory`]; [`TripleBlock::scan_from`] then
/// binary-searches it to jump a probe straight to its group.
pub struct GroupDirectory {
    entries: Vec<DirEntry>,
}

impl GroupDirectory {
    /// Number of a-groups indexed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One a-group: its leading id, the byte offset of its first b-group header
/// (right after `num_b`), its b-group count, and how many a-groups follow it.
struct DirEntry {
    a: u32,
    pos: usize,
    num_b: u32,
    a_rem_after: u32,
}

/// A lazy cursor over a [`TripleBlock`] body produced by [`TripleBlock::scan`].
/// Holds only the block bytes and the delta-decode accumulators, so it borrows
/// the block's bytes but allocates nothing.
pub struct BlockCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    // Running delta accumulators for the current (a, b, c).
    a: u32,
    b: u32,
    c: u32,
    // Groups/items not yet consumed at each level.
    a_rem: u32,
    b_rem: u32,
    c_rem: u32,
    started: bool,
    pa: Option<u32>,
    pb: Option<u32>,
    pc: Option<u32>,
}

impl Iterator for BlockCursor<'_> {
    type Item = Triple;

    fn next(&mut self) -> Option<Triple> {
        let bytes = self.bytes;
        if !self.started {
            self.a_rem = rd(bytes, &mut self.pos)?; // num_a
            self.started = true;
        }
        loop {
            // (1) Drain the c-list of the current matched (a, b) group. Every c is
            // decoded to keep the delta chain correct; only matches are emitted.
            while self.c_rem > 0 {
                self.c_rem -= 1;
                self.c = self.c.wrapping_add(rd(bytes, &mut self.pos)?);
                if self.pc.is_none_or(|z| z == self.c) {
                    return Some((self.a, self.b, self.c));
                }
            }
            // (2) Advance to the next b-group within the current a-group.
            while self.b_rem > 0 {
                self.b_rem -= 1;
                self.b = self.b.wrapping_add(rd(bytes, &mut self.pos)?);
                let num_c = rd(bytes, &mut self.pos)?;
                if self.pb.is_some_and(|y| y != self.b) {
                    for _ in 0..num_c {
                        rd(bytes, &mut self.pos)?; // group-skip: advance, never emit
                    }
                    continue;
                }
                self.c = 0; // the encoder resets prev_c per b-group
                self.c_rem = num_c;
                break;
            }
            if self.c_rem > 0 {
                continue; // re-enter (1) to drain the matched c-list
            }
            // (3) Advance to the next a-group.
            if self.a_rem == 0 {
                return None;
            }
            self.a_rem -= 1;
            self.a = self.a.wrapping_add(rd(bytes, &mut self.pos)?);
            let num_b = rd(bytes, &mut self.pos)?;
            self.b = 0; // the encoder resets prev_b per a-group
            if let Some(x) = self.pa {
                if self.a > x {
                    return None; // range-stop: a-groups are ascending
                }
                if self.a < x {
                    // skip this whole a-group (b-group headers + c-lists)
                    for _ in 0..num_b {
                        rd(bytes, &mut self.pos)?; // delta_b
                        let nc = rd(bytes, &mut self.pos)?;
                        for _ in 0..nc {
                            rd(bytes, &mut self.pos)?;
                        }
                    }
                    continue;
                }
            }
            self.b_rem = num_b;
        }
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
        assert!(blk.scan(None, None, None).next().is_none());
        assert!(blk.scan(Some(1), None, None).next().is_none());
    }

    /// The streaming `scan` cursor must, for every bound/unbound shape, yield
    /// exactly the full-decode result filtered by the same bounds.
    #[test]
    fn scan_matches_full_decode_every_shape() {
        let mut b = TripleBlockBuilder::new();
        for t in sample() {
            b.push(t);
        }
        let bytes = b.build();
        let blk = TripleBlock::parse(&bytes).unwrap();
        let all = blk.triples(); // sorted, deduped, ascending (a, b, c)

        let opt = |v: u32| [None, Some(v)];
        // Probe present values and an absent one in each position.
        for pa in opt(1).into_iter().chain([Some(5), Some(99)]) {
            for pb in opt(1).into_iter().chain([Some(2), Some(99)]) {
                for pc in opt(1).into_iter().chain([Some(9), Some(99)]) {
                    let want: Vec<Triple> = all
                        .iter()
                        .copied()
                        .filter(|&(a, bb, c)| {
                            pa.is_none_or(|x| x == a)
                                && pb.is_none_or(|x| x == bb)
                                && pc.is_none_or(|x| x == c)
                        })
                        .collect();
                    // The cursor preserves stored ascending order, so no re-sort.
                    let got: Vec<Triple> = blk.scan(pa, pb, pc).collect();
                    assert_eq!(got, want, "scan({pa:?},{pb:?},{pc:?})");
                }
            }
        }
    }

    /// A range-stop on a bound leading component must not over-read: once `a`
    /// passes the bound the cursor returns `None` and stops decoding.
    #[test]
    fn scan_range_stops_on_leading_bound() {
        let mut b = TripleBlockBuilder::new();
        for t in [(1, 1, 1), (1, 2, 2), (3, 1, 1), (5, 1, 1)] {
            b.push(t);
        }
        let bytes = b.build();
        let blk = TripleBlock::parse(&bytes).unwrap();
        let got: Vec<Triple> = blk.scan(Some(1), None, None).collect();
        assert_eq!(got, vec![(1, 1, 1), (1, 2, 2)]);
        // A bound `a` between stored groups yields nothing (and stops early).
        assert!(blk.scan(Some(2), None, None).next().is_none());
        assert!(blk.scan(Some(99), None, None).next().is_none());
    }

    /// Truncations and byte corruptions must never panic the cursor — it only
    /// ever yields a clean prefix (every read is bounds-checked).
    #[test]
    fn scan_never_panics_on_bad_bytes() {
        let mut b = TripleBlockBuilder::new();
        for t in sample() {
            b.push(t);
        }
        let bytes = b.build();
        for len in 0..bytes.len() {
            if let Ok(blk) = TripleBlock::parse(&bytes[..len]) {
                for pat in [(None, None, None), (Some(1u32), Some(1u32), Some(1u32))] {
                    let _ = blk.scan(pat.0, pat.1, pat.2).count();
                }
            }
        }
        for i in 0..bytes.len() {
            for v in [0x00u8, 0xff, 0x80, 0x7f] {
                let mut bad = bytes.clone();
                bad[i] = v;
                if let Ok(blk) = TripleBlock::parse(&bad) {
                    let _ = blk.scan(None, None, None).count();
                    let _ = blk.scan(Some(1), None, None).count();
                }
            }
        }
    }
}
