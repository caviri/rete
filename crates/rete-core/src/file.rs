//! `.rete` file assembly and reading (SPEC.md Â§4, Â§9).
//!
//! v0 layout:
//!
//! ```text
//! [0..128)   header
//! [dict]     dictionary container: 4 front-coded sections
//! [index]    permutation container: 3 triple blocks (SPO, POS, OSP)
//! [pyramid]  summary meta (and, in future, tile directories)
//! [footer]   trailing magic
//! ```
//!
//! The header points at the dictionary container (`dictionary_offset/len`) and
//! the permutation container (`root_dir_offset/len`); routed readers can fetch a
//! single permutation payload from that container.

use crate::dictionary::Dictionary;
use crate::header::{Header, FLAG_HAS_QUADS, HEADER_LEN, MAGIC};
use crate::index::{GraphIndex, IndexPermutation, Pattern};
use crate::meta::{ClassNode, CommunityDescriptor, LevelLinks, LevelRollup, PyramidMeta};
use crate::pyramid::{build_dendrogram, project_graph};
use crate::reader::RangeReader;
use crate::tiling::{choose_round_for_budget, summarize, SuperEdge};
use crate::triples::Triple;
use crate::varint::{read_uvarint, write_uvarint};

/// Default per-tile byte budget `T` (SPEC.md Â§7.1).
pub const DEFAULT_TILE_BUDGET: usize = 64 * 1024;

/// Build the encoded pyramid-meta section for a graph: cluster, pick a round
/// sized to `budget`, then emit the **summary** (quotient) graph. Returns
/// `(encoded_meta, pyramid_levels)`.
///
/// Per-community tiles are *not* stored: they would duplicate every triple, and
/// the exact ranged single-pattern path now routes into one permutation section
/// without that fourth copy. Physical community-tile directories are the next
/// storage step (SPEC Â§7.2).
pub fn build_pyramid_meta(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    budget: usize,
) -> (Vec<u8>, u16) {
    build_pyramid_meta_with(dict, triples, budget, None)
}

/// Like [`build_pyramid_meta`], but `type_override` forces the schema-pyramid's
/// type predicate (e.g. `wdt:P31`) instead of auto-detection.
pub fn build_pyramid_meta_with(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    budget: usize,
    type_override: Option<&str>,
) -> (Vec<u8>, u16) {
    let g = project_graph(dict, triples);
    let dend = build_dendrogram(&g);
    let round = choose_round_for_budget(dict, triples, &dend, budget);
    let summary = summarize(dict, triples, &dend, round);
    // Attach the v2 schema pyramid (the non-exclusive subClassOf DAG + per-level
    // type rollups + per-level lateral class relations + per-community
    // descriptors). Empty when the graph has no usable typing, in which case the
    // encoding stays byte-identical to a v1 pyramid-meta.
    let sp = crate::schema_pyramid::build_schema_pyramid_with(
        dict,
        triples,
        &dend,
        round,
        type_override,
    );
    let meta = PyramidMeta::new(round as u32, summary, &[]).with_schema(
        sp.class_hierarchy,
        sp.level_rollups,
        sp.level_links,
        sp.descriptors,
        sp.subclass_cycles,
        sp.disjoint_pairs,
        sp.equivalent_pairs,
    );
    (meta.encode(), dend.rounds() as u16)
}

/// No compression.
pub const CODEC_NONE: u8 = 0;
/// zstd compression (per section).
pub const CODEC_ZSTD: u8 = 1;
/// zstd compression level used by the writer.
#[cfg(feature = "compression")]
const ZSTD_LEVEL: i32 = 9;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("header: {0}")]
    Header(#[from] crate::header::HeaderError),
    #[error("malformed container: {0}")]
    Container(&'static str),
    #[error("unknown codec: {0}")]
    UnknownCodec(u8),
    #[error("decompression failed: {0}")]
    Decompress(std::io::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// The codec the writer uses: zstd when the `compression` feature is on, else
/// none. Reading honors whatever codec the header records (when supported).
fn writer_codec() -> u8 {
    if cfg!(feature = "compression") {
        CODEC_ZSTD
    } else {
        CODEC_NONE
    }
}

fn compress(codec: u8, bytes: &[u8]) -> Vec<u8> {
    match codec {
        #[cfg(feature = "compression")]
        CODEC_ZSTD => {
            zstd::encode_all(bytes, ZSTD_LEVEL).expect("zstd encode is infallible in-memory")
        }
        _ => bytes.to_vec(),
    }
}

fn decompress(codec: u8, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
    match codec {
        CODEC_NONE => Ok(bytes.to_vec()),
        // Pure-Rust decode so any target (including wasm) can read compressed
        // files, regardless of whether the C encoder was compiled in.
        CODEC_ZSTD => {
            use std::io::Read;
            let mut dec = ruzstd::StreamingDecoder::new(bytes)
                .map_err(|e| FileError::Decompress(std::io::Error::other(e.to_string())))?;
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map_err(FileError::Decompress)?;
            Ok(out)
        }
        other => Err(FileError::UnknownCodec(other)),
    }
}

/// Bytes this close are cheaper fetched as one read than as two round trips:
/// tiles are laid back-to-back so this only ever bridges a tile already made
/// resident by an earlier window — keep it tight to avoid re-fetching it.
const TILE_COALESCE_GAP: u64 = 4096;

/// The dictionary chunks a query's output terms touch are scattered across the
/// section (terms are sorted, output ids are not), so byte-adjacency is rare.
/// A wider gap trades a little over-fetch for far fewer round trips — the right
/// call on a latency-bound remote read, where one skipped 64 KiB chunk is much
/// cheaper than another request's RTT.
const DICT_COALESCE_GAP: u64 = 64 * 1024;

/// Fetch a set of ascending, disjoint byte ranges, coalescing ranges whose gap
/// is at most `gap` into one span, then fetching the spans through
/// [`RangeReader::read_many`] (which a parallelizable reader issues
/// concurrently). Returns each requested range's bytes in order; `None` if any
/// read fails.
fn read_coalesced<R: RangeReader + ?Sized>(
    reader: &R,
    ranges: &[ByteRange],
    gap: u64,
) -> Option<Vec<Vec<u8>>> {
    // Build the coalesced spans and remember which span each input range maps
    // into, so the fetched span blobs can be sliced back apart in order.
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let mut span_of: Vec<usize> = Vec::with_capacity(ranges.len());
    let mut i = 0;
    while i < ranges.len() {
        let start = ranges[i].offset;
        let mut end = ranges[i].offset.checked_add(ranges[i].len)?;
        let mut j = i + 1;
        while j < ranges.len() {
            let r = &ranges[j];
            if r.offset < end || r.offset - end > gap {
                break;
            }
            end = r.offset.checked_add(r.len)?;
            j += 1;
        }
        let si = spans.len();
        spans.push((start, end - start));
        for _ in i..j {
            span_of.push(si);
        }
        i = j;
    }
    let blobs = reader.read_many(&spans).ok()?;
    if blobs.len() != spans.len() {
        return None;
    }
    let mut out = Vec::with_capacity(ranges.len());
    for (k, r) in ranges.iter().enumerate() {
        let (span_start, _) = spans[span_of[k]];
        let blob = &blobs[span_of[k]];
        let lo = (r.offset - span_start) as usize;
        let hi = lo.checked_add(r.len as usize)?;
        out.push(blob.get(lo..hi)?.to_vec());
    }
    Some(out)
}

/// Content hash (first 16 bytes of blake3) over the file payload sections.
/// Identifies the immutable content independent of the header.
fn content_hash(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p);
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

/// A resolved triple as terms.
pub type TermTriple = (String, String, String);

/// One labelled byte region of a `.rete` file image (see
/// [`Rete::file_layout`]). `kind` is a stable machine tag: `header`,
/// `metadata`, `dictionary`, `directory`, `tile`, `pyramid`, `named-graphs`.
#[derive(Debug, Clone)]
pub struct LayoutSegment {
    pub kind: &'static str,
    pub label: String,
    pub offset: u64,
    pub len: u64,
}

/// A byte range in the `.rete` file image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u64,
}

impl ByteRange {
    pub fn end(self) -> u64 {
        self.offset + self.len
    }
}

/// Why a triple-pattern result is present in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripleProvenance {
    /// Matched triple resolved to canonical N-Triples tokens.
    pub terms: TermTriple,
    /// Matched triple in dictionary ID space.
    pub ids: Triple,
    /// Named graph IRI, or `None` for the default graph.
    pub graph: Option<String>,
    /// The resolved ID-space pattern that was matched.
    pub matched_pattern: Pattern,
    /// Permutation selected to answer the pattern.
    pub index_permutation: IndexPermutation,
    /// File byte range containing the dictionary container.
    pub dictionary_range: ByteRange,
    /// File byte range containing the permutation index container.
    pub index_range: ByteRange,
    /// File byte range containing the selected permutation payload inside the
    /// index container.
    pub index_section_range: ByteRange,
    /// File byte range containing the pyramid metadata, when present.
    pub pyramid_range: Option<ByteRange>,
    /// Physical tile identifier, once tile directories are materialized.
    /// Physical tile identifier (`PERM/index`, e.g. `POS/3`) for tiled (v0.2)
    /// files; `None` for pre-tiling files.
    pub tile: Option<String>,
    /// File byte range of that (compressed) tile â€” the exact bytes a ranged
    /// client would fetch to re-derive this match.
    pub tile_range: Option<ByteRange>,
}

/// Encode a length-prefixed container of byte sections, each compressed with
/// `codec` independently (so a range-reading client decompresses only the
/// sections it fetches). Stored length is the *compressed* length.
fn encode_container(sections: &[&[u8]], codec: u8) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, sections.len() as u64);
    for s in sections {
        let payload = compress(codec, s);
        write_uvarint(&mut out, payload.len() as u64);
        out.extend_from_slice(&payload);
    }
    out
}

/// Decode a container into owned, decompressed sections.
fn decode_container(bytes: &[u8], codec: u8) -> Result<Vec<Vec<u8>>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated count"))?;
    // `n` is untrusted; each section needs â‰¥1 byte, so cap the pre-allocation at
    // the buffer length rather than trusting the count (avoids an OOM on a bogus
    // header pointing at a small region).
    let mut out = Vec::with_capacity((n as usize).min(bytes.len()));
    for _ in 0..n {
        let (len, used) =
            read_uvarint(&bytes[pos..]).ok_or(FileError::Container("truncated length"))?;
        pos += used;
        let end = pos + len as usize;
        if end > bytes.len() {
            return Err(FileError::Container("section overruns buffer"));
        }
        out.push(decompress(codec, &bytes[pos..end])?);
        pos = end;
    }
    Ok(out)
}

fn checked_end(off: u64, len: u64) -> Result<u64, FileError> {
    off.checked_add(len)
        .ok_or(FileError::Container("section range overflows"))
}

/// Per-chunk budget for dictionary section bodies â€” same reasoning as
/// [`crate::index::INDEX_TILE_BUDGET`]: one fetch, one decompress per touch.
const DICT_CHUNK_BUDGET: usize = 64 * 1024;

/// Encode one dictionary section as a chunked payload (format v0.2):
/// `[header_len, raw header (term_count/interval/restart table)]
///  [num_chunks; per chunk: Î”first_run, first_term, comp_len]
///  [individually compressed run-aligned body slices]`.
/// The header keeps its original encoding, so restart offsets stay valid in
/// the section's coordinate space.
fn encode_chunked_dict_section(raw: &[u8], codec: u8) -> Vec<u8> {
    let meta = crate::dict::parse_meta(raw).unwrap_or(crate::dict::SectionMeta {
        term_count: 0,
        restart_interval: 1,
        restart_offsets: Vec::new(),
    });
    let body_start = meta.restart_offsets.first().copied().unwrap_or(raw.len());
    let header = &raw[..body_start.min(raw.len())];

    // Split runs into chunks by body-byte budget (whole runs only).
    let n_runs = meta.restart_offsets.len();
    let mut bounds: Vec<(usize, usize, usize)> = Vec::new(); // (first_run, start, end)
    let mut r = 0;
    while r < n_runs {
        let start = meta.restart_offsets[r];
        let mut r2 = r + 1;
        while r2 < n_runs && meta.restart_offsets[r2] - start < DICT_CHUNK_BUDGET {
            r2 += 1;
        }
        let end = if r2 < n_runs {
            meta.restart_offsets[r2]
        } else {
            raw.len()
        };
        bounds.push((r, start, end));
        r = r2;
    }

    let compressed: Vec<Vec<u8>> = bounds
        .iter()
        .map(|&(_, s, e)| compress(codec, &raw[s..e]))
        .collect();
    let mut out = Vec::new();
    write_uvarint(&mut out, header.len() as u64);
    out.extend_from_slice(header);
    write_uvarint(&mut out, bounds.len() as u64);
    let mut prev_run = 0usize;
    for (&(first_run, start, _), comp) in bounds.iter().zip(&compressed) {
        let first_term = crate::dict::run_first_term(raw, start).unwrap_or_default();
        write_uvarint(&mut out, (first_run - prev_run) as u64);
        write_uvarint(&mut out, first_term.len() as u64);
        out.extend_from_slice(&first_term);
        write_uvarint(&mut out, comp.len() as u64);
        prev_run = first_run;
    }
    for comp in &compressed {
        out.extend_from_slice(comp);
    }
    out
}

/// A parsed chunked-dict-section directory entry: the chunk's run/term/body
/// coordinates plus its compressed byte range *within the payload*.
struct DictChunkEntry {
    first_run: usize,
    first_term: Vec<u8>,
    body_start: usize,
    start: usize,
    end: usize,
}

/// Parse a chunked dictionary section's header + directory (not the chunks).
/// `bytes` may be a prefix of the payload; compressed ranges validate against
/// `total_len`.
fn parse_chunked_dict_dir(
    bytes: &[u8],
    total_len: usize,
) -> Result<(crate::dict::SectionMeta, Vec<DictChunkEntry>), FileError> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Result<u64, FileError> {
        let (v, n) = read_uvarint(bytes.get(*pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated dict chunk directory"))?;
        *pos += n;
        Ok(v)
    };
    let header_len = take(&mut pos)? as usize;
    let header = bytes
        .get(pos..pos.saturating_add(header_len))
        .ok_or(FileError::Container("truncated dict header"))?;
    let meta = crate::dict::parse_meta(header)
        .map_err(|_| FileError::Container("malformed dict header"))?;
    pos += header_len;

    let num_chunks = take(&mut pos)? as usize;
    let mut entries = Vec::with_capacity(num_chunks.min(bytes.len()));
    let mut lens = Vec::with_capacity(num_chunks.min(bytes.len()));
    let mut prev_run = 0usize;
    for _ in 0..num_chunks {
        let drun = take(&mut pos)? as usize;
        let tlen = take(&mut pos)? as usize;
        let term = bytes
            .get(pos..pos.saturating_add(tlen))
            .ok_or(FileError::Container("truncated dict chunk first term"))?
            .to_vec();
        pos += tlen;
        let clen = take(&mut pos)? as usize;
        let first_run = prev_run + drun;
        let body_start = meta
            .restart_offsets
            .get(first_run)
            .copied()
            .ok_or(FileError::Container("dict chunk run out of range"))?;
        entries.push(DictChunkEntry {
            first_run,
            first_term: term,
            body_start,
            start: 0,
            end: 0,
        });
        lens.push(clen);
        prev_run = first_run;
    }
    let mut start = pos;
    for (e, len) in entries.iter_mut().zip(lens) {
        let end = start
            .checked_add(len)
            .filter(|&e| e <= total_len)
            .ok_or(FileError::Container("dict chunk overruns section"))?;
        e.start = start;
        e.end = end;
        start = end;
    }
    Ok((meta, entries))
}

/// Fetch and parse a remote chunked dict section's header + directory: read a
/// small prefix and grow it geometrically until it parses, never fetching past
/// the section.
fn read_dict_dir_ranged<R: RangeReader>(
    reader: &R,
    section: ByteRange,
) -> Result<(crate::dict::SectionMeta, Vec<DictChunkEntry>), FileError> {
    let total = section.len as usize;
    let mut prefetch = 4096.min(total);
    loop {
        let prefix = reader.read_at(section.offset, prefetch as u64)?;
        match parse_chunked_dict_dir(&prefix, total) {
            Ok(parsed) => return Ok(parsed),
            Err(_) if prefetch < total => prefetch = prefetch.saturating_mul(2).min(total),
            Err(e) => return Err(e),
        }
    }
}

/// Decode one chunked dictionary section payload into a resident
/// [`crate::dict::ChunkedSection`] (chunks decompressed up front â€” the local
/// open path).
fn decode_chunked_dict_section(
    payload: &[u8],
    codec: u8,
) -> Result<crate::dict::ChunkedSection, FileError> {
    let (meta, entries) = parse_chunked_dict_dir(payload, payload.len())?;
    let chunks = entries
        .into_iter()
        .map(|e| {
            Ok(crate::dict::SectionChunk::resident(
                e.first_run,
                e.first_term,
                e.body_start,
                decompress(codec, &payload[e.start..e.end])?,
            ))
        })
        .collect::<Result<Vec<_>, FileError>>()?;
    Ok(crate::dict::ChunkedSection::from_parts(meta, chunks, None))
}

fn decode_dictionary_container(
    bytes: &[u8],
    codec: u8,
    version: u8,
) -> Result<Dictionary, FileError> {
    if version >= 2 {
        let dsecs = decode_container(bytes, CODEC_NONE)?;
        if dsecs.len() != 4 {
            return Err(FileError::Container("expected 4 dictionary sections"));
        }
        let mut sections = Vec::with_capacity(4);
        for sec in &dsecs {
            sections.push(decode_chunked_dict_section(sec, codec)?);
        }
        let arr: [crate::dict::ChunkedSection; 4] = sections
            .try_into()
            .map_err(|_| FileError::Container("expected 4 dictionary sections"))?;
        return Ok(Dictionary::from_chunked_sections(arr));
    }
    let mut dsecs = decode_container(bytes, codec)?;
    if dsecs.len() != 4 {
        return Err(FileError::Container("expected 4 dictionary sections"));
    }
    Ok(Dictionary::from_sections([
        std::mem::take(&mut dsecs[0]),
        std::mem::take(&mut dsecs[1]),
        std::mem::take(&mut dsecs[2]),
        std::mem::take(&mut dsecs[3]),
    ]))
}

/// Encode one permutation's tiled section payload (format v0.2):
/// `[num_tiles][per tile: delta(min_a), max_a - min_a, compressed_len][tilesâ€¦]`,
/// each tile compressed independently with `codec` so a ranged reader can
/// fetch and decompress exactly the tiles a query routes to. The directory
/// itself is uncompressed (it must be readable before any tile).
fn encode_tiled_section(tiles: &[crate::index::Tile], codec: u8) -> Vec<u8> {
    let compressed: Vec<Vec<u8>> = tiles.iter().map(|t| compress(codec, t.bytes())).collect();
    let mut out = Vec::new();
    write_uvarint(&mut out, tiles.len() as u64);
    let mut prev_min = 0u32;
    for (tile, comp) in tiles.iter().zip(&compressed) {
        let (min_a, max_a) = tile.leading_range();
        write_uvarint(&mut out, (min_a - prev_min) as u64);
        write_uvarint(&mut out, (max_a - min_a) as u64);
        write_uvarint(&mut out, comp.len() as u64);
        prev_min = min_a;
    }
    for comp in &compressed {
        out.extend_from_slice(comp);
    }
    out
}

/// A parsed v0.2 tile-directory entry: leading-id range plus the tile's byte
/// range *within the section payload*.
struct TileDirEntry {
    min_a: u32,
    max_a: u32,
    start: usize,
    end: usize,
}

/// Parse a tiled section payload's directory (not the tiles). `bytes` may be a
/// **prefix** of the payload (a ranged reader fetches the directory before any
/// tile); tile byte ranges are validated against `total_len`, the full payload
/// length. Every length is untrusted.
fn parse_tile_directory(bytes: &[u8], total_len: usize) -> Result<Vec<TileDirEntry>, FileError> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Result<u64, FileError> {
        let (v, n) = read_uvarint(bytes.get(*pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated tile directory"))?;
        *pos += n;
        Ok(v)
    };
    let num_tiles = take(&mut pos)? as usize;
    let mut entries = Vec::with_capacity(num_tiles.min(bytes.len()));
    let mut prev_min = 0u32;
    let mut lens = Vec::with_capacity(num_tiles.min(bytes.len()));
    for _ in 0..num_tiles {
        let dmin = take(&mut pos)? as u32;
        let span = take(&mut pos)? as u32;
        let len = take(&mut pos)? as usize;
        let min_a = prev_min.wrapping_add(dmin);
        entries.push(TileDirEntry {
            min_a,
            max_a: min_a.wrapping_add(span),
            start: 0,
            end: 0,
        });
        lens.push(len);
        prev_min = min_a;
    }
    let mut start = pos;
    for (e, len) in entries.iter_mut().zip(lens) {
        let end = start
            .checked_add(len)
            .filter(|&e| e <= total_len)
            .ok_or(FileError::Container("tile overruns section"))?;
        e.start = start;
        e.end = end;
        start = end;
    }
    Ok(entries)
}

/// Fetch and parse a remote tiled section's directory: read a small prefix and
/// grow it geometrically until the directory parses, never fetching past the
/// section. A directory that still fails on the whole section is corrupt.
fn read_tile_directory_ranged<R: RangeReader>(
    reader: &R,
    section: ByteRange,
) -> Result<Vec<TileDirEntry>, FileError> {
    let total = section.len as usize;
    let mut prefetch = 4096.min(total);
    loop {
        let prefix = reader.read_at(section.offset, prefetch as u64)?;
        match parse_tile_directory(&prefix, total) {
            Ok(dir) => return Ok(dir),
            Err(_) if prefetch < total => prefetch = prefetch.saturating_mul(2).min(total),
            Err(e) => return Err(e),
        }
    }
}

/// Per-tile absolute file ranges of each permutation section, for provenance.
/// Empty for pre-tiling (v0.1) files; a malformed directory yields an empty
/// section (provenance degrades, queries are unaffected).
fn tile_file_ranges(
    index_bytes: &[u8],
    container_offset: u64,
    section_ranges: &[ByteRange; 3],
    version: u8,
) -> [Vec<(u32, u32, ByteRange)>; 3] {
    let mut out: [Vec<(u32, u32, ByteRange)>; 3] = Default::default();
    if version < 2 {
        return out;
    }
    for (section, range) in out.iter_mut().zip(section_ranges) {
        let start = (range.offset - container_offset) as usize;
        let Some(payload) = index_bytes.get(start..start + range.len as usize) else {
            continue;
        };
        if let Ok(dir) = parse_tile_directory(payload, payload.len()) {
            *section = dir
                .into_iter()
                .map(|e| {
                    (
                        e.min_a,
                        e.max_a,
                        ByteRange {
                            offset: range.offset + e.start as u64,
                            len: (e.end - e.start) as u64,
                        },
                    )
                })
                .collect();
        }
    }
    out
}

/// Decode a tiled section payload into `(min_a, max_a, uncompressed tile)`
/// triples.
fn decode_tiled_section(payload: &[u8], codec: u8) -> Result<Vec<(u32, u32, Vec<u8>)>, FileError> {
    parse_tile_directory(payload, payload.len())?
        .into_iter()
        .map(|e| {
            Ok((
                e.min_a,
                e.max_a,
                decompress(codec, &payload[e.start..e.end])?,
            ))
        })
        .collect()
}

/// Decode the index container, gated on the file format version: v0.1 stores
/// one whole-section-compressed block per permutation; v0.2 stores raw tiled
/// payloads whose tiles are compressed individually.
fn decode_index_container(bytes: &[u8], codec: u8, version: u8) -> Result<GraphIndex, FileError> {
    if version >= 2 {
        let mut isecs = decode_container(bytes, CODEC_NONE)?;
        if isecs.len() != 3 {
            return Err(FileError::Container("expected 3 permutation sections"));
        }
        let mut sections: [Vec<(u32, u32, Vec<u8>)>; 3] = Default::default();
        for (i, sec) in isecs.iter_mut().enumerate() {
            sections[i] = decode_tiled_section(sec, codec)?;
        }
        return Ok(GraphIndex::from_tiles(sections));
    }
    let mut isecs = decode_container(bytes, codec)?;
    if isecs.len() != 3 {
        return Err(FileError::Container("expected 3 permutation blocks"));
    }
    Ok(GraphIndex::from_blocks([
        std::mem::take(&mut isecs[0]),
        std::mem::take(&mut isecs[1]),
        std::mem::take(&mut isecs[2]),
    ]))
}

fn container_section_payload_ranges(
    bytes: &[u8],
    container_offset: u64,
    expected_sections: usize,
) -> Result<Vec<ByteRange>, FileError> {
    let (section_count, mut pos) =
        read_uvarint(bytes).ok_or(FileError::Container("truncated count"))?;
    let section_count = usize::try_from(section_count)
        .map_err(|_| FileError::Container("section count too large"))?;
    if section_count != expected_sections {
        return Err(FileError::Container("unexpected section count"));
    }

    let mut ranges = Vec::with_capacity(section_count);
    for _ in 0..section_count {
        let remaining = bytes
            .get(pos..)
            .ok_or(FileError::Container("truncated length"))?;
        let (payload_len, used) =
            read_uvarint(remaining).ok_or(FileError::Container("truncated length"))?;
        pos = pos
            .checked_add(used)
            .ok_or(FileError::Container("section range overflows"))?;
        let payload_len_usize = usize::try_from(payload_len)
            .map_err(|_| FileError::Container("section length too large"))?;
        let payload_end = pos
            .checked_add(payload_len_usize)
            .ok_or(FileError::Container("section range overflows"))?;
        if payload_end > bytes.len() {
            return Err(FileError::Container("section overruns buffer"));
        }
        ranges.push(ByteRange {
            offset: checked_end(container_offset, pos as u64)?,
            len: payload_len,
        });
        pos = payload_end;
    }

    Ok(ranges)
}

fn decode_index_section_ranges(
    bytes: &[u8],
    container_offset: u64,
) -> Result<[ByteRange; 3], FileError> {
    let ranges = container_section_payload_ranges(bytes, container_offset, 3)?;
    ranges
        .try_into()
        .map_err(|_| FileError::Container("expected 3 permutation blocks"))
}

fn read_uvarint_at<R: RangeReader>(
    reader: &R,
    absolute_offset: u64,
    container_end: u64,
) -> Result<(u64, u64), FileError> {
    if absolute_offset >= container_end {
        return Err(FileError::Container("truncated container varint"));
    }
    let remaining = container_end - absolute_offset;
    let probe_len = remaining.min(10);
    let bytes = reader.read_at(absolute_offset, probe_len)?;
    read_uvarint(&bytes)
        .map(|(value, used)| (value, used as u64))
        .ok_or(FileError::Container("truncated container varint"))
}

/// Locate one section's payload byte range inside a remote container, walking
/// only the (tiny) varint framing â€” no payload bytes are fetched.
fn locate_container_section_ranged<R: RangeReader>(
    reader: &R,
    container_offset: u64,
    container_len: u64,
    section_index: usize,
    expected_sections: u64,
) -> Result<ByteRange, FileError> {
    let container_end = checked_end(container_offset, container_len)?;
    let (section_count, used) = read_uvarint_at(reader, container_offset, container_end)?;
    if section_count != expected_sections {
        return Err(FileError::Container("unexpected container section count"));
    }
    if section_index >= section_count as usize {
        return Err(FileError::Container(
            "container section index out of bounds",
        ));
    }

    let mut pos = checked_end(container_offset, used)?;
    for i in 0..section_count as usize {
        let (payload_len, len_used) = read_uvarint_at(reader, pos, container_end)?;
        pos = checked_end(pos, len_used)?;
        let payload_end = checked_end(pos, payload_len)?;
        if payload_end > container_end {
            return Err(FileError::Container("section overruns buffer"));
        }
        if i == section_index {
            return Ok(ByteRange {
                offset: pos,
                len: payload_len,
            });
        }
        pos = payload_end;
    }
    Err(FileError::Container("container section not found"))
}

/// Serialize a complete `.rete` file image from a dictionary, index, and an
/// (optionally empty) encoded pyramid-meta section. `pyramid_levels` records the
/// number of dendrogram rounds the pyramid spans (0 if no pyramid).
pub fn write_file(
    dict: &Dictionary,
    index: &GraphIndex,
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
) -> Vec<u8> {
    write_dataset(dict, index, &[], has_quads, pyramid_meta, pyramid_levels)
}

/// Encode an index container (v0.2): three raw tiled section payloads, tiles
/// compressed individually with `codec`.
fn encode_index_container(index: &GraphIndex, codec: u8) -> Vec<u8> {
    let payloads = index
        .tile_sections()
        .map(|tiles| encode_tiled_section(tiles, codec));
    encode_container(&[&payloads[0], &payloads[1], &payloads[2]], CODEC_NONE)
}

/// Encode the named-graphs section: each graph as `(iri, permutation container)`.
fn encode_named_graphs(named: &[(String, GraphIndex)], codec: u8) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, named.len() as u64);
    for (iri, index) in named {
        write_uvarint(&mut out, iri.len() as u64);
        out.extend_from_slice(iri.as_bytes());
        let container = encode_index_container(index, codec);
        write_uvarint(&mut out, container.len() as u64);
        out.extend_from_slice(&container);
    }
    out
}

fn decode_named_graphs(
    bytes: &[u8],
    codec: u8,
    version: u8,
) -> Result<Vec<(String, GraphIndex)>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated graph count"))?;
    // Bounds-checked slice within this (already bounded) section. Lengths read
    // below are untrusted, so every range is validated before indexing.
    let bound = |start: usize, len: u64| -> Result<usize, FileError> {
        start
            .checked_add(len as usize)
            .filter(|&e| e <= bytes.len())
            .ok_or(FileError::Container("named-graph field overruns buffer"))
    };
    let mut out = Vec::with_capacity((n as usize).min(bytes.len()));
    for _ in 0..n {
        let (ilen, u1) = read_uvarint(bytes.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated iri len"))?;
        pos += u1;
        let iend = bound(pos, ilen)?;
        let iri = String::from_utf8_lossy(&bytes[pos..iend]).into_owned();
        pos = iend;
        let (clen, u2) = read_uvarint(bytes.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated container len"))?;
        pos += u2;
        let cend = bound(pos, clen)?;
        let index = decode_index_container(&bytes[pos..cend], codec, version)?;
        out.push((iri, index));
        pos = cend;
    }
    Ok(out)
}

/// Serialize a full RDF *dataset*: the default-graph index plus zero or more
/// named graphs `(iri, index)`, all sharing one dictionary.
pub fn write_dataset(
    dict: &Dictionary,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
) -> Vec<u8> {
    write_dataset_with_metadata(
        dict,
        default_index,
        named,
        has_quads,
        pyramid_meta,
        pyramid_levels,
        &[],
    )
}

/// Serialize a dataset with an opaque **metadata** payload occupying the file's
/// metadata section (the application layer defines its meaning â€” the CLI stores a
/// JSON Dataset Card there). The section sits immediately after the 128-byte
/// header and before the dictionary, so `metadata_offset` stays at `HEADER_LEN`
/// and every downstream section shifts by `metadata.len()`. The payload is folded
/// into the `content_hash`, so `verify` covers it and it is tamper-evident.
///
/// Passing an empty `metadata` is byte-identical to [`write_dataset`]: the section
/// is omitted (`metadata_len = 0`, `dictionary_offset = HEADER_LEN`) and the hash
/// is computed over exactly the same parts (a zero-length hash update is a no-op).
pub fn write_dataset_with_metadata(
    dict: &Dictionary,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
    metadata: &[u8],
) -> Vec<u8> {
    let codec = writer_codec();
    let raw_sections = dict.sections();
    let dict_payloads: Vec<Vec<u8>> = raw_sections
        .iter()
        .map(|raw| encode_chunked_dict_section(raw, codec))
        .collect();
    let dict_container = encode_container(
        &[
            dict_payloads[0].as_slice(),
            dict_payloads[1].as_slice(),
            dict_payloads[2].as_slice(),
            dict_payloads[3].as_slice(),
        ],
        CODEC_NONE,
    );
    let index_container = encode_index_container(default_index, codec);
    let named_section = encode_named_graphs(named, codec);

    // The metadata section (if any) sits between the header and the dictionary,
    // so the dictionary â€” and everything after it â€” shifts forward by its length.
    let meta_section_len = metadata.len() as u64;
    let dict_offset = HEADER_LEN as u64 + meta_section_len;
    let dict_len = dict_container.len() as u64;
    let index_offset = dict_offset + dict_len;
    let index_len = index_container.len() as u64;
    let pyr_offset = index_offset + index_len;
    let pyr_len = pyramid_meta.len() as u64;
    let named_offset = pyr_offset + pyr_len;
    let named_len = if named.is_empty() {
        0
    } else {
        named_section.len() as u64
    };

    // Hash parts in physical order, with the metadata payload prepended when
    // present. Omitting it entirely (rather than hashing an empty slice) keeps the
    // no-metadata output's hash byte-identical to the pre-metadata writer.
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    if meta_section_len > 0 {
        parts.push(metadata);
    }
    parts.push(&dict_container);
    parts.push(&index_container);
    parts.push(pyramid_meta);
    if named_len > 0 {
        parts.push(&named_section);
    }

    // Length of the trailing schema-pyramid block (0 if none), so a reader can
    // fetch just that block for an index/dictionary/summary-free Tier-0 read.
    let schema_meta_len = crate::meta::schema_block_len(pyramid_meta);

    let header = Header {
        version: crate::header::VERSION,
        flags: if has_quads { FLAG_HAS_QUADS } else { 0 },
        metadata_offset: HEADER_LEN as u64,
        metadata_len: meta_section_len,
        dictionary_offset: dict_offset,
        dictionary_len: dict_len,
        root_dir_offset: index_offset,
        root_dir_len: index_len,
        pyramid_meta_offset: if pyr_len > 0 { pyr_offset } else { 0 },
        pyramid_meta_len: pyr_len,
        dict_codec: codec,
        block_codec: codec,
        pyramid_levels,
        quad_count: default_index.triple_count() as u64
            + named
                .iter()
                .map(|(_, idx)| idx.triple_count() as u64)
                .sum::<u64>(),
        term_count: dict.term_count() as u64,
        content_hash: content_hash(&parts),
        named_graphs_offset: if named_len > 0 { named_offset } else { 0 },
        named_graphs_len: named_len,
        schema_meta_len,
    };

    let mut out = Vec::with_capacity(
        HEADER_LEN
            + metadata.len()
            + dict_container.len()
            + index_container.len()
            + pyramid_meta.len()
            + named_section.len()
            + MAGIC.len(),
    );
    out.extend_from_slice(&header.to_bytes());
    if meta_section_len > 0 {
        out.extend_from_slice(metadata);
    }
    out.extend_from_slice(&dict_container);
    out.extend_from_slice(&index_container);
    out.extend_from_slice(pyramid_meta);
    if named_len > 0 {
        out.extend_from_slice(&named_section);
    }
    out.extend_from_slice(&MAGIC); // footer marker
    out
}

/// `rdf:type` â€” the predicate that assigns a class to a resource.
pub const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

/// An **ontology-aware** coarse graph: instead of structural communities, group
/// entities by their `rdf:type` class and aggregate relations between classes.
/// Returns `(subject_class, predicate, object_class, count)` over the default
/// graph. Entities with no type are `(untyped)`; literals are `(literal)`.
/// `rdf:type` triples themselves define the classes and are not counted as
/// relations. This is the dataset's effective schema with instance volumes.
pub fn schema_summary(rete: &Rete) -> Vec<(String, String, String, u32)> {
    use std::collections::{BTreeMap, HashMap};
    let triples = rete.dump(None);

    let mut class_of: HashMap<&str, &str> = HashMap::new();
    for (s, p, o) in &triples {
        if p == RDF_TYPE {
            class_of.insert(s.as_str(), o.as_str());
        }
    }
    let classify = |t: &str| -> String {
        if let Some(c) = class_of.get(t) {
            (*c).to_string()
        } else if t.starts_with('"') {
            "(literal)".to_string()
        } else {
            "(untyped)".to_string()
        }
    };

    let mut counts: BTreeMap<(String, String, String), u32> = BTreeMap::new();
    for (s, p, o) in &triples {
        if p == RDF_TYPE {
            continue; // type assertions define classes, not data relations
        }
        *counts
            .entry((classify(s), p.clone(), classify(o)))
            .or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((a, p, b), c)| (a, p, b, c))
        .collect()
}

/// Class populations: the number of resources of each `rdf:type` class in the
/// default graph, descending by count. The instance-count companion to
/// [`schema_summary`].
pub fn schema_classes(rete: &Rete) -> Vec<(String, u32)> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (_s, p, o) in rete.dump(None) {
        if p == RDF_TYPE {
            *counts.entry(o).or_default() += 1;
        }
    }
    let mut out: Vec<(String, u32)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Fetch **only** the metadata section (the opaque Dataset Card blob) via a
/// [`RangeReader`]: read the 128-byte header, then the metadata byte range â€”
/// nothing else. This is the index-free CARD tier of the exploration model: a
/// remote/S3 client learns the dataset's self-description in **two small range
/// requests**, never touching the dictionary, index, or pyramid. Returns `None`
/// when the file carries no metadata.
///
/// Companion to [`Rete::open_ranged`] (which deliberately *skips* the card to
/// keep the query path minimal); this is the explicit "I want the card" path.
pub fn read_metadata_ranged<R: RangeReader>(reader: &R) -> Result<Option<Vec<u8>>, FileError> {
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    if header.metadata_len == 0 {
        return Ok(None);
    }
    let bytes = reader.read_at(header.metadata_offset, header.metadata_len)?;
    Ok(Some(bytes))
}

/// Recompute the content hash from a file image and check it against the header
/// â€” detects corruption or truncation of the payload sections.
pub fn verify(bytes: &[u8]) -> Result<bool, FileError> {
    let header = Header::from_bytes(bytes)?;
    let slice = |off: u64, len: u64| -> Result<&[u8], FileError> {
        bytes
            .get(off as usize..(off + len) as usize)
            .ok_or(FileError::Container("section overruns buffer"))
    };
    let d = slice(header.dictionary_offset, header.dictionary_len)?;
    let i = slice(header.root_dir_offset, header.root_dir_len)?;
    let m = if header.pyramid_meta_len > 0 {
        slice(header.pyramid_meta_offset, header.pyramid_meta_len)?
    } else {
        &[]
    };
    // Match the writer's ordering exactly: the metadata payload is prepended when
    // present, then dict, index, pyramid-meta, and (if any) the named graphs.
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    if header.metadata_len > 0 {
        parts.push(slice(header.metadata_offset, header.metadata_len)?);
    }
    parts.push(d);
    parts.push(i);
    parts.push(m);
    if header.named_graphs_len > 0 {
        parts.push(slice(header.named_graphs_offset, header.named_graphs_len)?);
    }
    Ok(content_hash(&parts) == header.content_hash)
}

/// Faults the pyramid meta in on first access. `None` = the fetch failed.
type PyramidLoader = Box<dyn Fn() -> Option<PyramidMeta> + Send + Sync>;

/// The pyramid-meta section, held either resident (eager opens) or deferred
/// (the lazy remote open). SPARQL never touches the pyramid, but on a Wikidata
/// file it can be tens of MB (114k communities, millions of superedges), so a
/// remote SPARQL query must not pay to fetch it — it faults in only when a
/// community/pyramid query actually calls [`Rete::pyramid`].
enum PyramidSlot {
    Resident(Option<PyramidMeta>),
    Lazy {
        loader: PyramidLoader,
        cell: std::sync::OnceLock<Option<PyramidMeta>>,
    },
}

/// A read-only, in-memory view over a `.rete` file image.
pub struct Rete {
    header: Header,
    dict: Dictionary,
    index: GraphIndex,
    index_section_ranges: [ByteRange; 3],
    /// Per-permutation tile directories as absolute file ranges
    /// (`(min_a, max_a, compressed-tile range)`), for provenance. Empty for
    /// pre-tiling (v0.1) files.
    tile_ranges: [Vec<(u32, u32, ByteRange)>; 3],
    pyramid: PyramidSlot,
    named_graphs: Vec<(String, GraphIndex)>,
    /// Raw bytes of the metadata section (empty if the file has none). The
    /// application layer decodes this (the CLI stores a JSON Dataset Card here).
    /// Only [`Rete::open`] populates it; [`Rete::open_ranged`] leaves it empty to
    /// preserve its minimal-fetch budget.
    metadata: Vec<u8>,
}

impl Rete {
    /// Parse a full file image (v0 loads everything; a range-reading client
    /// will fetch only the sections it needs â€” same container format).
    pub fn open(bytes: &[u8]) -> Result<Self, FileError> {
        let header = Header::from_bytes(bytes)?;

        // Header offsets/lengths are untrusted (a `.rete` may be fetched truncated
        // or corrupt from an arbitrary URL). Slice through a checked helper so a
        // bad region yields an error instead of panicking on an OOB index.
        let region = |off: u64, len: u64| -> Result<&[u8], FileError> {
            let start = off as usize;
            let end = start
                .checked_add(len as usize)
                .filter(|&e| e <= bytes.len())
                .ok_or(FileError::Container("section range out of bounds"))?;
            Ok(&bytes[start..end])
        };

        let dict = decode_dictionary_container(
            region(header.dictionary_offset, header.dictionary_len)?,
            header.dict_codec,
            header.version,
        )?;

        let index_bytes = region(header.root_dir_offset, header.root_dir_len)?;
        let index = decode_index_container(index_bytes, header.block_codec, header.version)?;
        let index_section_ranges =
            decode_index_section_ranges(index_bytes, header.root_dir_offset)?;

        let pyramid = PyramidSlot::Resident(if header.pyramid_meta_len > 0 {
            Some(
                PyramidMeta::decode(region(header.pyramid_meta_offset, header.pyramid_meta_len)?)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        });

        let named_graphs = if header.named_graphs_len > 0 {
            decode_named_graphs(
                region(header.named_graphs_offset, header.named_graphs_len)?,
                header.block_codec,
                header.version,
            )?
        } else {
            Vec::new()
        };

        let metadata = if header.metadata_len > 0 {
            region(header.metadata_offset, header.metadata_len)?.to_vec()
        } else {
            Vec::new()
        };

        let tile_ranges = tile_file_ranges(
            index_bytes,
            header.root_dir_offset,
            &index_section_ranges,
            header.version,
        );
        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            named_graphs,
            metadata,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// The file's byte layout, for visualization: header, metadata,
    /// dictionary, each index permutation's tile directory and individual
    /// tiles, pyramid summary, and named graphs — sorted by offset. Bytes not
    /// covered by any segment are container framing (section directories and
    /// length fields).
    pub fn file_layout(&self) -> Vec<LayoutSegment> {
        let h = &self.header;
        let seg = |kind: &'static str, label: String, offset: u64, len: u64| LayoutSegment {
            kind,
            label,
            offset,
            len,
        };
        let mut out = vec![seg(
            "header",
            "header (fixed 128 bytes)".into(),
            0,
            crate::header::HEADER_LEN as u64,
        )];
        if h.metadata_len > 0 {
            out.push(seg(
                "metadata",
                "metadata (dataset card)".into(),
                h.metadata_offset,
                h.metadata_len,
            ));
        }
        out.push(seg(
            "dictionary",
            "dictionary (4 front-coded term sections)".into(),
            h.dictionary_offset,
            h.dictionary_len,
        ));
        for (si, perm) in [
            IndexPermutation::Spo,
            IndexPermutation::Pos,
            IndexPermutation::Osp,
        ]
        .into_iter()
        .enumerate()
        {
            let sec = self.index_section_ranges[si];
            if sec.len == 0 {
                continue;
            }
            let first_tile = self.tile_ranges[si]
                .first()
                .map(|&(_, _, r)| r.offset)
                .unwrap_or(sec.offset + sec.len);
            if first_tile > sec.offset {
                out.push(seg(
                    "directory",
                    format!("{} tile directory", perm.name()),
                    sec.offset,
                    first_tile - sec.offset,
                ));
            }
            for (ti, &(min_a, max_a, r)) in self.tile_ranges[si].iter().enumerate() {
                out.push(seg(
                    "tile",
                    format!("{} tile {ti} (leading ids {min_a}..{max_a})", perm.name()),
                    r.offset,
                    r.len,
                ));
            }
        }
        if h.pyramid_meta_len > 0 {
            out.push(seg(
                "pyramid",
                "pyramid summary (communities + superedges)".into(),
                h.pyramid_meta_offset,
                h.pyramid_meta_len,
            ));
        }
        if h.named_graphs_len > 0 {
            out.push(seg(
                "named-graphs",
                format!("named graphs ({})", self.named_graphs.len()),
                h.named_graphs_offset,
                h.named_graphs_len,
            ));
        }
        out.sort_by_key(|s| s.offset);
        out
    }

    /// Raw bytes of the file's metadata section, or `None` if it has none. The
    /// CLI stores a JSON Dataset Card here; `rete-core` treats it as opaque.
    /// Populated by [`Rete::open`] only â€” an [`Rete::open_ranged`] view returns
    /// `None` here (the card is not fetched on the minimal query path).
    pub fn metadata(&self) -> Option<&[u8]> {
        if self.metadata.is_empty() {
            None
        } else {
            Some(&self.metadata)
        }
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dict
    }

    /// The pyramid metadata (summary graph + tiles), if the file has a pyramid.
    pub fn pyramid(&self) -> Option<&PyramidMeta> {
        match &self.pyramid {
            PyramidSlot::Resident(p) => p.as_ref(),
            // Faults the (possibly large) pyramid section on first access only.
            PyramidSlot::Lazy { loader, cell } => cell.get_or_init(loader).as_ref(),
        }
    }

    /// The pyramid metadata **only if already resident or previously faulted** —
    /// never triggers a lazy range read. The query planner uses this for
    /// cardinality estimation so it is free for an in-memory file and never adds
    /// a fetch on the lazy remote path (which defers the pyramid by design).
    pub fn pyramid_if_loaded(&self) -> Option<&PyramidMeta> {
        match &self.pyramid {
            PyramidSlot::Resident(p) => p.as_ref(),
            PyramidSlot::Lazy { cell, .. } => cell.get().and_then(|o| o.as_ref()),
        }
    }

    /// The default-graph permutation index.
    pub fn default_index(&self) -> &GraphIndex {
        &self.index
    }

    /// Resolve every triple of a graph (`None` = default graph) back to terms.
    pub fn dump(&self, graph: Option<&str>) -> Vec<TermTriple> {
        // A dump resolves every term: batch-fault the whole dictionary up
        // front (coalesced range reads on a lazy remote open; no-op locally).
        self.dict.prefetch_all();
        let index = match graph {
            None => &self.index,
            Some(g) => match self.graph_index(g) {
                Some(i) => i,
                None => return Vec::new(),
            },
        };
        index
            .match_pattern((None, None, None))
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
            .collect()
    }

    /// All named graphs as `(iri, index)`.
    pub fn named_graphs(&self) -> &[(String, GraphIndex)] {
        &self.named_graphs
    }

    /// IRIs of the named graphs in this dataset (the default graph is unnamed).
    pub fn graph_names(&self) -> Vec<&str> {
        self.named_graphs
            .iter()
            .map(|(iri, _)| iri.as_str())
            .collect()
    }

    /// The permutation index of a named graph, or `None` if absent.
    pub fn graph_index(&self, iri: &str) -> Option<&GraphIndex> {
        self.named_graphs
            .iter()
            .find(|(name, _)| name == iri)
            .map(|(_, idx)| idx)
    }

    /// Match a triple pattern in dictionary-ID space (subject/predicate/object
    /// IDs), returning integer triples â€” the fast path used by the BGP engine.
    pub fn match_ids(
        &self,
        pattern: (Option<u32>, Option<u32>, Option<u32>),
    ) -> Vec<(u32, u32, u32)> {
        self.index.match_pattern(pattern)
    }

    /// All `(subject_node, object_node)` pairs for a predicate, as unified node
    /// IDs â€” no term resolution. The fast path for graph traversal.
    pub fn predicate_pairs(&self, predicate: &str) -> Vec<(u32, u32)> {
        let pid = match self.dict.predicate_id(predicate) {
            Some(p) => p,
            None => return Vec::new(),
        };
        self.index
            .match_pattern((None, Some(pid), None))
            .into_iter()
            .map(|(s, _p, o)| (self.dict.subject_node(s), self.dict.object_node(o)))
            .collect()
    }

    /// Open via a [`RangeReader`], fetching only the header and the named
    /// section ranges â€” never a linear scan of the whole resource. A full query
    /// open touches at most 4 ranges (header, dictionary, index, pyramid-meta).
    pub fn open_ranged<R: RangeReader>(reader: &R) -> Result<Self, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;

        let dict_bytes = reader.read_at(header.dictionary_offset, header.dictionary_len)?;
        let dict = decode_dictionary_container(&dict_bytes, header.dict_codec, header.version)?;

        let index_bytes = reader.read_at(header.root_dir_offset, header.root_dir_len)?;
        let index = decode_index_container(&index_bytes, header.block_codec, header.version)?;
        let index_section_ranges =
            decode_index_section_ranges(&index_bytes, header.root_dir_offset)?;

        let pyramid = PyramidSlot::Resident(if header.pyramid_meta_len > 0 {
            let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
            Some(
                PyramidMeta::decode(&mb)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        });

        let named_graphs = if header.named_graphs_len > 0 {
            let nb = reader.read_at(header.named_graphs_offset, header.named_graphs_len)?;
            decode_named_graphs(&nb, header.block_codec, header.version)?
        } else {
            Vec::new()
        };

        // The metadata section (Dataset Card) is deliberately NOT fetched here:
        // a ranged query open keeps to its small range budget. Use `Rete::open`
        // (or a dedicated card fetch) when the card is actually needed.
        let tile_ranges = tile_file_ranges(
            &index_bytes,
            header.root_dir_offset,
            &index_section_ranges,
            header.version,
        );
        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            named_graphs,
            metadata: Vec::new(),
        })
    }

    /// Open via an **owned** [`RangeReader`] with lazy tile faulting (tiled
    /// v0.2 files): fetches the header, dictionary, pyramid meta, named graphs,
    /// and each permutation's tile **directory** â€” but no default-graph tile
    /// payloads. Tiles fault in (one range request each) the first time a scan
    /// touches them, so a selective SPARQL query fetches O(touched tiles)
    /// bytes instead of the whole index.
    ///
    /// **Failure contract:** scans are infallible by design, so a failed tile
    /// fetch yields an empty tile and sets a sticky flag â€” after evaluating,
    /// callers MUST check [`index_incomplete`](Self::index_incomplete) and
    /// surface an error instead of the (possibly partial) results.
    ///
    /// Pre-tiling (v0.1) files fall back to [`Rete::open_ranged`] â€” there is
    /// nothing lazy to do.
    pub fn open_ranged_lazy<R: RangeReader + Send + Sync + 'static>(
        reader: R,
    ) -> Result<Self, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        if header.version < 2 {
            return Self::open_ranged(&reader);
        }
        let reader = std::sync::Arc::new(reader);

        // Lazily-chunked dictionary: locate the four sections, fetch each
        // section's header + restart table + chunk directory (small), and
        // fault the chunk bodies in on first term lookup.
        let mut dict_sections: Vec<crate::dict::ChunkedSection> = Vec::with_capacity(4);
        for si in 0..4 {
            let section = locate_container_section_ranged(
                reader.as_ref(),
                header.dictionary_offset,
                header.dictionary_len,
                si,
                4,
            )?;
            let (meta, entries) = read_dict_dir_ranged(reader.as_ref(), section)?;
            let ranges: Vec<ByteRange> = entries
                .iter()
                .map(|e| ByteRange {
                    offset: section.offset + e.start as u64,
                    len: (e.end - e.start) as u64,
                })
                .collect();
            let chunks: Vec<crate::dict::SectionChunk> = entries
                .into_iter()
                .map(|e| crate::dict::SectionChunk::remote(e.first_run, e.first_term, e.body_start))
                .collect();
            let chunk_reader = reader.clone();
            let codec = header.dict_codec;
            let loader_ranges = ranges.clone();
            let loader: crate::dict::ChunkLoader = Box::new(move |ci| {
                let range = loader_ranges.get(ci)?;
                let bytes = chunk_reader.read_at(range.offset, range.len).ok()?;
                decompress(codec, &bytes).ok()
            });
            // Full-section sweeps (export/dump) batch their chunk fetches:
            // adjacent chunk ranges coalesce into a handful of range reads.
            let bulk_reader = reader.clone();
            let bulk: crate::dict::ChunkBulkLoader = Box::new(move |cis| {
                let want: Option<Vec<ByteRange>> =
                    cis.iter().map(|&ci| ranges.get(ci).copied()).collect();
                let blobs = read_coalesced(bulk_reader.as_ref(), &want?, DICT_COALESCE_GAP)?;
                blobs.iter().map(|b| decompress(codec, b).ok()).collect()
            });
            dict_sections.push(
                crate::dict::ChunkedSection::from_parts(meta, chunks, Some(loader))
                    .with_bulk_loader(bulk),
            );
        }
        let dict_arr: [crate::dict::ChunkedSection; 4] = dict_sections
            .try_into()
            .map_err(|_| FileError::Container("expected 4 dictionary sections"))?;
        let dict = Dictionary::from_chunked_sections(dict_arr);

        // Locate the three index section payloads (container framing only)
        // and fetch just their tile directories.
        let mut index_section_ranges = [ByteRange { offset: 0, len: 0 }; 3];
        let mut tile_ranges: [Vec<(u32, u32, ByteRange)>; 3] = Default::default();
        let mut directories: [Vec<(u32, u32)>; 3] = Default::default();
        for si in 0..3 {
            let section = locate_container_section_ranged(
                reader.as_ref(),
                header.root_dir_offset,
                header.root_dir_len,
                si,
                3,
            )?;
            index_section_ranges[si] = section;
            let dir = read_tile_directory_ranged(reader.as_ref(), section)?;
            directories[si] = dir.iter().map(|e| (e.min_a, e.max_a)).collect();
            tile_ranges[si] = dir
                .into_iter()
                .map(|e| {
                    (
                        e.min_a,
                        e.max_a,
                        ByteRange {
                            offset: section.offset + e.start as u64,
                            len: (e.end - e.start) as u64,
                        },
                    )
                })
                .collect();
        }

        // The pyramid meta is large on real graphs (tens of MB) and SPARQL never
        // reads it, so defer its fetch: it faults in only if `pyramid()` is
        // called (community / pyramid_tree / inspect queries).
        let pyramid = if header.pyramid_meta_len > 0 {
            let pyr_reader = reader.clone();
            let pyr_off = header.pyramid_meta_offset;
            let pyr_len = header.pyramid_meta_len;
            PyramidSlot::Lazy {
                loader: Box::new(move || {
                    let mb = pyr_reader.read_at(pyr_off, pyr_len).ok()?;
                    PyramidMeta::decode(&mb).ok()
                }),
                cell: std::sync::OnceLock::new(),
            }
        } else {
            PyramidSlot::Resident(None)
        };

        let named_graphs = if header.named_graphs_len > 0 {
            let nb = reader.read_at(header.named_graphs_offset, header.named_graphs_len)?;
            decode_named_graphs(&nb, header.block_codec, header.version)?
        } else {
            Vec::new()
        };

        // The loader fetches and decompresses one tile per call; the bulk
        // loader serves multi-tile scans by coalescing adjacent tile ranges
        // into single range reads (tiles are back-to-back in their section,
        // so a full-section scan is typically one request).
        let codec = header.block_codec;
        let loader_ranges = tile_ranges.clone();
        let loader_reader = reader.clone();
        let loader: crate::index::TileLoader = Box::new(move |si, ti| {
            let (_, _, range) = loader_ranges.get(si)?.get(ti)?;
            let bytes = loader_reader.read_at(range.offset, range.len).ok()?;
            decompress(codec, &bytes).ok()
        });
        let bulk_ranges = tile_ranges.clone();
        let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis| {
            let section = bulk_ranges.get(si)?;
            let want: Option<Vec<ByteRange>> = tis
                .iter()
                .map(|&ti| section.get(ti).map(|&(_, _, r)| r))
                .collect();
            let blobs = read_coalesced(reader.as_ref(), &want?, TILE_COALESCE_GAP)?;
            blobs.iter().map(|b| decompress(codec, b).ok()).collect()
        });
        let index = GraphIndex::from_remote_directories(directories, loader).with_bulk_loader(bulk);

        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            named_graphs,
            metadata: Vec::new(),
        })
    }

    /// Did any lazy fetch (index tile or dictionary chunk) fail since this
    /// `Rete` was opened? When true, query results may be silently incomplete â€”
    /// callers using [`Rete::open_ranged_lazy`] must check this after
    /// evaluating and turn it into an error.
    pub fn index_incomplete(&self) -> bool {
        self.index.load_incomplete()
            || self.dict.load_incomplete()
            || self.named_graphs.iter().any(|(_, g)| g.load_incomplete())
    }

    fn resolve_query_pattern(
        &self,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Option<Pattern> {
        let sid = match s {
            Some(t) => match self.dict.subject_id(t) {
                Some(id) => Some(id),
                None => return None,
            },
            None => None,
        };
        let pid = match p {
            Some(t) => match self.dict.predicate_id(t) {
                Some(id) => Some(id),
                None => return None,
            },
            None => None,
        };
        let oid = match o {
            Some(t) => match self.dict.object_id(t) {
                Some(id) => Some(id),
                None => return None,
            },
            None => None,
        };
        Some((sid, pid, oid))
    }

    /// Evaluate a triple pattern and include the file/index provenance for every
    /// matched result. A bound term that is unknown to the dictionary yields no
    /// matches.
    pub fn query_with_provenance(
        &self,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Vec<TripleProvenance> {
        let pattern = match self.resolve_query_pattern(s, p, o) {
            Some(pattern) => pattern,
            None => return Vec::new(),
        };

        let index_permutation = GraphIndex::best_permutation(pattern);
        let dictionary_range = ByteRange {
            offset: self.header.dictionary_offset,
            len: self.header.dictionary_len,
        };
        let index_range = ByteRange {
            offset: self.header.root_dir_offset,
            len: self.header.root_dir_len,
        };
        let index_section_range = self.index_section_ranges[index_permutation.section_index()];
        let pyramid_range = (self.header.pyramid_meta_len > 0).then_some(ByteRange {
            offset: self.header.pyramid_meta_offset,
            len: self.header.pyramid_meta_len,
        });

        let tiles = &self.tile_ranges[index_permutation.section_index()];
        self.index
            .match_pattern(pattern)
            .into_iter()
            .filter_map(|(s, p, o)| {
                let terms = (
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                );
                // The physical tile holding this match: the one whose
                // leading-id range covers the match's permuted leading id.
                let a = index_permutation.forward((s, p, o)).0;
                let ti = tiles.partition_point(|&(_, max_a, _)| max_a < a);
                let (tile, tile_range) = match tiles.get(ti) {
                    Some(&(min_a, _, range)) if min_a <= a => (
                        Some(format!("{}/{ti}", index_permutation.name())),
                        Some(range),
                    ),
                    _ => (None, None),
                };
                Some(TripleProvenance {
                    terms,
                    ids: (s, p, o),
                    graph: None,
                    matched_pattern: pattern,
                    index_permutation,
                    dictionary_range,
                    index_range,
                    index_section_range,
                    pyramid_range,
                    tile,
                    tile_range,
                })
            })
            .collect()
    }

    /// Evaluate a triple pattern given as optional term strings, returning
    /// matching triples resolved back to terms. A bound term that is unknown to
    /// the dictionary yields no matches.
    pub fn query(&self, s: Option<&str>, p: Option<&str>, o: Option<&str>) -> Vec<TermTriple> {
        self.query_with_provenance(s, p, o)
            .into_iter()
            .map(|m| m.terms)
            .collect()
    }

    /// Evaluate one triple pattern through a [`RangeReader`] by fetching only
    /// the header, the dictionary, and â€” for a tiled (v0.2) file â€” the
    /// selected permutation section's tile **directory** plus the tile(s) the
    /// bound leading id routes to; an unbound leading id fetches the section's
    /// tile body in one request. v0.1 files fetch the whole selected section.
    /// Unknown bound terms return an empty result before touching the index.
    pub fn query_ranged<R: RangeReader>(
        reader: &R,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Result<Vec<TermTriple>, FileError> {
        let routed = match route_pattern(reader, s, p, o)? {
            Some(routed) => routed,
            None => return Ok(Vec::new()),
        };
        let matches = fetch_routed_matches(reader, &routed)?;
        Ok(matches
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    routed.dict.subject_term(s)?,
                    routed.dict.predicate_term(p)?,
                    routed.dict.object_term(o)?,
                ))
            })
            .collect())
    }

    /// Route one triple pattern to its permutation section without fetching
    /// any payload bytes. Returns `false` when a bound term is unknown and the
    /// index was skipped.
    pub fn route_pattern_ranged<R: RangeReader>(
        reader: &R,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Result<bool, FileError> {
        Ok(route_pattern(reader, s, p, o)?.is_some())
    }
}

/// A pattern routed to its permutation section: everything needed to fetch
/// matches, with no payload bytes read yet.
struct RoutedPattern {
    dict: Dictionary,
    pattern: Pattern,
    permutation: IndexPermutation,
    header: Header,
    /// Absolute byte range of the selected section's payload.
    section: ByteRange,
}

/// Resolve a pattern against the remote dictionary and locate its permutation
/// section (header + dictionary + container framing only).
fn route_pattern<R: RangeReader>(
    reader: &R,
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
) -> Result<Option<RoutedPattern>, FileError> {
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;

    let dict_bytes = reader.read_at(header.dictionary_offset, header.dictionary_len)?;
    let dict = decode_dictionary_container(&dict_bytes, header.dict_codec, header.version)?;

    let Some(pattern) = resolve_query_pattern(&dict, s, p, o) else {
        return Ok(None);
    };
    let permutation = GraphIndex::best_permutation(pattern);
    let section = locate_container_section_ranged(
        reader,
        header.root_dir_offset,
        header.root_dir_len,
        permutation.section_index(),
        3,
    )?;
    Ok(Some(RoutedPattern {
        dict,
        pattern,
        permutation,
        header,
        section,
    }))
}

/// Fetch and scan a routed pattern's matches. v0.2: read the tile directory,
/// then only the matching tile byte ranges (one tile for a bound leading id â€”
/// the O(matching bytes) promise); v0.1: the whole section, decompressed.
fn fetch_routed_matches<R: RangeReader>(
    reader: &R,
    routed: &RoutedPattern,
) -> Result<Vec<Triple>, FileError> {
    if routed.header.version < 2 {
        let payload = reader.read_at(routed.section.offset, routed.section.len)?;
        let block = decompress(routed.header.block_codec, &payload)?;
        return Ok(GraphIndex::match_serialized_block(
            &block,
            routed.permutation,
            routed.pattern,
        ));
    }

    let dir = read_tile_directory_ranged(reader, routed.section)?;
    let [pa, _, _] = routed.permutation.order_pattern(routed.pattern);
    let codec = routed.header.block_codec;
    let mut out = Vec::new();
    match pa {
        // Bound leading id: at most one tile contains it.
        Some(a) => {
            for e in dir.iter().filter(|e| e.min_a <= a && a <= e.max_a) {
                let bytes = reader.read_at(
                    routed.section.offset + e.start as u64,
                    (e.end - e.start) as u64,
                )?;
                let tile = decompress(codec, &bytes)?;
                out.extend(GraphIndex::match_serialized_block(
                    &tile,
                    routed.permutation,
                    routed.pattern,
                ));
            }
        }
        // Unbound leading id: every tile matters â€” fetch the contiguous tile
        // body in one request and slice it.
        None => {
            if let (Some(first), Some(last)) = (dir.first(), dir.last()) {
                let base = first.start;
                let body = reader.read_at(
                    routed.section.offset + base as u64,
                    (last.end - base) as u64,
                )?;
                for e in &dir {
                    let tile = decompress(codec, &body[e.start - base..e.end - base])?;
                    out.extend(GraphIndex::match_serialized_block(
                        &tile,
                        routed.permutation,
                        routed.pattern,
                    ));
                }
            }
        }
    }
    out.sort_unstable();
    Ok(out)
}

fn resolve_query_pattern(
    dict: &Dictionary,
    s: Option<&str>,
    p: Option<&str>,
    o: Option<&str>,
) -> Option<Pattern> {
    let sid = match s {
        Some(t) => Some(dict.subject_id(t)?),
        None => None,
    };
    let pid = match p {
        Some(t) => Some(dict.predicate_id(t)?),
        None => None,
    };
    let oid = match o {
        Some(t) => Some(dict.object_id(t)?),
        None => None,
    };
    Some((sid, pid, oid))
}

/// A lightweight, overview-only view of a file: the pyramid summary graph plus
/// just enough dictionary to label predicates. Fetched via ranges *without*
/// touching the (large) triple index â€” the "load the coarse graph first" path
/// from SPEC.md Â§7.2.
pub struct SummaryView {
    pub round: u32,
    pub summary: Vec<SuperEdge>,
    /// The shipped `subClassOf` hierarchy (v2 schema pyramid; empty on v1 files).
    pub class_hierarchy: Vec<ClassNode>,
    /// Per-level type rollups — the leveled legend, read index-free.
    pub level_rollups: Vec<LevelRollup>,
    /// Per-level lateral class-relation graph (the non-`is-a` connections).
    pub level_links: Vec<LevelLinks>,
    /// Per-community descriptors (Phase 4 progressive refinement; may be empty).
    pub descriptors: Vec<CommunityDescriptor>,
    /// `subClassOf` cycles (v2.1; empty on older files).
    pub subclass_cycles: Vec<Vec<String>>,
    /// `owl:disjointWith` class pairs (v2.1; empty on older files).
    pub disjoint_pairs: Vec<(String, String)>,
    /// `owl:equivalentClass` class pairs (v2.1; empty on older files).
    pub equivalent_pairs: Vec<(String, String)>,
    dict: Dictionary,
}

impl SummaryView {
    /// Read header â†’ dictionary â†’ pyramid-meta only (skips the index container).
    pub fn open_ranged<R: RangeReader>(reader: &R) -> Result<Option<Self>, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        if header.pyramid_meta_len == 0 {
            return Ok(None);
        }

        let dict_bytes = reader.read_at(header.dictionary_offset, header.dictionary_len)?;
        let dict = decode_dictionary_container(&dict_bytes, header.dict_codec, header.version)?;

        let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
        let meta =
            PyramidMeta::decode(&mb).map_err(|_| FileError::Container("malformed pyramid meta"))?;

        Ok(Some(SummaryView {
            round: meta.round,
            summary: meta.summary,
            class_hierarchy: meta.class_hierarchy,
            level_rollups: meta.level_rollups,
            level_links: meta.level_links,
            descriptors: meta.descriptors,
            subclass_cycles: meta.subclass_cycles,
            disjoint_pairs: meta.disjoint_pairs,
            equivalent_pairs: meta.equivalent_pairs,
            dict,
        }))
    }

    /// Number of semantic-zoom levels in the schema pyramid (0 if none shipped).
    pub fn level_count(&self) -> usize {
        self.level_rollups.len()
    }

    /// The type rollup at semantic level `k` (0 = coarsest/most abstract), or
    /// `None` if `k` is out of range. Index-free — answered from the pyramid-meta.
    pub fn level_rollup(&self, k: usize) -> Option<&LevelRollup> {
        self.level_rollups.get(k)
    }

    /// Resolve a predicate ID in the summary to its term.
    pub fn predicate_term(&self, id: u32) -> Option<String> {
        self.dict.predicate_term(id)
    }

    /// Exact number of triples using `predicate`, summed from the summary's
    /// superedge counts â€” answered without ever reading the triple index.
    pub fn predicate_total(&self, predicate: &str) -> u32 {
        match self.dict.predicate_id(predicate) {
            Some(pid) => self
                .summary
                .iter()
                .filter(|e| e.predicate == pid)
                .map(|e| e.count)
                .sum(),
            None => 0,
        }
    }

    /// All predicates with their exact triple totals, descending by count.
    pub fn predicate_totals(&self) -> Vec<(String, u32)> {
        let mut by_pred: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for e in &self.summary {
            *by_pred.entry(e.predicate).or_default() += e.count;
        }
        let mut out: Vec<(String, u32)> = by_pred
            .into_iter()
            .filter_map(|(pid, c)| self.dict.predicate_term(pid).map(|t| (t, c)))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Number of communities the summary spans (distinct supernode endpoints).
    pub fn community_count(&self) -> usize {
        let mut comms = std::collections::BTreeSet::new();
        for e in &self.summary {
            comms.insert(e.s_comm);
            comms.insert(e.o_comm);
        }
        comms.len()
    }

    /// **Index-free T-Box coherence (Tier-0).** Detect schema-level incoherent
    /// points purely from the shipped schema pyramid — no triple index, no
    /// instance data, O(ontology) regardless of graph size:
    /// - `subclass-cycle`: a set of classes that are mutually `rdfs:subClassOf`.
    /// - `unsatisfiable-class`: a class whose ancestor closure (over all parents,
    ///   folded through `owl:equivalentClass`) contains both ends of an
    ///   `owl:disjointWith` pair, so no individual can ever be one.
    ///
    /// Soundness is bounded by what the pyramid ships: the `subClassOf` hierarchy
    /// is capped (`MAX_HIERARCHY` in `schema_pyramid`), so on a very large ontology
    /// a pruned ancestor can hide an unsatisfiable class (a false *coherent*, never
    /// a false *incoherent*). Instance-level clashes (a node typed into disjoint
    /// classes, functional-property clashes) are NOT visible here — they need the
    /// A-Box (Tier-1/Tier-2 `reason`).
    pub fn tbox_coherence(&self) -> Vec<crate::reason::Inconsistency> {
        schema_coherence(
            &self.class_hierarchy,
            &self.subclass_cycles,
            &self.disjoint_pairs,
            &self.equivalent_pairs,
        )
    }

    /// True when [`tbox_coherence`](Self::tbox_coherence) finds no schema-level
    /// incoherent point.
    pub fn tbox_is_coherent(&self) -> bool {
        self.tbox_coherence().is_empty()
    }
}

/// Compute T-Box coherence points from the schema-pyramid fields alone — no
/// dictionary, no index, no instance data. Shared by [`SummaryView::tbox_coherence`]
/// and the dictionary-free [`read_schema_coherence_ranged`]. Emits `subclass-cycle`
/// and `unsatisfiable-class` (a class whose ancestor closure — over all parents,
/// folded through `owl:equivalentClass` — contains both ends of a disjoint pair).
pub fn schema_coherence(
    class_hierarchy: &[ClassNode],
    subclass_cycles: &[Vec<String>],
    disjoint_pairs: &[(String, String)],
    equivalent_pairs: &[(String, String)],
) -> Vec<crate::reason::Inconsistency> {
    use crate::reason::Inconsistency;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    const MAX_REACH: usize = 100_000;

    let mut out: Vec<Inconsistency> = Vec::new();

    for cyc in subclass_cycles {
        let detail = if cyc.len() == 1 {
            format!("{} is rdfs:subClassOf itself (a cycle)", cyc[0])
        } else {
            format!(
                "classes {{{}}} are mutually rdfs:subClassOf (a cycle)",
                cyc.join(", ")
            )
        };
        out.push(Inconsistency {
            kind: "subclass-cycle",
            detail,
        });
    }

    if !disjoint_pairs.is_empty() {
        // Upward adjacency: subClassOf parents + bidirectional equivalence.
        let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for n in class_hierarchy {
            let e = adj.entry(n.class.as_str()).or_default();
            for p in &n.parents {
                e.push(p.as_str());
            }
        }
        for (a, b) in equivalent_pairs {
            adj.entry(a.as_str()).or_default().push(b.as_str());
            adj.entry(b.as_str()).or_default().push(a.as_str());
        }

        // Candidate focus classes: every class named anywhere in the schema.
        let mut focuses: BTreeSet<&str> =
            class_hierarchy.iter().map(|n| n.class.as_str()).collect();
        for (a, b) in disjoint_pairs.iter().chain(equivalent_pairs) {
            focuses.insert(a.as_str());
            focuses.insert(b.as_str());
        }

        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for &c in &focuses {
            // reach(c) = {c} ∪ ancestors (capped BFS over `adj`).
            let mut reach: BTreeSet<&str> = BTreeSet::new();
            let mut q: VecDeque<&str> = VecDeque::new();
            reach.insert(c);
            q.push_back(c);
            while let Some(x) = q.pop_front() {
                if reach.len() > MAX_REACH {
                    break;
                }
                if let Some(ns) = adj.get(x) {
                    for &p in ns {
                        if reach.insert(p) {
                            q.push_back(p);
                        }
                    }
                }
            }
            for (x, y) in disjoint_pairs {
                if reach.contains(x.as_str()) && reach.contains(y.as_str()) && seen.insert(c) {
                    out.push(Inconsistency {
                        kind: "unsatisfiable-class",
                        detail: format!(
                            "{c} is a subclass of both {x} and {y}, which are \
                             owl:disjointWith — no individual can be a {c}"
                        ),
                    });
                    break;
                }
            }
        }
    }

    out.sort_by(|a, b| (a.kind, &a.detail).cmp(&(b.kind, &b.detail)));
    out
}

/// **Dictionary-free Tier-0 coherence read.** Fetch only the header and the
/// pyramid-meta range (2 small range reads) and run [`schema_coherence`] over the
/// schema pyramid — never touching the **dictionary** (which a literal-heavy file
/// makes large) or the triple index. `Ok(None)` if the file ships no pyramid.
///
/// This is what makes the Tier-0 check cheap on big graphs: the schema pyramid
/// carries its own class-string table, so coherence needs none of the dictionary.
pub fn read_schema_coherence_ranged<R: RangeReader>(
    reader: &R,
) -> Result<Option<Vec<crate::reason::Inconsistency>>, FileError> {
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    if header.pyramid_meta_len == 0 {
        return Ok(None);
    }
    // Fast path: the header records the trailing schema block's length, so read ONLY
    // that block (at the end of pyramid-meta) — never the community summary, the
    // dictionary, or the index. This is what makes Tier-0 flat at any graph size.
    if header.schema_meta_len > 0 && (header.schema_meta_len as u64) <= header.pyramid_meta_len {
        let off =
            header.pyramid_meta_offset + header.pyramid_meta_len - header.schema_meta_len as u64;
        let block = reader.read_at(off, header.schema_meta_len as u64)?;
        let (hierarchy, cycles, disjoint, equivalent) = crate::meta::decode_schema_block(&block)
            .map_err(|_| FileError::Container("malformed schema block"))?;
        return Ok(Some(schema_coherence(
            &hierarchy,
            &cycles,
            &disjoint,
            &equivalent,
        )));
    }
    // Fallback (pre-v0.2.1 files with no header field): decode the whole pyramid-meta.
    let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
    let meta =
        PyramidMeta::decode(&mb).map_err(|_| FileError::Container("malformed pyramid meta"))?;
    Ok(Some(schema_coherence(
        &meta.class_hierarchy,
        &meta.subclass_cycles,
        &meta.disjoint_pairs,
        &meta.equivalent_pairs,
    )))
}

/// The **schema summary** (per-class histogram + class relations at the finest
/// level) read over a [`RangeReader`] from the schema pyramid alone — the
/// index-free, range-readable source for a Schema view of a remote graph. Returns
/// `(classes, relations)` with `classes = [(class_iri, count)]` and `relations =
/// [(s_class, predicate, o_class, count)]`; `None` when the file has no schema
/// pyramid. Like [`read_schema_coherence_ranged`], it reads only the trailing
/// schema block, so it stays flat at any graph size.
#[allow(clippy::type_complexity)]
pub fn read_schema_summary_ranged<R: RangeReader>(
    reader: &R,
) -> Result<Option<(Vec<(String, u64)>, Vec<(String, String, String, u64)>)>, FileError> {
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    if header.pyramid_meta_len == 0 {
        return Ok(None);
    }
    if header.schema_meta_len > 0 && (header.schema_meta_len as u64) <= header.pyramid_meta_len {
        let off =
            header.pyramid_meta_offset + header.pyramid_meta_len - header.schema_meta_len as u64;
        let block = reader.read_at(off, header.schema_meta_len as u64)?;
        let summary = crate::meta::decode_schema_block_summary(&block)
            .map_err(|_| FileError::Container("malformed schema block"))?;
        return Ok(Some(summary));
    }
    // Fallback (pre-v0.2.1 files): decode the whole pyramid-meta, pull finest levels.
    let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
    let meta =
        PyramidMeta::decode(&mb).map_err(|_| FileError::Container("malformed pyramid meta"))?;
    if meta.level_rollups.is_empty() && meta.level_links.is_empty() {
        return Ok(None);
    }
    let classes = meta
        .level_rollups
        .iter()
        .max_by_key(|r| r.depth)
        .map(|r| r.classes.clone())
        .unwrap_or_default();
    let relations = meta
        .level_links
        .iter()
        .max_by_key(|l| l.depth)
        .map(|l| {
            l.links
                .iter()
                .map(|c| {
                    (
                        c.s_class.clone(),
                        c.predicate.clone(),
                        c.o_class.clone(),
                        c.count,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(Some((classes, relations)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::index::GraphIndexBuilder;

    #[test]
    fn read_coalesced_merges_within_gap_and_splits_beyond() {
        use crate::reader::{CountingReader, SliceReader};
        let bytes = vec![0u8; 4096];
        // Three 16-byte ranges: A..B gap = 32, B..C gap = 1024.
        let ranges = [
            ByteRange { offset: 0, len: 16 },
            ByteRange {
                offset: 48,
                len: 16,
            },
            ByteRange {
                offset: 1088,
                len: 16,
            },
        ];
        // Tight gap (16): nothing merges → one read per range.
        let r = CountingReader::new(SliceReader::new(&bytes));
        let out = read_coalesced(&r, &ranges, 16).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(r.requests(), 3);
        // Gap 64 merges A+B (gap 32) but not C (gap 1024) → two reads.
        let r = CountingReader::new(SliceReader::new(&bytes));
        read_coalesced(&r, &ranges, 64).unwrap();
        assert_eq!(r.requests(), 2);
        // Gap 4096 merges all three into one read, over-fetching the gaps.
        let r = CountingReader::new(SliceReader::new(&bytes));
        read_coalesced(&r, &ranges, 4096).unwrap();
        assert_eq!(r.requests(), 1);
    }

    fn build_image() -> Vec<u8> {
        let triples = [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();

        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let index = ib.build();

        let (meta, levels) = build_pyramid_meta(&dict, &triples_ids(&dict), DEFAULT_TILE_BUDGET);
        write_file(&dict, &index, false, &meta, levels)
    }

    fn triples_ids(dict: &Dictionary) -> Vec<(u32, u32, u32)> {
        [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ]
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
        .collect()
    }

    #[test]
    fn file_round_trips_header_and_counts() {
        let bytes = build_image();
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.header().quad_count, 3);
        assert!(rete.header().term_count >= 5);
        let expected_codec = writer_codec();
        assert_eq!(rete.header().dict_codec, expected_codec);
        assert_eq!(rete.header().block_codec, expected_codec);
        assert_eq!(&bytes[bytes.len() - 4..], &MAGIC); // footer marker
    }

    /// A pre-tiling (format v0.1) file must still open and query identically:
    /// the test reconstructs the old writer byte-for-byte (whole-section
    /// compression, one block per permutation, version byte 1) and runs the
    /// modern reader over it â€” both the in-memory and the routed paths.
    #[test]
    fn reads_v1_single_block_files() {
        let triples = [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<Triple> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();

        // The v0.1 writer: one TripleBlock per permutation, container-level
        // compression, version byte 1.
        let codec = writer_codec();
        let blocks: [Vec<u8>; 3] = [
            IndexPermutation::Spo,
            IndexPermutation::Pos,
            IndexPermutation::Osp,
        ]
        .map(|perm| {
            let mut b = crate::triples::TripleBlockBuilder::new();
            for &(s, p, o) in &ids {
                let t = match perm {
                    IndexPermutation::Spo => (s, p, o),
                    IndexPermutation::Pos => (p, o, s),
                    IndexPermutation::Osp => (o, s, p),
                };
                b.push(t);
            }
            b.build()
        });
        let raw_sections = dict.sections();
        let dict_container = encode_container(
            &[
                raw_sections[0].as_slice(),
                raw_sections[1].as_slice(),
                raw_sections[2].as_slice(),
                raw_sections[3].as_slice(),
            ],
            codec,
        );
        let index_container = encode_container(&[&blocks[0], &blocks[1], &blocks[2]], codec);
        let header = Header {
            version: crate::header::MIN_READ_VERSION,
            flags: 0,
            metadata_offset: HEADER_LEN as u64,
            metadata_len: 0,
            dictionary_offset: HEADER_LEN as u64,
            dictionary_len: dict_container.len() as u64,
            root_dir_offset: HEADER_LEN as u64 + dict_container.len() as u64,
            root_dir_len: index_container.len() as u64,
            pyramid_meta_offset: 0,
            pyramid_meta_len: 0,
            dict_codec: codec,
            block_codec: codec,
            pyramid_levels: 0,
            quad_count: ids.len() as u64,
            term_count: dict.term_count() as u64,
            content_hash: content_hash(&[&dict_container, &index_container]),
            named_graphs_offset: 0,
            named_graphs_len: 0,
            schema_meta_len: 0,
        };
        let mut v1 = Vec::new();
        v1.extend_from_slice(&header.to_bytes());
        v1.extend_from_slice(&dict_container);
        v1.extend_from_slice(&index_container);
        v1.extend_from_slice(&MAGIC);

        // In-memory open.
        let rete = Rete::open(&v1).unwrap();
        assert_eq!(rete.header().version, crate::header::MIN_READ_VERSION);
        assert_eq!(rete.query(Some("Alice"), Some("knows"), None).len(), 1);
        assert_eq!(rete.query(None, Some("knows"), None).len(), 2);
        assert_eq!(rete.query(None, None, None).len(), 3);

        // Ranged open + routed pattern read.
        use crate::reader::SliceReader;
        let reader = SliceReader::new(&v1);
        let ranged = Rete::open_ranged(&reader).unwrap();
        assert_eq!(ranged.query(None, Some("knows"), None).len(), 2);
        let routed = Rete::query_ranged(&reader, Some("Alice"), Some("knows"), None).unwrap();
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].2, "Bob");
    }

    /// A file whose index was built with a tiny tile budget (forcing many
    /// tiles per permutation) must round-trip through write/open and answer
    /// every query shape identically â€” through both the in-memory and the
    /// routed ranged read paths.
    #[test]
    fn multi_tile_file_round_trips_and_routes() {
        let triples: Vec<(String, String, String)> = (0..200)
            .map(|i| {
                (
                    format!("<http://ex/s/{i}>"),
                    format!("<http://ex/p/{}>", i % 5),
                    format!("<http://ex/o/{}>", i % 23),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(64);
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let index = ib.build();
        assert!(
            index.tile_sections()[0].len() > 3,
            "tiny budget must force many tiles"
        );
        let bytes = write_file(&dict, &index, false, &[], 0);

        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.header().version, crate::header::VERSION);
        assert_eq!(rete.query(None, None, None).len(), 200);
        assert_eq!(rete.query(Some("<http://ex/s/7>"), None, None).len(), 1);
        assert_eq!(
            rete.query(None, Some("<http://ex/p/3>"), None).len(),
            40,
            "predicate extent spans tiles"
        );
        assert_eq!(
            rete.query(None, None, Some("<http://ex/o/22>")).len(),
            8 // 22, 45, 68, ... < 200
        );

        // Routed ranged read must agree (and only decompress matching tiles).
        use crate::reader::SliceReader;
        let reader = SliceReader::new(&bytes);
        let routed = Rete::query_ranged(&reader, Some("<http://ex/s/7>"), None, None).unwrap();
        assert_eq!(routed.len(), 1);
        let routed = Rete::query_ranged(&reader, None, Some("<http://ex/p/3>"), None).unwrap();
        assert_eq!(routed.len(), 40);
        let routed = Rete::query_ranged(&reader, None, None, Some("<http://ex/o/22>")).unwrap();
        assert_eq!(routed.len(), 8);
    }

    /// A dictionary big enough to split into multiple chunks per section must
    /// round-trip every idâ†”term mapping through the chunked (v0.2) encoding â€”
    /// including terms at chunk boundaries and absent near-misses.
    #[test]
    fn multi_chunk_dictionary_round_trips() {
        let mut db = DictionaryBuilder::new();
        let term = |i: u32| format!("<http://example.org/some/long/prefix/entity/{i:06}>");
        for i in 0..6000u32 {
            db.observe(&term(i), "<http://ex/p>", &term(i + 1));
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for i in 0..6000u32 {
            ib.push(
                dict.encode(&term(i), "<http://ex/p>", &term(i + 1))
                    .unwrap(),
            );
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);
        let rete = Rete::open(&bytes).unwrap();
        let d = rete.dictionary();
        assert_eq!(d.term_count(), dict.term_count());
        for i in (0..6000).step_by(97).chain([0, 1, 5999, 6000]) {
            let t = term(i);
            let sid = dict.subject_id(&t);
            assert_eq!(d.subject_id(&t), sid, "subject_id({t})");
            if let Some(id) = sid {
                assert_eq!(d.subject_term(id).as_deref(), Some(t.as_str()));
            }
            let oid = dict.object_id(&t);
            assert_eq!(d.object_id(&t), oid, "object_id({t})");
        }
        assert_eq!(d.subject_id("<http://example.org/absent>"), None);
        assert_eq!(d.predicate_id("<http://ex/p>"), Some(1));
        assert_eq!(d.predicate_term(1).as_deref(), Some("<http://ex/p>"));
    }

    #[test]
    #[cfg(feature = "compression")]
    fn compression_shrinks_repetitive_data() {
        // Many triples sharing IRI prefixes â€” exactly what front-coding + zstd
        // should crush. The compressed file must be far smaller than the raw
        // term bytes, and still query correctly.
        let mut db = DictionaryBuilder::new();
        let triples: Vec<(String, String, String)> = (0..500)
            .map(|i| {
                (
                    format!("<http://example.org/entity/{i}>"),
                    "<http://example.org/p/relatedTo>".to_string(),
                    format!("<http://example.org/entity/{}>", (i + 1) % 500),
                )
            })
            .collect();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        let raw: usize = triples
            .iter()
            .map(|(s, p, o)| s.len() + p.len() + o.len())
            .sum();
        assert!(
            bytes.len() < raw / 2,
            "expected strong compression: file {} vs raw terms {raw}",
            bytes.len()
        );

        // Still queryable after compression.
        let rete = Rete::open(&bytes).unwrap();
        let r = rete.query(Some("<http://example.org/entity/0>"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].2, "<http://example.org/entity/1>");
    }

    fn big_file_with_pyramid() -> Vec<u8> {
        // A ring of 300 entities -> index dwarfs dict+meta.
        let triples: Vec<(String, String, String)> = (0..300)
            .map(|i| {
                (
                    format!("<http://ex/e{i}>"),
                    "<http://ex/next>".to_string(),
                    format!("<http://ex/e{}>", (i + 1) % 300),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        write_file(&dict, &ib.build(), false, &meta, levels)
    }

    #[test]
    fn ranged_open_is_minimal_and_correct() {
        use crate::reader::{CountingReader, SliceReader};
        let bytes = big_file_with_pyramid();

        let full = CountingReader::new(SliceReader::new(&bytes));
        let rete = Rete::open_ranged(&full).unwrap();
        // Full open touches at most 4 ranges (header, dict, index, meta).
        assert!(full.requests() <= 4, "requests = {}", full.requests());
        assert_eq!(
            rete.query(Some("<http://ex/e0>"), None, None)[0].2,
            "<http://ex/e1>"
        );

        // Summary-only open skips the index â†’ strictly fewer bytes than the file.
        let summ_reader = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&summ_reader).unwrap().unwrap();
        assert!(!view.summary.is_empty());
        assert!(
            summ_reader.bytes_read() < bytes.len() as u64,
            "summary read {} of {} bytes",
            summ_reader.bytes_read(),
            bytes.len()
        );
        // And fewer than a full open, since it never fetched the index.
        assert!(summ_reader.bytes_read() < full.bytes_read());
    }

    #[test]
    fn content_hash_is_set_and_verifies() {
        let bytes = build_image();
        let rete = Rete::open(&bytes).unwrap();
        assert_ne!(
            rete.header().content_hash,
            [0u8; 16],
            "hash must be populated"
        );
        assert!(verify(&bytes).unwrap(), "freshly built file verifies");

        // Same data builds an identical hash (deterministic).
        assert_eq!(
            Rete::open(&build_image()).unwrap().header().content_hash,
            rete.header().content_hash
        );

        // Corrupting a payload byte breaks verification.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 5; // inside payload, before footer magic
        tampered[last] ^= 0xff;
        assert!(!verify(&tampered).unwrap());
    }

    /// Build the standard 3-triple image with an opaque metadata payload.
    fn build_with_metadata(meta: &[u8]) -> Vec<u8> {
        let triples = [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let (pmeta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        write_dataset_with_metadata(&dict, &ib.build(), &[], false, &pmeta, levels, meta)
    }

    #[test]
    fn metadata_round_trips_and_shifts_offsets() {
        let card = br#"{"title":"My Dataset"}"#;
        let bytes = build_with_metadata(card);
        let rete = Rete::open(&bytes).unwrap();

        // The opaque payload reads back verbatim.
        assert_eq!(rete.metadata(), Some(card.as_slice()));
        let h = rete.header();
        assert_eq!(h.metadata_offset, HEADER_LEN as u64);
        assert_eq!(h.metadata_len, card.len() as u64);
        // The dictionary (and everything after it) shifted forward by the card.
        assert_eq!(h.dictionary_offset, HEADER_LEN as u64 + card.len() as u64);

        // The index still decodes correctly at its shifted offset.
        assert_eq!(
            rete.query(Some("Bob"), Some("knows"), Some("Carol")).len(),
            1
        );
        // The card is inside the content hash, so the file still verifies.
        assert!(verify(&bytes).unwrap());
    }

    #[test]
    fn empty_metadata_is_byte_identical_to_plain_writer() {
        // The `&[]` path must produce exactly the bytes of the metadata-free
        // writer for identical inputs â€” old files and outputs are unchanged.
        assert_eq!(
            build_with_metadata(&[]),
            build_image(),
            "empty-metadata output must equal the plain writer byte-for-byte"
        );
    }

    #[test]
    fn metadata_is_tamper_evident() {
        let card = br#"{"title":"x"}"#;
        let mut bytes = build_with_metadata(card);
        assert!(verify(&bytes).unwrap());
        // The card occupies [HEADER_LEN .. HEADER_LEN+card_len); flip a byte in it.
        bytes[HEADER_LEN + 2] ^= 0xff;
        assert!(
            !verify(&bytes).unwrap(),
            "tampering with the card must break verify()"
        );
    }

    #[test]
    fn ranged_opens_do_not_fetch_metadata() {
        use crate::reader::{CountingReader, SliceReader};
        let card = vec![0xABu8; 512]; // distinctive and sizable
        let bytes = build_with_metadata(&card);
        let total = bytes.len() as u64;

        // A full ranged open never loads the card and never reads its byte range.
        let r = CountingReader::new(SliceReader::new(&bytes));
        let rete = Rete::open_ranged(&r).unwrap();
        assert!(
            rete.metadata().is_none(),
            "open_ranged must not load the card"
        );
        assert!(r.requests() <= 4, "requests = {}", r.requests());
        assert!(
            r.bytes_read() <= total - card.len() as u64,
            "read {} of {} bytes; the {}-byte card must be skipped",
            r.bytes_read(),
            total,
            card.len()
        );

        // Summary-only open likewise ignores the card and still summarizes.
        let rs = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&rs).unwrap().unwrap();
        assert!(!view.summary.is_empty());
        assert!(rs.bytes_read() <= total - card.len() as u64);
    }

    #[test]
    fn metadata_ranged_fetches_only_header_and_card() {
        use crate::reader::{CountingReader, SliceReader};
        // The CARD tier: fetch the self-description over a RangeReader touching
        // only the header + metadata range â€” never the dictionary/index/pyramid.
        let card = vec![0xCDu8; 384];
        let bytes = build_with_metadata(&card);

        let r = CountingReader::new(SliceReader::new(&bytes));
        let got = read_metadata_ranged(&r).unwrap().unwrap();
        assert_eq!(got, card, "the card reads back verbatim");
        assert_eq!(r.requests(), 2, "exactly header + metadata ranges");
        assert_eq!(
            r.bytes_read(),
            HEADER_LEN as u64 + card.len() as u64,
            "no dictionary/index/pyramid bytes are touched"
        );

        // A cardless file resolves to None after a single header read.
        let plain = build_image();
        let rp = CountingReader::new(SliceReader::new(&plain));
        assert!(read_metadata_ranged(&rp).unwrap().is_none());
        assert_eq!(rp.requests(), 1, "header only for a cardless file");
        assert_eq!(rp.bytes_read(), HEADER_LEN as u64);
    }

    #[test]
    fn schema_summary_groups_by_type() {
        let rt = RDF_TYPE;
        let bytes = build_from(&[
            ("Alice", rt, "Person"),
            ("Bob", rt, "Person"),
            ("NYC", rt, "City"),
            ("Alice", "knows", "Bob"),
            ("Alice", "livesIn", "NYC"),
            ("Alice", "name", "\"Alice\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let summary = schema_summary(&rete);
        // Expect class-level relations, rdf:type excluded.
        assert!(summary.contains(&("Person".into(), "knows".into(), "Person".into(), 1)));
        assert!(summary.contains(&("Person".into(), "livesIn".into(), "City".into(), 1)));
        assert!(summary.contains(&("Person".into(), "name".into(), "(literal)".into(), 1)));
        // No rdf:type relations in the summary.
        assert!(!summary.iter().any(|(_, p, _, _)| p == RDF_TYPE));

        // Class populations: 2 People, 1 City, sorted by count desc.
        let classes = schema_classes(&rete);
        assert_eq!(
            classes,
            vec![("Person".into(), 2u32), ("City".into(), 1u32)]
        );
    }

    fn build_from(triples: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        write_file(&dict, &ib.build(), false, &[], 0)
    }

    /// Build a file WITH a pyramid (so the schema pyramid + coherence axioms ship).
    fn build_with_pyramid(triples: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let encoded: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for t in &encoded {
            ib.push(*t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &encoded, DEFAULT_TILE_BUDGET);
        write_dataset(&dict, &ib.build(), &[], false, &meta, levels)
    }

    #[test]
    fn tbox_coherence_flags_unsatisfiable_class_index_free() {
        use crate::reader::{CountingReader, SliceReader};
        let rt = RDF_TYPE;
        let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let disj = "<http://www.w3.org/2002/07/owl#disjointWith>";
        // C ⊑ D, C ⊑ E, D disjointWith E ⇒ C unsatisfiable — schema-only, with one
        // instance present just so the schema pyramid gets built.
        let bytes = build_with_pyramid(&[
            ("<http://ex/C>", sub, "<http://ex/D>"),
            ("<http://ex/C>", sub, "<http://ex/E>"),
            ("<http://ex/D>", disj, "<http://ex/E>"),
            ("<http://ex/x>", rt, "<http://ex/C>"),
        ]);

        let r = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&r).unwrap().unwrap();
        let points = view.tbox_coherence();
        assert!(
            points
                .iter()
                .any(|i| i.kind == "unsatisfiable-class" && i.detail.contains("http://ex/C>")),
            "expected C unsatisfiable from the schema alone, got {points:?}"
        );

        // Proven index-free: bytes read never reach the (root_dir) index section,
        // mirroring `schema_pyramid_round_trips_through_file_index_free`.
        let header = Header::from_bytes(&bytes[..HEADER_LEN]).unwrap();
        assert!(
            r.bytes_read() <= bytes.len() as u64 - header.root_dir_len,
            "tbox_coherence must not read the triple index"
        );
    }

    #[test]
    fn schema_coherence_reads_only_the_schema_block() {
        use crate::reader::{CountingReader, SliceReader};
        let rt = RDF_TYPE;
        let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let disj = "<http://www.w3.org/2002/07/owl#disjointWith>";
        // 500 instances with unique literals → a sizable dictionary + community
        // summary, so a whole-pyramid-meta read would be large; the schema block
        // (bounded by the tiny ontology) stays small.
        let mut triples: Vec<(String, String, String)> = vec![
            ("<http://ex/C>".into(), sub.into(), "<http://ex/D>".into()),
            ("<http://ex/C>".into(), sub.into(), "<http://ex/E>".into()),
            ("<http://ex/D>".into(), disj.into(), "<http://ex/E>".into()),
        ];
        for i in 0..500 {
            let s = format!("<http://ex/x{i}>");
            triples.push((s.clone(), rt.into(), "<http://ex/C>".into()));
            triples.push((
                s,
                "<http://ex/label>".into(),
                format!("\"unique label {i}\""),
            ));
        }
        let trefs: Vec<(&str, &str, &str)> = triples
            .iter()
            .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
            .collect();
        let bytes = build_with_pyramid(&trefs);

        let header = Header::from_bytes(&bytes[..HEADER_LEN]).unwrap();
        assert!(
            header.schema_meta_len > 0,
            "the writer recorded a schema-block length"
        );
        assert!(
            (header.schema_meta_len as u64) < header.pyramid_meta_len,
            "schema block ({}) should be far smaller than the whole pyramid-meta ({})",
            header.schema_meta_len,
            header.pyramid_meta_len
        );

        let r = CountingReader::new(SliceReader::new(&bytes));
        let points = read_schema_coherence_ranged(&r).unwrap().unwrap();
        assert!(points.iter().any(|i| i.kind == "unsatisfiable-class"));
        // It read only the header + the schema block — not the summary or dictionary.
        assert!(
            r.bytes_read() <= HEADER_LEN as u64 + header.schema_meta_len as u64,
            "read {} bytes; expected <= header + schema block ({})",
            r.bytes_read(),
            HEADER_LEN as u64 + header.schema_meta_len as u64
        );
    }

    #[test]
    fn tbox_coherence_clean_schema_is_coherent() {
        use crate::reader::SliceReader;
        let rt = RDF_TYPE;
        let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let bytes = build_with_pyramid(&[
            ("<http://ex/Dog>", sub, "<http://ex/Animal>"),
            ("<http://ex/x>", rt, "<http://ex/Dog>"),
        ]);
        let view = SummaryView::open_ranged(&SliceReader::new(&bytes))
            .unwrap()
            .unwrap();
        assert!(view.tbox_is_coherent(), "a plain hierarchy is coherent");
    }

    #[test]
    fn named_graphs_round_trip() {
        // One shared dictionary; default graph + a named graph "g1".
        let all = [
            ("Alice", "knows", "Bob"), // default
            ("Bob", "age", "30"),      // named g1
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in all {
            db.observe(s, p, o);
        }
        let dict = db.build();

        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode("Alice", "knows", "Bob").unwrap());
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode("Bob", "age", "30").unwrap());

        let named = vec![("http://ex/g1".to_string(), g1.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);

        assert!(verify(&bytes).unwrap());
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.graph_names(), vec!["http://ex/g1"]);
        // The named graph contains Bob age 30, not the default-graph triple.
        let gi = rete.graph_index("http://ex/g1").unwrap();
        assert_eq!(gi.triple_count(), 1);
        assert!(rete.graph_index("http://ex/missing").is_none());

        // quad_count counts ALL quads — default graph + named graphs (1 + 1),
        // not just the default index (which would report 1).
        assert_eq!(rete.header().quad_count, 2);

        // Default-graph query path is unchanged.
        assert_eq!(rete.query(Some("Alice"), None, None).len(), 1);

        // dump() round-trips each graph back to terms.
        assert_eq!(
            rete.dump(None),
            vec![("Alice".into(), "knows".into(), "Bob".into())]
        );
        assert_eq!(
            rete.dump(Some("http://ex/g1")),
            vec![("Bob".into(), "age".into(), "30".into())]
        );
    }

    #[test]
    fn pyramid_meta_round_trips_in_file() {
        let rete = Rete::open(&build_image()).unwrap();
        let pyr = rete.pyramid().expect("file has a pyramid");
        // Summary covers all 3 triples by count; tiles are not stored in v0.
        let total: u32 = pyr.summary.iter().map(|e| e.count).sum();
        assert_eq!(total, 3);
        assert!(!pyr.summary.is_empty());
        assert!(pyr.tiles.is_empty());
    }

    #[test]
    fn schema_pyramid_round_trips_through_file_index_free() {
        use crate::reader::{CountingReader, SliceReader};
        let sub = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
        let q = |s: &str, p: &str, o: &str| {
            (s.to_string(), p.to_string(), o.to_string(), None::<String>)
        };
        // Astronomer ⊑ Scientist ⊑ Person ⊑ Agent, instances at the leaves.
        let quads = vec![
            q("<a>", RDF_TYPE, "<Astronomer>"),
            q("<b>", RDF_TYPE, "<Astronomer>"),
            q("<c>", RDF_TYPE, "<Person>"),
            q("<Astronomer>", sub, "<Scientist>"),
            q("<Scientist>", sub, "<Person>"),
            q("<Person>", sub, "<Agent>"),
            q("<a>", "<knows>", "<b>"),
            q("<b>", "<knows>", "<c>"),
        ];
        let (bytes, _) =
            crate::ingest::assemble_dataset_with_opts(&quads, true, None, |_| Vec::new());

        // The v2 schema pyramid round-trips through the built file.
        let rete = Rete::open(&bytes).unwrap();
        let pyr = rete.pyramid().expect("pyramid present");
        assert!(!pyr.level_rollups.is_empty(), "schema pyramid shipped");
        assert!(pyr
            .class_hierarchy
            .iter()
            .any(|n| n.class == "<Agent>" && n.depth == 0));

        // It reads index-free: a SummaryView open never touches the index section.
        let r = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&r).unwrap().unwrap();
        assert!(view.level_count() >= 2, "multi-level pyramid");
        let coarse = view.level_rollup(0).unwrap();
        assert!(
            coarse.classes.iter().any(|(c, _)| c == "<Agent>"),
            "coarsest level rolls up to the root Agent"
        );
        let h = Header::from_bytes(&bytes).unwrap();
        assert!(
            r.bytes_read() <= bytes.len() as u64 - h.root_dir_len,
            "summary read {} bytes; the {}-byte index section must be skipped",
            r.bytes_read(),
            h.root_dir_len
        );
    }

    #[test]
    fn predicate_totals_from_summary_only() {
        use crate::reader::SliceReader;
        // build_image: 2 `knows` triples + 1 `age` triple.
        let bytes = build_image();
        let reader = SliceReader::new(&bytes);
        let view = SummaryView::open_ranged(&reader).unwrap().unwrap();
        assert_eq!(view.predicate_total("knows"), 2);
        assert_eq!(view.predicate_total("age"), 1);
        assert_eq!(view.predicate_total("missing"), 0);
        let totals = view.predicate_totals();
        assert_eq!(totals[0], ("knows".to_string(), 2)); // sorted by count desc
    }

    #[test]
    fn query_patterns_resolve_to_terms() {
        let rete = Rete::open(&build_image()).unwrap();

        // All triples.
        assert_eq!(rete.query(None, None, None).len(), 3);

        // Subject bound.
        let mut alice = rete.query(Some("Alice"), None, None);
        alice.sort();
        assert_eq!(
            alice,
            vec![
                ("Alice".into(), "age".into(), "30".into()),
                ("Alice".into(), "knows".into(), "Bob".into()),
            ]
        );

        // Predicate bound.
        assert_eq!(rete.query(None, Some("knows"), None).len(), 2);

        // Full triple, present and absent.
        assert_eq!(
            rete.query(Some("Bob"), Some("knows"), Some("Carol")),
            vec![("Bob".into(), "knows".into(), "Carol".into())]
        );
        assert!(rete.query(Some("Nobody"), None, None).is_empty());
        assert!(rete.query(None, Some("likes"), None).is_empty());
    }

    #[test]
    fn query_provenance_reports_terms_ids_sections_and_index_choice() {
        let bytes = build_image();
        let rete = Rete::open(&bytes).unwrap();

        let mut matches = rete.query_with_provenance(None, Some("knows"), None);
        matches.sort_by(|a, b| a.terms.cmp(&b.terms));

        assert_eq!(matches.len(), 2);
        assert_eq!(
            matches[0].terms,
            ("Alice".into(), "knows".into(), "Bob".into())
        );
        assert_eq!(
            matches[0].ids,
            rete.dictionary().encode("Alice", "knows", "Bob").unwrap()
        );
        assert_eq!(matches[0].graph.as_deref(), None);
        assert_eq!(
            matches[0].matched_pattern,
            (None, Some(matches[0].ids.1), None)
        );
        assert_eq!(
            matches[0].index_permutation,
            crate::index::IndexPermutation::Pos
        );

        let h = rete.header();
        assert_eq!(matches[0].dictionary_range.offset, h.dictionary_offset);
        assert_eq!(matches[0].dictionary_range.len, h.dictionary_len);
        assert_eq!(matches[0].index_range.offset, h.root_dir_offset);
        assert_eq!(matches[0].index_range.len, h.root_dir_len);
        assert!(
            matches[0].index_section_range.offset > h.root_dir_offset,
            "POS is section 1, so its payload starts after the container header and SPO payload"
        );
        assert!(matches[0].index_section_range.len > 0);
        assert!(matches[0].index_section_range.end() <= matches[0].index_range.end());
        assert!(matches[0].index_section_range.len < matches[0].index_range.len);
        assert_eq!(
            matches[0].pyramid_range.as_ref().map(|r| (r.offset, r.len)),
            Some((h.pyramid_meta_offset, h.pyramid_meta_len))
        );
        // Tiled (v0.2) files report the physical tile holding the match; its
        // compressed byte range nests inside the selected section payload.
        let tile_range = matches[0].tile_range.expect("tiled file reports a tile");
        assert!(matches[0]
            .tile
            .as_deref()
            .unwrap()
            .starts_with(matches[0].index_permutation.name()));
        assert!(matches[0].index_section_range.offset <= tile_range.offset);
        assert!(tile_range.end() <= matches[0].index_section_range.end());
    }
}
