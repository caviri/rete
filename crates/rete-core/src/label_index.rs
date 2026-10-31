//! The **LABEL_INDEX** section (kind 8, SPEC §6.4): every labelled subject's
//! display label, sorted for prefix search, in a multi-level table that a
//! remote reader walks with a handful of small range reads.
//!
//! It replaces the label block that used to sit inside the pyramid-meta
//! (`meta.rs` `LABELIDX_V1`). That block was fetched whole with the pyramid, so
//! it had to stay small — 8,192 entries, ranked by subject out-degree — and a
//! file without a pyramid had none. This section has no cap and no ranking, is
//! written by any build that asks for it (`rete build --label-index`, including
//! `--no-pyramid` and the memory-bounded build), and costs a first search one
//! section head, one directory node per level and one data block.
//!
//! ## Layout
//!
//! ```text
//! off  size
//! 0    16   digest: blake3(section[16..])[..16]  (the section is outside the header hash)
//! 16   1    version (= 1)
//! 17   1    levels: directory levels below the root (0 = the root lists data blocks)
//! 18   2    reserved (0)
//! 20   8    entry_count, u64 LE
//! 28   4    root_len, u32 LE
//! 32   root_len         root node (block codec)
//! 32+root_len …         body: data blocks, then the directory nodes level by level;
//!                       every offset in a node is relative to the body start
//! node  (decompressed): varint n; per child: varint shared, varint suffix_len, suffix
//!                       (the child's first key, front-coded), varint off, varint len, varint entries
//! block (decompressed): varint n; per entry: varint shared, varint suffix_len, suffix  (label)
//!                                            varint shared, varint suffix_len, suffix  (subject term)
//! ```
//!
//! An entry's **key** is its label lowercased ([`label_key`]); entries are sorted
//! by `(key, subject)`, one per `(subject, key)`. The subject is stored as its
//! dictionary term, so a search never reads the dictionary.
//!
//! ## Which labels (index-only; the triples are never changed)
//!
//! - **L1**, every literal of [`DIRECT_LABEL_PREDICATES`].
//! - **L2**, for a subject with no L1 label only: `"family, given"` from the
//!   smallest value of [`FAMILY_NAME_PREDICATES`] and of [`GIVEN_NAME_PREDICATES`]
//!   (or whichever of the two exists) — people who carry only `foaf:givenName`
//!   / `foaf:familyName` get a label.
//! - Normalized by [`normalize_label`]: escapes resolved, whitespace collapsed,
//!   trimmed, truncated to [`MAX_LABEL_BYTES`].

use std::collections::HashMap;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::file::{compress, decompress, FileError};
use crate::varint::{read_uvarint, write_uvarint};

/// The predicates whose literals are a subject's display label (rule L1). The
/// same list the pyramid's label block used. Angle-bracketed, as stored.
pub const DIRECT_LABEL_PREDICATES: &[&str] = &[
    "<http://www.w3.org/2000/01/rdf-schema#label>",
    "<http://www.w3.org/2004/02/skos/core#prefLabel>",
    "<http://www.w3.org/2004/02/skos/core#altLabel>",
    "<http://xmlns.com/foaf/0.1/name>",
    "<http://purl.org/dc/terms/title>",
    "<http://purl.org/dc/elements/1.1/title>",
    "<http://schema.org/name>",
];

/// Family-name predicates for rule L2 (composed person names).
pub const FAMILY_NAME_PREDICATES: &[&str] = &[
    "<http://xmlns.com/foaf/0.1/familyName>",
    "<http://xmlns.com/foaf/0.1/lastName>",
    "<http://xmlns.com/foaf/0.1/surname>",
    "<http://xmlns.com/foaf/0.1/family_name>",
    "<http://schema.org/familyName>",
    "<http://www.w3.org/2006/vcard/ns#family-name>",
];

/// Given-name predicates for rule L2 (composed person names).
pub const GIVEN_NAME_PREDICATES: &[&str] = &[
    "<http://xmlns.com/foaf/0.1/givenName>",
    "<http://xmlns.com/foaf/0.1/firstName>",
    "<http://xmlns.com/foaf/0.1/givenname>",
    "<http://schema.org/givenName>",
    "<http://www.w3.org/2006/vcard/ns#given-name>",
];

/// Longest label stored, in bytes; longer ones are cut at a char boundary.
pub const MAX_LABEL_BYTES: usize = 512;

/// Section format version written by this crate.
const VERSION: u8 = 1;
/// Bytes before the root node.
pub(crate) const PREAMBLE_LEN: usize = 32;
/// Length of the leading digest.
const DIGEST_LEN: usize = 16;
/// Uncompressed size a data block is closed at.
const BLOCK_TARGET: usize = 16 * 1024;
/// Most children in one directory node (and in the root).
const NODE_FANOUT: usize = 1024;
/// What a lazy open reads first: the preamble and, normally, the whole root.
pub(crate) const HEAD_READ: u64 = 64 * 1024;
/// Most decoded directory nodes a lazy index keeps.
const NODE_CACHE_CAP: usize = 256;
/// Most directory levels a reader accepts. Five levels of 1,024 children over
/// 16 KiB blocks address far more than any file can hold; a larger value is a
/// crafted or corrupt preamble.
const MAX_LEVELS: u8 = 5;
/// Most nodes and blocks one search may fetch. A well-formed index needs one
/// node per level plus the blocks of the matching run; the bound stops a
/// crafted directory (children pointing back at their parent) from turning a
/// search into an unbounded walk.
const MAX_FETCHES_PER_SEARCH: usize = 1 << 16;

/// The search key of a label: its Unicode lowercase form.
pub fn label_key(label: &str) -> String {
    label.to_lowercase()
}

/// Normalize a literal's lexical value into a stored label: whitespace runs
/// collapsed to one space, trimmed, cut to [`MAX_LABEL_BYTES`] at a char
/// boundary. `None` for an empty result.
pub fn normalize_label(lexical: &str) -> Option<String> {
    let mut out = String::with_capacity(lexical.len().min(MAX_LABEL_BYTES));
    for word in lexical.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
        if out.len() >= MAX_LABEL_BYTES {
            break;
        }
    }
    if out.len() > MAX_LABEL_BYTES {
        let mut cut = MAX_LABEL_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        while out.ends_with(' ') {
            out.pop();
        }
    }
    (!out.is_empty()).then_some(out)
}

/// What a label-candidate value is, by its predicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Slot {
    /// An L1 label.
    Direct = 0,
    /// An L2 family name.
    Family = 1,
    /// An L2 given name.
    Given = 2,
}

impl Slot {
    /// Read back from the memory-bounded build's spill (native only).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn from_u8(v: u8) -> Option<Slot> {
        match v {
            0 => Some(Slot::Direct),
            1 => Some(Slot::Family),
            2 => Some(Slot::Given),
            _ => None,
        }
    }
}

/// The [`Slot`] a predicate term feeds, if any.
pub(crate) fn slot_of_predicate(pred: &str) -> Option<Slot> {
    if DIRECT_LABEL_PREDICATES.contains(&pred) {
        Some(Slot::Direct)
    } else if FAMILY_NAME_PREDICATES.contains(&pred) {
        Some(Slot::Family)
    } else if GIVEN_NAME_PREDICATES.contains(&pred) {
        Some(Slot::Given)
    } else {
        None
    }
}

/// The normalized label value of an object term, when it is a literal.
pub(crate) fn candidate_value(object_term: &str) -> Option<String> {
    let lex = crate::terms::literal_lexical(object_term)?;
    normalize_label(&lex)
}

/// One subject's candidate values, gathered before the rules run.
#[derive(Debug, Default)]
pub(crate) struct SubjectCandidates {
    pub direct: Vec<String>,
    pub family: Vec<String>,
    pub given: Vec<String>,
}

impl SubjectCandidates {
    pub(crate) fn push(&mut self, slot: Slot, value: String) {
        match slot {
            Slot::Direct => self.direct.push(value),
            Slot::Family => self.family.push(value),
            Slot::Given => self.given.push(value),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.direct.is_empty() && self.family.is_empty() && self.given.is_empty()
    }

    /// Apply L1/L2: the `(key, label)` pairs this subject is indexed under, one
    /// per key, keeping the smallest label for a key. Order-independent.
    pub(crate) fn labels(&self) -> Vec<(String, String)> {
        let mut by_key: std::collections::BTreeMap<String, String> = Default::default();
        if !self.direct.is_empty() {
            for v in &self.direct {
                let k = label_key(v);
                match by_key.get_mut(&k) {
                    Some(cur) if v.as_str() < cur.as_str() => *cur = v.clone(),
                    Some(_) => {}
                    None => {
                        by_key.insert(k, v.clone());
                    }
                }
            }
        } else {
            let family = self.family.iter().min();
            let given = self.given.iter().min();
            let composed = match (family, given) {
                (Some(f), Some(g)) => format!("{f}, {g}"),
                (Some(f), None) => f.clone(),
                (None, Some(g)) => g.clone(),
                (None, None) => return Vec::new(),
            };
            if let Some(label) = normalize_label(&composed) {
                by_key.insert(label_key(&label), label);
            }
        }
        by_key.into_iter().collect()
    }
}

/// The label entries of an in-memory graph: `(key, subject term, label)`,
/// sorted by `(key, subject)`. `triples` must be the **deduplicated**
/// default-graph id-triples **sorted by subject** (SPO order), which is what
/// lets one pass group each subject's candidates.
pub(crate) fn entries_from_sorted_triples(
    dict: &crate::Dictionary,
    triples: &[(u32, u32, u32)],
) -> Vec<(String, String, String)> {
    let mut slot_of: HashMap<u32, Slot> = HashMap::new();
    for list in [
        DIRECT_LABEL_PREDICATES,
        FAMILY_NAME_PREDICATES,
        GIVEN_NAME_PREDICATES,
    ] {
        for p in list {
            if let (Some(pid), Some(slot)) = (dict.predicate_id(p), slot_of_predicate(p)) {
                slot_of.insert(pid, slot);
            }
        }
    }
    let mut out: Vec<(String, String, String)> = Vec::new();
    if slot_of.is_empty() {
        return out;
    }
    let mut cur: Option<u32> = None;
    let mut cands = SubjectCandidates::default();
    let flush = |s: u32, cands: &mut SubjectCandidates, out: &mut Vec<_>| {
        if cands.is_empty() {
            return;
        }
        if let Some(subject) = dict.subject_term(s) {
            for (key, label) in cands.labels() {
                out.push((key, subject.clone(), label));
            }
        }
        *cands = SubjectCandidates::default();
    };
    for &(s, p, o) in triples {
        if cur != Some(s) {
            if let Some(prev) = cur {
                flush(prev, &mut cands, &mut out);
            }
            cur = Some(s);
        }
        let Some(&slot) = slot_of.get(&p) else {
            continue;
        };
        if let Some(v) = dict.object_term(o).and_then(|t| candidate_value(&t)) {
            cands.push(slot, v);
        }
    }
    if let Some(prev) = cur {
        flush(prev, &mut cands, &mut out);
    }
    out.sort_unstable_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    out.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1);
    out
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// One directory entry: a child's first key, its byte range in the body, and
/// how many entries it covers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DirEntry {
    first_key: String,
    off: u64,
    len: u64,
    entries: u64,
}

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

fn push_front_coded(out: &mut Vec<u8>, prev: &str, cur: &str) {
    let shared = shared_prefix(prev, cur);
    write_uvarint(out, shared as u64);
    let suffix = &cur.as_bytes()[shared..];
    write_uvarint(out, suffix.len() as u64);
    out.extend_from_slice(suffix);
}

fn encode_node(children: &[DirEntry]) -> Vec<u8> {
    let mut raw = Vec::new();
    write_uvarint(&mut raw, children.len() as u64);
    let mut prev = "";
    for c in children {
        push_front_coded(&mut raw, prev, &c.first_key);
        write_uvarint(&mut raw, c.off);
        write_uvarint(&mut raw, c.len);
        write_uvarint(&mut raw, c.entries);
        prev = &c.first_key;
    }
    raw
}

/// Streams sorted entries into a LABEL_INDEX section. Data blocks go to the
/// `body` sink as they fill; the directory (≈ 40 B per 16 KiB block) stays in
/// memory until [`finish`](Self::finish).
pub(crate) struct LabelIndexWriter<W: Write> {
    codec: u8,
    body: W,
    body_len: u64,
    block: Vec<u8>,
    block_n: u64,
    block_first_key: Option<String>,
    prev_label: String,
    prev_subject: String,
    last: Option<(String, String)>,
    leaves: Vec<DirEntry>,
    entry_count: u64,
}

/// A finished section, split so the external build can stream its body from
/// disk: `head` is bytes `16..32 + root_len` (everything but the digest and
/// the body).
pub(crate) struct LabelIndexParts<W> {
    pub head: Vec<u8>,
    pub body: W,
    /// Read by the memory-bounded build (native only), to skip an empty section.
    #[cfg(not(target_arch = "wasm32"))]
    pub entry_count: u64,
}

impl<W: Write> LabelIndexWriter<W> {
    pub(crate) fn new(codec: u8, body: W) -> Self {
        LabelIndexWriter {
            codec,
            body,
            body_len: 0,
            block: Vec::new(),
            block_n: 0,
            block_first_key: None,
            prev_label: String::new(),
            prev_subject: String::new(),
            last: None,
            leaves: Vec::new(),
            entry_count: 0,
        }
    }

    /// Add one entry. Entries must arrive in strictly increasing
    /// `(key, subject)` order; a repeat of the previous `(key, subject)` is
    /// dropped.
    pub(crate) fn push(&mut self, key: &str, subject: &str, label: &str) -> std::io::Result<()> {
        if let Some((lk, ls)) = &self.last {
            match (lk.as_str(), ls.as_str()).cmp(&(key, subject)) {
                std::cmp::Ordering::Equal => return Ok(()),
                std::cmp::Ordering::Greater => {
                    return Err(std::io::Error::other("label entries out of order"))
                }
                std::cmp::Ordering::Less => {}
            }
        }
        self.last = Some((key.to_string(), subject.to_string()));
        if self.block_first_key.is_none() {
            self.block_first_key = Some(key.to_string());
            self.prev_label.clear();
            self.prev_subject.clear();
        }
        push_front_coded(&mut self.block, &self.prev_label, label);
        push_front_coded(&mut self.block, &self.prev_subject, subject);
        self.prev_label.clear();
        self.prev_label.push_str(label);
        self.prev_subject.clear();
        self.prev_subject.push_str(subject);
        self.block_n += 1;
        self.entry_count += 1;
        if self.block.len() >= BLOCK_TARGET {
            self.close_block()?;
        }
        Ok(())
    }

    fn close_block(&mut self) -> std::io::Result<()> {
        let Some(first_key) = self.block_first_key.take() else {
            return Ok(());
        };
        let mut raw = Vec::with_capacity(self.block.len() + 10);
        write_uvarint(&mut raw, self.block_n);
        raw.extend_from_slice(&self.block);
        let bytes = compress(self.codec, &raw);
        self.body.write_all(&bytes)?;
        self.leaves.push(DirEntry {
            first_key,
            off: self.body_len,
            len: bytes.len() as u64,
            entries: self.block_n,
        });
        self.body_len += bytes.len() as u64;
        self.block.clear();
        self.block_n = 0;
        Ok(())
    }

    /// Close the last block, write the directory levels, and return the parts.
    pub(crate) fn finish(mut self) -> std::io::Result<LabelIndexParts<W>> {
        self.close_block()?;
        let mut level = std::mem::take(&mut self.leaves);
        let mut levels = 0u8;
        while level.len() > NODE_FANOUT {
            let mut parents = Vec::with_capacity(level.len() / NODE_FANOUT + 1);
            for group in level.chunks(NODE_FANOUT) {
                let bytes = compress(self.codec, &encode_node(group));
                self.body.write_all(&bytes)?;
                parents.push(DirEntry {
                    first_key: group[0].first_key.clone(),
                    off: self.body_len,
                    len: bytes.len() as u64,
                    entries: group.iter().map(|c| c.entries).sum(),
                });
                self.body_len += bytes.len() as u64;
            }
            level = parents;
            levels += 1;
        }
        let root = compress(self.codec, &encode_node(&level));
        let mut head = Vec::with_capacity(PREAMBLE_LEN - DIGEST_LEN + root.len());
        head.push(VERSION);
        head.push(levels);
        head.extend_from_slice(&[0, 0]);
        head.extend_from_slice(&self.entry_count.to_le_bytes());
        head.extend_from_slice(&(root.len() as u32).to_le_bytes());
        head.extend_from_slice(&root);
        Ok(LabelIndexParts {
            head,
            body: self.body,
            #[cfg(not(target_arch = "wasm32"))]
            entry_count: self.entry_count,
        })
    }
}

/// Encode a complete LABEL_INDEX section from sorted `(key, subject, label)`
/// entries. Empty input yields an empty `Vec` (no section).
pub(crate) fn encode_section(entries: &[(String, String, String)], codec: u8) -> Vec<u8> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut w = LabelIndexWriter::new(codec, Vec::new());
    for (k, s, l) in entries {
        w.push(k, s, l).expect("in-memory label index write");
    }
    let parts = w.finish().expect("in-memory label index write");
    seal(&parts.head, &parts.body)
}

/// Prepend the digest to `head ++ body`.
pub(crate) fn seal(head: &[u8], body: &[u8]) -> Vec<u8> {
    let mut h = blake3::Hasher::new();
    h.update(head);
    h.update(body);
    let mut out = Vec::with_capacity(DIGEST_LEN + head.len() + body.len());
    out.extend_from_slice(&h.finalize().as_bytes()[..DIGEST_LEN]);
    out.extend_from_slice(head);
    out.extend_from_slice(body);
    out
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// The fixed preamble of a LABEL_INDEX section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelIndexPreamble {
    /// Directory levels below the root.
    pub levels: u8,
    /// Number of `(subject, label)` entries.
    pub entry_count: u64,
    /// Byte length of the root node.
    pub root_len: u32,
}

impl LabelIndexPreamble {
    /// Parse the first `PREAMBLE_LEN` bytes of a section.
    pub fn parse(bytes: &[u8]) -> Result<Self, FileError> {
        if bytes.len() < PREAMBLE_LEN {
            return Err(FileError::Container("label index: truncated preamble"));
        }
        if bytes[16] != VERSION {
            return Err(FileError::Container("label index: unknown version"));
        }
        if bytes[17] > MAX_LEVELS {
            return Err(FileError::Container("label index: too many levels"));
        }
        Ok(LabelIndexPreamble {
            levels: bytes[17],
            entry_count: u64::from_le_bytes(bytes[20..28].try_into().unwrap()),
            root_len: u32::from_le_bytes(bytes[28..32].try_into().unwrap()),
        })
    }
}

/// Fetches `(offset, len)` within the section.
type SectionLoader = Box<dyn Fn(u64, u64) -> Option<Vec<u8>> + Send + Sync>;

enum Source {
    Resident(Arc<[u8]>),
    Lazy(SectionLoader),
}

/// A parsed LABEL_INDEX: the root resident, inner nodes and blocks on demand.
pub struct LabelIndex {
    preamble: LabelIndexPreamble,
    root: Vec<DirEntry>,
    body_base: u64,
    section_len: u64,
    codec: u8,
    source: Source,
    nodes: Mutex<HashMap<u64, Arc<Vec<DirEntry>>>>,
}

impl std::fmt::Debug for LabelIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelIndex")
            .field("preamble", &self.preamble)
            .field("section_len", &self.section_len)
            .finish()
    }
}

/// Did a search reach its end, or must it go on?
enum Walk {
    Continue,
    Stop,
}

impl LabelIndex {
    /// Parse a whole section held in memory, checking its digest.
    pub fn from_section(section: &[u8], codec: u8) -> Result<Self, FileError> {
        if !digest_ok(section) {
            return Err(FileError::Container("label index: digest mismatch"));
        }
        let bytes: Arc<[u8]> = Arc::from(section);
        Self::open(
            &bytes[..],
            section.len() as u64,
            codec,
            Source::Resident(bytes.clone()),
        )
    }

    /// Open from the section's leading bytes (`head`, at least the preamble;
    /// normally `HEAD_READ` bytes) and a loader for any range of the section.
    /// The root is read from `head` when it fits, else fetched.
    pub fn from_head(
        head: &[u8],
        section_len: u64,
        codec: u8,
        loader: Box<dyn Fn(u64, u64) -> Option<Vec<u8>> + Send + Sync>,
    ) -> Result<Self, FileError> {
        Self::open(head, section_len, codec, Source::Lazy(loader))
    }

    fn open(head: &[u8], section_len: u64, codec: u8, source: Source) -> Result<Self, FileError> {
        let preamble = LabelIndexPreamble::parse(head)?;
        let root_end = PREAMBLE_LEN as u64 + preamble.root_len as u64;
        if root_end > section_len {
            return Err(FileError::Container("label index: root overruns section"));
        }
        let root_bytes: Vec<u8> = if root_end as usize <= head.len() {
            head[PREAMBLE_LEN..root_end as usize].to_vec()
        } else {
            match &source {
                Source::Resident(b) => b
                    .get(PREAMBLE_LEN..root_end as usize)
                    .ok_or(FileError::Container("label index: root overruns section"))?
                    .to_vec(),
                Source::Lazy(load) => load(PREAMBLE_LEN as u64, preamble.root_len as u64)
                    .ok_or(FileError::Container("label index: root unreadable"))?,
            }
        };
        let root = decode_node(&decompress(codec, &root_bytes)?)?;
        Ok(LabelIndex {
            preamble,
            root,
            body_base: root_end,
            section_len,
            codec,
            source,
            nodes: Mutex::new(HashMap::new()),
        })
    }

    /// Where the data blocks end in the body (the directory nodes follow
    /// them, lowest level first): the offset of the leftmost level-1 node, or
    /// the whole body on a one-level index. `fetch` reads `(offset, len)`
    /// relative to the **section** start. What the `0x06` writer splits on.
    pub(crate) fn v05_data_len(&self, fetch: &dyn Fn(u64, u64) -> Option<Vec<u8>>) -> Option<u64> {
        if self.preamble.levels == 0 {
            return Some(self.section_len - self.body_base);
        }
        let mut e = self.root.first()?.clone();
        for _ in 1..self.preamble.levels {
            let raw = fetch(self.body_base + e.off, e.len)?;
            let node = decode_node(&decompress(self.codec, &raw).ok()?).ok()?;
            e = node.first()?.clone();
        }
        Some(e.off)
    }

    /// Number of entries in the index.
    pub fn entry_count(&self) -> u64 {
        self.preamble.entry_count
    }

    /// Byte length of the whole section.
    pub fn section_len(&self) -> u64 {
        self.section_len
    }

    fn fetch(&self, body_off: u64, len: u64) -> Option<Vec<u8>> {
        let off = self.body_base.checked_add(body_off)?;
        let end = off.checked_add(len)?;
        if end > self.section_len {
            return None;
        }
        match &self.source {
            Source::Resident(b) => b.get(off as usize..end as usize).map(<[u8]>::to_vec),
            Source::Lazy(load) => load(off, len),
        }
    }

    fn node(&self, e: &DirEntry) -> Option<Arc<Vec<DirEntry>>> {
        if let Some(n) = self.nodes.lock().ok()?.get(&e.off) {
            return Some(n.clone());
        }
        let raw = decompress(self.codec, &self.fetch(e.off, e.len)?).ok()?;
        let node = Arc::new(decode_node(&raw).ok()?);
        if let Ok(mut cache) = self.nodes.lock() {
            if cache.len() >= NODE_CACHE_CAP {
                cache.clear();
            }
            cache.insert(e.off, node.clone());
        }
        Some(node)
    }

    /// Entries whose label starts with `prefix`, compared case-insensitively,
    /// as `(label, subject term)` in key order, at most `limit`. An empty
    /// `prefix` lists the index from the start.
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Vec<(String, String)> {
        let needle = label_key(prefix);
        let mut out = Vec::new();
        if limit == 0 {
            return out;
        }
        let root = self.root.clone();
        let mut budget = MAX_FETCHES_PER_SEARCH;
        let _ = self.walk(
            &root,
            self.preamble.levels,
            &needle,
            limit,
            &mut out,
            &mut budget,
        );
        out
    }

    fn walk(
        &self,
        children: &[DirEntry],
        depth: u8,
        needle: &str,
        limit: usize,
        out: &mut Vec<(String, String)>,
        budget: &mut usize,
    ) -> Walk {
        // The run of keys starting with `needle` begins in the last child whose
        // first key is <= needle (or the first child).
        let start = children
            .partition_point(|c| c.first_key.as_str() <= needle)
            .saturating_sub(1);
        for child in &children[start..] {
            if child.first_key.as_str() > needle && !child.first_key.starts_with(needle) {
                return Walk::Stop;
            }
            if *budget == 0 {
                return Walk::Stop;
            }
            *budget -= 1;
            let step = if depth == 0 {
                self.scan_block(child, needle, limit, out)
            } else {
                match self.node(child) {
                    Some(node) => self.walk(&node, depth - 1, needle, limit, out, budget),
                    None => Walk::Stop,
                }
            };
            if let Walk::Stop = step {
                return Walk::Stop;
            }
        }
        Walk::Continue
    }

    fn scan_block(
        &self,
        e: &DirEntry,
        needle: &str,
        limit: usize,
        out: &mut Vec<(String, String)>,
    ) -> Walk {
        let Some(raw) = self
            .fetch(e.off, e.len)
            .and_then(|b| decompress(self.codec, &b).ok())
        else {
            return Walk::Stop;
        };
        let Ok(entries) = decode_block(&raw) else {
            return Walk::Stop;
        };
        for (label, subject) in entries {
            let key = label_key(&label);
            if key.starts_with(needle) {
                out.push((label, subject));
                if out.len() >= limit {
                    return Walk::Stop;
                }
            } else if key.as_str() > needle {
                return Walk::Stop;
            }
        }
        Walk::Continue
    }
}

/// Does a section's leading digest match the rest of it?
pub(crate) fn digest_ok(section: &[u8]) -> bool {
    if section.len() < DIGEST_LEN {
        return false;
    }
    blake3::hash(&section[DIGEST_LEN..]).as_bytes()[..DIGEST_LEN] == section[..DIGEST_LEN]
}

struct Cursor<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn varint(&mut self) -> Result<u64, FileError> {
        let (v, n) = read_uvarint(self.b.get(self.pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("label index: truncated varint"))?;
        self.pos += n;
        Ok(v)
    }

    fn front_coded(&mut self, prev: &str) -> Result<String, FileError> {
        let shared = self.varint()? as usize;
        let len = self.varint()? as usize;
        let end = self
            .pos
            .checked_add(len)
            .filter(|&e| e <= self.b.len())
            .ok_or(FileError::Container("label index: string overruns"))?;
        let base = prev
            .get(..shared)
            .ok_or(FileError::Container("label index: bad shared prefix"))?;
        let suffix = std::str::from_utf8(&self.b[self.pos..end])
            .map_err(|_| FileError::Container("label index: bad utf8"))?;
        self.pos = end;
        let mut s = String::with_capacity(base.len() + suffix.len());
        s.push_str(base);
        s.push_str(suffix);
        Ok(s)
    }
}

fn decode_node(raw: &[u8]) -> Result<Vec<DirEntry>, FileError> {
    let mut c = Cursor { b: raw, pos: 0 };
    let n = c.varint()? as usize;
    let mut out = Vec::with_capacity(n.min(raw.len()));
    let mut prev = String::new();
    for _ in 0..n {
        let first_key = c.front_coded(&prev)?;
        let off = c.varint()?;
        let len = c.varint()?;
        let entries = c.varint()?;
        prev.clone_from(&first_key);
        out.push(DirEntry {
            first_key,
            off,
            len,
            entries,
        });
    }
    Ok(out)
}

fn decode_block(raw: &[u8]) -> Result<Vec<(String, String)>, FileError> {
    let mut c = Cursor { b: raw, pos: 0 };
    let n = c.varint()? as usize;
    let mut out = Vec::with_capacity(n.min(raw.len()));
    let (mut pl, mut ps) = (String::new(), String::new());
    for _ in 0..n {
        let label = c.front_coded(&pl)?;
        let subject = c.front_coded(&ps)?;
        pl.clone_from(&label);
        ps.clone_from(&subject);
        out.push((label, subject));
    }
    Ok(out)
}

/// Entry count of a file's LABEL_INDEX, from its 32-byte preamble — one
/// range read, never the index. `None` when the file has no such section or
/// its preamble is unreadable. The card's `signals.label_index` uses this.
pub fn read_label_index_entries_ranged<R: crate::RangeReader + ?Sized>(
    reader: &R,
    header: &crate::Header,
) -> Option<u64> {
    let s = header.label_index_section()?;
    let head = reader
        .read_at(s.offset, (PREAMBLE_LEN as u64).min(s.length))
        .ok()?;
    LabelIndexPreamble::parse(&head).ok().map(|p| p.entry_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::CODEC_NONE;

    fn entries(pairs: &[(&str, &str)]) -> Vec<(String, String, String)> {
        let mut v: Vec<_> = pairs
            .iter()
            .map(|(l, s)| (label_key(l), s.to_string(), l.to_string()))
            .collect();
        v.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        v
    }

    #[test]
    fn normalizes_whitespace_and_truncates() {
        assert_eq!(normalize_label("  a \n\t b  ").as_deref(), Some("a b"));
        assert_eq!(normalize_label(" \n "), None);
        let long = "é".repeat(400); // 800 bytes
        let n = normalize_label(&long).unwrap();
        assert!(n.len() <= MAX_LABEL_BYTES && n.is_char_boundary(n.len()));
    }

    #[test]
    fn composes_person_names_only_without_a_direct_label() {
        let mut c = SubjectCandidates::default();
        c.push(Slot::Family, "Svobodová".into());
        c.push(Slot::Given, "Jana".into());
        assert_eq!(
            c.labels(),
            vec![("svobodová, jana".to_string(), "Svobodová, Jana".to_string())]
        );
        c.push(Slot::Direct, "J. Svobodová".into());
        assert_eq!(c.labels().len(), 1);
        assert_eq!(c.labels()[0].1, "J. Svobodová");
        // one entry per key, keeping the smallest label
        let mut d = SubjectCandidates::default();
        d.push(Slot::Direct, "acta".into());
        d.push(Slot::Direct, "Acta".into());
        assert_eq!(d.labels(), vec![("acta".to_string(), "Acta".to_string())]);
        let mut f = SubjectCandidates::default();
        f.push(Slot::Family, "Smith".into());
        assert_eq!(f.labels()[0].1, "Smith");
    }

    #[test]
    fn round_trip_small_and_multilevel() {
        // Small: one block, root lists it.
        let small = entries(&[
            ("Acta Universitatis", "<a>"),
            ("acta u2", "<b>"),
            ("Beta", "<c>"),
        ]);
        let sec = encode_section(&small, CODEC_NONE);
        let idx = LabelIndex::from_section(&sec, CODEC_NONE).unwrap();
        assert_eq!(idx.entry_count(), 3);
        let hits = idx.prefix_search("ACTA U", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0].0, "acta u2",
            "key order: 'acta u2' < 'acta universitatis'"
        );
        assert!(idx.prefix_search("zz", 10).is_empty());
        assert_eq!(idx.prefix_search("", 2).len(), 2);
        assert!(idx.prefix_search("a", 0).is_empty());

        // Large: enough entries for more than NODE_FANOUT blocks → ≥ 1 level.
        let n = 400_000usize;
        let mut big: Vec<(String, String, String)> = (0..n)
            .map(|i| {
                let l = format!("Label {i:07} of a work with a reasonably long title");
                (label_key(&l), format!("<http://ex/s{i}>"), l)
            })
            .collect();
        big.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let sec = encode_section(&big, CODEC_NONE);
        let idx = LabelIndex::from_section(&sec, CODEC_NONE).unwrap();
        assert!(idx.preamble.levels >= 1, "multi-level: {:?}", idx.preamble);
        assert_eq!(idx.entry_count(), n as u64);
        let hits = idx.prefix_search("label 0123", 1000);
        assert_eq!(hits.len(), 1000);
        assert!(hits.iter().all(|(l, _)| l.starts_with("Label 0123")));
        let exact = idx.prefix_search("Label 0399999", 5);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].1, "<http://ex/s399999>");
        // the first and the last entries are reachable
        assert_eq!(idx.prefix_search("label 0000000", 1).len(), 1);
    }

    #[test]
    fn lazy_open_reads_a_bounded_head_then_one_node_per_level_and_a_block() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let n = 300_000usize;
        let mut big: Vec<(String, String, String)> = (0..n)
            .map(|i| {
                let l = format!("Name {i:07} {}", "x".repeat(i % 40));
                (label_key(&l), format!("<http://ex/p{i}>"), l)
            })
            .collect();
        big.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
        let sec = Arc::new(encode_section(&big, CODEC_NONE));
        let calls = Arc::new(AtomicUsize::new(0));
        let bytes = Arc::new(AtomicUsize::new(0));
        let (s2, c2, b2) = (sec.clone(), calls.clone(), bytes.clone());
        let loader = Box::new(move |off: u64, len: u64| {
            c2.fetch_add(1, SeqCst);
            b2.fetch_add(len as usize, SeqCst);
            s2.get(off as usize..(off + len) as usize)
                .map(<[u8]>::to_vec)
        });
        let head_len = (HEAD_READ as usize).min(sec.len());
        let idx =
            LabelIndex::from_head(&sec[..head_len], sec.len() as u64, CODEC_NONE, loader).unwrap();
        let levels = idx.preamble.levels as usize;
        assert_eq!(calls.load(SeqCst), 0, "the root came with the head");
        let hits = idx.prefix_search("name 0200000", 1);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            calls.load(SeqCst),
            levels + 1,
            "one node per level plus one block"
        );
        assert!(bytes.load(SeqCst) < 128 * 1024, "{}", bytes.load(SeqCst));
    }

    #[test]
    fn digest_detects_a_flipped_byte() {
        let sec = encode_section(&entries(&[("a", "<x>")]), CODEC_NONE);
        assert!(digest_ok(&sec));
        let mut bad = sec.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(!digest_ok(&bad));
        assert!(LabelIndex::from_section(&bad, CODEC_NONE).is_err());
    }

    #[test]
    fn writer_rejects_out_of_order_and_drops_repeats() {
        let mut w = LabelIndexWriter::new(CODEC_NONE, Vec::new());
        w.push("b", "<s>", "b").unwrap();
        w.push("b", "<s>", "B").unwrap(); // repeat of (key, subject): dropped
        assert!(w.push("a", "<s>", "a").is_err());
        let parts = w.finish().unwrap();
        assert_eq!(parts.entry_count, 1);
    }

    #[test]
    fn malformed_sections_error_cleanly() {
        let sec = encode_section(&entries(&[("a", "<x>"), ("b", "<y>")]), CODEC_NONE);
        for cut in [0, 10, 20, 31, 33] {
            let _ = LabelIndex::from_head(
                &sec[..cut.min(sec.len())],
                sec.len() as u64,
                CODEC_NONE,
                Box::new(|_, _| None),
            );
        }
        // a root_len pointing past the section
        let mut bad = sec.clone();
        bad[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(
            LabelIndex::from_head(&bad, bad.len() as u64, CODEC_NONE, Box::new(|_, _| None))
                .is_err()
        );
    }
}
