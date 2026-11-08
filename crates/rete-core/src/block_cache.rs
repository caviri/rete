//! A read-through **block cache** over any [`RangeReader`] — the client-side
//! half of lazy range serving (SPEC.md §9).
//!
//! The engine faults small, scattered byte ranges (a dictionary chunk here, an
//! index tile there). Issued one-for-one against a high-latency remote, that is
//! many round trips. This wrapper fetches fixed-size, **block-aligned** spans
//! instead of exact ranges and caches them, so:
//!
//! - nearby reads coalesce into a few block fetches,
//! - repeated reads (and the header re-read) are free, and
//! - it needs only plain single-range `Range: bytes=a-b` support — so it speeds
//!   up reads from **any** object store (S3, GCS, Azure, a CDN) that has no
//!   multi-range / `multipart/byteranges` support at all.
//!
//! The trade is a little over-fetch (whole blocks) for far fewer requests — the
//! right call on a latency-bound link. Missing blocks for one access are fetched
//! through [`RangeReader::read_many`], so a backend that *does* support
//! multi-range collapses them further still; the two optimizations compose.
//!
//! Residency is **bounded**: past a byte cap (default [`DEFAULT_CACHE_CAP`],
//! overridable with [`BlockCacheReader::with_cache_cap`]) the least-recently
//! touched blocks are evicted after each read. Without the cap, a wide scan of a
//! multi-GB remote file would grow the cache toward the whole file — on 32-bit
//! wasm (a 4 GiB address space shared with the decompressed tiles and dictionary
//! chunks) that is an out-of-memory crash, not a slowdown.

use crate::reader::RangeReader;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Condvar, Mutex};

/// Default block size: 64 KiB — large enough to swallow a dictionary chunk or an
/// index tile in one fetch, small enough to keep over-fetch modest.
pub const DEFAULT_BLOCK: u64 = 64 * 1024;

/// Default cap on resident cached bytes: 256 MiB. Large enough that a working
/// set (the tiles + dictionary chunks a query family touches) stays warm; small
/// enough that a full sweep of a multi-GB file leaves plenty of the 32-bit wasm
/// address space for the decompressed structures built on top of these bytes.
/// At the auto-tuned 128–512 KiB block sizes this is 512–2048 resident blocks,
/// so the eviction scan is trivial.
pub const DEFAULT_CACHE_CAP: u64 = 256 * 1024 * 1024;

/// Pick a [`BlockCacheReader`] block size from the file length: bigger files get
/// bigger blocks so a remote query makes far fewer (but larger) round trips —
/// 128 KiB ≤ 10 MB, 256 KiB ≤ 100 MB, 512 KiB above. The over-fetch is modest
/// next to the round-trip latency it saves on a high-latency link (S3/CDN). Shared
/// by the CLI and the wasm client so both size identically; the file length is
/// known for free from the opening `HEAD` / `Content-Range`.
pub fn auto_block(len: u64) -> u64 {
    const MB: u64 = 1 << 20;
    // The reader coalesces byte-adjacent missing blocks into one request, so a
    // large block buys little on a contiguous read — it only over-fetches the
    // *scattered* faults that dominate a selective query (a dictionary chunk is
    // 64 KiB; a 512 KiB block dragged in 8× the bytes). Keep the block near the
    // chunk size; nearby-but-not-adjacent faults still merge when they share a
    // block, and the bigger tier for huge files trades a little over-fetch for
    // fewer round trips on the biggest scans.
    let mult: u64 = if len > 100 * MB {
        2 // 128 KiB
    } else {
        1 // 64 KiB
    };
    mult * DEFAULT_BLOCK
}

/// What a cached block costs against the cap: its allocation's real size
/// (`Arc` header included, rounded to the allocator's class), not its length —
/// a 128 KiB block occupies 160 KiB under jemalloc.
fn charge(data: &Arc<[u8]>) -> u64 {
    crate::chunk_cache::alloc_footprint(data.len() as u64 + 16)
}

/// One resident block: its bytes plus the last-touch stamp eviction orders by.
struct CacheEntry {
    data: Arc<[u8]>,
    stamp: u64,
}

/// The cache state behind one mutex: resident blocks, their total byte size,
/// and the monotonic access counter that makes eviction least-recently-used.
struct CacheState {
    map: HashMap<u64, CacheEntry>,
    used: u64,
    tick: u64,
    /// Blocks a reader is fetching right now: a concurrent reader that wants
    /// one waits for it ([`BlockCacheReader::landed`]) instead of fetching the
    /// same bytes a second time.
    in_flight: HashSet<u64>,
}

pub struct BlockCacheReader<R> {
    inner: R,
    block: u64,
    len: u64,
    cap: u64,
    cache: Mutex<CacheState>,
    /// Signalled whenever a fetch lands (or fails) and clears its in-flight
    /// marks.
    landed: Condvar,
}

impl<R: RangeReader> BlockCacheReader<R> {
    /// Wrap `inner`, fetching `block`-aligned blocks (clamped to ≥ 4 KiB) and
    /// keeping at most [`DEFAULT_CACHE_CAP`] bytes of them resident.
    pub fn new(inner: R, block: u64) -> Self {
        let len = inner.len();
        Self {
            inner,
            block: block.max(4096),
            len,
            cap: DEFAULT_CACHE_CAP,
            cache: Mutex::new(CacheState {
                map: HashMap::new(),
                used: 0,
                tick: 0,
                in_flight: HashSet::new(),
            }),
            landed: Condvar::new(),
        }
    }

    /// Override the resident-byte cap. The cap is enforced *between* reads: a
    /// single read spanning more than `cap` bytes of blocks is still served
    /// exactly (its blocks are resident while it assembles) and the cache is
    /// trimmed back under the cap right after. `u64::MAX` disables eviction.
    pub fn with_cache_cap(mut self, cap: u64) -> Self {
        self.cap = cap;
        self
    }

    /// Bytes currently resident in the cache (for stats and tests).
    pub fn cached_bytes(&self) -> u64 {
        self.cache.lock().unwrap().used
    }

    /// Does the inner reader already hold `[offset, offset+len)` in memory?
    fn held(&self, offset: u64, len: u64) -> bool {
        offset
            .checked_add(len)
            .is_some_and(|e| e <= self.inner.resident_prefix())
    }

    fn bounds(&self, offset: u64, len: u64) -> std::io::Result<()> {
        if offset.checked_add(len).is_none_or(|e| e > self.len) {
            // Name the numbers: this fires when a read the file's own layout
            // demands overruns the length the transport reported, and the two
            // disagreeing values are the whole diagnosis (issue #95: a host
            // advertised its gzip size, 58 MB, for a 71 MB file).
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!(
                    "range out of bounds: read {offset}+{len} exceeds the reported resource length {} — \
                     the file's layout expects more bytes than the length probe found \
                     (a host serving a compressed Content-Length can cause this)",
                    self.len
                ),
            ));
        }
        Ok(())
    }

    /// Fetch and cache every block index in `want` that isn't resident, issuing
    /// the missing ones as coalesced spans through `read_many`. Already-resident
    /// wanted blocks get their recency stamp refreshed.
    fn ensure(&self, want: &BTreeSet<u64>) -> std::io::Result<()> {
        // Resident blocks are touched; blocks another reader is fetching are
        // waited for, not fetched again (a parallel open walks two containers
        // at once and both often want the next block); the rest are claimed.
        let (missing, others): (Vec<u64>, Vec<u64>) = {
            let mut st = self.cache.lock().unwrap();
            st.tick += 1;
            let tick = st.tick;
            let mut missing = Vec::new();
            let mut others = Vec::new();
            for &b in want {
                if let Some(e) = st.map.get_mut(&b) {
                    e.stamp = tick;
                } else if st.in_flight.contains(&b) {
                    others.push(b);
                } else {
                    missing.push(b);
                }
            }
            for &b in &missing {
                st.in_flight.insert(b);
            }
            (missing, others)
        };
        let fetched = self.fetch_claimed(&missing);
        if !others.is_empty() {
            let mut st = self.cache.lock().unwrap();
            while others.iter().any(|b| st.in_flight.contains(b)) {
                st = self.landed.wait(st).unwrap();
            }
            // A block the other reader failed to fetch is read directly by
            // `assemble`: correctness never depends on residency.
        }
        fetched
    }

    /// Fetch the blocks this reader claimed in [`ensure`](Self::ensure), cache
    /// them, and clear their in-flight marks whatever the outcome.
    fn fetch_claimed(&self, missing: &[u64]) -> std::io::Result<()> {
        if missing.is_empty() {
            return Ok(());
        }
        let r = self.fetch_missing(missing);
        let mut st = self.cache.lock().unwrap();
        for b in missing {
            st.in_flight.remove(b);
        }
        drop(st);
        self.landed.notify_all();
        r
    }

    fn fetch_missing(&self, missing: &[u64]) -> std::io::Result<()> {
        // Coalesce consecutive block indices into one span each.
        let mut spans: Vec<(u64, u64)> = Vec::new();
        let mut runs: Vec<(u64, u64)> = Vec::new();
        let mut i = 0;
        while i < missing.len() {
            let first = missing[i];
            let mut last = first;
            let mut j = i + 1;
            while j < missing.len() && missing[j] == last + 1 {
                last = missing[j];
                j += 1;
            }
            let off = first * self.block;
            let end = ((last + 1) * self.block).min(self.len);
            spans.push((off, end - off));
            runs.push((first, last));
            i = j;
        }
        let blobs = self.inner.read_many(&spans)?;
        if blobs.len() != spans.len() {
            return Err(std::io::Error::other("block fetch returned wrong count"));
        }
        // A blob of any other length than its span is not that span: caching
        // a short one would hand the parsers a silently truncated block.
        if let Some(((o, l), b)) = spans
            .iter()
            .zip(&blobs)
            .find(|((_, l), b)| b.len() as u64 != *l)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("block fetch at {o} returned {} bytes, not {l}", b.len()),
            ));
        }
        let mut st = self.cache.lock().unwrap();
        st.tick += 1;
        let tick = st.tick;
        for (&(first, last), blob) in runs.iter().zip(blobs.into_iter()) {
            let span_start = first * self.block;
            for b in first..=last {
                let lo = (b * self.block - span_start) as usize;
                let hi = ((((b + 1) * self.block).min(self.len)) - span_start) as usize;
                let hi = hi.min(blob.len());
                let lo = lo.min(hi);
                let data: Arc<[u8]> = Arc::from(&blob[lo..hi]);
                st.used += charge(&data);
                if let Some(old) = st.map.insert(b, CacheEntry { data, stamp: tick }) {
                    st.used -= charge(&old.data); // concurrent double-fetch
                }
            }
        }
        Ok(())
    }

    /// Evict least-recently-touched blocks until the resident total fits the
    /// cap. Called after a read has assembled its bytes, so nothing in flight
    /// depends on what gets dropped.
    ///
    /// Once over the cap it evicts down to 7/8 of it, not just under it:
    /// finding the least-recent blocks sorts the whole map, and a full cache
    /// (every read of a long export) used to pay that sort on every single
    /// read. The slack amortizes one sort over ~1/8 of the cache's turnover
    /// (only once the cache holds 32+ blocks, so a tiny cache stays exact LRU).
    fn trim(&self) {
        let mut st = self.cache.lock().unwrap();
        if st.used <= self.cap {
            return;
        }
        let slack = if self.cap / 8 >= 4 * self.block {
            self.cap / 8
        } else {
            0
        };
        let target = self.cap - slack;
        let mut order: Vec<(u64, u64)> = st.map.iter().map(|(&b, e)| (e.stamp, b)).collect();
        order.sort_unstable();
        for (_, b) in order {
            if st.used <= target {
                break;
            }
            if let Some(e) = st.map.remove(&b) {
                st.used -= charge(&e.data);
            }
        }
    }

    /// Copy `[offset, offset+len)` out of the cache. The needed blocks are
    /// cloned out under one short lock; a block missing here (evicted by a
    /// concurrent reader's trim between `ensure` and this call) is re-read
    /// directly from the inner reader — correctness never depends on residency.
    fn assemble(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let first = offset / self.block;
        let last = (offset + len - 1) / self.block;
        let resident: Vec<Option<Arc<[u8]>>> = {
            let st = self.cache.lock().unwrap();
            (first..=last)
                .map(|b| st.map.get(&b).map(|e| e.data.clone()))
                .collect()
        };
        let mut out = Vec::with_capacity(len as usize);
        let mut pos = offset;
        let end = offset + len;
        while pos < end {
            let b = pos / self.block;
            let block_start = b * self.block;
            let within = (pos - block_start) as usize;
            let fetched: Vec<u8>;
            let blk: &[u8] = match &resident[(b - first) as usize] {
                Some(data) => data,
                None => {
                    let blen = ((b + 1) * self.block).min(self.len) - block_start;
                    fetched = self.inner.read_at(block_start, blen)?;
                    &fetched
                }
            };
            let take = ((end - pos) as usize).min(blk.len().saturating_sub(within));
            if take == 0 {
                break;
            }
            out.extend_from_slice(&blk[within..within + take]);
            pos += take as u64;
        }
        if out.len() as u64 != len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                format!("short read: {} of {len} bytes at {offset}", out.len()),
            ));
        }
        Ok(out)
    }
}

impl<R: RangeReader> RangeReader for BlockCacheReader<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }

    fn resident_prefix(&self) -> u64 {
        self.inner.resident_prefix()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        self.bounds(offset, len)?;
        // Bytes the inner reader already holds (an HTTP open's first read) are
        // no fetch: asking it for the block-aligned span around them would
        // re-read them, and the rest of the block, in a request of its own.
        if self.held(offset, len) {
            return self.inner.read_at(offset, len);
        }
        let want: BTreeSet<u64> = (offset / self.block..=(offset + len - 1) / self.block).collect();
        self.ensure(&want)?;
        let out = self.assemble(offset, len)?;
        self.trim();
        Ok(out)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let mut want = BTreeSet::new();
        for &(o, l) in ranges {
            if l == 0 {
                continue;
            }
            self.bounds(o, l)?;
            if self.held(o, l) {
                continue;
            }
            for b in o / self.block..=(o + l - 1) / self.block {
                want.insert(b);
            }
        }
        self.ensure(&want)?;
        let out = ranges
            .iter()
            .map(|&(o, l)| {
                if l == 0 {
                    Ok(Vec::new())
                } else if self.held(o, l) {
                    self.inner.read_at(o, l)
                } else {
                    self.assemble(o, l)
                }
            })
            .collect::<std::io::Result<Vec<_>>>()?;
        self.trim();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::PrefixReader;
    use crate::reader::{CountingReader, SliceReader};

    /// An HTTP open already holds the file's first bytes: a read inside them
    /// is served as it is, not by fetching the block-aligned span around it.
    #[test]
    fn reads_inside_the_inner_prefix_fetch_no_block() {
        let data: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        let prefix: Arc<[u8]> = Arc::from(&data[..4096]);
        let counting = Arc::new(CountingReader::new(SliceReader::new(&data)));
        let r = BlockCacheReader::new(PrefixReader::new(counting.clone(), prefix), 16 * 1024);
        assert_eq!(r.read_at(0, 4096).unwrap(), data[..4096]);
        assert_eq!(r.read_many(&[(10, 100)]).unwrap()[0], data[10..110]);
        assert_eq!(counting.requests(), 0);
        // Past the prefix the block is fetched whole, as before.
        assert_eq!(r.read_at(5000, 10).unwrap(), data[5000..5010]);
        assert_eq!(counting.requests(), 1);
        assert_eq!(counting.bytes_read(), 16 * 1024);
    }

    #[test]
    fn caches_blocks_and_serves_exact_bytes() {
        let data: Vec<u8> = (0..100_000u32).map(|i| i as u8).collect();
        let counting = Arc::new(CountingReader::new(SliceReader::new(&data)));
        let r = BlockCacheReader::new(counting.clone(), 16 * 1024);

        // many small scattered reads inside the first few blocks
        for off in [10u64, 200, 5000, 16_500, 17_000, 33_000, 33_100] {
            assert_eq!(
                r.read_at(off, 32).unwrap(),
                data[off as usize..off as usize + 32]
            );
        }
        // exact bytes returned, but the 7 logical reads touched only 3 blocks
        // (0,1,2) → at most 3 physical fetches, not 7.
        assert!(
            counting.requests() <= 3,
            "physical fetches: {}",
            counting.requests()
        );

        // a repeat read is fully cached → no new fetch
        let before = counting.requests();
        assert_eq!(r.read_at(200, 16).unwrap(), data[200..216]);
        assert_eq!(counting.requests(), before);
    }

    #[test]
    fn read_many_fetches_missing_blocks_once() {
        let data: Vec<u8> = (0..200_000u32).map(|i| (i * 7) as u8).collect();
        let counting = Arc::new(CountingReader::new(SliceReader::new(&data)));
        let r = BlockCacheReader::new(counting.clone(), 32 * 1024);
        let ranges = [(0u64, 8u64), (40_000, 16), (40_050, 16), (130_000, 64)];
        let out = r.read_many(&ranges).unwrap();
        for (&(o, l), got) in ranges.iter().zip(&out) {
            assert_eq!(got, &data[o as usize..(o + l) as usize]);
        }
        // 4 ranges over blocks {0, 1, 4} → ≤ 3 physical fetches.
        assert!(
            counting.requests() <= 3,
            "physical: {}",
            counting.requests()
        );
    }

    /// A slow reader that counts its reads.
    struct Slow(Vec<u8>, std::sync::atomic::AtomicUsize);
    impl RangeReader for Slow {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            self.1.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(30));
            Ok(self.0[offset as usize..(offset + len) as usize].to_vec())
        }
    }

    /// Readers that want the same block at the same time fetch it once: the
    /// others wait for it to land (a parallel open walks two containers at
    /// once, and both used to fetch, and count, the next block).
    #[test]
    fn concurrent_readers_fetch_a_block_once() {
        let r = BlockCacheReader::new(Slow(vec![3u8; 100_000], Default::default()), 4096);
        std::thread::scope(|s| {
            for _ in 0..6 {
                s.spawn(|| assert_eq!(r.read_at(5000, 100).unwrap(), vec![3u8; 100]));
            }
        });
        assert_eq!(r.inner.1.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// A reader that answers every range one byte short (audit fix 9).
    struct Short(Vec<u8>);
    impl RangeReader for Short {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            let end = (offset + len).saturating_sub(1) as usize;
            Ok(self.0[offset as usize..end].to_vec())
        }
    }

    /// A short blob from the inner reader is an error, never a short block
    /// cached and handed out as if it were the range.
    #[test]
    fn a_short_blob_is_refused_not_cached() {
        let r = BlockCacheReader::new(Short(vec![9u8; 10_000]), 4096);
        let err = r.read_at(100, 50).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof, "{err}");
        assert!(r.read_many(&[(0, 10), (5000, 10)]).is_err());
    }

    #[test]
    fn out_of_bounds_errors() {
        let data = vec![0u8; 1000];
        let r = BlockCacheReader::new(SliceReader::new(&data), 8192);
        assert!(r.read_at(990, 20).is_err());
    }

    /// A sweep far wider than the cap must stay under the cap after every read
    /// while still returning exact bytes — the unbounded-growth regression.
    #[test]
    fn eviction_caps_resident_bytes_and_keeps_reads_exact() {
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i * 31) as u8).collect();
        let counting = Arc::new(CountingReader::new(SliceReader::new(&data)));
        // 64 KiB cap over 8 KiB blocks = at most 8 resident blocks.
        let cap = 64 * 1024;
        let r = BlockCacheReader::new(counting.clone(), 8192).with_cache_cap(cap);
        for off in (0..1_000_000u64 - 64).step_by(37_777) {
            assert_eq!(
                r.read_at(off, 64).unwrap(),
                data[off as usize..off as usize + 64]
            );
            assert!(
                r.cached_bytes() <= cap,
                "resident {} > cap {cap}",
                r.cached_bytes()
            );
        }
    }

    /// Eviction is least-recently-used: re-touching a block protects it, the
    /// stalest block goes first, and an evicted block re-fetches exactly once.
    #[test]
    fn eviction_is_lru() {
        let data: Vec<u8> = (0..64 * 1024u32).map(|i| i as u8).collect();
        let counting = Arc::new(CountingReader::new(SliceReader::new(&data)));
        // Cap = exactly 2 blocks of 8 KiB (as charged).
        let two = 2 * charge(&Arc::from(vec![0u8; 8192]));
        let r = BlockCacheReader::new(counting.clone(), 8192).with_cache_cap(two);
        let block = |i: u64| i * 8192;
        r.read_at(block(0), 16).unwrap(); // cache {0}
        r.read_at(block(1), 16).unwrap(); // cache {0,1}
        r.read_at(block(0), 16).unwrap(); // touch 0 → 1 is now the LRU

        let before = counting.requests();
        r.read_at(block(2), 16).unwrap(); // fetches 2, evicts 1, keeps 0
        assert_eq!(counting.requests(), before + 1);

        let before = counting.requests();
        r.read_at(block(0), 16).unwrap(); // still resident → no fetch
        assert_eq!(
            counting.requests(),
            before,
            "recently-touched block evicted"
        );

        let before = counting.requests();
        r.read_at(block(1), 16).unwrap(); // was evicted → exactly one refetch
        assert_eq!(counting.requests(), before + 1);
    }

    /// One read spanning more blocks than the cap allows must still return the
    /// exact bytes (blocks are resident while the read assembles) and the cache
    /// must be trimmed back under the cap immediately after.
    #[test]
    fn request_larger_than_cap_reads_exactly_then_trims() {
        let data: Vec<u8> = (0..256 * 1024u32).map(|i| (i * 7) as u8).collect();
        let cap = 16 * 1024;
        let r = BlockCacheReader::new(SliceReader::new(&data), 8192).with_cache_cap(cap);
        let (off, len) = (1000usize, 96 * 1024usize); // 6× the cap
        let out = r.read_at(off as u64, len as u64).unwrap();
        assert_eq!(out, &data[off..off + len]);
        assert!(
            r.cached_bytes() <= cap,
            "resident {} > cap {cap} after the read",
            r.cached_bytes()
        );
    }
}
