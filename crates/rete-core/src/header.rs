//! The fixed-size 1024-byte file header (SPEC.md §4.1).
//!
//! A client's very first request is `bytes=0-1023`; this struct is what those
//! bytes decode to, and it points at every other section of the file via a
//! **typed section directory**, so new top-level sections (e.g. a future text
//! index) are added as a new directory entry without reshaping the header.
//!
//! Layout: a fixed 64-byte **core** (magic, version, flags, content hash, counts,
//! codecs, `section_count`, `schema_meta_len`) followed by up to
//! [`MAX_SECTIONS`] **directory entries** of 24 bytes each `(kind, flags, offset,
//! length)`, zero-padded to 1024. The known section kinds populate the named
//! convenience fields below; unknown kinds (written by a newer build) are
//! preserved verbatim in [`Header::extra_sections`].

use std::convert::TryInto;

/// Magic bytes at offset 0: ASCII `RETE`.
pub const MAGIC: [u8; 4] = *b"RETE";

/// Current format generation written by this crate.
///
/// `0x05` is stable format generation 1, frozen on 2026-07-14 and first
/// released in Rete 0.3.0. It retains the six-wide index-permutation addressing
/// and the 1 KiB section-directory layout finalized in the last experimental
/// generation; *which* of those six a given file stores is [`Header::perms`]
/// (byte 50), not the generation.
///
/// The generation number is not a release version — there is no Rete 1.0.0, and
/// it is the Rust/CLI/WASM APIs, not the format, that are waiting on one. Nor
/// does it pin reader capability: writer semantics changed inside `0x05` nine
/// days after the freeze (#68 split oversized index groups across tiles), so a
/// `0x05` file can need a reader newer than the one that froze `0x05`.
pub const CURRENT_FORMAT_VERSION: u8 = 0x06;

/// Format generation `0x05` (stable generation 1, the 1 KiB section-directory
/// layout this module parses). Every reader still reads it while the catalog is
/// converted; the memory-bounded build, `merge` and `recard` still write it
/// until they move to `0x06`.
pub const LEGACY_FORMAT_VERSION: u8 = 0x05;

/// Oldest stable format generation accepted by this reader.
///
/// Files written before the generation-1 freeze used experimental generations
/// `0x01` through `0x04` and must be rebuilt from their RDF source. `0x05` has
/// not moved since it froze on 2026-07-14, but that is a track record, not a
/// promise: no backwards compatibility is guaranteed before 1.0.0, so this
/// constant may yet be raised and a `0x05` file may yet need a rebuild. See
/// `docs/compatibility.md`.
pub const MIN_STABLE_READ_VERSION: u8 = 0x05;

/// Fixed header size in bytes.
pub const HEADER_LEN: usize = 1024;

/// Byte offset of the first section-directory entry (i.e. the core size).
const SECTION_DIR_OFFSET: usize = 64;
/// Size of one section-directory entry.
const SECTION_ENTRY_LEN: usize = 24;
/// How many directory entries fit in the 1 KB frame.
pub const MAX_SECTIONS: usize = (HEADER_LEN - SECTION_DIR_OFFSET) / SECTION_ENTRY_LEN;

/// Flag bit: the file contains named graphs (quads) rather than triples only.
pub const FLAG_HAS_QUADS: u8 = 0b0000_0001;

/// Flag bit: each tiled permutation section carries a **tile-synopsis trailer**
/// — per-tile min/max of the two non-leading columns, appended after the tile
/// payloads. It lets a range reader prune a routed tile by a bound secondary
/// component *before* fetching it. See `file.rs::encode_tiled_section`.
pub const FLAG_TILE_SYNOPSIS: u8 = 0b0000_0010;

/// Flag bit: the file contains **RDF-star quoted triples** (`<< s p o >>`) as
/// dictionary terms. Purely informational for compatibility — a plain-RDF
/// consumer can detect from the header alone that some terms are quoted triples
/// (which it may not understand) without scanning the dictionary. The file is
/// otherwise a normal `.rete`: quoted triples are stored like any other term, so
/// this needs no format-version bump and old readers stay forward-compatible.
pub const FLAG_HAS_QUOTED_TRIPLES: u8 = 0b0000_0100;

/// Flag bit: the named graphs live in the **quad index** — the QUAD_INDEX
/// section ([`SectionKind::QuadIndex`], kind 10) holds every named-graph quad as
/// the fourth column of graph-leading permutations, the NAMED_GRAPHS section
/// (kind 5) is absent, and the index container carries one trailing **seal**
/// section (see `quad.rs`). Readers before this flag do not check it; they
/// refuse the file anyway, on the seal (the container's section count), so a
/// quad-index file never opens as one without named graphs
/// (`docs/compatibility.md`).
pub const FLAG_QUAD_INDEX: u8 = 0b0000_1000;

/// Flag bit: the full-text index also covers the **named graphs'** literals
/// (`rete build --text-index-graphs all`, Rete 0.4.0). It lets a query inside
/// `GRAPH`, under `FROM` or over the union default graph decide from the
/// header alone whether the index can prune its `FILTER(CONTAINS(…))`: without
/// the bit it scans and never faults the index's token table. Bit 3
/// (`0b0000_1000`) belongs to the quad index. Older readers ignore unknown bits.
pub const FLAG_TEXT_INDEX_NAMED_GRAPHS: u8 = 0b0001_0000;

/// Length of the BLAKE3 self-digest that heads a predicate-restricted text
/// index section ([`SectionKind::TextIndexPartial`], kind 11):
/// `digest = blake3(section[16..])[..16]`, the same rule as the LABEL_INDEX and
/// SCHEMA sections.
pub const TEXT_INDEX_PARTIAL_DIGEST_LEN: u64 = 16;

/// A top-level file section, addressed by [`SectionKind`] in the header directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    /// Dataset-card metadata.
    Metadata,
    /// Dictionary container (front-coded term sections).
    Dictionary,
    /// Permutation index container (SPO/POS/OSP).
    Index,
    /// Community + schema pyramid metadata.
    PyramidMeta,
    /// Named-graphs section.
    NamedGraphs,
    /// Full-text (word) index over literals — `token → subjects`.
    TextIndex,
    /// A full-text index that covers **only some predicates**
    /// (`rete build --text-index-include/--text-index-exclude`): the same
    /// layout as [`SectionKind::TextIndex`], under its own kind so that a
    /// reader which predates text-index scopes does not see it at all. Such a
    /// reader prunes `FILTER(CONTAINS(…))` with any text index it finds, as if
    /// the index covered every literal; with this one it would silently drop
    /// the rows of the predicates left out. Not seeing it, it scans instead.
    /// The header maps it onto the same `text_index_*` fields, with
    /// [`Header::text_index_partial`] set. Like kinds 8 and 9 it is **outside
    /// the content hash** — an older `verify`, which cannot see it, must not
    /// call the file corrupt — and starts with its own 16-byte digest
    /// ([`TEXT_INDEX_PARTIAL_DIGEST_LEN`]) instead; the kind-6 layout follows
    /// it ([`Header::text_index_payload`]).
    TextIndexPartial,
    /// Build-conditions record (timestamp, builder version, build parameters,
    /// measured starter-query costs), stored as an opaque JSON blob owned by the
    /// application layer (the CLI). **Deliberately excluded from the content
    /// hash**: two builds of identical data must hash identically, and this
    /// section is exactly the per-build facts (when, by which binary, how fast)
    /// that differ between them. `verify` therefore does not cover it.
    BuildInfo,
    /// The range-readable **label index** (`rete build --label-index`): every
    /// labelled subject's display label, sorted for prefix search, as a
    /// multi-level table a reader walks with a few small range reads. See
    /// [`crate::label_index`]. Optional; outside the content hash and
    /// self-digested (SPEC §6.4).
    LabelIndex,
    /// The **schema** section: the class hierarchy, the per-level type rollups
    /// and lateral class links, the coherence axioms, the planner's
    /// `query_stats` and the characteristic sets — computed without the
    /// community dendrogram. See [`crate::schema_section`]. Optional; outside
    /// the content hash and self-digested (SPEC §7.5).
    Schema,
    /// The quad index (kind 10): every named-graph quad in graph-leading
    /// permutations behind a sorted graph directory. Present exactly when the
    /// header has [`FLAG_QUAD_INDEX`]. Kept in [`Header::extra_sections`] like
    /// an unknown kind (it has no named convenience fields).
    QuadIndex,
    /// A section kind this build doesn't know — preserved verbatim on round-trip
    /// so a newer writer's sections survive an older reader.
    Unknown(u16),
}

impl SectionKind {
    fn to_u16(self) -> u16 {
        match self {
            SectionKind::Metadata => 1,
            SectionKind::Dictionary => 2,
            SectionKind::Index => 3,
            SectionKind::PyramidMeta => 4,
            SectionKind::NamedGraphs => 5,
            SectionKind::TextIndex => 6,
            SectionKind::BuildInfo => 7,
            SectionKind::LabelIndex => 8,
            SectionKind::Schema => 9,
            SectionKind::QuadIndex => 10,
            SectionKind::TextIndexPartial => 11,
            SectionKind::Unknown(k) => k,
        }
    }

    fn from_u16(k: u16) -> Self {
        match k {
            1 => SectionKind::Metadata,
            2 => SectionKind::Dictionary,
            3 => SectionKind::Index,
            4 => SectionKind::PyramidMeta,
            5 => SectionKind::NamedGraphs,
            6 => SectionKind::TextIndex,
            7 => SectionKind::BuildInfo,
            8 => SectionKind::LabelIndex,
            9 => SectionKind::Schema,
            10 => SectionKind::QuadIndex,
            11 => SectionKind::TextIndexPartial,
            other => SectionKind::Unknown(other),
        }
    }
}

/// One parsed/encoded section-directory entry: a typed `(offset, length)` into
/// the file, plus 16 bits of per-section flags (reserved).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub kind: SectionKind,
    pub flags: u16,
    pub offset: u64,
    pub length: u64,
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum HeaderError {
    #[error("buffer too small: need {HEADER_LEN} bytes, got {0}")]
    TooSmall(usize),
    #[error("bad magic: expected RETE")]
    BadMagic,
    #[error(
        "unsupported .rete format {found:#04x}; this Rete build reads {min:#04x}..={max:#04x}. Pre-1.0 files must be rebuilt from RDF source with `rete build`"
    )]
    UnsupportedVersion { found: u8, min: u8, max: u8 },
    #[error("section count {0} overruns the header frame")]
    BadSectionCount(usize),
    #[error("unusable index permutation mask {mask:#04x}: {why}")]
    BadPermMask { mask: u8, why: &'static str },
    #[error("malformed 0x06 header: {0}")]
    Malformed06(&'static str),
}

/// Decoded file header. All multi-byte fields are little-endian on disk. The
/// `*_offset` / `*_len` fields are a convenience view over the section directory
/// (populated from it on parse, emitted back to it on serialize).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Format version of the parsed file (currently
    /// [`MIN_STABLE_READ_VERSION`] through [`CURRENT_FORMAT_VERSION`]).
    pub version: u8,
    pub flags: u8,
    pub metadata_offset: u64,
    pub metadata_len: u64,
    pub dictionary_offset: u64,
    pub dictionary_len: u64,
    pub root_dir_offset: u64,
    pub root_dir_len: u64,
    pub pyramid_meta_offset: u64,
    pub pyramid_meta_len: u64,
    pub dict_codec: u8,
    pub block_codec: u8,
    pub pyramid_levels: u16,
    pub quad_count: u64,
    pub term_count: u64,
    /// First 16 bytes of the blake3 content hash — an immutable validator.
    pub content_hash: [u8; 16],
    /// Named-graphs section (0 if the file has only the default graph).
    pub named_graphs_offset: u64,
    pub named_graphs_len: u64,
    /// Byte length of the trailing **schema-pyramid block** within the pyramid-meta
    /// section (0 if none). A reader fetches *only* that block — at
    /// `pyramid_meta_offset + pyramid_meta_len - schema_meta_len` — for an
    /// index/dictionary/summary-free Tier-0 coherence check.
    pub schema_meta_len: u32,
    /// Full-text index section (0 if the file has none; built with
    /// `rete build --text-index`). See [`SectionKind::TextIndex`].
    pub text_index_offset: u64,
    pub text_index_len: u64,
    /// The full-text index covers only some predicates: it is written (and was
    /// read) as [`SectionKind::TextIndexPartial`] rather than
    /// [`SectionKind::TextIndex`]. `false` for every file without one.
    pub text_index_partial: bool,
    /// Build-conditions section (0 if absent). See [`SectionKind::BuildInfo`]:
    /// an opaque per-build record that is **not** covered by the content hash.
    pub build_info_offset: u64,
    pub build_info_len: u64,
    /// Which index permutations the file's index containers carry
    /// ([`crate::index::PermSet`]).
    ///
    /// On disk this is one byte at `[50]`, inside the reserved core span, and
    /// **`0` means all six** — so every file written before the mask existed
    /// decodes as [`PermSet::ALL`] and a full six-permutation build stays
    /// byte-identical to what it always was. A lean file writes its mask
    /// (`0b000_0111` for SPO+POS+OSP), and its index container then holds three
    /// sections rather than six, which is what an older reader trips over.
    ///
    /// [`PermSet::ALL`]: crate::index::PermSet::ALL
    pub perms: crate::index::PermSet,
    /// Directory entries of the kinds added after the generation-1 freeze —
    /// [`SectionKind::LabelIndex`] (8), [`SectionKind::Schema`] (9) — and of
    /// any kind this build doesn't recognize, preserved verbatim in directory
    /// order. None of them is covered by the header content hash.
    pub extra_sections: Vec<Section>,
}

impl Header {
    /// Serialize into a fixed 1024-byte array.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        // --- core (64 bytes) ---
        b[0..4].copy_from_slice(&MAGIC);
        b[4] = self.version;
        b[5] = self.flags;
        b[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        b[8..24].copy_from_slice(&self.content_hash);
        b[24..32].copy_from_slice(&self.quad_count.to_le_bytes());
        b[32..40].copy_from_slice(&self.term_count.to_le_bytes());
        b[40..42].copy_from_slice(&self.pyramid_levels.to_le_bytes());
        b[42] = self.dict_codec;
        b[43] = self.block_codec;
        // [44..46) section_count written below.
        b[46..50].copy_from_slice(&self.schema_meta_len.to_le_bytes());
        // [50] permutation mask; 0 = all six, so a full build is byte-identical
        // to every file written before the field existed.
        b[50] = if self.perms == crate::index::PermSet::ALL {
            0
        } else {
            self.perms.bits()
        };
        // [51..64) reserved.

        // --- section directory ---
        // The five always-present sections (verbatim, so the named offsets
        // round-trip exactly), then the optional text index (only when present, so
        // a file without one stays byte-identical), then preserved unknown kinds.
        let entry = |kind, offset, length| Section {
            kind,
            flags: 0,
            offset,
            length,
        };
        let mut entries: Vec<Section> = vec![
            entry(
                SectionKind::Metadata,
                self.metadata_offset,
                self.metadata_len,
            ),
            entry(
                SectionKind::Dictionary,
                self.dictionary_offset,
                self.dictionary_len,
            ),
            entry(SectionKind::Index, self.root_dir_offset, self.root_dir_len),
            entry(
                SectionKind::PyramidMeta,
                self.pyramid_meta_offset,
                self.pyramid_meta_len,
            ),
            entry(
                SectionKind::NamedGraphs,
                self.named_graphs_offset,
                self.named_graphs_len,
            ),
        ];
        if self.text_index_len > 0 {
            entries.push(entry(
                if self.text_index_partial {
                    SectionKind::TextIndexPartial
                } else {
                    SectionKind::TextIndex
                },
                self.text_index_offset,
                self.text_index_len,
            ));
        }
        if self.build_info_len > 0 {
            entries.push(entry(
                SectionKind::BuildInfo,
                self.build_info_offset,
                self.build_info_len,
            ));
        }
        entries.extend(self.extra_sections.iter().copied());
        debug_assert!(
            entries.len() <= MAX_SECTIONS,
            "too many sections for a 1 KB header"
        );
        let n = entries.len().min(MAX_SECTIONS);
        b[44..46].copy_from_slice(&(n as u16).to_le_bytes());
        for (i, s) in entries.iter().take(n).enumerate() {
            let p = SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN;
            b[p..p + 2].copy_from_slice(&s.kind.to_u16().to_le_bytes());
            b[p + 2..p + 4].copy_from_slice(&s.flags.to_le_bytes());
            // [p+4..p+8) reserved.
            b[p + 8..p + 16].copy_from_slice(&s.offset.to_le_bytes());
            b[p + 16..p + 24].copy_from_slice(&s.length.to_le_bytes());
        }
        b
    }

    /// Parse a header from the first 1024 bytes of a file.
    pub fn from_bytes(b: &[u8]) -> Result<Self, HeaderError> {
        if b.len() < HEADER_LEN {
            return Err(HeaderError::TooSmall(b.len()));
        }
        if b[0..4] != MAGIC {
            return Err(HeaderError::BadMagic);
        }
        if b[4] == crate::v06::VERSION {
            // A 0x06 file: its 128-byte header, seen as a logical header (no
            // physical offsets — see `v06::logical_header`).
            return crate::v06::Header06::from_bytes(b)
                .map(|h| crate::v06::logical_header(&h))
                .map_err(|e| match e {
                    crate::file::FileError::Container(m) => HeaderError::Malformed06(m),
                    _ => HeaderError::Malformed06("unreadable"),
                });
        }
        if !(MIN_STABLE_READ_VERSION..=CURRENT_FORMAT_VERSION).contains(&b[4]) {
            return Err(HeaderError::UnsupportedVersion {
                found: b[4],
                min: MIN_STABLE_READ_VERSION,
                max: CURRENT_FORMAT_VERSION,
            });
        }
        let u16_at = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());

        let section_count = u16_at(44) as usize;
        if SECTION_DIR_OFFSET + section_count * SECTION_ENTRY_LEN > HEADER_LEN {
            return Err(HeaderError::BadSectionCount(section_count));
        }

        let perms = if b[50] == 0 {
            crate::index::PermSet::ALL
        } else {
            crate::index::PermSet::from_bits(b[50])
                .map_err(|why| HeaderError::BadPermMask { mask: b[50], why })?
        };

        let mut h = Header {
            version: b[4],
            flags: b[5],
            metadata_offset: 0,
            metadata_len: 0,
            dictionary_offset: 0,
            dictionary_len: 0,
            root_dir_offset: 0,
            root_dir_len: 0,
            pyramid_meta_offset: 0,
            pyramid_meta_len: 0,
            dict_codec: b[42],
            block_codec: b[43],
            pyramid_levels: u16_at(40),
            quad_count: u64_at(24),
            term_count: u64_at(32),
            content_hash: b[8..24].try_into().unwrap(),
            named_graphs_offset: 0,
            named_graphs_len: 0,
            schema_meta_len: u32_at(46),
            perms,
            text_index_offset: 0,
            text_index_len: 0,
            text_index_partial: false,
            build_info_offset: 0,
            build_info_len: 0,
            extra_sections: Vec::new(),
        };
        for i in 0..section_count {
            let p = SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN;
            let kind = SectionKind::from_u16(u16_at(p));
            let offset = u64_at(p + 8);
            let length = u64_at(p + 16);
            match kind {
                SectionKind::Metadata => {
                    h.metadata_offset = offset;
                    h.metadata_len = length;
                }
                SectionKind::Dictionary => {
                    h.dictionary_offset = offset;
                    h.dictionary_len = length;
                }
                SectionKind::Index => {
                    h.root_dir_offset = offset;
                    h.root_dir_len = length;
                }
                SectionKind::PyramidMeta => {
                    h.pyramid_meta_offset = offset;
                    h.pyramid_meta_len = length;
                }
                SectionKind::NamedGraphs => {
                    h.named_graphs_offset = offset;
                    h.named_graphs_len = length;
                }
                SectionKind::TextIndex => {
                    h.text_index_offset = offset;
                    h.text_index_len = length;
                    h.text_index_partial = false;
                }
                SectionKind::TextIndexPartial => {
                    h.text_index_offset = offset;
                    h.text_index_len = length;
                    h.text_index_partial = true;
                }
                SectionKind::BuildInfo => {
                    h.build_info_offset = offset;
                    h.build_info_len = length;
                }
                SectionKind::LabelIndex
                | SectionKind::Schema
                | SectionKind::QuadIndex
                | SectionKind::Unknown(_) => h.extra_sections.push(Section {
                    kind,
                    flags: u16_at(p + 2),
                    offset,
                    length,
                }),
            }
        }
        Ok(h)
    }

    /// The LABEL_INDEX section (kind 8), if the file carries a non-empty one.
    pub fn label_index_section(&self) -> Option<Section> {
        self.section(SectionKind::LabelIndex)
            .filter(|s| s.length > 0)
    }

    /// The SCHEMA section (kind 9), if the file carries a non-empty one.
    pub fn schema_section(&self) -> Option<Section> {
        self.section(SectionKind::Schema).filter(|s| s.length > 0)
    }

    pub fn has_quads(&self) -> bool {
        self.flags & FLAG_HAS_QUADS != 0
    }

    /// The total file length implied by this header alone: the writer lays the
    /// sections out back-to-back after the 1 KiB header and ends the file with
    /// the 4-byte `RETE` footer, so the furthest `offset + length` in the
    /// section directory plus 4 **is** the file length.
    ///
    /// This is the one length signal a *remote* reader can always trust: an
    /// HTTP host may advertise the size of a compressed representation in
    /// `Content-Length` (GitHub Pages gzips — a 71 MB file HEADs as 58 MB)
    /// while serving ranges over the identity bytes, and may hide
    /// `Content-Range` from cross-origin JS by omitting
    /// `Access-Control-Expose-Headers`. The header travels *in band* — in the
    /// first 1 KiB range read — so it cannot be skewed by transport encoding.
    ///
    /// `None` if any directory entry overflows `u64` (a crafted or corrupt
    /// header must yield "unknown", never a panic or a wrapped length — the
    /// weekly fuzz caught `verify()` on exactly this class of input).
    pub fn expected_file_len(&self) -> Option<u64> {
        if self.version == crate::v06::VERSION {
            // A 0x06 header states its length; its logical view parks every
            // offset at that length (see `v06::logical_header`).
            return Some(self.metadata_offset);
        }
        let mut end = HEADER_LEN as u64;
        let named = [
            (self.metadata_offset, self.metadata_len),
            (self.dictionary_offset, self.dictionary_len),
            (self.root_dir_offset, self.root_dir_len),
            (self.pyramid_meta_offset, self.pyramid_meta_len),
            (self.named_graphs_offset, self.named_graphs_len),
            (self.text_index_offset, self.text_index_len),
            (self.build_info_offset, self.build_info_len),
        ];
        let extras = self.extra_sections.iter().map(|s| (s.offset, s.length));
        for (offset, length) in named.into_iter().chain(extras) {
            if length == 0 {
                continue; // absent section — its (0, 0) entry says nothing
            }
            end = end.max(offset.checked_add(length)?);
        }
        end.checked_add(MAGIC.len() as u64)
    }

    /// Does the file contain RDF-star quoted triples ([`FLAG_HAS_QUOTED_TRIPLES`])?
    pub fn has_quoted_triples(&self) -> bool {
        self.flags & FLAG_HAS_QUOTED_TRIPLES != 0
    }

    /// Do the tiled index sections carry a [`FLAG_TILE_SYNOPSIS`] trailer?
    pub fn has_tile_synopsis(&self) -> bool {
        self.flags & FLAG_TILE_SYNOPSIS != 0
    }

    /// Does the full-text index cover the named graphs' literals too
    /// ([`FLAG_TEXT_INDEX_NAMED_GRAPHS`])? Known from the header alone.
    pub fn text_index_covers_named_graphs(&self) -> bool {
        self.flags & FLAG_TEXT_INDEX_NAMED_GRAPHS != 0
    }

    /// The `(offset, length)` of the text index's payload — the token-table
    /// varint, the token table and the postings. That is the whole section for
    /// kind 6; a kind-11 section starts with its 16-byte self-digest, which this
    /// skips. `(offset, 0)` past a kind-11 section too short for its digest.
    pub fn text_index_payload(&self) -> (u64, u64) {
        if self.text_index_partial {
            (
                self.text_index_offset
                    .saturating_add(TEXT_INDEX_PARTIAL_DIGEST_LEN),
                self.text_index_len
                    .saturating_sub(TEXT_INDEX_PARTIAL_DIGEST_LEN),
            )
        } else {
            (self.text_index_offset, self.text_index_len)
        }
    }

    /// The directory entry for a section kind, or `None` if absent. Known kinds
    /// read from the named convenience fields; unknown kinds from
    /// [`extra_sections`](Self::extra_sections).
    pub fn section(&self, kind: SectionKind) -> Option<Section> {
        let (offset, length) = match kind {
            SectionKind::Metadata => (self.metadata_offset, self.metadata_len),
            SectionKind::Dictionary => (self.dictionary_offset, self.dictionary_len),
            SectionKind::Index => (self.root_dir_offset, self.root_dir_len),
            SectionKind::PyramidMeta => (self.pyramid_meta_offset, self.pyramid_meta_len),
            SectionKind::NamedGraphs => (self.named_graphs_offset, self.named_graphs_len),
            SectionKind::TextIndex if !self.text_index_partial => {
                (self.text_index_offset, self.text_index_len)
            }
            SectionKind::TextIndexPartial if self.text_index_partial => {
                (self.text_index_offset, self.text_index_len)
            }
            SectionKind::TextIndex | SectionKind::TextIndexPartial => (0, 0),
            SectionKind::BuildInfo => (self.build_info_offset, self.build_info_len),
            SectionKind::LabelIndex
            | SectionKind::Schema
            | SectionKind::QuadIndex
            | SectionKind::Unknown(_) => {
                return self.extra_sections.iter().find(|s| s.kind == kind).copied()
            }
        };
        Some(Section {
            kind,
            flags: 0,
            offset,
            length,
        })
    }

    /// Attach (or overwrite) a section's `(offset, length)` — the extension point
    /// for new top-level sections. Known kinds set the named fields; an unknown
    /// kind is appended to [`extra_sections`](Self::extra_sections).
    pub fn with_section(mut self, kind: SectionKind, offset: u64, length: u64) -> Self {
        match kind {
            SectionKind::Metadata => {
                self.metadata_offset = offset;
                self.metadata_len = length;
            }
            SectionKind::Dictionary => {
                self.dictionary_offset = offset;
                self.dictionary_len = length;
            }
            SectionKind::Index => {
                self.root_dir_offset = offset;
                self.root_dir_len = length;
            }
            SectionKind::PyramidMeta => {
                self.pyramid_meta_offset = offset;
                self.pyramid_meta_len = length;
            }
            SectionKind::NamedGraphs => {
                self.named_graphs_offset = offset;
                self.named_graphs_len = length;
            }
            SectionKind::TextIndex => {
                self.text_index_offset = offset;
                self.text_index_len = length;
                self.text_index_partial = false;
            }
            SectionKind::TextIndexPartial => {
                self.text_index_offset = offset;
                self.text_index_len = length;
                self.text_index_partial = true;
            }
            SectionKind::BuildInfo => {
                self.build_info_offset = offset;
                self.build_info_len = length;
            }
            SectionKind::LabelIndex | SectionKind::Schema | SectionKind::QuadIndex => {
                // A known post-freeze kind appears at most once: overwrite.
                match self.extra_sections.iter_mut().find(|s| s.kind == kind) {
                    Some(s) => {
                        s.offset = offset;
                        s.length = length;
                    }
                    None => self.extra_sections.push(Section {
                        kind,
                        flags: 0,
                        offset,
                        length,
                    }),
                }
            }
            SectionKind::Unknown(_) => self.extra_sections.push(Section {
                kind,
                flags: 0,
                offset,
                length,
            }),
        }
        self
    }

    /// Are the named graphs stored in the quad index ([`FLAG_QUAD_INDEX`])?
    pub fn has_quad_index(&self) -> bool {
        self.flags & FLAG_QUAD_INDEX != 0
    }

    /// The QUAD_INDEX section, when the directory lists a non-empty one.
    pub fn quad_index_section(&self) -> Option<Section> {
        self.section(SectionKind::QuadIndex)
            .filter(|s| s.length > 0)
    }

    /// How many sections the index container (kind 3) holds: one per stored
    /// permutation, plus the quad-index seal when [`FLAG_QUAD_INDEX`] is set.
    pub fn index_container_sections(&self) -> usize {
        self.perms.len() + usize::from(self.has_quad_index())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Header {
        Header {
            version: LEGACY_FORMAT_VERSION,
            flags: FLAG_HAS_QUADS,
            metadata_offset: 1024,
            metadata_len: 42,
            dictionary_offset: 1066,
            dictionary_len: 2048,
            root_dir_offset: 3114,
            root_dir_len: 256,
            pyramid_meta_offset: 3370,
            pyramid_meta_len: 64,
            dict_codec: 1,
            block_codec: 2,
            pyramid_levels: 3,
            perms: crate::index::PermSet::ALL,
            quad_count: 5,
            term_count: 9,
            content_hash: [7u8; 16],
            named_graphs_offset: 3434,
            named_graphs_len: 48,
            schema_meta_len: 99,
            text_index_offset: 0,
            text_index_len: 0,
            text_index_partial: false,
            build_info_offset: 0,
            build_info_len: 0,
            extra_sections: Vec::new(),
        }
    }

    #[test]
    fn round_trip() {
        let h = sample();
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(&bytes[0..4], b"RETE");
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        assert!(back.has_quads());
    }

    #[test]
    fn byte_layout_matches_spec() {
        // Pins the core fields and the first directory entry to exact offsets.
        let h = Header {
            content_hash: [0xCC; 16],
            quad_count: 0x99,
            term_count: 0xAA,
            pyramid_levels: 0xABCD,
            dict_codec: 0xA1,
            block_codec: 0xA2,
            schema_meta_len: 0xD00D,
            metadata_offset: 0x11,
            metadata_len: 0x22,
            dictionary_offset: 0x33,
            dictionary_len: 0x44,
            ..sample()
        };
        let b = h.to_bytes();
        let u16_at = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());

        // core
        assert_eq!(&b[0..4], b"RETE");
        assert_eq!(b[4], LEGACY_FORMAT_VERSION);
        assert_eq!(b[5], FLAG_HAS_QUADS);
        assert_eq!(u16_at(6), HEADER_LEN as u16);
        assert_eq!(&b[8..24], &[0xCC; 16]); // content hash
        assert_eq!(u64_at(24), 0x99); // quad count
        assert_eq!(u64_at(32), 0xAA); // term count
        assert_eq!(u16_at(40), 0xABCD); // pyramid levels
        assert_eq!(b[42], 0xA1); // dict codec
        assert_eq!(b[43], 0xA2); // block codec
        assert_eq!(u16_at(44), 5); // section_count: the 5 known sections
        assert_eq!(u32::from_le_bytes(b[46..50].try_into().unwrap()), 0xD00D); // schema-meta len
                                                                               // first directory entry = Metadata, at offset 64
        assert_eq!(u16_at(64), 1); // kind = Metadata
        assert_eq!(u64_at(72), 0x11); // metadata offset
        assert_eq!(u64_at(80), 0x22); // metadata length
                                      // second entry = Dictionary, at 88
        assert_eq!(u16_at(88), 2);
        assert_eq!(u64_at(96), 0x33);
        assert_eq!(u64_at(104), 0x44);
        assert_eq!(b.len(), HEADER_LEN);
    }

    #[test]
    fn expected_file_len_is_last_section_end_plus_footer() {
        // sample(): named graphs end furthest, at 3434 + 48 = 3482.
        assert_eq!(sample().expected_file_len(), Some(3482 + 4));

        // A zero-length section's offset must not count (absent sections are
        // written as (0, 0), and a stale offset with len 0 addresses nothing).
        let mut h = sample();
        h.text_index_offset = 1 << 40;
        h.text_index_len = 0;
        assert_eq!(h.expected_file_len(), Some(3482 + 4));

        // An empty file (header + footer only) is 1028 bytes.
        let mut e = sample();
        e.metadata_len = 0;
        e.dictionary_len = 0;
        e.root_dir_len = 0;
        e.pyramid_meta_len = 0;
        e.named_graphs_len = 0;
        assert_eq!(e.expected_file_len(), Some(1024 + 4));

        // A crafted header whose entry overflows u64 yields None, not a panic
        // or a wrapped (tiny) length.
        let mut c = sample();
        c.named_graphs_offset = u64::MAX - 8;
        c.named_graphs_len = 64;
        assert_eq!(c.expected_file_len(), None);

        // Unknown (future) sections extend the file too.
        let f = sample().with_section(SectionKind::Unknown(99), 1 << 20, 512);
        assert_eq!(f.expected_file_len(), Some((1 << 20) + 512 + 4));
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[4] = CURRENT_FORMAT_VERSION;
        assert!(matches!(
            Header::from_bytes(&bytes),
            Err(HeaderError::BadMagic)
        ));
    }

    #[test]
    fn stable_reader_accepts_v1_baseline_and_rejects_pre_v1() {
        let current = sample().to_bytes();
        assert_eq!(current[4], 0x05);
        assert_eq!(Header::from_bytes(&current).unwrap().version, 0x05);

        for old in 0x01..=0x04 {
            let mut bytes = current;
            bytes[4] = old;
            let error = Header::from_bytes(&bytes).unwrap_err();
            assert!(matches!(
                &error,
                HeaderError::UnsupportedVersion {
                    found,
                    min: 0x05,
                    max: 0x06
                } if *found == old
            ));
            assert!(error
                .to_string()
                .contains("Pre-1.0 files must be rebuilt from RDF source with `rete build`"));
        }

        for unsupported in [0x00, 0x07, 0xff] {
            let mut bytes = current;
            bytes[4] = unsupported;
            assert!(matches!(
                Header::from_bytes(&bytes),
                Err(HeaderError::UnsupportedVersion {
                    found,
                    min: 0x05,
                    max: 0x06
                }) if found == unsupported
            ));
        }
    }

    #[test]
    fn rejects_overrunning_section_count() {
        let mut bad = sample().to_bytes();
        bad[44..46].copy_from_slice(&9999u16.to_le_bytes());
        assert!(matches!(
            Header::from_bytes(&bad),
            Err(HeaderError::BadSectionCount(9999))
        ));
    }

    #[test]
    fn unknown_section_survives_round_trip() {
        // A section a future build added (kind 99) must be preserved verbatim by a
        // reader that doesn't know it, and readable via `section()`.
        let h = sample().with_section(SectionKind::Unknown(99), 4096, 512);
        let back = Header::from_bytes(&h.to_bytes()).unwrap();
        assert_eq!(back.extra_sections.len(), 1);
        let s = back.section(SectionKind::Unknown(99)).unwrap();
        assert_eq!((s.offset, s.length), (4096, 512));
        // Known sections still resolve.
        let dict = back.section(SectionKind::Dictionary).unwrap();
        assert_eq!(dict.offset, h.dictionary_offset);
        assert_eq!(h, back);
    }

    #[test]
    fn build_info_section_round_trips_and_is_optional() {
        // Absent (len 0): only the 5 always-present sections — a file without a
        // build-info section is byte-identical to one built before it existed.
        let plain = sample().to_bytes();
        assert_eq!(u16::from_le_bytes(plain[44..46].try_into().unwrap()), 5);
        assert_eq!(sample().section(SectionKind::BuildInfo).unwrap().length, 0);

        // Present: a directory entry that round-trips and resolves as kind 7.
        let h = sample().with_section(SectionKind::BuildInfo, 1066, 300);
        let bytes = h.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[44..46].try_into().unwrap()), 6);
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        let s = back.section(SectionKind::BuildInfo).unwrap();
        assert_eq!((s.offset, s.length), (1066, 300));
        assert!(back.extra_sections.is_empty(), "BuildInfo is a known kind");
        // And it extends the expected file length like any other section.
        let far = sample().with_section(SectionKind::BuildInfo, 1 << 21, 128);
        assert_eq!(far.expected_file_len(), Some((1 << 21) + 128 + 4));
    }

    #[test]
    fn label_index_and_schema_sections_are_known_extras() {
        // Absent: the directory is unchanged (5 entries), so a file without
        // them is byte-identical to one built before they existed.
        assert!(sample().label_index_section().is_none());
        assert!(sample().schema_section().is_none());

        let h = sample()
            .with_section(SectionKind::LabelIndex, 8000, 300)
            .with_section(SectionKind::Schema, 8300, 40);
        let bytes = h.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[44..46].try_into().unwrap()), 7);
        // entries 5 and 6 carry kinds 8 and 9 at the directory's byte layout
        assert_eq!(
            u16::from_le_bytes(bytes[64 + 5 * 24..][..2].try_into().unwrap()),
            8
        );
        assert_eq!(
            u16::from_le_bytes(bytes[64 + 6 * 24..][..2].try_into().unwrap()),
            9
        );
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        assert_eq!(back.extra_sections.len(), 2, "stored with the extras");
        let l = back.label_index_section().unwrap();
        assert_eq!((l.offset, l.length), (8000, 300));
        let s = back.schema_section().unwrap();
        assert_eq!((s.offset, s.length), (8300, 40));
        assert_eq!(back.expected_file_len(), Some(8340 + 4));

        // A known post-freeze kind is overwritten, never duplicated.
        let again = back.with_section(SectionKind::Schema, 9000, 10);
        assert_eq!(again.extra_sections.len(), 2);
        assert_eq!(again.schema_section().unwrap().offset, 9000);

        // A zero-length entry reads as absent.
        let empty = sample().with_section(SectionKind::LabelIndex, 0, 0);
        assert!(empty.label_index_section().is_none());
    }

    #[test]
    fn quad_index_flag_and_section_round_trip() {
        // Absent: no flag, no kind-10 entry, the container is the permutations.
        let plain = sample();
        assert!(!plain.has_quad_index());
        assert!(plain.quad_index_section().is_none());
        assert_eq!(plain.index_container_sections(), 6);

        // Present: flag bit 3, a kind-10 entry kept with the extra sections,
        // and one more container section (the seal).
        let mut h = sample().with_section(SectionKind::QuadIndex, 7000, 900);
        h.flags |= FLAG_QUAD_INDEX;
        h.named_graphs_offset = 0;
        h.named_graphs_len = 0;
        let bytes = h.to_bytes();
        assert_eq!(bytes[5] & 0b1000, 0b1000, "flag bit 3");
        let n = u16::from_le_bytes(bytes[44..46].try_into().unwrap()) as usize;
        let kinds: Vec<u16> = (0..n)
            .map(|i| {
                let p = SECTION_DIR_OFFSET + i * SECTION_ENTRY_LEN;
                u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap())
            })
            .collect();
        assert!(kinds.contains(&10), "a kind-10 directory entry: {kinds:?}");
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(back, h);
        assert!(back.has_quad_index());
        let s = back.quad_index_section().unwrap();
        assert_eq!(
            (s.kind, s.offset, s.length),
            (SectionKind::QuadIndex, 7000, 900)
        );
        assert_eq!(back.index_container_sections(), 7);
        assert_eq!(back.expected_file_len(), Some(7000 + 900 + 4));
        // Three permutations + seal = four: never the 3 or 6 an old reader wants.
        let mut lean = back.clone();
        lean.perms = crate::index::PermSet::CORE;
        assert_eq!(lean.index_container_sections(), 4);
        // Re-attaching overwrites rather than duplicating.
        let again = back.with_section(SectionKind::QuadIndex, 8000, 10);
        assert_eq!(again.extra_sections.len(), 1);
        assert_eq!(again.quad_index_section().unwrap().offset, 8000);
    }

    #[test]
    fn text_index_section_round_trips_and_is_optional() {
        // Absent (len 0): only the 5 always-present sections, so a file without a
        // text index is byte-identical to one built before this section existed.
        assert_eq!(
            u16::from_le_bytes(sample().to_bytes()[44..46].try_into().unwrap()),
            5
        );
        assert!(sample().section(SectionKind::TextIndex).unwrap().length == 0);

        // Present: a 6th directory entry that round-trips and resolves.
        let h = sample().with_section(SectionKind::TextIndex, 5000, 4096);
        let bytes = h.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[44..46].try_into().unwrap()), 6);
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        let s = back.section(SectionKind::TextIndex).unwrap();
        assert_eq!((s.offset, s.length), (5000, 4096));
        assert!(back.extra_sections.is_empty(), "TextIndex is a known kind");
        assert!(!back.text_index_partial);
    }

    /// A predicate-restricted text index is written as kind 11, maps onto the
    /// same `text_index_*` fields, and a reader that only knows kind 6 sees an
    /// unknown section — no text index at all — rather than a full one.
    #[test]
    fn partial_text_index_is_kind_11_and_invisible_as_kind_6() {
        let h = sample().with_section(SectionKind::TextIndexPartial, 5000, 4096);
        assert!(h.text_index_partial);
        let bytes = h.to_bytes();
        assert_eq!(u16::from_le_bytes(bytes[44..46].try_into().unwrap()), 6);
        // The 6th entry, at 64 + 5 × 24, carries kind 11.
        let p = SECTION_DIR_OFFSET + 5 * SECTION_ENTRY_LEN;
        assert_eq!(u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap()), 11);
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        assert!(back.text_index_partial);
        assert_eq!((back.text_index_offset, back.text_index_len), (5000, 4096));
        let s = back.section(SectionKind::TextIndexPartial).unwrap();
        assert_eq!((s.offset, s.length), (5000, 4096));
        assert_eq!(back.section(SectionKind::TextIndex).unwrap().length, 0);
        assert!(back.extra_sections.is_empty(), "kind 11 is known here");

        // What a reader that predates kind 11 decodes: an unknown kind, which
        // it preserves, and empty text-index fields — so it neither searches
        // nor prunes with the partial index.
        let mut old_view = bytes;
        old_view[p..p + 2].copy_from_slice(&0x7777u16.to_le_bytes());
        let old = Header::from_bytes(&old_view).unwrap();
        assert_eq!(old.text_index_len, 0);
        assert_eq!(old.extra_sections.len(), 1);
    }
}
