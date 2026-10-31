//! The memory-bounded build's **id remap tables**, spilled to disk.
//!
//! The dictionary merge ([`crate::extbuild`], phase 2) learns, for every term
//! of every chunk, the global id that replaces its chunk-local one; phase 3
//! rewrites each chunk's statements with those ids, one chunk after the other.
//! Up to 0.4.0 every chunk's table stayed resident from the start of the merge
//! to the end of phase 3: 4 bytes per chunk-local id, so the merge's memory
//! grew with the dataset instead of the budget (15.0 GB at 1.73 B chunk-local
//! terms on an OpenCitations Meta half, the limit of a 16 GB build).
//!
//! Here each chunk's table is written to disk as the merge produces it, in up
//! to five append-only files, one per *segment* of the chunk's local ids:
//!
//! | segment ([`Seg`]) | local ids | file holds |
//! |---|---|---|
//! | `Shared` | `1..=S`, the same in the subject and the object space | the final global id |
//! | `SubjOnly` | `S+1..=S+A` in the subject space | a provisional id |
//! | `ObjOnly` | `S+1..=S+B` in the object space | a provisional id |
//! | `Pred` | `1..=P` | the final global id |
//! | `Graph` | `1..=G` (per-graph layout) | the global graph ordinal |
//!
//! **Appends, in order.** A chunk's term file lists a segment's terms in
//! ascending order, and a local id is a term's position in its file
//! (`SpaceStream`); the k-way merge emits terms in ascending order. So within
//! one segment of one chunk the merge reaches the local ids in increasing
//! order, and each global id is appended to that segment's file.
//! [`RemapWriter::push`] checks it — a skipped or repeated local id is an
//! error, never a misnumbered file — and [`RemapWriter::finish`] checks that
//! every local id got exactly one global id. A term shared within its chunk
//! is written once, although both of its spaces carry it.
//!
//! **Provisional ids.** A chunk's subject-only term is, globally, shared or
//! subject-only, and a subject-only id is `n_shared + rank + 1`: `n_shared` is
//! only known once the merge ends. Its segment file stores a shared term's
//! final id (top bit clear) or [`ROLE_ONLY`]` | rank`, and [`RemapStore::load`]
//! fixes it up; the object-only segment likewise. That allows 2³¹ terms per
//! dictionary class, refused cleanly past it (the old in-RAM encoding put the
//! class in the top two bits and silently wrapped at 2³⁰).
//!
//! **On disk** each flush is one compressed block ([`crate::extbuild_spill`])
//! of class-aware deltas: within a segment the final ids and the provisional
//! ranks are two increasing sequences, so each value is written as the delta
//! from the previous one of its own class, with the class in the low bit —
//! about 8% of the 4 bytes per id measured on OpenCitations Meta.
//!
//! **Memory.** The merge holds one write buffer per segment being filled, per
//! chunk, sized so that they all fit in a sixteenth of the budget
//! ([`remap_buf_bytes`]); phase 3 holds one chunk's tables, 4 B per local id —
//! at most a quarter of the chunk budget, since every buffered statement is
//! charged at least 96 B and brings at most six ids. Neither grows with the
//! number of chunks.

#![cfg(not(target_arch = "wasm32"))]

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::build_trace as trace;
use crate::extbuild::ExtBuildError;
use crate::extbuild_spill::{put_block, spill_level, take_block};
use crate::varint::write_uvarint;

/// One chunk's local-id segments, in the order of [`SEG_FILE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seg {
    Shared = 0,
    SubjOnly = 1,
    ObjOnly = 2,
    Pred = 3,
    Graph = 4,
}

const SEGS: usize = 5;
const SEG_FILE: [&str; SEGS] = ["map.shared", "map.subj", "map.obj", "map.pred", "map.graph"];

/// Marks a provisional role-only id (`ROLE_ONLY | rank`) in the subject-only
/// and object-only segments; a shared term's final id there has it clear.
pub(crate) const ROLE_ONLY: u32 = 1 << 31;

/// Local-id counts of one chunk's segments, as [`Seg`] indexes them.
pub(crate) type SegCounts = [u32; SEGS];

/// Per-segment write buffer for a merge over `chunks` chunks within
/// `budget` bytes: the three segments the node merge fills at once share a
/// sixteenth of the budget, between 4 KiB and 256 KiB each.
pub(crate) fn remap_buf_bytes(budget: u64, chunks: usize) -> usize {
    let per = budget / 16 / (3 * chunks.max(1) as u64);
    (per.clamp(4 << 10, 256 << 10) as usize) & !3
}

/// The final id of the shared term of merge rank `rank` (0-based).
pub(crate) fn shared_id(rank: u32) -> Result<u32, ExtBuildError> {
    if rank >= ROLE_ONLY - 1 {
        return Err(ExtBuildError::Internal(
            "more than 2^31 shared terms: the dictionary is too large for one file",
        ));
    }
    Ok(rank + 1)
}

/// The provisional id of the subject- or object-only term of merge rank
/// `rank` (0-based within its class).
pub(crate) fn role_only_id(rank: u32) -> Result<u32, ExtBuildError> {
    if rank >= ROLE_ONLY {
        return Err(ExtBuildError::Internal(
            "more than 2^31 subject-only or object-only terms: the dictionary is \
             too large for one file",
        ));
    }
    Ok(ROLE_ONLY | rank)
}

fn seg_path(dir: &Path, ci: usize, seg: usize) -> PathBuf {
    dir.join(format!("c{ci}.{}", SEG_FILE[seg]))
}

/// Resident bytes of one chunk's loaded tables (the shared segment twice, in
/// both spaces) — what the pre-spill merge held for it from start to end.
fn table_bytes(c: &SegCounts) -> u64 {
    4 * (2 * c[0] as u64 + c[1] as u64 + c[2] as u64 + c[3] as u64 + c[4] as u64)
}

#[derive(Default)]
struct SegState {
    expected: u32,
    written: u32,
    /// buffered ids, little-endian
    buf: Vec<u8>,
    /// the last id written of each class (final, provisional)
    prev: [u32; 2],
}

/// Class-aware delta of one id: `(delta << 1) | class`, where class 1 is a
/// provisional [`ROLE_ONLY`] rank and the delta is from the previous id of
/// the same class.
fn put_id(out: &mut Vec<u8>, prev: &mut [u32; 2], v: u32) {
    let class = (v >> 31) as usize;
    let x = v & !ROLE_ONLY;
    let d = x.wrapping_sub(prev[class]) & !ROLE_ONLY;
    prev[class] = x;
    write_uvarint(out, ((d as u64) << 1) | class as u64);
}

/// The inverse of [`put_id`].
fn get_id(raw: &[u8], pos: &mut usize, prev: &mut [u32; 2]) -> Result<u32, ExtBuildError> {
    let (w, n) = crate::varint::read_uvarint(&raw[*pos..])
        .ok_or(ExtBuildError::Internal("a remap spill block is truncated"))?;
    *pos += n;
    let class = (w & 1) as usize;
    let x = prev[class].wrapping_add((w >> 1) as u32) & !ROLE_ONLY;
    prev[class] = x;
    Ok(if class == 1 { x | ROLE_ONLY } else { x })
}

/// The merge side: appends every chunk's global ids to its segment files.
pub(crate) struct RemapWriter {
    dir: PathBuf,
    chunks: Vec<[SegState; SEGS]>,
    buf_bytes: usize,
}

impl RemapWriter {
    /// A writer for chunks whose segments have `counts` local ids each, with
    /// a `buf_bytes` buffer per segment being filled.
    pub(crate) fn new(dir: &Path, counts: &[SegCounts], buf_bytes: usize) -> Self {
        RemapWriter {
            dir: dir.to_path_buf(),
            chunks: counts
                .iter()
                .map(|c| {
                    std::array::from_fn(|s| SegState {
                        expected: c[s],
                        ..Default::default()
                    })
                })
                .collect(),
            buf_bytes: buf_bytes.max(4),
        }
    }

    /// Record `id` for local id `local` (1-based, counted within the
    /// segment) of chunk `ci`'s segment `seg`. Local ids must arrive in
    /// increasing order with none skipped.
    pub(crate) fn push(
        &mut self,
        ci: usize,
        seg: Seg,
        local: u32,
        id: u32,
    ) -> Result<(), ExtBuildError> {
        let st = self
            .chunks
            .get_mut(ci)
            .map(|c| &mut c[seg as usize])
            .ok_or(ExtBuildError::Internal("remap for an unknown chunk"))?;
        if local != st.written + 1 || local > st.expected {
            return Err(ExtBuildError::Internal(
                "dictionary merge reached a chunk's local ids out of order",
            ));
        }
        st.written = local;
        if st.buf.capacity() == 0 {
            st.buf
                .reserve_exact(self.buf_bytes.min(4 * st.expected as usize));
        }
        st.buf.extend_from_slice(&id.to_le_bytes());
        if st.buf.len() >= self.buf_bytes {
            Self::append(&self.dir, ci, seg as usize, st)?;
        }
        Ok(())
    }

    /// Append the buffered ids to the segment file as one block.
    fn append(dir: &Path, ci: usize, seg: usize, st: &mut SegState) -> Result<(), ExtBuildError> {
        if st.buf.is_empty() {
            return Ok(());
        }
        let mut raw = Vec::with_capacity(st.buf.len() / 2);
        for w in st.buf.chunks_exact(4) {
            put_id(
                &mut raw,
                &mut st.prev,
                u32::from_le_bytes([w[0], w[1], w[2], w[3]]),
            );
        }
        let mut block = Vec::with_capacity(raw.len() / 4 + 16);
        put_block(&mut block, &raw, (st.buf.len() / 4) as u32, spill_level())?;
        // Opened per flush rather than held: a merge over K chunks would
        // otherwise keep up to 3K more descriptors open beside its 4K readers.
        let mut f = OpenOptions::new()
            .create(true)
            .append(true)
            .open(seg_path(dir, ci, seg))?;
        f.write_all(&block)?;
        st.buf.clear();
        Ok(())
    }

    /// Write out what every chunk has buffered for `seg` and free the
    /// buffers: the merge of that segment is over.
    pub(crate) fn flush(&mut self, seg: Seg) -> Result<(), ExtBuildError> {
        for (ci, c) in self.chunks.iter_mut().enumerate() {
            let st = &mut c[seg as usize];
            Self::append(&self.dir, ci, seg as usize, st)?;
            st.buf = Vec::new();
        }
        Ok(())
    }

    /// Flush every segment, check that every local id got its global id, and
    /// hand the tables to phase 3. `n_shared` fixes up the provisional ids.
    pub(crate) fn finish(mut self, n_shared: u32) -> Result<RemapStore, ExtBuildError> {
        for seg in [
            Seg::Shared,
            Seg::SubjOnly,
            Seg::ObjOnly,
            Seg::Pred,
            Seg::Graph,
        ] {
            self.flush(seg)?;
        }
        let mut counts = Vec::with_capacity(self.chunks.len());
        for c in &self.chunks {
            if c.iter().any(|s| s.written != s.expected) {
                return Err(ExtBuildError::Internal(
                    "dictionary merge left a chunk-local id without a global id",
                ));
            }
            counts.push(std::array::from_fn(|s| c[s].expected));
        }
        Ok(RemapStore {
            dir: self.dir,
            counts,
            n_shared,
        })
    }
}

/// One chunk's tables, loaded: `local id - 1 → global id`, per space.
pub(crate) struct ChunkRemap {
    pub(crate) subj: Vec<u32>,
    pub(crate) obj: Vec<u32>,
    pub(crate) pred: Vec<u32>,
    /// chunk-local graph rank → global graph ordinal (per-graph layout)
    pub(crate) graph: Vec<u32>,
}

/// The phase-3 side: the spilled tables of every chunk.
pub(crate) struct RemapStore {
    dir: PathBuf,
    counts: Vec<SegCounts>,
    n_shared: u32,
}

impl RemapStore {
    /// Bytes every chunk's tables take once loaded, summed: what the merge
    /// held resident before the tables were spilled.
    pub(crate) fn total_table_bytes(&self) -> u64 {
        self.counts.iter().map(table_bytes).sum()
    }

    /// The largest one chunk's tables take: what phase 3 holds at most.
    pub(crate) fn largest_table_bytes(&self) -> u64 {
        self.counts.iter().map(table_bytes).max().unwrap_or(0)
    }

    /// Load chunk `ci`'s tables, fixing up its provisional ids, and delete
    /// its files. Records `remap_peak_bytes` (the largest chunk loaded).
    pub(crate) fn load(&self, ci: usize) -> Result<ChunkRemap, ExtBuildError> {
        let c = self
            .counts
            .get(ci)
            .ok_or(ExtBuildError::Internal("remap for an unknown chunk"))?;
        let n_shared = self.n_shared;
        let fix = |v: u32| -> Result<u32, ExtBuildError> {
            if v & ROLE_ONLY == 0 {
                return Ok(v);
            }
            n_shared
                .checked_add(v & !ROLE_ONLY)
                .and_then(|x| x.checked_add(1))
                .ok_or(ExtBuildError::Internal(
                    "global term ids overflow 32 bits: the dictionary is too large for one file",
                ))
        };
        let (s, a, b) = (c[0] as usize, c[1] as usize, c[2] as usize);
        let mut subj = Vec::with_capacity(s + a);
        self.read(ci, Seg::Shared, &mut subj, &Ok)?;
        let mut obj = Vec::with_capacity(s + b);
        obj.extend_from_slice(&subj);
        self.read(ci, Seg::SubjOnly, &mut subj, &fix)?;
        self.read(ci, Seg::ObjOnly, &mut obj, &fix)?;
        let mut pred = Vec::with_capacity(c[3] as usize);
        self.read(ci, Seg::Pred, &mut pred, &Ok)?;
        let mut graph = Vec::with_capacity(c[4] as usize);
        self.read(ci, Seg::Graph, &mut graph, &Ok)?;
        for seg in 0..SEGS {
            let _ = std::fs::remove_file(seg_path(&self.dir, ci, seg));
        }
        trace::max("remap_peak_bytes", table_bytes(c));
        Ok(ChunkRemap {
            subj,
            obj,
            pred,
            graph,
        })
    }

    /// Append segment `seg` of chunk `ci` to `out`, through `f`.
    fn read(
        &self,
        ci: usize,
        seg: Seg,
        out: &mut Vec<u32>,
        f: &dyn Fn(u32) -> Result<u32, ExtBuildError>,
    ) -> Result<(), ExtBuildError> {
        let expected = self.counts[ci][seg as usize] as u64;
        if expected == 0 {
            return Ok(());
        }
        let path = seg_path(&self.dir, ci, seg as usize);
        let mut file = File::open(&path)?;
        let (mut payload, mut raw) = (Vec::new(), Vec::new());
        let mut prev = [0u32; 2];
        let mut got = 0u64;
        while let Some(count) = take_block(&mut file, &mut payload, &mut raw)? {
            let mut pos = 0;
            for _ in 0..count {
                out.push(f(get_id(&raw, &mut pos, &mut prev)?)?);
            }
            if pos != raw.len() {
                return Err(ExtBuildError::Internal(
                    "a remap spill block has trailing bytes",
                ));
            }
            got += count as u64;
        }
        if got != expected {
            return Err(ExtBuildError::Internal(
                "a remap spill file has the wrong length",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("rete-remap-{tag}-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Ids written through several buffer flushes come back in local-id
    /// order, with the provisional ones fixed up and the shared segment at
    /// the head of both spaces; the files are gone afterwards.
    #[test]
    fn round_trip_fixes_up_provisional_ids() {
        let dir = tmpdir("rt");
        // chunk 0: 3 shared, 2 subject-only, 1 object-only, 2 predicates
        // chunk 1: nothing but 1 graph
        let counts = [[3, 2, 1, 2, 0], [0, 0, 0, 0, 1]];
        let mut w = RemapWriter::new(&dir, &counts, 4); // a flush per id
        w.push(0, Seg::Shared, 1, shared_id(0).unwrap()).unwrap();
        w.push(0, Seg::SubjOnly, 1, role_only_id(0).unwrap())
            .unwrap();
        w.push(0, Seg::Shared, 2, shared_id(4).unwrap()).unwrap();
        w.push(0, Seg::ObjOnly, 1, shared_id(5).unwrap()).unwrap();
        w.push(0, Seg::SubjOnly, 2, role_only_id(7).unwrap())
            .unwrap();
        w.push(0, Seg::Shared, 3, shared_id(6).unwrap()).unwrap();
        w.flush(Seg::Shared).unwrap();
        w.push(0, Seg::Pred, 1, 1).unwrap();
        w.push(0, Seg::Pred, 2, 9).unwrap();
        w.push(1, Seg::Graph, 1, 4).unwrap();
        let store = w.finish(10).unwrap();
        assert_eq!(store.total_table_bytes(), 4 * (6 + 2 + 1 + 2 + 1));

        let m = store.load(0).unwrap();
        assert_eq!(m.subj, [1, 5, 7, 11, 18]);
        assert_eq!(m.obj, [1, 5, 7, 6]);
        assert_eq!(m.pred, [1, 9]);
        assert!(m.graph.is_empty());
        assert_eq!(store.load(1).unwrap().graph, [4]);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The order the format depends on is checked, not trusted.
    #[test]
    fn out_of_order_and_missing_ids_are_errors() {
        let dir = tmpdir("order");
        let mut w = RemapWriter::new(&dir, &[[2, 0, 0, 0, 0]], 64);
        assert!(w.push(0, Seg::Shared, 2, 1).is_err(), "skipped local id 1");
        w.push(0, Seg::Shared, 1, 1).unwrap();
        assert!(w.push(0, Seg::Shared, 1, 2).is_err(), "repeated local id");
        assert!(w.push(0, Seg::Pred, 1, 1).is_err(), "beyond the segment");
        assert!(w.finish(0).is_err(), "local id 2 never got a global id");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn class_limits_are_refused_not_wrapped() {
        assert!(shared_id((1 << 31) - 2).is_ok());
        assert!(shared_id((1 << 31) - 1).is_err());
        assert!(role_only_id((1 << 31) - 1).is_ok());
        assert!(role_only_id(1 << 31).is_err());
        let dir = tmpdir("ovf");
        let mut w = RemapWriter::new(&dir, &[[0, 1, 0, 0, 0]], 64);
        w.push(0, Seg::SubjOnly, 1, role_only_id(10).unwrap())
            .unwrap();
        let store = w.finish(u32::MAX - 5).unwrap();
        assert!(store.load(0).is_err(), "id past u32::MAX");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn buffers_share_a_sixteenth_of_the_budget() {
        assert_eq!(remap_buf_bytes(12_000 << 20, 200), 256 << 10);
        assert_eq!(remap_buf_bytes(64 << 20, 1_000), 4 << 10);
        let b = remap_buf_bytes(1_000 << 20, 80);
        assert!(b * 3 * 80 <= (1_000 << 20) / 16 && b.is_multiple_of(4));
    }
}
