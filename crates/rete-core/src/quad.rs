//! The **quad index** (SPEC.md §6.5): every named-graph quad stored once, as the
//! fourth column of graph-leading permutations (`G+SPO`, `G+POS`, `G+OSP`, and
//! `G+SOP`, `G+PSO`, `G+OPS` when the file carries six), cut into ~64 KiB tiles
//! that many graphs share, behind a sorted graph directory.
//!
//! It replaces the per-graph layout of the NAMED_GRAPHS section (kind 5), where
//! every graph carried its IRI as raw text and a full permutation container with
//! one zstd frame per tile, chained with no directory: on the OpenCitations
//! sample that was ~650 B per ~10-quad graph and a sequential walk to reach any
//! one of them. Here a graph costs a few bytes of segment header per order, and
//! a reader resolves it by binary search: dictionary (IRI → id), then each
//! order's tile directory.
//!
//! The query engine never sees the shared tiles. [`QuadNamed::graph_at`] hands
//! out a **view**: an ordinary remote-style [`GraphIndex`] over one graph's slice
//! of the tiles, whose tile loader cuts that graph's segment out of the shared
//! (cached, decompressed) tile and re-encodes it as a §6.1 block. So routing,
//! synopsis pruning, merge joins and every `Rete` API work unchanged.
//!
//! One [`QuadTiler`] decides tile boundaries for both builders (in-RAM here,
//! external in `extbuild`), so their outputs are byte-identical.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::chunk_cache::{CacheEntry, ChunkCache};
use crate::dictionary::Dictionary;
use crate::file::{compress, decompress, ByteRange, FileError, CODEC_NONE};
use crate::index::{GraphIndex, GroupSizer, IndexPermutation, PermSet, ALL_PERMS, NUM_PERMS};
use crate::reader::RangeReader;
use crate::triples::{Triple, TripleBlockBuilder};
use crate::varint::{read_uvarint, write_uvarint};

/// Magic of the QUAD_INDEX section and of the seal in the index container.
pub(crate) const QUAD_MAGIC: [u8; 4] = *b"RQIX";
/// Layout version of the section (and the seal).
pub(crate) const QUAD_VERSION: u8 = 1;
/// Seal: magic, version, 16-byte blake3 digest of the QUAD_INDEX section.
pub(crate) const SEAL_LEN: usize = 4 + 1 + 16;
/// Graphs per graph-directory block.
pub(crate) const DIR_BLOCK_GRAPHS: u64 = 1024;
/// Bytes charged against the tile budget for one segment header (Δg, zigzag
/// first a, body length, a-group count). An estimate: only determinism matters,
/// so both builders cut at the same rows.
const SEGMENT_OVERHEAD: usize = 8;
/// Upper bound on the preamble: 8 fixed bytes, four varints, at most
/// `2 + 2·6` part lengths.
const PREAMBLE_MAX: u64 = 8 + 4 * 10 + 14 * 10;
/// Ceiling of the sequential read-ahead over tiles or directory blocks.
const READ_AHEAD_MAX: usize = 64;
/// Entries per lazily-allocated slab of per-graph cells.
const ENTRY_SLAB: usize = 1024;
/// Largest span of all orders' tile directories fetched together on first use
/// (one request instead of one per order).
const DIRS_TOGETHER_MAX: u64 = 4 << 20;

/// A quad in one graph-leading order: `(g, a, b, c)`, `(a, b, c)` being the
/// permuted `(s, p, o)`.
pub(crate) type QuadRow = (u32, u32, u32, u32);

/// The orders a quad index may carry: exactly the file's default-graph set,
/// and only one of the two the CLI writes. Three plus the seal is four
/// container sections and six plus the seal is seven; a five-order set would
/// make seven-minus-one = six, which a 0.3.0–0.3.3 reader would accept (see
/// `docs/compatibility.md`, "named graphs in the quad index").
pub(crate) fn check_perms(perms: PermSet) -> Result<(), FileError> {
    if perms == PermSet::ALL || perms == PermSet::CORE {
        Ok(())
    } else {
        Err(FileError::Container(
            "the quad index needs the 3- or 6-permutation set",
        ))
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// Read one varint at `*pos`, advancing it; `None` when truncated.
fn rd64(bytes: &[u8], pos: &mut usize) -> Option<u64> {
    let (v, n) = read_uvarint(bytes.get(*pos..)?)?;
    *pos += n;
    Some(v)
}

/// [`rd64`] for a value that must fit an id.
fn rd32(bytes: &[u8], pos: &mut usize) -> Option<u32> {
    u32::try_from(rd64(bytes, pos)?).ok()
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Streaming tile cutter over rows sorted by `(g, a, b, c)` and deduplicated.
///
/// Whole `(g, a)` groups are packed until the estimated encoded size would pass
/// the budget; a group larger than the budget on its own is cut mid-group into
/// consecutive tiles, the mega-group rule of `index::build_tiles`. A graph may
/// run on into the next tile, where it opens a new segment. Holds one tile plus
/// one group slice, so memory is bounded by the budget whatever the input.
pub(crate) struct QuadTiler {
    budget: usize,
    tile: Vec<QuadRow>,
    tile_size: usize,
    group: Vec<QuadRow>,
    sizer: GroupSizer,
    gextra: usize,
    gtotal: usize,
    /// `(g, a)` of the last group packed into the current tile.
    prev: Option<(u32, u32)>,
    count: u64,
}

impl QuadTiler {
    /// A tiler at the index tile budget in force (`--tile-kib`,
    /// `crate::write_opts`; 64 KiB by default), like the default graph's tiles.
    pub(crate) fn new() -> Self {
        Self::with_budget(crate::write_opts::tile_budget())
    }

    pub(crate) fn with_budget(budget: usize) -> Self {
        QuadTiler {
            budget: budget.max(1),
            tile: Vec::new(),
            tile_size: 0,
            group: Vec::new(),
            sizer: GroupSizer::start(0, 0),
            gextra: 0,
            gtotal: 0,
            prev: None,
            count: 0,
        }
    }

    /// Push the next row (strictly ascending); `emit` receives each finished
    /// tile's rows.
    pub(crate) fn push(&mut self, q: QuadRow, emit: &mut dyn FnMut(Vec<QuadRow>)) {
        self.count += 1;
        if let Some(&(g0, a0, _, _)) = self.group.first() {
            if (q.0, q.1) != (g0, a0) {
                self.close_group(emit);
            }
        }
        if self.group.is_empty() {
            let (base, seg) = match self.prev {
                Some((pg, pa)) if pg == q.0 && !self.tile.is_empty() => (pa, 0),
                _ => (q.1, SEGMENT_OVERHEAD),
            };
            self.sizer = GroupSizer::start(q.1, base);
            self.gextra = seg;
        }
        self.group.push(q);
        self.gtotal = self.sizer.push(q.2, q.3) + self.gextra;
        if self.gtotal > self.budget {
            // Mega-group cut: everything buffered plus the slice measured so
            // far becomes one tile; the group goes on in a fresh one.
            self.tile.append(&mut self.group);
            self.flush(emit);
            self.gtotal = 0;
        }
    }

    fn close_group(&mut self, emit: &mut dyn FnMut(Vec<QuadRow>)) {
        let Some(&(g, a, _, _)) = self.group.first() else {
            return;
        };
        if !self.tile.is_empty() && self.tile_size + self.gtotal > self.budget {
            self.flush(emit);
        }
        self.tile_size += self.gtotal;
        self.prev = Some((g, a));
        self.tile.append(&mut self.group);
        self.gtotal = 0;
    }

    fn flush(&mut self, emit: &mut dyn FnMut(Vec<QuadRow>)) {
        if !self.tile.is_empty() {
            emit(std::mem::take(&mut self.tile));
        }
        self.tile_size = 0;
        self.prev = None;
    }

    /// Close the last group and tile.
    pub(crate) fn finish(mut self, emit: &mut dyn FnMut(Vec<QuadRow>)) -> u64 {
        self.close_group(emit);
        self.flush(emit);
        self.count
    }
}

/// One tile, encoded and packed: its key range, the synopsis of its two
/// non-leading columns, and its stored bytes (`raw` = not compressed).
pub(crate) struct EncodedTile {
    first: (u32, u32),
    last: (u32, u32),
    syn: (u32, u32, u32, u32),
    pub(crate) bytes: Vec<u8>,
    raw: bool,
}

/// Compress `body` with `codec`, keeping it raw when that does not shrink it.
fn pack(codec: u8, body: Vec<u8>) -> (Vec<u8>, bool) {
    if codec == CODEC_NONE {
        return (body, true);
    }
    let c = compress(codec, &body);
    if c.len() < body.len() {
        (c, false)
    } else {
        (body, true)
    }
}

/// Which trailing column of a graph-leading order holds the **subject**:
/// `(b, c)`. Within one graph the subjects are a handful of ids (OpenCitations:
/// a record's snapshots) that recur across every a-group, so in that column the
/// first value of a run is coded relative to the previous run's first value
/// (zigzag) instead of from 0 — measured on the M=1200 sample, it shrinks the
/// four orders with a trailing subject by 46–49% (G+POS 30.5 → 16.3 MB). The
/// object column is left alone: the same rule makes G+SPO 64% larger.
pub(crate) fn subject_columns(perm: IndexPermutation) -> (bool, bool) {
    let r = perm.roles();
    (r[1] == 0, r[2] == 0)
}

/// Encode one graph's rows of a tile (sorted `(a, b, c)`, first a carried in
/// the segment header): `num_a`, then the §6.1 groups with the first group's
/// `Δa` omitted — and, in the subject column ([`subject_columns`]), the first
/// value of each run as `zigzag(v − previous run's first v)`, 0-based per
/// segment.
fn encode_segment_body(rows: &[QuadRow], (zz_b, zz_c): (bool, bool), out: &mut Vec<u8>) {
    let mut num_a = 0u64;
    let mut i = 0;
    while i < rows.len() {
        let a = rows[i].1;
        while i < rows.len() && rows[i].1 == a {
            i += 1;
        }
        num_a += 1;
    }
    write_uvarint(out, num_a);
    let mut prev_a = rows[0].1;
    let (mut base_b, mut base_c) = (0i64, 0i64);
    let mut i = 0;
    let mut first = true;
    while i < rows.len() {
        let a = rows[i].1;
        let mut j = i;
        while j < rows.len() && rows[j].1 == a {
            j += 1;
        }
        if !first {
            write_uvarint(out, (a - prev_a) as u64);
        }
        first = false;
        prev_a = a;
        let group = &rows[i..j];
        let mut num_b = 0u64;
        let mut k = 0;
        while k < group.len() {
            let b = group[k].2;
            while k < group.len() && group[k].2 == b {
                k += 1;
            }
            num_b += 1;
        }
        write_uvarint(out, num_b);
        let mut prev_b = 0u32;
        let mut k = 0;
        while k < group.len() {
            let b = group[k].2;
            let mut l = k;
            while l < group.len() && group[l].2 == b {
                l += 1;
            }
            if zz_b && k == 0 {
                write_uvarint(out, zigzag(b as i64 - base_b));
                base_b = b as i64;
            } else {
                write_uvarint(out, (b - prev_b) as u64);
            }
            prev_b = b;
            write_uvarint(out, (l - k) as u64);
            let mut prev_c = 0u32;
            for (n, r) in group[k..l].iter().enumerate() {
                if zz_c && n == 0 {
                    write_uvarint(out, zigzag(r.3 as i64 - base_c));
                    base_c = r.3 as i64;
                } else {
                    write_uvarint(out, (r.3 - prev_c) as u64);
                }
                prev_c = r.3;
            }
            k = l;
        }
        i = j;
    }
}

/// Encode and pack one tile's rows (sorted, deduplicated, non-empty) of the
/// graph-leading order `perm`.
pub(crate) fn encode_tile(rows: &[QuadRow], codec: u8, perm: IndexPermutation) -> EncodedTile {
    let cols = subject_columns(perm);
    debug_assert!(!rows.is_empty());
    let mut segments = 0u64;
    let mut i = 0;
    while i < rows.len() {
        let g = rows[i].0;
        while i < rows.len() && rows[i].0 == g {
            i += 1;
        }
        segments += 1;
    }
    let mut out = Vec::new();
    write_uvarint(&mut out, segments);
    let mut prev_g = rows[0].0;
    let mut prev_fa = 0u32;
    let mut body = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let g = rows[i].0;
        let mut j = i;
        while j < rows.len() && rows[j].0 == g {
            j += 1;
        }
        write_uvarint(&mut out, (g - prev_g) as u64);
        prev_g = g;
        let fa = rows[i].1;
        write_uvarint(&mut out, zigzag(fa as i64 - prev_fa as i64));
        prev_fa = fa;
        body.clear();
        encode_segment_body(&rows[i..j], cols, &mut body);
        write_uvarint(&mut out, body.len() as u64);
        out.extend_from_slice(&body);
        i = j;
    }
    let (mut min_b, mut max_b, mut min_c, mut max_c) = (u32::MAX, 0u32, u32::MAX, 0u32);
    for r in rows {
        min_b = min_b.min(r.2);
        max_b = max_b.max(r.2);
        min_c = min_c.min(r.3);
        max_c = max_c.max(r.3);
    }
    let first = (rows[0].0, rows[0].1);
    let last = (rows[rows.len() - 1].0, rows[rows.len() - 1].1);
    let (bytes, raw) = pack(codec, out);
    EncodedTile {
        first,
        last,
        syn: (min_b, max_b, min_c, max_c),
        bytes,
        raw,
    }
}

/// Encode a batch of tiles, in parallel when the `parallel` feature is on
/// (order preserved, so the output is identical either way).
pub(crate) fn encode_tiles(
    batch: &[Vec<QuadRow>],
    codec: u8,
    perm: IndexPermutation,
) -> Vec<EncodedTile> {
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        batch
            .par_iter()
            .map(|t| encode_tile(t, codec, perm))
            .collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        batch.iter().map(|t| encode_tile(t, codec, perm)).collect()
    }
}

/// Accumulates one order's tile directory while its payloads are written
/// elsewhere (a `Vec` in RAM, a spill file in the external build).
#[derive(Default)]
pub(crate) struct PermDirWriter {
    entries: Vec<u8>,
    num_tiles: u64,
    prev_first_g: u32,
    payload_len: u64,
}

impl PermDirWriter {
    pub(crate) fn add(&mut self, t: &EncodedTile) {
        let e = &mut self.entries;
        write_uvarint(e, (t.first.0 - self.prev_first_g) as u64);
        write_uvarint(e, t.first.1 as u64);
        write_uvarint(e, (t.last.0 - t.first.0) as u64);
        write_uvarint(e, t.last.1 as u64);
        write_uvarint(e, ((t.bytes.len() as u64) << 1) | t.raw as u64);
        let (min_b, max_b, min_c, max_c) = t.syn;
        write_uvarint(e, min_b as u64);
        write_uvarint(e, (max_b - min_b) as u64);
        write_uvarint(e, min_c as u64);
        write_uvarint(e, (max_c - min_c) as u64);
        self.prev_first_g = t.first.0;
        self.num_tiles += 1;
        self.payload_len += t.bytes.len() as u64;
    }

    /// Bytes of the tiles' payloads written so far (the external builder
    /// spills them; it does not exist on wasm32).
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn payload_len(&self) -> u64 {
        self.payload_len
    }

    /// The finished directory part.
    pub(crate) fn finish(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.entries.len() + 10);
        write_uvarint(&mut out, self.num_tiles);
        out.extend_from_slice(&self.entries);
        out
    }
}

/// Accumulates the graph directory: `(graph id, quad count)` in ascending id
/// order, cut into blocks of [`DIR_BLOCK_GRAPHS`]. Block bodies go to `sink`;
/// the small block index stays in RAM.
pub(crate) struct GraphDirWriter {
    codec: u8,
    block: Vec<(u32, u64)>,
    index: Vec<u8>,
    num_blocks: u64,
    prev_first_g: u32,
    blocks_len: u64,
    graphs: u64,
}

impl GraphDirWriter {
    pub(crate) fn new(codec: u8) -> Self {
        GraphDirWriter {
            codec,
            block: Vec::with_capacity(DIR_BLOCK_GRAPHS as usize),
            index: Vec::new(),
            num_blocks: 0,
            prev_first_g: 0,
            blocks_len: 0,
            graphs: 0,
        }
    }

    pub(crate) fn push(
        &mut self,
        g: u32,
        count: u64,
        sink: &mut dyn std::io::Write,
    ) -> std::io::Result<()> {
        self.block.push((g, count));
        self.graphs += 1;
        if self.block.len() as u64 == DIR_BLOCK_GRAPHS {
            self.flush_block(sink)?;
        }
        Ok(())
    }

    fn flush_block(&mut self, sink: &mut dyn std::io::Write) -> std::io::Result<()> {
        let Some(&(first, _)) = self.block.first() else {
            return Ok(());
        };
        let mut body = Vec::with_capacity(self.block.len() * 3);
        let mut prev = first;
        for &(g, n) in &self.block {
            write_uvarint(&mut body, (g - prev) as u64);
            write_uvarint(&mut body, n);
            prev = g;
        }
        let (bytes, raw) = pack(self.codec, body);
        write_uvarint(&mut self.index, (first - self.prev_first_g) as u64);
        write_uvarint(&mut self.index, ((bytes.len() as u64) << 1) | raw as u64);
        self.prev_first_g = first;
        self.num_blocks += 1;
        self.blocks_len += bytes.len() as u64;
        sink.write_all(&bytes)?;
        self.block.clear();
        Ok(())
    }

    /// Flush the last block; returns `(block index part, blocks part length,
    /// graph count)`.
    pub(crate) fn finish(
        mut self,
        sink: &mut dyn std::io::Write,
    ) -> std::io::Result<(Vec<u8>, u64, u64)> {
        self.flush_block(sink)?;
        let mut out = Vec::with_capacity(self.index.len() + 10);
        write_uvarint(&mut out, self.num_blocks);
        out.extend_from_slice(&self.index);
        Ok((out, self.blocks_len, self.graphs))
    }
}

/// The section's preamble: magic, version, order mask, counts and the part
/// table (`part_lens` in part order — see SPEC.md §6.5).
pub(crate) fn encode_preamble(
    perms: PermSet,
    graph_count: u64,
    quad_count: u64,
    part_lens: &[u64],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&QUAD_MAGIC);
    out.push(QUAD_VERSION);
    out.push(perms.bits());
    out.extend_from_slice(&[0, 0]);
    write_uvarint(&mut out, graph_count);
    write_uvarint(&mut out, quad_count);
    write_uvarint(&mut out, DIR_BLOCK_GRAPHS);
    write_uvarint(&mut out, part_lens.len() as u64);
    for &l in part_lens {
        write_uvarint(&mut out, l);
    }
    out
}

/// A finished in-RAM QUAD_INDEX section.
pub(crate) struct EncodedQuadIndex {
    pub(crate) bytes: Vec<u8>,
    pub(crate) graph_count: u64,
    pub(crate) quad_count: u64,
}

#[cfg(feature = "parallel")]
fn sort_rows(v: &mut [QuadRow]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
}
#[cfg(not(feature = "parallel"))]
fn sort_rows(v: &mut [QuadRow]) {
    v.sort_unstable();
}

/// Tiles per encode batch: bounds the rows held outside the source vector.
const ENCODE_BATCH: usize = 256;

/// Encode a batch of finished tiles into `dir` + `payload`, emptying it.
fn drain_tiles(
    pending: &mut Vec<Vec<QuadRow>>,
    codec: u8,
    perm: IndexPermutation,
    dir: &mut PermDirWriter,
    payload: &mut Vec<u8>,
) {
    for t in encode_tiles(pending, codec, perm) {
        dir.add(&t);
        payload.extend_from_slice(&t.bytes);
    }
    pending.clear();
}

/// Tile one order's sorted rows into `payload`, returning its directory part.
fn tile_order(
    rows: &[QuadRow],
    codec: u8,
    perm: IndexPermutation,
    payload: &mut Vec<u8>,
) -> Vec<u8> {
    let mut dir = PermDirWriter::default();
    let mut pending: Vec<Vec<QuadRow>> = Vec::new();
    let mut tiler = QuadTiler::new();
    for &r in rows {
        tiler.push(r, &mut |t| pending.push(t));
        if pending.len() >= ENCODE_BATCH {
            drain_tiles(&mut pending, codec, perm, &mut dir, payload);
        }
    }
    tiler.finish(&mut |t| pending.push(t));
    drain_tiles(&mut pending, codec, perm, &mut dir, payload);
    dir.finish()
}

/// Build the QUAD_INDEX section from named-graph quads `(g, s, p, o)` (graph =
/// the graph name's subject id), for the orders in `perms`.
pub(crate) fn encode_quad_index(
    quads: Vec<[u32; 4]>,
    perms: PermSet,
    codec: u8,
) -> Result<EncodedQuadIndex, FileError> {
    check_perms(perms)?;
    // G+SPO rows are the input itself, sorted and deduplicated — every other
    // order is permuted from that (smaller) set.
    let mut spo: Vec<QuadRow> = quads
        .into_iter()
        .map(|q| (q[0], q[1], q[2], q[3]))
        .collect();
    sort_rows(&mut spo);
    spo.dedup();
    let quad_count = spo.len() as u64;

    // The graph directory: distinct graphs with their deduplicated counts.
    let mut gdir = GraphDirWriter::new(codec);
    let mut gblocks: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < spo.len() {
        let g = spo[i].0;
        let mut j = i;
        while j < spo.len() && spo[j].0 == g {
            j += 1;
        }
        gdir.push(g, (j - i) as u64, &mut gblocks)
            .map_err(FileError::Io)?;
        i = j;
    }
    let (gindex, gblocks_len, graph_count) = gdir.finish(&mut gblocks).map_err(FileError::Io)?;
    debug_assert_eq!(gblocks_len as usize, gblocks.len());

    let mut dirs: Vec<Vec<u8>> = Vec::with_capacity(perms.len());
    let mut payloads: Vec<Vec<u8>> = Vec::with_capacity(perms.len());
    for perm in perms.iter() {
        let mut payload = Vec::new();
        let dir = if perm == IndexPermutation::Spo {
            tile_order(&spo, codec, perm, &mut payload)
        } else {
            let mut rows: Vec<QuadRow> = spo
                .iter()
                .map(|&(g, s, p, o)| {
                    let (a, b, c) = perm.forward((s, p, o));
                    (g, a, b, c)
                })
                .collect();
            sort_rows(&mut rows);
            tile_order(&rows, codec, perm, &mut payload)
        };
        dirs.push(dir);
        payloads.push(payload);
    }

    let mut part_lens = vec![gindex.len() as u64];
    part_lens.extend(dirs.iter().map(|d| d.len() as u64));
    part_lens.push(gblocks.len() as u64);
    part_lens.extend(payloads.iter().map(|p| p.len() as u64));
    let mut bytes = encode_preamble(perms, graph_count, quad_count, &part_lens);
    bytes.extend_from_slice(&gindex);
    for d in &dirs {
        bytes.extend_from_slice(d);
    }
    bytes.extend_from_slice(&gblocks);
    for p in &payloads {
        bytes.extend_from_slice(p);
    }
    Ok(EncodedQuadIndex {
        bytes,
        graph_count,
        quad_count,
    })
}

/// The 16-byte digest a seal carries.
pub(crate) fn digest(bytes: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&blake3::hash(bytes).as_bytes()[..16]);
    out
}

/// The seal section appended to the index container of a quad-index file.
pub(crate) fn encode_seal(section_digest: [u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SEAL_LEN);
    out.extend_from_slice(&QUAD_MAGIC);
    out.push(QUAD_VERSION);
    out.extend_from_slice(&section_digest);
    out
}

/// Parse a seal, returning the digest it carries.
pub(crate) fn parse_seal(bytes: &[u8]) -> Result<[u8; 16], FileError> {
    if bytes.len() != SEAL_LEN || bytes[0..4] != QUAD_MAGIC {
        return Err(FileError::Container("malformed quad-index seal"));
    }
    if bytes[4] != QUAD_VERSION {
        return Err(FileError::Container(
            "unsupported quad-index version: this file needs a newer rete",
        ));
    }
    let mut d = [0u8; 16];
    d.copy_from_slice(&bytes[5..]);
    Ok(d)
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// The parsed preamble: counts and the absolute ranges of every part.
#[derive(Debug, Clone)]
struct Preamble {
    graph_count: u64,
    quad_count: u64,
    block_graphs: u64,
    /// Absolute byte ranges, in part order.
    parts: Vec<ByteRange>,
}

/// Parse the preamble of a section at absolute `base` of length `len` from its
/// first bytes `head`. Every field is untrusted.
fn parse_preamble(head: &[u8], base: u64, len: u64, perms: PermSet) -> Result<Preamble, FileError> {
    let bad = |why| Err(FileError::Container(why));
    if head.len() < 8 || head[0..4] != QUAD_MAGIC {
        return bad("malformed quad-index section");
    }
    if head[4] != QUAD_VERSION {
        return bad("unsupported quad-index version: this file needs a newer rete");
    }
    if head[5] != perms.bits() {
        return bad("quad-index order mask does not match the header permutation mask");
    }
    let mut pos = 8usize;
    let mut take = || -> Result<u64, FileError> {
        let (v, n) = read_uvarint(head.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated quad-index preamble"))?;
        pos += n;
        Ok(v)
    };
    let graph_count = take()?;
    let quad_count = take()?;
    let block_graphs = take()?;
    let part_count = take()?;
    let n = perms.len() as u64;
    if part_count != 2 + 2 * n {
        return bad("quad-index part count does not match its orders");
    }
    if block_graphs == 0 || graph_count > quad_count {
        return bad("malformed quad-index counts");
    }
    let mut lens = Vec::with_capacity(part_count as usize);
    for _ in 0..part_count {
        lens.push(take()?);
    }
    let mut at = pos as u64;
    let mut parts = Vec::with_capacity(lens.len());
    for l in lens {
        let end = at
            .checked_add(l)
            .filter(|&e| e <= len)
            .ok_or(FileError::Container("quad-index part overruns its section"))?;
        parts.push(ByteRange {
            offset: base + at,
            len: l,
        });
        at = end;
    }
    Ok(Preamble {
        graph_count,
        quad_count,
        block_graphs,
        parts,
    })
}

/// A decoded directory block, filled on first touch: `(graph id, quad count)`.
type BlockCell = OnceLock<Arc<[(u32, u64)]>>;

/// A lazily-allocated slab of per-graph cells.
type EntrySlab = OnceLock<Box<[QuadEntry]>>;

/// One graph-directory block reference.
#[derive(Debug, Clone, Copy)]
struct BlockRef {
    first_g: u32,
    range: ByteRange,
    raw: bool,
}

/// One tile of a graph-leading order, as its directory describes it.
#[derive(Debug, Clone, Copy)]
struct QTile {
    first_g: u32,
    first_a: u32,
    last_g: u32,
    last_a: u32,
    range: ByteRange,
    raw: bool,
    syn: (u32, u32, u32, u32),
}

/// Sequential read-ahead state of one stream (an order's tiles, or the
/// directory blocks): a miss right after the previous fetch doubles the window.
#[derive(Default, Clone, Copy)]
struct Ramp {
    next: usize,
    window: usize,
}

impl Ramp {
    fn window_for(&mut self, i: usize, exhaustive: bool) -> usize {
        if exhaustive {
            self.window = READ_AHEAD_MAX;
        } else if i == self.next && self.window > 0 {
            self.window = (self.window * 2).min(READ_AHEAD_MAX);
        } else {
            self.window = 1;
        }
        self.window
    }
}

/// The shared, graph-independent state of an open quad index. Views hold an
/// `Arc` of it in their tile loaders.
struct QuadCore {
    reader: Arc<dyn RangeReader + Send + Sync>,
    section: ByteRange,
    codec: u8,
    perms: PermSet,
    read_concurrency: usize,
    pre: OnceLock<Preamble>,
    blocks_index: OnceLock<Arc<[BlockRef]>>,
    blocks: OnceLock<Box<[BlockCell]>>,
    dirs: [OnceLock<Arc<[QTile]>>; NUM_PERMS],
    /// Decompressed tiles, keyed `(order section index, tile)`, shared by every
    /// view: one decompression serves all the graphs a tile holds.
    tiles: ChunkCache,
    /// `[0..6)` the orders' tile streams, `[6]` the directory blocks.
    ramps: Mutex<[Ramp; NUM_PERMS + 1]>,
    failed: AtomicBool,
}

impl QuadCore {
    fn fail(&self) {
        self.failed.store(true, Ordering::Relaxed);
    }

    fn pre(&self) -> Option<&Preamble> {
        if let Some(p) = self.pre.get() {
            return Some(p);
        }
        let head_len = self.section.len.min(PREAMBLE_MAX);
        let parsed = self
            .reader
            .read_at(self.section.offset, head_len)
            .map_err(FileError::Io)
            .and_then(|h| parse_preamble(&h, self.section.offset, self.section.len, self.perms));
        match parsed {
            Ok(p) => {
                let _ = self.pre.set(p);
                self.pre.get()
            }
            Err(_) => {
                self.fail();
                None
            }
        }
    }

    fn read_part(&self, k: usize) -> Option<Vec<u8>> {
        let r = *self.pre()?.parts.get(k)?;
        match self.reader.read_at(r.offset, r.len) {
            Ok(b) => Some(b),
            Err(_) => {
                self.fail();
                None
            }
        }
    }

    fn n_orders(&self) -> usize {
        self.perms.len()
    }

    fn blocks_index(&self) -> Option<&Arc<[BlockRef]>> {
        if let Some(b) = self.blocks_index.get() {
            return Some(b);
        }
        let pre = self.pre()?.clone();
        let bytes = self.read_part(0)?;
        let blocks_part = pre.parts[1 + self.n_orders()];
        let parsed = (|| -> Option<Vec<BlockRef>> {
            let mut pos = 0usize;
            let mut take = || -> Option<u64> {
                let (v, n) = read_uvarint(bytes.get(pos..)?)?;
                pos += n;
                Some(v)
            };
            let num = take()?;
            if num != pre.graph_count.div_ceil(pre.block_graphs) {
                return None;
            }
            let mut out = Vec::with_capacity((num as usize).min(bytes.len()));
            let mut first_g = 0u32;
            let mut at = 0u64;
            for i in 0..num {
                let d = u32::try_from(take()?).ok()?;
                if i > 0 && d == 0 {
                    return None; // blocks ascend strictly
                }
                first_g = first_g.checked_add(d)?;
                let packed = take()?;
                let clen = packed >> 1;
                let end = at.checked_add(clen).filter(|&e| e <= blocks_part.len)?;
                out.push(BlockRef {
                    first_g,
                    range: ByteRange {
                        offset: blocks_part.offset + at,
                        len: clen,
                    },
                    raw: packed & 1 == 1,
                });
                at = end;
            }
            Some(out)
        })();
        match parsed {
            Some(v) => {
                let n = v.len();
                let _ = self.blocks_index.set(Arc::from(v));
                let _ = self
                    .blocks
                    .set((0..n).map(|_| OnceLock::new()).collect::<Vec<_>>().into());
                self.blocks_index.get()
            }
            None => {
                self.fail();
                None
            }
        }
    }

    fn decode_block(
        &self,
        b: usize,
        r: &BlockRef,
        stored: &[u8],
        graph_count: u64,
        block_graphs: u64,
    ) -> Option<Arc<[(u32, u64)]>> {
        let body = if r.raw {
            stored.to_vec()
        } else {
            decompress(self.codec, stored).ok()?
        };
        let start = (b as u64).checked_mul(block_graphs)?;
        let n = graph_count.checked_sub(start)?.min(block_graphs) as usize;
        let mut out = Vec::with_capacity(n.min(body.len()));
        let mut pos = 0usize;
        let mut g = r.first_g;
        for i in 0..n {
            let (d, k) = read_uvarint(body.get(pos..)?)?;
            pos += k;
            if i > 0 && d == 0 {
                return None;
            }
            g = g.checked_add(u32::try_from(d).ok()?)?;
            let (c, k) = read_uvarint(body.get(pos..)?)?;
            pos += k;
            out.push((g, c));
        }
        Some(Arc::from(out))
    }

    /// Directory block `b` (decoded), with read-ahead over the blocks after it.
    fn block(&self, b: usize, exhaustive: bool) -> Option<Arc<[(u32, u64)]>> {
        let index = self.blocks_index()?.clone();
        let cells = self.blocks.get()?;
        if let Some(v) = cells.get(b)?.get() {
            return Some(v.clone());
        }
        let pre = self.pre()?.clone();
        let w = {
            let mut ramps = self.ramps.lock().unwrap();
            let w = ramps[NUM_PERMS].window_for(b, exhaustive);
            let end = (b + w).min(index.len());
            ramps[NUM_PERMS].next = end;
            end - b
        };
        let end = b + w;
        let span_start = index[b].range.offset;
        let span_end = index[end - 1].range.end();
        let span = match self.reader.read_at(span_start, span_end - span_start) {
            Ok(s) => s,
            Err(_) => {
                self.fail();
                return None;
            }
        };
        for (i, r) in index.iter().enumerate().take(end).skip(b) {
            if cells[i].get().is_some() {
                continue;
            }
            let lo = (r.range.offset - span_start) as usize;
            let stored = span.get(lo..lo + r.range.len as usize)?;
            match self.decode_block(i, r, stored, pre.graph_count, pre.block_graphs) {
                Some(v) => {
                    let _ = cells[i].set(v);
                }
                None => {
                    self.fail();
                    return None;
                }
            }
        }
        cells[b].get().cloned()
    }

    /// The graph at ordinal `i`: `(graph id, quad count)`.
    fn graph_at(&self, i: usize, exhaustive: bool) -> Option<(u32, u64)> {
        let bg = self.pre()?.block_graphs as usize;
        let block = self.block(i / bg, exhaustive)?;
        block.get(i % bg).copied()
    }

    /// The ordinal of graph id `g`, or `None` when the file has no such graph.
    fn ordinal_of(&self, g: u32) -> Option<usize> {
        let index = self.blocks_index()?.clone();
        let bi = index.partition_point(|r| r.first_g <= g);
        if bi == 0 {
            return None;
        }
        let block = self.block(bi - 1, false)?;
        let pos = block.binary_search_by_key(&g, |&(id, _)| id).ok()?;
        Some((bi - 1) * self.pre()?.block_graphs as usize + pos)
    }

    /// The tile directory of the order in section slot `si`. A view needs
    /// every order's directory, and they are adjacent parts, so the first miss
    /// reads all the missing ones in one request when together they are small
    /// ([`DIRS_TOGETHER_MAX`]); a larger file reads each on its own.
    fn dir(&self, si: usize) -> Option<Arc<[QTile]>> {
        if let Some(d) = self.dirs[si].get() {
            return Some(d.clone());
        }
        let pre = self.pre()?.clone();
        let n = self.n_orders();
        let first = pre.parts[1];
        let last = pre.parts[n];
        let together = last.end() - first.offset <= DIRS_TOGETHER_MAX;
        let want: Vec<IndexPermutation> = if together {
            self.perms
                .iter()
                .filter(|p| self.dirs[p.section_index()].get().is_none())
                .collect()
        } else {
            vec![ALL_PERMS[si]]
        };
        let span = ByteRange {
            offset: if together {
                first.offset
            } else {
                pre.parts[1 + self.perms.position(want[0])?].offset
            },
            len: if together {
                last.end() - first.offset
            } else {
                pre.parts[1 + self.perms.position(want[0])?].len
            },
        };
        let bytes = match self.reader.read_at(span.offset, span.len) {
            Ok(b) => b,
            Err(_) => {
                self.fail();
                return None;
            }
        };
        for perm in want {
            let k = self.perms.position(perm)?;
            let part = pre.parts[1 + k];
            let lo = (part.offset - span.offset) as usize;
            let parsed = bytes
                .get(lo..lo + part.len as usize)
                .and_then(|b| parse_tile_dir(b, pre.parts[2 + n + k]));
            match parsed {
                Some(v) => {
                    let _ = self.dirs[perm.section_index()].set(Arc::from(v));
                }
                None => {
                    self.fail();
                    return None;
                }
            }
        }
        self.dirs[si].get().cloned()
    }

    fn unpack(&self, t: &QTile, stored: &[u8]) -> Option<Arc<[u8]>> {
        if t.raw {
            Some(Arc::from(stored))
        } else {
            decompress(self.codec, stored).ok().map(Arc::from)
        }
    }

    /// Fault the tiles `want` (ascending) of order slot `si` into the shared
    /// cache, coalescing adjacent ones into single reads.
    fn fault(&self, si: usize, dir: &[QTile], want: &[usize]) -> Option<()> {
        let missing: Vec<usize> = want
            .iter()
            .copied()
            .filter(|&ti| !self.tiles.contains((si as u8, ti as u32)))
            .collect();
        if missing.is_empty() {
            return Some(());
        }
        let mut spans: Vec<(u64, u64)> = Vec::new();
        let mut members: Vec<Vec<usize>> = Vec::new();
        for &ti in &missing {
            let r = dir.get(ti)?.range;
            match spans.last_mut() {
                Some(last) if last.0 + last.1 == r.offset => {
                    last.1 += r.len;
                    members.last_mut()?.push(ti);
                }
                _ => {
                    spans.push((r.offset, r.len));
                    members.push(vec![ti]);
                }
            }
        }
        let blobs = match self.reader.read_many(&spans) {
            Ok(b) if b.len() == spans.len() => b,
            _ => {
                self.fail();
                return None;
            }
        };
        for ((start, _), (blob, tis)) in spans.iter().zip(blobs.iter().zip(&members)) {
            for &ti in tis {
                let t = &dir[ti];
                let lo = (t.range.offset - start) as usize;
                let stored = blob.get(lo..lo + t.range.len as usize)?;
                match self.unpack(t, stored) {
                    Some(body) => {
                        self.tiles.insert((si as u8, ti as u32), body);
                    }
                    None => {
                        self.fail();
                        return None;
                    }
                }
            }
        }
        Some(())
    }

    /// Tile `ti` of order slot `si`, decompressed, faulting it (and, when the
    /// access is sequential, the tiles after it) on a miss.
    fn tile(&self, si: usize, dir: &[QTile], ti: usize) -> Option<Arc<CacheEntry>> {
        if let Some(e) = self.tiles.get((si as u8, ti as u32)) {
            return Some(e);
        }
        let end = {
            let mut ramps = self.ramps.lock().unwrap();
            let w = ramps[si].window_for(ti, false);
            let end = (ti + w).min(dir.len());
            ramps[si].next = end;
            end
        };
        let want: Vec<usize> = (ti..end).collect();
        self.fault(si, dir, &want)?;
        // Inserted a moment ago; a cache cap smaller than one tile can have
        // evicted it already, in which case fetch it alone.
        if let Some(e) = self.tiles.get((si as u8, ti as u32)) {
            return Some(e);
        }
        let t = &dir[ti];
        let stored = match self.reader.read_at(t.range.offset, t.range.len) {
            Ok(b) => b,
            Err(_) => {
                self.fail();
                return None;
            }
        };
        let body = self.unpack(t, &stored)?;
        Some(self.tiles.insert((si as u8, ti as u32), body))
    }

    /// A view of graph `g`: a remote-style [`GraphIndex`] over its slice of
    /// every order's tiles. Reads the tile directories it needs, no tile.
    fn view(self: &Arc<Self>, g: u32, tile_cap: u64) -> Option<GraphIndex> {
        #[allow(clippy::type_complexity)]
        let mut directories: [Vec<(u32, u32, Option<(u32, u32, u32, u32)>)>; NUM_PERMS] =
            Default::default();
        let mut phys: [Vec<usize>; NUM_PERMS] = Default::default();
        let mut lens: [Vec<u32>; NUM_PERMS] = Default::default();
        let mut dirs: [Option<Arc<[QTile]>>; NUM_PERMS] = Default::default();
        for perm in self.perms.iter() {
            let si = perm.section_index();
            let dir = self.dir(si)?;
            let lo = dir.partition_point(|t| t.last_g < g);
            let hi = dir.partition_point(|t| t.first_g <= g);
            for (ti, t) in dir.iter().enumerate().take(hi).skip(lo) {
                let min_a = if t.first_g == g { t.first_a } else { 0 };
                let max_a = if t.last_g == g { t.last_a } else { u32::MAX };
                directories[si].push((min_a, max_a, Some(t.syn)));
                phys[si].push(ti);
                lens[si].push(t.range.len.min(u32::MAX as u64) as u32);
            }
            dirs[si] = Some(dir);
        }
        let shared = Arc::new((phys, dirs));
        let core = Arc::clone(self);
        let map = Arc::clone(&shared);
        let loader: crate::index::TileLoader = Box::new(move |si, ti| {
            let (phys, dirs) = &*map;
            let p = *phys.get(si)?.get(ti)?;
            let dir = dirs.get(si)?.as_ref()?;
            let entry = core.tile(si, dir, p)?;
            let block = segment_block(entry.body(), dir[p].first_g, g, ALL_PERMS[si]);
            if block.is_none() {
                core.fail();
            }
            block
        });
        let core = Arc::clone(self);
        let map = shared;
        let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis| {
            let (phys, dirs) = &*map;
            let dir = dirs.get(si)?.as_ref()?;
            let want: Vec<usize> = tis
                .iter()
                .map(|&ti| phys.get(si)?.get(ti).copied())
                .collect::<Option<_>>()?;
            core.fault(si, dir, &want)?;
            want.iter()
                .map(|&p| {
                    let entry = core.tile(si, dir, p)?;
                    segment_block(entry.body(), dir[p].first_g, g, ALL_PERMS[si])
                })
                .collect()
        });
        let mut index = GraphIndex::from_remote_directories(directories, self.perms, loader)
            .with_bulk_loader(bulk);
        index.set_tile_lens(lens);
        index.set_read_concurrency(self.read_concurrency);
        if tile_cap != u64::MAX {
            index.set_cache_cap(tile_cap);
        }
        Some(index)
    }
}

/// Parse one order's tile directory part; tile ranges are resolved against
/// `payload`, the order's payload part. Every field is untrusted.
fn parse_tile_dir(b: &[u8], payload: ByteRange) -> Option<Vec<QTile>> {
    let mut pos = 0usize;
    let num = rd64(b, &mut pos)?;
    let mut out = Vec::with_capacity((num as usize).min(b.len()));
    let mut first_g = 0u32;
    let mut at = 0u64;
    let mut prev_key = (0u32, 0u32);
    for i in 0..num {
        first_g = first_g.checked_add(rd32(b, &mut pos)?)?;
        let first_a = rd32(b, &mut pos)?;
        let last_g = first_g.checked_add(rd32(b, &mut pos)?)?;
        let last_a = rd32(b, &mut pos)?;
        let packed = rd64(b, &mut pos)?;
        let min_b = rd32(b, &mut pos)?;
        let max_b = min_b.checked_add(rd32(b, &mut pos)?)?;
        let min_c = rd32(b, &mut pos)?;
        let max_c = min_c.checked_add(rd32(b, &mut pos)?)?;
        // Keys ascend (equal only across a mega-group cut).
        if (first_g, first_a) > (last_g, last_a) || (i > 0 && (first_g, first_a) < prev_key) {
            return None;
        }
        prev_key = (last_g, last_a);
        let clen = packed >> 1;
        let end = at.checked_add(clen).filter(|&e| e <= payload.len)?;
        out.push(QTile {
            first_g,
            first_a,
            last_g,
            last_a,
            range: ByteRange {
                offset: payload.offset + at,
                len: clen,
            },
            raw: packed & 1 == 1,
            syn: (min_b, max_b, min_c, max_c),
        });
        at = end;
    }
    Some(out)
}

/// Decode a segment body (see [`encode_segment_body`]) into `(a, b, c)` rows.
/// Bounds-safe: `None` on any malformed varint or overrun.
fn decode_segment_body(
    body: &[u8],
    first_a: u32,
    (zz_b, zz_c): (bool, bool),
    out: &mut Vec<Triple>,
) -> Option<()> {
    let mut pos = 0usize;
    // A zigzag-coded run start: `base + unzigzag(v)`, kept inside `u32`.
    let rebase = |base: &mut i64, v: u64| -> Option<u32> {
        let x = base.checked_add(unzigzag(v))?;
        *base = x;
        u32::try_from(x).ok()
    };
    let (mut base_b, mut base_c) = (0i64, 0i64);
    let num_a = rd32(body, &mut pos)?;
    let mut a = first_a;
    for i in 0..num_a {
        if i > 0 {
            a = a.checked_add(rd32(body, &mut pos)?)?;
        }
        let num_b = rd32(body, &mut pos)?;
        let mut b = 0u32;
        for j in 0..num_b {
            b = if zz_b && j == 0 {
                rebase(&mut base_b, rd64(body, &mut pos)?)?
            } else {
                b.checked_add(rd32(body, &mut pos)?)?
            };
            let num_c = rd32(body, &mut pos)?;
            let mut c = 0u32;
            for k in 0..num_c {
                c = if zz_c && k == 0 {
                    rebase(&mut base_c, rd64(body, &mut pos)?)?
                } else {
                    c.checked_add(rd32(body, &mut pos)?)?
                };
                if out.len() >= body.len() {
                    return None; // every row costs at least one byte
                }
                out.push((a, b, c));
            }
        }
    }
    Some(())
}

/// Walk a decompressed tile's segments, calling `f(g, first_a, body)` for each
/// until it returns `false`. `None` on a malformed tile.
fn walk_segments(
    tile: &[u8],
    first_g: u32,
    mut f: impl FnMut(u32, u32, &[u8]) -> bool,
) -> Option<()> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Option<u64> {
        let (v, n) = read_uvarint(tile.get(*pos..)?)?;
        *pos += n;
        Some(v)
    };
    let n = take(&mut pos)?;
    let mut g = first_g;
    let mut fa: i64 = 0;
    for _ in 0..n {
        g = g.checked_add(u32::try_from(take(&mut pos)?).ok()?)?;
        fa = fa.checked_add(unzigzag(take(&mut pos)?))?;
        let first_a = u32::try_from(fa).ok()?;
        let len = usize::try_from(take(&mut pos)?).ok()?;
        let body = tile.get(pos..pos.checked_add(len)?)?;
        pos += len;
        if !f(g, first_a, body) {
            break;
        }
    }
    Some(())
}

/// Graph `g`'s rows in a decompressed tile as a §6.1 block (an empty block when
/// the tile holds none of them). `None` on a malformed tile.
fn segment_block(tile: &[u8], first_g: u32, g: u32, perm: IndexPermutation) -> Option<Vec<u8>> {
    let cols = subject_columns(perm);
    let mut rows: Vec<Triple> = Vec::new();
    let mut ok = true;
    walk_segments(tile, first_g, |sg, first_a, body| {
        if sg == g {
            ok = decode_segment_body(body, first_a, cols, &mut rows).is_some();
            false
        } else {
            sg < g
        }
    })?;
    if !ok {
        return None;
    }
    let mut b = TripleBlockBuilder::new();
    for t in rows {
        b.push(t);
    }
    Some(b.build())
}

/// Every row of a decompressed tile as `(g, a, b, c)` — for tests and checks.
#[cfg(test)]
fn tile_rows(tile: &[u8], first_g: u32, perm: IndexPermutation) -> Option<Vec<QuadRow>> {
    let cols = subject_columns(perm);
    let mut out = Vec::new();
    let mut ok = true;
    walk_segments(tile, first_g, |g, first_a, body| {
        let mut rows = Vec::new();
        ok &= decode_segment_body(body, first_a, cols, &mut rows).is_some();
        out.extend(rows.into_iter().map(|(a, b, c)| (g, a, b, c)));
        ok
    })?;
    ok.then_some(out)
}

/// One graph's lazily-filled cells.
#[derive(Default)]
struct QuadEntry {
    name: OnceLock<String>,
    view: OnceLock<Box<GraphIndex>>,
}

/// The named graphs of a quad-index file, opened over a [`RangeReader`]:
/// nothing is read at open. The graph count costs the preamble (one small
/// read), a name one directory block plus one dictionary chunk, a graph's
/// index its orders' tile directories, and a scan the tiles it touches.
///
/// Failure contract (as for lazy tiles and chunks): a failed read or a
/// malformed record sets a sticky flag, the accessor returns `None`, nothing
/// failed is memoised.
pub(crate) struct QuadNamed {
    core: Arc<QuadCore>,
    entries: OnceLock<Box<[EntrySlab]>>,
    tile_cap: AtomicU64,
}

impl QuadNamed {
    pub(crate) fn new(
        reader: Arc<dyn RangeReader + Send + Sync>,
        section: ByteRange,
        codec: u8,
        perms: PermSet,
        read_concurrency: usize,
    ) -> Self {
        QuadNamed {
            core: Arc::new(QuadCore {
                reader,
                section,
                codec,
                perms,
                read_concurrency,
                pre: OnceLock::new(),
                blocks_index: OnceLock::new(),
                blocks: OnceLock::new(),
                dirs: Default::default(),
                tiles: ChunkCache::unlimited(),
                ramps: Mutex::new(Default::default()),
                failed: AtomicBool::new(false),
            }),
            entries: OnceLock::new(),
            tile_cap: AtomicU64::new(u64::MAX),
        }
    }

    /// Over an owned copy of the section's bytes (the resident opens).
    pub(crate) fn resident(bytes: Vec<u8>, codec: u8, perms: PermSet) -> Self {
        let len = bytes.len() as u64;
        Self::new(
            Arc::new(OwnedReader(Arc::from(bytes))),
            ByteRange { offset: 0, len },
            codec,
            perms,
            1,
        )
    }

    pub(crate) fn count(&self) -> usize {
        self.core.pre().map(|p| p.graph_count as usize).unwrap_or(0)
    }

    /// Deduplicated quads across every named graph (from the preamble).
    pub(crate) fn quad_count(&self) -> u64 {
        self.core.pre().map(|p| p.quad_count).unwrap_or(0)
    }

    fn entry(&self, i: usize) -> Option<&QuadEntry> {
        let n = self.count();
        if i >= n {
            return None;
        }
        let table = self.entries.get_or_init(|| {
            (0..n.div_ceil(ENTRY_SLAB))
                .map(|_| OnceLock::new())
                .collect::<Vec<_>>()
                .into()
        });
        let slab = table.get(i / ENTRY_SLAB)?.get_or_init(|| {
            let len = ENTRY_SLAB.min(n - (i / ENTRY_SLAB) * ENTRY_SLAB);
            (0..len).map(|_| QuadEntry::default()).collect()
        });
        slab.get(i % ENTRY_SLAB)
    }

    /// The `i`-th graph's quad count, from the directory (no tile read).
    pub(crate) fn quad_count_at(&self, i: usize) -> Option<u64> {
        self.core.graph_at(i, false).map(|(_, n)| n)
    }

    pub(crate) fn name_at(&self, dict: &Dictionary, i: usize, exhaustive: bool) -> Option<&str> {
        let e = self.entry(i)?;
        if let Some(n) = e.name.get() {
            return Some(n.as_str());
        }
        let (g, _) = self.core.graph_at(i, exhaustive)?;
        match dict.subject_term(g) {
            Some(name) => {
                let _ = e.name.set(name);
                e.name.get().map(|s| s.as_str())
            }
            None => {
                // The directory names a graph id the dictionary does not hold.
                self.core.fail();
                None
            }
        }
    }

    pub(crate) fn graph_at(
        &self,
        dict: &Dictionary,
        i: usize,
        exhaustive: bool,
    ) -> Option<(&str, &GraphIndex)> {
        let name = self.name_at(dict, i, exhaustive)?;
        let e = self.entry(i)?;
        if e.view.get().is_none() {
            let (g, _) = self.core.graph_at(i, exhaustive)?;
            let cap = self.tile_cap.load(Ordering::Relaxed);
            let view = self.core.view(g, cap)?;
            let _ = e.view.set(Box::new(view));
        }
        Some((name, e.view.get()?.as_ref()))
    }

    /// The `i`-th graph as an **owned, unmemoised** view, when it holds at most
    /// `max_quads` quads (from the directory) — for a walk over many graphs
    /// (`GRAPH ?g`), which then holds one small view at a time instead of one
    /// per graph. The view shares the decompressed-tile cache, so consecutive
    /// graphs of one tile still cost one decompression. `None` for a larger
    /// graph (use [`graph_at`](Self::graph_at)) or on a failed read.
    pub(crate) fn transient_view(&self, i: usize, max_quads: u64) -> Option<GraphIndex> {
        let (g, n) = self.core.graph_at(i, false)?;
        if n > max_quads {
            return None;
        }
        self.core.view(g, self.tile_cap.load(Ordering::Relaxed))
    }

    fn ordinal_of_name(&self, dict: &Dictionary, iri: &str) -> Option<usize> {
        let g = dict.subject_id(iri)?;
        self.core.ordinal_of(g)
    }

    /// The index of the graph named `iri`: dictionary lookup, then directory
    /// binary search — never a walk. `None` when the file has no such graph.
    pub(crate) fn find(&self, dict: &Dictionary, iri: &str) -> Option<&GraphIndex> {
        let i = self.ordinal_of_name(dict, iri)?;
        self.graph_at(dict, i, false).map(|(_, g)| g)
    }

    /// Drop graph `iri`'s view (and its decoded blocks); it re-opens lazily.
    pub(crate) fn release(&mut self, dict: &Dictionary, iri: &str) {
        let Some(i) = self.ordinal_of_name(dict, iri) else {
            return;
        };
        let Some(table) = self.entries.get_mut() else {
            return;
        };
        if let Some(slab) = table.get_mut(i / ENTRY_SLAB).and_then(|s| s.get_mut()) {
            if let Some(e) = slab.get_mut(i % ENTRY_SLAB) {
                e.view.take();
            }
        }
    }

    pub(crate) fn for_each_opened(&self, mut f: impl FnMut(&GraphIndex)) {
        if let Some(table) = self.entries.get() {
            for slab in table.iter().filter_map(|s| s.get()) {
                for e in slab.iter() {
                    if let Some(v) = e.view.get() {
                        f(v);
                    }
                }
            }
        }
    }

    pub(crate) fn load_incomplete(&self) -> bool {
        if self.core.failed.load(Ordering::Relaxed) {
            return true;
        }
        let mut bad = false;
        self.for_each_opened(|g| bad |= g.load_incomplete());
        bad
    }

    pub(crate) fn reset_load_failures(&self) {
        self.core.failed.store(false, Ordering::Relaxed);
        self.for_each_opened(|g| g.reset_load_failure());
    }

    /// Cap the shared tile cache and every view's block cache (both re-fault
    /// through loaders, so any cap is sound).
    pub(crate) fn set_tile_cap(&self, cap: u64) {
        self.tile_cap.store(cap, Ordering::Relaxed);
        self.core.tiles.set_cap(cap);
        self.for_each_opened(|g| g.set_cache_cap(cap));
    }

    /// Heap held outside the caches: the routing parts read so far, the
    /// per-graph cells, and the open views' directories.
    pub(crate) fn directory_bytes(&self) -> u64 {
        let c = &self.core;
        let mut bytes = 0u64;
        if let Some(b) = c.blocks_index.get() {
            bytes += (b.len() * std::mem::size_of::<BlockRef>()) as u64;
        }
        if let Some(cells) = c.blocks.get() {
            for v in cells.iter().filter_map(|c| c.get()) {
                bytes += (v.len() * 16) as u64;
            }
        }
        for d in c.dirs.iter().filter_map(|d| d.get()) {
            bytes += (d.len() * std::mem::size_of::<QTile>()) as u64;
        }
        if let Some(table) = self.entries.get() {
            for slab in table.iter().filter_map(|s| s.get()) {
                bytes += (slab.len() * std::mem::size_of::<QuadEntry>()) as u64;
                for e in slab.iter() {
                    if let Some(n) = e.name.get() {
                        bytes += n.capacity() as u64;
                    }
                    if let Some(v) = e.view.get() {
                        bytes += v.directory_bytes();
                    }
                }
            }
        }
        bytes
    }
}

/// A [`RangeReader`] over an owned buffer.
struct OwnedReader(Arc<[u8]>);

impl RangeReader for OwnedReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let start = usize::try_from(offset).map_err(std::io::Error::other)?;
        let end = start
            .checked_add(usize::try_from(len).map_err(std::io::Error::other)?)
            .filter(|&e| e <= self.0.len())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "read past the section")
            })?;
        Ok(self.0[start..end].to_vec())
    }
}

/// Structural check of a QUAD_INDEX section held in RAM: preamble, directory,
/// every tile decodes and its rows lie inside the directory's key range, and
/// the counts agree. `verify` uses the digest; this is for tests and `rete
/// verify --deep`-style checks.
#[cfg(test)]
pub(crate) fn check_section(bytes: &[u8], perms: PermSet, codec: u8) -> Result<u64, String> {
    let q = QuadNamed::resident(bytes.to_vec(), codec, perms);
    let pre = q.core.pre().ok_or("preamble")?.clone();
    let n = pre.graph_count as usize;
    let mut total = 0u64;
    for i in 0..n {
        total += q.core.graph_at(i, true).ok_or("graph")?.1;
    }
    if total != pre.quad_count {
        return Err(format!("directory counts {total} != {}", pre.quad_count));
    }
    for perm in perms.iter() {
        let si = perm.section_index();
        let dir = q.core.dir(si).ok_or("dir")?;
        let mut seen = 0u64;
        for (ti, t) in dir.iter().enumerate() {
            let e = q.core.tile(si, &dir, ti).ok_or("tile")?;
            let rows = tile_rows(e.body(), t.first_g, perm).ok_or("rows")?;
            if rows.first().map(|r| (r.0, r.1)) != Some((t.first_g, t.first_a))
                || rows.last().map(|r| (r.0, r.1)) != Some((t.last_g, t.last_a))
            {
                return Err(format!("{} tile {ti} key range", perm.name()));
            }
            seen += rows.len() as u64;
        }
        if seen != pre.quad_count {
            return Err(format!("{} holds {seen} rows", perm.name()));
        }
    }
    Ok(pre.quad_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::writer_codec;

    fn quads(graphs: u32, per: u32) -> Vec<[u32; 4]> {
        let mut out = Vec::new();
        for g in 1..=graphs {
            for k in 0..per {
                let s = g * 3 + (k % 3);
                let p = 1 + (k % 4);
                let o = 1 + (g * 7 + k * 13) % 500;
                out.push([g * 2, s, p, o]);
            }
        }
        out
    }

    #[test]
    fn zigzag_round_trips() {
        for v in [0i64, 1, -1, 2, -2, 1 << 40, -(1 << 40), i64::from(u32::MAX)] {
            assert_eq!(unzigzag(zigzag(v)), v);
        }
    }

    #[test]
    fn section_round_trips_every_order() {
        for perms in [PermSet::ALL, PermSet::CORE] {
            let q = quads(3000, 9);
            let enc = encode_quad_index(q.clone(), perms, writer_codec()).unwrap();
            assert_eq!(enc.graph_count, 3000);
            assert_eq!(
                check_section(&enc.bytes, perms, writer_codec()),
                Ok(enc.quad_count)
            );
            let mut want = q;
            want.sort_unstable();
            want.dedup();
            assert_eq!(enc.quad_count, want.len() as u64);
        }
    }

    #[test]
    fn tiles_are_shared_and_a_view_sees_only_its_graph() {
        let q = quads(5000, 10);
        let enc = encode_quad_index(q.clone(), PermSet::ALL, writer_codec()).unwrap();
        let named = QuadNamed::resident(enc.bytes, writer_codec(), PermSet::ALL);
        let dir = named.core.dir(0).unwrap();
        assert!(
            dir.len() < 100,
            "5000 small graphs must share tiles, got {} tiles",
            dir.len()
        );
        for g in [2u32, 4000, 10000] {
            let i = named.core.ordinal_of(g).unwrap();
            assert_eq!(named.core.graph_at(i, false).unwrap(), (g, 10));
            let view = named.core.view(g, u64::MAX).unwrap();
            let mut got = view.match_pattern((None, None, None));
            got.sort_unstable();
            let mut want: Vec<Triple> = q
                .iter()
                .filter(|r| r[0] == g)
                .map(|r| (r[1], r[2], r[3]))
                .collect();
            want.sort_unstable();
            want.dedup();
            assert_eq!(got, want, "graph {g}");
            // Every pattern shape routes inside the view like a plain index.
            let plain = crate::GraphIndexBuilder::from_triples(want.clone()).build();
            let (s, p, o) = want[0];
            for pat in [
                (Some(s), None, None),
                (None, Some(p), None),
                (None, None, Some(o)),
                (Some(s), Some(p), None),
                (None, Some(p), Some(o)),
                (Some(s), None, Some(o)),
                (Some(s), Some(p), Some(o)),
            ] {
                let mut a = view.match_pattern(pat);
                let mut b = plain.match_pattern(pat);
                a.sort_unstable();
                b.sort_unstable();
                assert_eq!(a, b, "graph {g} pattern {pat:?}");
            }
            assert!(!view.load_incomplete());
        }
        assert_eq!(named.core.ordinal_of(3), None, "odd ids are not graphs");
        assert_eq!(named.core.ordinal_of(u32::MAX), None);
    }

    #[test]
    fn a_big_graph_spans_tiles_and_mega_groups_split() {
        // One graph of 60k quads with a single predicate (a mega-group in POS)
        // between two small ones.
        let mut q = Vec::new();
        for k in 0..60_000u32 {
            q.push([10, 1 + k, 7, 1 + (k * 31) % 9973]);
        }
        q.push([5, 1, 1, 1]);
        q.push([20, 2, 2, 2]);
        let enc = encode_quad_index(q.clone(), PermSet::ALL, writer_codec()).unwrap();
        check_section(&enc.bytes, PermSet::ALL, writer_codec()).unwrap();
        let named = QuadNamed::resident(enc.bytes, writer_codec(), PermSet::ALL);
        let view = named.core.view(10, u64::MAX).unwrap();
        let pos = crate::IndexPermutation::Pos.section_index();
        assert!(view.tile_sections()[pos].len() > 1, "the graph spans tiles");
        let hits = view.match_pattern((None, Some(7), None));
        assert_eq!(hits.len(), 60_000);
        assert_eq!(view.match_pattern((Some(1), None, None)).len(), 1);
        let small = named.core.view(20, u64::MAX).unwrap();
        assert_eq!(small.match_pattern((None, None, None)), vec![(2, 2, 2)]);
    }

    #[test]
    fn tiler_output_is_independent_of_how_rows_arrive() {
        // The external builder feeds the tiler from a merge, the in-RAM one from
        // a slice: identical rows must give identical tiles.
        let mut rows: Vec<QuadRow> = quads(800, 30)
            .into_iter()
            .map(|q| (q[0], q[1], q[2], q[3]))
            .collect();
        rows.sort_unstable();
        rows.dedup();
        let run = |budget: usize| {
            let mut tiles = Vec::new();
            let mut t = QuadTiler::with_budget(budget);
            for &r in &rows {
                t.push(r, &mut |x| tiles.push(x));
            }
            t.finish(&mut |x| tiles.push(x));
            tiles
        };
        let a = run(512);
        assert!(a.len() > 10);
        assert_eq!(a.concat(), rows, "tiles partition the rows in order");
        assert_eq!(a, run(512));
    }

    #[test]
    fn incompressible_tiles_are_stored_raw() {
        // One tiny graph: the zstd frame would be larger than the tile.
        let enc = encode_quad_index(vec![[1, 2, 3, 4]], PermSet::CORE, writer_codec()).unwrap();
        let named = QuadNamed::resident(enc.bytes, writer_codec(), PermSet::CORE);
        let dir = named.core.dir(0).unwrap();
        assert!(dir[0].raw, "a 10-byte tile is not worth a zstd frame");
        let view = named.core.view(1, u64::MAX).unwrap();
        assert_eq!(view.match_pattern((None, None, None)), vec![(2, 3, 4)]);
    }

    #[test]
    fn corrupt_sections_fail_cleanly() {
        let enc = encode_quad_index(quads(200, 5), PermSet::ALL, writer_codec()).unwrap();
        let codec = writer_codec();
        for cut in [0usize, 4, 9, enc.bytes.len() / 2, enc.bytes.len() - 1] {
            let named = QuadNamed::resident(enc.bytes[..cut].to_vec(), codec, PermSet::ALL);
            let _ = named.count();
            let _ = named
                .core
                .view(2, u64::MAX)
                .map(|v| v.match_pattern((None, None, None)));
        }
        // Flip bytes everywhere: never a panic, and any failure is reported.
        for i in (0..enc.bytes.len()).step_by(97) {
            let mut b = enc.bytes.clone();
            b[i] ^= 0x5a;
            let named = QuadNamed::resident(b, codec, PermSet::ALL);
            for k in 0..named.count().min(300) {
                if let Some((g, _)) = named.core.graph_at(k, false) {
                    if let Some(v) = named.core.view(g, u64::MAX) {
                        let _ = v.match_pattern((None, None, None));
                    }
                }
            }
        }
        // A mask that disagrees with the header is refused.
        let named = QuadNamed::resident(enc.bytes.clone(), codec, PermSet::CORE);
        assert_eq!(named.count(), 0);
        assert!(named.load_incomplete());
    }

    #[test]
    fn seal_round_trips_and_refuses_garbage() {
        let d = digest(b"abc");
        assert_eq!(parse_seal(&encode_seal(d)).unwrap(), d);
        assert!(parse_seal(b"RQIX").is_err());
        let mut s = encode_seal(d);
        s[4] = 2;
        assert!(parse_seal(&s).is_err());
    }

    #[test]
    fn the_subject_column_is_the_one_coded_across_runs() {
        use IndexPermutation::*;
        for (perm, cols) in [
            (Spo, (false, false)),
            (Sop, (false, false)),
            (Pos, (false, true)),
            (Ops, (false, true)),
            (Osp, (true, false)),
            (Pso, (true, false)),
        ] {
            assert_eq!(subject_columns(perm), cols, "{}", perm.name());
        }
        // A segment whose subjects repeat across groups round-trips, and the
        // cross-run deltas make it smaller than coding each run from 0.
        let rows: Vec<QuadRow> = (0..40u32)
            .map(|k| (7, 100 + k, 5_000_000 + k * 977, 9_000_000 + k % 3))
            .collect();
        let (mut with, mut without) = (Vec::new(), Vec::new());
        encode_segment_body(&rows, (false, true), &mut with);
        encode_segment_body(&rows, (false, false), &mut without);
        assert!(with.len() < without.len());
        let mut back = Vec::new();
        decode_segment_body(&with, rows[0].1, (false, true), &mut back).unwrap();
        let want: Vec<Triple> = rows.iter().map(|r| (r.1, r.2, r.3)).collect();
        assert_eq!(back, want);
    }

    #[test]
    fn five_permutations_are_refused() {
        let five = PermSet::from_bits(0b0001_1111).unwrap();
        assert!(encode_quad_index(vec![[1, 1, 1, 1]], five, CODEC_NONE).is_err());
    }
}
