//! Format generation `0x06` (SPEC.md §4): everything a reader needs to plan a
//! query sits at the **front** of the file — a 128-byte header, the root block,
//! the Dataset Card, the planner statistics and every leaf directory — and the
//! payload follows, clustered by section.
//!
//! ```text
//! 0          header (128 B)
//! 128        root block: section table + directory roots
//!            card region: card | build record | slack
//!            front sections: stats+schema, then the leaf directories
//! data_off   data sections: dictionary chunks, index tiles, quad tiles,
//!            label blocks, text postings, pyramid
//! file_len-8 footer "RETE" 06 00 00 00
//! ```
//!
//! Payload blocks (dictionary chunks, index tiles, quad tiles, label blocks,
//! postings) are byte-identical to `0x05`; only their framing and their
//! directories move. [`parts`] lifts a `0x05` file into [`parts::Parts`],
//! [`write`] assembles a `0x06` file from them, [`open`] reads one.

pub mod open;
pub mod parts;
pub mod write;

#[cfg(test)]
mod roundtrip;

use crate::file::FileError;
use crate::varint::{read_uvarint, write_uvarint};

/// The format byte of this generation.
pub const VERSION: u8 = 0x06;
/// Fixed header length.
pub const HEADER_LEN: usize = 128;
/// What a reader fetches first: header, root, card, statistics and, on a small
/// file, every leaf directory (SPEC.md §9).
pub const FIRST_READ: u64 = 64 * 1024;
/// The 8-byte footer: the generation, then `RETE` — so every reader that
/// confirms a length by the file's last four bytes finds the magic there.
pub const FOOTER: [u8; 8] = *b"\x06\x00\x00\x00RETE";
/// The writer's root budget: leaves grow until the stored root fits.
pub const ROOT_BUDGET: usize = 40 * 1024;
/// Starting (uncompressed) size of a leaf block.
pub const LEAF_START: usize = 16 * 1024;
/// Slack added to the card region beyond twice the card + record.
pub const CARD_SLACK_MIN: u32 = 8 * 1024;

/// Header flag: the file has named graphs (in the quad index).
pub const F_NAMED: u8 = 0b0001;
/// Header flag: some terms are RDF-star quoted triples.
pub const F_QUOTED: u8 = 0b0010;
/// Header flag: the text index also covers the named graphs' literals.
pub const F_TEXT_NAMED: u8 = 0b0100;
/// Header flag: quad tiles are clustered graph-major (all orders of a block of
/// graphs adjacent) rather than order-major.
pub const F_QUAD_GRAPH_MAJOR: u8 = 0b1000;

/// Presence bit: a text index.
pub const P_TEXT: u16 = 1;
/// Presence bit: the text index leaves some predicates out (kind 11 in 0x05).
pub const P_TEXT_PARTIAL: u16 = 2;
/// Presence bit: a label index.
pub const P_LABELS: u16 = 4;
/// Presence bit: a stats + schema section.
pub const P_SCHEMA: u16 = 8;
/// Presence bit: the community pyramid.
pub const P_PYRAMID: u16 = 16;
/// Presence bit: the quad index.
pub const P_QUAD: u16 = 32;

/// Section kinds of the root's section table.
#[allow(missing_docs)]
pub mod kind {
    pub const SCHEMA: u8 = 1;
    pub const DICT_LEAVES: u8 = 2;
    pub const INDEX_LEAVES: u8 = 3;
    pub const QUAD_LEAVES: u8 = 4;
    pub const LABEL_LEAVES: u8 = 5;
    pub const TEXT_LEAVES: u8 = 6;
    pub const DICT_DATA: u8 = 16;
    pub const INDEX_DATA: u8 = 17;
    pub const QUAD_DATA: u8 = 18;
    pub const LABEL_DATA: u8 = 19;
    pub const TEXT_DATA: u8 = 20;
    pub const PYRAMID: u8 = 21;

    /// Every kind, in file order (front kinds, then data kinds).
    pub const ORDER: [u8; 12] = [
        SCHEMA,
        DICT_LEAVES,
        INDEX_LEAVES,
        QUAD_LEAVES,
        LABEL_LEAVES,
        TEXT_LEAVES,
        DICT_DATA,
        INDEX_DATA,
        QUAD_DATA,
        LABEL_DATA,
        TEXT_DATA,
        PYRAMID,
    ];

    /// A short name, for `verify --section` and `info`.
    pub fn name(k: u8) -> &'static str {
        match k {
            SCHEMA => "schema",
            DICT_LEAVES => "dict-leaves",
            INDEX_LEAVES => "index-leaves",
            QUAD_LEAVES => "quad-leaves",
            LABEL_LEAVES => "label-leaves",
            TEXT_LEAVES => "text-leaves",
            DICT_DATA => "dict",
            INDEX_DATA => "index",
            QUAD_DATA => "quad",
            LABEL_DATA => "labels",
            TEXT_DATA => "text",
            PYRAMID => "pyramid",
            _ => "unknown",
        }
    }

    /// Front sections live before the data region.
    pub fn is_front(k: u8) -> bool {
        k < DICT_DATA
    }
}

/// Unit-root tags inside the root block.
#[allow(missing_docs)]
pub mod tag {
    pub const DICT: u8 = 1;
    pub const INDEX: u8 = 2;
    pub const QUAD: u8 = 3;
    pub const LABELS: u8 = 4;
    pub const TEXT: u8 = 5;
}

/// The parsed 128-byte header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct Header06 {
    pub flags: u8,
    pub content_hash: [u8; 16],
    pub file_len: u64,
    pub root_len: u32,
    pub card_region_len: u32,
    pub data_offset: u64,
    pub quad_count: u64,
    pub term_count: u64,
    pub graph_count: u64,
    pub perm_mask: u8,
    pub dict_codec: u8,
    pub block_codec: u8,
    pub internal_codec: u8,
    pub pyramid_levels: u16,
    pub presence: u16,
    pub card_off: u32,
    pub card_len: u32,
    pub card_digest: [u8; 16],
    pub build_off: u32,
    pub build_len: u32,
    pub root_digest: [u8; 16],
}

impl Header06 {
    /// Serialize the fixed 128 bytes.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&crate::header::MAGIC);
        b[4] = VERSION;
        b[5] = self.flags;
        b[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        b[8..24].copy_from_slice(&self.content_hash);
        b[24..32].copy_from_slice(&self.file_len.to_le_bytes());
        b[32..36].copy_from_slice(&self.root_len.to_le_bytes());
        b[36..40].copy_from_slice(&self.card_region_len.to_le_bytes());
        b[40..48].copy_from_slice(&self.data_offset.to_le_bytes());
        b[48..56].copy_from_slice(&self.quad_count.to_le_bytes());
        b[56..64].copy_from_slice(&self.term_count.to_le_bytes());
        b[64..72].copy_from_slice(&self.graph_count.to_le_bytes());
        b[72] = self.perm_mask;
        b[73] = self.dict_codec;
        b[74] = self.block_codec;
        b[75] = self.internal_codec;
        b[76..78].copy_from_slice(&self.pyramid_levels.to_le_bytes());
        b[78..80].copy_from_slice(&self.presence.to_le_bytes());
        b[80..84].copy_from_slice(&self.card_off.to_le_bytes());
        b[84..88].copy_from_slice(&self.card_len.to_le_bytes());
        b[88..104].copy_from_slice(&self.card_digest);
        b[104..108].copy_from_slice(&self.build_off.to_le_bytes());
        b[108..112].copy_from_slice(&self.build_len.to_le_bytes());
        b[112..128].copy_from_slice(&self.root_digest);
        b
    }

    /// Parse and bounds-check the fixed 128 bytes. Every field is untrusted.
    pub fn from_bytes(b: &[u8]) -> Result<Self, FileError> {
        if b.len() < HEADER_LEN {
            return Err(FileError::Container("0x06 header truncated"));
        }
        if b[0..4] != crate::header::MAGIC || b[4] != VERSION {
            return Err(FileError::Container("not a 0x06 file"));
        }
        if u16::from_le_bytes([b[6], b[7]]) as usize != HEADER_LEN {
            return Err(FileError::Container("0x06 header length is not 128"));
        }
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        let h16 = |o: usize| {
            let mut x = [0u8; 16];
            x.copy_from_slice(&b[o..o + 16]);
            x
        };
        let h = Header06 {
            flags: b[5],
            content_hash: h16(8),
            file_len: u64_at(24),
            root_len: u32_at(32),
            card_region_len: u32_at(36),
            data_offset: u64_at(40),
            quad_count: u64_at(48),
            term_count: u64_at(56),
            graph_count: u64_at(64),
            perm_mask: b[72],
            dict_codec: b[73],
            block_codec: b[74],
            internal_codec: b[75],
            pyramid_levels: u16::from_le_bytes([b[76], b[77]]),
            presence: u16::from_le_bytes([b[78], b[79]]),
            card_off: u32_at(80),
            card_len: u32_at(84),
            card_digest: h16(88),
            build_off: u32_at(104),
            build_len: u32_at(108),
            root_digest: h16(112),
        };
        // Structural bounds: the front must fit in the file, the card and the
        // record in their region.
        let front_min = (HEADER_LEN as u64)
            .checked_add(h.root_len as u64)
            .and_then(|x| x.checked_add(h.card_region_len as u64))
            .ok_or(FileError::Container("0x06 front overflows"))?;
        if front_min > h.data_offset || h.data_offset > h.file_len {
            return Err(FileError::Container("0x06 front overruns the data region"));
        }
        let card_end = h.card_off as u64 + h.card_len as u64;
        let build_end = h.build_off as u64 + h.build_len as u64;
        if card_end > h.card_region_len as u64 || build_end > h.card_region_len as u64 {
            return Err(FileError::Container("0x06 card region overrun"));
        }
        crate::index::PermSet::from_bits(h.perm_mask)
            .map_err(|_| FileError::Container("0x06 permutation mask invalid"))?;
        Ok(h)
    }

    /// Absolute offset of the root block.
    pub fn root_offset(&self) -> u64 {
        HEADER_LEN as u64
    }
    /// Absolute offset of the card region.
    pub fn card_region_offset(&self) -> u64 {
        HEADER_LEN as u64 + self.root_len as u64
    }
    /// Absolute offset where the front sections start (after the card region).
    pub fn front_base(&self) -> u64 {
        self.card_region_offset() + self.card_region_len as u64
    }
    /// The stored card's absolute range (`len == 0`: no card).
    pub fn card_range(&self) -> crate::file::ByteRange {
        crate::file::ByteRange {
            offset: self.card_region_offset() + self.card_off as u64,
            len: self.card_len as u64,
        }
    }
    /// The build record's absolute range (`len == 0`: none).
    pub fn build_range(&self) -> crate::file::ByteRange {
        crate::file::ByteRange {
            offset: self.card_region_offset() + self.build_off as u64,
            len: self.build_len as u64,
        }
    }
    /// The permutations the file carries.
    pub fn perms(&self) -> crate::index::PermSet {
        crate::index::PermSet::from_bits(self.perm_mask).unwrap_or(crate::index::PermSet::ALL)
    }
    /// Does the file carry this presence bit?
    pub fn has(&self, bit: u16) -> bool {
        self.presence & bit != 0
    }
}

/// The first 16 bytes of a BLAKE3 digest.
pub fn digest16(bytes: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    out.copy_from_slice(&blake3::hash(bytes).as_bytes()[..16]);
    out
}

/// The content hash of a header whose card and root digests are set (SPEC.md
/// §5): the logical header fields, the card and the root, never an offset and
/// never the build record.
pub fn content_hash(h: &Header06) -> [u8; 16] {
    let b = h.to_bytes();
    let mut x = blake3::Hasher::new();
    x.update(b"rete/0x06");
    x.update(&b[4..8]);
    x.update(&b[48..80]);
    x.update(&h.card_len.to_le_bytes());
    x.update(&h.card_digest);
    x.update(&h.root_len.to_le_bytes());
    x.update(&h.root_digest);
    let mut out = [0u8; 16];
    out.copy_from_slice(&x.finalize().as_bytes()[..16]);
    out
}

/// One entry of the root's section table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sec06 {
    /// [`kind`].
    pub kind: u8,
    /// Offset relative to its region base (the front base, or `data_offset`).
    pub offset: u64,
    /// Stored length.
    pub len: u64,
    /// `BLAKE3(stored bytes)[..16]`.
    pub digest: [u8; 16],
}

/// Size of one section-table entry.
pub const SEC_ENTRY_LEN: usize = 34;

impl Sec06 {
    /// The absolute byte range of this section in a file with header `h`.
    pub fn abs(&self, h: &Header06) -> crate::file::ByteRange {
        let base = if kind::is_front(self.kind) {
            h.front_base()
        } else {
            h.data_offset
        };
        crate::file::ByteRange {
            offset: base + self.offset,
            len: self.len,
        }
    }
}

/// The decoded root block.
#[derive(Debug, Clone, Default)]
pub struct Root06 {
    /// The section table, in file order.
    pub sections: Vec<Sec06>,
    /// The unit roots, decompressed: `(tag, payload)`.
    pub units: Vec<(u8, Vec<u8>)>,
}

impl Root06 {
    /// The section of `kind`, if present.
    pub fn section(&self, k: u8) -> Option<&Sec06> {
        self.sections.iter().find(|s| s.kind == k)
    }

    /// The unit root payload with `tag`, if present.
    pub fn unit(&self, t: u8) -> Option<&[u8]> {
        self.units
            .iter()
            .find(|(x, _)| *x == t)
            .map(|(_, p)| p.as_slice())
    }

    /// Serialize: the fixed section table, then the compressed unit roots.
    pub fn encode(&self, codec: u8) -> Vec<u8> {
        let mut units = Vec::new();
        for (t, p) in &self.units {
            units.push(*t);
            write_uvarint(&mut units, p.len() as u64);
            units.extend_from_slice(p);
        }
        let stored = crate::file::compress(codec, &units);
        let mut out =
            Vec::with_capacity(2 + SEC_ENTRY_LEN * self.sections.len() + 4 + stored.len());
        out.push(1u8);
        out.push(self.sections.len() as u8);
        for s in &self.sections {
            out.push(s.kind);
            out.push(0);
            out.extend_from_slice(&s.offset.to_le_bytes());
            out.extend_from_slice(&s.len.to_le_bytes());
            out.extend_from_slice(&s.digest);
        }
        out.extend_from_slice(&(stored.len() as u32).to_le_bytes());
        out.extend_from_slice(&stored);
        out
    }

    /// Parse a stored root block. Every count and length is untrusted.
    pub fn decode(bytes: &[u8], codec: u8) -> Result<Self, FileError> {
        let bad = |w| FileError::Container(w);
        if bytes.len() < 2 || bytes[0] != 1 {
            return Err(bad("0x06 root block: unknown version"));
        }
        let n = bytes[1] as usize;
        let table_end = 2 + n * SEC_ENTRY_LEN;
        if bytes.len() < table_end + 4 {
            return Err(bad("0x06 root block truncated"));
        }
        let mut sections = Vec::with_capacity(n);
        for i in 0..n {
            let e = &bytes[2 + i * SEC_ENTRY_LEN..2 + (i + 1) * SEC_ENTRY_LEN];
            let mut digest = [0u8; 16];
            digest.copy_from_slice(&e[18..34]);
            sections.push(Sec06 {
                kind: e[0],
                offset: u64::from_le_bytes(e[2..10].try_into().unwrap()),
                len: u64::from_le_bytes(e[10..18].try_into().unwrap()),
                digest,
            });
        }
        let ulen = u32::from_le_bytes(bytes[table_end..table_end + 4].try_into().unwrap()) as usize;
        let stored = bytes
            .get(table_end + 4..table_end + 4 + ulen)
            .ok_or(bad("0x06 root units overrun"))?;
        let units_raw = crate::file::decompress(codec, stored)?;
        let mut units = Vec::new();
        let mut c = Cur::new(&units_raw);
        while !c.done() {
            let t = c.u8()?;
            let l = c.len()?;
            units.push((t, c.bytes(l)?.to_vec()));
        }
        Ok(Root06 { sections, units })
    }
}

/// A bounds-checked cursor over untrusted bytes.
pub struct Cur<'a> {
    b: &'a [u8],
    /// Current position.
    pub pos: usize,
}

// `len` reads a length varint; it is not the cursor's own length.
#[allow(clippy::len_without_is_empty)]
impl<'a> Cur<'a> {
    /// A cursor at the start of `b`.
    pub fn new(b: &'a [u8]) -> Self {
        Cur { b, pos: 0 }
    }
    /// At the end?
    pub fn done(&self) -> bool {
        self.pos >= self.b.len()
    }
    /// One varint.
    pub fn uv(&mut self) -> Result<u64, FileError> {
        let (v, n) = read_uvarint(self.b.get(self.pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("0x06: truncated varint"))?;
        self.pos += n;
        Ok(v)
    }
    /// One varint as a `u32`.
    pub fn u32v(&mut self) -> Result<u32, FileError> {
        u32::try_from(self.uv()?).map_err(|_| FileError::Container("0x06: value exceeds u32"))
    }
    /// One varint as a length bounded by the remaining bytes.
    pub fn len(&mut self) -> Result<usize, FileError> {
        let v = self.uv()?;
        let rest = self.b.len().saturating_sub(self.pos) as u64;
        if v > rest {
            return Err(FileError::Container("0x06: length overruns"));
        }
        Ok(v as usize)
    }
    /// One varint as a count, sanity-bounded by `max`.
    pub fn count(&mut self, max: u64) -> Result<usize, FileError> {
        let v = self.uv()?;
        if v > max {
            return Err(FileError::Container("0x06: count out of range"));
        }
        Ok(v as usize)
    }
    /// One byte.
    pub fn u8(&mut self) -> Result<u8, FileError> {
        let v = *self
            .b
            .get(self.pos)
            .ok_or(FileError::Container("0x06: truncated"))?;
        self.pos += 1;
        Ok(v)
    }
    /// `n` raw bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], FileError> {
        let s = self
            .b
            .get(
                self.pos
                    ..self
                        .pos
                        .checked_add(n)
                        .ok_or(FileError::Container("0x06: overflow"))?,
            )
            .ok_or(FileError::Container("0x06: truncated bytes"))?;
        self.pos += n;
        Ok(s)
    }
    /// A length-prefixed byte string.
    pub fn lp(&mut self) -> Result<&'a [u8], FileError> {
        let n = self.len()?;
        self.bytes(n)
    }
}

/// Append a length-prefixed byte string.
pub fn put_lp(out: &mut Vec<u8>, b: &[u8]) {
    write_uvarint(out, b.len() as u64);
    out.extend_from_slice(b);
}

// ---------------------------------------------------------------------------
// Unit roots
// ---------------------------------------------------------------------------

/// One leaf of a two-level directory, as the root describes it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LeafRef {
    /// Index of the leaf's first entry in the whole unit.
    pub first: usize,
    /// Entries in the leaf.
    pub n: usize,
    /// Leaf offset within its leaves section.
    pub off: u64,
    /// Stored (compressed) leaf length.
    pub len: u64,
}

/// The root of one dictionary section.
#[derive(Debug, Clone, Default)]
pub struct DictSecRoot {
    /// Terms in the section.
    pub term_count: u32,
    /// Restart interval of its front-coded runs.
    pub restart_interval: u32,
    /// Chunks in the section.
    pub n_chunks: usize,
    /// Offset of the section's first chunk within DICT_DATA.
    pub data_off: u64,
    /// Bytes of chunk data.
    pub data_len: u64,
    /// Leaves: `(leaf, first run, chunk offset within the section's data, first key)`.
    pub leaves: Vec<(LeafRef, usize, u64, Vec<u8>)>,
}

/// Encode the dictionary unit root.
pub fn encode_dict_root(secs: &[DictSecRoot; 4]) -> Vec<u8> {
    let mut o = Vec::new();
    for s in secs {
        write_uvarint(&mut o, s.term_count as u64);
        write_uvarint(&mut o, s.restart_interval as u64);
        write_uvarint(&mut o, s.n_chunks as u64);
        write_uvarint(&mut o, s.data_off);
        write_uvarint(&mut o, s.data_len);
        write_uvarint(&mut o, s.leaves.len() as u64);
        for (l, run, coff, key) in &s.leaves {
            write_uvarint(&mut o, l.n as u64);
            write_uvarint(&mut o, l.off);
            write_uvarint(&mut o, l.len);
            write_uvarint(&mut o, *run as u64);
            write_uvarint(&mut o, *coff);
            put_lp(&mut o, key);
        }
    }
    o
}

/// Decode the dictionary unit root.
pub fn decode_dict_root(p: &[u8]) -> Result<[DictSecRoot; 4], FileError> {
    let mut c = Cur::new(p);
    let mut out: [DictSecRoot; 4] = Default::default();
    for s in out.iter_mut() {
        s.term_count = c.u32v()?;
        s.restart_interval = c.u32v()?;
        s.n_chunks = c.count(u32::MAX as u64)?;
        s.data_off = c.uv()?;
        s.data_len = c.uv()?;
        let nl = c.count(p.len() as u64)?;
        let mut first = 0usize;
        for _ in 0..nl {
            let n = c.count(u32::MAX as u64)?;
            let off = c.uv()?;
            let len = c.uv()?;
            let run = c.count(u64::MAX)?;
            let coff = c.uv()?;
            let key = c.lp()?.to_vec();
            s.leaves
                .push((LeafRef { first, n, off, len }, run, coff, key));
            first = first
                .checked_add(n)
                .ok_or(FileError::Container("0x06 dict leaves overflow"))?;
        }
        if first != s.n_chunks {
            return Err(FileError::Container(
                "0x06 dict leaves do not cover the chunks",
            ));
        }
        if s.restart_interval == 0 && s.term_count > 0 {
            return Err(FileError::Container("0x06 dict zero restart interval"));
        }
    }
    Ok(out)
}

/// The root of one permutation's tile directory.
#[derive(Debug, Clone, Default)]
pub struct PermRoot {
    /// Section index (`IndexPermutation::section_index`).
    pub si: usize,
    /// Tiles.
    pub n_tiles: usize,
    /// Offset of the permutation's first tile within INDEX_DATA.
    pub data_off: u64,
    /// Bytes of tiles.
    pub data_len: u64,
    /// Leaves: `(leaf, first min_a, last max_a, tile offset within the permutation's data)`.
    pub leaves: Vec<(LeafRef, u32, u32, u64)>,
}

/// Encode the index unit root (permutations in section-index order).
pub fn encode_index_root(perms: &[PermRoot]) -> Vec<u8> {
    let mut o = Vec::new();
    write_uvarint(&mut o, perms.len() as u64);
    for p in perms {
        write_uvarint(&mut o, p.si as u64);
        write_uvarint(&mut o, p.n_tiles as u64);
        write_uvarint(&mut o, p.data_off);
        write_uvarint(&mut o, p.data_len);
        write_uvarint(&mut o, p.leaves.len() as u64);
        for (l, mn, mx, toff) in &p.leaves {
            write_uvarint(&mut o, l.n as u64);
            write_uvarint(&mut o, l.off);
            write_uvarint(&mut o, l.len);
            write_uvarint(&mut o, *mn as u64);
            write_uvarint(&mut o, *mx as u64);
            write_uvarint(&mut o, *toff);
        }
    }
    o
}

/// Decode the index unit root.
pub fn decode_index_root(p: &[u8]) -> Result<Vec<PermRoot>, FileError> {
    let mut c = Cur::new(p);
    let n = c.count(6)?;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let si = c.count(5)?;
        let n_tiles = c.count(u32::MAX as u64)?;
        let data_off = c.uv()?;
        let data_len = c.uv()?;
        let nl = c.count(p.len() as u64)?;
        let mut leaves = Vec::with_capacity(nl);
        let mut first = 0usize;
        for _ in 0..nl {
            let ln = c.count(u32::MAX as u64)?;
            let off = c.uv()?;
            let len = c.uv()?;
            let mn = c.u32v()?;
            let mx = c.u32v()?;
            let toff = c.uv()?;
            leaves.push((
                LeafRef {
                    first,
                    n: ln,
                    off,
                    len,
                },
                mn,
                mx,
                toff,
            ));
            first = first
                .checked_add(ln)
                .ok_or(FileError::Container("0x06 index leaves overflow"))?;
        }
        if first != n_tiles {
            return Err(FileError::Container(
                "0x06 index leaves do not cover the tiles",
            ));
        }
        out.push(PermRoot {
            si,
            n_tiles,
            data_off,
            data_len,
            leaves,
        });
    }
    Ok(out)
}

/// The root of the text index.
#[derive(Debug, Clone, Default)]
pub struct TextRoot {
    /// The scope block (empty for the default scope).
    pub scope: Vec<u8>,
    /// Distinct tokens.
    pub n_tokens: usize,
    /// Leaves: `(leaf, first token)`.
    pub leaves: Vec<(LeafRef, Vec<u8>)>,
}

/// Encode the text unit root.
pub fn encode_text_root(t: &TextRoot) -> Vec<u8> {
    let mut o = Vec::new();
    put_lp(&mut o, &t.scope);
    write_uvarint(&mut o, t.n_tokens as u64);
    write_uvarint(&mut o, t.leaves.len() as u64);
    for (l, first) in &t.leaves {
        write_uvarint(&mut o, l.n as u64);
        write_uvarint(&mut o, l.off);
        write_uvarint(&mut o, l.len);
        put_lp(&mut o, first);
    }
    o
}

/// Decode the text unit root.
pub fn decode_text_root(p: &[u8]) -> Result<TextRoot, FileError> {
    let mut c = Cur::new(p);
    let scope = c.lp()?.to_vec();
    let n_tokens = c.count(u32::MAX as u64)?;
    let nl = c.count(p.len() as u64)?;
    let mut leaves = Vec::with_capacity(nl);
    let mut first = 0usize;
    for _ in 0..nl {
        let n = c.count(u32::MAX as u64)?;
        let off = c.uv()?;
        let len = c.uv()?;
        let tok = c.lp()?.to_vec();
        leaves.push((LeafRef { first, n, off, len }, tok));
        first = first
            .checked_add(n)
            .ok_or(FileError::Container("0x06 text leaves overflow"))?;
    }
    if first != n_tokens {
        return Err(FileError::Container(
            "0x06 text leaves do not cover the tokens",
        ));
    }
    Ok(TextRoot {
        scope,
        n_tokens,
        leaves,
    })
}

/// The root of the label index: the 0x05 preamble and root node, and where the
/// data blocks end in the 0x05 body coordinates (SPEC.md §6.4).
#[derive(Debug, Clone, Default)]
pub struct LabelRoot {
    /// Bytes of data blocks (the 0x05 body offsets below this are blocks, the
    /// rest are directory nodes).
    pub data_len: u64,
    /// The 32-byte 0x05 preamble (digest zeroed) followed by the root node.
    pub head: Vec<u8>,
}

/// Encode the label unit root.
pub fn encode_label_root(l: &LabelRoot) -> Vec<u8> {
    let mut o = Vec::new();
    write_uvarint(&mut o, l.data_len);
    put_lp(&mut o, &l.head);
    o
}

/// Decode the label unit root.
pub fn decode_label_root(p: &[u8]) -> Result<LabelRoot, FileError> {
    let mut c = Cur::new(p);
    let data_len = c.uv()?;
    let head = c.lp()?.to_vec();
    if head.len() < 32 {
        return Err(FileError::Container("0x06 label root truncated"));
    }
    Ok(LabelRoot { data_len, head })
}

/// The root of the quad index.
#[derive(Debug, Clone, Default)]
pub struct QuadRoot {
    /// Named graphs.
    pub graph_count: u64,
    /// Named-graph quads.
    pub quad_count: u64,
    /// Graphs per graph-directory block.
    pub block_graphs: u64,
    /// The 0x05 `P0`: the graph-directory block index (offsets from the start of
    /// QUAD_LEAVES, where the blocks are).
    pub p0: Vec<u8>,
    /// Bytes of graph-directory blocks at the start of QUAD_LEAVES.
    pub gdir_len: u64,
    /// Per order present (mask order): `(section index, tiles, leaves)`; a leaf
    /// is `(leaf, first (g, a), last (g, a), offset of its first tile in QUAD_DATA)`.
    pub orders: Vec<QuadOrderRoot>,
}

/// One order of the quad index.
#[derive(Debug, Clone, Default)]
pub struct QuadOrderRoot {
    /// Section index of the order (`G+X` uses `X`'s index).
    pub si: usize,
    /// Tiles.
    pub n_tiles: usize,
    /// Leaves.
    pub leaves: Vec<QuadLeafRoot>,
}

/// A quad order's leaf in the root: the leaf, the first tile's first `(g, a)`,
/// the last tile's last `(g, a)`, and the first tile's offset in `quad`.
pub type QuadLeafRoot = (LeafRef, (u32, u32), (u32, u32), u64);

/// Encode the quad unit root.
pub fn encode_quad_root(q: &QuadRoot) -> Vec<u8> {
    let mut o = Vec::new();
    write_uvarint(&mut o, q.graph_count);
    write_uvarint(&mut o, q.quad_count);
    write_uvarint(&mut o, q.block_graphs);
    put_lp(&mut o, &q.p0);
    write_uvarint(&mut o, q.gdir_len);
    write_uvarint(&mut o, q.orders.len() as u64);
    for ord in &q.orders {
        write_uvarint(&mut o, ord.si as u64);
        write_uvarint(&mut o, ord.n_tiles as u64);
        write_uvarint(&mut o, ord.leaves.len() as u64);
        for (l, f, la, toff) in &ord.leaves {
            write_uvarint(&mut o, l.n as u64);
            write_uvarint(&mut o, l.off);
            write_uvarint(&mut o, l.len);
            write_uvarint(&mut o, f.0 as u64);
            write_uvarint(&mut o, f.1 as u64);
            write_uvarint(&mut o, la.0 as u64);
            write_uvarint(&mut o, la.1 as u64);
            write_uvarint(&mut o, *toff);
        }
    }
    o
}

/// Decode the quad unit root.
pub fn decode_quad_root(p: &[u8]) -> Result<QuadRoot, FileError> {
    let mut c = Cur::new(p);
    let graph_count = c.uv()?;
    let quad_count = c.uv()?;
    let block_graphs = c.uv()?;
    let p0 = c.lp()?.to_vec();
    let gdir_len = c.uv()?;
    let no = c.count(6)?;
    let mut orders = Vec::with_capacity(no);
    for _ in 0..no {
        let si = c.count(5)?;
        let n_tiles = c.count(u32::MAX as u64)?;
        let nl = c.count(p.len() as u64)?;
        let mut leaves = Vec::with_capacity(nl);
        let mut first = 0usize;
        for _ in 0..nl {
            let n = c.count(u32::MAX as u64)?;
            let off = c.uv()?;
            let len = c.uv()?;
            let f = (c.u32v()?, c.u32v()?);
            let la = (c.u32v()?, c.u32v()?);
            let toff = c.uv()?;
            leaves.push((LeafRef { first, n, off, len }, f, la, toff));
            first = first
                .checked_add(n)
                .ok_or(FileError::Container("0x06 quad leaves overflow"))?;
        }
        if first != n_tiles {
            return Err(FileError::Container(
                "0x06 quad leaves do not cover the tiles",
            ));
        }
        orders.push(QuadOrderRoot {
            si,
            n_tiles,
            leaves,
        });
    }
    if block_graphs == 0 && graph_count > 0 {
        return Err(FileError::Container("0x06 quad block size is zero"));
    }
    Ok(QuadRoot {
        graph_count,
        quad_count,
        block_graphs,
        p0,
        gdir_len,
        orders,
    })
}

// ---------------------------------------------------------------------------
// Leaf entries
// ---------------------------------------------------------------------------

/// One dictionary chunk as a leaf lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkEnt {
    /// Index of the chunk's first front-coded run.
    pub first_run: usize,
    /// Routing separator (SPEC.md §6.2).
    pub key: Vec<u8>,
    /// Compressed chunk length.
    pub clen: u64,
}

/// Encode chunk entries (the first `first_run` absolute, then deltas).
pub fn encode_chunk_leaf(ents: &[ChunkEnt]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut prev = 0usize;
    for (i, e) in ents.iter().enumerate() {
        write_uvarint(
            &mut o,
            (if i == 0 {
                e.first_run
            } else {
                e.first_run - prev
            }) as u64,
        );
        put_lp(&mut o, &e.key);
        write_uvarint(&mut o, e.clen);
        prev = e.first_run;
    }
    o
}

/// Decode `n` chunk entries.
pub fn decode_chunk_leaf(b: &[u8], n: usize) -> Result<Vec<ChunkEnt>, FileError> {
    let mut c = Cur::new(b);
    let mut out = Vec::with_capacity(n.min(b.len()));
    let mut prev = 0usize;
    for i in 0..n {
        let d = c.count(u64::MAX)?;
        let first_run = if i == 0 {
            d
        } else {
            prev.checked_add(d)
                .ok_or(FileError::Container("0x06 chunk run overflow"))?
        };
        let key = c.lp()?.to_vec();
        let clen = c.uv()?;
        out.push(ChunkEnt {
            first_run,
            key,
            clen,
        });
        prev = first_run;
    }
    Ok(out)
}

/// One index tile as a leaf lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEnt {
    /// Leading-id range.
    pub min_a: u32,
    /// Leading-id range, inclusive.
    pub max_a: u32,
    /// Compressed length.
    pub clen: u64,
    /// `(min_b, max_b, min_c, max_c)`.
    pub syn: (u32, u32, u32, u32),
}

/// Encode tile entries: the 0x05 directory rows, then the synopsis rows.
pub fn encode_tile_leaf(ents: &[TileEnt]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut prev = 0u32;
    for (i, e) in ents.iter().enumerate() {
        write_uvarint(
            &mut o,
            (if i == 0 {
                e.min_a
            } else {
                e.min_a.wrapping_sub(prev)
            }) as u64,
        );
        write_uvarint(&mut o, e.max_a.wrapping_sub(e.min_a) as u64);
        write_uvarint(&mut o, e.clen);
        prev = e.min_a;
    }
    for e in ents {
        write_uvarint(&mut o, e.syn.0 as u64);
        write_uvarint(&mut o, e.syn.1.wrapping_sub(e.syn.0) as u64);
        write_uvarint(&mut o, e.syn.2 as u64);
        write_uvarint(&mut o, e.syn.3.wrapping_sub(e.syn.2) as u64);
    }
    o
}

/// Decode `n` tile entries.
pub fn decode_tile_leaf(b: &[u8], n: usize) -> Result<Vec<TileEnt>, FileError> {
    let mut c = Cur::new(b);
    let mut out = Vec::with_capacity(n.min(b.len()));
    let mut prev = 0u32;
    for i in 0..n {
        let d = c.u32v()?;
        let min_a = if i == 0 { d } else { prev.wrapping_add(d) };
        let span = c.u32v()?;
        let clen = c.uv()?;
        out.push(TileEnt {
            min_a,
            max_a: min_a.wrapping_add(span),
            clen,
            syn: (0, 0, 0, 0),
        });
        prev = min_a;
    }
    for e in out.iter_mut() {
        let mb = c.u32v()?;
        let db = c.u32v()?;
        let mc = c.u32v()?;
        let dc = c.u32v()?;
        e.syn = (mb, mb.wrapping_add(db), mc, mc.wrapping_add(dc));
    }
    Ok(out)
}

/// One text token as a leaf lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenEnt {
    /// The token.
    pub token: Vec<u8>,
    /// Posting offset within TEXT_DATA.
    pub off: u64,
    /// Posting length.
    pub len: u64,
}

/// Encode token entries, front-coded within the leaf.
pub fn encode_token_leaf(ents: &[TokenEnt]) -> Vec<u8> {
    let mut o = Vec::new();
    let mut prev: &[u8] = &[];
    for e in ents {
        let shared = prev
            .iter()
            .zip(&e.token)
            .take_while(|(a, b)| a == b)
            .count();
        write_uvarint(&mut o, shared as u64);
        put_lp(&mut o, &e.token[shared..]);
        write_uvarint(&mut o, e.off);
        write_uvarint(&mut o, e.len);
        prev = &e.token;
    }
    o
}

/// Decode `n` token entries.
pub fn decode_token_leaf(b: &[u8], n: usize) -> Result<Vec<TokenEnt>, FileError> {
    let mut c = Cur::new(b);
    let mut out: Vec<TokenEnt> = Vec::with_capacity(n.min(b.len()));
    for _ in 0..n {
        let shared = c.count(u32::MAX as u64)?;
        let suffix = c.lp()?;
        let mut token = match out.last() {
            Some(p) if shared <= p.token.len() => p.token[..shared].to_vec(),
            None if shared == 0 => Vec::new(),
            _ => return Err(FileError::Container("0x06 token front-coding out of range")),
        };
        token.extend_from_slice(suffix);
        let off = c.uv()?;
        let len = c.uv()?;
        out.push(TokenEnt { token, off, len });
    }
    Ok(out)
}

/// One quad tile as a leaf lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QTileEnt {
    /// Leading key of the first row.
    pub first: (u32, u32),
    /// Leading key of the last row.
    pub last: (u32, u32),
    /// Stored length.
    pub clen: u64,
    /// Stored raw (not compressed).
    pub raw: bool,
    /// `(min_b, max_b, min_c, max_c)`.
    pub syn: (u32, u32, u32, u32),
    /// Absolute offset of the tile within QUAD_DATA.
    pub off: u64,
}

/// Encode quad tile entries. A tile's offset is written as `0` when it follows
/// the previous tile of the leaf (or, for the first, sits at the leaf's root
/// offset), else as `offset + 1` (PMTiles' clustered encoding).
pub fn encode_qtile_leaf(ents: &[QTileEnt], leaf_off: u64) -> Vec<u8> {
    let mut o = Vec::new();
    let mut prev_g = 0u32;
    let mut next = leaf_off;
    for (i, e) in ents.iter().enumerate() {
        write_uvarint(
            &mut o,
            (if i == 0 {
                e.first.0
            } else {
                e.first.0 - prev_g
            }) as u64,
        );
        write_uvarint(&mut o, e.first.1 as u64);
        write_uvarint(&mut o, (e.last.0 - e.first.0) as u64);
        write_uvarint(&mut o, e.last.1 as u64);
        write_uvarint(&mut o, (e.clen << 1) | e.raw as u64);
        write_uvarint(&mut o, e.syn.0 as u64);
        write_uvarint(&mut o, e.syn.1.wrapping_sub(e.syn.0) as u64);
        write_uvarint(&mut o, e.syn.2 as u64);
        write_uvarint(&mut o, e.syn.3.wrapping_sub(e.syn.2) as u64);
        write_uvarint(&mut o, if e.off == next { 0 } else { e.off + 1 });
        next = e.off + e.clen;
        prev_g = e.first.0;
    }
    o
}

/// Decode `n` quad tile entries of a leaf whose first tile sits at `leaf_off`.
pub fn decode_qtile_leaf(b: &[u8], n: usize, leaf_off: u64) -> Result<Vec<QTileEnt>, FileError> {
    let mut c = Cur::new(b);
    let mut out = Vec::with_capacity(n.min(b.len()));
    let mut prev_g = 0u32;
    let mut next = leaf_off;
    for i in 0..n {
        let d = c.u32v()?;
        let g = if i == 0 {
            d
        } else {
            prev_g
                .checked_add(d)
                .ok_or(FileError::Container("0x06 quad graph overflow"))?
        };
        let fa = c.u32v()?;
        let dg = c.u32v()?;
        let la = c.u32v()?;
        let packed = c.uv()?;
        let mb = c.u32v()?;
        let db = c.u32v()?;
        let mc = c.u32v()?;
        let dc = c.u32v()?;
        let oc = c.uv()?;
        let off = if oc == 0 { next } else { oc - 1 };
        let clen = packed >> 1;
        out.push(QTileEnt {
            first: (g, fa),
            last: (
                g.checked_add(dg)
                    .ok_or(FileError::Container("0x06 quad graph overflow"))?,
                la,
            ),
            clen,
            raw: packed & 1 == 1,
            syn: (mb, mb.wrapping_add(db), mc, mc.wrapping_add(dc)),
            off,
        });
        next = off
            .checked_add(clen)
            .ok_or(FileError::Container("0x06 quad offset overflow"))?;
        prev_g = g;
    }
    Ok(out)
}

/// Cut `sizes` (each entry's encoded size) into leaves of about `target`
/// bytes: `(start, end)` index ranges, never empty, covering everything.
pub fn cut(sizes: &[usize], target: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut acc = 0usize;
    for (i, &s) in sizes.iter().enumerate() {
        acc += s;
        if acc >= target {
            out.push((start, i + 1));
            start = i + 1;
            acc = 0;
        }
    }
    if start < sizes.len() {
        out.push((start, sizes.len()));
    }
    out
}

/// A logical, 0x05-shaped [`crate::header::Header`] for a 0x06 file, so code
/// that asks presence questions (`text_index_len > 0`, `label_index_section()`)
/// answers them the same for both formats. Its offsets all point at the end of
/// the file, so a physical read that was not ported to 0x06 fails loudly instead
/// of reading the wrong bytes.
pub fn logical_header(h: &Header06) -> crate::header::Header {
    use crate::header::{Section, SectionKind};
    let end = h.file_len;
    let mut flags = crate::header::FLAG_TILE_SYNOPSIS;
    if h.flags & F_NAMED != 0 {
        flags |= crate::header::FLAG_HAS_QUADS;
    }
    if h.flags & F_QUOTED != 0 {
        flags |= crate::header::FLAG_HAS_QUOTED_TRIPLES;
    }
    if h.has(P_QUAD) {
        flags |= crate::header::FLAG_QUAD_INDEX;
    }
    if h.flags & F_TEXT_NAMED != 0 && h.has(P_TEXT) {
        flags |= crate::header::FLAG_TEXT_INDEX_NAMED_GRAPHS;
    }
    let present = |bit: u16| if h.has(bit) { 1u64 } else { 0 };
    let mut extra = Vec::new();
    if h.has(P_LABELS) {
        extra.push(Section {
            kind: SectionKind::LabelIndex,
            flags: 0,
            offset: end,
            length: 1,
        });
    }
    if h.has(P_SCHEMA) {
        extra.push(Section {
            kind: SectionKind::Schema,
            flags: 0,
            offset: end,
            length: 1,
        });
    }
    if h.has(P_QUAD) {
        extra.push(Section {
            kind: SectionKind::QuadIndex,
            flags: 0,
            offset: end,
            length: 1,
        });
    }
    crate::header::Header {
        version: VERSION,
        flags,
        metadata_offset: end,
        metadata_len: h.card_len as u64,
        dictionary_offset: end,
        dictionary_len: 0,
        root_dir_offset: end,
        root_dir_len: 0,
        pyramid_meta_offset: end,
        pyramid_meta_len: present(P_PYRAMID),
        dict_codec: h.dict_codec,
        block_codec: h.block_codec,
        pyramid_levels: h.pyramid_levels,
        quad_count: h.quad_count,
        term_count: h.term_count,
        content_hash: h.content_hash,
        named_graphs_offset: 0,
        named_graphs_len: 0,
        schema_meta_len: 0,
        text_index_offset: end,
        text_index_len: present(P_TEXT),
        text_index_partial: h.has(P_TEXT_PARTIAL),
        build_info_offset: end,
        build_info_len: h.build_len as u64,
        perms: h.perms(),
        extra_sections: extra,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_round_trips() {
        let h = Header06 {
            flags: F_NAMED | F_QUAD_GRAPH_MAJOR,
            content_hash: [7; 16],
            file_len: 10_000,
            root_len: 300,
            card_region_len: 100,
            data_offset: 2_000,
            quad_count: 5,
            term_count: 9,
            graph_count: 2,
            perm_mask: 0b11_1111,
            dict_codec: 1,
            block_codec: 1,
            internal_codec: 1,
            pyramid_levels: 0,
            presence: P_TEXT | P_QUAD,
            card_off: 0,
            card_len: 40,
            card_digest: [3; 16],
            build_off: 40,
            build_len: 20,
            root_digest: [4; 16],
        };
        let b = h.to_bytes();
        assert_eq!(Header06::from_bytes(&b).unwrap(), h);
        // The hash ignores offsets and the build record.
        let mut h2 = h.clone();
        h2.data_offset = 3_000;
        h2.build_len = 0;
        h2.file_len = 11_000;
        assert_eq!(content_hash(&h), content_hash(&h2));
        h2.card_digest = [9; 16];
        assert_ne!(content_hash(&h), content_hash(&h2));
    }

    #[test]
    fn leaves_round_trip() {
        let c = vec![
            ChunkEnt {
                first_run: 0,
                key: vec![],
                clen: 10,
            },
            ChunkEnt {
                first_run: 5,
                key: b"ab".to_vec(),
                clen: 20,
            },
        ];
        assert_eq!(decode_chunk_leaf(&encode_chunk_leaf(&c), 2).unwrap(), c);
        let t = vec![
            TileEnt {
                min_a: 3,
                max_a: 9,
                clen: 100,
                syn: (1, 4, 2, 8),
            },
            TileEnt {
                min_a: 9,
                max_a: 20,
                clen: 7,
                syn: (0, 0, 5, 5),
            },
        ];
        assert_eq!(decode_tile_leaf(&encode_tile_leaf(&t), 2).unwrap(), t);
        let k = vec![
            TokenEnt {
                token: b"apple".to_vec(),
                off: 0,
                len: 3,
            },
            TokenEnt {
                token: b"apply".to_vec(),
                off: 3,
                len: 4,
            },
        ];
        assert_eq!(decode_token_leaf(&encode_token_leaf(&k), 2).unwrap(), k);
        let q = vec![
            QTileEnt {
                first: (5, 1),
                last: (6, 2),
                clen: 30,
                raw: true,
                syn: (0, 1, 2, 3),
                off: 100,
            },
            QTileEnt {
                first: (6, 3),
                last: (9, 0),
                clen: 10,
                raw: false,
                syn: (4, 4, 4, 4),
                off: 500,
            },
        ];
        assert_eq!(
            decode_qtile_leaf(&encode_qtile_leaf(&q, 100), 2, 100).unwrap(),
            q
        );
    }

    #[test]
    fn cut_covers_everything() {
        assert_eq!(cut(&[5, 5, 5, 5, 5], 10), vec![(0, 2), (2, 4), (4, 5)]);
        assert_eq!(cut(&[], 10), Vec::<(usize, usize)>::new());
        assert_eq!(cut(&[50], 10), vec![(0, 1)]);
    }
}
