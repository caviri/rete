//! An evictable, byte-capped cache for decompressed dictionary chunks and index
//! tiles (bounded-export, phase 0).
//!
//! Before this module, three structures faulted a decompressed body in on demand
//! and **never released it** — `SectionChunk.data` (a chunk's decoded body),
//! `SectionChunk.runs` (its per-run offset table), and `Tile.data` (a decoded
//! index tile). A full export touches every chunk of every section, so by the end
//! the whole decompressed dictionary was resident — the memory floor a ranged
//! open otherwise avoids. This cache is the single place all three retentions now
//! live, so a future phase can put a real byte cap on them.
//!
//! **Phase 0 is a pure refactor: the default cap is unlimited ([`u64::MAX`]), so
//! nothing is ever evicted and residency, output and timing are identical to the
//! `OnceLock`-forever design it replaces.** The eviction machinery below is
//! exercised only by tests (which set a tiny cap) until a later phase wires a
//! real budget through.
//!
//! ## The `Arc` handout invariant
//!
//! [`get`](ChunkCache::get) / [`insert`](ChunkCache::insert) hand back an
//! `Arc<CacheEntry>`. Eviction only drops the *map entry* (and its share of the
//! resident-byte total); the bytes themselves live until the last `Arc` clone is
//! dropped. So a body handed to a caller stays valid across any later eviction —
//! a resolver can hold an entry across a decode without pinning the map slot.
//!
//! ## Thread-safety
//!
//! The cache is `Send + Sync`: a `Mutex` guards the map and the recency order,
//! and the body decode happens **off-lock** (the caller decodes, then inserts).
//! `Dictionary`/`GraphIndex` are shared across query threads, so this is
//! required; wasm is single-threaded, where the `Mutex` is simply uncontended.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// One flat key space over both consumers: the high byte is the section /
/// permutation, the low is the chunk / tile index within it. Two `ChunkCache`
/// instances (one per `Dictionary`, one per `GraphIndex`) keep the two key
/// spaces trivially disjoint.
pub type CacheKey = (u8, u32);

/// Charged per entry on top of its body so a section of many tiny chunks cannot
/// silently overshoot the cap through bookkeeping alone (the `Arc` refcount
/// header, the map node, the recency-order node). A deliberate slight over-count.
const ENTRY_OVERHEAD: u64 = 96;

/// A cached decompressed body plus, for a dictionary chunk, its lazily-derived
/// per-run offset table. Evicted as a unit; handed out behind an `Arc` so it
/// outlives its map slot (see the module docs).
///
/// Index tiles never populate `runs` — their derived group directory stays a
/// per-`Tile` `OnceLock` — so the field costs one unset word there.
pub struct CacheEntry {
    body: Arc<[u8]>,
    /// A dictionary chunk's per-run byte offsets, relative to `body`. Computed
    /// once on first lookup (via [`runs`](CacheEntry::runs)) and cached here so
    /// it is evicted together with the body it indexes.
    runs: OnceLock<Arc<[usize]>>,
}

impl CacheEntry {
    /// The decompressed body.
    #[inline]
    pub fn body(&self) -> &[u8] {
        &self.body
    }

    /// A shared handle to the body (cheap `Arc` clone) — for a caller that must
    /// keep the bytes alive past the entry handle itself.
    #[inline]
    pub fn body_arc(&self) -> Arc<[u8]> {
        Arc::clone(&self.body)
    }

    /// This chunk's per-run byte offsets, computing them from the body via
    /// `compute` on first call and caching the result inside the entry. An empty
    /// body yields `&[]` and is **not** cached — matching the old
    /// `SectionChunk::run_offsets`, whose empty-body path (the transient
    /// fetch-failure sentinel) was likewise never memoized.
    #[inline]
    pub fn runs(&self, compute: impl FnOnce(&[u8]) -> Vec<usize>) -> &[usize] {
        if self.body.is_empty() {
            return &[];
        }
        &self.runs.get_or_init(|| Arc::from(compute(&self.body)))[..]
    }

    /// Bytes this entry charges against the cap. Runs are derived lazily after
    /// insertion, so they are not counted here (small, and never charged while
    /// the cap is unlimited); the body plus a fixed overhead is the charge.
    #[inline]
    fn charge(&self) -> u64 {
        self.body.len() as u64 + ENTRY_OVERHEAD
    }
}

/// A snapshot of the cache's observability counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub decoded_chunks: u64,
    pub decoded_bytes: u64,
    pub evictions: u64,
    pub resident_bytes: u64,
    pub entries: u64,
}

struct Slot {
    entry: Arc<CacheEntry>,
    /// Recency stamp; also this slot's key into `order`.
    seq: u64,
    charge: u64,
}

#[derive(Default)]
struct CacheInner {
    map: HashMap<CacheKey, Slot>,
    /// Recency order: `seq -> key`, ascending, so the least-recently-used entry
    /// is `order`'s first key. An `O(log n)` LRU with no `unsafe`.
    order: BTreeMap<u64, CacheKey>,
    next_seq: u64,
    resident_bytes: u64,
}

/// A `Mutex<LRU>` of decompressed chunk/tile bodies, byte-capped on the
/// decompressed bytes actually held. See the module docs.
pub struct ChunkCache {
    inner: Mutex<CacheInner>,
    cap: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    decoded_chunks: AtomicU64,
    decoded_bytes: AtomicU64,
    evictions: AtomicU64,
}

impl std::fmt::Debug for ChunkCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.stats();
        f.debug_struct("ChunkCache")
            .field("cap", &self.cap.load(Ordering::Relaxed))
            .field("resident_bytes", &s.resident_bytes)
            .field("entries", &s.entries)
            .finish()
    }
}

impl ChunkCache {
    /// A cache with an explicit byte cap.
    pub fn new(cap: u64) -> Self {
        ChunkCache {
            inner: Mutex::new(CacheInner::default()),
            cap: AtomicU64::new(cap),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            decoded_chunks: AtomicU64::new(0),
            decoded_bytes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// An **unlimited** cache — nothing is ever evicted. This is the phase-0
    /// default for every consumer (local opens, ranged opens, query paths,
    /// `--in-memory`, wasm), so residency is identical to the previous design.
    pub fn unlimited() -> Self {
        Self::new(u64::MAX)
    }

    /// Wrap [`unlimited`](Self::unlimited) in an `Arc` — the shape every
    /// consumer stores.
    pub fn unlimited_arc() -> Arc<Self> {
        Arc::new(Self::unlimited())
    }

    /// Change the byte cap and immediately evict down to it (phase ≥ 2).
    pub fn set_cap(&self, cap: u64) {
        self.cap.store(cap, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        self.evict_to_cap(&mut inner);
    }

    /// The current byte cap.
    pub fn cap(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    /// A cached entry, if present, bumping its recency. Records a hit or miss.
    pub fn get(&self, key: CacheKey) -> Option<Arc<CacheEntry>> {
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        if let Some(slot) = inner.map.get_mut(&key) {
            let old = slot.seq;
            slot.seq = seq;
            let entry = Arc::clone(&slot.entry);
            inner.order.remove(&old);
            inner.order.insert(seq, key);
            inner.next_seq += 1;
            drop(inner);
            self.hits.fetch_add(1, Ordering::Relaxed);
            Some(entry)
        } else {
            drop(inner);
            self.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Is `key` currently resident? (No recency bump, no hit/miss accounting —
    /// used by the batch prefetchers to decide what still needs faulting.)
    pub fn contains(&self, key: CacheKey) -> bool {
        self.inner.lock().unwrap().map.contains_key(&key)
    }

    /// Insert `body` under `key` (or, if a racing thread inserted first, keep
    /// theirs), and return the stored entry. The returned `Arc` is valid even if
    /// this same insert immediately evicts the slot (an oversized body under a
    /// tiny cap) — the handout invariant.
    pub fn insert(&self, key: CacheKey, body: Arc<[u8]>) -> Arc<CacheEntry> {
        self.decoded_chunks.fetch_add(1, Ordering::Relaxed);
        self.decoded_bytes
            .fetch_add(body.len() as u64, Ordering::Relaxed);
        let mut inner = self.inner.lock().unwrap();
        let seq = inner.next_seq;
        // Lost the race: someone already stored this key. Keep the existing
        // entry (so all callers observe one body per key) and bump its recency.
        if let Some(slot) = inner.map.get_mut(&key) {
            let old = slot.seq;
            slot.seq = seq;
            let entry = Arc::clone(&slot.entry);
            inner.order.remove(&old);
            inner.order.insert(seq, key);
            inner.next_seq += 1;
            return entry;
        }
        let entry = Arc::new(CacheEntry {
            body,
            runs: OnceLock::new(),
        });
        let charge = entry.charge();
        inner.next_seq += 1;
        inner.map.insert(
            key,
            Slot {
                entry: Arc::clone(&entry),
                seq,
                charge,
            },
        );
        inner.order.insert(seq, key);
        inner.resident_bytes += charge;
        self.evict_to_cap(&mut inner);
        entry
    }

    /// Get `key`, or fault it in through `fetch` (called **off-lock**) and cache
    /// the result. `fetch` returning `None` (a failed decode/fetch) caches
    /// nothing and yields `None`, so a later call retries.
    pub fn get_or_fetch(
        &self,
        key: CacheKey,
        fetch: impl FnOnce() -> Option<Arc<[u8]>>,
    ) -> Option<Arc<CacheEntry>> {
        if let Some(e) = self.get(key) {
            return Some(e);
        }
        let body = fetch()?;
        Some(self.insert(key, body))
    }

    /// Evict least-recently-used entries until the resident total is within the
    /// cap. A no-op while the cap is [`u64::MAX`] (phase 0).
    fn evict_to_cap(&self, inner: &mut CacheInner) {
        let cap = self.cap.load(Ordering::Relaxed);
        while inner.resident_bytes > cap {
            let Some((&seq, &key)) = inner.order.iter().next() else {
                break;
            };
            inner.order.remove(&seq);
            if let Some(slot) = inner.map.remove(&key) {
                inner.resident_bytes -= slot.charge;
                // The `Arc` inside `slot` drops here; any clone already handed
                // out keeps the bytes alive (the handout invariant).
            }
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// A snapshot of the observability counters.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.lock().unwrap();
        CacheStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            decoded_chunks: self.decoded_chunks.load(Ordering::Relaxed),
            decoded_bytes: self.decoded_bytes.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            resident_bytes: inner.resident_bytes,
            entries: inner.map.len() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(n: usize, fill: u8) -> Arc<[u8]> {
        Arc::from(vec![fill; n])
    }

    #[test]
    fn insert_and_get_round_trip() {
        let cache = ChunkCache::unlimited();
        let e = cache.insert((0, 7), body(32, 0xAB));
        assert_eq!(e.body(), &[0xAB; 32]);
        let got = cache.get((0, 7)).expect("resident");
        assert_eq!(got.body(), &[0xAB; 32]);
        assert!(Arc::ptr_eq(&e, &got), "same entry handed out");
        assert!(cache.get((0, 8)).is_none(), "absent key misses");
        let s = cache.stats();
        assert_eq!(s.entries, 1);
        assert_eq!(s.evictions, 0);
    }

    #[test]
    fn unlimited_cap_never_evicts() {
        let cache = ChunkCache::unlimited();
        for i in 0..1000u32 {
            cache.insert((1, i), body(4096, i as u8));
        }
        let s = cache.stats();
        assert_eq!(s.entries, 1000, "every insert stays resident");
        assert_eq!(s.evictions, 0, "unlimited cap evicts nothing");
        for i in 0..1000u32 {
            assert!(cache.contains((1, i)), "chunk {i} still resident");
        }
    }

    #[test]
    fn arc_stays_valid_across_a_forced_eviction() {
        // A cap large enough for exactly one 100-byte body (+overhead).
        let cache = ChunkCache::new(100 + ENTRY_OVERHEAD);
        let first = cache.insert((0, 0), body(100, 1));
        assert!(cache.contains((0, 0)));
        // A second insert pushes the resident total over the cap and evicts the
        // LRU (the first) from the map.
        let _second = cache.insert((0, 1), body(100, 2));
        assert!(!cache.contains((0, 0)), "LRU key evicted from the map");
        assert!(cache.contains((0, 1)), "newest key kept");
        assert!(cache.stats().evictions >= 1, "an eviction happened");
        // The handle we already hold is still valid — eviction only dropped the
        // map slot, not the bytes.
        assert_eq!(
            first.body(),
            &[1u8; 100],
            "handed-out Arc survives eviction"
        );
    }

    #[test]
    fn recency_protects_the_hot_entry() {
        let cache = ChunkCache::new(2 * (100 + ENTRY_OVERHEAD));
        cache.insert((0, 0), body(100, 0));
        cache.insert((0, 1), body(100, 1));
        // Touch key 0 so key 1 becomes the LRU.
        assert!(cache.get((0, 0)).is_some());
        cache.insert((0, 2), body(100, 2)); // over cap -> evict LRU (key 1)
        assert!(cache.contains((0, 0)), "recently used key survives");
        assert!(!cache.contains((0, 1)), "LRU key evicted");
        assert!(cache.contains((0, 2)), "newest key present");
    }

    #[test]
    fn runs_are_derived_once_and_cached() {
        let cache = ChunkCache::unlimited();
        let e = cache.insert((0, 0), body(10, 0));
        let calls = std::cell::Cell::new(0);
        let r1 = e.runs(|_| {
            calls.set(calls.get() + 1);
            vec![0, 4, 8]
        });
        assert_eq!(r1, &[0, 4, 8]);
        let r2 = e.runs(|_| {
            calls.set(calls.get() + 1);
            vec![9, 9, 9]
        });
        assert_eq!(r2, &[0, 4, 8], "cached, not recomputed");
        assert_eq!(calls.get(), 1, "compute ran exactly once");
        // Empty body: never cached, always empty.
        let empty = cache.insert((0, 1), Arc::from(Vec::<u8>::new()));
        assert!(empty.runs(|_| vec![1, 2, 3]).is_empty());
    }

    #[test]
    fn get_or_fetch_faults_once_then_hits() {
        let cache = ChunkCache::unlimited();
        let calls = std::cell::Cell::new(0);
        let e1 = cache
            .get_or_fetch((2, 5), || {
                calls.set(calls.get() + 1);
                Some(body(16, 0x11))
            })
            .unwrap();
        assert_eq!(e1.body(), &[0x11; 16]);
        let e2 = cache
            .get_or_fetch((2, 5), || {
                calls.set(calls.get() + 1);
                Some(body(16, 0x22))
            })
            .unwrap();
        assert_eq!(e2.body(), &[0x11; 16], "second call hits the cache");
        assert_eq!(calls.get(), 1, "fetch ran once");
        // A failed fetch caches nothing.
        assert!(cache.get_or_fetch((2, 6), || None).is_none());
        assert!(!cache.contains((2, 6)));
    }
}
