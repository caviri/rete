//! Front-coded, restart-indexed dictionary sections (SPEC.md §5.1).
//!
//! A section holds the UTF-8 terms of one kind (shared / subjects / objects /
//! predicates / graphs), sorted and assigned dense 1-based IDs. Terms are stored
//! in runs of `R`; each run starts with a full term and front-codes the rest.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

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

impl DictSectionBuilder {
    pub fn new() -> Self {
        Self {
            terms: Vec::new(),
            restart_interval: DEFAULT_RESTART_INTERVAL,
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
    pub restart_offsets: Vec<usize>,
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
        rel.push(take(&mut pos)? as usize);
    }
    let body_start = pos;
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
    let mut pos = run_entry_into(bytes, *meta.restart_offsets.get(run)?, &mut buf)?;
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
        run_entry_into(bytes, meta.restart_offsets[mid], &mut buf)?;
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
    let mut pos = run_entry_into(bytes, meta.restart_offsets[run], &mut buf)?;
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
    /// First term of the chunk (for chunk-level binary search in `id`);
    /// unused (empty) for the single local chunk.
    first_term: Vec<u8>,
    body_start: usize,
    data: OnceLock<Vec<u8>>,
}

impl SectionChunk {
    /// A remote chunk descriptor (data faults in through the loader).
    pub fn remote(first_run: usize, first_term: Vec<u8>, body_start: usize) -> Self {
        SectionChunk {
            first_run,
            first_term,
            body_start,
            data: OnceLock::new(),
        }
    }

    /// A resident chunk (data already decoded).
    pub fn resident(
        first_run: usize,
        first_term: Vec<u8>,
        body_start: usize,
        data: Vec<u8>,
    ) -> Self {
        let cell = OnceLock::new();
        let _ = cell.set(data);
        SectionChunk {
            first_run,
            first_term,
            body_start,
            data: cell,
        }
    }
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
        let Some(bulk) = &self.bulk else { return };
        let missing: Vec<usize> = (0..self.chunks.len())
            .filter(|&ci| self.chunks[ci].data.get().is_none())
            .collect();
        if missing.len() < 2 {
            return;
        }
        if let Some(bodies) = bulk(&missing) {
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

    /// Did any chunk fetch fail since this section was opened? (Sticky.)
    pub fn load_incomplete(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    fn chunk_data(&self, ci: usize) -> &[u8] {
        self.chunks[ci].data.get_or_init(|| match &self.loader {
            Some(load) => load(ci).unwrap_or_else(|| {
                self.failed.store(true, Ordering::Relaxed);
                Vec::new()
            }),
            None => Vec::new(),
        })
    }

    /// The chunk holding `run` (chunks ascend by `first_run`; the first chunk
    /// always starts at run 0).
    fn chunk_of_run(&self, run: usize) -> Option<usize> {
        let i = self.chunks.partition_point(|c| c.first_run <= run);
        i.checked_sub(1)
    }

    /// Resolve `id` (1-based) to its term. One chunk fault at most.
    pub fn term(&self, id: u32) -> Option<String> {
        if id == ABSENT || id > self.meta.term_count {
            return None;
        }
        let idx = (id - 1) as usize;
        let run = idx / self.meta.restart_interval as usize;
        let steps = idx % self.meta.restart_interval as usize;
        let ci = self.chunk_of_run(run)?;
        let bytes = self.chunk_data(ci);
        let off = self
            .meta
            .restart_offsets
            .get(run)?
            .checked_sub(self.chunks[ci].body_start)?;
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
        let chunk = &self.chunks[ci];
        let bytes = self.chunk_data(ci);
        let base = chunk.body_start;

        // Binary search this chunk's runs by their first (full) term.
        let run_end = self
            .chunks
            .get(ci + 1)
            .map(|c| c.first_run)
            .unwrap_or(self.meta.restart_offsets.len());
        let mut buf = Vec::new();
        let mut lo = chunk.first_run;
        let mut hi = run_end;
        while lo < hi {
            let mid = (lo + hi) / 2;
            let off = self.meta.restart_offsets.get(mid)?.checked_sub(base)?;
            run_entry_into(bytes, off, &mut buf)?;
            if buf.as_slice() <= term.as_bytes() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == chunk.first_run {
            return None; // smaller than every term in (and before) this chunk
        }
        let run = lo - 1;
        let off = self.meta.restart_offsets.get(run)?.checked_sub(base)?;
        let mut pos = run_entry_into(bytes, off, &mut buf)?;
        let base_id = (run * self.meta.restart_interval as usize) as u32 + 1;
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
        write_uvarint(&mut out, off.saturating_sub(body_start) as u64);
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
