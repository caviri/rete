//! An exact distinct-value counter for `COUNT(DISTINCT ?x)` that holds a
//! bounded amount of memory and spills the rest to disk.
//!
//! The aggregate used to keep every distinct value in a `BTreeSet<Val>`, about
//! 40 bytes each before allocator overhead: `COUNT(DISTINCT ?r)` over the 610 M
//! `dcite:PidRelation`s of the DataCite graph ran a 16 GB container out of
//! memory. Here dictionary ids (almost every value) go to a flat `Vec<i64>`,
//! which is sorted and deduplicated whenever it doubles, so it holds at most
//! about twice the distinct values plus one chunk. When the deduplicated set
//! passes the spill threshold ([`Rete::spill_threshold`], a quarter of the
//! memory budget) it is written out as a sorted run (delta-encoded varints in a
//! temporary file) and the buffer starts again. The count is then a k-way merge
//! of the runs, reading one value per run at a time.
//!
//! Computed values (`Val::Str`, from BIND or an expression) are rare and are
//! kept in a set beside the ids; they spill the same way, as sorted runs of
//! length-prefixed strings. An id and a string are never equal (values are
//! canonical), so the two counts add up.
//!
//! The answer is exact at any threshold; the threshold only decides how much
//! of the work happens on disk. When a run cannot be written (no writable
//! temporary directory, or wasm, which has none) the set simply stays in
//! memory, as it always did.
//!
//! [`Rete::spill_threshold`]: crate::Rete::spill_threshold

use std::collections::{BTreeSet, BinaryHeap};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::rc::Rc;

use crate::row::Val;

/// The id buffer is compacted (sorted + deduplicated) once it holds this many
/// entries more than it did after the last compaction, at the least.
const MIN_CHUNK: usize = 1 << 20;

/// Bytes charged per kept string on top of its text (set node, `Rc` header).
const STR_OVERHEAD: u64 = 64;

/// Distinct values seen so far — see the module docs.
pub(super) struct DistinctSet {
    ids: Vec<i64>,
    /// `ids.len()` after the last compaction (the sorted, unique prefix).
    compacted: usize,
    strs: BTreeSet<Rc<str>>,
    str_bytes: u64,
    /// Spill once the deduplicated in-memory set exceeds this many bytes.
    threshold: u64,
    id_runs: Vec<Run>,
    str_runs: Vec<Run>,
}

impl DistinctSet {
    pub(super) fn new(threshold: u64) -> Self {
        DistinctSet {
            ids: Vec::new(),
            compacted: 0,
            strs: BTreeSet::new(),
            str_bytes: 0,
            threshold,
            id_runs: Vec::new(),
            str_runs: Vec::new(),
        }
    }

    pub(super) fn insert(&mut self, v: &Val) {
        match v {
            Val::Id(id) => {
                self.ids.push(*id);
                if self.ids.len() >= self.compacted.saturating_mul(2).max(MIN_CHUNK) {
                    self.compact_ids();
                }
            }
            Val::Str(s) => {
                if self.strs.insert(s.clone()) {
                    self.str_bytes += s.len() as u64 + STR_OVERHEAD;
                    if self.str_bytes > self.threshold / 2 {
                        self.spill_strs();
                    }
                }
            }
        }
    }

    /// Sort and deduplicate the id buffer; spill it when what is left is
    /// still more than half the threshold.
    fn compact_ids(&mut self) {
        self.ids.sort_unstable();
        self.ids.dedup();
        self.compacted = self.ids.len();
        if (self.ids.len() as u64).saturating_mul(8) > self.threshold / 2 {
            if let Some(run) = Run::write_ids(&self.ids) {
                self.id_runs.push(run);
                self.ids = Vec::new();
                self.compacted = 0;
            }
        }
    }

    fn spill_strs(&mut self) {
        if let Some(run) = Run::write_strs(&self.strs) {
            self.str_runs.push(run);
            self.strs.clear();
            self.str_bytes = 0;
        }
    }

    /// Runs written to disk so far (ids and strings).
    #[cfg(test)]
    pub(super) fn runs(&self) -> usize {
        self.id_runs.len() + self.str_runs.len()
    }

    /// The exact number of distinct values inserted.
    pub(super) fn count(mut self) -> u64 {
        self.ids.sort_unstable();
        self.ids.dedup();
        let ids = if self.id_runs.is_empty() {
            self.ids.len() as u64
        } else {
            let mut sources: Vec<Box<dyn Iterator<Item = i64>>> = Vec::new();
            for run in &self.id_runs {
                sources.push(Box::new(run.read_ids()));
            }
            sources.push(Box::new(std::mem::take(&mut self.ids).into_iter()));
            count_merged(sources)
        };
        let strs = if self.str_runs.is_empty() {
            self.strs.len() as u64
        } else {
            let mut sources: Vec<Box<dyn Iterator<Item = Vec<u8>>>> = Vec::new();
            for run in &self.str_runs {
                sources.push(Box::new(run.read_strs()));
            }
            let mem: Vec<Vec<u8>> = std::mem::take(&mut self.strs)
                .into_iter()
                .map(|s| s.as_bytes().to_vec())
                .collect();
            sources.push(Box::new(mem.into_iter()));
            count_merged(sources)
        };
        ids + strs
    }
}

/// Count the distinct values of several ascending, internally unique streams.
fn count_merged<T: Ord>(mut sources: Vec<Box<dyn Iterator<Item = T> + '_>>) -> u64 {
    let mut heap: BinaryHeap<std::cmp::Reverse<(T, usize)>> = BinaryHeap::new();
    for (i, s) in sources.iter_mut().enumerate() {
        if let Some(v) = s.next() {
            heap.push(std::cmp::Reverse((v, i)));
        }
    }
    let mut n = 0u64;
    let mut last: Option<T> = None;
    while let Some(std::cmp::Reverse((v, i))) = heap.pop() {
        if let Some(next) = sources[i].next() {
            heap.push(std::cmp::Reverse((next, i)));
        }
        if last.as_ref() != Some(&v) {
            n += 1;
            last = Some(v);
        }
    }
    n
}

/// One sorted run on disk, deleted when dropped.
struct Run {
    path: PathBuf,
}

impl Drop for Run {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

impl Run {
    /// A fresh, unique path in the temporary directory, or `None` where
    /// spilling is unavailable.
    fn create() -> Option<(Run, std::fs::File)> {
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rete-distinct-{}-{n}.run", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .ok()?;
        Some((Run { path }, file))
    }

    /// Write ascending unique ids: the first as a zigzag varint, then deltas.
    fn write_ids(ids: &[i64]) -> Option<Run> {
        let (run, file) = Run::create()?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        let mut buf = Vec::with_capacity(10);
        let mut prev: Option<i64> = None;
        for &v in ids {
            buf.clear();
            match prev {
                None => crate::varint::write_uvarint(&mut buf, ((v << 1) ^ (v >> 63)) as u64),
                Some(p) => crate::varint::write_uvarint(&mut buf, v.wrapping_sub(p) as u64),
            }
            w.write_all(&buf).ok()?;
            prev = Some(v);
        }
        w.flush().ok()?;
        Some(run)
    }

    /// Write ascending unique strings, each as a varint length and its bytes.
    fn write_strs(strs: &BTreeSet<Rc<str>>) -> Option<Run> {
        let (run, file) = Run::create()?;
        let mut w = BufWriter::with_capacity(1 << 20, file);
        let mut buf = Vec::with_capacity(10);
        for s in strs {
            buf.clear();
            crate::varint::write_uvarint(&mut buf, s.len() as u64);
            w.write_all(&buf).ok()?;
            w.write_all(s.as_bytes()).ok()?;
        }
        w.flush().ok()?;
        Some(run)
    }

    fn reader(&self) -> Option<BufReader<std::fs::File>> {
        std::fs::File::open(&self.path)
            .ok()
            .map(|f| BufReader::with_capacity(1 << 20, f))
    }

    fn read_ids(&self) -> impl Iterator<Item = i64> {
        let mut r = self.reader();
        let mut prev: Option<i64> = None;
        std::iter::from_fn(move || {
            let u = read_varint(r.as_mut()?)?;
            let v = match prev {
                None => ((u >> 1) as i64) ^ -((u & 1) as i64),
                Some(p) => p.wrapping_add(u as i64),
            };
            prev = Some(v);
            Some(v)
        })
    }

    fn read_strs(&self) -> impl Iterator<Item = Vec<u8>> {
        let mut r = self.reader();
        std::iter::from_fn(move || {
            let r = r.as_mut()?;
            let n = read_varint(r)? as usize;
            let mut s = vec![0u8; n];
            r.read_exact(&mut s).ok()?;
            Some(s)
        })
    }
}

/// One LEB128 varint from a byte stream (`None` at a clean or broken end).
fn read_varint(r: &mut impl Read) -> Option<u64> {
    let mut v = 0u64;
    let mut shift = 0u32;
    loop {
        let mut b = [0u8; 1];
        r.read_exact(&mut b).ok()?;
        v |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            return Some(v);
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(vals: &[Val]) -> u64 {
        vals.iter().collect::<BTreeSet<_>>().len() as u64
    }

    /// The count is exact whether everything stays in memory, the ids spill
    /// into many runs, or the strings do, with negative (predicate) ids,
    /// duplicates across runs, and values repeated millions of times.
    #[test]
    fn counts_exactly_at_any_threshold() {
        let mut vals: Vec<Val> = Vec::new();
        let mut x: u64 = 1;
        for i in 0..3_000_000u64 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(1);
            let id = ((x >> 20) % 900_000) as i64 - 1000; // some predicate tags
            vals.push(Val::Id(id));
            if i % 1000 == 0 {
                vals.push(Val::Str(Rc::from(format!("\"v{}\"", (x >> 40) % 1500))));
            }
        }
        // A value repeated far more often than any chunk holds.
        vals.extend(std::iter::repeat_n(Val::Id(42), 2_500_000));
        let want = exact(&vals);
        for threshold in [u64::MAX, 4 << 20, 64 << 10, 1] {
            let mut set = DistinctSet::new(threshold);
            for v in &vals {
                set.insert(v);
            }
            let spilled = set.runs();
            if threshold == u64::MAX {
                assert_eq!(spilled, 0);
            } else if threshold <= 64 << 10 {
                assert!(spilled > 1, "threshold {threshold}: {spilled} runs");
            }
            assert_eq!(set.count(), want, "threshold {threshold}");
        }
    }

    #[test]
    fn empty_and_single_values() {
        assert_eq!(DistinctSet::new(1).count(), 0);
        let mut s = DistinctSet::new(1);
        s.insert(&Val::Id(-5));
        s.insert(&Val::Id(-5));
        s.insert(&Val::Str(Rc::from("x")));
        assert_eq!(s.count(), 2);
    }

    #[test]
    fn runs_are_removed_when_done() {
        let mut s = DistinctSet::new(1);
        for i in 0..(MIN_CHUNK as i64 * 3) {
            s.insert(&Val::Id(i));
        }
        let paths: Vec<PathBuf> = s.id_runs.iter().map(|r| r.path.clone()).collect();
        assert!(!paths.is_empty());
        assert!(paths.iter().all(|p| p.exists()));
        assert_eq!(s.count(), MIN_CHUNK as u64 * 3);
        assert!(paths.iter().all(|p| !p.exists()));
    }
}
