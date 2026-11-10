//! The `0x07` quad orders, derived from the GSPO tiles.
//!
//! Every writer (the in-RAM build, the memory-bounded build, `convert`,
//! `merge`) hands the assembler a quad index whose only order of interest is
//! `G+SPO`; the other orders of a `0x05`/`0x06` input (`G+POS`, `G+OSP`, ...)
//! are not written to `0x07`. [`derive`] builds the three graph-last orders
//! SPOG, POSG and OSPG from the GSPO tiles: stream every row, permute, sort,
//! cut with [`crate::quad7::Tiler`], encode, and lay the payload out past the
//! end of the input as extra address space ([`Layered`]), which the assembler
//! copies like any other range. One implementation, so the in-RAM and the
//! external build, and a convert, write the same bytes.
//!
//! One order at a time. Up to [`crate::write_opts::quad_run_rows`] rows are
//! sorted in RAM (16 bytes a row); past that, sorted runs of that size are
//! spilled (zstd, delta-coded, `extbuild_spill`) and merged straight into the
//! tiler, so the peak is one run plus a tile whatever the file.

#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;

use super::parts::{Parts, QuadPart};
use super::QTileEnt;
use crate::file::FileError;
use crate::index::IndexPermutation;
use crate::quad7::{self, Row4};
use crate::reader::RangeReader;

/// Tiles encoded per parallel batch.
const ENCODE_BATCH: usize = 256;
/// GSPO tiles read and decoded per batch.
const READ_BATCH: usize = 256;

/// Does `parts` have a quad index that still lacks the graph-last orders?
pub(crate) fn needed(parts: &Parts) -> bool {
    parts
        .quad
        .as_ref()
        .is_some_and(|q| !q.orders.iter().any(|(si, _)| quad7::is_last(*si)))
}

enum Src {
    Bytes(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    File(PathBuf),
}

struct Piece {
    start: u64,
    len: u64,
    src: Src,
}

/// The derived orders' payload, held in memory or in spill files, placed after
/// the end of the input. Dropping it deletes the files.
pub(crate) struct Extras {
    pieces: Vec<Piece>,
    base: u64,
}

impl Extras {
    /// Extra address space starting at `base` (the input's end).
    pub(super) fn new(base: u64) -> Self {
        Extras {
            pieces: Vec::new(),
            base,
        }
    }

    /// Append a finished sink of `len` bytes at the current end.
    pub(super) fn push(&mut self, len: u64, sink: Sink) -> u64 {
        let start = self.pieces.last().map_or(self.base, |p| p.start + p.len);
        let src = match sink {
            Sink::Mem(v) => Src::Bytes(v),
            #[cfg(not(target_arch = "wasm32"))]
            Sink::File(_, path) => Src::File(path),
        };
        self.pieces.push(Piece { start, len, src });
        start
    }
}

impl Drop for Extras {
    fn drop(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        for p in &self.pieces {
            if let Src::File(path) = &p.src {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// `input`, then the [`Extras`] past its end.
pub(crate) struct Layered<'a> {
    input: &'a dyn RangeReader,
    extras: &'a Extras,
}

impl<'a> Layered<'a> {
    pub(crate) fn new(input: &'a dyn RangeReader, extras: &'a Extras) -> Self {
        Layered { input, extras }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn read_file_at(path: &std::path::Path, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; len as usize];
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        f.read_exact_at(&mut buf, offset)?;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0usize;
        while done < buf.len() {
            let n = f.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (&f, offset);
        return Err(std::io::ErrorKind::Unsupported.into());
    }
    Ok(buf)
}

impl RangeReader for Layered<'_> {
    fn len(&self) -> u64 {
        self.extras
            .pieces
            .last()
            .map_or(self.extras.base, |p| p.start + p.len)
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|&e| e <= self.len())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "past the derived orders")
            })?;
        let base = self.extras.base;
        if end <= base {
            return self.input.read_at(offset, len);
        }
        let mut out = Vec::with_capacity(len as usize);
        let mut at = offset;
        if at < base {
            out.extend_from_slice(&self.input.read_at(at, base - at)?);
            at = base;
        }
        let mut i = self
            .extras
            .pieces
            .partition_point(|p| p.start + p.len <= at);
        while at < end {
            let p = &self.extras.pieces[i];
            let from = at - p.start;
            let n = (p.len - from).min(end - at);
            match &p.src {
                Src::Bytes(b) => out.extend_from_slice(&b[from as usize..(from + n) as usize]),
                #[cfg(not(target_arch = "wasm32"))]
                Src::File(path) => out.extend_from_slice(&read_file_at(path, from, n)?),
            }
            at += n;
            i += 1;
        }
        Ok(out)
    }

    fn concurrency(&self) -> usize {
        self.input.concurrency()
    }
}

/// Where an order's payload goes while it is written.
pub(super) enum Sink {
    Mem(Vec<u8>),
    #[cfg(not(target_arch = "wasm32"))]
    File(std::io::BufWriter<std::fs::File>, PathBuf),
}

impl Sink {
    /// A sink in memory, or (not in a browser) a file `name` in the scratch directory.
    pub(super) fn new(
        in_memory: bool,
        dir: &std::path::Path,
        name: &str,
    ) -> Result<Sink, FileError> {
        #[cfg(not(target_arch = "wasm32"))]
        if !in_memory {
            std::fs::create_dir_all(dir)?;
            let path = dir.join(name);
            let f = std::fs::File::create(&path)?;
            return Ok(Sink::File(
                std::io::BufWriter::with_capacity(1 << 20, f),
                path,
            ));
        }
        let _ = (in_memory, dir, name);
        Ok(Sink::Mem(Vec::new()))
    }

    pub(super) fn flush(&mut self) -> Result<(), FileError> {
        #[cfg(not(target_arch = "wasm32"))]
        if let Sink::File(w, _) = self {
            std::io::Write::flush(w)?;
        }
        Ok(())
    }

    pub(super) fn write(&mut self, b: &[u8]) -> Result<(), FileError> {
        match self {
            Sink::Mem(v) => v.extend_from_slice(b),
            #[cfg(not(target_arch = "wasm32"))]
            Sink::File(w, _) => std::io::Write::write_all(w, b)?,
        }
        Ok(())
    }
}

/// One order being written: tiler, pending tiles, directory entries, payload.
struct OrderWriter {
    scol: usize,
    codec: u8,
    tiler: Option<quad7::Tiler>,
    pending: Vec<Vec<Row4>>,
    ents: Vec<QTileEnt>,
    sink: Sink,
    at: u64,
}

impl OrderWriter {
    fn new(perm: IndexPermutation, codec: u8, sink: Sink) -> Self {
        OrderWriter {
            scol: quad7::subject_col(perm),
            codec,
            tiler: Some(quad7::Tiler::new(perm)),
            pending: Vec::new(),
            ents: Vec::new(),
            sink,
            at: 0,
        }
    }

    fn drain(&mut self) -> Result<(), FileError> {
        for t in quad7::encode_batch(&self.pending, self.scol, self.codec) {
            self.ents.push(QTileEnt {
                first: t.first,
                last: t.last,
                clen: t.bytes.len() as u64,
                raw: t.raw,
                syn: t.syn,
                off: self.at,
            });
            self.at += t.bytes.len() as u64;
            self.sink.write(&t.bytes)?;
        }
        self.pending.clear();
        Ok(())
    }

    fn push(&mut self, r: Row4) -> Result<(), FileError> {
        if let Some(t) = self.tiler.as_mut() {
            let pending = &mut self.pending;
            t.push(r, &mut |x| pending.push(x));
        }
        if self.pending.len() >= ENCODE_BATCH {
            self.drain()?;
        }
        Ok(())
    }

    /// Finish: the rows seen, the entries (offsets local to the payload) and
    /// the payload.
    fn finish(mut self) -> Result<(u64, Vec<QTileEnt>, Sink), FileError> {
        let n = match self.tiler.take() {
            Some(t) => {
                let pending = &mut self.pending;
                t.finish(&mut |x| pending.push(x))
            }
            None => 0,
        };
        self.drain()?;
        #[cfg(not(target_arch = "wasm32"))]
        if let Sink::File(w, _) = &mut self.sink {
            std::io::Write::flush(w)?;
        }
        Ok((n, self.ents, self.sink))
    }
}

/// Call `f(g, s, p, o)` for every quad of the `G+SPO` tiles, in order.
fn for_each_gspo(
    codec: u8,
    input: &dyn RangeReader,
    tiles: &[QTileEnt],
    f: &mut dyn FnMut(u32, u32, u32, u32) -> Result<(), FileError>,
) -> Result<(), FileError> {
    let bad = || FileError::Container("a G+SPO quad tile does not decode");
    for chunk in tiles.chunks(READ_BATCH) {
        let ranges: Vec<(u64, u64)> = chunk.iter().map(|t| (t.off, t.clen)).collect();
        let blobs = input.read_many(&ranges)?;
        let decode =
            |(t, stored): (&QTileEnt, &Vec<u8>)| -> Result<Vec<crate::quad::QuadRow>, FileError> {
                let body = if t.raw {
                    stored.clone()
                } else {
                    crate::file::decompress(codec, stored)?
                };
                crate::quad::tile_rows(&body, t.first.0, IndexPermutation::Spo).ok_or_else(bad)
            };
        #[cfg(feature = "parallel")]
        let decoded: Vec<Result<Vec<crate::quad::QuadRow>, FileError>> = {
            use rayon::prelude::*;
            chunk.par_iter().zip(blobs.par_iter()).map(decode).collect()
        };
        #[cfg(not(feature = "parallel"))]
        let decoded: Vec<Result<Vec<crate::quad::QuadRow>, FileError>> =
            chunk.iter().zip(blobs.iter()).map(decode).collect();
        for rows in decoded {
            for (g, s, p, o) in rows? {
                f(g, s, p, o)?;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "parallel")]
fn sort_rows(v: &mut [Row4]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
}
#[cfg(not(feature = "parallel"))]
fn sort_rows(v: &mut [Row4]) {
    v.sort_unstable();
}

/// A per-process sequence number: two spills of one process never share a name,
/// whatever the clock says.
#[cfg(not(target_arch = "wasm32"))]
pub(super) fn spill_seq() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Record the size of this order's sort runs in the build record's
/// `scratch_peak_bytes` (a lower bound, sampled once the runs are written, as the
/// external build samples its own). Only the files named `tag…` are counted: the
/// directory may be the output's own. No directory read unless a trace is recording.
#[cfg(not(target_arch = "wasm32"))]
fn note_scratch(dir: &std::path::Path, tag: &str) {
    if !crate::build_trace::active() {
        return;
    }
    let bytes: u64 = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_string_lossy().starts_with(tag))
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0);
    crate::build_trace::max("scratch_peak_bytes", bytes);
}

/// Build one graph-last order from the GSPO tiles. Returns its entries (offsets
/// local to its payload) and the payload.
fn build_order(
    perm: IndexPermutation,
    codec: u8,
    input: &dyn RangeReader,
    gspo: &[QTileEnt],
    total: u64,
    spill_dir: &std::path::Path,
) -> Result<(Vec<QTileEnt>, Sink), FileError> {
    #[cfg(target_arch = "wasm32")]
    let _ = spill_dir;
    let run_rows = crate::write_opts::quad_run_rows();
    let mismatch = || FileError::Container("the G+SPO tiles hold another quad count than the root");
    // A browser build has no disk to spill to and sorts in RAM whatever the size.
    if total <= run_rows as u64 || cfg!(target_arch = "wasm32") {
        let mut rows: Vec<Row4> = Vec::with_capacity(total as usize);
        for_each_gspo(codec, input, gspo, &mut |g, s, p, o| {
            rows.push(quad7::row_of(perm, g, s, p, o));
            Ok(())
        })?;
        sort_rows(&mut rows);
        let mut w = OrderWriter::new(perm, codec, Sink::Mem(Vec::new()));
        for &r in &rows {
            w.push(r)?;
        }
        drop(rows);
        let (n, ents, sink) = w.finish()?;
        if n != total {
            return Err(mismatch());
        }
        return Ok((ents, sink));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use crate::extbuild_spill::{merge_runs, RunSpiller};
        let dir = spill_dir.to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let tag = format!(
            "q7-{}-{:x}-{}-{}",
            quad7::slot_name(quad7::last_slot(perm)).replace('+', ""),
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
            spill_seq()
        );
        let mut sp = RunSpiller::<Row4>::new_exact(&dir, &tag, run_rows);
        for_each_gspo(codec, input, gspo, &mut |g, s, p, o| {
            sp.push(quad7::row_of(perm, g, s, p, o))?;
            Ok(())
        })?;
        let runs = sp.finish()?;
        note_scratch(&dir, &tag);
        let path = dir.join(format!("{tag}.tiles"));
        let file = std::fs::File::create(&path)?;
        let mut w = OrderWriter::new(
            perm,
            codec,
            Sink::File(std::io::BufWriter::with_capacity(1 << 20, file), path),
        );
        merge_runs(runs, |r: Row4| w.push(r))?;
        let (n, ents, sink) = w.finish()?;
        if n != total {
            return Err(mismatch());
        }
        Ok((ents, sink))
    }
    #[cfg(target_arch = "wasm32")]
    {
        unreachable!("a wasm build sorts in RAM");
    }
}

/// The graph directory cut into blocks of [`crate::quad::DIR_BLOCK_GRAPHS_07`]
/// graphs: the new block index (`P0`) and the new blocks, read from the input's
/// own (the writers before `0x07` cut them at 1,024). Streams a batch of blocks
/// at a time.
fn reblock(
    q: &QuadPart,
    input: &dyn RangeReader,
    codec: u8,
) -> Result<(Vec<u8>, Vec<u8>), FileError> {
    use super::Cur;
    let bad = || FileError::Container("a quad graph directory does not decode");
    let mut c = Cur::new(&q.p0);
    let n = c.count(u32::MAX as u64)?;
    // Per block: its first graph, its stored length and whether it is raw.
    let mut refs: Vec<(u32, u64, bool)> = Vec::with_capacity(n.min(1 << 20));
    let mut first = 0u32;
    for _ in 0..n {
        first = first.checked_add(c.u32v()?).ok_or_else(bad)?;
        let packed = c.uv()?;
        refs.push((first, packed >> 1, packed & 1 == 1));
    }
    let old = q.block_graphs.max(1);
    let mut w =
        crate::quad::GraphDirWriter::with_block_graphs(codec, crate::quad::DIR_BLOCK_GRAPHS_07);
    let mut sink: Vec<u8> = Vec::new();
    let mut at = q.gdir.offset;
    let mut done = 0u64;
    for chunk in refs.chunks(256) {
        let mut ranges = Vec::with_capacity(chunk.len());
        for &(_, clen, _) in chunk {
            ranges.push((at, clen));
            at += clen;
        }
        for (blob, &(first, _, raw)) in input.read_many(&ranges)?.iter().zip(chunk) {
            let body = if raw {
                blob.clone()
            } else {
                crate::file::decompress(codec, blob)?
            };
            let in_block = q.graph_count.saturating_sub(done).min(old);
            let (mut pos, mut g) = (0usize, first);
            for _ in 0..in_block {
                let (d, k) = crate::varint::read_uvarint(body.get(pos..).ok_or_else(bad)?)
                    .ok_or_else(bad)?;
                pos += k;
                let (cnt, k) = crate::varint::read_uvarint(body.get(pos..).ok_or_else(bad)?)
                    .ok_or_else(bad)?;
                pos += k;
                g = g
                    .checked_add(u32::try_from(d).map_err(|_| bad())?)
                    .ok_or_else(bad)?;
                w.push(g, cnt, &mut sink)?;
            }
            done += in_block;
        }
    }
    let (p0, blocks_len, graphs) = w.finish(&mut sink)?;
    debug_assert_eq!(blocks_len as usize, sink.len());
    if graphs != q.graph_count {
        return Err(FileError::Container(
            "the quad graph directory holds another graph count than the root",
        ));
    }
    Ok((p0, sink))
}

/// Build the graph-last orders of `parts`' quad index and return the parts
/// with the orders `[G+SPO, SPOG, POSG, OSPG]`, and the payload the new orders'
/// tile offsets point into (past the end of `input`).
pub(crate) fn derive(
    parts: &Parts,
    input: &dyn RangeReader,
    spill_dir: &std::path::Path,
) -> Result<(Parts, Extras), FileError> {
    let q = parts
        .quad
        .as_ref()
        .ok_or(FileError::Container("no quad index"))?;
    let gspo = q
        .orders
        .iter()
        .find(|(si, _)| *si == 0)
        .map(|(_, t)| t.as_slice())
        .ok_or(FileError::Container("a quad index without its G+SPO order"))?;
    let mut extras = Extras {
        pieces: Vec::new(),
        base: input.len(),
    };
    let mut orders: Vec<(usize, Vec<QTileEnt>)> = vec![(0, gspo.to_vec())];
    let mut next = input.len();
    for perm in [
        IndexPermutation::Spo,
        IndexPermutation::Pos,
        IndexPermutation::Osp,
    ] {
        let (mut ents, sink) = build_order(
            perm,
            parts.block_codec,
            input,
            gspo,
            q.quad_count,
            spill_dir,
        )?;
        let len = ents.last().map_or(0, |e| e.off + e.clen);
        for e in &mut ents {
            e.off += next;
        }
        orders.push((quad7::last_slot(perm), ents));
        let src = match sink {
            Sink::Mem(v) => Src::Bytes(v),
            #[cfg(not(target_arch = "wasm32"))]
            Sink::File(_, path) => Src::File(path),
        };
        extras.pieces.push(Piece {
            start: next,
            len,
            src,
        });
        next += len;
    }
    let mut quad = QuadPart {
        orders,
        ..q.clone()
    };
    // Fewer, bigger graph-directory blocks: the block index is in the root.
    if q.block_graphs != crate::quad::DIR_BLOCK_GRAPHS_07 {
        let (p0, blocks) = reblock(q, input, parts.block_codec)?;
        let len = blocks.len() as u64;
        quad.p0 = p0;
        quad.block_graphs = crate::quad::DIR_BLOCK_GRAPHS_07;
        quad.gdir = crate::file::ByteRange { offset: next, len };
        extras.pieces.push(Piece {
            start: next,
            len,
            src: Src::Bytes(blocks),
        });
    }
    let mut p = parts.clone();
    p.quad = Some(quad);
    Ok((p, extras))
}

/// A 64-bit mix of a quad `(g, s, p, o)`; the per-order sums of it must agree.
fn quad_hash(g: u32, s: u32, p: u32, o: u32) -> u64 {
    let mut x = ((g as u64) << 32 | s as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 29;
    x = (x ^ ((p as u64) << 32 | o as u64)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    x.wrapping_mul(0x94D0_49BB_1331_11EB)
}

/// What a deep check of one order found.
struct OrderCheck {
    rows: u64,
    sum: u64,
}

/// Structurally check the quad orders of a `0x06`/`0x07` file, tile by tile:
/// every tile decodes, its rows ascend (within the tile and across tiles), its
/// first and last keys and its synopsis are the directory's, each order holds
/// the file's quad count, and all orders hold the same quads (an order-
/// independent checksum of `(g, s, p, o)`). Returns what is wrong, empty when
/// the orders are sound, and how many orders it checked (0: no quad index).
/// Reads every quad tile once; memory is a batch.
pub fn check_orders(input: &dyn RangeReader) -> Result<(usize, Vec<String>), FileError> {
    let parts = super::parts::from_v06(input)?;
    let Some(q) = &parts.quad else {
        return Ok((0, Vec::new()));
    };
    let mut bad = Vec::new();
    let mut results: Vec<(usize, OrderCheck)> = Vec::new();
    for (si, tiles) in &q.orders {
        let name = quad7::slot_name(*si);
        let last = quad7::is_last(*si);
        let perm = quad7::perm_of(*si);
        let scol = quad7::subject_col(perm);
        let mut rows_total = 0u64;
        let mut sum = 0u64;
        let mut prev: Option<(u32, u32, u32, u32)> = None;
        let mut ti = 0usize;
        // A clustered SPOG ascends within each class run only.
        let mut run_starts: std::collections::BTreeSet<usize> = Default::default();
        if let Some(c) = parts
            .cluster
            .iter()
            .find(|c| c.si == *si && *si == super::QUAD_SPOG)
        {
            let mut at = 0usize;
            for &n in &c.run_tiles {
                run_starts.insert(at);
                at += n;
            }
        }
        for chunk in tiles.chunks(READ_BATCH) {
            let ranges: Vec<(u64, u64)> = chunk.iter().map(|t| (t.off, t.clen)).collect();
            let blobs = input.read_many(&ranges)?;
            for (t, stored) in chunk.iter().zip(&blobs) {
                let here = ti;
                ti += 1;
                if run_starts.contains(&here) {
                    prev = None;
                }
                let body = if t.raw {
                    Ok(stored.clone())
                } else {
                    crate::file::decompress(parts.block_codec, stored)
                };
                let Ok(body) = body else {
                    bad.push(format!("{name} tile {here}: does not decompress"));
                    continue;
                };
                // Rows as (key1, key2, c, g) for graph-last, (g, a, b, c) for GSPO:
                // sorted by their stored order either way.
                let rows: Option<Vec<(u32, u32, u32, u32)>> = if last {
                    let mut v = Vec::new();
                    quad7::decode_tile(&body, scol, &mut v).map(|_| v)
                } else {
                    crate::quad::tile_rows(&body, t.first.0, perm)
                };
                let Some(rows) = rows.filter(|r| !r.is_empty()) else {
                    bad.push(format!("{name} tile {here}: does not decode"));
                    continue;
                };
                // The leading key is the first two columns of either layout.
                let (k0, k1) = (
                    (rows[0].0, rows[0].1),
                    (rows[rows.len() - 1].0, rows[rows.len() - 1].1),
                );
                if k0 != t.first || k1 != t.last {
                    bad.push(format!(
                        "{name} tile {here}: key range differs from the directory"
                    ));
                }
                if rows.windows(2).any(|w| w[0] >= w[1]) || prev.is_some_and(|p| p >= rows[0]) {
                    bad.push(format!(
                        "{name} tile {here}: rows are not strictly ascending"
                    ));
                }
                prev = rows.last().copied();
                // The synopsis of the two trailing columns: `(b, c)` of GSPO,
                // `(c, g)` of a graph-last order.
                let (mut lo, mut hi) = ((u32::MAX, u32::MAX), (0u32, 0u32));
                for r in &rows {
                    lo.0 = lo.0.min(r.2);
                    hi.0 = hi.0.max(r.2);
                    lo.1 = lo.1.min(r.3);
                    hi.1 = hi.1.max(r.3);
                }
                if t.syn != (lo.0, hi.0, lo.1, hi.1) {
                    bad.push(format!(
                        "{name} tile {here}: synopsis differs from the rows"
                    ));
                }
                for r in &rows {
                    let (g, s, p, o) = if last {
                        let (g, (s, p, o)) = quad7::quad_of(perm, r);
                        (g, s, p, o)
                    } else {
                        let (s, p, o) = perm.back((r.1, r.2, r.3));
                        (r.0, s, p, o)
                    };
                    sum = sum.wrapping_add(quad_hash(g, s, p, o));
                }
                rows_total += rows.len() as u64;
            }
        }
        if rows_total != q.quad_count {
            bad.push(format!(
                "{name}: {rows_total} rows, the file has {} named quads",
                q.quad_count
            ));
        }
        results.push((
            *si,
            OrderCheck {
                rows: rows_total,
                sum,
            },
        ));
    }
    if let Some((_, first)) = results.first() {
        for (si, r) in &results {
            if r.sum != first.sum && r.rows == first.rows {
                bad.push(format!(
                    "{} holds other quads than {}",
                    quad7::slot_name(*si),
                    quad7::slot_name(results[0].0)
                ));
            }
        }
    }
    Ok((q.orders.len(), bad))
}
