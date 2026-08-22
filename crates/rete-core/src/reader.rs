//! Range-reading abstraction (SPEC.md §9).
//!
//! A client doesn't need the whole file: it reads the 1 KiB header, learns
//! where each section lives, then fetches only those byte ranges. [`RangeReader`]
//! is the seam — back it with a local file, an in-memory slice, or an HTTP
//! `Range` client. [`CountingReader`] wraps any reader to measure how few bytes
//! a given access pattern actually touches.

use crate::adaptive::{AdaptiveReadController, ReadIntent};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

fn materializable_len_with_limit(len: u64, limit: usize) -> std::io::Result<usize> {
    let len = usize::try_from(len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "range length does not fit this target's usize",
        )
    })?;
    if len > limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "range length exceeds this target's Vec limit",
        ));
    }
    Ok(len)
}

/// Convert a wire length into a length that this target can safely materialize
/// as a `Vec`.  File offsets stay `u64`; only an actual in-memory range is
/// limited by `usize` and Rust's `isize::MAX` allocation contract.
pub fn materializable_len(len: u64) -> std::io::Result<usize> {
    materializable_len_with_limit(len, isize::MAX as usize)
}

fn checked_resident_range_with_limit(
    offset: u64,
    len: u64,
    available: usize,
    address_limit: usize,
) -> std::io::Result<Range<usize>> {
    let end = offset.checked_add(len).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "resident range overflows")
    })?;
    let address_limit = u64::try_from(address_limit).unwrap_or(u64::MAX);
    if offset > address_limit || end > address_limit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "resident range does not fit this target's address space",
        ));
    }
    let start = usize::try_from(offset).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "resident range offset does not fit this target's usize",
        )
    })?;
    let end = usize::try_from(end).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "resident range end does not fit this target's usize",
        )
    })?;
    if end > available {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "resident range out of bounds",
        ));
    }
    Ok(start..end)
}

/// Validate an in-memory byte range without narrowing its wire coordinates.
/// Offsets remain `u64` for ranged readers; callers that already hold a
/// resident slice must also prove the range and resulting `Vec` fit this
/// target before indexing it.
pub fn checked_resident_range(
    offset: u64,
    len: u64,
    available: usize,
) -> std::io::Result<Range<usize>> {
    materializable_len(len)?;
    checked_resident_range_with_limit(offset, len, available, usize::MAX)
}

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

    /// Read exactly the requested range without opportunistically widening it.
    ///
    /// Most readers already fetch exact ranges, so the default delegates to
    /// [`read_at`](Self::read_at). Wrappers that normally prefetch or align
    /// reads (such as a block cache) override this for framing metadata whose
    /// physical transfer boundary is part of the caller's contract.
    fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.read_at(offset, len)
    }

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

    /// Read several ranges while preserving why the engine requested this
    /// batch. Readers without adaptive scheduling simply delegate to
    /// [`read_many`](Self::read_many); wrappers may use the intent to select a
    /// bounded physical plan without changing returned bytes.
    fn read_many_with_intent(
        &self,
        ranges: &[(u64, u64)],
        _intent: ReadIntent,
    ) -> std::io::Result<Vec<Vec<u8>>> {
        self.read_many(ranges)
    }

    /// Session-local adaptive controller attached to this physical source, if
    /// any. Wrappers must forward the same [`Arc`] so indexes and dictionaries
    /// learn from the cache's physical reads rather than separate models.
    fn adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>> {
        None
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

    fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        (**self).read_at_precise(offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        (**self).read_many(ranges)
    }

    fn read_many_with_intent(
        &self,
        ranges: &[(u64, u64)],
        intent: ReadIntent,
    ) -> std::io::Result<Vec<Vec<u8>>> {
        (**self).read_many_with_intent(ranges, intent)
    }

    fn adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>> {
        (**self).adaptive_controller()
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
        let range = checked_resident_range(offset, len, self.data.len())?;
        Ok(self
            .data
            .get(range)
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })?
            .to_vec())
    }
}

/// A [`RangeReader`] that owns a complete in-memory `.rete` image.
///
/// Unlike [`SliceReader`], this reader is `'static`, so a lazily opened
/// [`Rete`](crate::Rete) can retain it and fault dictionary chunks or index
/// tiles from the resident image on demand.
pub struct OwnedMemoryRangeReader {
    data: Vec<u8>,
    len: u64,
}

impl OwnedMemoryRangeReader {
    /// Wrap an owned file image for exact positional reads.
    pub fn new(data: Vec<u8>) -> std::io::Result<Self> {
        let len = u64::try_from(data.len()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "in-memory file length exceeds the ranged-reader limit",
            )
        })?;
        Ok(Self { data, len })
    }

    fn out_of_bounds(&self, offset: u64, len: u64) -> std::io::Error {
        std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            format!(
                "in-memory range out of bounds: requested {len} bytes at offset {offset} \
                 from a {}-byte file",
                self.len
            ),
        )
    }
}

impl RangeReader for OwnedMemoryRangeReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let end = offset
            .checked_add(len)
            .filter(|end| *end <= self.len)
            .ok_or_else(|| self.out_of_bounds(offset, len))?;
        let start = usize::try_from(offset).map_err(|_| self.out_of_bounds(offset, len))?;
        let end = usize::try_from(end).map_err(|_| self.out_of_bounds(offset, len))?;
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

    /// Number of single-range calls (`read_at` or `read_at_precise`) made so far.
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

    fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let out = self.inner.read_at_precise(offset, len)?;
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

    fn read_many_with_intent(
        &self,
        ranges: &[(u64, u64)],
        intent: ReadIntent,
    ) -> std::io::Result<Vec<Vec<u8>>> {
        let out = self.inner.read_many_with_intent(ranges, intent)?;
        self.requests.fetch_add(out.len() as u64, Ordering::Relaxed);
        self.bytes
            .fetch_add(out.iter().map(|b| b.len() as u64).sum(), Ordering::Relaxed);
        Ok(out)
    }

    fn adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>> {
        self.inner.adaptive_controller()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AdaptiveReadController, ReadIntent};
    use std::sync::Arc;

    struct DistinguishingReader;

    struct IntentReader {
        controller: Arc<AdaptiveReadController>,
        intent_reads: AtomicU64,
    }

    impl RangeReader for DistinguishingReader {
        fn len(&self) -> u64 {
            16
        }

        fn read_at(&self, _offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            Ok(vec![0x11; len as usize])
        }

        fn read_at_precise(&self, _offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            Ok(vec![0x22; len as usize])
        }
    }

    impl RangeReader for IntentReader {
        fn len(&self) -> u64 {
            32
        }

        fn read_at(&self, _offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            Ok(vec![0x33; len as usize])
        }

        fn read_many_with_intent(
            &self,
            ranges: &[(u64, u64)],
            _intent: ReadIntent,
        ) -> std::io::Result<Vec<Vec<u8>>> {
            self.intent_reads.fetch_add(1, Ordering::Relaxed);
            ranges
                .iter()
                .map(|&(_, len)| Ok(vec![0x44; len as usize]))
                .collect()
        }

        fn adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>> {
            Some(self.controller.clone())
        }
    }

    #[test]
    fn slice_reader_serves_ranges_and_bounds_check() {
        let data = (0u8..=255).collect::<Vec<_>>();
        let r = SliceReader::new(&data);
        assert_eq!(r.len(), 256);
        assert_eq!(r.read_at(10, 4).unwrap(), vec![10, 11, 12, 13]);
        assert!(r.read_at(254, 10).is_err()); // overruns
    }

    #[test]
    fn owned_memory_reader_serves_exact_ranges_and_rejects_overflow() {
        let r = OwnedMemoryRangeReader::new(vec![10, 20, 30, 40]).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(r.read_at(1, 2).unwrap(), vec![20, 30]);
        assert_eq!(r.read_at(4, 0).unwrap(), Vec::<u8>::new());

        let overrun = r.read_at(3, 2).unwrap_err();
        assert_eq!(overrun.kind(), std::io::ErrorKind::UnexpectedEof);
        let overflow = r.read_at(u64::MAX, 2).unwrap_err();
        assert_eq!(overflow.kind(), std::io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn materializable_length_accepts_vec_boundary_and_rejects_larger() {
        assert_eq!(
            materializable_len(isize::MAX as u64).unwrap(),
            isize::MAX as usize
        );
        assert!(materializable_len(isize::MAX as u64 + 1).is_err());
    }

    #[test]
    fn materializable_length_simulates_a_32_bit_target_without_aliasing() {
        let too_wide = u64::from(u32::MAX) + 1;
        assert!(materializable_len_with_limit(too_wide, u32::MAX as usize).is_err());
        assert_eq!(
            materializable_len_with_limit(17, u32::MAX as usize).unwrap(),
            17
        );
    }

    #[test]
    fn resident_slice_bounds_simulate_32_bit_offsets_and_lengths() {
        let max32 = u32::MAX as usize;
        assert!(checked_resident_range_with_limit(u64::from(u32::MAX) + 1, 0, 16, max32,).is_err());
        assert!(checked_resident_range_with_limit(0, u64::from(u32::MAX) + 7, 16, max32,).is_err());
        assert_eq!(
            checked_resident_range_with_limit(3, 4, 16, max32).unwrap(),
            3..7
        );
    }

    #[test]
    fn slice_reader_rejects_unrepresentable_resident_coordinates() {
        let data = [0u8; 16];
        let r = SliceReader::new(&data);
        assert!(r.read_at(u64::from(u32::MAX) + 1, 1).is_err());
        assert!(r.read_at(0, u64::from(u32::MAX) + 7).is_err());
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

    #[test]
    fn counting_reader_forwards_and_tallies_precise_reads() {
        let r = CountingReader::new(DistinguishingReader);
        assert_eq!(r.read_at_precise(3, 4).unwrap(), vec![0x22; 4]);
        assert_eq!(r.requests(), 1);
        assert_eq!(r.bytes_read(), 4);
    }

    #[test]
    fn arc_and_counting_reader_forward_intent_and_controller() {
        let controller = Arc::new(AdaptiveReadController::new());
        let inner = Arc::new(IntentReader {
            controller: controller.clone(),
            intent_reads: AtomicU64::new(0),
        });
        let reader = Arc::new(CountingReader::new(inner.clone()));

        let out = reader
            .read_many_with_intent(&[(0, 3), (8, 2)], ReadIntent::SelectiveProbe)
            .unwrap();

        assert_eq!(out, vec![vec![0x44; 3], vec![0x44; 2]]);
        assert_eq!(inner.intent_reads.load(Ordering::Relaxed), 1);
        assert_eq!(reader.requests(), 2);
        assert_eq!(reader.bytes_read(), 5);
        assert!(Arc::ptr_eq(
            &reader.adaptive_controller().unwrap(),
            &controller
        ));
    }
}
