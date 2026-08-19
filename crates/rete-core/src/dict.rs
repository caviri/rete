//! Front-coded, restart-indexed dictionary sections (SPEC.md §5.1).
//!
//! A section holds the UTF-8 terms of one kind (shared / subjects / objects /
//! predicates / graphs), sorted and assigned dense 1-based IDs. Terms are stored
//! in runs of `R`; each run starts with a full term and front-codes the rest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::adaptive::ReadIntent;
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
        self.build_sorted_unique()
    }

    /// Serialize terms that are already sorted and unique. This keeps the
    /// canonicalizer from repeating either operation after role partitioning.
    #[allow(dead_code)]
    pub(crate) fn from_sorted_unique(terms: Vec<String>) -> Self {
        Self {
            terms,
            restart_interval: env_restart_interval(),
        }
    }

    pub(crate) fn build_sorted_unique(self) -> Vec<u8> {
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
    parse_meta_inner(bytes, Some(bytes.len() as u64))
}

/// Parse a chunked container's standalone raw-section header.  Its restart
/// offsets name the *decompressed* raw section, so a compressed container
/// length cannot safely bound them; checked reconstruction, ordering, and the
/// exact expected restart count still apply.
pub(crate) fn parse_meta_header_fragment(bytes: &[u8]) -> Result<SectionMeta, DictError> {
    parse_meta_inner(bytes, None)
}

fn parse_meta_inner(bytes: &[u8], section_end: Option<u64>) -> Result<SectionMeta, DictError> {
    let mut pos = 0;
    let take = |pos: &mut usize| -> Result<u64, DictError> {
        let (v, n) =
            read_uvarint(&bytes[*pos..]).ok_or(DictError::Malformed("truncated header"))?;
        *pos += n;
        Ok(v)
    };
    let term_count = u32::try_from(take(&mut pos)?)
        .map_err(|_| DictError::Malformed("term count exceeds u32"))?;
    let restart_interval = u32::try_from(take(&mut pos)?)
        .map_err(|_| DictError::Malformed("restart interval exceeds u32"))?;
    let num_restarts = usize::try_from(take(&mut pos)?)
        .map_err(|_| DictError::Malformed("restart count too large"))?;
    if restart_interval == 0 {
        return Err(DictError::Malformed("zero restart interval"));
    }
    let expected_restarts = u64::from(term_count).div_ceil(u64::from(restart_interval));
    if u64::try_from(num_restarts).ok() != Some(expected_restarts) {
        return Err(DictError::Malformed(
            "restart count does not match term count",
        ));
    }
    // `num_restarts` is untrusted; each restart is ≥1 byte, so cap the
    // pre-allocation at the buffer length to avoid an OOM on a bogus count.
    let mut rel = Vec::with_capacity(num_restarts.min(bytes.len()));
    for _ in 0..num_restarts {
        rel.push(take(&mut pos)?);
    }
    let body_start = u64::try_from(pos).map_err(|_| DictError::Malformed("header too large"))?;
    let mut restart_offsets = Vec::with_capacity(rel.len());
    let mut previous = None;
    for relative in rel {
        let absolute = body_start
            .checked_add(relative)
            .ok_or(DictError::Malformed("restart offset overflows"))?;
        if section_end.is_some_and(|end| absolute >= end) {
            return Err(DictError::Malformed("restart offset outside section"));
        }
        if previous.is_some_and(|last| absolute <= last) {
            return Err(DictError::Malformed("restart offsets are not monotone"));
        }
        restart_offsets.push(absolute);
        previous = Some(absolute);
    }
    Ok(SectionMeta {
        term_count,
        restart_interval,
        restart_offsets,
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
    let shared = usize::try_from(shared).ok()?;
    let p = pos.checked_add(n1)?;
    let (suf, n2) = read_uvarint(bytes.get(p..)?)?;
    let suffix = usize::try_from(suf).ok()?;
    let start = p.checked_add(n2)?;
    let end = start.checked_add(suffix).filter(|&e| e <= bytes.len())?;
    if shared > buf.len() {
        return None;
    }
    buf.truncate(shared);
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
    let idx = usize::try_from(id - 1).ok()?;
    let interval = usize::try_from(meta.restart_interval).ok()?;
    let run = idx / interval;
    let steps = idx % interval;
    let mut buf = Vec::new();
    let restart = usize::try_from(*meta.restart_offsets.get(run)?)
        .ok()
        .filter(|&offset| offset < bytes.len())?;
    let mut pos = run_entry_into(bytes, restart, &mut buf)?;
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
        let restart = usize::try_from(meta.restart_offsets[mid])
            .ok()
            .filter(|&offset| offset < bytes.len())?;
        run_entry_into(bytes, restart, &mut buf)?;
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
    let restart = usize::try_from(meta.restart_offsets[run])
        .ok()
        .filter(|&offset| offset < bytes.len())?;
    let mut pos = run_entry_into(bytes, restart, &mut buf)?;
    let interval = u64::from(meta.restart_interval);
    let run = u64::try_from(run).ok()?;
    let base_id = u32::try_from(run.checked_mul(interval)?.checked_add(1)?).ok()?;
    // saturating_sub: corrupt metadata where run*interval > term_count must not
    // underflow-panic.
    let run_len = meta.restart_interval.min(
        meta.term_count
            .checked_sub(u32::try_from(run.checked_mul(interval)?).ok()?)?,
    );
    for step in 0..run_len {
        if buf.as_slice() == term.as_bytes() {
            return base_id.checked_add(step);
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
pub type ChunkBulkLoader = Box<dyn Fn(&[usize], ReadIntent) -> Option<Vec<Vec<u8>>> + Send + Sync>;

/// One chunk: a run-aligned slice of the section body. `body_start` is the
/// offset (in the section's coordinate space — the same space
/// [`SectionMeta::restart_offsets`] uses) where `data[0]` sits.
pub struct SectionChunk {
    first_run: usize,
    /// First term of the chunk (for chunk-level binary search in `id`);
    /// unused (empty) for the single local chunk.
    first_term: Vec<u8>,
    body_start: u64,
    data: OnceLock<Vec<u8>>,
    /// This chunk's per-run byte offsets, **relative to its own decompressed
    /// body** — the chunk-local stand-in for the section-wide restart table.
    /// Derived by scanning the body once on first lookup and cached, so a
    /// remote open never materializes the section's millions of restart
    /// offsets (a 50 M-term section's table is ~24 MiB — an iOS-Safari OOM).
    runs: OnceLock<Vec<usize>>,
}

impl SectionChunk {
    /// A remote chunk descriptor (data faults in through the loader).
    pub fn remote(first_run: usize, first_term: Vec<u8>, body_start: u64) -> Self {
        SectionChunk {
            first_run,
            first_term,
            body_start,
            data: OnceLock::new(),
            runs: OnceLock::new(),
        }
    }

    /// A resident chunk (data already decoded).
    pub fn resident(first_run: usize, first_term: Vec<u8>, body_start: u64, data: Vec<u8>) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(data);
        SectionChunk {
            first_run,
            first_term,
            body_start,
            data: cell,
            runs: OnceLock::new(),
        }
    }

    /// The byte offset (into `data`) of each run in this chunk, computed once by
    /// scanning the decompressed body and cached. `data` must be this chunk's
    /// body. Offset 0 is the chunk's first run (chunks are run-aligned); every
    /// `restart_interval` terms starts the next run. An EMPTY body (the
    /// transient-fetch-failure sentinel — never a real chunk) is not cached, or
    /// a failed chunk's empty offset table would outlive the retryable data.
    fn run_offsets(&self, data: &[u8], restart_interval: usize) -> &[usize] {
        if let Some(r) = self.runs.get() {
            return r;
        }
        if data.is_empty() {
            return &[];
        }
        self.runs
            .get_or_init(|| chunk_run_offsets(data, restart_interval))
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

/// A dictionary section whose body is served chunk-by-chunk: metadata + chunk
/// directory always present, chunk bytes local or faulted in on first touch.
pub struct ChunkedSection {
    meta: SectionMeta,
    chunks: Vec<SectionChunk>,
    loader: Option<ChunkLoader>,
    bulk: Option<ChunkBulkLoader>,
    failed: AtomicBool,
}

impl ChunkedSection {
    /// A local section: the whole serialized section (header + body) as one
    /// pre-set chunk at coordinate 0, so the absolute restart offsets index it
    /// directly. Malformed bytes degrade to an empty section (no panics on
    /// untrusted files), matching the previous reader behavior.
    pub fn local(section_bytes: Vec<u8>) -> Self {
        let meta = parse_meta(&section_bytes).unwrap_or(SectionMeta {
            term_count: 0,
            restart_interval: 1,
            restart_offsets: Vec::new(),
        });
        let data = OnceLock::new();
        let _ = data.set(section_bytes);
        ChunkedSection {
            meta,
            chunks: vec![SectionChunk {
                first_run: 0,
                first_term: Vec::new(),
                body_start: 0,
                data,
                runs: OnceLock::new(),
            }],
            loader: None,
            bulk: None,
            failed: AtomicBool::new(false),
        }
    }

    /// A section from parsed parts: metadata + chunk list, with an optional
    /// loader for non-resident chunks (the remote lazy-open path) — resident
    /// chunk lists (a locally-decoded chunked section) pass `None`.
    pub fn from_parts(
        meta: SectionMeta,
        chunks: Vec<SectionChunk>,
        loader: Option<ChunkLoader>,
    ) -> Self {
        ChunkedSection {
            meta,
            chunks,
            loader,
            bulk: None,
            failed: AtomicBool::new(false),
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
        self.prefetch_chunks_with_intent(
            &(0..self.chunks.len()).collect::<Vec<_>>(),
            ReadIntent::FullScan,
        );
    }

    /// Batch-fault a *specific* set of chunks (the subset a bounded query's
    /// output touches) through the bulk loader, coalescing byte-adjacent ranges
    /// into single reads. `cis` should be ascending and deduped; chunks already
    /// resident are skipped. Like [`prefetch_all`](Self::prefetch_all) a single
    /// missing chunk is left for the per-chunk loader, and a failed batch leaves
    /// the chunks unloaded for that loader to retry (and record failures).
    pub fn prefetch_chunks(&self, cis: &[usize]) {
        self.prefetch_chunks_with_intent(cis, ReadIntent::DictionaryResolve);
    }

    fn prefetch_chunks_with_intent(&self, cis: &[usize], intent: ReadIntent) {
        let Some(bulk) = &self.bulk else { return };
        let missing: Vec<usize> = cis
            .iter()
            .copied()
            .filter(|&ci| self.chunks.get(ci).is_some_and(|c| c.data.get().is_none()))
            .collect();
        if missing.len() < 2 {
            return;
        }
        if let Some(bodies) = bulk(&missing, intent) {
            if bodies.len() == missing.len() {
                for (&ci, body) in missing.iter().zip(bodies) {
                    let _ = self.chunks[ci].data.set(body);
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

    /// A FAILED fetch records the failure and returns an empty slice WITHOUT
    /// caching it, so a later resolution retries the chunk — a transient
    /// network error must not permanently poison a resident session.
    fn chunk_data(&self, ci: usize) -> &[u8] {
        let cell = &self.chunks[ci].data;
        if let Some(d) = cell.get() {
            return d;
        }
        match &self.loader {
            Some(load) => match load(ci) {
                Some(bytes) => cell.get_or_init(|| bytes),
                None => {
                    self.failed.store(true, Ordering::Relaxed);
                    &[]
                }
            },
            None => cell.get_or_init(Vec::new),
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
        let run =
            usize::try_from(u64::from(id - 1) / u64::from(self.meta.restart_interval)).ok()?;
        self.chunk_of_run(run)
    }

    /// The byte offset (into chunk `ci`'s decompressed body `bytes`) of `run`.
    /// Full/local sections use the section-wide restart table (unchanged
    /// behavior); a *lite* remote section (empty `restart_offsets`) derives it
    /// from the chunk itself, so the open never holds the whole table.
    fn run_off_in_chunk(&self, ci: usize, run: usize, bytes: &[u8], ri: usize) -> Option<usize> {
        let chunk = &self.chunks[ci];
        if self.meta.restart_offsets.is_empty() {
            chunk
                .run_offsets(bytes, ri)
                .get(run.checked_sub(chunk.first_run)?)
                .copied()
        } else {
            let offset = self
                .meta
                .restart_offsets
                .get(run)?
                .checked_sub(chunk.body_start)?;
            usize::try_from(offset)
                .ok()
                .filter(|&offset| offset < bytes.len())
        }
    }

    /// One past the last run index held by chunk `ci` (its run range is
    /// `[first_run, run_end)`).
    fn run_end_of_chunk(&self, ci: usize, bytes: &[u8], ri: usize) -> Option<usize> {
        if let Some(next) = self.chunks.get(ci + 1) {
            return Some(next.first_run);
        }
        let chunk = &self.chunks[ci];
        if self.meta.restart_offsets.is_empty() {
            chunk
                .first_run
                .checked_add(chunk.run_offsets(bytes, ri).len())
        } else {
            Some(self.meta.restart_offsets.len())
        }
    }

    /// Resolve `id` (1-based) to its term. One chunk fault at most.
    /// Chunk index holding term `id` — [`term`](Self::term)'s routing without
    /// faulting the chunk. `None` for an absent/out-of-range id.
    pub(crate) fn chunk_of_term(&self, id: u32) -> Option<usize> {
        if id == ABSENT || id > self.meta.term_count {
            return None;
        }
        let run =
            usize::try_from(u64::from(id - 1) / u64::from(self.meta.restart_interval)).ok()?;
        self.chunk_of_run(run)
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
        let bytes = self.chunk_data(ci);
        let off = self.run_off_in_chunk(ci, run, bytes, ri)?;
        let mut buf = Vec::new();
        let mut pos = run_entry_into(bytes, off, &mut buf)?;
        for _ in 0..steps {
            pos = entry_into(bytes, pos, &mut buf)?;
        }
        Some(String::from_utf8_lossy(&buf).into_owned())
    }

    /// Resolve `term` to its ID. Chunk-level binary search runs on the (local)
    /// chunk directory, so this also costs at most one chunk fault.
    pub fn id(&self, term: &str) -> Option<u32> {
        if self.chunks.is_empty() {
            return None;
        }
        let ri = self.meta.restart_interval as usize;
        // Pick the chunk: the last one whose first term is <= `term` (the
        // single local chunk skips the search — its `first_term` is unset).
        let ci = if self.chunks.len() == 1 {
            0
        } else {
            let i = self
                .chunks
                .partition_point(|c| c.first_term.as_slice() <= term.as_bytes());
            i.checked_sub(1)?
        };
        let first_run = self.chunks[ci].first_run;
        let bytes = self.chunk_data(ci);

        // Binary search this chunk's runs by their first (full) term.
        let run_end = self.run_end_of_chunk(ci, bytes, ri)?;
        let mut buf = Vec::new();
        let mut lo = first_run;
        let mut hi = run_end;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = self.run_off_in_chunk(ci, mid, bytes, ri)?;
            run_entry_into(bytes, off, &mut buf)?;
            if buf.as_slice() <= term.as_bytes() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == first_run {
            return None; // smaller than every term in (and before) this chunk
        }
        let run = lo - 1;
        let off = self.run_off_in_chunk(ci, run, bytes, ri)?;
        let mut pos = run_entry_into(bytes, off, &mut buf)?;
        let interval = u64::from(self.meta.restart_interval);
        let run_u64 = u64::try_from(run).ok()?;
        let base_id = u32::try_from(run_u64.checked_mul(interval)?.checked_add(1)?).ok()?;
        // saturating_sub: corrupt metadata must not underflow-panic.
        let run_len = self.meta.restart_interval.min(
            self.meta
                .term_count
                .checked_sub(u32::try_from(run_u64.checked_mul(interval)?).ok()?)?,
        );
        for step in 0..run_len {
            if buf.as_slice() == term.as_bytes() {
                return base_id.checked_add(step);
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
            if let Some(bytes) = self.chunks[0].data.get() {
                return bytes.clone();
            }
        }
        let mut out = encode_section_header(&self.meta);
        for ci in 0..self.chunks.len() {
            out.extend_from_slice(self.chunk_data(ci));
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

    #[test]
    fn parse_meta_rejects_dictionary_ids_and_counts_above_u32() {
        let mut bytes = Vec::new();
        write_uvarint(&mut bytes, u32::MAX as u64 + 1);
        write_uvarint(&mut bytes, 1);
        write_uvarint(&mut bytes, 0);
        assert!(matches!(
            parse_meta(&bytes),
            Err(DictError::Malformed("term count exceeds u32"))
        ));

        let mut bytes = Vec::new();
        write_uvarint(&mut bytes, 0);
        write_uvarint(&mut bytes, u32::MAX as u64 + 1);
        write_uvarint(&mut bytes, 0);
        assert!(matches!(
            parse_meta(&bytes),
            Err(DictError::Malformed("restart interval exceeds u32"))
        ));
    }

    #[test]
    fn parse_meta_rejects_tenth_byte_u64_overflow() {
        // `[term_count=1][interval=1][restart_count=1][restart_offset]`
        // followed by one complete front-coded entry.  The old shared parser
        // accepted this overflowing restart offset as zero.
        let mut bytes = vec![1, 1, 1];
        bytes.extend_from_slice(&[0x80; 9]);
        bytes.push(0x02);
        bytes.extend_from_slice(&[0, 1, b'a']);
        assert!(parse_meta(&bytes).is_err());
    }

    #[test]
    fn hostile_entry_and_restart_offsets_do_not_alias_usize() {
        let mut hostile = Vec::new();
        write_uvarint(&mut hostile, u64::MAX);
        write_uvarint(&mut hostile, u64::MAX);
        assert!(entry_into(&hostile, 0, &mut Vec::new()).is_none());

        let meta = SectionMeta {
            term_count: 1,
            restart_interval: 1,
            restart_offsets: vec![u64::MAX],
        };
        assert_eq!(section_term(&[0, 1, b'x'], &meta, 1), None);
        assert_eq!(section_id(&[0, 1, b'x'], &meta, "x"), None);
    }

    #[test]
    fn parse_meta_rejects_overflowing_and_non_monotone_restart_offsets() {
        let mut overflow = Vec::new();
        write_uvarint(&mut overflow, 1);
        write_uvarint(&mut overflow, 1);
        write_uvarint(&mut overflow, 1);
        write_uvarint(&mut overflow, u64::MAX);
        let result = std::panic::catch_unwind(|| parse_meta(&overflow));
        assert!(result.is_ok(), "untrusted restart offset must not panic");
        assert!(result.unwrap().is_err());

        let mut non_monotone = Vec::new();
        write_uvarint(&mut non_monotone, 2);
        write_uvarint(&mut non_monotone, 1);
        write_uvarint(&mut non_monotone, 2);
        write_uvarint(&mut non_monotone, 1);
        write_uvarint(&mut non_monotone, 0);
        non_monotone.extend_from_slice(&[0, 1, b'a', 0, 1, b'b']);
        assert!(parse_meta(&non_monotone).is_err());
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
