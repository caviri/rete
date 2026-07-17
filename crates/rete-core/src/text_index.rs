//! Full-text (whole-word) index over literals — `token → subjects`.
//!
//! Built at `rete build --text-index` and stored as the optional `TextIndex`
//! file section (SPEC §6). Every string-literal object is tokenized into words;
//! the index maps each word to the **sorted subject ids** that carry it, so a
//! reader can answer "which entities mention `glucose`?" without scanning the
//! literals — and a *remote* reader fetches only the posting lists it queries.
//!
//! On-disk section layout:
//! ```text
//! varint  token_table_len
//! token table (compressed with the file's block codec):
//!   varint num_tokens
//!   per token (sorted): varint shared_prefix_len   # front-coded vs previous token
//!                       varint suffix_len, suffix bytes
//!                       varint posting_off, varint posting_len   # into the postings blob
//! postings blob (uncompressed, so a single posting range-reads directly):
//!   per token (same order): varint count, then `count` delta-varint subject ids
//! ```
//! The token table is small (distinct words) and read whole; the postings blob is
//! the bulk and is fetched one posting at a time on the remote path.

use std::collections::{BTreeMap, BTreeSet};

use crate::file::{compress, decompress, FileError};
use crate::varint::{read_uvarint, write_uvarint};

/// Minimum token length kept (1-char words are low-value and bloat the index).
const MIN_TOKEN_LEN: usize = 2;

/// Most table tokens a [`TextIndex::substring`] piece may match before the
/// lookup declines (each match is a posting read — a range fetch remotely).
const SUBSTRING_TOKEN_CAP: usize = 512;

/// Split text into index/query tokens: Unicode-alphanumeric runs, lowercased,
/// length ≥ `MIN_TOKEN_LEN`. The build and query sides MUST use this same
/// function so a query word matches how it was indexed.
pub fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN)
        .map(|t| t.to_lowercase())
}

/// Length of the longest shared **byte** prefix of `a` and `b`, clamped down to a
/// char boundary of `b` so front-coded suffixes stay valid UTF-8.
fn shared_prefix(a: &str, b: &str) -> usize {
    let mut n = a
        .as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count();
    while n > 0 && !b.is_char_boundary(n) {
        n -= 1;
    }
    n
}

/// Accumulates `token → subjects` and serializes the section.
#[derive(Default)]
pub struct TextIndexBuilder {
    postings: BTreeMap<String, BTreeSet<u32>>,
}

impl TextIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `subject` carries `token`.
    pub fn add(&mut self, token: &str, subject: u32) {
        self.postings
            .entry(token.to_string())
            .or_default()
            .insert(subject);
    }

    /// Tokenize `text` and record every word for `subject`.
    pub fn add_text(&mut self, text: &str, subject: u32) {
        for tok in tokenize(text) {
            self.postings.entry(tok).or_default().insert(subject);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.postings.is_empty()
    }

    /// Serialize the section (deterministic: tokens sorted, subjects ascending).
    pub fn build(&self, codec: u8) -> Vec<u8> {
        // Postings blob + per-token (offset, len) into it.
        let mut postings = Vec::new();
        let mut entries: Vec<(&str, u64, u64)> = Vec::with_capacity(self.postings.len());
        for (token, subjects) in &self.postings {
            let off = postings.len() as u64;
            write_uvarint(&mut postings, subjects.len() as u64);
            let mut prev = 0u32;
            for &s in subjects {
                write_uvarint(&mut postings, (s - prev) as u64);
                prev = s;
            }
            entries.push((token, off, postings.len() as u64 - off));
        }
        // Front-coded token table.
        let mut tt = Vec::new();
        write_uvarint(&mut tt, entries.len() as u64);
        let mut prev = "";
        for (token, off, len) in &entries {
            let shared = shared_prefix(prev, token);
            write_uvarint(&mut tt, shared as u64);
            let suffix = &token.as_bytes()[shared..];
            write_uvarint(&mut tt, suffix.len() as u64);
            tt.extend_from_slice(suffix);
            write_uvarint(&mut tt, *off);
            write_uvarint(&mut tt, *len);
            prev = token;
        }
        let ctt = compress(codec, &tt);

        let mut out = Vec::with_capacity(10 + ctt.len() + postings.len());
        write_uvarint(&mut out, ctt.len() as u64);
        out.extend_from_slice(&ctt);
        out.extend_from_slice(&postings);
        out
    }
}

/// Where a [`TextIndex`] reads posting lists from.
enum Postings {
    /// The whole postings blob, resident (local / fully-loaded reads).
    Resident(Vec<u8>),
    /// Fetch one posting `(offset_within_blob, len)` on demand (remote reads).
    Remote(Box<dyn Fn(u64, u64) -> Option<Vec<u8>> + Send + Sync>),
}

/// A parsed text index: the token table (always resident) plus a posting source.
pub struct TextIndex {
    /// `(token, posting_offset, posting_len)`, sorted by token.
    tokens: Vec<(String, u64, u64)>,
    postings: Postings,
}

impl TextIndex {
    /// Parse a whole section (local): token table + the full postings blob resident.
    pub fn from_section(section: &[u8], codec: u8) -> Result<Self, FileError> {
        let (tokens, postings_start) = parse_token_table(section, codec)?;
        let postings = section
            .get(postings_start..)
            .ok_or(FileError::Container("text-index postings overrun"))?
            .to_vec();
        Ok(TextIndex {
            tokens,
            postings: Postings::Resident(postings),
        })
    }

    /// Build from a section **prefix** holding the token table, plus a loader that
    /// fetches a posting `(offset_within_postings_blob, len)` on demand (remote).
    pub fn from_token_table(
        prefix: &[u8],
        codec: u8,
        loader: Box<dyn Fn(u64, u64) -> Option<Vec<u8>> + Send + Sync>,
    ) -> Result<Self, FileError> {
        let (tokens, _postings_start) = parse_token_table(prefix, codec)?;
        Ok(TextIndex {
            tokens,
            postings: Postings::Remote(loader),
        })
    }

    /// Byte offset of the postings blob within the section (token-table end).
    /// The remote opener uses this to base its posting-range loader.
    pub fn postings_base(section_prefix: &[u8]) -> Option<usize> {
        let (len, n) = read_uvarint(section_prefix)?;
        Some(n + len as usize)
    }

    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    fn posting_for(&self, off: u64, len: u64) -> Option<Vec<u8>> {
        match &self.postings {
            Postings::Resident(blob) => blob
                .get(off as usize..(off + len) as usize)
                .map(<[u8]>::to_vec),
            Postings::Remote(load) => load(off, len),
        }
    }

    /// Subjects whose literals contain the exact `token` (case-insensitive — the
    /// caller passes a [`tokenize`]d word), sorted ascending.
    pub fn lookup(&self, token: &str) -> Vec<u32> {
        let i = self.tokens.partition_point(|(t, _, _)| t.as_str() < token);
        match self.tokens.get(i) {
            Some((t, off, len)) if t == token => self
                .posting_for(*off, *len)
                .map(|b| decode_postings(&b))
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Subjects whose literals contain a word that CONTAINS `piece` as a
    /// substring — the union over every matching table token. The token table
    /// is always resident so the scan is in-memory; each matching token costs
    /// one posting read (a range fetch on the remote path), so an unselective
    /// piece matching more than [`SUBSTRING_TOKEN_CAP`] tokens returns `None`
    /// and the caller falls back to its non-indexed path.
    pub fn substring(&self, piece: &str) -> Option<Vec<u32>> {
        let matching: Vec<&(String, u64, u64)> = self
            .tokens
            .iter()
            .filter(|(t, _, _)| t.contains(piece))
            .collect();
        if matching.len() > SUBSTRING_TOKEN_CAP {
            return None;
        }
        let mut out = BTreeSet::new();
        for (_, off, len) in matching {
            if let Some(b) = self.posting_for(*off, *len) {
                out.extend(decode_postings(&b));
            }
        }
        Some(out.into_iter().collect())
    }

    /// Subjects whose literals contain a word **starting with** `prefix` — the
    /// union over every token in the `prefix…` range. Sorted, deduped.
    pub fn prefix(&self, prefix: &str) -> Vec<u32> {
        let start = self.tokens.partition_point(|(t, _, _)| t.as_str() < prefix);
        let mut out = BTreeSet::new();
        for (t, off, len) in &self.tokens[start..] {
            if !t.starts_with(prefix) {
                break;
            }
            if let Some(b) = self.posting_for(*off, *len) {
                out.extend(decode_postings(&b));
            }
        }
        out.into_iter().collect()
    }
}

/// Parse the token table from a section (or a prefix covering it). Returns the
/// `(token, off, len)` list and the byte offset where the postings blob begins.
#[allow(clippy::type_complexity)]
fn parse_token_table(
    bytes: &[u8],
    codec: u8,
) -> Result<(Vec<(String, u64, u64)>, usize), FileError> {
    let (ttlen, n) = read_uvarint(bytes).ok_or(FileError::Container("truncated text-index len"))?;
    let end = n
        .checked_add(ttlen as usize)
        .filter(|&e| e <= bytes.len())
        .ok_or(FileError::Container("text-index token table overruns"))?;
    let tt = decompress(codec, &bytes[n..end])?;

    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Option<u64> {
        let (v, k) = read_uvarint(tt.get(*pos..)?)?;
        *pos += k;
        Some(v)
    };
    let count = take(&mut pos).ok_or(FileError::Container("truncated token count"))? as usize;
    let mut tokens: Vec<(String, u64, u64)> = Vec::with_capacity(count.min(tt.len()));
    let mut prev = String::new();
    for _ in 0..count {
        let shared = take(&mut pos).ok_or(FileError::Container("truncated token"))? as usize;
        let slen = take(&mut pos).ok_or(FileError::Container("truncated token"))? as usize;
        let sstart = pos;
        let send = sstart
            .checked_add(slen)
            .filter(|&e| e <= tt.len())
            .ok_or(FileError::Container("token suffix overruns"))?;
        let suffix = &tt[sstart..send];
        pos = send;
        let off = take(&mut pos).ok_or(FileError::Container("truncated posting off"))?;
        let len = take(&mut pos).ok_or(FileError::Container("truncated posting len"))?;
        let base = prev.get(..shared.min(prev.len())).unwrap_or(&prev);
        let mut token = String::with_capacity(base.len() + suffix.len());
        token.push_str(base);
        token.push_str(
            std::str::from_utf8(suffix).map_err(|_| FileError::Container("bad token utf8"))?,
        );
        tokens.push((token.clone(), off, len));
        prev = token;
    }
    Ok((tokens, end))
}

/// Decode a posting list: `count` then `count` delta-varint ascending subject ids.
fn decode_postings(bytes: &[u8]) -> Vec<u32> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Option<u64> {
        let (v, k) = read_uvarint(bytes.get(*pos..)?)?;
        *pos += k;
        Some(v)
    };
    let Some(count) = take(&mut pos) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity((count as usize).min(bytes.len()));
    let mut prev = 0u32;
    for _ in 0..count {
        let Some(d) = take(&mut pos) else { break };
        prev = prev.wrapping_add(d as u32);
        out.push(prev);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CODEC_NONE;

    #[test]
    fn tokenize_splits_lowercases_and_drops_short() {
        // Splits on non-alphanumerics, lowercases, drops 1-char tokens ("D", "6", "β").
        let got: Vec<String> = tokenize("Alpha-D-Glucose, 6-phosphate (β)").collect();
        assert_eq!(
            got.iter().map(String::as_str).collect::<Vec<_>>(),
            ["alpha", "glucose", "phosphate"]
        );
    }

    #[test]
    fn round_trip_lookup_prefix_and_and() {
        let mut b = TextIndexBuilder::new();
        b.add_text("alpha-D-glucose", 10);
        b.add_text("glucose 6-phosphate", 11);
        b.add_text("benzene ring", 12);
        let bytes = b.build(CODEC_NONE);
        let idx = TextIndex::from_section(&bytes, CODEC_NONE).unwrap();

        assert_eq!(idx.lookup("glucose"), vec![10, 11]);
        assert_eq!(idx.lookup("benzene"), vec![12]);
        assert!(idx.lookup("missing").is_empty());
        // prefix unions tokens (alpha, …) — here just "alpha".
        assert_eq!(idx.prefix("alph"), vec![10]);
        // token-prefix that spans several: "ph" → "phosphate".
        assert_eq!(idx.prefix("phos"), vec![11]);
    }

    #[test]
    fn remote_loader_fetches_only_the_queried_posting() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;
        let mut b = TextIndexBuilder::new();
        b.add_text("glucose here", 1);
        b.add_text("benzene there", 2);
        let bytes = b.build(CODEC_NONE);
        let base = TextIndex::postings_base(&bytes).unwrap();
        let postings = bytes[base..].to_vec();

        let calls = Arc::new(AtomicUsize::new(0));
        let (p, c) = (postings.clone(), calls.clone());
        let loader = Box::new(move |off: u64, len: u64| {
            c.fetch_add(1, SeqCst);
            p.get(off as usize..(off + len) as usize)
                .map(<[u8]>::to_vec)
        });
        // Token table prefix = everything up to the postings blob.
        let idx = TextIndex::from_token_table(&bytes[..base], CODEC_NONE, loader).unwrap();
        assert_eq!(idx.lookup("glucose"), vec![1]);
        assert_eq!(calls.load(SeqCst), 1, "one posting fetched");
        assert!(idx.lookup("nope").is_empty());
        assert_eq!(calls.load(SeqCst), 1, "an absent token fetches nothing");
    }
}
