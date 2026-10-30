//! A small **external sort** for the memory-bounded build's label and schema
//! passes: push records in any order, get them back sorted and deduplicated,
//! holding at most one run of roughly `run_bytes` in memory. Runs spill to
//! files in a caller-owned directory and are k-way merged on the way out.
//!
//! Records are anything that implements [`SortRecord`]: an `Ord` type that can
//! write itself to, and read itself back from, a byte stream.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::varint::write_uvarint;

/// A record the external sort can spill and reload.
pub(crate) trait SortRecord: Ord + Sized {
    /// Append the record's encoding.
    fn write_to(&self, out: &mut Vec<u8>);
    /// Read one record, `Ok(None)` at a clean end of stream.
    fn read_from<R: Read>(r: &mut R) -> std::io::Result<Option<Self>>;
    /// Approximate resident bytes, for sizing runs.
    fn mem_size(&self) -> usize;
}

fn read_varint<R: Read>(r: &mut R) -> std::io::Result<Option<u64>> {
    let mut v = 0u64;
    let mut shift = 0u32;
    let mut first = true;
    loop {
        let mut b = [0u8; 1];
        match r.read(&mut b)? {
            0 if first => return Ok(None),
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            _ => {}
        }
        first = false;
        if shift > 63 {
            return Err(std::io::Error::other("varint overflow"));
        }
        v |= ((b[0] & 0x7f) as u64) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(Some(v));
        }
        shift += 7;
    }
}

fn need<T>(v: Option<T>) -> std::io::Result<T> {
    v.ok_or_else(|| std::io::ErrorKind::UnexpectedEof.into())
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_uvarint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn read_str<R: Read>(r: &mut R, first: Option<u64>) -> std::io::Result<Option<String>> {
    let len = match first {
        Some(l) => l,
        None => match read_varint(r)? {
            Some(l) => l,
            None => return Ok(None),
        },
    };
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    String::from_utf8(buf)
        .map(Some)
        .map_err(|_| std::io::Error::other("bad utf8 in spill"))
}

/// `(a, b, c)` strings: the label passes' `(subject, slot, value)` (slot as a
/// one-character string) and `(key, subject, label)` records.
impl SortRecord for (String, String, String) {
    fn write_to(&self, out: &mut Vec<u8>) {
        write_str(out, &self.0);
        write_str(out, &self.1);
        write_str(out, &self.2);
    }
    fn read_from<R: Read>(r: &mut R) -> std::io::Result<Option<Self>> {
        let Some(a) = read_str(r, None)? else {
            return Ok(None);
        };
        let b = need(read_str(r, None)?)?;
        let c = need(read_str(r, None)?)?;
        Ok(Some((a, b, c)))
    }
    fn mem_size(&self) -> usize {
        self.0.len() + self.1.len() + self.2.len() + 3 * 24
    }
}

/// Fixed-width id pairs (`(subject, class)` type assignments).
impl SortRecord for (u32, u32) {
    fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
        out.extend_from_slice(&self.1.to_le_bytes());
    }
    fn read_from<R: Read>(r: &mut R) -> std::io::Result<Option<Self>> {
        let mut b = [0u8; 8];
        match read_exact_or_eof(r, &mut b)? {
            false => Ok(None),
            true => Ok(Some((
                u32::from_le_bytes(b[0..4].try_into().unwrap()),
                u32::from_le_bytes(b[4..8].try_into().unwrap()),
            ))),
        }
    }
    fn mem_size(&self) -> usize {
        8
    }
}

/// Fixed-width id triples (the deferred `(object, class(s), predicate)` links).
impl SortRecord for (u32, u32, u32) {
    fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
        out.extend_from_slice(&self.1.to_le_bytes());
        out.extend_from_slice(&self.2.to_le_bytes());
    }
    fn read_from<R: Read>(r: &mut R) -> std::io::Result<Option<Self>> {
        let mut b = [0u8; 12];
        match read_exact_or_eof(r, &mut b)? {
            false => Ok(None),
            true => Ok(Some((
                u32::from_le_bytes(b[0..4].try_into().unwrap()),
                u32::from_le_bytes(b[4..8].try_into().unwrap()),
                u32::from_le_bytes(b[8..12].try_into().unwrap()),
            ))),
        }
    }
    fn mem_size(&self) -> usize {
        12
    }
}

/// Two ids and a string: the label pass's `(subject, slot, value)`.
impl SortRecord for (u32, u32, String) {
    fn write_to(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.0.to_le_bytes());
        out.extend_from_slice(&self.1.to_le_bytes());
        write_str(out, &self.2);
    }
    fn read_from<R: Read>(r: &mut R) -> std::io::Result<Option<Self>> {
        let mut b = [0u8; 8];
        if !read_exact_or_eof(r, &mut b)? {
            return Ok(None);
        }
        let c = need(read_str(r, None)?)?;
        Ok(Some((
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            u32::from_le_bytes(b[4..8].try_into().unwrap()),
            c,
        )))
    }
    fn mem_size(&self) -> usize {
        8 + self.2.len() + 24
    }
}

/// `read_exact`, but a clean EOF before the first byte is `Ok(false)`.
fn read_exact_or_eof<R: Read>(r: &mut R, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut got = 0;
    while got < buf.len() {
        match r.read(&mut buf[got..])? {
            0 if got == 0 => return Ok(false),
            0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            n => got += n,
        }
    }
    Ok(true)
}

#[cfg(test)]
thread_local! {
    /// Tests set this to force many tiny runs (and so the k-way merge).
    pub(crate) static TEST_RUN_BYTES: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Collects records; spills a sorted, deduplicated run whenever the buffered
/// records reach `run_bytes`.
pub(crate) struct ExtSorter<T: SortRecord> {
    dir: PathBuf,
    name: String,
    run_bytes: usize,
    buf: Vec<T>,
    buf_bytes: usize,
    runs: Vec<PathBuf>,
    pushed: u64,
    dedup: bool,
}

impl<T: SortRecord> ExtSorter<T> {
    /// A sorter whose runs live in `dir` as `<name>.run<i>`.
    pub(crate) fn new(dir: &Path, name: &str, run_bytes: usize) -> Self {
        #[cfg(test)]
        let run_bytes = TEST_RUN_BYTES
            .with(|c| c.get())
            .unwrap_or(run_bytes.max(1 << 20));
        #[cfg(not(test))]
        let run_bytes = run_bytes.max(1 << 20);
        ExtSorter {
            dir: dir.to_path_buf(),
            name: name.to_string(),
            run_bytes,
            buf: Vec::new(),
            buf_bytes: 0,
            runs: Vec::new(),
            pushed: 0,
            dedup: true,
        }
    }

    /// Like [`new`](Self::new), but equal records are all kept (a count of
    /// statements, not a set of them).
    pub(crate) fn new_keep_duplicates(dir: &Path, name: &str, run_bytes: usize) -> Self {
        ExtSorter {
            dedup: false,
            ..Self::new(dir, name, run_bytes)
        }
    }

    /// Add a record.
    pub(crate) fn push(&mut self, r: T) -> std::io::Result<()> {
        self.buf_bytes += r.mem_size() + std::mem::size_of::<T>();
        self.buf.push(r);
        self.pushed += 1;
        if self.buf_bytes >= self.run_bytes {
            self.spill()?;
        }
        Ok(())
    }

    /// Records pushed so far (before deduplication).
    pub(crate) fn pushed(&self) -> u64 {
        self.pushed
    }

    fn spill(&mut self) -> std::io::Result<()> {
        if self.buf.is_empty() {
            return Ok(());
        }
        self.buf.sort_unstable();
        if self.dedup {
            self.buf.dedup();
        }
        let path = self
            .dir
            .join(format!("{}.run{}", self.name, self.runs.len()));
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let mut enc = Vec::new();
        for r in self.buf.drain(..) {
            enc.clear();
            r.write_to(&mut enc);
            w.write_all(&enc)?;
        }
        w.flush()?;
        self.runs.push(path);
        self.buf_bytes = 0;
        self.buf.shrink_to_fit();
        Ok(())
    }

    /// Finish: every record once, ascending.
    pub(crate) fn finish(mut self) -> std::io::Result<Sorted<T>> {
        if self.runs.is_empty() {
            self.buf.sort_unstable();
            if self.dedup {
                self.buf.dedup();
            }
            let v = std::mem::take(&mut self.buf);
            return Ok(Sorted::Memory(v.into_iter()));
        }
        self.spill()?;
        crate::build_trace::note_merge(self.runs.len());
        let mut readers = Vec::with_capacity(self.runs.len());
        let mut heap = BinaryHeap::new();
        for (i, p) in self.runs.iter().enumerate() {
            let mut rd = BufReader::with_capacity(1 << 18, File::open(p)?);
            if let Some(r) = T::read_from(&mut rd)? {
                heap.push(std::cmp::Reverse((r, i)));
            }
            readers.push(rd);
        }
        Ok(Sorted::Runs {
            readers,
            heap,
            runs: std::mem::take(&mut self.runs),
            last: None,
            dedup: self.dedup,
        })
    }
}

/// The sorted, deduplicated output of an [`ExtSorter`].
pub(crate) enum Sorted<T: SortRecord> {
    /// Everything fit in one run; it never touched the disk.
    Memory(std::vec::IntoIter<T>),
    /// A k-way merge over spilled runs.
    Runs {
        readers: Vec<BufReader<File>>,
        heap: BinaryHeap<std::cmp::Reverse<(T, usize)>>,
        runs: Vec<PathBuf>,
        last: Option<T>,
        dedup: bool,
    },
}

impl<T: SortRecord + Clone> Sorted<T> {
    /// The next record, or `Ok(None)` when exhausted.
    pub(crate) fn next_record(&mut self) -> std::io::Result<Option<T>> {
        match self {
            Sorted::Memory(it) => Ok(it.next()),
            Sorted::Runs {
                readers,
                heap,
                last,
                dedup,
                ..
            } => loop {
                let Some(std::cmp::Reverse((r, i))) = heap.pop() else {
                    return Ok(None);
                };
                if let Some(n) = T::read_from(&mut readers[i])? {
                    heap.push(std::cmp::Reverse((n, i)));
                }
                if !*dedup || last.as_ref() != Some(&r) {
                    *last = Some(r.clone());
                    return Ok(Some(r));
                }
            },
        }
    }
}

impl<T: SortRecord> Drop for Sorted<T> {
    fn drop(&mut self) {
        if let Sorted::Runs { runs, .. } = self {
            for p in runs.iter() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rete-extsort-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn sorts_and_dedups_across_spilled_runs() {
        let dir = tmpdir("strings");
        let mut s: ExtSorter<(String, String, String)> = ExtSorter::new(&dir, "t", 1);
        // run_bytes is floored at 1 MiB; push enough to spill several runs.
        let n = 60_000u32;
        for i in (0..n).rev() {
            let r = (
                format!("k{:06}", i % 20_000),
                "s".to_string(),
                "v".repeat(16),
            );
            s.push(r.clone()).unwrap();
        }
        assert!(s.runs.len() >= 2, "spilled {} runs", s.runs.len());
        let mut out = s.finish().unwrap();
        let mut got = Vec::new();
        while let Some(r) = out.next_record().unwrap() {
            got.push(r.0);
        }
        assert_eq!(got.len(), 20_000);
        assert!(got.windows(2).all(|w| w[0] < w[1]));
        drop(out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_input_stays_in_memory() {
        let dir = tmpdir("ids");
        let mut s: ExtSorter<(u32, u32)> = ExtSorter::new(&dir, "t", 1 << 20);
        for (a, b) in [(3, 1), (1, 2), (3, 1), (2, 9)] {
            s.push((a, b)).unwrap();
        }
        let mut out = s.finish().unwrap();
        let mut got = Vec::new();
        while let Some(r) = out.next_record().unwrap() {
            got.push(r);
        }
        assert_eq!(got, vec![(1, 2), (2, 9), (3, 1)]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
