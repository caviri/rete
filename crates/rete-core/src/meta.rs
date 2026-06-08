//! Pyramid-meta section encoding (SPEC.md §7.3): the summary (quotient) graph
//! plus the per-community tile directory, stored in the `.rete` file at the
//! header's `pyramid_meta_offset`.
//!
//! Layout:
//! ```text
//! varint round                      # dendrogram round used for this pyramid
//! varint num_superedges
//!   per edge: varint s_comm, predicate, o_comm, count
//! varint num_tiles
//!   per tile: varint community, varint block_len, block_bytes (SPO triple block)
//! ```

use crate::tiling::{SuperEdge, Tile};
use crate::varint::{read_uvarint, write_uvarint};

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("malformed pyramid meta: {0}")]
    Malformed(&'static str),
}

/// Decoded pyramid metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PyramidMeta {
    pub round: u32,
    pub summary: Vec<SuperEdge>,
    /// `(community, encoded SPO triple block)` per tile.
    pub tiles: Vec<(u32, Vec<u8>)>,
}

impl PyramidMeta {
    /// Build from a chosen round, the summary superedges, and the tiles.
    pub fn new(round: u32, summary: Vec<SuperEdge>, tiles: &[Tile]) -> Self {
        let tiles = tiles
            .iter()
            .map(|t| (t.community as u32, t.encoded.clone()))
            .collect();
        PyramidMeta {
            round,
            summary,
            tiles,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_uvarint(&mut out, self.round as u64);
        write_uvarint(&mut out, self.summary.len() as u64);
        for e in &self.summary {
            write_uvarint(&mut out, e.s_comm as u64);
            write_uvarint(&mut out, e.predicate as u64);
            write_uvarint(&mut out, e.o_comm as u64);
            write_uvarint(&mut out, e.count as u64);
        }
        write_uvarint(&mut out, self.tiles.len() as u64);
        for (community, block) in &self.tiles {
            write_uvarint(&mut out, *community as u64);
            write_uvarint(&mut out, block.len() as u64);
            out.extend_from_slice(block);
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaError> {
        let mut pos = 0;
        let g = |pos: &mut usize| -> Result<u64, MetaError> {
            let (v, n) = read_uvarint(&bytes[*pos..]).ok_or(MetaError::Malformed("truncated"))?;
            *pos += n;
            Ok(v)
        };
        let round = g(&mut pos)? as u32;
        // Counts are untrusted; each entry consumes several bytes, so cap the
        // pre-allocation at the buffer length to avoid an OOM on a bogus count.
        let n_edges = g(&mut pos)? as usize;
        let mut summary = Vec::with_capacity(n_edges.min(bytes.len()));
        for _ in 0..n_edges {
            summary.push(SuperEdge {
                s_comm: g(&mut pos)? as usize,
                predicate: g(&mut pos)? as u32,
                o_comm: g(&mut pos)? as usize,
                count: g(&mut pos)? as u32,
            });
        }
        let n_tiles = g(&mut pos)? as usize;
        let mut tiles = Vec::with_capacity(n_tiles.min(bytes.len()));
        for _ in 0..n_tiles {
            let community = g(&mut pos)? as u32;
            let len = g(&mut pos)? as usize;
            // checked_add: a corrupt `len` must not overflow-panic before the bound check.
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or(MetaError::Malformed("tile block overruns buffer"))?;
            tiles.push((community, bytes[pos..end].to_vec()));
            pos = end;
        }
        Ok(PyramidMeta {
            round,
            summary,
            tiles,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_round_trips() {
        let summary = vec![
            SuperEdge {
                s_comm: 0,
                predicate: 5,
                o_comm: 0,
                count: 4,
            },
            SuperEdge {
                s_comm: 0,
                predicate: 5,
                o_comm: 1,
                count: 1,
            },
        ];
        let tiles = vec![
            Tile {
                community: 0,
                triples: vec![],
                encoded: vec![1, 2, 3],
            },
            Tile {
                community: 1,
                triples: vec![],
                encoded: vec![9, 8],
            },
        ];
        let meta = PyramidMeta::new(2, summary, &tiles);
        let bytes = meta.encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(meta, back);
        assert_eq!(back.round, 2);
        assert_eq!(back.tiles[0], (0, vec![1, 2, 3]));
    }

    #[test]
    fn empty_meta() {
        let meta = PyramidMeta {
            round: 0,
            summary: vec![],
            tiles: vec![],
        };
        assert_eq!(PyramidMeta::decode(&meta.encode()).unwrap(), meta);
    }
}
