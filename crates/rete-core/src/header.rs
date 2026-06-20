//! The fixed-size 128-byte file header (SPEC.md §4.1).
//!
//! A client's very first request is `bytes=0-127`; this struct is what those
//! bytes decode to, and it points at every other section of the file.

use std::convert::TryInto;

/// Magic bytes at offset 0: ASCII `RETE`.
pub const MAGIC: [u8; 4] = *b"RETE";

/// Current format version (written by this crate): tiled permutation sections.
pub const VERSION: u8 = 0x02;

/// Oldest format version this crate still reads (single-block permutation
/// sections, pre-tiling).
pub const MIN_READ_VERSION: u8 = 0x01;

/// Fixed header size in bytes.
pub const HEADER_LEN: usize = 128;

/// Flag bit: the file contains named graphs (quads) rather than triples only.
pub const FLAG_HAS_QUADS: u8 = 0b0000_0001;

/// Flag bit: each tiled permutation section carries a **tile-synopsis trailer**
/// — per-tile min/max of the two non-leading columns, appended after the tile
/// payloads. It lets a range reader prune a routed tile by a bound secondary
/// component *before* fetching it. Backward-compatible: the trailer sits past the
/// last tile, so a reader that predates this flag locates tiles by length and
/// never reads it. See `file.rs::encode_tiled_section`.
pub const FLAG_TILE_SYNOPSIS: u8 = 0b0000_0010;

#[derive(Debug, thiserror::Error)]
pub enum HeaderError {
    #[error("buffer too small: need {HEADER_LEN} bytes, got {0}")]
    TooSmall(usize),
    #[error("bad magic: expected RETE")]
    BadMagic,
    #[error("unsupported version: {0:#x}")]
    BadVersion(u8),
}

/// Decoded file header. All multi-byte fields are little-endian on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    /// Format version of the parsed file (within
    /// [`MIN_READ_VERSION`]..=[`VERSION`]); decoders gate layout changes on it.
    /// The writer always emits [`VERSION`].
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
    /// section (0 if none, e.g. a typeless or pre-v0.2.1 file). It lets a reader
    /// fetch *only* the schema block — at `pyramid_meta_offset + pyramid_meta_len -
    /// schema_meta_len` — for an index/dictionary/summary-free Tier-0 coherence
    /// check, instead of reading the whole (graph-scaling) pyramid-meta.
    pub schema_meta_len: u32,
}

impl Header {
    /// Serialize into a fixed 128-byte array.
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&MAGIC);
        b[4] = self.version;
        b[5] = self.flags;
        b[6..8].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        b[8..16].copy_from_slice(&self.metadata_offset.to_le_bytes());
        b[16..24].copy_from_slice(&self.metadata_len.to_le_bytes());
        b[24..32].copy_from_slice(&self.dictionary_offset.to_le_bytes());
        b[32..40].copy_from_slice(&self.dictionary_len.to_le_bytes());
        b[40..48].copy_from_slice(&self.root_dir_offset.to_le_bytes());
        b[48..56].copy_from_slice(&self.root_dir_len.to_le_bytes());
        b[56..64].copy_from_slice(&self.pyramid_meta_offset.to_le_bytes());
        b[64..72].copy_from_slice(&self.pyramid_meta_len.to_le_bytes());
        b[72] = self.dict_codec;
        b[73] = self.block_codec;
        b[74..76].copy_from_slice(&self.pyramid_levels.to_le_bytes());
        b[76..84].copy_from_slice(&self.quad_count.to_le_bytes());
        b[84..92].copy_from_slice(&self.term_count.to_le_bytes());
        b[92..108].copy_from_slice(&self.content_hash);
        b[108..116].copy_from_slice(&self.named_graphs_offset.to_le_bytes());
        b[116..124].copy_from_slice(&self.named_graphs_len.to_le_bytes());
        b[124..128].copy_from_slice(&self.schema_meta_len.to_le_bytes());
        b
    }

    /// Parse a header from the first 128 bytes of a file.
    pub fn from_bytes(b: &[u8]) -> Result<Self, HeaderError> {
        if b.len() < HEADER_LEN {
            return Err(HeaderError::TooSmall(b.len()));
        }
        if b[0..4] != MAGIC {
            return Err(HeaderError::BadMagic);
        }
        if !(MIN_READ_VERSION..=VERSION).contains(&b[4]) {
            return Err(HeaderError::BadVersion(b[4]));
        }
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());
        Ok(Header {
            version: b[4],
            flags: b[5],
            metadata_offset: u64_at(8),
            metadata_len: u64_at(16),
            dictionary_offset: u64_at(24),
            dictionary_len: u64_at(32),
            root_dir_offset: u64_at(40),
            root_dir_len: u64_at(48),
            pyramid_meta_offset: u64_at(56),
            pyramid_meta_len: u64_at(64),
            dict_codec: b[72],
            block_codec: b[73],
            pyramid_levels: u16::from_le_bytes(b[74..76].try_into().unwrap()),
            quad_count: u64_at(76),
            term_count: u64_at(84),
            content_hash: b[92..108].try_into().unwrap(),
            named_graphs_offset: u64_at(108),
            named_graphs_len: u64_at(116),
            schema_meta_len: u32::from_le_bytes(b[124..128].try_into().unwrap()),
        })
    }

    pub fn has_quads(&self) -> bool {
        self.flags & FLAG_HAS_QUADS != 0
    }

    /// Do the tiled index sections carry a [`FLAG_TILE_SYNOPSIS`] trailer?
    pub fn has_tile_synopsis(&self) -> bool {
        self.flags & FLAG_TILE_SYNOPSIS != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let h = Header {
            version: VERSION,
            flags: FLAG_HAS_QUADS,
            metadata_offset: 128,
            metadata_len: 42,
            dictionary_offset: 170,
            dictionary_len: 1024,
            root_dir_offset: 1194,
            root_dir_len: 256,
            pyramid_meta_offset: 1450,
            pyramid_meta_len: 64,
            dict_codec: 1,
            block_codec: 2,
            pyramid_levels: 3,
            quad_count: 5,
            term_count: 9,
            content_hash: [7u8; 16],
            named_graphs_offset: 2000,
            named_graphs_len: 48,
            schema_meta_len: 99,
        };
        let bytes = h.to_bytes();
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(&bytes[0..4], b"RETE");
        let back = Header::from_bytes(&bytes).unwrap();
        assert_eq!(h, back);
        assert!(back.has_quads());
    }

    #[test]
    fn byte_layout_matches_spec() {
        // Pins each field to the exact offset documented in SPEC.md §4.1. The
        // round-trip test above is symmetric and would survive a field reorder;
        // this would not. Distinct values make each field's bytes identifiable.
        let h = Header {
            version: VERSION,
            flags: FLAG_HAS_QUADS,
            metadata_offset: 0x11,
            metadata_len: 0x22,
            dictionary_offset: 0x33,
            dictionary_len: 0x44,
            root_dir_offset: 0x55,
            root_dir_len: 0x66,
            pyramid_meta_offset: 0x77,
            pyramid_meta_len: 0x88,
            dict_codec: 0xA1,
            block_codec: 0xA2,
            pyramid_levels: 0xABCD,
            quad_count: 0x99,
            term_count: 0xAA,
            content_hash: [0xCC; 16],
            named_graphs_offset: 0xBEEF,
            named_graphs_len: 0xF00D,
            schema_meta_len: 0xD00D,
        };
        let b = h.to_bytes();
        let u16_at = |o: usize| u16::from_le_bytes(b[o..o + 2].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(b[o..o + 8].try_into().unwrap());

        assert_eq!(&b[0..4], b"RETE");
        assert_eq!(b[4], VERSION);
        assert_eq!(b[5], FLAG_HAS_QUADS);
        assert_eq!(u16_at(6), HEADER_LEN as u16);
        assert_eq!(u64_at(8), 0x11); // metadata offset
        assert_eq!(u64_at(16), 0x22); // metadata length
        assert_eq!(u64_at(24), 0x33); // dictionary offset
        assert_eq!(u64_at(32), 0x44); // dictionary length
        assert_eq!(u64_at(40), 0x55); // root dir offset
        assert_eq!(u64_at(48), 0x66); // root dir length
        assert_eq!(u64_at(56), 0x77); // pyramid-meta offset
        assert_eq!(u64_at(64), 0x88); // pyramid-meta length
        assert_eq!(b[72], 0xA1); // dict codec
        assert_eq!(b[73], 0xA2); // block codec
        assert_eq!(u16_at(74), 0xABCD); // pyramid levels
        assert_eq!(u64_at(76), 0x99); // quad count
        assert_eq!(u64_at(84), 0xAA); // term count
        assert_eq!(&b[92..108], &[0xCC; 16]); // content hash
        assert_eq!(u64_at(108), 0xBEEF); // named-graphs offset
        assert_eq!(u64_at(116), 0xF00D); // named-graphs length
        assert_eq!(u32::from_le_bytes(b[124..128].try_into().unwrap()), 0xD00D); // schema-meta len
        assert_eq!(b.len(), HEADER_LEN);
    }

    #[test]
    fn rejects_bad_magic() {
        let mut bytes = [0u8; HEADER_LEN];
        bytes[4] = VERSION;
        assert!(matches!(
            Header::from_bytes(&bytes),
            Err(HeaderError::BadMagic)
        ));
    }

    #[test]
    fn accepts_v1_and_rejects_unknown_versions() {
        let h = Header {
            version: MIN_READ_VERSION,
            flags: 0,
            metadata_offset: HEADER_LEN as u64,
            metadata_len: 0,
            dictionary_offset: HEADER_LEN as u64,
            dictionary_len: 0,
            root_dir_offset: HEADER_LEN as u64,
            root_dir_len: 0,
            pyramid_meta_offset: 0,
            pyramid_meta_len: 0,
            dict_codec: 0,
            block_codec: 0,
            pyramid_levels: 0,
            quad_count: 0,
            term_count: 0,
            content_hash: [0; 16],
            named_graphs_offset: 0,
            named_graphs_len: 0,
            schema_meta_len: 0,
        };
        let back = Header::from_bytes(&h.to_bytes()).unwrap();
        assert_eq!(back.version, MIN_READ_VERSION);

        let mut bad = h.to_bytes();
        bad[4] = VERSION + 1;
        assert!(matches!(
            Header::from_bytes(&bad),
            Err(HeaderError::BadVersion(_))
        ));
        bad[4] = 0;
        assert!(matches!(
            Header::from_bytes(&bad),
            Err(HeaderError::BadVersion(0))
        ));
    }
}
