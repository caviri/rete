//! Full-text (whole-word) index over literals — `token → subjects`.
//!
//! Built at `rete build --text-index` and stored as the optional `TextIndex`
//! file section (SPEC §6.3). Every literal object in the index's **scope** is
//! tokenized into words; the index maps each word to the **sorted subject ids**
//! that carry it, so a reader can answer "which entities mention `glucose`?"
//! without scanning the literals — and a *remote* reader fetches only the
//! posting lists it queries.
//!
//! On-disk section layout:
//! ```text
//! varint  token_table_len
//! token table (compressed with the file's block codec):
//!   varint num_tokens
//!   per token (sorted): varint shared_prefix_len   # front-coded vs previous token
//!                       varint suffix_len, suffix bytes
//!                       varint posting_off, varint posting_len   # into the postings blob
//!   [scope block]       # optional, see below — absent for the default scope
//! postings blob (uncompressed, so a single posting range-reads directly):
//!   per token (same order): varint count, then `count` delta-varint subject ids
//! ```
//! The token table is small (distinct words) and read whole; the postings blob is
//! the bulk and is fetched one posting at a time on the remote path.
//!
//! ## Scope
//!
//! By default the index covers the literal objects of the **default graph**,
//! whatever their predicate — the only index any build wrote before 0.4.0. A
//! build may widen it to every named graph (`--text-index-graphs all`) or
//! narrow it to some predicates (`--text-index-include` / `--text-index-exclude`).
//! A non-default scope is recorded in a **scope block** after the last token
//! entry of the token table:
//!
//! ```text
//! u8     1                       # scope-block version
//! u8     graphs                  # 0 = default graph only, 1 = default + every named graph
//! u8     predicates              # 0 = every predicate, 1 = only those listed, 2 = all but those listed
//! varint n, then n × (varint len, bytes)   # predicate IRIs as N-Triples tokens `<…>`, ascending
//! ```
//!
//! The default scope writes no block, so its section is byte-identical to one
//! written before scopes existed, and a reader that predates the block stops
//! after the token entries and never sees it. The scope matters to SPARQL: the
//! `FILTER(CONTAINS(…))` pushdown treats the index as an over-approximation of
//! the subjects whose literals can match, which holds only for the graphs and
//! predicates the index covers. A **predicate-restricted** index is therefore
//! stored under its own section kind ([`crate::header::SectionKind::TextIndexPartial`]):
//! a reader older than that kind does not see the index at all and scans,
//! instead of pruning with an index that never saw the predicate it filters on.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};

use crate::file::{compress, decompress, FileError};
use crate::varint::{read_uvarint, write_uvarint};

/// Minimum token length kept (1-char words are low-value and bloat the index).
const MIN_TOKEN_LEN: usize = 2;

/// Most table tokens a [`TextIndex::substring`] piece may match before the
/// lookup declines (each match is a posting read — a range fetch remotely).
const SUBSTRING_TOKEN_CAP: usize = 512;

/// Version byte of the scope block this crate writes.
const SCOPE_BLOCK_VERSION: u8 = 1;

/// Split text into index/query tokens: Unicode-alphanumeric runs, lowercased,
/// length ≥ `MIN_TOKEN_LEN`. The build and query sides MUST use this same
/// function so a query word matches how it was indexed.
pub fn tokenize(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= MIN_TOKEN_LEN)
        .map(|t| t.to_lowercase())
}

/// Length of the longest shared **byte** prefix of `a` and `b`, clamped down to a
/// char boundary of `b` so front-coded suffixes stay valid UTF-8. `b` is valid
/// UTF-8, and a byte index is a char boundary of it exactly when it is the end
/// or does not land on a continuation byte (`0b10xx_xxxx`) — the same rule as
/// `str::is_char_boundary`, on bytes, so the spilled tokens of the
/// memory-bounded build front-code exactly as the in-RAM `String`s do.
fn shared_prefix(a: &[u8], b: &[u8]) -> usize {
    let mut n = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    while n > 0 && n < b.len() && (b[n] & 0xC0) == 0x80 {
        n -= 1;
    }
    n
}

// ---------------------------------------------------------------------------
// Scope
// ---------------------------------------------------------------------------

/// Which graphs' literals a text index covers (`rete build --text-index-graphs`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextIndexGraphs {
    /// The default graph only — what every index built before 0.4.0 covers.
    #[default]
    Default,
    /// The default graph and every named graph.
    All,
}

/// Which predicates' literals a text index covers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextIndexPredicates {
    /// Every predicate.
    #[default]
    All,
    /// Only these predicates (N-Triples `<iri>` tokens, ascending, distinct).
    Only(Vec<String>),
    /// Every predicate except these (same spelling).
    Except(Vec<String>),
}

/// What a text index covers: a graph scope and a predicate filter. The
/// [`Default`] is the pre-0.4.0 index — the default graph, every predicate —
/// and it is the one scope that writes no scope block.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextIndexScope {
    /// The graphs whose literals are indexed.
    pub graphs: TextIndexGraphs,
    /// The predicates whose literals are indexed.
    pub predicates: TextIndexPredicates,
}

/// Spell a predicate IRI the way the dictionary stores it: an N-Triples token
/// `<…>`. Accepts the IRI bare or already bracketed; refuses anything that is
/// not an absolute IRI (no scheme, whitespace, brackets inside), because a
/// typo here silently indexes the wrong literals.
fn predicate_token(iri: &str) -> Result<String, String> {
    let t = iri.trim();
    let inner = t
        .strip_prefix('<')
        .and_then(|r| r.strip_suffix('>'))
        .unwrap_or(t);
    let scheme_ok = inner
        .split_once(':')
        .is_some_and(|(s, rest)| !s.is_empty() && !rest.is_empty());
    if inner.is_empty()
        || !scheme_ok
        || inner
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '<' | '>' | '"'))
    {
        return Err(format!(
            "`{iri}` is not an absolute predicate IRI (expected e.g. http://purl.org/dc/terms/description)"
        ));
    }
    Ok(format!("<{inner}>"))
}

impl TextIndexScope {
    /// Build a scope from the CLI's spelling: a graph scope and predicate IRIs
    /// to include or to exclude (bare or `<…>`). Giving both lists is an error —
    /// an include list already says exactly what is indexed. Empty lists mean
    /// every predicate.
    pub fn from_options<I, E>(
        graphs: TextIndexGraphs,
        include: I,
        exclude: E,
    ) -> Result<Self, String>
    where
        I: IntoIterator,
        I::Item: AsRef<str>,
        E: IntoIterator,
        E::Item: AsRef<str>,
    {
        let norm = |items: Vec<String>| -> Result<Vec<String>, String> {
            let mut out = items
                .iter()
                .map(|s| predicate_token(s))
                .collect::<Result<Vec<_>, _>>()?;
            out.sort();
            out.dedup();
            Ok(out)
        };
        let include = norm(
            include
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        )?;
        let exclude = norm(
            exclude
                .into_iter()
                .map(|s| s.as_ref().to_string())
                .collect(),
        )?;
        let predicates = match (include.is_empty(), exclude.is_empty()) {
            (true, true) => TextIndexPredicates::All,
            (false, true) => TextIndexPredicates::Only(include),
            (true, false) => TextIndexPredicates::Except(exclude),
            (false, false) => {
                return Err(
                    "give --text-index-include or --text-index-exclude, not both".to_string(),
                )
            }
        };
        Ok(TextIndexScope { graphs, predicates })
    }

    /// The pre-0.4.0 scope (default graph, every predicate) — written with no
    /// scope block.
    pub fn is_default(&self) -> bool {
        *self == TextIndexScope::default()
    }

    /// Does the scope leave some predicate's literals out? Such an index is
    /// written under [`crate::header::SectionKind::TextIndexPartial`] so that
    /// readers which do not know scopes never prune a filter with it.
    pub fn restricts_predicates(&self) -> bool {
        !matches!(self.predicates, TextIndexPredicates::All)
    }

    /// Are the named graphs' literals indexed too?
    pub fn covers_named_graphs(&self) -> bool {
        self.graphs == TextIndexGraphs::All
    }

    /// Are literals of predicate `p` (an N-Triples token `<…>`) indexed?
    pub fn covers_predicate(&self, p: &str) -> bool {
        match &self.predicates {
            TextIndexPredicates::All => true,
            TextIndexPredicates::Only(list) => list.binary_search_by(|x| x.as_str().cmp(p)).is_ok(),
            TextIndexPredicates::Except(list) => {
                list.binary_search_by(|x| x.as_str().cmp(p)).is_err()
            }
        }
    }

    /// Does the index cover a triple pattern's literals? `p` is the pattern's
    /// constant predicate, or `None` for a variable one — which can bind any
    /// predicate, so only an every-predicate index covers it.
    pub fn covers_pattern_predicate(&self, p: Option<&str>) -> bool {
        match p {
            Some(p) => self.covers_predicate(p),
            None => !self.restricts_predicates(),
        }
    }

    /// The predicate list of an include/exclude scope (empty for every-predicate).
    pub fn listed_predicates(&self) -> &[String] {
        match &self.predicates {
            TextIndexPredicates::All => &[],
            TextIndexPredicates::Only(l) | TextIndexPredicates::Except(l) => l,
        }
    }

    /// One line for logs and `rete info`: what the index covers.
    pub fn describe(&self) -> String {
        let graphs = match self.graphs {
            TextIndexGraphs::Default => "default graph",
            TextIndexGraphs::All => "default + named graphs",
        };
        match &self.predicates {
            TextIndexPredicates::All => format!("{graphs}, every predicate"),
            TextIndexPredicates::Only(l) => format!("{graphs}, only {}", l.join(" ")),
            TextIndexPredicates::Except(l) => format!("{graphs}, all but {}", l.join(" ")),
        }
    }

    /// Append the scope block to an uncompressed token table — nothing at all
    /// for the default scope.
    pub(crate) fn encode(&self, out: &mut Vec<u8>) {
        if self.is_default() {
            return;
        }
        out.push(SCOPE_BLOCK_VERSION);
        out.push(match self.graphs {
            TextIndexGraphs::Default => 0,
            TextIndexGraphs::All => 1,
        });
        out.push(match self.predicates {
            TextIndexPredicates::All => 0,
            TextIndexPredicates::Only(_) => 1,
            TextIndexPredicates::Except(_) => 2,
        });
        let list = self.listed_predicates();
        write_uvarint(out, list.len() as u64);
        for p in list {
            write_uvarint(out, p.len() as u64);
            out.extend_from_slice(p.as_bytes());
        }
    }

    /// Decode the bytes after the last token entry. Empty = the default scope.
    /// A block of a version this reader does not know decodes as a scope that
    /// covers **no** predicate: the index still answers `rete search`, and the
    /// SPARQL pushdown never prunes with coverage it cannot read.
    fn decode(bytes: &[u8]) -> Result<Self, FileError> {
        let Some((&version, rest)) = bytes.split_first() else {
            return Ok(TextIndexScope::default());
        };
        if version != SCOPE_BLOCK_VERSION {
            return Ok(TextIndexScope {
                graphs: TextIndexGraphs::Default,
                predicates: TextIndexPredicates::Only(Vec::new()),
            });
        }
        let bad = FileError::Container("malformed text-index scope block");
        let (&g, rest) = rest.split_first().ok_or(bad)?;
        let (&m, rest) = rest
            .split_first()
            .ok_or(FileError::Container("malformed text-index scope block"))?;
        let graphs = match g {
            0 => TextIndexGraphs::Default,
            1 => TextIndexGraphs::All,
            _ => return Err(FileError::Container("unknown text-index graph scope")),
        };
        let mut pos = 0usize;
        let (n, k) =
            read_uvarint(rest).ok_or(FileError::Container("malformed text-index scope block"))?;
        pos += k;
        let mut list = Vec::with_capacity((n as usize).min(rest.len()));
        for _ in 0..n {
            let (len, k) = read_uvarint(rest.get(pos..).unwrap_or_default())
                .ok_or(FileError::Container("malformed text-index scope block"))?;
            pos += k;
            let end = pos
                .checked_add(len as usize)
                .filter(|&e| e <= rest.len())
                .ok_or(FileError::Container("text-index scope predicate overruns"))?;
            let p = std::str::from_utf8(&rest[pos..end])
                .map_err(|_| FileError::Container("bad text-index scope utf8"))?;
            list.push(p.to_string());
            pos = end;
        }
        let predicates = match m {
            0 => TextIndexPredicates::All,
            1 => TextIndexPredicates::Only(list),
            2 => TextIndexPredicates::Except(list),
            _ => return Err(FileError::Container("unknown text-index predicate scope")),
        };
        Ok(TextIndexScope { graphs, predicates })
    }
}

/// A predicate-restricted section as written (kind 11): the 16-byte
/// self-digest `blake3(payload)[..16]`, then the payload — the kind-6 layout.
/// The digest keeps the section verifiable although it is outside the content
/// hash (an older `verify`, which cannot see kind 11, must not fail on it).
pub(crate) fn with_partial_digest(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 16);
    out.extend_from_slice(&blake3::hash(payload).as_bytes()[..16]);
    out.extend_from_slice(payload);
    out
}

/// What a build's `with_text_index` argument may be: `false` / `true` (no
/// index / the default scope — the spelling every caller used before scopes),
/// a [`TextIndexScope`], or an `Option` of one.
pub trait IntoTextIndex {
    /// `None` = build no text index.
    fn into_text_index(self) -> Option<TextIndexScope>;
}

impl IntoTextIndex for bool {
    fn into_text_index(self) -> Option<TextIndexScope> {
        self.then(TextIndexScope::default)
    }
}

impl IntoTextIndex for TextIndexScope {
    fn into_text_index(self) -> Option<TextIndexScope> {
        Some(self)
    }
}

impl IntoTextIndex for Option<TextIndexScope> {
    fn into_text_index(self) -> Option<TextIndexScope> {
        self
    }
}

impl IntoTextIndex for &Option<TextIndexScope> {
    fn into_text_index(self) -> Option<TextIndexScope> {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Append subject `s` to a posting's delta-varint run (`prev` starts at 0;
/// subjects must arrive strictly ascending).
pub(crate) fn push_delta(out: &mut Vec<u8>, prev: &mut u32, s: u32) {
    write_uvarint(out, (s - *prev) as u64);
    *prev = s;
}

/// The one serializer of the token table entries and the postings blob, shared
/// by the in-RAM [`TextIndexBuilder`] and the memory-bounded build
/// (`text_index_spill`): both feed it the same `(token, subjects)` sequence in
/// ascending token order, so the two produce the same bytes by construction.
/// It writes the token entries **without** the leading `num_tokens` varint (not
/// known until the end) — the caller prepends it, then appends the scope block.
pub(crate) struct SectionWriter<E: Write, P: Write> {
    entries: E,
    postings: P,
    prev: Vec<u8>,
    tokens: u64,
    offset: u64,
}

impl<E: Write, P: Write> SectionWriter<E, P> {
    pub(crate) fn new(entries: E, postings: P) -> Self {
        SectionWriter {
            entries,
            postings,
            prev: Vec::new(),
            tokens: 0,
            offset: 0,
        }
    }

    /// Append one token: its `count` subjects, already delta-encoded.
    pub(crate) fn push(&mut self, token: &[u8], count: u64, deltas: &[u8]) -> io::Result<()> {
        self.push_with(token, count, deltas.len() as u64, |p| p.write_all(deltas))
    }

    /// Append one token whose `deltas_len` bytes of deltas `write` emits
    /// (the spilled path copies an oversized posting from disk).
    pub(crate) fn push_with(
        &mut self,
        token: &[u8],
        count: u64,
        deltas_len: u64,
        write: impl FnOnce(&mut P) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut head = Vec::with_capacity(10);
        write_uvarint(&mut head, count);
        self.postings.write_all(&head)?;
        write(&mut self.postings)?;
        let len = head.len() as u64 + deltas_len;

        let shared = shared_prefix(&self.prev, token);
        let suffix = &token[shared..];
        let mut e = Vec::with_capacity(suffix.len() + 16);
        write_uvarint(&mut e, shared as u64);
        write_uvarint(&mut e, suffix.len() as u64);
        e.extend_from_slice(suffix);
        write_uvarint(&mut e, self.offset);
        write_uvarint(&mut e, len);
        self.entries.write_all(&e)?;

        self.offset += len;
        self.tokens += 1;
        self.prev.clear();
        self.prev.extend_from_slice(token);
        Ok(())
    }

    /// The two sinks, the token count and the postings-blob length.
    pub(crate) fn finish(self) -> (E, P, u64, u64) {
        (self.entries, self.postings, self.tokens, self.offset)
    }
}

/// Accumulates `token → subjects` and serializes the section.
#[derive(Default)]
pub struct TextIndexBuilder {
    postings: BTreeMap<String, BTreeSet<u32>>,
    scope: TextIndexScope,
}

impl TextIndexBuilder {
    /// A builder for the default scope (default graph, every predicate).
    pub fn new() -> Self {
        Self::default()
    }

    /// A builder whose section records `scope` (the caller decides which
    /// literals to feed it; this only writes the scope block).
    pub fn with_scope(scope: TextIndexScope) -> Self {
        TextIndexBuilder {
            postings: BTreeMap::new(),
            scope,
        }
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

    /// No token recorded (the build then writes no section at all).
    pub fn is_empty(&self) -> bool {
        self.postings.is_empty()
    }

    /// Serialize the section (deterministic: tokens sorted, subjects ascending).
    pub fn build(&self, codec: u8) -> Vec<u8> {
        let mut w = SectionWriter::new(Vec::new(), Vec::new());
        let mut deltas = Vec::new();
        for (token, subjects) in &self.postings {
            deltas.clear();
            let mut prev = 0u32;
            for &s in subjects {
                push_delta(&mut deltas, &mut prev, s);
            }
            w.push(token.as_bytes(), subjects.len() as u64, &deltas)
                .expect("writes to a Vec are infallible");
        }
        let (entries, postings, tokens, _) = w.finish();
        let mut tt = Vec::with_capacity(entries.len() + 16);
        write_uvarint(&mut tt, tokens);
        tt.extend_from_slice(&entries);
        self.scope.encode(&mut tt);
        let ctt = compress(codec, &tt);

        let mut out = Vec::with_capacity(10 + ctt.len() + postings.len());
        write_uvarint(&mut out, ctt.len() as u64);
        out.extend_from_slice(&ctt);
        out.extend_from_slice(&postings);
        out
    }
}

/// Build the TEXT_INDEX section of an in-RAM build from the dictionary and the
/// encoded id-triples: every literal object of the graphs and predicates in
/// `scope` is tokenized and its words recorded for the triple's subject.
/// `named` is consulted only when the scope covers named graphs. Empty when no
/// literal is in scope — the build then writes no section.
///
/// Prints one line per listed predicate that no literal matched: a misspelt
/// `--text-index-exclude` would otherwise exclude nothing, silently.
pub(crate) fn build_section<'a>(
    dict: &crate::dictionary::Dictionary,
    scope: &TextIndexScope,
    default: &[(u32, u32, u32)],
    named: impl Iterator<Item = &'a [(u32, u32, u32)]>,
    codec: u8,
) -> Vec<u8> {
    use crate::terms::{is_literal, literal_lexical};
    // Resolve the listed predicates to ids once; an IRI the graph never uses as
    // a predicate has no id and can match nothing.
    let listed: Vec<Option<u32>> = scope
        .listed_predicates()
        .iter()
        .map(|p| dict.predicate_id(p))
        .collect();
    let mut seen = vec![false; listed.len()];
    let mut covered = |p: u32| -> bool {
        let hit = listed.iter().position(|id| *id == Some(p));
        if let Some(i) = hit {
            seen[i] = true;
        }
        match scope.predicates {
            TextIndexPredicates::All => true,
            TextIndexPredicates::Only(_) => hit.is_some(),
            TextIndexPredicates::Except(_) => hit.is_none(),
        }
    };
    let mut b = TextIndexBuilder::with_scope(scope.clone());
    let named: Vec<&[(u32, u32, u32)]> = if scope.covers_named_graphs() {
        named.collect()
    } else {
        Vec::new()
    };
    for triples in std::iter::once(default).chain(named) {
        for &(s, p, o) in triples {
            let Some(term) = dict.object_term(o) else {
                continue;
            };
            if !is_literal(&term) || !covered(p) {
                continue;
            }
            if let Some(lit) = literal_lexical(&term) {
                b.add_text(&lit, s);
            }
        }
    }
    warn_unmatched(scope, &seen);
    if b.is_empty() {
        Vec::new()
    } else {
        b.build(codec)
    }
}

/// Report each `--text-index-include`/`--text-index-exclude` predicate that no
/// in-scope literal carried (`seen[i]` for `scope.listed_predicates()[i]`).
pub(crate) fn warn_unmatched(scope: &TextIndexScope, seen: &[bool]) {
    let flag = match scope.predicates {
        TextIndexPredicates::All => return,
        TextIndexPredicates::Only(_) => "--text-index-include",
        TextIndexPredicates::Except(_) => "--text-index-exclude",
    };
    for (p, _) in scope
        .listed_predicates()
        .iter()
        .zip(seen)
        .filter(|(_, s)| !**s)
    {
        eprintln!(
            "warning: {flag} {p}: no literal object with this predicate in the indexed graphs"
        );
    }
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

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
    scope: TextIndexScope,
}

impl TextIndex {
    /// Parse a whole section (local): token table + the full postings blob resident.
    pub fn from_section(section: &[u8], codec: u8) -> Result<Self, FileError> {
        let (tokens, scope, postings_start) = parse_token_table(section, codec)?;
        let postings = section
            .get(postings_start..)
            .ok_or(FileError::Container("text-index postings overrun"))?
            .to_vec();
        Ok(TextIndex {
            tokens,
            postings: Postings::Resident(postings),
            scope,
        })
    }

    /// Build from a section **prefix** holding the token table, plus a loader that
    /// fetches a posting `(offset_within_postings_blob, len)` on demand (remote).
    pub fn from_token_table(
        prefix: &[u8],
        codec: u8,
        loader: Box<dyn Fn(u64, u64) -> Option<Vec<u8>> + Send + Sync>,
    ) -> Result<Self, FileError> {
        let (tokens, scope, _postings_start) = parse_token_table(prefix, codec)?;
        Ok(TextIndex {
            tokens,
            postings: Postings::Remote(loader),
            scope,
        })
    }

    /// Byte offset of the postings blob within the section (token-table end).
    /// The remote opener uses this to base its posting-range loader.
    pub fn postings_base(section_prefix: &[u8]) -> Option<usize> {
        let (len, n) = read_uvarint(section_prefix)?;
        Some(n + len as usize)
    }

    /// Number of distinct tokens.
    pub fn token_count(&self) -> usize {
        self.tokens.len()
    }

    /// What this index covers (the default scope for every pre-0.4.0 file).
    pub fn scope(&self) -> &TextIndexScope {
        &self.scope
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
/// `(token, off, len)` list, the scope, and the byte offset where the postings
/// blob begins.
#[allow(clippy::type_complexity)]
fn parse_token_table(
    bytes: &[u8],
    codec: u8,
) -> Result<(Vec<(String, u64, u64)>, TextIndexScope, usize), FileError> {
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
    let scope = TextIndexScope::decode(tt.get(pos..).unwrap_or_default())?;
    Ok((tokens, scope, end))
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
        assert!(idx.scope().is_default());
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

    /// The byte-level prefix rule must agree with `str::is_char_boundary`, or
    /// the spilled build (bytes) and the in-RAM build (`String`s) would
    /// front-code multi-byte tokens differently.
    #[test]
    fn shared_prefix_on_bytes_matches_the_str_rule() {
        let words = [
            "",
            "a",
            "ab",
            "abc",
            "é",
            "éa",
            "éé",
            "eé",
            "ü",
            "ünïcode",
            "ünïcodé",
            "日本",
            "日本語",
            "日",
            "z",
            "zß",
            "zss",
        ];
        for a in words {
            for b in words {
                let mut n = a
                    .as_bytes()
                    .iter()
                    .zip(b.as_bytes())
                    .take_while(|(x, y)| x == y)
                    .count();
                while n > 0 && !b.is_char_boundary(n) {
                    n -= 1;
                }
                assert_eq!(
                    shared_prefix(a.as_bytes(), b.as_bytes()),
                    n,
                    "{a:?} vs {b:?}"
                );
            }
        }
    }

    /// The pinned bytes of a default-scope section: what every build wrote
    /// before scopes existed, so a refactor of the writer cannot move them.
    #[test]
    fn default_scope_section_bytes_are_pinned() {
        let mut b = TextIndexBuilder::new();
        b.add_text("ab ac", 1);
        b.add_text("ab", 3);
        let bytes = b.build(CODEC_NONE);
        // tt = [2 tokens] [0,2,"ab",off 0,len 3] [1,1,"c",off 3,len 2]
        // postings = [2, 1, 2] [1, 1]
        let tt: &[u8] = &[2, 0, 2, b'a', b'b', 0, 3, 1, 1, b'c', 3, 2];
        let mut want = vec![tt.len() as u8];
        want.extend_from_slice(tt);
        want.extend_from_slice(&[2, 1, 2, 1, 1]);
        assert_eq!(bytes, want);
    }

    #[test]
    fn scope_block_round_trips_and_default_writes_none() {
        let mut tt = Vec::new();
        TextIndexScope::default().encode(&mut tt);
        assert!(tt.is_empty(), "the default scope writes no block");

        let scopes = [
            TextIndexScope {
                graphs: TextIndexGraphs::All,
                predicates: TextIndexPredicates::All,
            },
            TextIndexScope::from_options(
                TextIndexGraphs::Default,
                Vec::<String>::new(),
                ["http://purl.org/dc/terms/description", "<http://ex/b>"],
            )
            .unwrap(),
            TextIndexScope::from_options(
                TextIndexGraphs::All,
                ["http://ex/z", "http://ex/a", "http://ex/z"],
                Vec::<String>::new(),
            )
            .unwrap(),
        ];
        for scope in scopes {
            let mut b = TextIndexBuilder::with_scope(scope.clone());
            b.add_text("word", 1);
            let bytes = b.build(CODEC_NONE);
            let idx = TextIndex::from_section(&bytes, CODEC_NONE).unwrap();
            assert_eq!(idx.scope(), &scope);
            assert_eq!(idx.lookup("word"), vec![1]);
            // The remote path parses the same table prefix to the same scope.
            let base = TextIndex::postings_base(&bytes).unwrap();
            let remote =
                TextIndex::from_token_table(&bytes[..base], CODEC_NONE, Box::new(|_, _| None))
                    .unwrap();
            assert_eq!(remote.scope(), &scope);
        }
    }

    #[test]
    fn scope_options_normalize_and_validate() {
        let s = TextIndexScope::from_options(
            TextIndexGraphs::Default,
            Vec::<String>::new(),
            ["<http://ex/b>", "http://ex/a", "http://ex/b"],
        )
        .unwrap();
        assert_eq!(
            s.predicates,
            TextIndexPredicates::Except(vec!["<http://ex/a>".into(), "<http://ex/b>".into()])
        );
        assert!(s.restricts_predicates());
        assert!(!s.covers_predicate("<http://ex/a>"));
        assert!(s.covers_predicate("<http://ex/c>"));
        assert!(s.covers_pattern_predicate(Some("<http://ex/c>")));
        assert!(
            !s.covers_pattern_predicate(None),
            "a variable predicate can bind ex:a"
        );

        let only = TextIndexScope::from_options(
            TextIndexGraphs::All,
            ["http://ex/a"],
            Vec::<String>::new(),
        )
        .unwrap();
        assert!(only.covers_predicate("<http://ex/a>"));
        assert!(!only.covers_predicate("<http://ex/b>"));
        assert!(only.covers_named_graphs());

        let all = TextIndexScope::default();
        assert!(all.covers_pattern_predicate(None));
        assert!(!all.restricts_predicates());

        for bad in ["description", "", "http://ex/a b", "<http://ex/a", ":x"] {
            assert!(
                TextIndexScope::from_options(TextIndexGraphs::Default, [bad], Vec::<String>::new())
                    .is_err(),
                "{bad:?} should be refused"
            );
        }
        assert!(TextIndexScope::from_options(
            TextIndexGraphs::Default,
            ["http://ex/a"],
            ["http://ex/b"]
        )
        .is_err());
    }

    /// A scope block of a later version must not make the file unreadable, and
    /// must not be read as covering anything the SPARQL pushdown could prune on.
    #[test]
    fn unknown_scope_version_covers_no_predicate() {
        let s = TextIndexScope::decode(&[9, 1, 2, 3]).unwrap();
        assert!(!s.covers_predicate("<http://ex/a>"));
        assert!(!s.covers_pattern_predicate(None));
        // A truncated v1 block is malformed, not silently the default.
        assert!(TextIndexScope::decode(&[1, 0]).is_err());
        assert!(TextIndexScope::decode(&[1, 0, 1, 1, 5, b'<']).is_err());
    }
}
