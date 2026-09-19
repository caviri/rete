//! Front-coded, restart-indexed dictionary sections (SPEC.md §5.1).
//!
//! A section holds the UTF-8 terms of one kind (shared / subjects / objects /
//! predicates / graphs), sorted and assigned dense 1-based IDs. Terms are stored
//! in runs of `R`; each run starts with a full term and front-codes the rest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::chunk_cache::{CacheEntry, ChunkCache};
use crate::varint::{read_uvarint, write_uvarint};

/// Default restart interval: a full term every `R` entries.
pub const DEFAULT_RESTART_INTERVAL: u32 = 16;

/// Reserved ID meaning "no such term".
pub const ABSENT: u32 = 0;

#[derive(Debug, thiserror::Error)]
pub enum DictError {
    #[error("malformed dictionary section: {0}")]
    Malformed(&'static str),
}

/// Build a dictionary section from terms (any order; sorted + deduped here).
#[derive(Default)]
pub struct DictSectionBuilder {
    terms: Vec<String>,
    restart_interval: u32,
}

/// Terms per restart run, overridable via `RETE_DICT_RESTART_INTERVAL`.
///
/// The interval sets random-access granularity as well as front-coding gain:
/// chunks are cut on whole-run boundaries, so with very large literals (e.g.
/// base64-embedded images) a single run dwarfs the 64 KiB chunk budget and one
/// term lookup drags in ~`r` terms' worth of bytes. Setting `1` makes every term
/// its own restart — direct seek, larger restart table, no front-coding (which
/// is worthless for base64 anyway, since such terms share no prefix).
pub fn env_restart_interval() -> u32 {
    static R: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *R.get_or_init(|| {
        std::env::var("RETE_DICT_RESTART_INTERVAL")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&v| v >= 1)
            .unwrap_or(DEFAULT_RESTART_INTERVAL)
    })
}

impl DictSectionBuilder {
    pub fn new() -> Self {
        Self {
            terms: Vec::new(),
            restart_interval: env_restart_interval(),
        }
    }

    pub fn with_restart_interval(mut self, r: u32) -> Self {
        assert!(r >= 1, "restart interval must be >= 1");
        self.restart_interval = r;
        self
    }

    pub fn push(&mut self, term: impl Into<String>) {
        self.terms.push(term.into());
    }

    /// Serialize to bytes. Terms are sorted and deduped; the resulting IDs are
    /// `1..=n` in sorted order.
    pub fn build(mut self) -> Vec<u8> {
        self.terms.sort_unstable();
        self.terms.dedup();
        let r = self.restart_interval as usize;
        let n = self.terms.len();
        let num_restarts = n.div_ceil(r);

        // Encode body and capture each run's starting offset (relative to body).
        let mut body = Vec::new();
        let mut restart_offsets = Vec::with_capacity(num_restarts);
        let mut prev = "";
        for (i, term) in self.terms.iter().enumerate() {
            if i % r == 0 {
                restart_offsets.push(body.len() as u64);
                // restart entry: shared = 0, full term
                write_uvarint(&mut body, 0);
                write_uvarint(&mut body, term.len() as u64);
                body.extend_from_slice(term.as_bytes());
            } else {
                let shared = common_prefix_len(prev, term);
                let suffix = &term.as_bytes()[shared..];
                write_uvarint(&mut body, shared as u64);
                write_uvarint(&mut body, suffix.len() as u64);
                body.extend_from_slice(suffix);
            }
            prev = term;
        }

        // header || restart-offset table || body
        let mut out = Vec::new();
        write_uvarint(&mut out, n as u64);
        write_uvarint(&mut out, self.restart_interval as u64);
        write_uvarint(&mut out, num_restarts as u64);
        for off in &restart_offsets {
            write_uvarint(&mut out, *off);
        }
        out.extend_from_slice(&body);
        out
    }
}

/// Parsed section metadata (the restart table) — cache this once and reuse it
/// across lookups instead of re-parsing the section header every time.
#[derive(Debug, Clone)]
pub struct SectionMeta {
    pub term_count: u32,
    pub restart_interval: u32,
    /// Absolute offsets into the section bytes for each run start.
    ///
    /// `u64`, not `usize`: a section may exceed 4 GiB and wasm32 is a 32-bit
    /// target, so a `usize` offset silently truncates there (see #70). A
    /// dictionary carrying embedded media is exactly this case — WikiArt's is
    /// 23.4 GB.
    pub restart_offsets: Vec<u64>,
}

/// Parse only the header/restart table of a section.
pub fn parse_meta(bytes: &[u8]) -> Result<SectionMeta, DictError> {
    let mut pos = 0;
    let take = |pos: &mut usize| -> Result<u64, DictError> {
        let (v, n) =
            read_uvarint(&bytes[*pos..]).ok_or(DictError::Malformed("truncated header"))?;
        *pos += n;
        Ok(v)
    };
    let term_count = take(&mut pos)? as u32;
    let restart_interval = take(&mut pos)? as u32;
    let num_restarts = take(&mut pos)? as usize;
    if restart_interval == 0 {
        return Err(DictError::Malformed("zero restart interval"));
    }
    // `num_restarts` is untrusted; each restart is ≥1 byte, so cap the
    // pre-allocation at the buffer length to avoid an OOM on a bogus count.
    let mut rel = Vec::with_capacity(num_restarts.min(bytes.len()));
    for _ in 0..num_restarts {
        rel.push(take(&mut pos)?);
    }
    let body_start = pos as u64;
    Ok(SectionMeta {
        term_count,
        restart_interval,
        restart_offsets: rel.into_iter().map(|o| body_start + o).collect(),
    })
}

/// Decode the front-coded entry at `pos` *into* `buf`: truncate `buf` to the
/// entry's shared-prefix length and append its suffix bytes. For a restart
/// entry the stored shared length is 0, so the same decode works for both
/// entry kinds. Returns the next entry's position; `None` on malformed bytes
/// (including a shared length longer than the previous term). Allocation-free
/// after `buf`'s first growth — this is the hot path of every term resolution.
#[inline]
fn entry_into(bytes: &[u8], pos: usize, buf: &mut Vec<u8>) -> Option<usize> {
    let (shared, n1) = read_uvarint(bytes.get(pos..)?)?;
    let p = pos + n1;
    let (suf, n2) = read_uvarint(bytes.get(p..)?)?;
    let start = p + n2;
    let end = start
        .checked_add(suf as usize)
        .filter(|&e| e <= bytes.len())?;
    if shared as usize > buf.len() {
        return None;
    }
    buf.truncate(shared as usize);
    buf.extend_from_slice(&bytes[start..end]);
    Some(end)
}

/// Decode a run's restart (full-term) entry at `off` into `buf`.
fn run_entry_into(bytes: &[u8], off: usize, buf: &mut Vec<u8>) -> Option<usize> {
    buf.clear(); // a restart entry stands alone; stale prefix bytes must not leak
    entry_into(bytes, off, buf)
}

/// Resolve `id` (1-based) to its term using cached metadata. Returns `None` on
/// any inconsistency in untrusted metadata/bytes rather than panicking.
pub fn section_term(bytes: &[u8], meta: &SectionMeta, id: u32) -> Option<String> {
    if id == ABSENT || id > meta.term_count {
        return None;
    }
    let idx = (id - 1) as usize;
    let run = idx / meta.restart_interval as usize;
    let steps = idx % meta.restart_interval as usize;
    let mut buf = Vec::new();
    let mut pos = run_entry_into(bytes, *meta.restart_offsets.get(run)? as usize, &mut buf)?;
    for _ in 0..steps {
        pos = entry_into(bytes, pos, &mut buf)?;
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Resolve `term` to its ID using cached metadata.
pub fn section_id(bytes: &[u8], meta: &SectionMeta, term: &str) -> Option<u32> {
    let mut buf = Vec::new();
    // Binary search restart runs by their first (full) term.
    let mut lo = 0usize;
    let mut hi = meta.restart_offsets.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        run_entry_into(bytes, meta.restart_offsets[mid] as usize, &mut buf)?;
        if buf.as_slice() <= term.as_bytes() {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return None; // smaller than every term
    }
    let run = lo - 1;
    let mut pos = run_entry_into(bytes, meta.restart_offsets[run] as usize, &mut buf)?;
    let base_id = (run * meta.restart_interval as usize) as u32 + 1;
    // saturating_sub: corrupt metadata where run*interval > term_count must not
    // underflow-panic.
    let run_len = meta.restart_interval.min(
        meta.term_count
            .saturating_sub(run as u32 * meta.restart_interval),
    );
    for step in 0..run_len {
        if buf.as_slice() == term.as_bytes() {
            return Some(base_id + step);
        }
        if buf.as_slice() > term.as_bytes() {
            return None;
        }
        if step + 1 < run_len {
            pos = entry_into(bytes, pos, &mut buf)?;
        }
    }
    None
}

// --- chunked sections ---------------------------------------------------------
//
// A section's body is split (on run boundaries) into **chunks** that are
// compressed and fetched independently (format v0.2), so a remote client
// resolves a term with one chunk fault instead of downloading the whole
// section. Local sections are the degenerate case: one pre-set chunk holding
// the entire serialized section — a single code path serves both.

/// Fetches one chunk's decompressed body slice on demand (`None` = the fetch
/// failed; the section records it and the lookup misses — callers over remote
/// data must check [`ChunkedSection::load_incomplete`] after evaluating).
pub type ChunkLoader = Box<dyn Fn(usize) -> Option<Vec<u8>> + Send + Sync>;

/// Fetches **many** chunks in one round trip: given ascending chunk indices,
/// returns each chunk's decompressed body in the same order. The ranged
/// reader implements this by coalescing byte-adjacent chunk ranges into
/// single range reads — a full-dictionary sweep (export, dump) costs a few
/// requests per section instead of one per chunk. `None` = the batch failed;
/// callers fall back to the per-chunk [`ChunkLoader`].
pub type ChunkBulkLoader = Box<dyn Fn(&[usize]) -> Option<Vec<Vec<u8>>> + Send + Sync>;

/// One chunk: a run-aligned slice of the section body. `body_start` is the
/// offset (in the section's coordinate space — the same space
/// [`SectionMeta::restart_offsets`] uses) where `data[0]` sits.
pub struct SectionChunk {
    first_run: usize,
    /// The chunk's **routing key** — the only thing the chunk-level binary
    /// search in [`ChunkedSection::id`] compares against. It is a *separator*,
    /// **not a term**:
    ///
    /// ```text
    /// last_term(chunk i-1)  <  key(i)  <=  first_term(chunk i)
    /// ```
    ///
    /// and chunk 0's key is empty (`b"" <= anything`, and nothing routes before
    /// chunk 0 anyway). Writers store the *shortest* such string — see
    /// [`shortest_separator`] — which is why a file's chunk directory is a few
    /// bytes per chunk instead of a full copy of every boundary term.
    ///
    /// Files written before that change carry the boundary term verbatim; the
    /// verbatim first term is the degenerate separator, so both read the same
    /// way and no version check is needed. Nothing may reconstruct a term from
    /// this field, compare it for equality with a term, or report it as one: a
    /// key that is *not* a separator (a truncation, a hash) mis-routes silently
    /// — `term(id)` and every dump keep working because they route by
    /// `first_run`, and only `id(term)` lies.
    ///
    /// Unused (empty) for the single local chunk.
    key: Vec<u8>,
    body_start: u64,
}

impl SectionChunk {
    /// A chunk descriptor: routing metadata only. The decompressed body (and
    /// its derived run-offset table) lives in the section's [`ChunkCache`],
    /// keyed by `(section_index, chunk_index)`, faulted in on first touch and
    /// evictable — not held here. `key` is the routing separator, **not a
    /// term** (see the `key` field).
    pub fn new(first_run: usize, key: Vec<u8>, body_start: u64) -> Self {
        SectionChunk {
            first_run,
            key,
            body_start,
        }
    }
}

/// Scan a chunk's decompressed body for the byte offset of each run start (see
/// [`SectionChunk::run_offsets`]). O(terms-in-chunk), run once per faulted chunk.
fn chunk_run_offsets(data: &[u8], restart_interval: usize) -> Vec<usize> {
    let mut offs = vec![0usize];
    let mut pos = 0usize;
    let mut buf = Vec::new();
    let mut count = 0usize;
    while pos < data.len() {
        let Some(next) = entry_into(data, pos, &mut buf) else {
            break;
        };
        count += 1;
        pos = next;
        if pos < data.len() && count.is_multiple_of(restart_interval) {
            offs.push(pos);
        }
    }
    offs
}

/// The first (full) term of the run starting at `off`, as raw bytes. `None`
/// on malformed bytes.
pub fn run_first_term(bytes: &[u8], off: usize) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    run_entry_into(bytes, off, &mut buf)?;
    Some(buf)
}

/// The LAST term encoded in `bytes[run_off..end]`, where `run_off` is a run
/// (restart) boundary and `end` is the end of that run's chunk. Walks the
/// front-coded entries forward, so it decodes at most `restart_interval` of
/// them. `None` on malformed bytes.
///
/// Writers use it for one thing: the previous chunk's last term, the lower
/// bound of the next chunk's [`shortest_separator`].
pub fn run_last_term(bytes: &[u8], run_off: usize, end: usize) -> Option<Vec<u8>> {
    let lim = bytes.get(..end)?;
    let mut buf = Vec::new();
    let mut pos = run_entry_into(lim, run_off, &mut buf)?;
    while pos < end {
        pos = entry_into(lim, pos, &mut buf)?;
    }
    Some(buf)
}

/// The **shortest** byte string `s` with `prev_last < s <= first`: `first`
/// truncated one byte past where it first differs from `prev_last`.
///
/// This is the routing key a chunk directory stores (`SectionChunk::key`).
/// `first` is the chunk's first term and `prev_last`
/// the previous chunk's last term, so `prev_last < first` holds for any sorted
/// section; if it does not (malformed input), the full `first` is returned —
/// always a valid separator, just not a short one.
///
/// It really is the shortest. Let `k` be the length of the common prefix of
/// `prev_last` and `first`; the result has length `k + 1`. No shorter `s`
/// exists: any `s` with `|s| <= k` shares that prefix's bytes with both, so it
/// is either a prefix of `prev_last` (hence `<= prev_last`), or it first
/// differs from the shared prefix — below it, making `s < prev_last`, or above
/// it, making `s > first`. Every case is outside `(prev_last, first]`.
pub fn shortest_separator(prev_last: &[u8], first: &[u8]) -> Vec<u8> {
    let shared = prev_last
        .iter()
        .zip(first)
        .take_while(|(a, b)| a == b)
        .count();
    if shared >= first.len() {
        // `first` is a prefix of `prev_last`, i.e. `first <= prev_last`: not a
        // sorted pair. Keep the whole term.
        return first.to_vec();
    }
    first[..shared + 1].to_vec()
}

/// A dictionary section whose body is served chunk-by-chunk: metadata + chunk
/// directory always present, chunk bytes local or faulted in on first touch.
pub struct ChunkedSection {
    meta: SectionMeta,
    chunks: Vec<SectionChunk>,
    loader: Option<ChunkLoader>,
    bulk: Option<ChunkBulkLoader>,
    failed: AtomicBool,
    /// Decompressed chunk bodies (and their derived run tables) live here, keyed
    /// by `(section_index, chunk_index)`, faulted in on first touch and
    /// evictable. Shared across a [`Dictionary`]'s four sections. Phase 0 caps
    /// it at unlimited, so residency is identical to the old per-chunk
    /// `OnceLock`s.
    cache: Arc<ChunkCache>,
    /// This section's discriminant in the shared cache's key space (0 shared,
    /// 1 subject-only, 2 object-only, 3 predicates).
    section_index: u8,
}

impl ChunkedSection {
    /// A local section: the whole serialized section (header + body) as one
    /// resident chunk at coordinate 0, so the absolute restart offsets index it
    /// directly. Malformed bytes degrade to an empty section (no panics on
    /// untrusted files), matching the previous reader behavior. The body is
    /// stored in `cache` at `(section_index, 0)` (unlimited cap ⇒ resident for
    /// the section's lifetime, exactly as before).
    pub fn local(section_bytes: Vec<u8>, cache: Arc<ChunkCache>, section_index: u8) -> Self {
        let meta = parse_meta(&section_bytes).unwrap_or(SectionMeta {
            term_count: 0,
            restart_interval: 1,
            restart_offsets: Vec::new(),
        });
        cache.insert((section_index, 0), Arc::from(section_bytes));
        ChunkedSection {
            meta,
            chunks: vec![SectionChunk::new(0, Vec::new(), 0)],
            loader: None,
            bulk: None,
            failed: AtomicBool::new(false),
            cache,
            section_index,
        }
    }

    /// A section from parsed parts: metadata + chunk list, with an optional
    /// loader for non-resident chunks (the remote lazy-open path) — resident
    /// chunk lists (a locally-decoded chunked section) supply their bodies
    /// through `resident` instead.
    pub fn from_parts(
        meta: SectionMeta,
        chunks: Vec<SectionChunk>,
        loader: Option<ChunkLoader>,
        cache: Arc<ChunkCache>,
        section_index: u8,
    ) -> Self {
        ChunkedSection {
            meta,
            chunks,
            loader,
            bulk: None,
            failed: AtomicBool::new(false),
            cache,
            section_index,
        }
    }

    /// A section whose chunk bodies are already decoded (the local chunked-file
    /// open path): the descriptors and their bodies arrive together and the
    /// bodies are seeded into the cache. No loader — every chunk is resident.
    pub fn resident(
        meta: SectionMeta,
        chunks: Vec<(SectionChunk, Vec<u8>)>,
        cache: Arc<ChunkCache>,
        section_index: u8,
    ) -> Self {
        let descriptors = chunks
            .into_iter()
            .enumerate()
            .map(|(ci, (chunk, body))| {
                cache.insert((section_index, ci as u32), Arc::from(body));
                chunk
            })
            .collect();
        ChunkedSection {
            meta,
            chunks: descriptors,
            loader: None,
            bulk: None,
            failed: AtomicBool::new(false),
            cache,
            section_index,
        }
    }

    /// Attach a batched chunk fetcher (see [`ChunkBulkLoader`]): full-section
    /// sweeps ([`prefetch_all`](Self::prefetch_all)) go through it instead of
    /// faulting chunk by chunk.
    pub fn with_bulk_loader(mut self, bulk: ChunkBulkLoader) -> Self {
        self.bulk = Some(bulk);
        self
    }

    /// Batch-fault every unloaded chunk through the bulk loader, if one is
    /// attached and at least two chunks are missing. Callers about to sweep
    /// the whole section (export/dump term resolution) call this once; a
    /// failed batch leaves the chunks unloaded for the per-chunk loader to
    /// retry (and record failures) lookup by lookup.
    pub fn prefetch_all(&self) {
        self.prefetch_chunks(&(0..self.chunks.len()).collect::<Vec<_>>());
    }

    /// Batch-fault a *specific* set of chunks (the subset a bounded query's
    /// output touches) through the bulk loader, coalescing byte-adjacent ranges
    /// into single reads. `cis` should be ascending and deduped; chunks already
    /// resident are skipped. Like [`prefetch_all`](Self::prefetch_all) a single
    /// missing chunk is left for the per-chunk loader, and a failed batch leaves
    /// the chunks unloaded for that loader to retry (and record failures).
    pub fn prefetch_chunks(&self, cis: &[usize]) {
        let Some(bulk) = &self.bulk else { return };
        let missing: Vec<usize> = cis
            .iter()
            .copied()
            .filter(|&ci| {
                ci < self.chunks.len() && !self.cache.contains((self.section_index, ci as u32))
            })
            .collect();
        if missing.len() < 2 {
            return;
        }
        if let Some(bodies) = bulk(&missing) {
            if bodies.len() == missing.len() {
                for (&ci, body) in missing.iter().zip(bodies) {
                    self.cache
                        .insert((self.section_index, ci as u32), Arc::from(body));
                }
            }
        }
    }

    pub fn meta(&self) -> &SectionMeta {
        &self.meta
    }

    pub fn term_count(&self) -> u32 {
        self.meta.term_count
    }

    /// Did any chunk fetch fail since this section was opened — or since the
    /// last [`reset_load_failure`](Self::reset_load_failure)?
    pub fn load_incomplete(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Forget recorded fetch failures (see `GraphIndex::reset_load_failure` —
    /// the per-query reset for resident sessions). Failed chunks were never
    /// cached, so the next resolution retries them.
    pub fn reset_load_failure(&self) {
        self.failed.store(false, Ordering::Relaxed);
    }

    /// The cache entry for chunk `ci`, faulting its body in through the loader
    /// on a miss. Returns an owned [`CacheEntry`] handle (an `Arc`) the caller
    /// holds for the duration of one decode — valid even if a later eviction
    /// drops the map slot (the handout invariant).
    ///
    /// A FAILED fetch records the failure and caches nothing (returns `None`),
    /// so a later resolution retries the chunk — a transient network error must
    /// not permanently poison a resident session. A local section with no
    /// loader returns an empty body for a missing chunk, matching the old
    /// `get_or_init(Vec::new)` behavior.
    fn chunk_entry(&self, ci: usize) -> Option<Arc<CacheEntry>> {
        let key = (self.section_index, ci as u32);
        if let Some(e) = self.cache.get(key) {
            return Some(e);
        }
        match &self.loader {
            Some(load) => match load(ci) {
                Some(bytes) => Some(self.cache.insert(key, Arc::from(bytes))),
                None => {
                    self.failed.store(true, Ordering::Relaxed);
                    None
                }
            },
            None => Some(self.cache.insert(key, Arc::from(Vec::new()))),
        }
    }

    /// The chunk holding `run` (chunks ascend by `first_run`; the first chunk
    /// always starts at run 0).
    fn chunk_of_run(&self, run: usize) -> Option<usize> {
        let i = self.chunks.partition_point(|c| c.first_run <= run);
        i.checked_sub(1)
    }

    /// The chunk index holding term `id` (1-based, section-local), or `None` if
    /// out of range. Mirrors the run→chunk math in [`term`](Self::term) so a
    /// caller can group a set of output ids by chunk and batch-prefetch them.
    pub fn chunk_of_id(&self, id: u32) -> Option<usize> {
        if id == ABSENT || id > self.meta.term_count {
            return None;
        }
        let run = (id - 1) as usize / self.meta.restart_interval as usize;
        self.chunk_of_run(run)
    }

    /// The byte offset (into chunk `ci`'s decompressed body) of `run`.
    /// Full/local sections use the section-wide restart table (unchanged
    /// behavior); a *lite* remote section (empty `restart_offsets`) derives it
    /// from the chunk itself — that per-chunk run table is cached inside the
    /// `entry` (evicted with the body), so the open never holds the whole table.
    fn run_off_in_chunk(
        &self,
        ci: usize,
        run: usize,
        entry: &CacheEntry,
        ri: usize,
    ) -> Option<usize> {
        let chunk = &self.chunks[ci];
        if self.meta.restart_offsets.is_empty() {
            entry
                .runs(|b| chunk_run_offsets(b, ri))
                .get(run.checked_sub(chunk.first_run)?)
                .copied()
        } else {
            self.meta
                .restart_offsets
                .get(run)?
                .checked_sub(chunk.body_start)
                .map(|o| o as usize)
        }
    }

    /// One past the last run index held by chunk `ci` (its run range is
    /// `[first_run, run_end)`).
    fn run_end_of_chunk(&self, ci: usize, entry: &CacheEntry, ri: usize) -> usize {
        if let Some(next) = self.chunks.get(ci + 1) {
            return next.first_run;
        }
        let chunk = &self.chunks[ci];
        if self.meta.restart_offsets.is_empty() {
            chunk.first_run + entry.runs(|b| chunk_run_offsets(b, ri)).len()
        } else {
            self.meta.restart_offsets.len()
        }
    }

    /// Resolve `id` (1-based) to its term. One chunk fault at most.
    /// Chunk index holding term `id` — [`term`](Self::term)'s routing without
    /// faulting the chunk. `None` for an absent/out-of-range id.
    pub(crate) fn chunk_of_term(&self, id: u32) -> Option<usize> {
        if id == ABSENT || id > self.meta.term_count {
            return None;
        }
        self.chunk_of_run((id - 1) as usize / self.meta.restart_interval as usize)
    }

    pub fn term(&self, id: u32) -> Option<String> {
        if id == ABSENT || id > self.meta.term_count {
            return None;
        }
        let idx = (id - 1) as usize;
        let ri = self.meta.restart_interval as usize;
        let run = idx / ri;
        let steps = idx % ri;
        let ci = self.chunk_of_run(run)?;
        let entry = self.chunk_entry(ci)?;
        let bytes: &[u8] = entry.body();
        let off = self.run_off_in_chunk(ci, run, &entry, ri)?;
        let mut buf = Vec::new();
        let mut pos = run_entry_into(bytes, off, &mut buf)?;
        for _ in 0..steps {
            pos = entry_into(bytes, pos, &mut buf)?;
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Resolve many ids of THIS section in one chunk-ordered, single-pass walk,
    /// filling `out[slot]` for each `(id, slot)` in `jobs`.
    ///
    /// `jobs` is sorted in place by id. Because chunks and runs are *contiguous
    /// ascending* id ranges, sorting by id also groups the jobs by chunk and by
    /// run — so each chunk is faulted **once** and each run walked **once**
    /// forward, picking off every requested id as the decode passes its
    /// position. This is where the redundant per-id re-walk that
    /// [`term`](Self::term) does (decode from the run start for *every* id)
    /// disappears: within a run the walk advances monotonically across all the
    /// window's ids that live there.
    ///
    /// For any id this fills `out[slot]` with exactly what [`term`](Self::term)
    /// returns for that id (the same decoded bytes, the same
    /// `String::from_utf8_lossy`), and leaves `out[slot]` untouched where
    /// `term(id)` is `None` — an absent/out-of-range id, a failed chunk fault,
    /// or malformed bytes. Duplicate ids each fill their own slot. It is the
    /// batch twin of `term`, used by the export window
    /// ([`Dictionary::resolve_window`](crate::dictionary::Dictionary::resolve_window))
    /// so a window's scattered object ids touch each chunk once instead of
    /// re-faulting and re-walking per row.
    ///
    /// [`crate::dictionary::Dictionary::resolve_window`]: the sole caller.
    pub fn resolve_into(&self, jobs: &mut [(u32, usize)], out: &mut [Option<String>]) {
        let ri = self.meta.restart_interval as usize;
        if ri == 0 {
            return;
        }
        let tc = self.meta.term_count;
        // Sort by id: chunks and runs are contiguous ascending id ranges, so
        // this groups jobs by chunk and by run in one pass.
        jobs.sort_unstable_by_key(|&(id, _)| id);
        let mut i = 0;
        while i < jobs.len() {
            let id0 = jobs[i].0;
            // Absent / out of range: `term(id)` is None. Leave the slot None.
            if id0 == ABSENT || id0 > tc {
                i += 1;
                continue;
            }
            let Some(ci) = self.chunk_of_run((id0 - 1) as usize / ri) else {
                i += 1;
                continue;
            };
            // Fault the chunk once. A failed fault -> `term(id)` is None for
            // every id this chunk holds; advance past them, slots left None.
            let Some(entry) = self.chunk_entry(ci) else {
                while i < jobs.len() {
                    let id = jobs[i].0;
                    if id == ABSENT || id > tc {
                        i += 1;
                        continue;
                    }
                    if self.chunk_of_run((id - 1) as usize / ri) != Some(ci) {
                        break;
                    }
                    i += 1;
                }
                continue;
            };
            let bytes: &[u8] = entry.body();
            // Walk this chunk's runs, one forward pass per run.
            while i < jobs.len() {
                let id = jobs[i].0;
                if id == ABSENT || id > tc {
                    i += 1;
                    continue;
                }
                let run = (id - 1) as usize / ri;
                if self.chunk_of_run(run) != Some(ci) {
                    break; // first id of the next chunk
                }
                // Decode this run's restart entry. Any failure here means
                // `term(id)` is None for the whole run — skip every job in it.
                let run_start = self.run_off_in_chunk(ci, run, &entry, ri).and_then(|off| {
                    let mut buf = Vec::new();
                    run_entry_into(bytes, off, &mut buf).map(|pos| (buf, pos))
                });
                let Some((mut buf, mut pos)) = run_start else {
                    while i < jobs.len() {
                        let id = jobs[i].0;
                        if id == ABSENT || id > tc {
                            i += 1;
                            continue;
                        }
                        if (id - 1) as usize / ri != run {
                            break;
                        }
                        i += 1;
                    }
                    continue;
                };
                // `buf` holds the run's term #`cur_step`; advance monotonically.
                let mut cur_step = 0usize;
                let mut broken = false;
                while i < jobs.len() {
                    let id = jobs[i].0;
                    if id == ABSENT || id > tc {
                        i += 1;
                        continue;
                    }
                    if (id - 1) as usize / ri != run {
                        break; // next run (or chunk)
                    }
                    let steps = (id - 1) as usize % ri;
                    if !broken {
                        while cur_step < steps {
                            match entry_into(bytes, pos, &mut buf) {
                                Some(next) => {
                                    pos = next;
                                    cur_step += 1;
                                }
                                None => {
                                    // Malformed past this point: `term(id)` is
                                    // None here and for every larger id in the
                                    // run (they all decode through this entry).
                                    broken = true;
                                    break;
                                }
                            }
                        }
                        if !broken && cur_step == steps {
                            out[jobs[i].1] = Some(String::from_utf8_lossy(&buf).into_owned());
                        }
                    }
                    i += 1;
                }
            }
        }
    }

    /// Resolve `term` to its ID. Chunk-level binary search runs on the (local)
    /// chunk directory, so this also costs at most one chunk fault.
    pub fn id(&self, term: &str) -> Option<u32> {
        if self.chunks.is_empty() {
            return None;
        }
        let ri = self.meta.restart_interval as usize;
        // Pick the chunk: the last one whose ROUTING KEY is <= `term` (the
        // single local chunk skips the search — its key is unset).
        //
        // This is the ONLY read of [`SectionChunk::key`] in the codebase, and
        // all it needs is the separator invariant `last(i-1) < key(i) <=
        // first(i)`: `term` lands in chunk `i` exactly when `key(i) <= term <
        // key(i+1)`, which covers every term of chunk `i` and nothing in any
        // other chunk. A verbatim first term satisfies it too, which is why
        // files written either way route identically.
        //
        // With separators the search can land on chunk `i` for a `term` in the
        // gap `key(i) <= term < first(i)`; the run search below then hits its
        // `lo == first_run` branch and answers `None`, which is correct — such
        // a term is strictly between two chunks, i.e. absent.
        let ci = if self.chunks.len() == 1 {
            0
        } else {
            let i = self
                .chunks
                .partition_point(|c| c.key.as_slice() <= term.as_bytes());
            i.checked_sub(1)?
        };
        let first_run = self.chunks[ci].first_run;
        let entry = self.chunk_entry(ci)?;
        let bytes: &[u8] = entry.body();

        // Binary search this chunk's runs by their first (full) term.
        let run_end = self.run_end_of_chunk(ci, &entry, ri);
        let mut buf = Vec::new();
        let mut lo = first_run;
        let mut hi = run_end;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = self.run_off_in_chunk(ci, mid, &entry, ri)?;
            run_entry_into(bytes, off, &mut buf)?;
            if buf.as_slice() <= term.as_bytes() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == first_run {
            // Smaller than every term in (and before) this chunk. Unreachable
            // when the key is a verbatim first term; reachable, and correct,
            // when it is a separator: `key(ci) <= term < first(ci)` puts the
            // term strictly between two chunks.
            return None;
        }
        let run = lo - 1;
        let off = self.run_off_in_chunk(ci, run, &entry, ri)?;
        let mut pos = run_entry_into(bytes, off, &mut buf)?;
        let base_id = (run * ri) as u32 + 1;
        // saturating_sub: corrupt metadata must not underflow-panic.
        let run_len = self.meta.restart_interval.min(
            self.meta
                .term_count
                .saturating_sub(run as u32 * self.meta.restart_interval),
        );
        for step in 0..run_len {
            if buf.as_slice() == term.as_bytes() {
                return Some(base_id + step);
            }
            if buf.as_slice() > term.as_bytes() {
                return None;
            }
            if step + 1 < run_len {
                pos = entry_into(bytes, pos, &mut buf)?;
            }
        }
        None
    }

    /// The full serialized section (header + body), for the file writer. The
    /// local single-chunk case returns the stored bytes; a chunked section
    /// re-assembles them (header re-encoded from the metadata).
    pub fn raw_section_bytes(&self) -> Vec<u8> {
        if self.chunks.len() == 1 && self.chunks[0].body_start == 0 {
            if let Some(entry) = self.cache.get((self.section_index, 0)) {
                return entry.body().to_vec();
            }
        }
        let mut out = encode_section_header(&self.meta);
        for ci in 0..self.chunks.len() {
            if let Some(entry) = self.chunk_entry(ci) {
                out.extend_from_slice(entry.body());
            }
        }
        out
    }
}

/// Re-encode a section header (term_count, interval, restart table) from its
/// parsed metadata — the inverse of [`parse_meta`]. Restart offsets are stored
/// body-relative on disk; `meta` holds them absolute, so the body start is
/// re-derived as the first run's offset.
pub fn encode_section_header(meta: &SectionMeta) -> Vec<u8> {
    let body_start = meta.restart_offsets.first().copied().unwrap_or(0);
    let mut out = Vec::new();
    write_uvarint(&mut out, meta.term_count as u64);
    write_uvarint(&mut out, meta.restart_interval as u64);
    write_uvarint(&mut out, meta.restart_offsets.len() as u64);
    for off in &meta.restart_offsets {
        write_uvarint(&mut out, off.saturating_sub(body_start));
    }
    out
}

/// A parsed, read-only dictionary section (parses its metadata on construction).
pub struct DictSection<'a> {
    bytes: &'a [u8],
    meta: SectionMeta,
}

impl<'a> DictSection<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self, DictError> {
        Ok(Self {
            bytes,
            meta: parse_meta(bytes)?,
        })
    }

    pub fn len(&self) -> u32 {
        self.meta.term_count
    }

    pub fn is_empty(&self) -> bool {
        self.meta.term_count == 0
    }

    /// Resolve `id` (1-based) to its term, or `None` if out of range.
    pub fn term(&self, id: u32) -> Option<String> {
        section_term(self.bytes, &self.meta, id)
    }

    /// Resolve `term` to its ID, or `None` if absent.
    pub fn id(&self, term: &str) -> Option<u32> {
        section_id(self.bytes, &self.meta, term)
    }
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a *lite* (empty section-wide restart table), genuinely multi-chunk
    /// [`ChunkedSection`] from `terms`: the exact shape a ranged open produces,
    /// so `resolve_into`'s per-chunk run-table branch and its cross-chunk /
    /// cross-run walk are all exercised. `runs_per_chunk` chops the runs into
    /// several chunks regardless of size, so a tiny fixture still spans many.
    fn lite_multichunk(terms: &[&str], ri: u32, runs_per_chunk: usize) -> (ChunkedSection, u32) {
        let mut b = DictSectionBuilder::new().with_restart_interval(ri);
        for t in terms {
            b.push(*t);
        }
        let raw = b.build();
        let full_meta = parse_meta(&raw).unwrap();
        let n_runs = full_meta.restart_offsets.len();
        let mut chunks = Vec::new();
        let mut bodies: Vec<Vec<u8>> = Vec::new();
        let mut r = 0;
        while r < n_runs {
            let first_run = r;
            let start = full_meta.restart_offsets[r] as usize;
            let r2 = (r + runs_per_chunk).min(n_runs);
            let end = if r2 < n_runs {
                full_meta.restart_offsets[r2] as usize
            } else {
                raw.len()
            };
            chunks.push(SectionChunk::new(first_run, Vec::new(), start as u64));
            bodies.push(raw[start..end].to_vec());
            r = r2;
        }
        assert!(chunks.len() > 1, "fixture must be multi-chunk");
        // Lite: no section-wide restart table -> run offsets come from each
        // decoded chunk body, cached in its cache entry (the ranged path).
        let lite_meta = SectionMeta {
            term_count: full_meta.term_count,
            restart_interval: full_meta.restart_interval,
            restart_offsets: Vec::new(),
        };
        let loader: ChunkLoader = Box::new(move |ci| bodies.get(ci).cloned());
        let sec = ChunkedSection::from_parts(
            lite_meta,
            chunks,
            Some(loader),
            ChunkCache::unlimited_arc(),
            0,
        );
        (sec, full_meta.term_count)
    }

    /// The windowed batch resolver must fill exactly what a per-id `term()`
    /// would, for a scrambled, duplicated, out-of-range id set spread across
    /// many chunks and runs — the byte-identity guarantee `dump_filtered_each`
    /// rests on (phase 1). Also cross-checked against the ground-truth
    /// `section_term` decoder.
    #[test]
    fn resolve_into_matches_term_across_chunks_and_runs() {
        // Shared prefixes (front-coding), a long literal (a run that dwarfs its
        // neighbors), blank nodes — 240 distinct terms.
        let owned: Vec<String> = (0..240)
            .map(|i| match i % 4 {
                0 => format!("<http://example.org/entity/{i:05}>"),
                1 => format!("<http://example.org/entity/{i:05}/sub/leaf>"),
                2 => format!("\"literal value {} padded {}\"", i, "x".repeat(i % 50)),
                _ => format!("_:b{i:05}"),
            })
            .collect();
        let terms: Vec<&str> = owned.iter().map(String::as_str).collect();

        for (ri, rpc) in [(1u32, 5usize), (4, 3), (16, 2)] {
            let (sec, count) = lite_multichunk(&terms, ri, rpc);
            let raw = {
                let mut b = DictSectionBuilder::new().with_restart_interval(ri);
                for t in &terms {
                    b.push(*t);
                }
                b.build()
            };
            let full_meta = parse_meta(&raw).unwrap();

            // A scrambled probe set: every id once, a run of duplicates, some
            // out-of-range ids (0 and past the end) that must stay `None`.
            let mut probe: Vec<u32> = (1..=count).collect();
            probe.extend([1, 1, count, count, ABSENT, count + 3, count + 100]);
            // Deterministic shuffle so chunk order != probe order.
            let mut state = 0xDEAD_BEEF_1234_5678u64;
            for k in (1..probe.len()).rev() {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                probe.swap(k, (state % (k as u64 + 1)) as usize);
            }

            let mut jobs: Vec<(u32, usize)> = probe.iter().map(|&id| (id, 0)).collect();
            for (slot, j) in jobs.iter_mut().enumerate() {
                j.1 = slot;
            }
            let mut out: Vec<Option<String>> = vec![None; probe.len()];
            sec.resolve_into(&mut jobs, &mut out);

            for (slot, &id) in probe.iter().enumerate() {
                let via_term = sec.term(id);
                assert_eq!(
                    out[slot], via_term,
                    "resolve_into(id={id}) != term(id) at ri={ri} rpc={rpc}"
                );
                // Ground truth for in-range ids.
                let truth = section_term(&raw, &full_meta, id);
                assert_eq!(out[slot], truth, "resolve_into(id={id}) != section_term");
            }
        }
    }

    fn sample() -> Vec<String> {
        // Deliberately unsorted, with shared prefixes and a duplicate.
        [
            "http://ex.org/Alice",
            "http://ex.org/Bob",
            "http://ex.org/Alan",
            "http://ex.org/knows",
            "http://ex.org/Alice", // dup
            "zeta",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    #[test]
    fn round_trip_all_ids_and_terms() {
        for r in [1u32, 2, 16, 1000] {
            let mut b = DictSectionBuilder::new().with_restart_interval(r);
            for t in sample() {
                b.push(t);
            }
            let bytes = b.build();
            let sec = DictSection::parse(&bytes).unwrap();

            // Expected sorted unique set.
            let mut expected = sample();
            expected.sort();
            expected.dedup();
            assert_eq!(sec.len() as usize, expected.len());

            for (i, term) in expected.iter().enumerate() {
                let id = (i + 1) as u32;
                assert_eq!(
                    sec.term(id).as_deref(),
                    Some(term.as_str()),
                    "term({id}) r={r}"
                );
                assert_eq!(sec.id(term), Some(id), "id({term}) r={r}");
            }
        }
    }

    #[test]
    fn lookups_for_absent_terms() {
        let mut b = DictSectionBuilder::new();
        for t in sample() {
            b.push(t);
        }
        let bytes = b.build();
        let sec = DictSection::parse(&bytes).unwrap();
        assert_eq!(sec.id("aaa-before-everything"), None);
        assert_eq!(sec.id("zzz-after-everything"), None);
        assert_eq!(sec.id("http://ex.org/Alic"), None); // prefix, not present
        assert_eq!(sec.term(0), None);
        assert_eq!(sec.term(9999), None);
    }

    /// Property-style stress of the front-coded decode paths: a large
    /// deterministic-pseudo-random term pool (IRIs with heavy shared prefixes,
    /// literals, blank nodes — duplicates likely) must round-trip every id and
    /// term across restart intervals, and near-miss probes (a stored term ± a
    /// suffix/truncation, including run-boundary terms) must resolve to `None`.
    /// Term resolution is the engine's hot path, so this is the test that
    /// guards the buffer-reuse decode.
    #[test]
    fn randomized_round_trip_and_near_misses_across_restart_intervals() {
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let pool: Vec<String> = (0..1500)
            .map(|i| {
                let n = next();
                match n % 4 {
                    0 => format!("<http://example.org/entity/{n:x}>"),
                    1 => format!("\"literal value {} with spaces\"", n % 300), // dups likely
                    2 => format!("<http://example.org/entity/{}/sub/{i}>", n % 64), // long shared prefixes
                    _ => format!("_:b{}", n % 256),
                }
            })
            .collect();
        let mut expected = pool.clone();
        expected.sort();
        expected.dedup();

        for r in [1u32, 3, 16, 64] {
            let mut b = DictSectionBuilder::new().with_restart_interval(r);
            for t in &pool {
                b.push(t.clone());
            }
            let bytes = b.build();
            let sec = DictSection::parse(&bytes).unwrap();
            assert_eq!(sec.len() as usize, expected.len(), "r={r}");

            for (i, term) in expected.iter().enumerate() {
                let id = (i + 1) as u32;
                assert_eq!(
                    sec.term(id).as_deref(),
                    Some(term.as_str()),
                    "term({id}) r={r}"
                );
                assert_eq!(sec.id(term), Some(id), "id({term}) r={r}");
            }
            // Near misses around run boundaries (and a sample of the rest):
            // an appended suffix or a truncation is never a stored term unless
            // it happens to collide with one.
            for (i, term) in expected.iter().enumerate() {
                let near_boundary = (i as u32) % r <= 1;
                if !near_boundary && i % 37 != 0 {
                    continue;
                }
                let longer = format!("{term}\u{1}");
                assert_eq!(sec.id(&longer), None, "near-miss long r={r}");
                let mut shorter = term.clone();
                shorter.pop();
                if !shorter.is_empty() && expected.binary_search(&shorter).is_err() {
                    assert_eq!(sec.id(&shorter), None, "near-miss short {shorter:?} r={r}");
                }
            }
            assert_eq!(sec.term(expected.len() as u32 + 1), None, "past-end r={r}");
        }
    }
}
