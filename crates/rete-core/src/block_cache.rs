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

use crate::reader::RangeReader;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

/// Default block size: 64 KiB — large enough to swallow a dictionary chunk or an
/// index tile in one fetch, small enough to keep over-fetch modest.
pub const DEFAULT_BLOCK: u64 = 64 * 1024;

pub struct BlockCacheReader<R> {
    inner: R,
    block: u64,
    len: u64,
    cache: Mutex<HashMap<u64, Arc<[u8]>>>,
}

impl<R: RangeReader> BlockCacheReader<R> {
    /// Wrap `inner`, fetching `block`-aligned blocks (clamped to ≥ 4 KiB).
    pub fn new(inner: R, block: u64) -> Self {
        let len = inner.len();
        Self {
            inner,
            block: block.max(4096),
            len,
            cache: Mutex::new(HashMap::new()),
        }
    }

    fn bounds(&self, offset: u64, len: u64) -> std::io::Result<()> {
        if offset.checked_add(len).map_or(true, |e| e > self.len) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "range out of bounds",
            ));
        }
        Ok(())
    }

    /// Fetch and cache every block index in `want` that isn't resident, issuing
    /// the missing ones as coalesced spans through `read_many`.
    fn ensure(&self, want: &BTreeSet<u64>) -> std::io::Result<()> {
        let missing: Vec<u64> = {
            let cache = self.cache.lock().unwrap();
            want.iter()
                .copied()
                .filter(|b| !cache.contains_key(b))
                .collect()
        };
        if missing.is_empty() {
            return Ok(());
        }
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
        let mut cache = self.cache.lock().unwrap();
        for (&(first, last), blob) in runs.iter().zip(blobs.into_iter()) {
            let span_start = first * self.block;
            for b in first..=last {
                let lo = (b * self.block - span_start) as usize;
                let hi = ((((b + 1) * self.block).min(self.len)) - span_start) as usize;
                let hi = hi.min(blob.len());
                let lo = lo.min(hi);
                cache.insert(b, Arc::from(&blob[lo..hi]));
            }
        }
        Ok(())
    }

    /// Copy `[offset, offset+len)` out of already-resident blocks.
    fn assemble(&self, offset: u64, len: u64) -> Vec<u8> {
        let mut out = Vec::with_capacity(len as usize);
        let cache = self.cache.lock().unwrap();
        let mut pos = offset;
        let end = offset + len;
        while pos < end {
            let b = pos / self.block;
            let blk = cache.get(&b).expect("block ensured before assemble");
            let within = (pos - b * self.block) as usize;
            let take = ((end - pos) as usize).min(blk.len().saturating_sub(within));
            if take == 0 {
                break;
            }
            out.extend_from_slice(&blk[within..within + take]);
            pos += take as u64;
        }
        out
    }
}

impl<R: RangeReader> RangeReader for BlockCacheReader<R> {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new());
        }
        self.bounds(offset, len)?;
        let want: BTreeSet<u64> = (offset / self.block..=(offset + len - 1) / self.block).collect();
        self.ensure(&want)?;
        Ok(self.assemble(offset, len))
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        let mut want = BTreeSet::new();
        for &(o, l) in ranges {
            if l == 0 {
                continue;
            }
            self.bounds(o, l)?;
            for b in o / self.block..=(o + l - 1) / self.block {
                want.insert(b);
            }
        }
        self.ensure(&want)?;
        Ok(ranges
            .iter()
            .map(|&(o, l)| {
                if l == 0 {
                    Vec::new()
                } else {
                    self.assemble(o, l)
                }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::{CountingReader, SliceReader};

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

    #[test]
    fn out_of_bounds_errors() {
        let data = vec![0u8; 1000];
        let r = BlockCacheReader::new(SliceReader::new(&data), 8192);
        assert!(r.read_at(990, 20).is_err());
    }
}
