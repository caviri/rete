//! The memory-bounded build's **compressed spill** (G23).
//!
//! Every intermediate file of [`crate::extbuild`] that is written once and read
//! back in order goes through one of the codecs here. None of it reaches the
//! output: the `.rete` bytes do not depend on this module, only the disk the
//! build needs on the way.
//!
//! | codec | records | used for |
//! |---|---|---|
//! | [`RunSpiller`] / [`RunReader`] | `u32` tuples, delta-varint | chunk statements, sort runs |
//! | [`TermWriter`] / [`TermReader`] | sorted N-Triples tokens, front-coded | chunk term files, graph names |
//! | [`ZWriter`] / [`ZReader`] | bytes | text-index words, label and schema sort runs |
//!
//! **Blocks.** A spill file is a sequence of independent blocks, each
//! `[u32 payload len][u32 raw len][u32 count][u8 codec][payload]` (little
//! endian), the payload zstd-compressed at a fast level ([`spill_level`],
//! `RETE_SPILL_ZSTD_LEVEL`, default 1; `0` stores blocks uncompressed, which
//! is also what a build without the `compression` feature writes). A block is
//! stored raw whenever zstd would not shrink it. Blocks are what make the
//! codec cheap to parallelize — a sort run is encoded on every core and
//! written in order — and what bound a reader's memory: one block, whatever
//! the file.
//!
//! **Id records** ([`IdRecord`]: triples and quads of `u32` ids) are written
//! column by column against the previous record of the block: the leading
//! columns that equal the previous record's cost a zero byte each, the first
//! column that differs is a delta, and the columns after it are plain
//! varints. In a sorted run the deltas are small and the zeros long, and zstd
//! takes most of what is left. Deltas wrap, so an unsorted stream still
//! round-trips — it only compresses worse.
//!
//! **Runs free their disk as they are read.** A sort run ([`RunFile`]) is a
//! chain of segment files of at most [`SEGMENT_BYTES`]; its [`RunReader`]
//! deletes each segment once it has read it. A k-way merge consumes every run
//! at about the same pace, so the space a merge frees keeps up with the space
//! the next permutation's runs take: the build holds about **one** copy of its
//! id statements at any time, plus one segment per run.

#![cfg(not(target_arch = "wasm32"))]

use std::fs::File;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};

use crate::varint::write_uvarint;

/// `[u32 payload len][u32 raw len][u32 count][u8 codec]`
const FRAME_HEAD: usize = 13;
/// A block stored as is.
const CODEC_RAW: u8 = 0;
/// A block compressed as one zstd frame.
const CODEC_ZSTD: u8 = 1;
/// The default [`spill_level`]: zstd's fastest regular level. Measured on the
/// OpenCitations Meta sample against levels 3 and −1: level 3 saves a few
/// percent more for twice the time, −1 loses a quarter of the ratio.
const DEFAULT_LEVEL: i32 = 1;

/// Records per id block: 32 Ki records, 384 KiB as triples once decoded.
pub(crate) const ID_BLOCK_RECORDS: usize = 1 << 15;
/// Raw bytes per byte or term block.
pub(crate) const BYTE_BLOCK: usize = 64 << 10;
/// A run's segment file is cut at the first block boundary past this, so a
/// merge over K runs keeps at most K segments' worth of consumed records on
/// disk.
pub(crate) const SEGMENT_BYTES: u64 = 32 << 20;
/// Blocks a run encodes per parallel batch: bounds the encoded bytes held
/// before they are written.
const PAR_BLOCKS: usize = 64;

/// The spill's zstd level: `RETE_SPILL_ZSTD_LEVEL`, or [`DEFAULT_LEVEL`].
/// `0` stores every block uncompressed (delta-varint only); a negative level
/// is one of zstd's faster modes. A build without the `compression` feature
/// always stores.
pub(crate) fn spill_level() -> i32 {
    static LEVEL: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *LEVEL.get_or_init(|| {
        std::env::var("RETE_SPILL_ZSTD_LEVEL")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(DEFAULT_LEVEL)
    })
}

// ---------------------------------------------------------------------------
// Blocks
// ---------------------------------------------------------------------------

#[cfg(feature = "compression")]
thread_local! {
    /// One compressor per thread and level, reused across blocks.
    static CCTX: std::cell::RefCell<Option<(i32, zstd::bulk::Compressor<'static>, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
    /// One decompressor per thread: every reader of a merge shares it, so a
    /// merge over thousands of term files holds one zstd context, not one each.
    static DCTX: std::cell::RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { std::cell::RefCell::new(None) };
}

/// zstd-compress `raw` and append the payload to `out`; `false` (nothing
/// appended) when it would not be smaller than `raw`.
#[cfg(feature = "compression")]
fn zstd_append(raw: &[u8], out: &mut Vec<u8>, level: i32) -> io::Result<bool> {
    CCTX.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !matches!(&*slot, Some((l, _, _)) if *l == level) {
            *slot = Some((level, zstd::bulk::Compressor::new(level)?, Vec::new()));
        }
        let (_, cctx, scratch) = slot.as_mut().expect("just set");
        scratch.clear();
        scratch.reserve(zstd::zstd_safe::compress_bound(raw.len()));
        let n = cctx.compress_to_buffer(raw, scratch)?;
        if n >= raw.len() {
            return Ok(false);
        }
        out.extend_from_slice(&scratch[..n]);
        Ok(true)
    })
}

/// Append one block holding `raw` (`count` records) to `out`.
pub(crate) fn put_block(out: &mut Vec<u8>, raw: &[u8], count: u32, level: i32) -> io::Result<()> {
    let at = out.len();
    out.extend_from_slice(&[0u8; FRAME_HEAD]);
    #[cfg(feature = "compression")]
    let codec = if level != 0 && !raw.is_empty() && zstd_append(raw, out, level)? {
        CODEC_ZSTD
    } else {
        CODEC_RAW
    };
    #[cfg(not(feature = "compression"))]
    let codec = {
        let _ = level;
        CODEC_RAW
    };
    if codec == CODEC_RAW {
        out.extend_from_slice(raw);
    }
    let payload = u32::try_from(out.len() - at - FRAME_HEAD)
        .map_err(|_| io::Error::other("spill block over 4 GiB"))?;
    let raw_len =
        u32::try_from(raw.len()).map_err(|_| io::Error::other("spill block over 4 GiB"))?;
    out[at..at + 4].copy_from_slice(&payload.to_le_bytes());
    out[at + 4..at + 8].copy_from_slice(&raw_len.to_le_bytes());
    out[at + 8..at + 12].copy_from_slice(&count.to_le_bytes());
    out[at + 12] = codec;
    Ok(())
}

/// `read_exact`, except that a clean end of stream before the first byte is
/// `Ok(false)`.
fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..]) {
            Ok(0) if got == 0 => return Ok(false),
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(true)
}

/// Read the next block of `r` into `raw` (decoded), using `payload` as
/// scratch. Returns the block's record count, `None` at the end of the file.
pub(crate) fn take_block(
    r: &mut impl Read,
    payload: &mut Vec<u8>,
    raw: &mut Vec<u8>,
) -> io::Result<Option<u32>> {
    let mut head = [0u8; FRAME_HEAD];
    if !read_exact_or_eof(r, &mut head)? {
        return Ok(None);
    }
    let le = |i: usize| u32::from_le_bytes([head[i], head[i + 1], head[i + 2], head[i + 3]]);
    let (plen, rlen, count, codec) = (le(0) as usize, le(4) as usize, le(8), head[12]);
    payload.clear();
    payload.resize(plen, 0);
    r.read_exact(payload)?;
    raw.clear();
    match codec {
        CODEC_RAW => {
            if plen != rlen {
                return Err(io::Error::other("spill block: stored length mismatch"));
            }
            std::mem::swap(payload, raw);
        }
        #[cfg(feature = "compression")]
        CODEC_ZSTD => {
            raw.reserve(rlen);
            let n = DCTX.with(|cell| -> io::Result<usize> {
                let mut slot = cell.borrow_mut();
                if slot.is_none() {
                    *slot = Some(zstd::bulk::Decompressor::new()?);
                }
                slot.as_mut()
                    .expect("just set")
                    .decompress_to_buffer(&payload[..], raw)
            })?;
            if n != rlen {
                return Err(io::Error::other("spill block: decoded length mismatch"));
            }
        }
        #[cfg(not(feature = "compression"))]
        CODEC_ZSTD => {
            return Err(io::Error::other(
                "spill block: zstd in a build without the compression feature",
            ))
        }
        _ => return Err(io::Error::other("spill block: unknown codec")),
    }
    Ok(Some(count))
}

fn read_varint(buf: &[u8], pos: &mut usize) -> io::Result<u64> {
    let (v, n) = crate::varint::read_uvarint(&buf[*pos..])
        .ok_or_else(|| io::Error::other("spill block: truncated varint"))?;
    *pos += n;
    Ok(v)
}

// ---------------------------------------------------------------------------
// Id records
// ---------------------------------------------------------------------------

/// A fixed-width record of `u32` ids that the id codec spills.
pub(crate) trait IdRecord: Copy + Ord + Send + Sync {
    /// Columns per record.
    const WIDTH: usize;
    /// Column `i`.
    fn col(&self, i: usize) -> u32;
    /// The record of the first [`Self::WIDTH`] columns of `c`.
    fn from_cols(c: [u32; 4]) -> Self;
}

impl IdRecord for (u32, u32, u32) {
    const WIDTH: usize = 3;
    fn col(&self, i: usize) -> u32 {
        [self.0, self.1, self.2][i]
    }
    fn from_cols(c: [u32; 4]) -> Self {
        (c[0], c[1], c[2])
    }
}

impl IdRecord for (u32, u32, u32, u32) {
    const WIDTH: usize = 4;
    fn col(&self, i: usize) -> u32 {
        [self.0, self.1, self.2, self.3][i]
    }
    fn from_cols(c: [u32; 4]) -> Self {
        (c[0], c[1], c[2], c[3])
    }
}

/// Delta-varint `recs` into `out` (one block's raw bytes).
fn encode_ids<R: IdRecord>(recs: &[R], out: &mut Vec<u8>) {
    let mut prev = [0u32; 4];
    for r in recs {
        let mut i = 0;
        // Equal leading columns cost a zero each; the first that differs is
        // a (wrapping) delta, and the columns after it are written plain.
        loop {
            let v = r.col(i);
            let d = v.wrapping_sub(prev[i]);
            write_uvarint(out, d as u64);
            prev[i] = v;
            i += 1;
            if d != 0 || i == R::WIDTH {
                break;
            }
        }
        for (j, p) in prev.iter_mut().enumerate().take(R::WIDTH).skip(i) {
            let v = r.col(j);
            write_uvarint(out, v as u64);
            *p = v;
        }
    }
}

/// Decode `count` records of one block's raw bytes, appending them to `out`.
fn decode_ids<R: IdRecord>(raw: &[u8], count: u32, out: &mut Vec<R>) -> io::Result<()> {
    let mut prev = [0u32; 4];
    let mut pos = 0;
    let narrow = |v: u64| u32::try_from(v).map_err(|_| io::Error::other("spill id over 32 bits"));
    for _ in 0..count {
        let mut i = 0;
        loop {
            let d = narrow(read_varint(raw, &mut pos)?)?;
            prev[i] = prev[i].wrapping_add(d);
            i += 1;
            if d != 0 || i == R::WIDTH {
                break;
            }
        }
        for p in prev.iter_mut().take(R::WIDTH).skip(i) {
            *p = narrow(read_varint(raw, &mut pos)?)?;
        }
        out.push(R::from_cols(prev));
    }
    if pos != raw.len() {
        return Err(io::Error::other("spill block: trailing bytes"));
    }
    Ok(())
}

/// One whole block (frame included) of `recs`.
fn id_block<R: IdRecord>(
    recs: &[R],
    level: i32,
    raw: &mut Vec<u8>,
    out: &mut Vec<u8>,
) -> io::Result<()> {
    raw.clear();
    encode_ids(recs, raw);
    put_block(out, raw, recs.len() as u32, level)
}

/// Encode `recs` as consecutive blocks, on every core when the `parallel`
/// feature is on, and hand each batch's bytes to `sink` in order.
fn encode_id_blocks<R: IdRecord>(
    recs: &[R],
    level: i32,
    sink: &mut dyn FnMut(&[u8]) -> io::Result<()>,
) -> io::Result<()> {
    for batch in recs.chunks(ID_BLOCK_RECORDS * PAR_BLOCKS) {
        #[cfg(feature = "parallel")]
        let blocks: Vec<io::Result<Vec<u8>>> = {
            use rayon::prelude::*;
            batch
                .par_chunks(ID_BLOCK_RECORDS)
                .map(|b| {
                    let (mut raw, mut out) = (Vec::new(), Vec::new());
                    id_block(b, level, &mut raw, &mut out).map(|()| out)
                })
                .collect()
        };
        #[cfg(not(feature = "parallel"))]
        let blocks: Vec<io::Result<Vec<u8>>> = batch
            .chunks(ID_BLOCK_RECORDS)
            .map(|b| {
                let (mut raw, mut out) = (Vec::new(), Vec::new());
                id_block(b, level, &mut raw, &mut out).map(|()| out)
            })
            .collect();
        for b in blocks {
            sink(&b?)?;
        }
    }
    Ok(())
}

/// Reads one run segment back, a block at a time.
pub(crate) struct IdReader<R: IdRecord> {
    file: File,
    payload: Vec<u8>,
    raw: Vec<u8>,
    recs: Vec<R>,
    pos: usize,
}

impl<R: IdRecord> IdReader<R> {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(IdReader {
            file: File::open(path)?,
            payload: Vec::new(),
            raw: Vec::new(),
            recs: Vec::new(),
            pos: 0,
        })
    }

    #[inline]
    pub(crate) fn next(&mut self) -> io::Result<Option<R>> {
        if self.pos == self.recs.len() && !self.refill()? {
            return Ok(None);
        }
        let r = self.recs[self.pos];
        self.pos += 1;
        Ok(Some(r))
    }

    fn refill(&mut self) -> io::Result<bool> {
        self.recs.clear();
        self.pos = 0;
        loop {
            let Some(count) = take_block(&mut self.file, &mut self.payload, &mut self.raw)? else {
                return Ok(false);
            };
            decode_ids(&self.raw, count, &mut self.recs)?;
            if !self.recs.is_empty() {
                return Ok(true);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Sort runs
// ---------------------------------------------------------------------------

/// One sorted, deduplicated run of id records, as a chain of segment files
/// `<base>.0`, `<base>.1`, … (see the module docs).
#[derive(Debug, Clone)]
pub(crate) struct RunFile {
    base: PathBuf,
    segments: u32,
}

impl RunFile {
    fn segment(&self, j: u32) -> PathBuf {
        segment_path(&self.base, j)
    }

    /// Delete whatever is left of the run.
    pub(crate) fn remove(&self) {
        for j in 0..self.segments {
            let _ = std::fs::remove_file(self.segment(j));
        }
    }
}

fn segment_path(base: &Path, j: u32) -> PathBuf {
    let mut name = base.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{j}"));
    base.with_file_name(name)
}

/// Write `recs` as the run `base`. Any order round-trips; sorted records are
/// what the codec is for.
pub(crate) fn write_run<R: IdRecord>(base: &Path, recs: &[R]) -> io::Result<RunFile> {
    let mut run = RunFile {
        base: base.to_path_buf(),
        segments: 0,
    };
    let mut seg: Option<(File, u64)> = None;
    encode_id_blocks(recs, spill_level(), &mut |block| {
        if seg.as_ref().is_none_or(|(_, n)| *n >= SEGMENT_BYTES) {
            if let Some((mut f, _)) = seg.take() {
                f.flush()?;
            }
            seg = Some((File::create(run.segment(run.segments))?, 0));
            run.segments += 1;
        }
        let (f, n) = seg.as_mut().expect("just opened");
        f.write_all(block)?;
        *n += block.len() as u64;
        Ok(())
    })?;
    if let Some((mut f, _)) = seg {
        f.flush()?;
    }
    Ok(run)
}

#[cfg(feature = "parallel")]
fn sort_dedup<R: IdRecord>(v: &mut Vec<R>) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
    v.dedup();
}
#[cfg(not(feature = "parallel"))]
fn sort_dedup<R: IdRecord>(v: &mut Vec<R>) {
    v.sort_unstable();
    v.dedup();
}

/// Sort `recs`, drop duplicates, and write them as the run `base`.
pub(crate) fn spill_sorted_run<R: IdRecord>(base: &Path, recs: &mut Vec<R>) -> io::Result<RunFile> {
    sort_dedup(recs);
    write_run(base, recs)
}

/// Collects id records into sorted, deduplicated runs of at most `run_len`
/// records — the external sort's first half.
pub(crate) struct RunSpiller<R: IdRecord> {
    dir: PathBuf,
    name: String,
    run_len: usize,
    buf: Vec<R>,
    runs: Vec<RunFile>,
}

impl<R: IdRecord> RunSpiller<R> {
    /// Runs live in `dir` as `<name>.run<i>.<segment>`.
    pub(crate) fn new(dir: &Path, name: &str, run_len: usize) -> Self {
        let run_len = run_len.max(1);
        RunSpiller {
            dir: dir.to_path_buf(),
            name: name.to_string(),
            run_len,
            buf: Vec::with_capacity(run_len.min(1 << 22)),
            runs: Vec::new(),
        }
    }

    #[inline]
    pub(crate) fn push(&mut self, r: R) -> io::Result<()> {
        self.buf.push(r);
        if self.buf.len() >= self.run_len {
            self.spill()?;
        }
        Ok(())
    }

    fn spill(&mut self) -> io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        let base = self
            .dir
            .join(format!("{}.run{}", self.name, self.runs.len()));
        let run = spill_sorted_run(&base, &mut self.buf)?;
        self.runs.push(run);
        self.buf.clear();
        Ok(())
    }

    /// Spill what is buffered and return every run.
    pub(crate) fn finish(mut self) -> io::Result<Vec<RunFile>> {
        self.spill()?;
        Ok(std::mem::take(&mut self.runs))
    }
}

impl<R: IdRecord> Drop for RunSpiller<R> {
    fn drop(&mut self) {
        for r in &self.runs {
            r.remove();
        }
    }
}

/// Reads one [`RunFile`] in order, deleting each segment once it is read.
pub(crate) struct RunReader<R: IdRecord> {
    run: RunFile,
    next_seg: u32,
    cur: Option<IdReader<R>>,
}

impl<R: IdRecord> RunReader<R> {
    pub(crate) fn open(run: RunFile) -> Self {
        RunReader {
            run,
            next_seg: 0,
            cur: None,
        }
    }

    #[inline]
    pub(crate) fn next(&mut self) -> io::Result<Option<R>> {
        loop {
            if let Some(r) = self.cur.as_mut() {
                if let Some(x) = r.next()? {
                    return Ok(Some(x));
                }
                // this segment is read: give its space back now
                self.cur = None;
                let _ = std::fs::remove_file(self.run.segment(self.next_seg - 1));
            }
            if self.next_seg == self.run.segments {
                return Ok(None);
            }
            self.cur = Some(IdReader::open(&self.run.segment(self.next_seg))?);
            self.next_seg += 1;
        }
    }
}

impl<R: IdRecord> Drop for RunReader<R> {
    fn drop(&mut self) {
        // An abandoned reader (an error upstream) leaves nothing behind.
        self.cur = None;
        for j in self.next_seg.saturating_sub(1)..self.run.segments {
            let _ = std::fs::remove_file(self.run.segment(j));
        }
    }
}

/// k-way merge of sorted runs, handing each distinct record to `f` in order
/// and deleting the runs as they are read. Returns the distinct records.
pub(crate) fn merge_runs<R, E>(
    runs: Vec<RunFile>,
    mut f: impl FnMut(R) -> Result<(), E>,
) -> Result<u64, E>
where
    R: IdRecord,
    E: From<io::Error>,
{
    use std::cmp::Reverse;
    crate::build_trace::note_merge(runs.len());
    let mut readers: Vec<RunReader<R>> = runs.into_iter().map(RunReader::open).collect();
    let mut heap = std::collections::BinaryHeap::with_capacity(readers.len());
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some(x) = r.next()? {
            heap.push(Reverse((x, i)));
        }
    }
    let mut last: Option<R> = None;
    let mut n = 0u64;
    while let Some(Reverse((x, i))) = heap.pop() {
        if let Some(y) = readers[i].next()? {
            heap.push(Reverse((y, i)));
        }
        if last != Some(x) {
            f(x)?;
            last = Some(x);
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Bytes
// ---------------------------------------------------------------------------

/// A spill file written as compressed blocks of [`BYTE_BLOCK`] raw bytes.
/// `flush` closes the current block early; dropping it flushes too, ignoring
/// errors, like a `BufWriter`.
pub(crate) struct ZWriter {
    file: Option<File>,
    /// the block being filled
    raw: Vec<u8>,
    /// full blocks waiting to be compressed, [`Z_BATCH`] at a time
    pending: Vec<Vec<u8>>,
    level: i32,
    block: usize,
    bytes: u64,
}

/// Blocks a [`ZWriter`] compresses together, on every core when the
/// `parallel` feature is on: the writers sit on the build's one parsing
/// thread, which should not spend its time in zstd.
const Z_BATCH: usize = 16;

impl ZWriter {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        Ok(ZWriter {
            file: Some(File::create(path)?),
            raw: Vec::with_capacity(BYTE_BLOCK),
            pending: Vec::new(),
            level: spill_level(),
            block: BYTE_BLOCK,
            bytes: 0,
        })
    }

    /// Queue the block being filled; compress and write the queue when it is
    /// full, or when `drain`.
    fn emit(&mut self, drain: bool) -> io::Result<()> {
        if !self.raw.is_empty() {
            let full = std::mem::replace(&mut self.raw, Vec::with_capacity(self.block));
            self.pending.push(full);
        }
        if self.pending.is_empty() || (!drain && self.pending.len() < Z_BATCH) {
            return Ok(());
        }
        let level = self.level;
        let frame = |raw: &Vec<u8>| -> io::Result<Vec<u8>> {
            let mut out = Vec::with_capacity(raw.len() / 2 + FRAME_HEAD);
            put_block(&mut out, raw, raw.len() as u32, level).map(|()| out)
        };
        #[cfg(feature = "parallel")]
        let frames: Vec<io::Result<Vec<u8>>> = {
            use rayon::prelude::*;
            self.pending.par_iter().map(frame).collect()
        };
        #[cfg(not(feature = "parallel"))]
        let frames: Vec<io::Result<Vec<u8>>> = self.pending.iter().map(frame).collect();
        self.pending.clear();
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| io::Error::other("spill writer finished"))?;
        for f in frames {
            let f = f?;
            file.write_all(&f)?;
            self.bytes += f.len() as u64;
        }
        Ok(())
    }

    /// Write the last blocks and close the file; returns its length.
    pub(crate) fn finish(mut self) -> io::Result<u64> {
        self.emit(true)?;
        if let Some(mut f) = self.file.take() {
            f.flush()?;
        }
        Ok(self.bytes)
    }
}

impl Write for ZWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Never more than one block's worth at a time: a large write is cut
        // at block boundaries rather than becoming one oversized block.
        let n = buf
            .len()
            .min(self.block - self.raw.len().min(self.block - 1));
        self.raw.extend_from_slice(&buf[..n]);
        if self.raw.len() >= self.block {
            self.emit(false)?;
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.emit(true)?;
        match self.file.as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for ZWriter {
    fn drop(&mut self) {
        if self.file.is_some() {
            let _ = self.emit(true);
        }
    }
}

/// Reads a [`ZWriter`]'s file back as a plain byte stream (`Read` +
/// `BufRead`), one decoded block in memory. Opened `consuming`, it deletes
/// the file once it has read it to its end.
pub(crate) struct ZReader {
    file: Option<File>,
    consume: Option<PathBuf>,
    payload: Vec<u8>,
    block: Vec<u8>,
    pos: usize,
}

impl ZReader {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(ZReader {
            file: Some(File::open(path)?),
            consume: None,
            payload: Vec::new(),
            block: Vec::new(),
            pos: 0,
        })
    }

    /// Like [`open`](Self::open), deleting the file at its end.
    pub(crate) fn open_consuming(path: &Path) -> io::Result<Self> {
        let mut r = Self::open(path)?;
        r.consume = Some(path.to_path_buf());
        Ok(r)
    }
}

impl BufRead for ZReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        while self.pos == self.block.len() {
            let Some(file) = self.file.as_mut() else {
                return Ok(&[]);
            };
            self.pos = 0;
            if take_block(file, &mut self.payload, &mut self.block)?.is_none() {
                self.file = None; // close before deleting
                self.block.clear();
                if let Some(p) = self.consume.take() {
                    let _ = std::fs::remove_file(p);
                }
                return Ok(&[]);
            }
        }
        Ok(&self.block[self.pos..])
    }

    fn consume(&mut self, amt: usize) {
        self.pos = (self.pos + amt).min(self.block.len());
    }
}

impl Read for ZReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let avail = self.fill_buf()?;
        let n = avail.len().min(buf.len());
        buf[..n].copy_from_slice(&avail[..n]);
        self.consume(n);
        Ok(n)
    }
}

// ---------------------------------------------------------------------------
// Terms
// ---------------------------------------------------------------------------

/// Writes N-Triples tokens in ascending order, front-coded against the
/// previous one: `[shared prefix len][suffix len][suffix]`, through a
/// [`ZWriter`].
pub(crate) struct TermWriter {
    z: ZWriter,
    prev: Vec<u8>,
    rec: Vec<u8>,
}

impl TermWriter {
    pub(crate) fn create(path: &Path) -> io::Result<Self> {
        Ok(TermWriter {
            z: ZWriter::create(path)?,
            prev: Vec::new(),
            rec: Vec::new(),
        })
    }

    pub(crate) fn push(&mut self, term: &str) -> io::Result<()> {
        let t = term.as_bytes();
        let shared = self.prev.iter().zip(t).take_while(|(a, b)| a == b).count();
        self.rec.clear();
        write_uvarint(&mut self.rec, shared as u64);
        write_uvarint(&mut self.rec, (t.len() - shared) as u64);
        self.rec.extend_from_slice(&t[shared..]);
        self.z.write_all(&self.rec)?;
        self.prev.truncate(shared);
        self.prev.extend_from_slice(&t[shared..]);
        Ok(())
    }

    pub(crate) fn finish(self) -> io::Result<u64> {
        self.z.finish()
    }
}

/// Reads a [`TermWriter`]'s file back, term by term.
pub(crate) struct TermReader {
    z: ZReader,
    cur: Vec<u8>,
}

impl TermReader {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        Ok(TermReader {
            z: ZReader::open(path)?,
            cur: Vec::new(),
        })
    }

    /// Like [`open`](Self::open), deleting the file once it is read.
    pub(crate) fn open_consuming(path: &Path) -> io::Result<Self> {
        Ok(TermReader {
            z: ZReader::open_consuming(path)?,
            cur: Vec::new(),
        })
    }

    pub(crate) fn next(&mut self) -> io::Result<Option<String>> {
        let Some(shared) = read_stream_varint(&mut self.z)? else {
            return Ok(None);
        };
        let len = read_stream_varint(&mut self.z)?
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let shared = shared as usize;
        if shared > self.cur.len() {
            return Err(io::Error::other("term spill: bad shared prefix"));
        }
        self.cur.truncate(shared);
        let at = self.cur.len();
        self.cur.resize(at + len as usize, 0);
        self.z.read_exact(&mut self.cur[at..])?;
        String::from_utf8(self.cur.clone())
            .map(Some)
            .map_err(|_| io::Error::other("term spill: bad utf-8"))
    }
}

/// A varint from a byte stream: `None` at a clean end of stream.
pub(crate) fn read_stream_varint(r: &mut impl BufRead) -> io::Result<Option<u64>> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let buf = r.fill_buf()?;
        let Some(&b) = buf.first() else {
            return if shift == 0 {
                Ok(None)
            } else {
                Err(io::ErrorKind::UnexpectedEof.into())
            };
        };
        r.consume(1);
        if shift > 63 {
            return Err(io::Error::other("varint overflow"));
        }
        v |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            return Ok(Some(v));
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("rete-spill-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn lcg(seed: &mut u64) -> u32 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (*seed >> 33) as u32
    }

    #[test]
    fn id_blocks_round_trip_sorted_unsorted_and_extreme() {
        let d = tmpdir("ids");
        let mut seed = 7u64;
        let mut recs: Vec<(u32, u32, u32)> = (0..100_000)
            .map(|_| (lcg(&mut seed) % 1000, lcg(&mut seed) % 7, lcg(&mut seed)))
            .collect();
        recs.extend([
            (0, 0, 0),
            (u32::MAX, u32::MAX, u32::MAX),
            (u32::MAX, 0, 1),
            (5, 5, 5),
            (5, 5, 5),
        ]);
        for sorted in [false, true] {
            let mut v = recs.clone();
            if sorted {
                v.sort_unstable();
            }
            let run = write_run(&d.join(format!("t{sorted}")), &v).unwrap();
            let mut r = RunReader::<(u32, u32, u32)>::open(run);
            let mut back = Vec::new();
            while let Some(x) = r.next().unwrap() {
                back.push(x);
            }
            assert_eq!(back, v);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sorted_runs_shrink_and_free_their_segments_as_they_are_read() {
        let d = tmpdir("runs");
        let mut seed = 11u64;
        let quads: Vec<(u32, u32, u32, u32)> = (0..300_000)
            .map(|_| {
                (
                    lcg(&mut seed) % 50,
                    lcg(&mut seed) % 100_000,
                    lcg(&mut seed) % 40,
                    lcg(&mut seed) % 200_000,
                )
            })
            .collect();
        let mut sp = RunSpiller::new(&d, "q", 70_000);
        for &q in &quads {
            sp.push(q).unwrap();
        }
        // and once more: duplicates across runs merge away
        for &q in quads.iter().take(1000) {
            sp.push(q).unwrap();
        }
        let runs = sp.finish().unwrap();
        assert_eq!(runs.len(), 5);
        let on_disk: u64 = std::fs::read_dir(&d)
            .unwrap()
            .map(|e| e.unwrap().metadata().unwrap().len())
            .sum();
        // 16 B raw per record; sorted deltas must do far better
        assert!(on_disk < 300_000 * 16 / 2, "{on_disk}");
        let mut want = quads.clone();
        want.sort_unstable();
        want.dedup();
        let mut got = Vec::new();
        let n = merge_runs::<(u32, u32, u32, u32), io::Error>(runs, |q| {
            got.push(q);
            Ok(())
        })
        .unwrap();
        assert_eq!(n as usize, want.len());
        assert_eq!(got, want);
        assert_eq!(
            std::fs::read_dir(&d).unwrap().count(),
            0,
            "every segment deleted"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_long_run_is_cut_into_segments_and_each_goes_once_read() {
        let d = tmpdir("seg");
        // incompressible enough to pass several segments
        let mut seed = 3u64;
        let mut v: Vec<(u32, u32, u32)> = (0..4_000_000)
            .map(|_| (lcg(&mut seed), lcg(&mut seed), lcg(&mut seed)))
            .collect();
        let run = spill_sorted_run(&d.join("r"), &mut v).unwrap();
        assert!(run.segments >= 2, "{} segment(s)", run.segments);
        let mut r = RunReader::<(u32, u32, u32)>::open(run.clone());
        let mut seen = 0usize;
        let mut prev = None;
        while let Some(x) = r.next().unwrap() {
            assert!(prev < Some(x));
            prev = Some(x);
            seen += 1;
            if seen == v.len() - 1 {
                // the last record is in the last segment: the first is gone
                assert!(
                    !run.segment(0).exists(),
                    "the first segment is freed mid-run"
                );
                assert!(run.segment(run.segments - 1).exists());
            }
        }
        assert_eq!(seen, v.len());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn byte_stream_and_terms_round_trip() {
        let d = tmpdir("bytes");
        let p = d.join("z");
        let mut w = ZWriter::create(&p).unwrap();
        let mut want = Vec::new();
        for i in 0..200_000u32 {
            let s = format!("word{}|", i % 977);
            w.write_all(s.as_bytes()).unwrap();
            want.extend_from_slice(s.as_bytes());
            if i % 50_000 == 0 {
                w.flush().unwrap(); // a short block
            }
        }
        w.finish().unwrap();
        let mut r = ZReader::open_consuming(&p).unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, want);
        assert!(
            !p.exists(),
            "a consuming reader deletes its file at the end"
        );

        let terms: Vec<String> = {
            let mut t: Vec<String> = (0..50_000u32)
                .map(|i| format!("<https://w3id.org/oc/meta/br/06{:09}>", i * 37))
                .chain([
                    "\"é\"@fr".to_string(),
                    "\"\"".to_string(),
                    "_:b0".to_string(),
                ])
                .collect();
            t.sort();
            t
        };
        let p = d.join("t");
        let mut w = TermWriter::create(&p).unwrap();
        for t in &terms {
            w.push(t).unwrap();
        }
        let len = w.finish().unwrap();
        let raw: usize = terms.iter().map(|t| t.len() + 1).sum();
        assert!((len as usize) < raw / 4, "{len} of {raw}");
        let mut r = TermReader::open(&p).unwrap();
        for t in &terms {
            assert_eq!(r.next().unwrap().as_deref(), Some(t.as_str()));
        }
        assert_eq!(r.next().unwrap(), None);
        // an empty file is an empty stream
        let e = d.join("e");
        TermWriter::create(&e).unwrap().finish().unwrap();
        assert_eq!(TermReader::open(&e).unwrap().next().unwrap(), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
