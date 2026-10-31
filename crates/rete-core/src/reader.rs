//! Range-reading abstraction (SPEC.md §9).
//!
//! A client doesn't need the whole file: it reads the 1 KiB header, learns
//! where each section lives, then fetches only those byte ranges. [`RangeReader`]
//! is the seam — back it with a local file, an in-memory slice, or an HTTP
//! `Range` client. [`CountingReader`] wraps any reader to measure how few bytes
//! a given access pattern actually touches.

use std::sync::atomic::{AtomicU64, Ordering};

/// Something that can serve arbitrary byte ranges of a `.rete` resource.
pub trait RangeReader {
    /// Total resource length in bytes.
    fn len(&self) -> u64;

    /// True when the resource is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read `len` bytes starting at `offset`. Implementations should error on
    /// an out-of-bounds range rather than truncating.
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>>;

    /// Read several `(offset, len)` ranges, returning each range's bytes in
    /// request order. These ranges are independent, so a reader whose backing
    /// store is high-latency but parallelizable (an HTTP client) overrides this
    /// to issue the reads concurrently — turning N round trips into ~N/P. The
    /// default fetches them sequentially. Any range failing fails the batch.
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        ranges
            .iter()
            .map(|&(offset, len)| self.read_at(offset, len))
            .collect()
    }

    /// How many ranges this reader can usefully have in flight at once — the
    /// planner's hint for probe-vs-scan and batch-size decisions (a phone's
    /// serial sync-XHR reader reports 1; the CLI's threaded HTTP client and the
    /// browser's concurrent-fetch variants report their fan-out). Purely
    /// advisory: correctness never depends on it. Defaults to 1 (sequential).
    fn concurrency(&self) -> usize {
        1
    }
}

/// Sharing a reader (e.g. keeping a counting handle while a lazily-faulting
/// [`Rete`](crate::Rete) owns another) just delegates.
impl<R: RangeReader + ?Sized> RangeReader for std::sync::Arc<R> {
    fn len(&self) -> u64 {
        (**self).len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        (**self).read_at(offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        (**self).read_many(ranges)
    }

    fn concurrency(&self) -> usize {
        (**self).concurrency()
    }
}

/// A [`RangeReader`] over an in-memory byte slice (tests, embedded files).
pub struct SliceReader<'a> {
    data: &'a [u8],
}

impl<'a> SliceReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }
}

impl RangeReader for SliceReader<'_> {
    fn len(&self) -> u64 {
        self.data.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let start = offset as usize;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= self.data.len())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })?;
        Ok(self.data[start..end].to_vec())
    }
}

/// Wraps a reader and tallies how many ranges were requested and how many bytes
/// were returned — the metric that matters for a range-streamed format.
/// Atomically counted, so it stays `Sync` (a lazily-faulting remote index holds
/// its reader behind a shared loader).
pub struct CountingReader<R> {
    inner: R,
    requests: AtomicU64,
    bytes: AtomicU64,
}

impl<R: RangeReader> CountingReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner,
            requests: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }

    /// Number of `read_at` calls made so far.
    pub fn requests(&self) -> u64 {
        self.requests.load(Ordering::Relaxed)
    }

    /// Total bytes returned so far.
    pub fn bytes_read(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }
}

impl<R: RangeReader> RangeReader for CountingReader<R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let out = self.inner.read_at(offset, len)?;
        self.requests.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(out.len() as u64, Ordering::Relaxed);
        Ok(out)
    }

    /// Delegate to the inner reader (preserving its parallelism) and tally each
    /// returned range as one request — the count reflects the coalesced spans
    /// actually fetched, however the inner reader issues them.
    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let out = self.inner.read_many(ranges)?;
        self.requests.fetch_add(out.len() as u64, Ordering::Relaxed);
        self.bytes
            .fetch_add(out.iter().map(|b| b.len() as u64).sum(), Ordering::Relaxed);
        Ok(out)
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// Wraps a [`RangeReader`] so every access is shifted by `base` bytes: a `.rete`
/// that starts at byte `base` of the backing resource — e.g. behind an HTML shell
/// in a **polyglot** file that is both a web page and a graph — reads exactly as
/// if it began at offset 0, lazily, touching only the `.rete` bytes a query needs
/// and never the prefix. Pair with [`detect_polyglot_base`] to find `base`.
pub struct OffsetReader<R> {
    inner: R,
    base: u64,
}

impl<R> OffsetReader<R> {
    pub fn new(inner: R, base: u64) -> Self {
        Self { inner, base }
    }
    pub fn base(&self) -> u64 {
        self.base
    }
    pub fn into_inner(self) -> R {
        self.inner
    }
}

impl<R: RangeReader> RangeReader for OffsetReader<R> {
    fn len(&self) -> u64 {
        self.inner.len().saturating_sub(self.base)
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_at(self.base + offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let shifted: Vec<(u64, u64)> = ranges.iter().map(|&(o, l)| (self.base + o, l)).collect();
        self.inner.read_many(&shifted)
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// The marker a polyglot (HTML + `.rete`) file carries in its first bytes so a
/// reader can locate the embedded `.rete` without knowing the file size: ASCII
/// `RETE-BASE:` followed by [`POLYGLOT_DIGITS`] zero-padded decimal digits — the
/// byte offset where the `.rete` begins. It is emitted inside an HTML comment so
/// browsers ignore it, and it sits within the first header window a reader fetches.
pub const POLYGLOT_MARKER: &[u8] = b"RETE-BASE:";
/// Fixed decimal width of the offset that follows [`POLYGLOT_MARKER`].
pub const POLYGLOT_DIGITS: usize = 16;

/// If `head` (the first bytes of a resource whose byte 0 is NOT the `RETE` magic)
/// carries a [`POLYGLOT_MARKER`], return the byte offset of the embedded `.rete`.
pub fn detect_polyglot_base(head: &[u8]) -> Option<u64> {
    let pos = head
        .windows(POLYGLOT_MARKER.len())
        .position(|w| w == POLYGLOT_MARKER)?;
    let start = pos + POLYGLOT_MARKER.len();
    let digits = head.get(start..start + POLYGLOT_DIGITS)?;
    std::str::from_utf8(digits).ok()?.parse::<u64>().ok()
}

/// A [`RangeReader`] over owned, shared bytes (a whole file image held in
/// memory — the resident opens of a `0x06` file go through the lazy machinery
/// over one of these).
pub struct BytesReader(pub std::sync::Arc<[u8]>);

impl RangeReader for BytesReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|&e| e <= self.0.len() as u64)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })?;
        Ok(self.0[offset as usize..end as usize].to_vec())
    }
}

/// A reader **seeded** with the file's leading bytes: any range inside them is
/// served from memory, the rest goes to `inner`. The open reads the first
/// [`crate::v06::FIRST_READ`] bytes once; a `0x05` open's header, card and
/// first container prefixes then cost nothing more.
pub struct PrefixReader<R> {
    inner: R,
    prefix: std::sync::Arc<[u8]>,
}

impl<R: RangeReader> PrefixReader<R> {
    /// Seed `inner` with `prefix` (its bytes from offset 0).
    pub fn new(inner: R, prefix: std::sync::Arc<[u8]>) -> Self {
        PrefixReader { inner, prefix }
    }

    fn cut(&self, offset: u64, len: u64) -> Option<Vec<u8>> {
        let end = offset.checked_add(len)?;
        (end <= self.prefix.len() as u64)
            .then(|| self.prefix[offset as usize..end as usize].to_vec())
    }
}

impl<R: RangeReader> RangeReader for PrefixReader<R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        match self.cut(offset, len) {
            Some(b) => Ok(b),
            None => self.inner.read_at(offset, len),
        }
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let mut out: Vec<Option<Vec<u8>>> = ranges.iter().map(|&(o, l)| self.cut(o, l)).collect();
        let rest: Vec<(u64, u64)> = ranges
            .iter()
            .zip(&out)
            .filter(|(_, b)| b.is_none())
            .map(|(r, _)| *r)
            .collect();
        if !rest.is_empty() {
            let mut fetched = self.inner.read_many(&rest)?.into_iter();
            for slot in out.iter_mut().filter(|b| b.is_none()) {
                *slot = fetched.next();
            }
        }
        out.into_iter()
            .map(|b| b.ok_or_else(|| std::io::Error::other("read_many left a range unfetched")))
            .collect()
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slice_reader_serves_ranges_and_bounds_check() {
        let data = (0u8..=255).collect::<Vec<_>>();
        let r = SliceReader::new(&data);
        assert_eq!(r.len(), 256);
        assert_eq!(r.read_at(10, 4).unwrap(), vec![10, 11, 12, 13]);
        assert!(r.read_at(254, 10).is_err()); // overruns
    }

    #[test]
    fn counting_reader_tallies() {
        let data = vec![0u8; 100];
        let r = CountingReader::new(SliceReader::new(&data));
        r.read_at(0, 10).unwrap();
        r.read_at(50, 20).unwrap();
        assert_eq!(r.requests(), 2);
        assert_eq!(r.bytes_read(), 30);
    }
}
