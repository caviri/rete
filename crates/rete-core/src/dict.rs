//! Front-coded, restart-indexed dictionary sections (SPEC.md §5.1).
//!
//! A section holds the UTF-8 terms of one kind (shared / subjects / objects /
//! predicates / graphs), sorted and assigned dense 1-based IDs. Terms are stored
//! in runs of `R`; each run starts with a full term and front-codes the rest.

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

/// Decode a run's restart (full-term) entry at `off`; returns `(term, next_pos)`.
/// `None` if the bytes are malformed/truncated (an untrusted section).
fn run_entry(bytes: &[u8], off: usize) -> Option<(String, usize)> {
    let (_shared, n1) = read_uvarint(bytes.get(off..)?)?;
    let p = off + n1;
    let (len, n2) = read_uvarint(bytes.get(p..)?)?;
    let start = p + n2;
    let end = start
        .checked_add(len as usize)
        .filter(|&e| e <= bytes.len())?;
    Some((
        String::from_utf8_lossy(&bytes[start..end]).into_owned(),
        end,
    ))
}

/// Decode a front-coded entry at `pos` given the previous term. `None` on
/// malformed bytes (including a shared-prefix length longer than `prev`).
fn delta_entry(bytes: &[u8], pos: usize, prev: &str) -> Option<(String, usize)> {
    let (shared, n1) = read_uvarint(bytes.get(pos..)?)?;
    let p = pos + n1;
    let (suf, n2) = read_uvarint(bytes.get(p..)?)?;
    let start = p + n2;
    let end = start
        .checked_add(suf as usize)
        .filter(|&e| e <= bytes.len())?;
    let mut term = prev.as_bytes().get(..shared as usize)?.to_vec();
    term.extend_from_slice(&bytes[start..end]);
    Some((String::from_utf8_lossy(&term).into_owned(), end))
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
    let (mut cur, mut pos) = run_entry(bytes, *meta.restart_offsets.get(run)?)?;
    for _ in 0..steps {
        let (next, np) = delta_entry(bytes, pos, &cur)?;
        cur = next;
        pos = np;
    }
    Some(cur)
}

/// Resolve `term` to its ID using cached metadata.
pub fn section_id(bytes: &[u8], meta: &SectionMeta, term: &str) -> Option<u32> {
    // Binary search restart runs by their first (full) term.
    let mut lo = 0usize;
    let mut hi = meta.restart_offsets.len();
    while lo < hi {
        let mid = (lo + hi) / 2;
        let (first, _) = run_entry(bytes, meta.restart_offsets[mid])?;
        if first.as_str() <= term {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo == 0 {
        return None; // smaller than every term
    }
    let run = lo - 1;
    let (mut cur, mut pos) = run_entry(bytes, meta.restart_offsets[run])?;
    let base_id = (run * meta.restart_interval as usize) as u32 + 1;
    // saturating_sub: corrupt metadata where run*interval > term_count must not
    // underflow-panic.
    let run_len = meta.restart_interval.min(
        meta.term_count
            .saturating_sub(run as u32 * meta.restart_interval),
    );
    for step in 0..run_len {
        if cur == term {
            return Some(base_id + step);
        }
        if cur.as_str() > term {
            return None;
        }
        if step + 1 < run_len {
            let (next, np) = delta_entry(bytes, pos, &cur)?;
            cur = next;
            pos = np;
        }
    }
    None
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
}
