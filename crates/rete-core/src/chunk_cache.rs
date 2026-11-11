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
//! The cache is `Send + Sync`: the map is split into parts, each behind a
//! `RwLock` (a lookup reads one part and stamps the entry's recency
//! atomically, so lookups neither queue nor share a line they write),
//! and the body decode happens **off-lock** (the caller decodes, then inserts).
//! `Dictionary`/`GraphIndex` are shared across query threads, so this is
//! required; wasm is single-threaded, where the lock is simply uncontended.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// One flat key space over both consumers: the high byte is the section /
/// permutation, the low is the chunk / tile index within it. Two `ChunkCache`
/// instances (one per `Dictionary`, one per `GraphIndex`) keep the two key
/// spaces trivially disjoint.
pub type CacheKey = (u8, u32);

/// Charged per entry on top of its body so a section of many tiny chunks cannot
/// silently overshoot the cap through bookkeeping alone (the `Arc` refcount
/// header, the map node, the recency-order node). A deliberate slight over-count.
const ENTRY_OVERHEAD: u64 = 96;

/// The `Arc<[u8]>` header (strong + weak counts) allocated in front of a body.
const ARC_HEADER: u64 = 16;

/// The bytes an allocation of `n` bytes really occupies: `n` rounded up to the
/// allocator's size class, using jemalloc's spacing (four classes per doubling:
/// a request in `(2^k, 2^(k+1)]` rounds up to a multiple of `2^(k-2)`), and to
/// 16 bytes below 64.
///
/// Charging the request size instead undercounted badly where it matters
/// most: a dictionary chunk is cut at ~64 KiB on a run boundary, so most
/// bodies are a little *over* 64 KiB and land in the 80 KiB class — 20% the
/// cache never saw. On openaire-relation at `--memory-budget-mb 4096` that was
/// ~0.7 GB of resident memory outside the cap. Allocators with finer classes
/// (glibc) waste less than this charges, so the cap errs on the safe side.
pub fn alloc_footprint(n: u64) -> u64 {
    if n <= 64 {
        return n.div_ceil(16).max(1) * 16;
    }
    let k = 63 - (n - 1).leading_zeros() as u64; // 2^k < n <= 2^(k+1)
    let step = 1u64 << (k - 2);
    n.div_ceil(step) * step
}

/// A cached decompressed body plus, for a dictionary chunk, its lazily-derived
/// per-run offset table. Evicted as a unit; handed out behind an `Arc` so it
/// outlives its map slot (see the module docs).
///
/// An index tile's derived group directory lives here too (`dir`), so it is
/// evicted with the tile it indexes: it used to be a per-`Tile` `OnceLock` in
/// the always-resident tile directory, and a probe-heavy query that touched a
/// million tiles kept a million directories — often larger than the tiles —
/// whatever the cache cap. Dictionary chunks never populate `dir`, tiles never
/// populate `runs`; each costs one unset word on the other.
pub struct CacheEntry {
    body: Arc<[u8]>,
    /// A dictionary chunk's per-run byte offsets, relative to `body`. Computed
    /// once on first lookup (via [`runs`](CacheEntry::runs)) and cached here so
    /// it is evicted together with the body it indexes.
    runs: OnceLock<Arc<[usize]>>,
    /// An index tile's a-group directory (see [`group_dir`](CacheEntry::group_dir)).
    dir: OnceLock<crate::triples::GroupDirectory>,
    /// A quad tile's segment index (see [`segments`](CacheEntry::segments));
    /// `None` inside when the tile is malformed.
    segs: OnceLock<Option<Arc<[TileSeg]>>>,
}

/// One segment of a graph-leading quad tile: `(graph id, first a, offset of
/// its body in the tile, length of its body)`.
pub type TileSeg = (u32, u32, u32, u32);

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

    /// This tile's a-group directory, building it with `compute` on first call.
    /// The `bool` is true when this call built it — the caller then charges its
    /// size to the cache ([`ChunkCache::charge_extra`]).
    #[inline]
    pub fn group_dir(
        &self,
        compute: impl FnOnce() -> crate::triples::GroupDirectory,
    ) -> (&crate::triples::GroupDirectory, bool) {
        let mut built = false;
        let d = self.dir.get_or_init(|| {
            built = true;
            compute()
        });
        (d, built)
    }

    /// This quad tile's segments, in graph order, indexing them with `compute`
    /// on first call (`None` from it: a malformed tile, remembered as such).
    /// The `bool` is true when this call built the index — the caller then
    /// charges its size to the cache ([`ChunkCache::charge_extra`]).
    pub fn segments(
        &self,
        compute: impl FnOnce(&[u8]) -> Option<Vec<TileSeg>>,
    ) -> (Option<&[TileSeg]>, bool) {
        let mut built = false;
        let s = self.segs.get_or_init(|| {
            built = true;
            compute(&self.body).map(Arc::from)
        });
        (s.as_deref(), built)
    }

    /// Bytes this entry charges against the cap: what the allocator really
    /// hands out for the body ([`alloc_footprint`]) plus a fixed overhead. Runs
    /// are derived lazily after insertion, so they are not counted (about 1% of
    /// a body).
    #[inline]
    fn charge(&self) -> u64 {
        alloc_footprint(self.body.len() as u64 + ARC_HEADER) + ENTRY_OVERHEAD
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
    /// Recency stamp: the cache's clock when the entry was last taken.
    /// Atomic, so a lookup stamps it under its part's *read* lock.
    seq: AtomicU64,
    charge: u64,
}

/// How many independently locked parts the map is split into.
const PARTS: usize = 64;

/// One part of the map, with its own lock and its own counters: threads
/// looking up different entries seldom touch the same part.
#[derive(Default)]
struct Part {
    map: RwLock<HashMap<CacheKey, Slot>>,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// An LRU of decompressed chunk/tile bodies, byte-capped on the
/// decompressed bytes actually held. See the module docs.
///
/// A lookup writes nothing every thread shares: it read-locks one of
/// [`PARTS`] parts of the map, stamps the entry with the clock (which it only
/// reads; inserts advance it) and counts the hit on that part. Under one
/// lock, or one sequence counter bumped per lookup, sixteen threads decoding
/// scattered terms spent more time passing that cache line around than
/// decoding: a query was slower on sixteen threads than on eight.
pub struct ChunkCache {
    /// This cache's number, unique in the process: what a thread's
    /// [`RECENT`] entries are keyed by (an address could be reused).
    id: u64,
    parts: Box<[Part]>,
    /// The decompressed bytes held, over all parts.
    resident: AtomicU64,
    /// Advanced by every insert; an entry's recency is the clock when it was
    /// last taken, so entries taken between two inserts are equally recent.
    clock: AtomicU64,
    /// One eviction at a time.
    evicting: std::sync::Mutex<()>,
    cap: AtomicU64,
    decoded_chunks: AtomicU64,
    decoded_bytes: AtomicU64,
    evictions: AtomicU64,
}

/// Up to this many entries an eviction stops exactly at the cap; a larger
/// cache evicts a little further ([`ChunkCache::evict_to_cap`]).
const EXACT_EVICTION_ENTRIES: usize = 64;

/// How many entries a thread remembers ([`RECENT`]).
const RECENT_SLOTS: usize = 4;

/// The entries a thread took from any cache last.
struct Recent {
    slots: [Option<(u64, CacheKey, Arc<CacheEntry>)>; RECENT_SLOTS],
    next: usize,
}

thread_local! {
    /// The last few entries this thread got, of any cache: a `get` of one of
    /// them is answered here, without the cache's lock.
    ///
    /// A lookup per row lands in the tile the row before it did, and the lock
    /// is one per cache. A query on sixteen threads spent a quarter of its
    /// time queueing for it. An entry answered from here keeps its body alive
    /// if the cache evicts it meanwhile (a few bodies per thread, not charged
    /// to the cap) and does not count as a use for the eviction order: it is
    /// counted when the thread first takes it and again when it comes back to
    /// it. The bytes under a key never change, so a remembered entry is never
    /// stale.
    static RECENT: std::cell::RefCell<Recent> = const {
        std::cell::RefCell::new(Recent {
            slots: [None, None, None, None],
            next: 0,
        })
    };
}

static NEXT_CACHE_ID: AtomicU64 = AtomicU64::new(1);

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
            id: NEXT_CACHE_ID.fetch_add(1, Ordering::Relaxed),
            parts: (0..PARTS).map(|_| Part::default()).collect(),
            resident: AtomicU64::new(0),
            clock: AtomicU64::new(0),
            evicting: std::sync::Mutex::new(()),
            cap: AtomicU64::new(cap),
            decoded_chunks: AtomicU64::new(0),
            decoded_bytes: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
        }
    }

    /// The part of the map `key` lives in.
    fn part(&self, key: CacheKey) -> &Part {
        // Consecutive chunks of one section land in different parts.
        let h = (key.1 as usize).wrapping_mul(0x9E37_79B1) ^ ((key.0 as usize) << 7);
        &self.parts[(h >> 8) % PARTS]
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
        self.evict_to_cap();
    }

    /// The current byte cap.
    pub fn cap(&self) -> u64 {
        self.cap.load(Ordering::Relaxed)
    }

    /// A cached entry, if present, bumping its recency. Records a hit or miss.
    pub fn get(&self, key: CacheKey) -> Option<Arc<CacheEntry>> {
        let recent = RECENT.with(|r| {
            let r = r.try_borrow().ok()?;
            r.slots
                .iter()
                .flatten()
                .find(|(id, k, _)| *id == self.id && *k == key)
                .map(|(_, _, e)| Arc::clone(e))
        });
        if recent.is_some() {
            return recent;
        }
        let found = self.get_shared(key);
        if let Some(e) = &found {
            RECENT.with(|r| {
                if let Ok(mut r) = r.try_borrow_mut() {
                    let at = r.next;
                    r.slots[at] = Some((self.id, key, Arc::clone(e)));
                    r.next = (at + 1) % RECENT_SLOTS;
                }
            });
        }
        found
    }

    /// [`get`](Self::get) from the shared map, under its part's read lock.
    fn get_shared(&self, key: CacheKey) -> Option<Arc<CacheEntry>> {
        let part = self.part(key);
        let found = {
            let map = part.map.read().unwrap();
            map.get(&key).map(|slot| {
                // Newer than every insert so far, as recent as the next one.
                let now = self.clock.load(Ordering::Relaxed) + 1;
                if slot.seq.load(Ordering::Relaxed) != now {
                    slot.seq.store(now, Ordering::Relaxed);
                }
                Arc::clone(&slot.entry)
            })
        };
        match &found {
            Some(_) => part.hits.fetch_add(1, Ordering::Relaxed),
            None => part.misses.fetch_add(1, Ordering::Relaxed),
        };
        found
    }

    /// Is `key` currently resident? (No recency bump, no hit/miss accounting —
    /// used by the batch prefetchers to decide what still needs faulting.)
    pub fn contains(&self, key: CacheKey) -> bool {
        self.part(key).map.read().unwrap().contains_key(&key)
    }

    /// Insert `body` under `key` (or, if a racing thread inserted first, keep
    /// theirs), and return the stored entry. The returned `Arc` is valid even if
    /// this same insert immediately evicts the slot (an oversized body under a
    /// tiny cap) — the handout invariant.
    pub fn insert(&self, key: CacheKey, body: Arc<[u8]>) -> Arc<CacheEntry> {
        self.decoded_chunks.fetch_add(1, Ordering::Relaxed);
        self.decoded_bytes
            .fetch_add(body.len() as u64, Ordering::Relaxed);
        let seq = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        let entry = {
            let mut map = self.part(key).map.write().unwrap();
            // Lost the race: someone already stored this key. Keep the existing
            // entry (so all callers observe one body per key) and bump its recency.
            if let Some(slot) = map.get(&key) {
                slot.seq.store(seq, Ordering::Relaxed);
                return Arc::clone(&slot.entry);
            }
            let entry = Arc::new(CacheEntry {
                body,
                runs: OnceLock::new(),
                dir: OnceLock::new(),
                segs: OnceLock::new(),
            });
            let charge = entry.charge();
            map.insert(
                key,
                Slot {
                    entry: Arc::clone(&entry),
                    seq: AtomicU64::new(seq),
                    charge,
                },
            );
            self.resident.fetch_add(charge, Ordering::Relaxed);
            entry
        };
        self.evict_to_cap();
        entry
    }

    /// Charge `extra` more bytes to `key`'s slot — for state derived from a
    /// body after it was inserted (a tile's group directory) — then evict down
    /// to the cap. A no-op if the slot no longer holds `entry` (it was evicted,
    /// or replaced): that state then dies with the last handle to it.
    pub fn charge_extra(&self, key: CacheKey, entry: &Arc<CacheEntry>, extra: u64) {
        {
            let mut map = self.part(key).map.write().unwrap();
            let Some(slot) = map.get_mut(&key) else {
                return;
            };
            if !Arc::ptr_eq(&slot.entry, entry) {
                return;
            }
            slot.charge += extra;
            self.resident.fetch_add(extra, Ordering::Relaxed);
        }
        self.evict_to_cap();
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
    ///
    /// No recency order is kept between evictions (a lookup only stamps its
    /// entry), so an eviction reads every part's stamps and sorts them. A
    /// large cache then drops a thirty-second of its cap at once, so that a
    /// run of inserts at the cap does not sort for each one; a small one (the
    /// caps tests set) evicts exactly down to the cap.
    fn evict_to_cap(&self) {
        let cap = self.cap.load(Ordering::Relaxed);
        if self.resident.load(Ordering::Relaxed) <= cap {
            return;
        }
        let _one = self.evicting.lock().unwrap();
        if self.resident.load(Ordering::Relaxed) <= cap {
            return;
        }
        let mut by_age: Vec<(u64, CacheKey)> = Vec::new();
        for part in self.parts.iter() {
            let map = part.map.read().unwrap();
            by_age.extend(
                map.iter()
                    .map(|(k, slot)| (slot.seq.load(Ordering::Relaxed), *k)),
            );
        }
        let target = if by_age.len() > EXACT_EVICTION_ENTRIES {
            cap - cap / 32
        } else {
            cap
        };
        by_age.sort_unstable();
        for (_, key) in by_age {
            if self.resident.load(Ordering::Relaxed) <= target {
                break;
            }
            if let Some(slot) = self.part(key).map.write().unwrap().remove(&key) {
                self.resident.fetch_sub(slot.charge, Ordering::Relaxed);
                // The `Arc` inside `slot` drops here; any clone already handed
                // out keeps the bytes alive (the handout invariant).
                self.evictions.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// A snapshot of the observability counters.
    pub fn stats(&self) -> CacheStats {
        let (mut hits, mut misses, mut entries) = (0u64, 0u64, 0u64);
        for part in self.parts.iter() {
            hits += part.hits.load(Ordering::Relaxed);
            misses += part.misses.load(Ordering::Relaxed);
            entries += part.map.read().unwrap().len() as u64;
        }
        CacheStats {
            hits,
            misses,
            decoded_chunks: self.decoded_chunks.load(Ordering::Relaxed),
            decoded_bytes: self.decoded_bytes.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            resident_bytes: self.resident.load(Ordering::Relaxed),
            entries,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(n: usize, fill: u8) -> Arc<[u8]> {
        Arc::from(vec![fill; n])
    }

    fn charge_of(n: u64) -> u64 {
        alloc_footprint(n + ARC_HEADER) + ENTRY_OVERHEAD
    }

    #[test]
    fn derived_state_is_charged_and_evicted_with_its_body() {
        let one = charge_of(1000);
        let cache = ChunkCache::new(2 * one + 10);
        let a = cache.insert((0, 1), body(1000, 1));
        let b = cache.insert((0, 2), body(1000, 2));
        assert_eq!(cache.stats().resident_bytes, 2 * one);
        // Building `b`'s directory and charging it pushes the total over the
        // cap: the least-recently-used entry (`a`) goes.
        let (dir, built) = b.group_dir(crate::triples::GroupDirectory::empty_for_tests);
        assert!(built && dir.is_empty());
        let (_, again) = b.group_dir(|| unreachable!("built once"));
        assert!(!again);
        cache.charge_extra((0, 2), &b, 100);
        let st = cache.stats();
        assert_eq!(st.entries, 1);
        assert_eq!(st.resident_bytes, one + 100);
        assert!(cache.get((0, 1)).is_none() && cache.get((0, 2)).is_some());
        // A handle whose slot is gone (or holds another entry) charges nothing.
        cache.charge_extra((0, 1), &a, 1 << 20);
        cache.charge_extra((0, 2), &a, 1 << 20);
        assert_eq!(cache.stats().resident_bytes, one + 100);
    }

    /// An entry this thread took stays in its hands: asked for again it is
    /// answered without the map, also after the map dropped it, and never for
    /// another cache that uses the same key.
    #[test]
    fn a_thread_keeps_the_entries_it_took_last() {
        let cache = ChunkCache::new(charge_of(100));
        let other = ChunkCache::unlimited();
        cache.insert((0, 1), body(100, 1));
        let first = cache.get((0, 1)).expect("resident");
        // The second insert evicts the first from the map.
        cache.insert((0, 2), body(100, 2));
        assert!(!cache.contains((0, 1)));
        let again = cache.get((0, 1)).expect("remembered by this thread");
        assert!(Arc::ptr_eq(&first, &again));
        assert!(other.get((0, 1)).is_none(), "another cache, the same key");
        // Another thread has not taken it, and the map no longer has it.
        std::thread::scope(|s| {
            s.spawn(|| assert!(cache.get((0, 1)).is_none()));
        });
        // More entries than the thread remembers push the oldest out.
        let wide = ChunkCache::unlimited();
        for i in 0..(RECENT_SLOTS as u32 + 1) {
            wide.insert((1, i), body(8, i as u8));
            assert!(wide.get((1, i)).is_some());
        }
        assert!(cache.get((0, 1)).is_none(), "pushed out by later entries");
    }

    #[test]
    fn footprint_follows_four_classes_per_doubling() {
        assert_eq!(alloc_footprint(1), 16);
        assert_eq!(alloc_footprint(64), 64);
        assert_eq!(alloc_footprint(65), 80);
        assert_eq!(alloc_footprint(65_536), 65_536);
        // A chunk body just over 64 KiB is charged its 80 KiB class.
        assert_eq!(alloc_footprint(65_537), 81_920);
        assert_eq!(alloc_footprint(100_000), 114_688);
        for n in [65u64, 1000, 70_000, 1 << 20, 5_000_001] {
            let c = alloc_footprint(n);
            assert!(c >= n && c - n <= n / 4, "{n} -> {c}");
        }
    }

    #[test]
    fn charge_is_the_allocation_class_not_the_length() {
        let cache = ChunkCache::unlimited();
        cache.insert((0, 0), body(65_600, 1));
        assert_eq!(cache.stats().resident_bytes, charge_of(65_600));
        assert_eq!(charge_of(65_600), 81_920 + ENTRY_OVERHEAD);
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
        let cache = ChunkCache::new(charge_of(100));
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
        let cache = ChunkCache::new(2 * charge_of(100));
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
