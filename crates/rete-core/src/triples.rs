//! Integer triple blocks: grouped, delta-coded adjacency with a zone map
//! (SPEC.md §6.1).
//!
//! A block stores triples `(a, b, c)` of dictionary IDs for one permutation,
//! sorted ascending. `a`/`b`/`c` are the permuted roles — for an SPO block
//! `a=subject, b=predicate, c=object`; for POS, `a=predicate, b=object,
//! c=subject`; and so on. The encoding is role-agnostic.

#[cfg(test)]
use crate::varint::read_uvarint;
use crate::varint::{uvarint_len, write_uvarint};

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
        encode_sorted_unique(&self.triples)
    }
}

struct EncodingPlan {
    zone: [u32; 7],
    num_a: u64,
    encoded_len: usize,
}

/// Plan the exact encoded size without materializing the nested a/b/c groups.
fn plan_sorted_unique(t: &[Triple]) -> EncodingPlan {
    let count = u32::try_from(t.len()).expect("triple block count exceeds u32::MAX");
    let (mut min_b, mut max_b) = (u32::MAX, 0u32);
    let (mut min_c, mut max_c) = (u32::MAX, 0u32);
    for &(_, b, c) in t {
        min_b = min_b.min(b);
        max_b = max_b.max(b);
        min_c = min_c.min(c);
        max_c = max_c.max(c);
    }
    let zone = [t[0].0, t[t.len() - 1].0, min_b, max_b, min_c, max_c, count];
    let mut encoded_len = zone
        .iter()
        .map(|&value| uvarint_len(value as u64))
        .sum::<usize>();

    let mut num_a = 0u64;
    let mut prev_a = 0u32;
    let mut i = 0usize;
    while i < t.len() {
        let a = t[i].0;
        num_a += 1;
        encoded_len = encoded_len
            .checked_add(uvarint_len((a - prev_a) as u64))
            .expect("encoded triple block length overflow");
        prev_a = a;

        let mut num_b = 0u64;
        let mut prev_b = 0u32;
        while i < t.len() && t[i].0 == a {
            let b = t[i].1;
            num_b += 1;
            encoded_len = encoded_len
                .checked_add(uvarint_len((b - prev_b) as u64))
                .expect("encoded triple block length overflow");
            prev_b = b;

            let start = i;
            let mut prev_c = 0u32;
            while i < t.len() && t[i].0 == a && t[i].1 == b {
                encoded_len = encoded_len
                    .checked_add(uvarint_len((t[i].2 - prev_c) as u64))
                    .expect("encoded triple block length overflow");
                prev_c = t[i].2;
                i += 1;
            }
            encoded_len = encoded_len
                .checked_add(uvarint_len((i - start) as u64))
                .expect("encoded triple block length overflow");
        }
        encoded_len = encoded_len
            .checked_add(uvarint_len(num_b))
            .expect("encoded triple block length overflow");
    }
    encoded_len = encoded_len
        .checked_add(uvarint_len(num_a))
        .expect("encoded triple block length overflow");

    EncodingPlan {
        zone,
        num_a,
        encoded_len,
    }
}

/// Encode a lexicographically sorted, duplicate-free triple slice directly.
/// Tile builders already establish this precondition, avoiding their previous
/// copy, re-sort, dedup, nested grouping allocations, and output growth.
pub(crate) fn encode_sorted_unique(t: &[Triple]) -> Vec<u8> {
    assert!(
        t.windows(2).all(|pair| pair[0] < pair[1]),
        "encode_sorted_unique requires triples sorted and unique"
    );
    if t.is_empty() {
        return vec![0; 8];
    }

    let plan = plan_sorted_unique(t);
    let mut out = Vec::with_capacity(plan.encoded_len);
    for value in plan.zone {
        write_uvarint(&mut out, value as u64);
    }
    write_uvarint(&mut out, plan.num_a);

    let mut prev_a = 0u32;
    let mut i = 0usize;
    while i < t.len() {
        let a = t[i].0;
        write_uvarint(&mut out, (a - prev_a) as u64);
        prev_a = a;

        let a_start = i;
        let mut num_b = 0u64;
        while i < t.len() && t[i].0 == a {
            num_b += 1;
            let b = t[i].1;
            while i < t.len() && t[i].0 == a && t[i].1 == b {
                i += 1;
            }
        }
        write_uvarint(&mut out, num_b);

        i = a_start;
        let mut prev_b = 0u32;
        while i < t.len() && t[i].0 == a {
            let b = t[i].1;
            write_uvarint(&mut out, (b - prev_b) as u64);
            prev_b = b;
            let start = i;
            while i < t.len() && t[i].0 == a && t[i].1 == b {
                i += 1;
            }
            write_uvarint(&mut out, (i - start) as u64);
            let mut prev_c = 0u32;
            for &(_, _, c) in &t[start..i] {
                write_uvarint(&mut out, (c - prev_c) as u64);
                prev_c = c;
            }
        }
    }
    debug_assert_eq!(out.len(), plan.encoded_len);
    out
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
            read_u32_at(bytes, pos).ok_or(TripleError::Malformed("truncated"))
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
        let g = |pos: &mut usize| read_u32_at(self.bytes, pos);
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

    /// Build an a-group directory without checking each encoded byte access.
    ///
    /// # Safety
    ///
    /// `self.bytes` must be one complete, immutable block produced by rete's
    /// encoder. Every count and u32 LEB128 in the block must be canonical and
    /// terminate inside this same allocation. No recovery is possible for a
    /// malformed or truncated block; use [`group_directory`](Self::group_directory)
    /// for all untrusted files.
    #[cfg(feature = "unsafe-decode-bench")]
    pub(crate) unsafe fn group_directory_unchecked(&self) -> GroupDirectory {
        let bytes = self.bytes;
        let mut entries = Vec::new();
        let mut p = self.body_start;
        let next = |pos: &mut usize| {
            // SAFETY: upheld by this method's caller for the complete immutable
            // block; the directory walk follows the encoder's exact structure.
            unsafe { rd_unchecked(bytes, pos) }
        };
        let num_a = next(&mut p);
        entries.reserve(num_a as usize);
        let mut a = 0u32;
        for i in 0..num_a {
            a = a.wrapping_add(next(&mut p));
            let num_b = next(&mut p);
            entries.push(DirEntry {
                a,
                pos: p,
                num_b,
                a_rem_after: num_a - 1 - i,
            });
            for _ in 0..num_b {
                next(&mut p);
                let num_c = next(&mut p);
                for _ in 0..num_c {
                    next(&mut p);
                }
            }
        }
        GroupDirectory { entries }
    }

    /// Jump to a directory-selected a-group using unchecked byte decoding.
    ///
    /// # Safety
    ///
    /// The block must satisfy [`scan_unchecked`](Self::scan_unchecked)'s safety
    /// contract, and `dir` must have been built from this exact block image.
    #[cfg(feature = "unsafe-decode-bench")]
    pub(crate) unsafe fn scan_from_unchecked(
        &self,
        dir: &GroupDirectory,
        pa: u32,
        pb: Option<u32>,
        pc: Option<u32>,
    ) -> UncheckedBlockCursor<'a> {
        let mut cursor = UncheckedBlockCursor {
            bytes: self.bytes,
            pos: self.body_start,
            a: 0,
            b: 0,
            c: 0,
            a_rem: 0,
            b_rem: 0,
            c_rem: 0,
            started: true,
            pa: Some(pa),
            pb,
            pc,
        };
        if let Ok(i) = dir.entries.binary_search_by_key(&pa, |entry| entry.a) {
            let entry = &dir.entries[i];
            cursor.pos = entry.pos;
            cursor.a = entry.a;
            cursor.a_rem = entry.a_rem_after;
            cursor.b_rem = entry.num_b;
        }
        cursor
    }

    /// Stream matching triples without checking each encoded byte access.
    ///
    /// # Safety
    ///
    /// `self.bytes` must be one complete, immutable block produced by rete's
    /// encoder. Its count fields must describe exactly the following canonical
    /// u32 LEB128 values, all terminating inside this same allocation. The
    /// returned cursor must not outlive or observe mutation of that allocation.
    /// Invalid bytes can cause out-of-bounds reads; normal readers must use
    /// [`scan`](Self::scan).
    #[cfg(feature = "unsafe-decode-bench")]
    pub(crate) unsafe fn scan_unchecked(
        &self,
        pa: Option<u32>,
        pb: Option<u32>,
        pc: Option<u32>,
    ) -> UncheckedBlockCursor<'a> {
        UncheckedBlockCursor {
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
    read_u32_at(bytes, pos)
}

/// Decode one builder-emitted u32 LEB128 without leaving `*pos` advanced on a
/// malformed value. The overwhelmingly common one-byte value takes one checked
/// load and one branch; longer values are unrolled to the u32 maximum of five
/// bytes. The fifth byte may carry only four payload bits.
#[inline(always)]
fn read_u32_at(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    let start = *pos;
    let b0 = *bytes.get(start)?;
    if b0 < 0x80 {
        *pos = start + 1;
        return Some(u32::from(b0));
    }

    let b1 = *bytes.get(start + 1)?;
    let mut value = u32::from(b0 & 0x7f) | (u32::from(b1 & 0x7f) << 7);
    if b1 < 0x80 {
        *pos = start + 2;
        return Some(value);
    }

    let b2 = *bytes.get(start + 2)?;
    value |= u32::from(b2 & 0x7f) << 14;
    if b2 < 0x80 {
        *pos = start + 3;
        return Some(value);
    }

    let b3 = *bytes.get(start + 3)?;
    value |= u32::from(b3 & 0x7f) << 21;
    if b3 < 0x80 {
        *pos = start + 4;
        return Some(value);
    }

    let b4 = *bytes.get(start + 4)?;
    if b4 & 0xf0 != 0 {
        return None;
    }
    value |= u32::from(b4) << 28;
    *pos = start + 5;
    Some(value)
}

/// Read one builder-emitted u32 LEB128 without bounds or termination checks.
///
/// # Safety
///
/// `bytes` must be a complete immutable rete triple block, `*pos` must point
/// at its next encoded u32, and that value must terminate within five bytes in
/// this same allocation. The only derived pointer comes from `bytes`; no write
/// or alias is created, and `pos` advances only within the allocation.
#[cfg(feature = "unsafe-decode-bench")]
#[inline(always)]
unsafe fn rd_unchecked(bytes: &[u8], pos: &mut usize) -> u32 {
    let mut value = 0u32;
    let mut shift = 0u32;
    loop {
        // SAFETY: the caller guarantees that `pos` addresses the next byte of a
        // terminating u32 LEB128 inside this slice's allocation.
        let byte = unsafe { *bytes.get_unchecked(*pos) };
        *pos += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
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

/// Research-only cursor whose constructor requires a complete builder-produced
/// block. It is unavailable in default artifacts.
#[cfg(feature = "unsafe-decode-bench")]
pub(crate) struct UncheckedBlockCursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    a: u32,
    b: u32,
    c: u32,
    a_rem: u32,
    b_rem: u32,
    c_rem: u32,
    started: bool,
    pa: Option<u32>,
    pb: Option<u32>,
    pc: Option<u32>,
}

#[cfg(feature = "unsafe-decode-bench")]
impl UncheckedBlockCursor<'_> {
    #[inline(always)]
    fn read(&mut self) -> u32 {
        // SAFETY: only TripleBlock's unsafe constructors can create this cursor;
        // their contract guarantees every state-machine read remains in-bounds
        // and the immutable slice outlives the cursor.
        unsafe { rd_unchecked(self.bytes, &mut self.pos) }
    }
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

#[cfg(feature = "unsafe-decode-bench")]
impl Iterator for UncheckedBlockCursor<'_> {
    type Item = Triple;

    #[inline]
    fn next(&mut self) -> Option<Triple> {
        if !self.started {
            self.a_rem = self.read();
            self.started = true;
        }
        loop {
            while self.c_rem > 0 {
                self.c_rem -= 1;
                self.c = self.c.wrapping_add(self.read());
                if self.pc.is_none_or(|value| value == self.c) {
                    return Some((self.a, self.b, self.c));
                }
            }
            while self.b_rem > 0 {
                self.b_rem -= 1;
                self.b = self.b.wrapping_add(self.read());
                let num_c = self.read();
                if self.pb.is_some_and(|value| value != self.b) {
                    for _ in 0..num_c {
                        self.read();
                    }
                    continue;
                }
                self.c = 0;
                self.c_rem = num_c;
                break;
            }
            if self.c_rem > 0 {
                continue;
            }
            if self.a_rem == 0 {
                return None;
            }
            self.a_rem -= 1;
            self.a = self.a.wrapping_add(self.read());
            let num_b = self.read();
            self.b = 0;
            if let Some(bound_a) = self.pa {
                if self.a > bound_a {
                    return None;
                }
                if self.a < bound_a {
                    for _ in 0..num_b {
                        self.read();
                        let num_c = self.read();
                        for _ in 0..num_c {
                            self.read();
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

    #[test]
    fn checked_u32_decoder_covers_one_through_five_bytes() {
        let cases: &[(u32, &[u8])] = &[
            (0, &[0x00]),
            (127, &[0x7f]),
            (128, &[0x80, 0x01]),
            (16_384, &[0x80, 0x80, 0x01]),
            (1 << 21, &[0x80, 0x80, 0x80, 0x01]),
            (u32::MAX, &[0xff, 0xff, 0xff, 0xff, 0x0f]),
        ];
        for &(want, bytes) in cases {
            let mut pos = 0;
            assert_eq!(read_u32_at(bytes, &mut pos), Some(want));
            assert_eq!(pos, bytes.len());
        }
    }

    #[test]
    fn checked_u32_decoder_rejects_truncation_and_overflow_without_consuming() {
        for bytes in [
            &[0x80][..],
            &[0x80, 0x80][..],
            &[0x80, 0x80, 0x80][..],
            &[0x80, 0x80, 0x80, 0x80][..],
            &[0x80, 0x80, 0x80, 0x80, 0x80][..],
            &[0xff, 0xff, 0xff, 0xff, 0x10][..],
        ] {
            let mut pos = 0;
            assert_eq!(read_u32_at(bytes, &mut pos), None);
            assert_eq!(pos, 0);
        }
    }

    #[test]
    fn checked_u32_decoder_matches_generic_varint_for_builder_values() {
        for value in [
            0,
            1,
            127,
            128,
            16_383,
            16_384,
            1 << 21,
            u32::MAX - 1,
            u32::MAX,
        ] {
            let mut encoded = Vec::new();
            write_uvarint(&mut encoded, value as u64);
            let mut pos = 0;
            assert_eq!(read_u32_at(&encoded, &mut pos), Some(value));
            assert_eq!(pos, encoded.len());
            assert_eq!(read_uvarint(&encoded), Some((value as u64, encoded.len())));
        }
    }

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
    fn sorted_unique_encoder_matches_literal_format_bytes() {
        assert_eq!(encode_sorted_unique(&[]), vec![0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            encode_sorted_unique(&[(1, 2, 3)]),
            vec![1, 1, 2, 2, 3, 3, 1, 1, 1, 1, 2, 1, 3]
        );
        assert_eq!(
            encode_sorted_unique(&[(1, 2, 3), (1, 2, 5), (1, 4, 1), (3, 1, 2)]),
            vec![1, 3, 1, 4, 1, 5, 4, 2, 1, 2, 2, 2, 3, 2, 2, 1, 1, 2, 1, 1, 1, 2]
        );
    }

    #[test]
    #[should_panic(expected = "sorted and unique")]
    fn sorted_unique_encoder_rejects_duplicates() {
        encode_sorted_unique(&[(1, 2, 3), (1, 2, 3)]);
    }

    #[test]
    #[should_panic(expected = "sorted and unique")]
    fn sorted_unique_encoder_rejects_descending_input() {
        encode_sorted_unique(&[(2, 1, 1), (1, 1, 1)]);
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

    #[cfg(feature = "unsafe-decode-bench")]
    #[test]
    fn unchecked_cursor_matches_safe_every_pattern() {
        let mut builder = TripleBlockBuilder::new();
        for t in sample() {
            builder.push(t);
        }
        let bytes = builder.build();
        let block = TripleBlock::parse(&bytes).unwrap();

        let values = [None, Some(1), Some(2), Some(5), Some(99)];
        for pa in values {
            for pb in values {
                for pc in values {
                    let safe: Vec<_> = block.scan(pa, pb, pc).collect();
                    // SAFETY: `bytes` was just emitted by TripleBlockBuilder and
                    // remains immutable for the cursor's complete lifetime.
                    let unchecked: Vec<_> = unsafe { block.scan_unchecked(pa, pb, pc) }.collect();
                    assert_eq!(unchecked, safe, "scan({pa:?}, {pb:?}, {pc:?})");
                }
            }
        }

        let safe_dir = block.group_directory();
        // SAFETY: the complete immutable block was builder-produced above.
        let unchecked_dir = unsafe { block.group_directory_unchecked() };
        for pa in [0, 1, 2, 5, 99] {
            for pb in values {
                for pc in values {
                    let safe: Vec<_> = block.scan_from(&safe_dir, pa, pb, pc).collect();
                    // SAFETY: both the block and directory were produced by rete
                    // from the same complete immutable encoded byte allocation.
                    let unchecked: Vec<_> =
                        unsafe { block.scan_from_unchecked(&unchecked_dir, pa, pb, pc) }.collect();
                    assert_eq!(unchecked, safe, "scan_from({pa}, {pb:?}, {pc:?})");
                }
            }
        }
    }
}
