//! The logical parts of a file, independent of its generation: what the `0x06`
//! assembler ([`super::write`]) lays out. [`from_v05`] lifts them from a `0x05`
//! file by reading its framing and directories; payload blocks are referenced
//! by their byte range in the input and copied verbatim.

use super::{ChunkEnt, QTileEnt, TileEnt, TokenEnt};
use crate::file::{ByteRange, FileError};
use crate::header::Header;
use crate::reader::RangeReader;

/// A dictionary section: its metadata and chunks (with their input offsets).
#[derive(Debug, Clone, Default)]
pub struct DictPart {
    /// Terms in the section.
    pub term_count: u32,
    /// Restart interval.
    pub restart_interval: u32,
    /// Chunks, each with the absolute input offset of its stored bytes.
    pub chunks: Vec<(ChunkEnt, u64)>,
}

/// One permutation's tiles (with their input offsets).
#[derive(Debug, Clone, Default)]
pub struct PermPart {
    /// Section index of the permutation.
    pub si: usize,
    /// Tiles, each with the absolute input offset of its stored bytes.
    pub tiles: Vec<(TileEnt, u64)>,
}

/// The quad index.
#[derive(Debug, Clone, Default)]
pub struct QuadPart {
    /// Named graphs.
    pub graph_count: u64,
    /// Named-graph quads.
    pub quad_count: u64,
    /// Graphs per directory block.
    pub block_graphs: u64,
    /// The 0x05 `P0` block index (offsets relative to the blocks' start).
    pub p0: Vec<u8>,
    /// Input range of the graph-directory blocks.
    pub gdir: ByteRange,
    /// Per order: `(section index, tiles)`, `QTileEnt::off` = absolute input offset.
    pub orders: Vec<(usize, Vec<QTileEnt>)>,
}

/// The label index.
#[derive(Debug, Clone, Default)]
pub struct LabelPart {
    /// The 32-byte preamble (digest zeroed) and the root node.
    pub head: Vec<u8>,
    /// Bytes of data blocks at the start of the body.
    pub data_len: u64,
    /// Input range of the body (data blocks, then directory nodes).
    pub body: ByteRange,
}

/// The text index.
#[derive(Debug, Clone, Default)]
pub struct TextPart {
    /// The scope block (empty: default scope).
    pub scope: Vec<u8>,
    /// Tokens, posting offsets relative to the postings blob.
    pub tokens: Vec<TokenEnt>,
    /// Input range of the postings blob.
    pub postings: ByteRange,
    /// The index leaves some predicates out.
    pub partial: bool,
    /// The index covers the named graphs.
    pub named: bool,
}

/// Everything a `0x06` file is assembled from.
#[derive(Debug, Clone, Default)]
pub struct Parts {
    /// Statements.
    pub quad_count: u64,
    /// Terms.
    pub term_count: u64,
    /// Permutation mask (explicit bits).
    pub perm_mask: u8,
    /// Dictionary chunk codec.
    pub dict_codec: u8,
    /// Tile codec.
    pub block_codec: u8,
    /// Community-pyramid rounds.
    pub pyramid_levels: u16,
    /// The file holds named graphs.
    pub named: bool,
    /// Some terms are quoted triples.
    pub quoted: bool,
    /// The Dataset Card (raw JSON).
    pub card: Option<Vec<u8>>,
    /// The build record (raw JSON, possibly space-padded).
    pub build: Option<Vec<u8>>,
    /// Stats + schema: the 0x05 SCHEMA payload after its digest.
    pub schema: Option<Vec<u8>>,
    /// The four dictionary sections (shared, subject-only, object-only, predicates).
    pub dict: [DictPart; 4],
    /// The stored permutations, in section-index order.
    pub index: Vec<PermPart>,
    /// The quad index.
    pub quad: Option<QuadPart>,
    /// The label index.
    pub labels: Option<LabelPart>,
    /// The text index.
    pub text: Option<TextPart>,
    /// Input range of the community pyramid.
    pub pyramid: Option<ByteRange>,
}

fn read(r: &dyn RangeReader, range: ByteRange) -> Result<Vec<u8>, FileError> {
    if range.len == 0 {
        return Ok(Vec::new());
    }
    Ok(r.read_at(range.offset, range.len)?)
}

/// Decode a front-coded chunk body to its first and last terms.
fn first_last(body: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut pos = 0usize;
    let mut buf: Vec<u8> = Vec::new();
    let mut first = None;
    while pos < body.len() {
        let (shared, n1) = crate::varint::read_uvarint(body.get(pos..)?)?;
        let p = pos + n1;
        let (suf, n2) = crate::varint::read_uvarint(body.get(p..)?)?;
        let start = p + n2;
        let end = start.checked_add(suf as usize)?;
        let s = body.get(start..end)?;
        if shared as usize > buf.len() {
            return None;
        }
        buf.truncate(shared as usize);
        buf.extend_from_slice(s);
        if first.is_none() {
            first = Some(buf.clone());
        }
        pos = end;
    }
    Some((first?, buf))
}

/// Read and decompress the chunks of a dictionary section in batches, and
/// re-derive every routing key as the shortest separator (SPEC.md §6.2).
fn rekey(r: &dyn RangeReader, codec: u8, chunks: &mut [(ChunkEnt, u64)]) -> Result<(), FileError> {
    if chunks.len() < 2 {
        if let Some(c) = chunks.first_mut() {
            c.0.key.clear();
        }
        return Ok(());
    }
    const BATCH: u64 = 8 << 20;
    let mut prev_last: Option<Vec<u8>> = None;
    let mut i = 0usize;
    while i < chunks.len() {
        // A run of byte-adjacent chunks up to BATCH bytes, read at once.
        let start = chunks[i].1;
        let mut j = i;
        let mut end = start;
        while j < chunks.len() && chunks[j].1 == end && (end - start < BATCH || j == i) {
            end += chunks[j].0.clen;
            j += 1;
        }
        let span = r.read_at(start, end - start)?;
        for c in chunks.iter_mut().take(j).skip(i) {
            let lo = (c.1 - start) as usize;
            let stored = span
                .get(lo..lo + c.0.clen as usize)
                .ok_or(FileError::Container("dictionary chunk overruns"))?;
            let body = crate::file::decompress(codec, stored)?;
            let (first, last) =
                first_last(&body).ok_or(FileError::Container("malformed dictionary chunk"))?;
            c.0.key = match &prev_last {
                None => Vec::new(),
                Some(pl) => crate::dict::shortest_separator(pl, &first),
            };
            prev_last = Some(last);
        }
        i = j;
    }
    Ok(())
}

/// The [`LabelPart`] of a `0x05` LABEL_INDEX section at `offset..+length`
/// of `r`: its preamble and root kept, its body split into data blocks and
/// directory nodes.
pub(crate) fn label_part<R: RangeReader + ?Sized>(
    r: &R,
    offset: u64,
    length: u64,
    codec: u8,
) -> Result<LabelPart, FileError> {
    let head_len = crate::label_index::HEAD_READ.min(length);
    let head = r.read_at(offset, head_len)?;
    let pre = crate::label_index::LabelIndexPreamble::parse(&head)?;
    let root_end = 32u64 + pre.root_len as u64;
    let mut lhead = if root_end as usize <= head.len() {
        head[..root_end as usize].to_vec()
    } else {
        r.read_at(offset, root_end)?
    };
    lhead[..16].fill(0);
    let rr = |o: u64, l: u64| r.read_at(offset + o, l).ok();
    let li = crate::label_index::LabelIndex::from_head(
        &lhead,
        length,
        codec,
        Box::new(move |_o: u64, _l: u64| None),
    )?;
    let data_len = li
        .v05_data_len(&rr)
        .ok_or(FileError::Container("label index: unreadable directory"))?;
    Ok(LabelPart {
        head: lhead,
        data_len,
        body: ByteRange {
            offset: offset + root_end,
            len: length - root_end,
        },
    })
}

/// Lift a `0x05` file into [`Parts`]: header, framing and directories are
/// read; payload is referenced by range. Dictionary chunk keys are re-derived
/// (so a file written before separator keys gets them). A file with
/// per-graph named graphs (kind 5) is refused: the quad index is the only
/// named-graph layout of `0x06`, and getting there re-encodes the graphs.
pub fn from_v05(r: &dyn RangeReader) -> Result<Parts, FileError> {
    let head = r.read_at(0, crate::header::HEADER_LEN as u64)?;
    let h = Header::from_bytes(&head)?;
    if h.version != 0x05 {
        return Err(FileError::Container("not a 0x05 file"));
    }
    if h.named_graphs_len > 0 {
        return Err(FileError::Container(
            "per-graph named graphs (kind 5): convert re-encodes them as a quad index",
        ));
    }
    let mut p = Parts {
        quad_count: h.quad_count,
        term_count: h.term_count,
        perm_mask: h.perms.bits(),
        dict_codec: h.dict_codec,
        block_codec: h.block_codec,
        pyramid_levels: h.pyramid_levels,
        named: h.has_quads(),
        quoted: h.flags & crate::header::FLAG_HAS_QUOTED_TRIPLES != 0,
        ..Default::default()
    };
    if h.metadata_len > 0 {
        p.card = Some(read(
            r,
            ByteRange {
                offset: h.metadata_offset,
                len: h.metadata_len,
            },
        )?);
    }
    if h.build_info_len > 0 {
        p.build = Some(read(
            r,
            ByteRange {
                offset: h.build_info_offset,
                len: h.build_info_len,
            },
        )?);
    }
    if let Some(s) = h.schema_section() {
        let b = read(
            r,
            ByteRange {
                offset: s.offset,
                len: s.length,
            },
        )?;
        if b.len() > 16 {
            p.schema = Some(b[16..].to_vec());
        }
    }
    // Dictionary: four chunked sections in a container.
    for si in 0..4 {
        let sec = crate::file::locate_container_section_ranged(
            r,
            h.dictionary_offset,
            h.dictionary_len,
            si,
            4,
        )?;
        let (meta, entries) = crate::file::read_dict_dir_ranged(r, sec)?;
        let mut chunks: Vec<(ChunkEnt, u64)> = entries
            .into_iter()
            .map(|e| {
                (
                    ChunkEnt {
                        first_run: e.first_run,
                        key: e.key,
                        clen: e.end - e.start,
                    },
                    sec.offset + e.start,
                )
            })
            .collect();
        rekey(r, h.dict_codec, &mut chunks)?;
        p.dict[si] = DictPart {
            term_count: meta.term_count,
            restart_interval: meta.restart_interval,
            chunks,
        };
    }
    // Index: one tiled section per stored permutation (plus a quad seal).
    let sections = h.index_container_sections();
    for perm in h.perms.iter() {
        let pos = h.perms.position(perm).unwrap_or(0);
        let sec = crate::file::locate_container_section_ranged(
            r,
            h.root_dir_offset,
            h.root_dir_len,
            pos,
            sections as u64,
        )?;
        let dir = crate::file::read_tile_directory_ranged(r, sec)?;
        let syn = if h.has_tile_synopsis() {
            crate::file::read_tile_synopsis_ranged(r, sec, &dir)
        } else {
            vec![None; dir.len()]
        };
        let mut tiles = Vec::with_capacity(dir.len());
        for (e, s) in dir.iter().zip(syn) {
            let range = ByteRange {
                offset: sec.offset + e.start,
                len: e.end - e.start,
            };
            let syn = match s {
                Some(s) => s,
                None => {
                    // Files without a synopsis trailer: take it from the tile's
                    // own zone map.
                    let stored = r.read_at(range.offset, range.len)?;
                    let body = crate::file::decompress(h.block_codec, &stored)?;
                    let blk = crate::triples::TripleBlock::parse(&body)
                        .map_err(|_| FileError::Container("malformed tile"))?;
                    let z = blk.zone();
                    (z.min_b, z.max_b, z.min_c, z.max_c)
                }
            };
            tiles.push((
                TileEnt {
                    min_a: e.min_a,
                    max_a: e.max_a,
                    clen: range.len,
                    syn,
                },
                range.offset,
            ));
        }
        p.index.push(PermPart {
            si: perm.section_index(),
            tiles,
        });
    }
    // Quad index.
    if let Some(q) = h.quad_index_section() {
        p.quad = Some(crate::quad::v05_parts(
            r,
            ByteRange {
                offset: q.offset,
                len: q.length,
            },
            h.perms,
        )?);
    }
    // Label index.
    if let Some(s) = h.label_index_section() {
        p.labels = Some(label_part(r, s.offset, s.length, h.block_codec)?);
    }
    // Text index.
    let (toff, tlen) = h.text_index_payload();
    if tlen > 0 {
        let probe = r.read_at(toff, 10.min(tlen))?;
        let (ttl, n) = crate::varint::read_uvarint(&probe)
            .ok_or(FileError::Container("truncated text-index len"))?;
        let prefix_len = (n as u64 + ttl).min(tlen);
        let prefix = r.read_at(toff, prefix_len)?;
        let (tokens, scope, scope_raw, pstart) =
            crate::text_index::parse_token_table_raw(&prefix, h.block_codec)?;
        p.text = Some(TextPart {
            scope: scope_raw,
            tokens: tokens
                .into_iter()
                .map(|(t, off, len)| TokenEnt {
                    token: t.into_bytes(),
                    off,
                    len,
                })
                .collect(),
            postings: ByteRange {
                offset: toff + pstart as u64,
                len: tlen - pstart as u64,
            },
            partial: scope.restricts_predicates(),
            named: scope.covers_named_graphs(),
        });
    }
    if h.pyramid_meta_len > 0 {
        p.pyramid = Some(ByteRange {
            offset: h.pyramid_meta_offset,
            len: h.pyramid_meta_len,
        });
    }
    Ok(p)
}
