//! The graph-last quad orders of format `0x07` (SPEC.md §6.5, "Format 0x07
//! order set"): **SPOG**, **POSG** and **OSPG**, next to the graph-leading
//! **GSPO** that `0x06` already has.
//!
//! A quad row is `(a, b, c, g)`, `(a, b, c)` being the permuted `(s, p, o)`.
//! Rows are sorted lexicographically, so a lookup on a subject, a predicate and
//! object, or two of them, is a prefix of one order: the directory finds one
//! tile, not a pass over the file. The graph is the last column and costs about
//! one byte per row (see [`encode_tile`]).
//!
//! One [`Tiler`] decides the tile boundaries for every writer (in-RAM,
//! memory-bounded, convert), so they write the same bytes.

use crate::index::{GroupSizer, IndexPermutation, ALL_PERMS};
use crate::quad::{pack, unzigzag, zigzag};
use crate::varint::{read_uvarint, write_uvarint};

/// Quad order slots. `0..6` are the graph-leading orders (`G+X`, `X` = the
/// permutation of the same slot index); `6..9` are the graph-last orders.
pub(crate) const NUM_SLOTS: usize = 9;
/// The first graph-last slot: SPOG. POSG is 7, OSPG is 8.
pub(crate) const FIRST_LAST_SLOT: usize = 6;
/// The mask of the graph-last slots.
pub(crate) const LAST_MASK: u16 = 0b1_1100_0000;

/// Is `si` a graph-last order?
pub(crate) fn is_last(si: usize) -> bool {
    si >= FIRST_LAST_SLOT
}

/// The permutation of the `(a, b, c)` columns of slot `si`.
pub(crate) fn perm_of(si: usize) -> IndexPermutation {
    ALL_PERMS[if is_last(si) {
        si - FIRST_LAST_SLOT
    } else {
        si
    }]
}

/// The slot of the graph-last order over `perm` (SPO, POS or OSP).
pub(crate) fn last_slot(perm: IndexPermutation) -> usize {
    FIRST_LAST_SLOT + perm.section_index()
}

/// `G+SPO`, `SPO+G`, ...
pub(crate) fn slot_name(si: usize) -> String {
    let p = perm_of(si).name();
    if is_last(si) {
        format!("{p}+G")
    } else {
        format!("G+{p}")
    }
}

/// Which of `(a, b, c)` holds the subject.
pub(crate) fn subject_col(perm: IndexPermutation) -> usize {
    let r = perm.roles();
    r.iter().position(|&x| x == 0).unwrap_or(0)
}

/// A row of a graph-last order: `(a, b, c, g)`.
pub(crate) type Row4 = (u32, u32, u32, u32);

fn subj(r: &Row4, col: usize) -> u32 {
    match col {
        0 => r.0,
        1 => r.1,
        _ => r.2,
    }
}

fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// The graph code of a row: `zigzag((g - s) - (prev_g - prev_s))`, where `s` is
/// the row's subject id and `prev` the previous row of the same tile; the first
/// row of a tile has `prev_g - prev_s = 0`. Both ids come from string-sorted
/// role dictionaries, so `g - s` is nearly constant along a run, which makes
/// this one byte for 85 to 93% of the rows (measured on the 2018 nanopubs).
fn graph_code(g: u32, s: u32, prev: i64) -> (u64, i64) {
    let d = g as i64 - s as i64;
    (zigzag(d - prev), d)
}

/// Encode one tile's rows (sorted, no duplicates, not empty), before the codec:
///
/// ```text
/// varint num_a
/// per a-group:  varint Δa          (from the previous a of the tile; the first absolute)
///               varint num_b
///   per b-group: varint Δb         (from the previous b within this a; the first from 0)
///                varint num_rows
///     per row:   varint Δc         (from the previous c within this b; the first from 0;
///                                   0 again when the triple is in several graphs)
///                varint graph code (see `graph_code`)
/// ```
pub(crate) fn encode_tile(rows: &[Row4], scol: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(rows.len() * 3 + 16);
    let mut num_a = 0u64;
    let mut i = 0;
    while i < rows.len() {
        let a = rows[i].0;
        while i < rows.len() && rows[i].0 == a {
            i += 1;
        }
        num_a += 1;
    }
    write_uvarint(&mut out, num_a);
    let mut prev_a = 0u32;
    let mut prev_gs = 0i64;
    let mut i = 0;
    while i < rows.len() {
        let a = rows[i].0;
        let mut j = i;
        let mut num_b = 0u64;
        let mut last_b = None;
        while j < rows.len() && rows[j].0 == a {
            if last_b != Some(rows[j].1) {
                num_b += 1;
                last_b = Some(rows[j].1);
            }
            j += 1;
        }
        write_uvarint(&mut out, (a - prev_a) as u64);
        write_uvarint(&mut out, num_b);
        prev_a = a;
        let mut prev_b = 0u32;
        let mut k = i;
        while k < j {
            let b = rows[k].1;
            let mut l = k;
            while l < j && rows[l].1 == b {
                l += 1;
            }
            write_uvarint(&mut out, (b - prev_b) as u64);
            write_uvarint(&mut out, (l - k) as u64);
            prev_b = b;
            let mut prev_c = 0u32;
            for r in &rows[k..l] {
                write_uvarint(&mut out, (r.2 - prev_c) as u64);
                let (code, d) = graph_code(r.3, subj(r, scol), prev_gs);
                write_uvarint(&mut out, code);
                prev_gs = d;
                prev_c = r.2;
            }
            k = l;
        }
        i = j;
    }
    out
}

/// Decode a tile ([`encode_tile`]) into `out`. Bounds-safe: `None` on a
/// malformed varint, an overrun or an id outside `u32`.
pub(crate) fn decode_tile(body: &[u8], scol: usize, out: &mut Vec<Row4>) -> Option<()> {
    let mut pos = 0usize;
    let mut take = || -> Option<u64> {
        let (v, n) = read_uvarint(body.get(pos..)?)?;
        pos += n;
        Some(v)
    };
    let num_a = u32::try_from(take()?).ok()?;
    let mut a = 0u32;
    let mut prev_gs = 0i64;
    for _ in 0..num_a {
        a = a.checked_add(u32::try_from(take()?).ok()?)?;
        let num_b = u32::try_from(take()?).ok()?;
        let mut b = 0u32;
        for _ in 0..num_b {
            b = b.checked_add(u32::try_from(take()?).ok()?)?;
            let n = u32::try_from(take()?).ok()?;
            let mut c = 0u32;
            for _ in 0..n {
                c = c.checked_add(u32::try_from(take()?).ok()?)?;
                let code = take()?;
                if out.len() >= body.len() {
                    return None; // every row costs at least two bytes
                }
                let mut row = (a, b, c, 0u32);
                let s = subj(&row, scol) as i64;
                let d = prev_gs.checked_add(unzigzag(code))?;
                prev_gs = d;
                row.3 = u32::try_from(s.checked_add(d)?).ok()?;
                out.push(row);
            }
        }
    }
    Some(())
}

/// One encoded tile, ready for the payload: its leading keys, the synopsis of
/// the two trailing columns `(min c, max c, min g, max g)`, its stored bytes.
pub(crate) struct EncodedTile {
    pub(crate) first: (u32, u32),
    pub(crate) last: (u32, u32),
    pub(crate) syn: (u32, u32, u32, u32),
    pub(crate) bytes: Vec<u8>,
    pub(crate) raw: bool,
}

/// Encode and pack one tile.
pub(crate) fn encode_packed(rows: &[Row4], scol: usize, codec: u8) -> EncodedTile {
    let (mut min_c, mut max_c, mut min_g, mut max_g) = (u32::MAX, 0u32, u32::MAX, 0u32);
    for r in rows {
        min_c = min_c.min(r.2);
        max_c = max_c.max(r.2);
        min_g = min_g.min(r.3);
        max_g = max_g.max(r.3);
    }
    let first = (rows[0].0, rows[0].1);
    let last = (rows[rows.len() - 1].0, rows[rows.len() - 1].1);
    let (bytes, raw) = pack(codec, encode_tile(rows, scol));
    EncodedTile {
        first,
        last,
        syn: (min_c, max_c, min_g, max_g),
        bytes,
        raw,
    }
}

/// Encode a batch of tiles, in parallel when the `parallel` feature is on
/// (order preserved, so the output is identical either way).
pub(crate) fn encode_batch(batch: &[Vec<Row4>], scol: usize, codec: u8) -> Vec<EncodedTile> {
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        batch
            .par_iter()
            .map(|t| encode_packed(t, scol, codec))
            .collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        batch
            .iter()
            .map(|t| encode_packed(t, scol, codec))
            .collect()
    }
}

/// Streaming tile cutter over rows sorted by `(a, b, c, g)`.
///
/// Whole leading-id (`a`) groups are packed until the estimated encoded size
/// would pass the budget; a group larger than the budget on its own is cut
/// mid-group into consecutive tiles, the mega-group rule of the other tilers.
/// Holds one tile and one group, so memory is bounded by the budget whatever
/// the input. The estimate counts the same bytes [`encode_tile`] writes (the
/// graph code against the previous row of the tile), so tiles come out near
/// the budget; only determinism matters for the cuts.
pub(crate) struct Tiler {
    budget: usize,
    scol: usize,
    tile: Vec<Row4>,
    tile_size: usize,
    /// `a` of the last group packed into the tile.
    tile_a: Option<u32>,
    /// `g - s` of the last row packed into the tile.
    tile_gs: i64,
    group: Vec<Row4>,
    sizer: GroupSizer,
    group_gs: i64,
    gbytes: usize,
    gtotal: usize,
    count: u64,
}

impl Tiler {
    /// A tiler at the index tile budget in force (`--tile-kib`).
    pub(crate) fn new(perm: IndexPermutation) -> Self {
        Self::with_budget(perm, crate::write_opts::tile_budget())
    }

    pub(crate) fn with_budget(perm: IndexPermutation, budget: usize) -> Self {
        Tiler {
            budget: budget.max(1),
            scol: subject_col(perm),
            tile: Vec::new(),
            tile_size: 0,
            tile_a: None,
            tile_gs: 0,
            group: Vec::new(),
            sizer: GroupSizer::start(0, 0),
            group_gs: 0,
            gbytes: 0,
            gtotal: 0,
            count: 0,
        }
    }

    /// Push the next row (strictly ascending); `emit` receives each finished
    /// tile's rows.
    pub(crate) fn push(&mut self, r: Row4, emit: &mut dyn FnMut(Vec<Row4>)) {
        self.count += 1;
        if let Some(first) = self.group.first() {
            if first.0 != r.0 {
                self.close_group(emit);
            }
        }
        if self.group.is_empty() {
            self.sizer = GroupSizer::start(r.0, self.tile_a.unwrap_or(0));
            self.group_gs = self.tile_gs;
            self.gbytes = 0;
        }
        let (code, d) = graph_code(r.3, subj(&r, self.scol), self.group_gs);
        self.group_gs = d;
        self.gbytes += varint_len(code);
        self.group.push(r);
        self.gtotal = self.sizer.push(r.1, r.2) + self.gbytes;
        if self.gtotal > self.budget {
            // Mega-group cut: what is buffered plus the slice measured so far
            // becomes one tile; the group goes on in a fresh one.
            self.tile.append(&mut self.group);
            self.flush(emit);
            self.gtotal = 0;
        }
    }

    fn close_group(&mut self, emit: &mut dyn FnMut(Vec<Row4>)) {
        let Some(&(a, ..)) = self.group.first() else {
            return;
        };
        if !self.tile.is_empty() && self.tile_size + self.gtotal > self.budget {
            self.flush(emit);
        }
        self.tile_size += self.gtotal;
        self.tile_a = Some(a);
        self.tile_gs = self.group_gs;
        self.tile.append(&mut self.group);
        self.gtotal = 0;
    }

    fn flush(&mut self, emit: &mut dyn FnMut(Vec<Row4>)) {
        if !self.tile.is_empty() {
            emit(std::mem::take(&mut self.tile));
        }
        self.tile_size = 0;
        self.tile_a = None;
        self.tile_gs = 0;
    }

    /// Close the last group and tile; the number of rows pushed.
    pub(crate) fn finish(mut self, emit: &mut dyn FnMut(Vec<Row4>)) -> u64 {
        self.close_group(emit);
        self.flush(emit);
        self.count
    }
}

/// The columns `(a, b, c, g)` of a quad `(g, s, p, o)` in the graph-last order
/// over `perm`.
pub(crate) fn row_of(perm: IndexPermutation, g: u32, s: u32, p: u32, o: u32) -> Row4 {
    let (a, b, c) = perm.forward((s, p, o));
    (a, b, c, g)
}

/// The quad `(g, (s, p, o))` of a graph-last row.
pub(crate) fn quad_of(perm: IndexPermutation, r: &Row4) -> (u32, (u32, u32, u32)) {
    (r.3, perm.back((r.0, r.1, r.2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: u32) -> Vec<Row4> {
        let mut v = Vec::new();
        for a in 1..=n {
            for b in [3, 5, 9] {
                for c in [a, a + 1, a + 7] {
                    for g in [a, a + 2, 4000 + a] {
                        v.push((a * 2, b, c, g));
                    }
                }
            }
        }
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn tile_round_trips_in_every_subject_column() {
        let v = rows(40);
        for scol in 0..3 {
            let body = encode_tile(&v, scol);
            let mut got = Vec::new();
            decode_tile(&body, scol, &mut got).unwrap();
            assert_eq!(got, v);
        }
    }

    #[test]
    fn a_truncated_tile_is_refused() {
        let v = rows(5);
        let body = encode_tile(&v, 0);
        for cut in [0, 1, body.len() / 2, body.len() - 1] {
            let mut got = Vec::new();
            assert!(
                decode_tile(&body[..cut], 0, &mut got).is_none() || got.len() < v.len(),
                "cut {cut}"
            );
        }
    }

    #[test]
    fn the_tiler_covers_every_row_once_and_cuts_a_mega_group() {
        let v = rows(300);
        for budget in [40usize, 500, 64 << 10] {
            let mut t = Tiler::with_budget(IndexPermutation::Spo, budget);
            let mut tiles: Vec<Vec<Row4>> = Vec::new();
            for &r in &v {
                t.push(r, &mut |x| tiles.push(x));
            }
            assert_eq!(t.finish(&mut |x| tiles.push(x)), v.len() as u64);
            let flat: Vec<Row4> = tiles.iter().flatten().copied().collect();
            assert_eq!(flat, v);
            if budget == 40 {
                assert!(tiles.len() > 300, "tiny budget cuts inside a group");
            }
        }
    }
}
