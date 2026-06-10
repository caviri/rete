//! Range-reading abstraction (SPEC.md §9).
//!
//! A client doesn't need the whole file: it reads the 128-byte header, learns
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
