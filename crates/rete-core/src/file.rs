//! `.rete` file assembly and reading (SPEC.md Â§4, Â§9).
//!
//! v0 layout:
//!
//! ```text
//! [0..128)   header
//! [dict]     dictionary container: 4 front-coded sections
//! [index]    permutation container: 6 triple blocks (SPO/POS/OSP/SOP/PSO/OPS)
//! [pyramid]  summary meta (and, in future, tile directories)
//! [footer]   trailing magic
//! ```
//!
//! The header points at the dictionary container (`dictionary_offset/len`) and
//! the permutation container (`root_dir_offset/len`); routed readers can fetch a
//! single permutation payload from that container.

use crate::dictionary::Dictionary;
use crate::header::{
    Header, FLAG_HAS_QUADS, FLAG_HAS_QUOTED_TRIPLES, FLAG_TILE_SYNOPSIS, HEADER_LEN, MAGIC,
};
use crate::index::{GraphIndex, IndexPermutation, Pattern, PermSet, NUM_PERMS};
use crate::meta::{ClassNode, CommunityDescriptor, LevelLinks, LevelRollup, PyramidMeta};
use crate::pyramid::{build_dendrogram, project_graph, PyramidAlgo};
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
/// type predicate (e.g. `wdt:P31`) instead of auto-detection. Uses the default
/// [`PyramidAlgo::Louvain`] community algorithm — byte-identical to before.
pub fn build_pyramid_meta_with(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    budget: usize,
    type_override: Option<&str>,
) -> (Vec<u8>, u16) {
    build_pyramid_meta_algo(dict, triples, budget, type_override, PyramidAlgo::Louvain)
}

/// Like [`build_pyramid_meta_with`], but selects the community [`PyramidAlgo`].
/// [`PyramidAlgo::Types`] partitions by `rdf:type` — the deterministic,
/// parallelizable alternative to Louvain (one linear pass, no modularity) that
/// still emits the full summary + `query_stats`; it falls back to Louvain when the
/// graph has no usable typing. Everything downstream of the dendrogram (round
/// choice, summary, schema pyramid, planner stats) is shared across algorithms.
pub fn build_pyramid_meta_algo(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    budget: usize,
    type_override: Option<&str>,
    algo: PyramidAlgo,
) -> (Vec<u8>, u16) {
    // Optional sub-phase timing (set RETE_BUILD_TIMING=1) — the pyramid build is
    // the dominant cost of a big `rete build`; this shows where inside it.
    // `Instant::now()` must stay behind the flag: `std::time` is unsupported on
    // `wasm32-unknown-unknown` and panics ("time not implemented"), so an
    // unconditional clock read would break every in-browser `build()`.
    let timing = std::env::var_os("RETE_BUILD_TIMING").is_some();
    let mut t = timing.then(std::time::Instant::now);
    let mut lap = |label: &str| {
        if let Some(t0) = &mut t {
            eprintln!(
                "  [pyramid] {label}: {:.0} ms",
                t0.elapsed().as_secs_f64() * 1000.0
            );
            *t0 = std::time::Instant::now();
        }
    };

    // The community partition — the only step that differs by algorithm.
    let louvain = |lap: &mut dyn FnMut(&str)| {
        let g = project_graph(dict, triples);
        lap("project_graph");
        let d = build_dendrogram(&g);
        lap("build_dendrogram (Louvain)");
        d
    };
    let dend = match algo {
        PyramidAlgo::Louvain => louvain(&mut lap),
        PyramidAlgo::Types => {
            match crate::schema_pyramid::build_type_dendrogram(dict, triples, type_override) {
                Some(d) => {
                    lap("build_type_dendrogram");
                    d
                }
                None => {
                    eprintln!(
                        "  [pyramid] --pyramid-algo types: no usable rdf:type \
                         predicate — falling back to louvain"
                    );
                    louvain(&mut lap)
                }
            }
        }
    };
    let round = choose_round_for_budget(dict, triples, &dend, budget);
    lap("choose_round_for_budget");
    let summary = summarize(dict, triples, &dend, round);
    lap("summarize");
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
    lap("build_schema_pyramid");
    let predicate_stats = compute_predicate_stats(triples);
    lap("compute_predicate_stats");
    let char_sets = compute_char_sets(triples);
    lap("compute_char_sets");
    let label_index = compute_label_index(dict, triples);
    lap("compute_label_index");
    let meta = PyramidMeta::new(round as u32, summary, &[])
        .with_schema(
            sp.class_hierarchy,
            sp.level_rollups,
            sp.level_links,
            sp.descriptors,
            sp.subclass_cycles,
            sp.disjoint_pairs,
            sp.equivalent_pairs,
        )
        .with_predicate_stats(predicate_stats)
        .with_char_sets(char_sets)
        .with_label_index(label_index);
    let out = (meta.encode(), dend.rounds() as u16);
    lap("encode");
    out
}

/// The label predicates a [`compute_label_index`] entry can come from — the
/// common "human-readable name of this subject" terms, angle-bracketed as the
/// dictionary stores them. Order is irrelevant (we union their ids).
const LABEL_PREDICATES: &[&str] = &[
    "<http://www.w3.org/2000/01/rdf-schema#label>",
    "<http://www.w3.org/2004/02/skos/core#prefLabel>",
    "<http://www.w3.org/2004/02/skos/core#altLabel>",
    "<http://xmlns.com/foaf/0.1/name>",
    "<http://purl.org/dc/terms/title>",
    "<http://purl.org/dc/elements/1.1/title>",
    "<http://schema.org/name>",
];

/// Build the bounded **label index** for prefix search: the display labels of
/// the most-connected labeled subjects, sorted by the label's lowercased form.
/// Ranking keeps autocomplete useful on a huge graph (the prominent entities
/// survive the bound); a graph with fewer than `MAX_LABELS` labels keeps them
/// all. Deterministic (degree, then subject id, then label) so builds are
/// reproducible. O(triples) transient memory, freed before the file is written.
fn compute_label_index(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
) -> Vec<crate::meta::LabelEntry> {
    use crate::terms::{is_literal, literal_lexical};
    use std::collections::{HashMap, HashSet};
    const MAX_LABELS: usize = 8192;

    // Resolve the label predicates that actually occur in this graph.
    let label_pids: HashSet<u32> = LABEL_PREDICATES
        .iter()
        .filter_map(|p| dict.predicate_id(p))
        .collect();
    if label_pids.is_empty() {
        return Vec::new();
    }
    // Subject degree (triple count) — the ranking used to bound the index.
    let mut degree: HashMap<u32, u32> = HashMap::new();
    for &(s, _p, _o) in triples {
        *degree.entry(s).or_insert(0) += 1;
    }
    // Candidate (subject, label) pairs, deduped on (subject, lowercased label).
    let mut seen: HashSet<(u32, String)> = HashSet::new();
    let mut candidates: Vec<(u32, String, u32)> = Vec::new(); // (degree, label, subject)
    for &(s, p, o) in triples {
        if !label_pids.contains(&p) {
            continue;
        }
        let Some(term) = dict.object_term(o) else {
            continue;
        };
        if !is_literal(&term) {
            continue;
        }
        let Some(label) = literal_lexical(&term) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        if seen.insert((s, label.to_lowercase())) {
            candidates.push((*degree.get(&s).unwrap_or(&0), label, s));
        }
    }
    // Keep the most-connected entities when over budget: rank by degree desc,
    // then subject asc, then label asc (deterministic).
    if candidates.len() > MAX_LABELS {
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.2.cmp(&b.2))
                .then_with(|| a.1.cmp(&b.1))
        });
        candidates.truncate(MAX_LABELS);
    }
    // Final order: by lowercased label (search key), then label, then subject.
    candidates.sort_by(|a, b| {
        a.1.to_lowercase()
            .cmp(&b.1.to_lowercase())
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    candidates
        .into_iter()
        .map(|(_deg, label, subject)| crate::meta::LabelEntry { label, subject })
        .collect()
}

/// Build the full-text index section (`token → subjects`) over every
/// string-literal object: tokenize each literal into words and record the
/// subject that carries it. Empty when the graph has no literals. Opt-in
/// (`rete build --text-index`); O(literal bytes) transient. The token table is
/// compressed with [`writer_codec`] (the reader decompresses with `block_codec`).
pub(crate) fn compute_text_index(dict: &Dictionary, triples: &[(u32, u32, u32)]) -> Vec<u8> {
    use crate::terms::{is_literal, literal_lexical};
    let mut b = crate::text_index::TextIndexBuilder::new();
    for &(s, _p, o) in triples {
        let Some(term) = dict.object_term(o) else {
            continue;
        };
        if !is_literal(&term) {
            continue;
        }
        if let Some(lit) = literal_lexical(&term) {
            b.add_text(&lit, s);
        }
    }
    if b.is_empty() {
        Vec::new()
    } else {
        b.build(writer_codec())
    }
}

/// The top entity **shapes** (characteristic sets): group subjects by the exact
/// set of predicates they carry, keep the most common. Bounded to `MAX_CHAR_SETS`
/// and sorted deterministically (by subject count, then predicate list) so the
/// encoding is reproducible. O(triples) transient memory.
fn compute_char_sets(triples: &[(u32, u32, u32)]) -> Vec<crate::meta::CharSet> {
    use std::collections::{BTreeSet, HashMap};
    const MAX_CHAR_SETS: usize = 128;
    let mut by_subject: HashMap<u32, BTreeSet<u32>> = HashMap::new();
    for &(s, p, _o) in triples {
        by_subject.entry(s).or_default().insert(p);
    }
    let mut shapes: HashMap<Vec<u32>, u64> = HashMap::new();
    for set in by_subject.into_values() {
        *shapes.entry(set.into_iter().collect()).or_insert(0) += 1;
    }
    let mut v: Vec<crate::meta::CharSet> = shapes
        .into_iter()
        .map(|(predicates, subjects)| crate::meta::CharSet {
            predicates,
            subjects,
        })
        .collect();
    v.sort_by(|a, b| {
        b.subjects
            .cmp(&a.subjects)
            .then_with(|| a.predicates.cmp(&b.predicates))
    });
    v.truncate(MAX_CHAR_SETS);
    v
}

/// Per-predicate cardinality for the cost-based planner, in one pass over the
/// triples (deduped, so a per-(subject,predicate) count is its distinct-object
/// count). Returned sorted by predicate id for a reproducible encoding. Holds a
/// transient `(subject -> count, object -> count)` map per predicate — O(triples)
/// memory, freed before the file is written.
fn compute_predicate_stats(triples: &[(u32, u32, u32)]) -> Vec<crate::meta::PredStat> {
    use std::collections::HashMap;
    #[allow(clippy::type_complexity)]
    let mut acc: HashMap<u32, (HashMap<u32, u32>, HashMap<u32, u32>, u64)> = HashMap::new();
    for &(s, p, o) in triples {
        let e = acc.entry(p).or_default();
        *e.0.entry(s).or_insert(0) += 1;
        *e.1.entry(o).or_insert(0) += 1;
        e.2 += 1;
    }
    let mut stats: Vec<crate::meta::PredStat> = acc
        .into_iter()
        .map(|(predicate, (subj, obj, count))| crate::meta::PredStat {
            predicate,
            count,
            distinct_subjects: subj.len() as u64,
            distinct_objects: obj.len() as u64,
            max_objects_per_subject: subj.values().copied().max().unwrap_or(0),
            max_subjects_per_object: obj.values().copied().max().unwrap_or(0),
        })
        .collect();
    stats.sort_by_key(|p| p.predicate);
    stats
}

/// No compression.
pub const CODEC_NONE: u8 = 0;
/// zstd compression (per section).
pub const CODEC_ZSTD: u8 = 1;
/// zstd compression level used by the writer.
#[cfg(feature = "compression")]
const ZSTD_LEVEL: i32 = 9;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
pub(crate) fn writer_codec() -> u8 {
    if cfg!(feature = "compression") {
        CODEC_ZSTD
    } else {
        CODEC_NONE
    }
}

/// Intersection of two ascending-sorted, deduped id lists — the AND of two
/// posting lists in a multi-word text search. Linear merge, output sorted.
fn intersect_sorted(a: &[u32], b: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(a.len().min(b.len()));
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                out.push(a[i]);
                i += 1;
                j += 1;
            }
        }
    }
    out
}

pub(crate) fn compress(codec: u8, bytes: &[u8]) -> Vec<u8> {
    match codec {
        #[cfg(feature = "compression")]
        CODEC_ZSTD => {
            zstd::encode_all(bytes, ZSTD_LEVEL).expect("zstd encode is infallible in-memory")
        }
        _ => bytes.to_vec(),
    }
}

pub(crate) fn decompress(codec: u8, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
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
    let body_start = meta
        .restart_offsets
        .first()
        .copied()
        .unwrap_or(raw.len() as u64);
    let header = &raw[..(body_start.min(raw.len() as u64)) as usize];

    // Split runs into chunks by body-byte budget (whole runs only).
    let n_runs = meta.restart_offsets.len();
    let mut bounds: Vec<(usize, u64, u64)> = Vec::new(); // (first_run, start, end)
    let mut r = 0;
    while r < n_runs {
        let start = meta.restart_offsets[r];
        let mut r2 = r + 1;
        while r2 < n_runs && meta.restart_offsets[r2] - start < DICT_CHUNK_BUDGET as u64 {
            r2 += 1;
        }
        let end = if r2 < n_runs {
            meta.restart_offsets[r2]
        } else {
            raw.len() as u64
        };
        bounds.push((r, start, end));
        r = r2;
    }

    let compressed: Vec<Vec<u8>> = bounds
        .iter()
        .map(|&(_, s, e)| compress(codec, &raw[s as usize..e as usize]))
        .collect();
    let mut out = Vec::new();
    write_uvarint(&mut out, header.len() as u64);
    out.extend_from_slice(header);
    write_uvarint(&mut out, bounds.len() as u64);
    let mut prev_run = 0usize;
    for (&(first_run, start, _), comp) in bounds.iter().zip(&compressed) {
        let first_term = crate::dict::run_first_term(raw, start as usize).unwrap_or_default();
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
    body_start: u64,
    start: u64,
    end: u64,
}

/// Parse a chunked dictionary section's header + directory (not the chunks).
/// `bytes` may be a prefix of the payload; compressed ranges validate against
/// `total_len`.
fn parse_chunked_dict_dir(
    bytes: &[u8],
    total_len: u64,
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
        let clen = take(&mut pos)?;
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
    let mut start = pos as u64;
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
/// Read a chunked dictionary section's directory over a range reader WITHOUT
/// materializing the section-wide restart table. That table is one offset per
/// restart run — a 50 M-term section has ~3 M of them (~24 MiB resident), and
/// holding it is an iOS-Safari OOM on a big remote file. We read only the tiny
/// header prefix (term_count / interval) and the chunk directory, skipping the
/// restart-table bytes entirely; per-run offsets are derived per chunk on fault
/// (`SectionChunk::run_offsets`). The returned meta has an empty
/// `restart_offsets`, which the chunked lookups read as "derive per chunk".
fn read_dict_dir_ranged<R: RangeReader>(
    reader: &R,
    section: ByteRange,
) -> Result<(crate::dict::SectionMeta, Vec<DictChunkEntry>), FileError> {
    let total = section.len;
    // Initial prefix: the header prefix ([header_len][term_count][interval]) and,
    // for a *small* section, the whole chunk directory too — so those still cost
    // a single read. A big section has a huge restart table between the header
    // and the directory; we detect that (dir_start past the prefix) and range-
    // read only the directory below, never fetching the table.
    let init = 8192.min(total); // never over-read past the section (a tiny/empty
                                // section holds only its header + a stub directory)
    let head = reader.read_at(section.offset, init)?;
    let (header_len, n0) =
        read_uvarint(&head).ok_or(FileError::Container("truncated dict header len"))?;
    let hbase = n0; // first byte of the header body
    let (term_count, n1) = read_uvarint(head.get(hbase..).unwrap_or(&[]))
        .ok_or(FileError::Container("truncated dict term_count"))?;
    let (restart_interval, _n2) = read_uvarint(head.get(hbase + n1..).unwrap_or(&[]))
        .ok_or(FileError::Container("truncated dict interval"))?;
    if restart_interval == 0 {
        return Err(FileError::Container("zero restart interval"));
    }
    // The chunk directory begins right after the header body — i.e. past the
    // `header_len` bytes, which include the restart table we never materialize.
    let dir_start = (hbase as u64)
        .checked_add(header_len)
        .filter(|&d| d <= total)
        .ok_or(FileError::Container("dict header overruns section"))?;
    let dir_total = total - dir_start;
    let meta = crate::dict::SectionMeta {
        term_count: term_count as u32,
        restart_interval: restart_interval as u32,
        restart_offsets: Vec::new(),
    };
    let finish = |mut entries: Vec<DictChunkEntry>| {
        for e in &mut entries {
            e.start += dir_start; // dir-relative → section-relative
            e.end += dir_start;
        }
        (meta.clone(), entries)
    };
    // Fast path: the directory already sits in the prefix we read (small section
    // — its restart table is tiny, so the ~few KiB over-read is negligible).
    // A short prefix is not wasted: those bytes seed the probe below.
    let mut have: Vec<u8> = Vec::new();
    if dir_start < head.len() as u64 {
        match parse_chunk_dir_only(&head[dir_start as usize..], dir_total)? {
            ChunkDirParse::Done(entries) => return Ok(finish(entries)),
            ChunkDirParse::Truncated { .. } => have = head[dir_start as usize..].to_vec(),
        }
    }
    // Big section: range-read the directory on its own, skipping the table.
    //
    // Nothing records the directory's byte length, so it has to be probed. Two
    // things keep that probe near ONE directory's worth of bytes. Each round
    // **appends** to the bytes already held instead of re-reading from the
    // start — the previous loop re-read the whole prefix every time, so a
    // 234 MB directory (epfl-infoscience's object section) cost 537 MB of range
    // reads to fetch. And a truncated parse reports how many entries fit in how
    // many bytes, which extrapolates the rest instead of blindly doubling. The
    // extrapolation may only quadruple a round, so a wild guess on a directory
    // with wildly uneven entries (one stored term can be hundreds of KB) can
    // never fetch the section body.
    let mut want = 4096u64.min(dir_total).max(1);
    loop {
        let held = have.len() as u64;
        if want > held {
            let extra = reader.read_at(section.offset + dir_start + held, want - held)?;
            if extra.is_empty() {
                return Err(FileError::Container("truncated dict chunk directory"));
            }
            have.extend_from_slice(&extra);
        }
        let held = have.len() as u64;
        match parse_chunk_dir_only(&have, dir_total)? {
            ChunkDirParse::Done(entries) => return Ok(finish(entries)),
            ChunkDirParse::Truncated { .. } if held >= dir_total => {
                return Err(FileError::Container("truncated dict chunk directory"));
            }
            ChunkDirParse::Truncated {
                parsed,
                used,
                total,
            } => {
                // Extrapolate from a sample big enough to mean something;
                // otherwise just double. Undershooting only costs another
                // (append-only) round, so the estimate carries a small margin
                // rather than a generous one.
                let est = if parsed >= 16 && used > 0 {
                    let whole = ((used as u64) / (parsed as u64)).saturating_mul(total as u64);
                    whole.saturating_add(whole / 8).saturating_add(64)
                } else {
                    // Too small a sample to extrapolate from: double.
                    held.saturating_mul(2)
                };
                want = est
                    .max(held.saturating_add(1))
                    .min(held.saturating_mul(4))
                    .min(dir_total);
            }
        }
    }
}

/// What [`parse_chunk_dir_only`] made of a chunk-directory prefix: either the
/// whole directory, or how far a truncated one got (the probe's step hint).
/// Truncation is not an error here — the caller is deliberately reading a
/// prefix and deciding how much more to fetch.
enum ChunkDirParse {
    Done(Vec<DictChunkEntry>),
    Truncated {
        /// Entries fully decoded from the prefix.
        parsed: usize,
        /// Bytes those entries occupied (directory-relative).
        used: usize,
        /// Chunk count the directory declares.
        total: usize,
    },
}

/// Parse just the chunk directory (the bytes after a section header):
/// `[num_chunks][per chunk: Δfirst_run, first_term_len, first_term, comp_len]`.
/// Chunk byte ranges (`start`/`end`) come back relative to the directory's own
/// start; `body_start` is 0 (a lite section never uses it — lookups derive run
/// offsets per chunk). Bodies aren't needed here, so `dir` may end at the first
/// body as long as it covers the whole directory. A `dir` that stops mid-entry
/// is reported as [`ChunkDirParse::Truncated`], not an error: only bytes that
/// cannot be a directory at any length (a chunk range overrunning the section)
/// fail.
fn parse_chunk_dir_only(dir: &[u8], dir_total: u64) -> Result<ChunkDirParse, FileError> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Option<u64> {
        let (v, n) = read_uvarint(dir.get(*pos..).unwrap_or(&[]))?;
        *pos += n;
        Some(v)
    };
    let Some(num_chunks) = take(&mut pos) else {
        return Ok(ChunkDirParse::Truncated {
            parsed: 0,
            used: 0,
            total: 0,
        });
    };
    let num_chunks = num_chunks as usize;
    let mut entries = Vec::with_capacity(num_chunks.min(dir.len()));
    let mut lens: Vec<u64> = Vec::with_capacity(num_chunks.min(dir.len()));
    let mut prev_run = 0usize;
    for _ in 0..num_chunks {
        let entry_start = pos;
        let short = ChunkDirParse::Truncated {
            parsed: entries.len(),
            used: entry_start,
            total: num_chunks,
        };
        let (Some(drun), Some(tlen)) = (take(&mut pos), take(&mut pos)) else {
            return Ok(short);
        };
        let (drun, tlen) = (drun as usize, tlen as usize);
        let Some(term) = dir.get(pos..pos.saturating_add(tlen)) else {
            return Ok(short);
        };
        let term = term.to_vec();
        pos += tlen;
        let Some(clen) = take(&mut pos) else {
            return Ok(short);
        };
        let first_run = prev_run + drun;
        entries.push(DictChunkEntry {
            first_run,
            first_term: term,
            body_start: 0,
            start: 0,
            end: 0,
        });
        lens.push(clen);
        prev_run = first_run;
    }
    let mut start = pos as u64;
    for (e, len) in entries.iter_mut().zip(lens) {
        let end = start
            .checked_add(len)
            .filter(|&e| e <= dir_total)
            .ok_or(FileError::Container("dict chunk overruns section"))?;
        e.start = start;
        e.end = end;
        start = end;
    }
    Ok(ChunkDirParse::Done(entries))
}

/// Decode one chunked dictionary section payload into a resident
/// [`crate::dict::ChunkedSection`] (chunks decompressed up front â€” the local
/// open path).
fn decode_chunked_dict_section(
    payload: &[u8],
    codec: u8,
) -> Result<crate::dict::ChunkedSection, FileError> {
    let (meta, entries) = parse_chunked_dict_dir(payload, payload.len() as u64)?;
    let chunks = entries
        .into_iter()
        .map(|e| {
            Ok(crate::dict::SectionChunk::resident(
                e.first_run,
                e.first_term,
                e.body_start,
                decompress(codec, &payload[e.start as usize..e.end as usize])?,
            ))
        })
        .collect::<Result<Vec<_>, FileError>>()?;
    Ok(crate::dict::ChunkedSection::from_parts(meta, chunks, None))
}

fn decode_dictionary_container(bytes: &[u8], codec: u8) -> Result<Dictionary, FileError> {
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
    Ok(Dictionary::from_chunked_sections(arr))
}

/// Encode one permutation's tiled section payload (format v0.2):
/// `[num_tiles][per tile: delta(min_a), max_a - min_a, compressed_len][tilesâ€¦]`,
/// each tile compressed independently with `codec` so a ranged reader can
/// fetch and decompress exactly the tiles a query routes to. The directory
/// itself is uncompressed (it must be readable before any tile).
fn encode_tiled_section(tiles: &[crate::index::Tile], codec: u8) -> Vec<u8> {
    // Per-tile compression is the bulk of serialization time on a large graph and
    // the tiles are independent, so compress them across all cores. `par_iter`
    // preserves order, so the output is byte-identical to the serial map.
    #[cfg(feature = "parallel")]
    let compressed: Vec<Vec<u8>> = {
        use rayon::prelude::*;
        tiles
            .par_iter()
            .map(|t| compress(codec, t.bytes()))
            .collect()
    };
    #[cfg(not(feature = "parallel"))]
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
    // Tile-synopsis trailer (FLAG_TILE_SYNOPSIS): per tile, the inclusive min/max
    // of the two non-leading columns `(min_b, span_b, min_c, span_c)`, derived
    // from each tile's zone map. Appended **after** the tile payloads so a reader
    // that predates the flag — which locates tiles by length and stops — never
    // reads it (backward-compatible). A reader honoring the flag reads it from the
    // section tail. On the (impossible for a built tile) parse failure, emit a
    // full range so nothing is ever wrongly pruned.
    for tile in tiles {
        let (min_b, max_b, min_c, max_c) = match crate::triples::TripleBlock::parse(tile.bytes()) {
            Ok(b) => {
                let z = b.zone();
                (z.min_b, z.max_b, z.min_c, z.max_c)
            }
            Err(_) => (0, u32::MAX, 0, u32::MAX),
        };
        write_uvarint(&mut out, min_b as u64);
        write_uvarint(&mut out, (max_b - min_b) as u64);
        write_uvarint(&mut out, min_c as u64);
        write_uvarint(&mut out, (max_c - min_c) as u64);
    }
    out
}

/// A parsed v0.2 tile-directory entry: leading-id range plus the tile's byte
/// range *within the section payload*.
struct TileDirEntry {
    min_a: u32,
    max_a: u32,
    start: u64,
    end: u64,
}

/// One tile's synopsis: inclusive min/max of the two non-leading columns.
type TileSynopsis = (u32, u32, u32, u32);

/// Parse the **tile-synopsis trailer** (when [`FLAG_TILE_SYNOPSIS`] is set): the
/// `num_tiles × (min_b, span_b, min_c, span_c)` records that follow the last tile
/// payload, starting at `trailer_start` within `payload`. Returns one synopsis per
/// tile, in directory order. `payload` may be just the trailer slice (remote) or
/// the whole section (local); `trailer_start` is the offset of the trailer within
/// it. A short/garbled trailer yields `None` (the caller keeps `None` synopses —
/// pruning simply doesn't fire, never a wrong result).
fn parse_tile_synopsis(
    payload: &[u8],
    trailer_start: usize,
    num_tiles: usize,
) -> Option<Vec<TileSynopsis>> {
    let mut pos = trailer_start;
    let take = |pos: &mut usize| -> Option<u32> {
        let (v, n) = read_uvarint(payload.get(*pos..)?)?;
        *pos += n;
        u32::try_from(v).ok()
    };
    let mut out = Vec::with_capacity(num_tiles.min(payload.len()));
    for _ in 0..num_tiles {
        let min_b = take(&mut pos)?;
        let max_b = min_b.checked_add(take(&mut pos)?)?;
        let min_c = take(&mut pos)?;
        let max_c = min_c.checked_add(take(&mut pos)?)?;
        out.push((min_b, max_b, min_c, max_c));
    }
    Some(out)
}

/// Parse a tiled section payload's directory (not the tiles). `bytes` may be a
/// **prefix** of the payload (a ranged reader fetches the directory before any
/// tile); tile byte ranges are validated against `total_len`, the full payload
/// length. Every length is untrusted.
fn parse_tile_directory(bytes: &[u8], total_len: u64) -> Result<Vec<TileDirEntry>, FileError> {
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
        let len = take(&mut pos)?;
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
    let mut start = pos as u64;
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
    let total = section.len;
    let mut prefetch = 4096u64.min(total);
    loop {
        let prefix = reader.read_at(section.offset, prefetch)?;
        match parse_tile_directory(&prefix, total) {
            Ok(dir) => return Ok(dir),
            Err(_) if prefetch < total => prefetch = prefetch.saturating_mul(2).min(total),
            Err(e) => return Err(e),
        }
    }
}

/// Fetch and parse a remote section's **tile-synopsis trailer** (only when the
/// header's [`FLAG_TILE_SYNOPSIS`] is set): one targeted range read of the bytes
/// past the last tile, parsed into one synopsis per tile (directory order). A
/// missing/short/garbled trailer degrades to all-`None` — pruning simply doesn't
/// fire, never a wrong result. The directory gives the trailer's start (the last
/// tile's end).
fn read_tile_synopsis_ranged<R: RangeReader>(
    reader: &R,
    section: ByteRange,
    dir: &[TileDirEntry],
) -> Vec<Option<TileSynopsis>> {
    let n = dir.len();
    let none = vec![None; n];
    let trailer_start = dir.iter().map(|e| e.end).max().unwrap_or(0);
    let total = section.len;
    if n == 0 || trailer_start >= total {
        return none; // no trailer bytes present
    }
    let trailer_len = total - trailer_start;
    let Ok(bytes) = reader.read_at(section.offset + trailer_start, trailer_len) else {
        return none;
    };
    match parse_tile_synopsis(&bytes, 0, n) {
        Some(v) => v.into_iter().map(Some).collect(),
        None => none,
    }
}

/// Per-tile absolute file ranges of each permutation section, for provenance.
/// A malformed directory yields an empty section (provenance degrades, queries
/// are unaffected).
fn tile_file_ranges(
    index_bytes: &[u8],
    container_offset: u64,
    section_ranges: &[ByteRange; NUM_PERMS],
) -> [Vec<(u32, u32, ByteRange)>; NUM_PERMS] {
    let mut out: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    for (section, range) in out.iter_mut().zip(section_ranges) {
        // A permutation the file does not carry has a zeroed range. Its
        // `offset - container_offset` underflows, and before this guard it
        // panicked the whole `open_ranged` path on the first lean file.
        if range.len == 0 || range.offset < container_offset {
            continue;
        }
        let start = (range.offset - container_offset) as usize;
        let Some(payload) = index_bytes.get(start..start + range.len as usize) else {
            continue;
        };
        if let Ok(dir) = parse_tile_directory(payload, payload.len() as u64) {
            *section = dir
                .into_iter()
                .map(|e| {
                    (
                        e.min_a,
                        e.max_a,
                        ByteRange {
                            offset: range.offset + e.start,
                            len: (e.end - e.start),
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
    parse_tile_directory(payload, payload.len() as u64)?
        .into_iter()
        .map(|e| {
            Ok((
                e.min_a,
                e.max_a,
                decompress(codec, &payload[e.start as usize..e.end as usize])?,
            ))
        })
        .collect()
}

/// Decode the index container: one raw tiled section payload per permutation
/// the file carries (`perms`, from the header — [`PermSet::ALL`] for every file
/// written before the mask existed), each tile compressed individually.
///
/// The section count is checked against `perms.len()`, so a file whose header
/// and container disagree is rejected rather than silently short-read. A reader
/// that does not know about lean files at all reaches the same conclusion by a
/// different route: it passes six, the container says three, and this errors —
/// which is why a lean file cannot be misread as an empty one.
fn decode_index_container(
    bytes: &[u8],
    codec: u8,
    perms: PermSet,
) -> Result<GraphIndex, FileError> {
    let mut isecs = decode_container(bytes, CODEC_NONE)?;
    if isecs.len() != perms.len() {
        return Err(FileError::Container(
            "index container section count does not match the header permutation mask",
        ));
    }
    let mut sections: [Vec<(u32, u32, Vec<u8>)>; NUM_PERMS] = Default::default();
    for (perm, sec) in perms.iter().zip(isecs.iter_mut()) {
        sections[perm.section_index()] = decode_tiled_section(sec, codec)?;
    }
    Ok(GraphIndex::from_tiles(sections, perms))
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
    perms: PermSet,
) -> Result<[ByteRange; NUM_PERMS], FileError> {
    let ranges = container_section_payload_ranges(bytes, container_offset, perms.len())?;
    let mut out = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
    for (perm, range) in perms.iter().zip(ranges) {
        out[perm.section_index()] = range;
    }
    Ok(out)
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

/// Open one index container (six tiled permutation sections) **lazily** over a
/// range reader: fetch each section's tile directory (and synopsis trailer,
/// when the file has one) but no tile payloads — tiles fault in on first scan.
/// Returns the index plus its section and tile file ranges (provenance).
///
/// This is the machinery `open_ranged_lazy` always used for the default
/// graph's container at `root_dir_offset`; a named graph's container is the
/// same format at a different offset, so the lazy named-graphs path opens a
/// LARGE graph through this too instead of decoding it resident.
#[allow(clippy::type_complexity)]
fn open_index_container_lazy(
    reader: &std::sync::Arc<dyn RangeReader + Send + Sync>,
    container: ByteRange,
    block_codec: u8,
    has_synopsis: bool,
    read_concurrency: usize,
    perms: PermSet,
) -> Result<
    (
        GraphIndex,
        [ByteRange; NUM_PERMS],
        [Vec<(u32, u32, ByteRange)>; NUM_PERMS],
    ),
    FileError,
> {
    let mut index_section_ranges = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
    let mut tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    #[allow(clippy::type_complexity)]
    let mut directories: [Vec<(u32, u32, Option<TileSynopsis>)>; NUM_PERMS] = Default::default();
    for (pos, perm) in perms.iter().enumerate() {
        let si = perm.section_index();
        let section = locate_container_section_ranged(
            reader,
            container.offset,
            container.len,
            pos,
            perms.len() as u64,
        )?;
        index_section_ranges[si] = section;
        let dir = read_tile_directory_ranged(reader, section)?;
        // Tile synopses (one extra small tail read per section) let a routed
        // scan prune a tile by a bound secondary component before faulting it.
        let syn = if has_synopsis {
            read_tile_synopsis_ranged(reader, section, &dir)
        } else {
            vec![None; dir.len()]
        };
        directories[si] = dir
            .iter()
            .zip(syn)
            .map(|(e, s)| (e.min_a, e.max_a, s))
            .collect();
        tile_ranges[si] = dir
            .into_iter()
            .map(|e| {
                (
                    e.min_a,
                    e.max_a,
                    ByteRange {
                        offset: section.offset + e.start,
                        len: (e.end - e.start),
                    },
                )
            })
            .collect();
    }

    // The loader fetches and decompresses one tile per call; the bulk
    // loader serves multi-tile scans by coalescing adjacent tile ranges
    // into single range reads (tiles are back-to-back in their section,
    // so a full-section scan is typically one request).
    let codec = block_codec;
    let loader_ranges = tile_ranges.clone();
    let loader_reader = reader.clone();
    let loader: crate::index::TileLoader = Box::new(move |si, ti| {
        let (_, _, range) = loader_ranges.get(si)?.get(ti)?;
        let bytes = loader_reader.read_at(range.offset, range.len).ok()?;
        decompress(codec, &bytes).ok()
    });
    let bulk_ranges = tile_ranges.clone();
    let bulk_reader = reader.clone();
    let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis| {
        let section = bulk_ranges.get(si)?;
        let want: Option<Vec<ByteRange>> = tis
            .iter()
            .map(|&ti| section.get(ti).map(|&(_, _, r)| r))
            .collect();
        let blobs = read_coalesced(bulk_reader.as_ref(), &want?, TILE_COALESCE_GAP)?;
        blobs.iter().map(|b| decompress(codec, b).ok()).collect()
    });
    let mut index =
        GraphIndex::from_remote_directories(directories, perms, loader).with_bulk_loader(bulk);
    // Per-tile encoded lengths (from the directory) feed the join planner's
    // fatness gates — free here, unavailable later without a fetch.
    index.set_tile_lens(std::array::from_fn(|si| {
        tile_ranges[si]
            .iter()
            .map(|&(_, _, r)| r.len.min(u32::MAX as u64) as u32)
            .collect()
    }));
    // The reader's fan-out widens the planner's remote probe budget: a
    // desktop/CLI reader overlapping 16 range reads probes far more cheaply
    // than a phone's serial sync-XHR path.
    index.set_read_concurrency(read_concurrency);
    Ok((index, index_section_ranges, tile_ranges))
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

/// Encode an index container (v0.2): one raw tiled section payload per
/// permutation the index carries, in [`crate::index::ALL_PERMS`] order, tiles
/// compressed individually with `codec`. A three-permutation index writes three
/// sections — not six with three empty, which would be indistinguishable from
/// an empty graph to a reader that does not check the header mask.
fn encode_index_container(index: &GraphIndex, codec: u8) -> Vec<u8> {
    let sections = index.tile_sections();
    let payloads: Vec<Vec<u8>> = index
        .perms()
        .iter()
        .map(|perm| encode_tiled_section(sections[perm.section_index()], codec))
        .collect();
    let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_slice()).collect();
    encode_container(&refs, CODEC_NONE)
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
    perms: PermSet,
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
        let index = decode_index_container(&bytes[pos..cend], codec, perms)?;
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
        &[],
    )
}

/// Serialize a dictionary to its on-file container bytes (4 front-coded, chunked
/// sections). Exposed so a low-RAM build can serialize **and drop** the live
/// `Dictionary` before building the permutation index — the index build works on
/// id-triples and never needs the dictionary.
pub(crate) fn encode_dict_container(dict: &Dictionary, codec: u8) -> Vec<u8> {
    let raw_sections = dict.sections();
    let dict_payloads: Vec<Vec<u8>> = raw_sections
        .iter()
        .map(|raw| encode_chunked_dict_section(raw, codec))
        .collect();
    encode_container(
        &[
            dict_payloads[0].as_slice(),
            dict_payloads[1].as_slice(),
            dict_payloads[2].as_slice(),
            dict_payloads[3].as_slice(),
        ],
        CODEC_NONE,
    )
}

/// Serialize a dataset with an opaque **metadata** payload occupying the file's
/// metadata section (the application layer defines its meaning — the CLI stores a
/// JSON Dataset Card there). The section sits immediately after the header and
/// before the dictionary, so `metadata_offset` stays at `HEADER_LEN` and every
/// downstream section shifts by `metadata.len()`. The payload is folded into the
/// `content_hash`, so `verify` covers it and it is tamper-evident.
///
/// Passing an empty `metadata` is byte-identical to [`write_dataset`]: the section
/// is omitted (`metadata_len = 0`, `dictionary_offset = HEADER_LEN`) and the hash
/// is computed over exactly the same parts (a zero-length hash update is a no-op).
#[allow(clippy::too_many_arguments)]
pub fn write_dataset_with_metadata(
    dict: &Dictionary,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
    metadata: &[u8],
    text_index: &[u8],
) -> Vec<u8> {
    let codec = writer_codec();
    let dict_container = encode_dict_container(dict, codec);
    write_dataset_from_parts(
        &dict_container,
        dict.term_count() as u64,
        default_index,
        named,
        has_quads,
        dict.has_quoted_triples(),
        pyramid_meta,
        pyramid_levels,
        metadata,
        text_index,
        codec,
    )
}

/// Assemble the final file image from an **already-serialized** dictionary
/// container (so the caller can drop the live `Dictionary` before calling this)
/// plus the permutation index and optional sections. The byte output is identical
/// to serializing the dictionary inline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn write_dataset_from_parts(
    dict_container: &[u8],
    term_count: u64,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    has_quoted_triples: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
    metadata: &[u8],
    text_index: &[u8],
    codec: u8,
) -> Vec<u8> {
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
    // Optional full-text index between the pyramid and the named graphs.
    let text_offset = pyr_offset + pyr_len;
    let text_len = text_index.len() as u64;
    let named_offset = text_offset + text_len;
    let named_len = if named.is_empty() {
        0
    } else {
        named_section.len() as u64
    };

    // Hash parts in physical order, with the metadata payload prepended when
    // present. Omitting it entirely (rather than hashing an empty slice) keeps the
    // no-metadata output's hash byte-identical to the pre-metadata writer.
    // `verify()` rebuilds this exact list from the header — any section added
    // here must be added there too (and covered by a tamper test).
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    if meta_section_len > 0 {
        parts.push(metadata);
    }
    parts.push(dict_container);
    parts.push(&index_container);
    parts.push(pyramid_meta);
    if text_len > 0 {
        parts.push(text_index);
    }
    if named_len > 0 {
        parts.push(&named_section);
    }

    // Length of the trailing schema-pyramid block (0 if none), so a reader can
    // fetch just that block for an index/dictionary/summary-free Tier-0 read.
    let schema_meta_len = crate::meta::schema_block_len(pyramid_meta);

    let header = Header {
        version: crate::header::CURRENT_FORMAT_VERSION,
        flags: FLAG_TILE_SYNOPSIS
            | if has_quads { FLAG_HAS_QUADS } else { 0 }
            | if has_quoted_triples {
                FLAG_HAS_QUOTED_TRIPLES
            } else {
                0
            },
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
        // Every container in the file — default graph and named graphs alike —
        // is written from an index built with the same set, so the mask is a
        // file-level fact and the default index is its source of truth.
        perms: default_index.perms(),
        quad_count: default_index.triple_count() as u64
            + named
                .iter()
                .map(|(_, idx)| idx.triple_count() as u64)
                .sum::<u64>(),
        term_count,
        content_hash: content_hash(&parts),
        named_graphs_offset: if named_len > 0 { named_offset } else { 0 },
        named_graphs_len: named_len,
        schema_meta_len,
        text_index_offset: if text_len > 0 { text_offset } else { 0 },
        text_index_len: text_len,
        build_info_offset: 0,
        build_info_len: 0,
        extra_sections: Vec::new(),
    };

    let mut out = Vec::with_capacity(
        HEADER_LEN
            + metadata.len()
            + dict_container.len()
            + index_container.len()
            + pyramid_meta.len()
            + text_index.len()
            + named_section.len()
            + MAGIC.len(),
    );
    out.extend_from_slice(&header.to_bytes());
    if meta_section_len > 0 {
        out.extend_from_slice(metadata);
    }
    out.extend_from_slice(dict_container);
    out.extend_from_slice(&index_container);
    out.extend_from_slice(pyramid_meta);
    if text_len > 0 {
        out.extend_from_slice(text_index);
    }
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

/// Fetch the metadata (Dataset Card) **and** build-info sections via a
/// [`RangeReader`] in the fewest requests: the 1 KiB header, then — because the
/// writer lays the build-info section immediately after the metadata — **one**
/// coalesced range covering both. A file with only one of the two costs the
/// same two requests; a file with neither costs just the header read. This
/// keeps "card + build conditions" within the CARD tier's 1 header + 1 range
/// budget instead of adding a third request.
///
/// Returns `(metadata, build_info)`, each `None` when absent.
#[allow(clippy::type_complexity)]
pub fn read_card_and_build_info_ranged<R: RangeReader>(
    reader: &R,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), FileError> {
    read_card_and_build_info_with_header(reader).map(|(_, m, b)| (m, b))
}

/// [`read_card_and_build_info_ranged`], also returning the parsed [`Header`] it
/// had to read anyway.
///
/// The header is not a bonus: it carries the content hash and the section
/// directory — including whether the file has a TEXT_INDEX section, which the
/// card does not state (see
/// [`read_text_index_token_table_len_ranged`]). A caller that wants both and
/// used the tuple-only form would issue a **second** 1 KiB request for bytes it
/// already had, turning the CARD tier's documented "one header + one coalesced
/// range" into three requests.
#[allow(clippy::type_complexity)]
pub fn read_card_and_build_info_with_header<R: RangeReader>(
    reader: &R,
) -> Result<(Header, Option<Vec<u8>>, Option<Vec<u8>>), FileError> {
    let head = reader.read_at(0, HEADER_LEN as u64)?;
    let header = Header::from_bytes(&head)?;
    let meta = (header.metadata_offset, header.metadata_len);
    let build = (header.build_info_offset, header.build_info_len);
    if meta.1 > 0 && build.1 > 0 && build.0 == meta.0 + meta.1 {
        // Adjacent (the layout this crate writes): one read spans both.
        let both = reader.read_at(meta.0, meta.1 + build.1)?;
        let (m, b) = both.split_at(meta.1 as usize);
        return Ok((header, Some(m.to_vec()), Some(b.to_vec())));
    }
    let fetch = |off: u64, len: u64| -> Result<Option<Vec<u8>>, FileError> {
        if len == 0 {
            return Ok(None);
        }
        Ok(Some(reader.read_at(off, len)?))
    };
    Ok((header, fetch(meta.0, meta.1)?, fetch(build.0, build.1)?))
}

/// The build-info section of a whole file image, or `None` if absent. The
/// payload is an opaque application-layer blob (the CLI stores build-conditions
/// JSON there); it is **not** covered by the content hash — see
/// [`attach_build_info`].
pub fn read_build_info(bytes: &[u8]) -> Result<Option<Vec<u8>>, FileError> {
    let header = Header::from_bytes(bytes)?;
    if header.build_info_len == 0 {
        return Ok(None);
    }
    let start = header.build_info_offset as usize;
    let end = start
        .checked_add(header.build_info_len as usize)
        .filter(|&e| e <= bytes.len())
        .ok_or(FileError::Container("build-info section overruns buffer"))?;
    Ok(Some(bytes[start..end].to_vec()))
}

/// Everything needed to splice a build-info section into a finished file
/// **without holding the file in memory**: the rewritten header, where the
/// section goes, and where the bytes that do not move resume.
///
/// See [`plan_build_info`] for what the fields mean together; the layout is
/// always `[header][metadata][build info][everything else, verbatim]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfoPlan {
    /// The header bytes to write at offset 0.
    pub header: [u8; HEADER_LEN],
    /// Offset the new section is written at — equivalently, the length of the
    /// unchanged `[header][metadata]` prefix.
    pub insert: u64,
    /// Offset in the **old** file where the bytes that do not move resume. The
    /// span `insert..tail_start` is the old build-info section, if any, and is
    /// the only part of the old file the new one drops.
    pub tail_start: u64,
    /// Length of the file this plan produces.
    pub new_len: u64,
}

/// Plan the splice of a build-info section of `info_len` bytes into a file
/// whose first [`HEADER_LEN`] bytes are `head` and whose total length is
/// `file_len`.
///
/// The section goes immediately after the metadata section — adjacent, so
/// [`read_card_and_build_info_ranged`] fetches card + build info in one
/// coalesced range — and every later section's offset shifts by the length
/// delta. That shift is the whole of the arithmetic, and it lives here so the
/// in-memory splice ([`attach_build_info`]) and any streaming rewriter derive
/// the same header from the same rule instead of each carrying a copy that can
/// drift.
///
/// The content hash is **deliberately left untouched**: build info records the
/// facts that differ between two builds of identical data (timestamp, builder,
/// measured costs), and folding them into the hash would break the
/// reproducible-hash property. `verify` accordingly ignores this section, on an
/// old reader (which sees an unknown kind-7 entry) and a new one alike — so a
/// file that gains, loses or changes a build record **keeps its content
/// identity**.
///
/// `info_len` of 0 plans the removal of an existing section.
pub fn plan_build_info(
    head: &[u8],
    file_len: u64,
    info_len: u64,
) -> Result<BuildInfoPlan, FileError> {
    let mut header = Header::from_bytes(head)?;
    // The insert point: immediately after the metadata section. The writers in
    // this crate always place metadata (when present) at HEADER_LEN.
    let insert = HEADER_LEN as u64 + header.metadata_len;
    let old_len = header.build_info_len;
    if old_len > 0 && header.build_info_offset != insert {
        return Err(FileError::Container(
            "existing build-info section is not adjacent to the metadata",
        ));
    }
    let tail_start = insert.saturating_add(old_len);
    if tail_start > file_len || insert > file_len {
        return Err(FileError::Container("build-info splice out of bounds"));
    }

    // Shift every section that lives at or after the old tail.
    let shift = |off: &mut u64, len: u64| {
        if len > 0 && *off >= tail_start {
            *off = *off - old_len + info_len;
        }
    };
    shift(&mut header.dictionary_offset, header.dictionary_len);
    shift(&mut header.root_dir_offset, header.root_dir_len);
    shift(&mut header.pyramid_meta_offset, header.pyramid_meta_len);
    shift(&mut header.named_graphs_offset, header.named_graphs_len);
    shift(&mut header.text_index_offset, header.text_index_len);
    for s in &mut header.extra_sections {
        if s.length > 0 && s.offset >= tail_start {
            s.offset = s.offset - old_len + info_len;
        }
    }
    if info_len == 0 {
        header.build_info_offset = 0;
        header.build_info_len = 0;
    } else {
        header.build_info_offset = insert;
        header.build_info_len = info_len;
    }

    Ok(BuildInfoPlan {
        header: header.to_bytes(),
        insert,
        tail_start,
        new_len: file_len - old_len + info_len,
    })
}

/// Attach (or replace) a **build-info** section in a finished file image,
/// returning the new image. Layout and hash semantics: [`plan_build_info`].
///
/// Passing an empty `info` removes an existing section (or returns the image
/// unchanged when there is none).
pub fn attach_build_info(image: &[u8], info: &[u8]) -> Result<Vec<u8>, FileError> {
    let plan = plan_build_info(image, image.len() as u64, info.len() as u64)?;
    let mut out = Vec::with_capacity(plan.new_len as usize);
    out.extend_from_slice(&plan.header);
    out.extend_from_slice(&image[HEADER_LEN..plan.insert as usize]);
    out.extend_from_slice(info);
    out.extend_from_slice(&image[plan.tail_start as usize..]);
    debug_assert_eq!(out.len() as u64, plan.new_len);
    Ok(out)
}

/// Replace the **metadata** section (the Dataset Card blob) of a finished file
/// image, returning the new image. The section is the first one after the
/// header, so every later section's offset shifts by the length delta.
///
/// Unlike [`attach_build_info`], the metadata payload **is** inside the content
/// hash — it is part of what the file says about itself — so the hash is
/// recomputed here. The result is byte-identical to what the writer would have
/// produced had it been handed this payload in the first place
/// (`write_dataset_with_metadata`): the section sits at `HEADER_LEN`, every
/// later offset is derived from its length, and the hash folds the same parts
/// in the same order.
///
/// Why splice rather than rebuild: the only caller that needs this is a build
/// that has already assembled the file and then learned something about it by
/// **running** its own starter queries against it. Re-deriving the whole image
/// to correct a metadata blob would mean re-parsing the input; a splice costs
/// one memcpy and one rehash of an image that is already resident.
///
/// Passing an empty payload removes the section.
pub fn replace_metadata(image: &[u8], metadata: &[u8]) -> Result<Vec<u8>, FileError> {
    let mut header = Header::from_bytes(image)?;
    // The writers in this crate always place metadata (when present) at
    // HEADER_LEN; anything else is a file this function must not reshape.
    let insert = HEADER_LEN as u64;
    if header.metadata_len > 0 && header.metadata_offset != insert {
        return Err(FileError::Container(
            "metadata section does not sit immediately after the header",
        ));
    }
    let old_len = header.metadata_len;
    let tail_start = (insert + old_len) as usize;
    if tail_start > image.len() {
        return Err(FileError::Container("metadata splice out of bounds"));
    }
    let new_len = metadata.len() as u64;

    let shift = |off: &mut u64, len: u64| {
        if len > 0 && *off >= insert + old_len {
            *off = *off - old_len + new_len;
        }
    };
    shift(&mut header.dictionary_offset, header.dictionary_len);
    shift(&mut header.root_dir_offset, header.root_dir_len);
    shift(&mut header.pyramid_meta_offset, header.pyramid_meta_len);
    shift(&mut header.named_graphs_offset, header.named_graphs_len);
    shift(&mut header.text_index_offset, header.text_index_len);
    shift(&mut header.build_info_offset, header.build_info_len);
    for s in &mut header.extra_sections {
        if s.length > 0 && s.offset >= insert + old_len {
            s.offset = s.offset - old_len + new_len;
        }
    }
    header.metadata_offset = insert;
    header.metadata_len = new_len;

    let mut out = Vec::with_capacity(image.len() - old_len as usize + metadata.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(metadata);
    out.extend_from_slice(&image[tail_start..]);
    // The payload changed, so the hash must: recompute it over the same part
    // list `verify` will rebuild from this very header, then re-serialize the
    // header carrying it.
    header.content_hash = hash_of_sections(&header, &out)?;
    out[..HEADER_LEN].copy_from_slice(&header.to_bytes());
    Ok(out)
}

/// The content hash of a file image, folded over exactly the parts the writer
/// hashed (see `write_dataset_from_parts`): the metadata payload when present,
/// then dict, index, pyramid-meta, and — when present — the text index and the
/// named graphs. [`verify`] compares this against the header; [`replace_metadata`]
/// writes it.
fn hash_of_sections(header: &Header, bytes: &[u8]) -> Result<[u8; 16], FileError> {
    let slice = |off: u64, len: u64| -> Result<&[u8], FileError> {
        // `off` and `len` come STRAIGHT FROM THE FILE HEADER, so a crafted or
        // corrupt .rete can set them near u64::MAX and make `off + len` overflow
        // — which panics in debug and wraps in release, BEFORE `.get()` gets the
        // chance to reject the range. Found by the weekly fuzz run
        // (fuzz_targets/open.rs, "attempt to add with overflow"). Saturating the
        // sum turns both cases into the same clean Container error.
        let end = off.saturating_add(len);
        bytes
            .get(off as usize..end as usize)
            .ok_or(FileError::Container("section overruns buffer"))
    };
    let d = slice(header.dictionary_offset, header.dictionary_len)?;
    let i = slice(header.root_dir_offset, header.root_dir_len)?;
    let m = if header.pyramid_meta_len > 0 {
        slice(header.pyramid_meta_offset, header.pyramid_meta_len)?
    } else {
        &[]
    };
    let mut parts: Vec<&[u8]> = Vec::with_capacity(6);
    if header.metadata_len > 0 {
        parts.push(slice(header.metadata_offset, header.metadata_len)?);
    }
    parts.push(d);
    parts.push(i);
    parts.push(m);
    if header.text_index_len > 0 {
        parts.push(slice(header.text_index_offset, header.text_index_len)?);
    }
    if header.named_graphs_len > 0 {
        parts.push(slice(header.named_graphs_offset, header.named_graphs_len)?);
    }
    Ok(content_hash(&parts))
}

/// Recompute the content hash from a file image and check it against the header
/// â€” detects corruption or truncation of the payload sections.
pub fn verify(bytes: &[u8]) -> Result<bool, FileError> {
    let header = Header::from_bytes(bytes)?;
    Ok(hash_of_sections(&header, bytes)? == header.content_hash)
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

/// Faults the text index in on first search. `None` = the file has none, or the
/// fetch/parse failed.
type TextIndexLoader = Box<dyn Fn() -> Option<crate::text_index::TextIndex> + Send + Sync>;

/// Answers the token table's byte length from the section's first ≤10 bytes —
/// the length varint alone, never the table it measures.
type TokenTableProbe = Box<dyn Fn() -> Option<u64> + Send + Sync>;

/// The TEXT_INDEX section, held either resident (eager opens decode the whole
/// thing) or deferred (the lazy remote open keeps only a loader that fetches the
/// token table on first search, then faults posting lists one at a time). SPARQL
/// never touches it, so the lazy remote path keeps its small range budget.
///
/// Both variants can also answer how long the section's leading **token table**
/// is without faulting it — the figure a caller needs to state what a first
/// search will cost, since `header.text_index_len` counts the postings blob too
/// and overstates it by several times on a literal-heavy graph.
enum TextIndexSlot {
    Resident {
        index: Option<crate::text_index::TextIndex>,
        /// Measured from the section bytes this open already held; `None` when
        /// the file carries no text index.
        token_table_len: Option<u64>,
    },
    Lazy {
        loader: TextIndexLoader,
        cell: std::sync::OnceLock<Option<crate::text_index::TextIndex>>,
        /// One ≤10-byte range read, memoized in `token_table_cell`.
        token_table: TokenTableProbe,
        token_table_cell: std::sync::OnceLock<Option<u64>>,
    },
}

impl TextIndexSlot {
    /// The parsed index, faulting it in (token table first) on the lazy path.
    fn index(&self) -> Option<&crate::text_index::TextIndex> {
        match self {
            TextIndexSlot::Resident { index, .. } => index.as_ref(),
            TextIndexSlot::Lazy { loader, cell, .. } => cell.get_or_init(loader).as_ref(),
        }
    }

    /// Byte length of the section's leading token table — see
    /// [`Rete::text_index_token_table_len`].
    fn token_table_len(&self) -> Option<u64> {
        match self {
            TextIndexSlot::Resident {
                token_table_len, ..
            } => *token_table_len,
            TextIndexSlot::Lazy {
                token_table,
                token_table_cell,
                ..
            } => *token_table_cell.get_or_init(token_table),
        }
    }
}

/// Decode a whole TEXT_INDEX section held in memory, measuring its token table
/// on the way past. `section` is `None` when the file carries no text index.
fn resident_text_index_slot(section: Option<&[u8]>, codec: u8) -> Result<TextIndexSlot, FileError> {
    let Some(section) = section else {
        return Ok(TextIndexSlot::Resident {
            index: None,
            token_table_len: None,
        });
    };
    Ok(TextIndexSlot::Resident {
        index: Some(
            crate::text_index::TextIndex::from_section(section, codec)
                .map_err(|_| FileError::Container("malformed text index"))?,
        ),
        token_table_len: crate::text_index::TextIndex::postings_base(section).map(|b| b as u64),
    })
}

/// Byte length of the TEXT_INDEX section's leading **token table**, from a
/// header a caller has already parsed — the prefix range a first
/// [`Rete::text_search`] fetches, and the honest figure to quote as its cost
/// (`header.text_index_len` counts the postings blob too and overstates it
/// several-fold). `None` when the file carries no text index, or its first
/// bytes could not be read.
///
/// Costs **one range read of ≤10 bytes** — the section's leading length varint
/// and nothing else, never the table it measures. This is the free-standing
/// companion to [`Rete::text_index_token_table_len`], for readers that hold a
/// [`Header`] and a [`RangeReader`] but never open the file (the CARD tier:
/// `rete card`, `rete card-url`).
pub fn read_text_index_token_table_len_ranged<R: RangeReader + ?Sized>(
    reader: &R,
    header: &Header,
) -> Option<u64> {
    if header.text_index_len == 0 {
        return None;
    }
    read_token_table_len(reader, header.text_index_offset, header.text_index_len)
}

/// Byte length of the TEXT_INDEX section's leading token table — its length
/// varint plus the compressed table, i.e. the prefix range a first search
/// fetches. Reads the section's first ≤10 bytes and nothing else.
fn read_token_table_len<R: RangeReader + ?Sized>(reader: &R, off: u64, len: u64) -> Option<u64> {
    let head = reader.read_at(off, 10u64.min(len)).ok()?;
    let (ttlen, n) = crate::varint::read_uvarint(&head)?;
    Some((n as u64 + ttlen).min(len))
}

/// A named graph's container decodes RESIDENT (one range read, one decode) up
/// to this size on the lazy path; a larger container opens as a remote-lazy
/// tile index instead, so one selective `GRAPH <g>` query over a multi-GB
/// graph faults only the tile directories plus the tiles it touches.
const NAMED_GRAPH_RESIDENT_MAX: u64 = 1 << 20; // 1 MiB

/// Entries per lazily-allocated named-graph slab. The outer table is sized by
/// the section's leading count varint, which is UNTRUSTED — slabbing keeps a
/// hostile count from ballooning the allocation (the outer table costs a few
/// bytes per claimed slab; entry storage only materializes when touched).
const NAMED_SLAB: usize = 1024;

/// First read of the named-graph walk, and the floor its ramp resets to. One
/// chunk covers a few dozen small-graph records, so a `LIMIT`ed `GRAPH ?g`
/// pays exactly one small read — the targeted-query win this size protects.
const NAMED_WALK_CHUNK: u64 = 64 * 1024;

/// Ceiling of the walk's geometric read ramp — the read size an exhaustive
/// walk converges to (and starts at, when the demand is known exhaustive).
///
/// Sized from measurement, not taste. Against the nkod reference file
/// (67.9 MB, 31,974 named graphs, 65.4 MB section) the fixed 64 KiB walk cost
/// 262 requests / ~16.5 s in Chromium where the old eager whole-section fetch
/// cost ~8.5 s — i.e. ~31 ms of per-request overhead on ~8 s of transfer
/// (~7.7 MB/s). At 8 MiB per read a chunk transfers in ~1 s, putting the
/// request overhead near 3% of total time; halving the cap to 4 MiB doubles
/// that overhead, doubling the cap to 16 MiB buys back under 2% while doubling
/// both the walk buffer a wasm32 session must hold resident and the worst-case
/// over-read past the point a query stops. 8 MiB is the knee.
const NAMED_WALK_CHUNK_MAX: u64 = 8 * 1024 * 1024;

/// One named graph, walked lazily out of the NAMED_GRAPHS section.
#[derive(Default)]
struct NamedEntry {
    /// `(iri, absolute byte range of the graph's index container)` — filled
    /// when the sequential directory walk reaches this slot.
    meta: std::sync::OnceLock<(String, ByteRange)>,
    /// The graph's permutation index, opened on first access. Boxed: most
    /// entries of a many-graph dataset are never touched by a given query.
    index: std::sync::OnceLock<Box<GraphIndex>>,
}

/// Sequential position of the lazy named-graphs walk. The section has no
/// random-access directory — each entry's offset is the previous entry's end —
/// so entries are parsed strictly in order and memoised.
#[derive(Default)]
struct NamedWalk {
    /// Next entry index to parse.
    next: usize,
    /// Absolute file offset of that entry's header; `0` = the leading count
    /// varint has not been consumed yet (offset 0 is the file header, never
    /// inside a section).
    pos: u64,
    /// Read buffer carried ACROSS walk calls: `buf` holds the section bytes at
    /// `[buf_off, buf_off + buf.len())`. Entry-by-entry iteration (the
    /// `GRAPH ?g` path advances one entry per call) parses ~30 small-graph
    /// headers per 64 KiB chunk instead of re-reading a chunk per entry.
    /// [`LazyNamedGraphs::open_graph`] also serves whole small containers out
    /// of it, so a bulk chunk's ride-along payloads are never re-fetched.
    buf: Vec<u8>,
    buf_off: u64,
    /// Current read size of the geometric ramp (`0` = not started; treated as
    /// [`NAMED_WALK_CHUNK`]). Doubles on every buffer refill up to
    /// [`NAMED_WALK_CHUNK_MAX`], so a walk that keeps going converges to bulk
    /// reads after fetching at most as many bytes as it already used (the
    /// doubling bound: wasted read-ahead ≤ useful bytes). Reset to the floor
    /// whenever an oversized container is parsed — see `big_seen`.
    chunk: u64,
    /// A container larger than [`NAMED_GRAPH_RESIDENT_MAX`] has been parsed.
    /// Such graphs open tile-lazily (their payload bytes are NOT wanted by the
    /// walk), so ride-along prefetch stops paying off: the ramp resets and the
    /// exhaustive-demand hint stops seeding full-size reads. Without this, a
    /// header walk over a file of multi-MB containers would fetch the payloads
    /// it exists to hop over.
    big_seen: bool,
}

/// The NAMED_GRAPHS section opened lazily over a [`RangeReader`]: nothing is
/// fetched at open. The section is a count varint followed by
/// `iri_len | iri | container_len | container` records, so the directory is
/// walked forward on demand — header bytes only, skipping container payloads —
/// and each touched graph's index is decoded (or opened tile-lazily) on first
/// access. A `LIMIT`ed `GRAPH ?g` query therefore reads a prefix of the
/// section; only a query that genuinely touches every graph (e.g. a full
/// `COUNT` over `GRAPH ?g`) walks it all — which is exactly what the eager
/// open used to fetch unconditionally.
///
/// **Failure contract** (same as lazy tiles/chunks): a failed read or a
/// malformed record sets a sticky flag and the accessor returns `None`;
/// nothing failed is ever memoised, so a later evaluation retries.
struct LazyNamedGraphs {
    reader: std::sync::Arc<dyn RangeReader + Send + Sync>,
    /// Absolute byte range of the NAMED_GRAPHS section.
    section: ByteRange,
    codec: u8,
    has_synopsis: bool,
    read_concurrency: usize,
    /// The file's permutation set — every graph container in a file carries
    /// the same one, so it comes from the header rather than per graph.
    perms: PermSet,
    /// `(count, slab table)` — set once the leading count varint is read.
    /// Slabs allocate on first touch; boxed slices never move, so `&` handed
    /// out to entries stay valid for `&self`'s lifetime with no unsafe.
    #[allow(clippy::type_complexity)]
    dir: std::sync::OnceLock<(usize, Box<[std::sync::OnceLock<Box<[NamedEntry]>>]>)>,
    walk: std::sync::Mutex<NamedWalk>,
    failed: std::sync::atomic::AtomicBool,
}

impl LazyNamedGraphs {
    fn new(
        reader: std::sync::Arc<dyn RangeReader + Send + Sync>,
        section: ByteRange,
        codec: u8,
        has_synopsis: bool,
        read_concurrency: usize,
        perms: PermSet,
    ) -> Self {
        LazyNamedGraphs {
            reader,
            section,
            codec,
            has_synopsis,
            read_concurrency,
            perms,
            dir: std::sync::OnceLock::new(),
            walk: std::sync::Mutex::new(NamedWalk::default()),
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Record a fetch/parse failure (checked by `index_incomplete`).
    fn fail(&self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// The directory scaffold, reading the leading count varint on first call.
    #[allow(clippy::type_complexity)]
    fn directory(&self) -> Option<&(usize, Box<[std::sync::OnceLock<Box<[NamedEntry]>>]>)> {
        if let Some(d) = self.dir.get() {
            return Some(d);
        }
        let end = self.section.offset.checked_add(self.section.len)?;
        let (n, used) = match read_uvarint_at(&self.reader, self.section.offset, end) {
            Ok(v) => v,
            Err(_) => {
                self.fail();
                return None;
            }
        };
        // Each record needs at least 2 bytes of framing (two length varints),
        // so a count beyond len/2 is malformed — refuse before sizing by it.
        if n > self.section.len / 2 {
            self.fail();
            return None;
        }
        let n = n as usize;
        let slabs = n.div_ceil(NAMED_SLAB);
        let table: Box<[std::sync::OnceLock<Box<[NamedEntry]>>]> =
            (0..slabs).map(|_| std::sync::OnceLock::new()).collect();
        let _ = self.dir.set((n, table));
        {
            // Idempotent under an init race: both racers computed the same
            // `used` from the same bytes.
            let mut w = self.walk.lock().unwrap();
            if w.pos == 0 {
                w.pos = self.section.offset + used;
            }
        }
        self.dir.get()
    }

    fn count(&self) -> usize {
        self.directory().map(|(n, _)| *n).unwrap_or(0)
    }

    /// The `i`-th entry's cell (allocating its slab on first touch).
    fn entry(&self, i: usize) -> Option<&NamedEntry> {
        let (n, table) = self.directory()?;
        if i >= *n {
            return None;
        }
        let slab = table[i / NAMED_SLAB].get_or_init(|| {
            let len = NAMED_SLAB.min(n - (i / NAMED_SLAB) * NAMED_SLAB);
            (0..len).map(|_| NamedEntry::default()).collect()
        });
        slab.get(i % NAMED_SLAB)
    }

    /// The next walk read's length, advancing the geometric ramp: at least
    /// `need` (a long IRI's header), at most the section remainder, and
    /// otherwise the current ramp value — which doubles per refill up to
    /// [`NAMED_WALK_CHUNK_MAX`]. The doubling keeps a stopping walk's wasted
    /// read-ahead bounded by the bytes it already used, while a walk that
    /// keeps going converges to bulk reads: the runtime safety net for
    /// exhaustive-looking queries the demand analysis could not prove.
    fn next_read_len(w: &mut NamedWalk, pos: u64, end: u64, need: u64) -> u64 {
        let cur = w.chunk.max(NAMED_WALK_CHUNK);
        w.chunk = (cur * 2).min(NAMED_WALK_CHUNK_MAX);
        cur.max(need).min(end - pos)
    }

    /// Walk entry headers forward until `entries[upto].meta` is set. Reads the
    /// section in chunks and hops over container payloads, so the walk fetches
    /// O(headers) — not O(section) — where containers are large; small
    /// containers ride along in the same chunk and cost nothing extra.
    ///
    /// `exhaustive` is the demand hint from the query engine: the caller has
    /// PROVEN it will visit and open every named graph (an unrestricted
    /// `GRAPH ?g` under a consumer that drains the pipeline), so the walk
    /// starts at full-size reads instead of ramping up to them — the whole
    /// section arrives in `section/8 MiB` requests instead of hundreds. The
    /// hint is per-call, never memoised: a later targeted query on the same
    /// handle walks small again. It also defers to the layout: once an
    /// oversized (tile-lazy) container is seen, ride-along bytes stop being
    /// useful and the hint no longer seeds bulk reads.
    fn ensure_meta(&self, upto: usize, exhaustive: bool) -> Option<()> {
        let target = self.entry(upto)?;
        if target.meta.get().is_some() {
            return Some(());
        }
        let end = self.section.offset.checked_add(self.section.len)?;
        let mut w = self.walk.lock().unwrap();
        if exhaustive && !w.big_seen {
            w.chunk = NAMED_WALK_CHUNK_MAX;
        }
        while w.next <= upto {
            let pos = w.pos;
            if pos >= end {
                // The count varint promised more entries than the bytes hold.
                self.fail();
                return None;
            }
            // Make sure the whole header (two varints + the IRI, ≤ 20+iri_len
            // bytes) is buffered; refetch a larger window when the IRI is long.
            let have =
                |b: &[u8], off: u64, need: u64| pos >= off && pos + need <= off + b.len() as u64;
            if !have(&w.buf, w.buf_off, 20.min(end - pos)) {
                let len = Self::next_read_len(&mut w, pos, end, 20.min(end - pos));
                w.buf = match self.reader.read_at(pos, len) {
                    Ok(b) => b,
                    Err(_) => {
                        self.fail();
                        return None;
                    }
                };
                w.buf_off = pos;
            }
            let rel = (pos - w.buf_off) as usize;
            let Some((ilen, u1)) = read_uvarint(&w.buf[rel..]) else {
                self.fail();
                return None;
            };
            let header_need = u1 as u64 + ilen + 10; // iri_len + iri + container_len varint
            if pos + u1 as u64 + ilen > end {
                self.fail(); // IRI overruns the section: malformed
                return None;
            }
            if !have(&w.buf, w.buf_off, header_need.min(end - pos)) {
                let len = Self::next_read_len(&mut w, pos, end, header_need);
                w.buf = match self.reader.read_at(pos, len) {
                    Ok(b) => b,
                    Err(_) => {
                        self.fail();
                        return None;
                    }
                };
                w.buf_off = pos;
            }
            let rel = (pos - w.buf_off) as usize;
            let istart = rel + u1;
            let iend = istart + ilen as usize;
            let iri = String::from_utf8_lossy(&w.buf[istart..iend]).into_owned();
            let Some((clen, u2)) = read_uvarint(&w.buf[iend..]) else {
                self.fail();
                return None;
            };
            let cstart = pos + u1 as u64 + ilen + u2 as u64;
            let cend = match cstart.checked_add(clen) {
                Some(e) if e <= end => e,
                _ => {
                    self.fail(); // container overruns the section: malformed
                    return None;
                }
            };
            if clen > NAMED_GRAPH_RESIDENT_MAX {
                // This graph opens tile-lazily: the walk exists to HOP its
                // payload, so big read-ahead here is pure over-fetch. Drop the
                // ramp back to the floor and stop the exhaustive hint from
                // re-seeding it (headers stay cheap on payload-heavy files).
                w.big_seen = true;
                w.chunk = NAMED_WALK_CHUNK;
            }
            let range = ByteRange {
                offset: cstart,
                len: clen,
            };
            if let Some(e) = self.entry(w.next) {
                let _ = e.meta.set((iri, range));
            }
            w.pos = cend;
            w.next += 1;
        }
        Some(())
    }

    /// The `i`-th graph's IRI — a header walk, never an index decode.
    /// `exhaustive`: see [`ensure_meta`](Self::ensure_meta).
    fn name_at(&self, i: usize, exhaustive: bool) -> Option<&str> {
        self.ensure_meta(i, exhaustive)?;
        self.entry(i)?.meta.get().map(|(iri, _)| iri.as_str())
    }

    /// The `i`-th graph as `(iri, index)`, opening the index on first access.
    /// `exhaustive`: see [`ensure_meta`](Self::ensure_meta).
    fn graph_at(&self, i: usize, exhaustive: bool) -> Option<(&str, &GraphIndex)> {
        self.ensure_meta(i, exhaustive)?;
        let e = self.entry(i)?;
        let (iri, range) = e.meta.get()?;
        if e.index.get().is_none() {
            let opened = self.open_graph(*range)?;
            let _ = e.index.set(Box::new(opened));
        }
        Some((iri.as_str(), e.index.get()?.as_ref()))
    }

    /// A small container's bytes: out of the walk buffer when the last chunk
    /// already carried them (bulk chunks always do — that is their point),
    /// else one targeted range read. Never called for tile-lazy containers.
    fn container_bytes(&self, range: ByteRange) -> Option<Vec<u8>> {
        {
            let w = self.walk.lock().unwrap();
            let buf_end = w.buf_off + w.buf.len() as u64;
            if range.offset >= w.buf_off && range.offset + range.len <= buf_end {
                let a = (range.offset - w.buf_off) as usize;
                return Some(w.buf[a..a + range.len as usize].to_vec());
            }
        }
        match self.reader.read_at(range.offset, range.len) {
            Ok(b) => Some(b),
            Err(_) => {
                self.fail();
                None
            }
        }
    }

    /// Open one graph's index container: small ones resident (one read),
    /// large ones as a remote-lazy tile index — the same machinery as the
    /// default graph, so a selective query over a huge named graph fetches
    /// only the tile directories plus the tiles it touches.
    fn open_graph(&self, range: ByteRange) -> Option<GraphIndex> {
        if range.len <= NAMED_GRAPH_RESIDENT_MAX {
            let bytes = self.container_bytes(range)?;
            match decode_index_container(&bytes, self.codec, self.perms) {
                Ok(g) => Some(g),
                Err(_) => {
                    self.fail();
                    None
                }
            }
        } else {
            match open_index_container_lazy(
                &self.reader,
                range,
                self.codec,
                self.has_synopsis,
                self.read_concurrency,
                self.perms,
            ) {
                Ok((g, _, _)) => Some(g),
                Err(_) => {
                    self.fail();
                    None
                }
            }
        }
    }

    /// Visit every already-opened graph index (never triggers a fetch).
    fn for_each_opened(&self, mut f: impl FnMut(&GraphIndex)) {
        if let Some((_, table)) = self.dir.get() {
            for slab in table.iter().filter_map(|s| s.get()) {
                for e in slab.iter() {
                    if let Some(g) = e.index.get() {
                        f(g);
                    }
                }
            }
        }
    }
}

/// The NAMED_GRAPHS section, held resident (in-memory and eager ranged opens —
/// unchanged) or walked lazily (the lazy ranged open, where fetching and
/// decoding every graph's index up front defeated remote laziness: 67 MB
/// fetched and ~32k indexes built before the first query on a many-graph file).
enum NamedGraphsSlot {
    Resident(Vec<(String, GraphIndex)>),
    Lazy(LazyNamedGraphs),
}

impl NamedGraphsSlot {
    fn count(&self) -> usize {
        match self {
            NamedGraphsSlot::Resident(v) => v.len(),
            NamedGraphsSlot::Lazy(l) => l.count(),
        }
    }

    /// `exhaustive` is the walk-demand hint (resident opens ignore it): see
    /// [`LazyNamedGraphs::ensure_meta`].
    fn name_at(&self, i: usize, exhaustive: bool) -> Option<&str> {
        match self {
            NamedGraphsSlot::Resident(v) => v.get(i).map(|(iri, _)| iri.as_str()),
            NamedGraphsSlot::Lazy(l) => l.name_at(i, exhaustive),
        }
    }

    fn graph_at(&self, i: usize, exhaustive: bool) -> Option<(&str, &GraphIndex)> {
        match self {
            NamedGraphsSlot::Resident(v) => v.get(i).map(|(iri, g)| (iri.as_str(), g)),
            NamedGraphsSlot::Lazy(l) => l.graph_at(i, exhaustive),
        }
    }

    fn find(&self, iri: &str) -> Option<&GraphIndex> {
        match self {
            NamedGraphsSlot::Resident(v) => v.iter().find(|(name, _)| name == iri).map(|(_, g)| g),
            NamedGraphsSlot::Lazy(l) => {
                // Walk headers only until the IRI matches; decode just that
                // graph's index. An absent IRI costs a full header walk — the
                // layout has no by-name directory. A point lookup is targeted
                // demand: never hint the walk exhaustive.
                for i in 0..l.count() {
                    if l.name_at(i, false)? == iri {
                        return l.graph_at(i, false).map(|(_, g)| g);
                    }
                }
                None
            }
        }
    }

    fn load_incomplete(&self) -> bool {
        match self {
            NamedGraphsSlot::Resident(v) => v.iter().any(|(_, g)| g.load_incomplete()),
            NamedGraphsSlot::Lazy(l) => {
                if l.failed.load(std::sync::atomic::Ordering::Relaxed) {
                    return true;
                }
                let mut bad = false;
                l.for_each_opened(|g| bad |= g.load_incomplete());
                bad
            }
        }
    }

    fn reset_load_failures(&self) {
        match self {
            NamedGraphsSlot::Resident(v) => {
                for (_, g) in v {
                    g.reset_load_failure();
                }
            }
            NamedGraphsSlot::Lazy(l) => {
                l.failed.store(false, std::sync::atomic::Ordering::Relaxed);
                l.for_each_opened(|g| g.reset_load_failure());
            }
        }
    }
}

/// A read-only, in-memory view over a `.rete` file image.
pub struct Rete {
    header: Header,
    dict: Dictionary,
    index: GraphIndex,
    index_section_ranges: [ByteRange; NUM_PERMS],
    /// Per-permutation tile directories as absolute file ranges
    /// (`(min_a, max_a, compressed-tile range)`), for provenance. Empty for
    /// pre-tiling (v0.1) files.
    tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS],
    pyramid: PyramidSlot,
    text_index: TextIndexSlot,
    named_graphs: NamedGraphsSlot,
    /// Raw bytes of the metadata section (empty if the file has none). The
    /// application layer decodes this (the CLI stores a JSON Dataset Card here).
    /// Only [`Rete::open`] populates it; [`Rete::open_ranged`] leaves it empty to
    /// preserve its minimal-fetch budget.
    metadata: Vec<u8>,
    /// Executes remote `SERVICE` blocks (SPARQL 1.1 federated query) — attached
    /// by the host via [`Rete::set_service_client`]; `None` means a non-SILENT
    /// `SERVICE` fails the query. Like the range readers, the engine never does
    /// I/O itself.
    service_client: Option<Box<dyn crate::service::ServiceClient>>,
    /// First failed non-SILENT `SERVICE` call of the current query. The row
    /// pipeline is infallible (the same contract as lazy tile fetches), so the
    /// failure is recorded here and taken by the top-level eval entry points.
    service_error: std::sync::Mutex<Option<String>>,
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
        )?;

        let index_bytes = region(header.root_dir_offset, header.root_dir_len)?;
        let index = decode_index_container(index_bytes, header.block_codec, header.perms)?;
        let index_section_ranges =
            decode_index_section_ranges(index_bytes, header.root_dir_offset, header.perms)?;

        let pyramid = PyramidSlot::Resident(if header.pyramid_meta_len > 0 {
            Some(
                PyramidMeta::decode(region(header.pyramid_meta_offset, header.pyramid_meta_len)?)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        });

        // The TEXT_INDEX section (opt-in `--text-index`); decode the whole thing
        // resident on a full-image open.
        let text_index = resident_text_index_slot(
            if header.text_index_len > 0 {
                Some(region(header.text_index_offset, header.text_index_len)?)
            } else {
                None
            },
            header.block_codec,
        )?;

        let named_graphs = NamedGraphsSlot::Resident(if header.named_graphs_len > 0 {
            decode_named_graphs(
                region(header.named_graphs_offset, header.named_graphs_len)?,
                header.block_codec,
                header.perms,
            )?
        } else {
            Vec::new()
        });

        let metadata = if header.metadata_len > 0 {
            region(header.metadata_offset, header.metadata_len)?.to_vec()
        } else {
            Vec::new()
        };

        let tile_ranges =
            tile_file_ranges(index_bytes, header.root_dir_offset, &index_section_ranges);
        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            text_index,
            named_graphs,
            metadata,
            service_client: None,
            service_error: std::sync::Mutex::new(None),
        })
    }

    /// Attach the client that executes `SERVICE <endpoint> { … }` blocks
    /// (SPARQL 1.1 federated query) against remote SPARQL endpoints. Without
    /// one, a non-SILENT `SERVICE` fails the query with a clear error and a
    /// `SERVICE SILENT` degrades to one empty solution, per the spec.
    pub fn set_service_client(&mut self, client: Box<dyn crate::service::ServiceClient>) {
        self.service_client = Some(client);
    }

    pub(crate) fn service_client(&self) -> Option<&dyn crate::service::ServiceClient> {
        self.service_client.as_deref()
    }

    /// Record a failed non-SILENT `SERVICE` call (first error wins).
    pub(crate) fn record_service_error(&self, msg: &str) {
        let mut e = self.service_error.lock().unwrap();
        if e.is_none() {
            *e = Some(msg.to_string());
        }
    }

    /// Take (and clear) the pending `SERVICE` failure — called by every
    /// top-level eval entry so it can never leak into a later query.
    pub(crate) fn take_service_error(&self) -> Option<String> {
        self.service_error.lock().unwrap().take()
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
        for (si, perm) in crate::index::ALL_PERMS.into_iter().enumerate() {
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
                format!("named graphs ({})", self.named_graphs.count()),
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

    /// Per-predicate planner statistics from the query-stats block — empty when
    /// the file has none or the pyramid isn't resident (the lazy path doesn't
    /// fault it just for stats). See [`crate::meta::PredStat`].
    pub fn predicate_stats(&self) -> &[crate::meta::PredStat] {
        self.pyramid_if_loaded()
            .map(|p| p.predicate_stats.as_slice())
            .unwrap_or(&[])
    }

    /// The entity shapes (characteristic sets) from the pyramid — empty when the
    /// file has none or the pyramid isn't resident. See [`crate::meta::CharSet`].
    pub fn char_sets(&self) -> &[crate::meta::CharSet] {
        self.pyramid_if_loaded()
            .map(|p| p.char_sets.as_slice())
            .unwrap_or(&[])
    }

    /// The label index from the pyramid — empty when the file has none or the
    /// pyramid isn't resident. See [`crate::meta::LabelEntry`].
    pub fn label_index(&self) -> &[crate::meta::LabelEntry] {
        self.pyramid_if_loaded()
            .map(|p| p.label_index.as_slice())
            .unwrap_or(&[])
    }

    /// Prefix-search the label index: the subjects whose label starts with
    /// `prefix` (case-insensitive), as `(label, subject_iri)`, capped at `limit`.
    /// Unlike the planner accessors, this **faults the pyramid** (where the index
    /// lives) on the lazy path — a prefix search is an explicit read, not a free
    /// estimate. Returns an empty vec when the file carries no label index.
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Vec<(String, String)> {
        let Some(pyr) = self.pyramid() else {
            return Vec::new();
        };
        pyr.prefix_search(prefix, limit)
            .into_iter()
            .filter_map(|e| {
                self.dict
                    .subject_term(e.subject)
                    .map(|iri| (e.label.clone(), iri))
            })
            .collect()
    }

    /// The full-text index (TEXT_INDEX section), faulting it in on first access
    /// on the lazy remote path. `None` when the file carries no text index.
    pub(crate) fn text_index(&self) -> Option<&crate::text_index::TextIndex> {
        self.text_index.index()
    }

    /// Whether this file carries a full-text (TEXT_INDEX) section, i.e. it was
    /// built with `--text-index`. Cheap — reads the header, never faults.
    pub fn has_text_index(&self) -> bool {
        self.header.text_index_len > 0
    }

    /// Byte length of the TEXT_INDEX section's leading **token table** — the
    /// length varint plus the compressed table, which is exactly what a first
    /// [`text_search`](Self::text_search) faults. `None` when the file carries
    /// no text index (or its head could not be read).
    ///
    /// This is the number to quote as the cost of a first search, NOT
    /// `header().text_index_len`: that counts the postings blob too, which is
    /// the bulk of the section and is only ever fetched one posting list at a
    /// time. On the published `epfl-infoscience.rete` the section is 195 MB and
    /// its token table 29 MB — a 6.5× difference.
    ///
    /// Costs nothing on a resident open (measured while the bytes were in hand);
    /// a lazy/ranged open pays one ≤10-byte range read, memoized, and still
    /// never faults the table itself.
    pub fn text_index_token_table_len(&self) -> Option<u64> {
        self.text_index.token_table_len()
    }

    /// Full-text search over the literals: subject IRIs that carry **every** word
    /// in `words` (whole-word, case-insensitive — AND semantics), optionally also
    /// requiring a word that **starts with** `prefix` (token-prefix). Results are
    /// ordered by subject id and capped at `limit` (0 = uncapped). Empty when the
    /// file has no text index or nothing matches.
    ///
    /// Like [`prefix_search`](Self::prefix_search), this **faults** the index on
    /// the lazy remote path — a search is an explicit read, and only the queried
    /// posting lists are fetched, not the whole index.
    pub fn text_search(&self, words: &[&str], prefix: Option<&str>, limit: usize) -> Vec<String> {
        text_search_in(self.text_index(), &self.dict, words, prefix, limit)
    }

    /// The default-graph permutation index.
    pub fn default_index(&self) -> &GraphIndex {
        &self.index
    }

    /// Resolve every triple of a graph (`None` = default graph) back to terms.
    pub fn dump(&self, graph: Option<&str>) -> Vec<TermTriple> {
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

    /// Stream every triple of a graph (`None` = default) to `f`, resolving terms
    /// a batch at a time — no full `Vec` materialization, so it is safe on graphs far
    /// larger than RAM. `rete export` uses this to serialize 100M+ triple files
    /// that `dump()` (which collects every term into a `Vec<String>`) would OOM on.
    ///
    /// On a lazy/ranged open the dictionary is **not** prefetched whole: each
    /// batch faults only the chunks its own terms live in. Dumping ONE named
    /// graph therefore costs that graph's terms, not the file's. Dumping
    /// *everything* still reads every dictionary byte and leaves every chunk
    /// resident — that is inherent, and it, not the prefetch, is what sets the
    /// peak of a full export. See the comment in the body for both measurements.
    pub fn dump_each<F: FnMut(&str, &str, &str)>(&self, graph: Option<&str>, mut f: F) {
        let index = match graph {
            None => Some(&self.index),
            Some(g) => self.graph_index(g),
        };
        let Some(index) = index else { return };
        let dict = &self.dict;
        // Resolve in fixed-size batches with ONE coalesced dictionary fault per
        // batch — the same `prefetch_terms` call `dump_batch` makes, for the
        // same reason.
        //
        // This used to be `Dictionary::prefetch_all` up front, on the argument
        // that a full dump reaches every term anyway so the bytes are owed
        // either way. That argument only holds for a dump of EVERYTHING, and
        // `dump_each` takes a graph. Measured on `cordis.rete` (801 MB, a
        // 417 MB dictionary, six named graphs), lazily opened:
        //
        //   dump_each(Some(fundingschemes))  417 MB read, 2510 MB peak RSS
        //                              →     173 MB read,  619 MB peak RSS
        //   dump_each(None) — the EMPTY default graph, which is the first thing
        //   `rete export` calls on a quads file:
        //                                    415 MB read, 2508 MB peak RSS
        //                              →       0 MB read,   95 MB peak RSS
        //
        // The 2.5 GB comes from asking for every chunk at once: the bulk loader
        // coalesces adjacent chunks into one span, so the whole dictionary
        // section is materialized three times over before the first triple —
        // the fetched span, the per-chunk copies sliced out of it, and the
        // decompressed bodies.
        //
        // Be clear about what this does NOT buy. A dump of the whole default
        // graph still faults every chunk, and chunks stay resident once
        // faulted, so `rete export` of a single-graph file is unchanged:
        // `figshare.rete` (222 MB, a 126 MB dictionary) peaks at 874 MB lazily
        // either way. The resident dictionary is the floor there, not this.
        const RESOLVE_BATCH: usize = 4096;
        let mut ids: Vec<(u32, u32, u32)> = Vec::with_capacity(RESOLVE_BATCH);
        let mut nodes: Vec<u32> = Vec::with_capacity(RESOLVE_BATCH * 2);
        let mut preds: Vec<u32> = Vec::with_capacity(RESOLVE_BATCH);
        let mut flush = |ids: &mut Vec<(u32, u32, u32)>, f: &mut F| {
            if ids.is_empty() {
                return;
            }
            nodes.clear();
            preds.clear();
            for &(s, p, o) in ids.iter() {
                nodes.push(dict.subject_node(s));
                nodes.push(dict.object_node(o));
                preds.push(p);
            }
            dict.prefetch_terms(&nodes, &preds);
            for &(s, p, o) in ids.iter() {
                if let (Some(s), Some(p), Some(o)) = (
                    dict.subject_term(s),
                    dict.predicate_term(p),
                    dict.object_term(o),
                ) {
                    f(&s, &p, &o);
                }
            }
            ids.clear();
        };
        for t in index.scan_iter((None, None, None)) {
            ids.push(t);
            if ids.len() == RESOLVE_BATCH {
                flush(&mut ids, &mut f);
            }
        }
        flush(&mut ids, &mut f);
    }

    /// The **pull** twin of [`Rete::dump_each`](Self::dump_each): every triple of a
    /// graph (`None` = default) as a lazy iterator of resolved terms.
    ///
    /// Same constant-memory scan — one triple decoded and resolved per `next()`,
    /// never a `Vec` of the whole graph — but the caller drives it, so it can
    /// stop early or be **suspended and resumed** across a foreign-function
    /// boundary. That is what the wasm/JS client's batched quad cursor needs:
    /// a callback cannot be paused mid-scan to hand control back to JavaScript,
    /// an iterator can.
    ///
    /// The dictionary is NOT prefetched: terms fault in as they are resolved, so
    /// stopping after a handful of triples costs a handful of dictionary reads
    /// rather than the whole dictionary — which on a lazy remote open of a large
    /// file would be gigabytes before the first triple, defeating the point of a
    /// pull API. [`dump_each`](Self::dump_each) does not prefetch it whole
    /// either; it batches the same per-term faults. Peak memory is
    /// O(faulted dictionary chunks + faulted index tiles), *not* O(triples).
    pub fn dump_iter(&self, graph: Option<&str>) -> impl Iterator<Item = TermTriple> + '_ {
        self.query_iter(graph, None, None, None)
    }

    /// [`dump_iter`](Self::dump_iter) generalized to a triple **pattern**: the
    /// lazy pull form of [`query_in_graph`](Self::query_in_graph). A bound term
    /// unknown to the dictionary, or an unknown graph IRI, yields nothing.
    ///
    /// Same constant-memory guarantee — one triple decoded and resolved per
    /// `next()`, never a `Vec` of every match — and the same absence of a
    /// dictionary prefetch, so stopping after a handful of matches costs a
    /// handful of dictionary reads. `query_in_graph` is the eager twin; prefer
    /// this one wherever the consumer can stop early (`ASK`, `LIMIT`, an RDF4J
    /// `getStatements` that is about to be sliced).
    pub fn query_iter(
        &self,
        graph: Option<&str>,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> impl Iterator<Item = TermTriple> + '_ {
        let index = match graph {
            None => Some(&self.index),
            Some(g) => self.graph_index(g),
        };
        index
            .zip(self.resolve_query_pattern(s, p, o))
            .into_iter()
            .flat_map(|(ix, pattern)| ix.scan_iter(pattern))
            .filter_map(move |(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
    }

    /// One bounded, **resumable** slice of a pattern's matches inside one graph
    /// (`None` = default): `(triples, next_cursor, done)`. Start at `cursor = 0`
    /// and feed the returned cursor back until `done`.
    ///
    /// This is [`query_in_graph`](Self::query_in_graph) made streamable across a
    /// boundary that cannot hold a Rust borrow — the wasm/JVM one. The engine
    /// keeps no iterator alive between calls; the entire resume state is the
    /// opaque `u64` (see [`GraphIndex::scan_batch`]), so a client's cursor
    /// survives being suspended, and a cursor that is abandoned leaks nothing
    /// but a `u64`.
    ///
    /// The dictionary is faulted **per batch**, not whole: taking ten matches
    /// off the front of a 9.8-billion-quad file reads ten matches' worth of
    /// terms. That is the difference between `LIMIT 1` answering in bounded time
    /// and it reading a 30 GB dictionary first.
    ///
    /// `max_quads` is a floor (batches end on an a-group boundary), and every
    /// call either returns at least one row or reports `done`.
    pub fn query_batch(
        &self,
        graph: Option<&str>,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
        cursor: u64,
        max_quads: usize,
    ) -> (Vec<TermTriple>, u64, bool) {
        let index = match graph {
            None => &self.index,
            // An unknown graph IRI is an empty scan, not an error — same as
            // `query_in_graph` and `dump_batch`.
            Some(g) => match self.graph_index(g) {
                Some(i) => i,
                None => return (Vec::new(), cursor, true),
            },
        };
        // A bound term the dictionary does not know can match nothing, and the
        // index is never touched.
        let Some(pattern) = self.resolve_query_pattern(s, p, o) else {
            return (Vec::new(), cursor, true);
        };
        let (ids, next, done) = index.scan_batch(pattern, cursor, max_quads);
        // One coalesced dictionary fault for THIS batch — the same call
        // `query_in_graph` and `dump_batch` make, and the reason a partial walk
        // costs in proportion to what it actually read.
        if !ids.is_empty() {
            let mut nodes = Vec::with_capacity(ids.len() * 2);
            let mut preds = Vec::with_capacity(ids.len());
            for &(s, p, o) in &ids {
                nodes.push(self.dict.subject_node(s));
                nodes.push(self.dict.object_node(o));
                preds.push(p);
            }
            self.dict.prefetch_terms(&nodes, &preds);
        }
        let triples = ids
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
            .collect();
        (triples, next, done)
    }

    /// Resolve one bounded slice of a graph's triples. Returns
    /// `(triples, next_cursor, done)`; start with `cursor = 0` and feed the
    /// returned cursor back until `done`.
    ///
    /// This is the PULL half of the dump, and it exists because [`Rete::dump_each`] is
    /// push-based: it drives the scan itself and hands each triple to a
    /// callback. A client that wants to pull (a Python generator, a JS async
    /// iterator) has to hold the scan's stack between calls — which means a
    /// thread (unavailable on Pyodide and in wasm) or a self-referential struct
    /// holding an iterator that borrows the `Rete` stored next to it, whose
    /// soundness rests on drop order plus an unstated promise that this crate's
    /// lazily-faulted caches stay write-once. Neither is worth it for a dump.
    ///
    /// Instead the scan is made RESUMABLE: SPO tiles are ordered by subject id,
    /// so "every triple of subject `sid`, for ascending `sid`" visits exactly the
    /// tiles a full scan visits, in the same order, and the whole resume state
    /// collapses to one `u32`. No thread, no `unsafe`, no borrow held across
    /// calls — and unlike `(offset, limit)` it is O(n) overall rather than
    /// O(n²/limit), because nothing is ever re-scanned.
    ///
    /// `max_quads` is the FLOOR of a batch, not a hard cut: a batch always ends
    /// on a subject boundary, so no subject is split across two calls. Work per
    /// call is bounded by a probe budget too — a sparse named graph whose few
    /// subjects are spread over a huge id space returns early with `done = false`
    /// rather than grinding through absent ids, and the caller just asks again.
    pub fn dump_batch(
        &self,
        graph: Option<&str>,
        cursor: u32,
        max_quads: usize,
    ) -> (Vec<TermTriple>, u32, bool) {
        /// The first subject id at or after `sid` that any SPO tile can hold, or
        /// `None` when `sid` is past the last tile. Tiles ascend by leading
        /// range, so this is a binary search; it returns `sid` itself when a tile
        /// already covers it (an ordinary "this subject has no triples" step).
        fn next_tile_start(tiles: &[crate::index::Tile], sid: u32) -> Option<u32> {
            let i = tiles.partition_point(|t| t.leading_range().1 < sid);
            tiles.get(i).map(|t| t.leading_range().0.max(sid))
        }

        let index = match graph {
            None => self.default_index(),
            // An unknown graph IRI is an empty dump, not an error — same as
            // `dump_each`.
            Some(g) => match self.graph_index(g) {
                Some(i) => i,
                None => return (Vec::new(), cursor, true),
            },
        };
        // Tile leading ranges live in the section directory, so both the span and
        // the gap jumps below are free on a lazy/remote open — no tile is fetched
        // to decide where to look.
        let tiles = index.tile_sections()[IndexPermutation::Spo.section_index()];
        let (Some(first), Some(last)) = (
            tiles.first().map(|t| t.leading_range().0),
            tiles.last().map(|t| t.leading_range().1),
        ) else {
            return (Vec::new(), cursor, true); // empty graph
        };

        let mut sid = cursor.max(first);
        let mut probes = max_quads.saturating_mul(4).max(1 << 16);
        let mut ids: Vec<(u32, u32, u32)> = Vec::new();
        let mut exhausted = false;
        while sid <= last && probes > 0 {
            probes -= 1;
            let hits = index.match_pattern((Some(sid), None, None));
            if hits.is_empty() {
                // No triples at this id. If it also falls in a hole BETWEEN
                // tiles, jump straight to the next tile's first id instead of
                // stepping through the gap one absent id at a time.
                match next_tile_start(tiles, sid) {
                    None => {
                        exhausted = true;
                        break;
                    }
                    Some(next) if next > sid => {
                        sid = next;
                        continue;
                    }
                    Some(_) => {}
                }
            } else {
                ids.extend(hits);
            }
            if sid == u32::MAX {
                exhausted = true;
                break;
            }
            sid += 1;
            if ids.len() >= max_quads {
                break;
            }
        }
        let done = exhausted || sid > last;

        // One coalesced dictionary fault for this batch, then resolve.
        // Deliberately NOT `Dictionary::prefetch_all` (what `dump_each` uses):
        // on a lazy open that pulls the ENTIRE dictionary — gigabytes on a big
        // file — before the first quad, so taking five quads off the front would
        // cost as much as a full dump. Per-batch prefetch keeps a partial walk
        // proportional to what it actually read.
        let dict = self.dictionary();
        if !ids.is_empty() {
            let mut nodes = Vec::with_capacity(ids.len() * 2);
            let mut preds = Vec::with_capacity(ids.len());
            for &(s, p, o) in &ids {
                nodes.push(dict.subject_node(s));
                nodes.push(dict.object_node(o));
                preds.push(p);
            }
            dict.prefetch_terms(&nodes, &preds);
        }
        let triples = ids
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    dict.subject_term(s)?,
                    dict.predicate_term(p)?,
                    dict.object_term(o)?,
                ))
            })
            .collect();
        (triples, sid, done)
    }

    /// How many named graphs this dataset has. On a lazy ranged open this
    /// reads only the section's leading count varint — never the graphs.
    pub fn named_graph_count(&self) -> usize {
        self.named_graphs.count()
    }

    /// The `i`-th named graph's IRI (stored order). On a lazy ranged open this
    /// walks entry HEADERS up to `i` — it never decodes a graph's index.
    pub fn named_graph_name_at(&self, i: usize) -> Option<&str> {
        self.named_graphs.name_at(i, false)
    }

    /// The `i`-th named graph as `(iri, index)`. On a lazy ranged open the
    /// index is fetched and decoded on first access and memoised; check
    /// [`index_incomplete`](Self::index_incomplete) after evaluating.
    pub fn named_graph_at(&self, i: usize) -> Option<(&str, &GraphIndex)> {
        self.named_graphs.graph_at(i, false)
    }

    /// [`named_graph_name_at`](Self::named_graph_name_at) under a declared
    /// demand: `exhaustive = true` tells a lazy walk the caller will visit
    /// every graph, so it may read the section in bulk chunks. Purely a fetch
    /// strategy — results are identical either way. Only the SPARQL evaluator
    /// sets it, from shapes where full consumption is provable.
    pub(crate) fn named_graph_name_at_demand(&self, i: usize, exhaustive: bool) -> Option<&str> {
        self.named_graphs.name_at(i, exhaustive)
    }

    /// [`named_graph_at`](Self::named_graph_at) under a declared demand: see
    /// [`named_graph_name_at_demand`](Self::named_graph_name_at_demand).
    pub(crate) fn named_graph_at_demand(
        &self,
        i: usize,
        exhaustive: bool,
    ) -> Option<(&str, &GraphIndex)> {
        self.named_graphs.graph_at(i, exhaustive)
    }

    /// IRIs of the named graphs in this dataset (the default graph is unnamed).
    /// On a lazy ranged open this walks the whole directory (headers only) —
    /// prefer [`named_graph_count`](Self::named_graph_count) when the number
    /// is all that's needed.
    pub fn graph_names(&self) -> Vec<&str> {
        (0..self.named_graphs.count())
            .filter_map(|i| self.named_graphs.name_at(i, false))
            .collect()
    }

    /// The permutation index of a named graph, or `None` if absent.
    pub fn graph_index(&self, iri: &str) -> Option<&GraphIndex> {
        self.named_graphs.find(iri)
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
        let dict = decode_dictionary_container(&dict_bytes, header.dict_codec)?;

        let index_bytes = reader.read_at(header.root_dir_offset, header.root_dir_len)?;
        let index = decode_index_container(&index_bytes, header.block_codec, header.perms)?;
        let index_section_ranges =
            decode_index_section_ranges(&index_bytes, header.root_dir_offset, header.perms)?;

        let pyramid = PyramidSlot::Resident(if header.pyramid_meta_len > 0 {
            let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
            Some(
                PyramidMeta::decode(&mb)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        });

        // Fetch the whole TEXT_INDEX section resident (this opener does one range
        // read per section; the lazy opener below is the one that defers it).
        let text_index_bytes = if header.text_index_len > 0 {
            Some(reader.read_at(header.text_index_offset, header.text_index_len)?)
        } else {
            None
        };
        let text_index = resident_text_index_slot(text_index_bytes.as_deref(), header.block_codec)?;

        let named_graphs = NamedGraphsSlot::Resident(if header.named_graphs_len > 0 {
            let nb = reader.read_at(header.named_graphs_offset, header.named_graphs_len)?;
            decode_named_graphs(&nb, header.block_codec, header.perms)?
        } else {
            Vec::new()
        });

        // The metadata section (Dataset Card) is deliberately NOT fetched here:
        // a ranged query open keeps to its small range budget. Use `Rete::open`
        // (or a dedicated card fetch) when the card is actually needed.
        let tile_ranges =
            tile_file_ranges(&index_bytes, header.root_dir_offset, &index_section_ranges);
        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            text_index,
            named_graphs,
            metadata: Vec::new(),
            service_client: None,
            service_error: std::sync::Mutex::new(None),
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
    pub fn open_ranged_lazy<R: RangeReader + Send + Sync + 'static>(
        reader: R,
    ) -> Result<Self, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        let reader = std::sync::Arc::new(reader);
        // Captured before the loader closures take the Arc: the reader's
        // concurrent-range fan-out, stamped onto the index for the planner.
        let read_concurrency = reader.concurrency();

        // Lazily-chunked dictionary: locate the four sections, fetch each
        // section's header + restart table + chunk directory (small), and
        // fault the chunk bodies in on first term lookup.
        let dict = ranged_chunked_dictionary(&reader, &header, [true; 4])?;

        // Locate the six index section payloads (container framing only) and
        // fetch just their tile directories — shared with the per-named-graph
        // lazy opener, which opens a large graph's container the same way.
        let reader_dyn: std::sync::Arc<dyn RangeReader + Send + Sync> = reader.clone();
        let (index, index_section_ranges, tile_ranges) = open_index_container_lazy(
            &reader_dyn,
            ByteRange {
                offset: header.root_dir_offset,
                len: header.root_dir_len,
            },
            header.block_codec,
            header.has_tile_synopsis(),
            read_concurrency,
            header.perms,
        )?;

        // The pyramid meta is large on real graphs (tens of MB) and SPARQL never
        // reads it, so defer its fetch: it faults in only if `pyramid()` is
        // called (community / pyramid_tree / inspect queries).
        let pyramid = ranged_pyramid_slot(&reader, &header);

        // The TEXT_INDEX section is also deferred: a text search faults the token
        // table on first call (the leading varint then its compressed bytes), then
        // fetches individual posting lists by `(offset, len)` — never the whole
        // postings blob. A SPARQL query, which never searches, pays nothing.
        let text_index = ranged_text_index_slot(&reader, &header);

        // Named graphs are the last eagerly-fetched section standing on this
        // path — and on a many-graph file they dwarf everything else (67 MB
        // fetched and ~32k graph indexes decoded before the first query, on a
        // file whose queries then touch a handful of graphs). Defer them like
        // the pyramid and text index: nothing is read at open; a query walks
        // the directory and decodes only the graphs it touches.
        let named_graphs = if header.named_graphs_len > 0 {
            NamedGraphsSlot::Lazy(LazyNamedGraphs::new(
                reader_dyn,
                ByteRange {
                    offset: header.named_graphs_offset,
                    len: header.named_graphs_len,
                },
                header.block_codec,
                header.has_tile_synopsis(),
                read_concurrency,
                header.perms,
            ))
        } else {
            NamedGraphsSlot::Resident(Vec::new())
        };

        Ok(Self {
            header,
            dict,
            index,
            index_section_ranges,
            tile_ranges,
            pyramid,
            text_index,
            named_graphs,
            metadata: Vec::new(),
            service_client: None,
            service_error: std::sync::Mutex::new(None),
        })
    }

    /// Did any lazy fetch (index tile or dictionary chunk) fail since this
    /// `Rete` was opened? When true, query results may be silently incomplete â€”
    /// callers using [`Rete::open_ranged_lazy`] must check this after
    /// evaluating and turn it into an error.
    pub fn index_incomplete(&self) -> bool {
        self.index.load_incomplete()
            || self.dict.load_incomplete()
            || self.named_graphs.load_incomplete()
    }

    /// Forget recorded lazy-fetch failures — the start-of-evaluation reset for
    /// a RESIDENT session (a browser worker holding one `Rete` across many
    /// queries): it makes [`index_incomplete`](Self::index_incomplete) a
    /// per-query verdict instead of a per-open one, so a single transient
    /// network failure no longer fails every subsequent query on the session.
    /// Sound because failed tiles/chunks are never cached — the next
    /// evaluation simply retries the fetch.
    pub fn reset_load_failures(&self) {
        self.index.reset_load_failure();
        self.dict.reset_load_failure();
        self.named_graphs.reset_load_failures();
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

        let index_permutation = GraphIndex::best_permutation_in(self.header.perms, pattern);
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

    /// Match a triple pattern **within a single graph** — `None` is the default
    /// graph, `Some(iri)` a named graph — resolving matches to canonical terms.
    /// This is [`Rete::query`] (default-graph only) generalized to any graph: the
    /// graph-scoped primitive a quad-aware consumer (e.g. an RDF4J `Sail`'s
    /// `getStatements`) needs. An unknown graph IRI, or a bound term absent from
    /// the shared dictionary, yields an empty result. All graphs share one
    /// dictionary, so the pattern resolves once against that ID space.
    pub fn query_in_graph(
        &self,
        graph: Option<&str>,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Vec<TermTriple> {
        let pattern = match self.resolve_query_pattern(s, p, o) {
            Some(pattern) => pattern,
            None => return Vec::new(),
        };
        let index = match graph {
            None => &self.index,
            Some(g) => match self.graph_index(g) {
                Some(i) => i,
                None => return Vec::new(),
            },
        };
        let ids = index.match_pattern(pattern);
        // One coalesced dictionary fault for THIS pattern's output, not the
        // whole dictionary. `prefetch_all` used to sit here, which made a
        // bounded `(s, p, o, g)` lookup — the primitive an RDF4J `Sail`'s
        // `getStatements` calls, once per named graph via `query_quads` — pay
        // for every term in the file: on `cordis.rete` (801 MB, a 417 MB
        // dictionary) a 15-row answer read 416 MB and peaked at 2.5 GB RSS.
        // Same call `dump_batch` makes, for the same reason.
        if !ids.is_empty() {
            let mut nodes = Vec::with_capacity(ids.len() * 2);
            let mut preds = Vec::with_capacity(ids.len());
            for &(s, p, o) in &ids {
                nodes.push(self.dict.subject_node(s));
                nodes.push(self.dict.object_node(o));
                preds.push(p);
            }
            self.dict.prefetch_terms(&nodes, &preds);
        }
        ids.into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
            .collect()
    }

    /// Match a triple pattern across the default graph **and every named graph**,
    /// tagging each match with its graph (`None` = default). The quad-level
    /// companion to [`Rete::query`]; default-graph matches come first, then each
    /// named graph in stored order.
    pub fn query_quads(
        &self,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> Vec<(TermTriple, Option<String>)> {
        let mut out: Vec<(TermTriple, Option<String>)> = self
            .query_in_graph(None, s, p, o)
            .into_iter()
            .map(|t| (t, None))
            .collect();
        for i in 0..self.named_graphs.count() {
            // A quad dump visits every graph by construction — exhaustive
            // demand, so a lazy walk may read the section in bulk chunks.
            let Some(iri) = self.named_graphs.name_at(i, true) else {
                continue;
            };
            for triple in self.query_in_graph(Some(iri), s, p, o) {
                out.push((triple, Some(iri.to_string())));
            }
        }
        out
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
    let dict = decode_dictionary_container(&dict_bytes, header.dict_codec)?;

    let Some(pattern) = resolve_query_pattern(&dict, s, p, o) else {
        return Ok(None);
    };
    let permutation = GraphIndex::best_permutation_in(header.perms, pattern);
    let section = locate_container_section_ranged(
        reader,
        header.root_dir_offset,
        header.root_dir_len,
        header
            .perms
            .position(permutation)
            .ok_or(FileError::Container("routed to an absent permutation"))?,
        header.perms.len() as u64,
    )?;
    Ok(Some(RoutedPattern {
        dict,
        pattern,
        permutation,
        header,
        section,
    }))
}

/// Fetch and scan a routed pattern's matches: read the tile directory, then only
/// the matching tile byte ranges (the run of covering tiles for a bound leading
/// id — one for ordinary groups, several for a split mega-group — the
/// O(matching bytes) promise).
fn fetch_routed_matches<R: RangeReader>(
    reader: &R,
    routed: &RoutedPattern,
) -> Result<Vec<Triple>, FileError> {
    let dir = read_tile_directory_ranged(reader, routed.section)?;
    let [pa, _, _] = routed.permutation.order_pattern(routed.pattern);
    let codec = routed.header.block_codec;
    let mut out = Vec::new();
    match pa {
        // Bound leading id: the run of covering tiles (several for a split
        // mega-group; one otherwise).
        Some(a) => {
            for e in dir.iter().filter(|e| e.min_a <= a && a <= e.max_a) {
                let bytes = reader.read_at(routed.section.offset + e.start, e.end - e.start)?;
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
                let body = reader.read_at(routed.section.offset + base, last.end - base)?;
                for e in &dir {
                    let tile = decompress(
                        codec,
                        &body[(e.start - base) as usize..(e.end - base) as usize],
                    )?;
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

/// The lazily-chunked dictionary of a ranged open: per section, fetch the
/// header + chunk directory and fault the chunk bodies in on first term lookup.
///
/// `want` selects which of the four sections (0 shared, 1 subject-only,
/// 2 object-only, 3 predicates) are really read; a skipped one becomes an empty
/// section whose terms resolve to `None`. A **directory is not small** on a
/// literal-heavy graph — it carries each chunk's first term verbatim, so the
/// object-only section of a dataset that stores abstracts can run to hundreds of
/// megabytes, and fetching it is most of what a remote open costs. A reader that
/// only ever resolves subjects (see [`SearchView`]) skips sections 2 and 3 and
/// pays none of it. [`Rete::open_ranged_lazy`] wants all four.
fn ranged_chunked_dictionary<R: RangeReader + Send + Sync + 'static>(
    reader: &std::sync::Arc<R>,
    header: &Header,
    want: [bool; 4],
) -> Result<Dictionary, FileError> {
    let mut dict_sections: Vec<crate::dict::ChunkedSection> = Vec::with_capacity(4);
    for si in 0..4 {
        if !want[si] {
            dict_sections.push(crate::dict::ChunkedSection::from_parts(
                crate::dict::SectionMeta {
                    term_count: 0,
                    restart_interval: 1,
                    restart_offsets: Vec::new(),
                },
                Vec::new(),
                None,
            ));
            continue;
        }
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
                offset: section.offset + e.start,
                len: (e.end - e.start),
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
    Ok(Dictionary::from_chunked_sections(dict_arr))
}

/// The pyramid meta is large on real graphs (tens of MB) and SPARQL never reads
/// it, so a ranged open defers its fetch: it faults in only if `pyramid()` is
/// called (community / pyramid_tree / label-prefix queries).
fn ranged_pyramid_slot<R: RangeReader + Send + Sync + 'static>(
    reader: &std::sync::Arc<R>,
    header: &Header,
) -> PyramidSlot {
    if header.pyramid_meta_len == 0 {
        return PyramidSlot::Resident(None);
    }
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
}

/// The deferred TEXT_INDEX of a ranged open: a text search faults the token
/// table on first call (the leading varint then its compressed bytes), then
/// fetches individual posting lists by `(offset, len)` — never the whole
/// postings blob. A caller that never searches pays nothing.
fn ranged_text_index_slot<R: RangeReader + Send + Sync + 'static>(
    reader: &std::sync::Arc<R>,
    header: &Header,
) -> TextIndexSlot {
    if header.text_index_len == 0 {
        return TextIndexSlot::Resident {
            index: None,
            token_table_len: None,
        };
    }
    let ti_reader = reader.clone();
    let probe_reader = reader.clone();
    let ti_off = header.text_index_offset;
    let ti_len = header.text_index_len;
    let codec = header.block_codec;
    TextIndexSlot::Lazy {
        loader: Box::new(move || {
            // The section opens with `varint token_table_len`; read enough to
            // decode it (a uvarint is ≤ 10 bytes), then fetch the varint + the
            // compressed token table as one prefix range.
            let prefix_len = read_token_table_len(&*ti_reader, ti_off, ti_len)?;
            let prefix = ti_reader.read_at(ti_off, prefix_len).ok()?;
            let postings_base = crate::text_index::TextIndex::postings_base(&prefix)? as u64;
            let postings_abs = ti_off + postings_base;
            let pr = ti_reader.clone();
            let posting_loader =
                Box::new(move |off: u64, len: u64| pr.read_at(postings_abs + off, len).ok());
            crate::text_index::TextIndex::from_token_table(&prefix, codec, posting_loader).ok()
        }),
        cell: std::sync::OnceLock::new(),
        // The same leading varint the loader starts from, asked on its own so a
        // caller can *state* the cost of a first search without paying it.
        token_table: Box::new(move || read_token_table_len(&*probe_reader, ti_off, ti_len)),
        token_table_cell: std::sync::OnceLock::new(),
    }
}

/// Full-text search over `ti`, resolving the matched subject ids through `dict`.
/// The engine behind [`Rete::text_search`] and [`SearchView::text_search`].
fn text_search_in(
    ti: Option<&crate::text_index::TextIndex>,
    dict: &Dictionary,
    words: &[&str],
    prefix: Option<&str>,
    limit: usize,
) -> Vec<String> {
    let Some(ti) = ti else {
        return Vec::new();
    };
    // Each query word is tokenized exactly as at build time (so "Glucose"
    // matches the stored "glucose"); a word that splits into several tokens
    // requires all of them. AND across every required token + the prefix.
    let mut acc: Option<Vec<u32>> = None;
    if let Some(p) = prefix {
        acc = Some(ti.prefix(&p.to_lowercase()));
    }
    for w in words {
        for tok in crate::text_index::tokenize(w) {
            let posting = ti.lookup(&tok);
            acc = Some(match acc {
                Some(a) => intersect_sorted(&a, &posting),
                None => posting,
            });
            if acc.as_ref().is_some_and(|a| a.is_empty()) {
                return Vec::new();
            }
        }
    }
    let ids = acc.unwrap_or_default();
    let mut out = Vec::with_capacity(if limit > 0 {
        limit.min(ids.len())
    } else {
        ids.len()
    });
    for id in ids {
        if let Some(iri) = dict.subject_term(id) {
            out.push(iri);
            if limit > 0 && out.len() >= limit {
                break;
            }
        }
    }
    out
}

/// A `.rete` opened for **search only**: the dictionary's chunk directories, and
/// the TEXT_INDEX / pyramid deferred behind their lazy slots. Nothing else.
///
/// What makes this cheap is skipping dictionary sections 2 and 3 (object-only
/// and predicates) — NOT skipping the index. Measured on the published
/// `epfl-infoscience.rete` (1.64 GB), a full [`Rete::open_ranged_lazy`] reads
/// 536,947,344 B, of which the six permutation tile directories are **49,940 B
/// in 40 reads** and the dictionary is 536,896,380 B in 36 reads: the
/// object-only chunk directory alone is 234,400,728 B, because it stores every
/// chunk's first term verbatim (see #198). This view opens sections 0 and 1
/// only — both search modes return subject IRIs — and costs **21,554 B in 9
/// reads** on the same file. It also skips the index container and defers the
/// TEXT_INDEX and pyramid, so it pays only for what it reads: the token table
/// on the first search, then one range per posting list and per dictionary
/// chunk holding a matched subject. This is what backs `rete search-url`.
pub struct SearchView {
    header: Header,
    dict: Dictionary,
    pyramid: PyramidSlot,
    text_index: TextIndexSlot,
}

impl SearchView {
    /// Open for search over any [`RangeReader`] (HTTP or local): header +
    /// dictionary chunk directories only, with the text index and pyramid
    /// faulting in on first use.
    pub fn open_ranged<R: RangeReader + Send + Sync + 'static>(
        reader: R,
    ) -> Result<Self, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        let reader = std::sync::Arc::new(reader);
        // Sections 0 (shared) and 1 (subject-only) are the only ones a search
        // resolves — both modes return subject IRIs. Skipping 2 and 3 is what
        // makes this open cheap where a full one is not.
        let dict = ranged_chunked_dictionary(&reader, &header, [true, true, false, false])?;
        let pyramid = ranged_pyramid_slot(&reader, &header);
        let text_index = ranged_text_index_slot(&reader, &header);
        Ok(Self {
            header,
            dict,
            pyramid,
            text_index,
        })
    }

    /// The file header (already fetched by [`open_ranged`](Self::open_ranged)).
    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Whether the file carries a TEXT_INDEX section — a header read, no fault.
    pub fn has_text_index(&self) -> bool {
        self.header.text_index_len > 0
    }

    /// Whether the file carries a pyramid (where the label index lives) — a
    /// header read, no fault.
    pub fn has_pyramid(&self) -> bool {
        self.header.pyramid_meta_len > 0
    }

    /// Full-text search — see [`Rete::text_search`] for the semantics. Faults
    /// the token table on the first call, then only the queried postings.
    pub fn text_search(&self, words: &[&str], prefix: Option<&str>, limit: usize) -> Vec<String> {
        text_search_in(self.text_index.index(), &self.dict, words, prefix, limit)
    }

    /// What that first search will cost — see
    /// [`Rete::text_index_token_table_len`]. One ≤10-byte range read, so a
    /// caller can quote the figure before deciding to search.
    pub fn text_index_token_table_len(&self) -> Option<u64> {
        self.text_index.token_table_len()
    }

    /// Label-prefix search — see [`Rete::prefix_search`]. Faults the pyramid
    /// (which carries the label index), so it is the pricier of the two.
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Vec<(String, String)> {
        let pyr = match &self.pyramid {
            PyramidSlot::Resident(p) => p.as_ref(),
            PyramidSlot::Lazy { loader, cell } => cell.get_or_init(loader).as_ref(),
        };
        let Some(pyr) = pyr else {
            return Vec::new();
        };
        pyr.prefix_search(prefix, limit)
            .into_iter()
            .filter_map(|e| {
                self.dict
                    .subject_term(e.subject)
                    .map(|iri| (e.label.clone(), iri))
            })
            .collect()
    }
}

/// A lightweight, overview-only view of a file: the pyramid summary graph plus
/// just enough dictionary to label predicates. Fetched via ranges *without*
/// touching the (large) triple index — the "load the coarse graph first" path
/// from SPEC.md §7.2.
#[must_use]
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
        let dict = decode_dictionary_container(&dict_bytes, header.dict_codec)?;

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

    /// A three-permutation file must survive every opener — resident, ranged,
    /// ranged-lazy — and answer every pattern shape with the rows the
    /// six-permutation twin gives, including through a named graph's own
    /// container. The mask is a file-level fact, so getting it to the named
    /// graphs' decoder is a separate thing that can be forgotten.
    #[test]
    fn three_permutation_file_round_trips_every_open_path() {
        use crate::index::PermSet;
        use crate::reader::SliceReader;

        let build = |perms: PermSet| {
            let mut db = crate::DictionaryBuilder::new();
            let mut triples = Vec::new();
            for i in 0..60u32 {
                let (s, p, o) = (
                    format!("<http://ex/s{}>", i % 11),
                    format!("<http://ex/p{}>", i % 3),
                    format!("<http://ex/o{}>", i % 7),
                );
                db.observe(&s, &p, &o);
                triples.push((s, p, o));
            }
            let dict = db.build();
            let ids: Vec<(u32, u32, u32)> = triples
                .iter()
                .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
                .collect();
            let def = GraphIndexBuilder::from_triples(ids.clone())
                .with_tile_budget(96)
                .with_perms(perms)
                .build();
            let named = GraphIndexBuilder::from_triples(ids)
                .with_tile_budget(96)
                .with_perms(perms)
                .build();
            write_dataset(
                &dict,
                &def,
                &[("<http://ex/g>".to_string(), named)],
                true,
                &[],
                0,
            )
        };

        let six = build(PermSet::ALL);
        let three = build(PermSet::CORE);
        assert!(three.len() < six.len());
        assert_eq!(Header::from_bytes(&three).unwrap().perms, PermSet::CORE);
        assert_eq!(Header::from_bytes(&six).unwrap().perms, PermSet::ALL);

        let shapes: Vec<(Option<&str>, Option<&str>, Option<&str>)> = vec![
            (None, None, None),
            (Some("<http://ex/s3>"), None, None),
            (None, Some("<http://ex/p1>"), None),
            (None, None, Some("<http://ex/o5>")),
            (Some("<http://ex/s3>"), Some("<http://ex/p1>"), None),
            (Some("<http://ex/s3>"), None, Some("<http://ex/o5>")),
            (None, Some("<http://ex/p1>"), Some("<http://ex/o5>")),
            (
                Some("<http://ex/s3>"),
                Some("<http://ex/p1>"),
                Some("<http://ex/o5>"),
            ),
        ];

        for (image, tag) in [(&six, "six"), (&three, "three")] {
            let resident = Rete::open(image).unwrap();
            let leaked: &'static [u8] = Box::leak(image.clone().into_boxed_slice());
            let ranged = Rete::open_ranged(&SliceReader::new(leaked)).unwrap();
            let lazy = Rete::open_ranged_lazy(SliceReader::new(leaked)).unwrap();
            for (s, p, o) in &shapes {
                let want: Vec<_> = {
                    let mut v = Rete::open(&six).unwrap().query(*s, *p, *o);
                    v.sort();
                    v
                };
                for (got, path) in [
                    (resident.query(*s, *p, *o), "resident"),
                    (ranged.query(*s, *p, *o), "ranged"),
                    (lazy.query(*s, *p, *o), "lazy"),
                ] {
                    let mut got = got;
                    got.sort();
                    assert_eq!(got, want, "{tag}/{path} disagreed on {:?}", (s, p, o));
                }
                // The named graph's container carries the same mask.
                let mut g = resident.query_in_graph(Some("<http://ex/g>"), *s, *p, *o);
                g.sort();
                assert_eq!(g, want, "{tag}/named disagreed on {:?}", (s, p, o));
            }
        }

        // And the single-pattern ROUTED read (the `query-url` path) locates its
        // section by container position, not by the format's fixed six-wide
        // slot — the one place the two indexes differ.
        let leaked: &'static [u8] = Box::leak(three.clone().into_boxed_slice());
        let reader = SliceReader::new(leaked);
        let routed = Rete::query_ranged(&reader, None, None, Some("<http://ex/o5>")).unwrap();
        let mut want = Rete::open(&six)
            .unwrap()
            .query(None, None, Some("<http://ex/o5>"));
        want.sort();
        let mut got = routed;
        got.sort();
        assert_eq!(got, want, "routed single-pattern read on a lean file");
    }

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
        assert_eq!(rete.header().version, crate::header::CURRENT_FORMAT_VERSION);
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

    /// The tile-synopsis trailer round-trips through encode/parse, and each parsed
    /// synopsis is **exactly** the tile block's own b/c zone — so the directory
    /// can never prune a tile the tile itself would have matched.
    /// Section-internal byte offsets are u64: a directory whose tiles sit past
    /// 4 GiB must parse with exact offsets on EVERY platform. On wasm32 (32-bit
    /// usize) the old parse truncated a >4 GiB section length and rejected the
    /// tail ("dict chunk overruns section" on the first >4 GiB dictionary —
    /// crossref's 5.2 GB g.obj — the playground regression this guards).
    #[test]
    fn tile_directory_offsets_survive_past_4gib() {
        let mut dir = Vec::new();
        write_uvarint(&mut dir, 2); // two tiles
        write_uvarint(&mut dir, 5); // tile 1: Δmin_a
        write_uvarint(&mut dir, 0); //         span
        write_uvarint(&mut dir, 3 << 30); //   len = 3 GiB
        write_uvarint(&mut dir, 1); // tile 2: Δmin_a
        write_uvarint(&mut dir, 0);
        write_uvarint(&mut dir, 2 << 30); //   len = 2 GiB
        let total = dir.len() as u64 + (3u64 << 30) + (2u64 << 30) + 64;
        let entries = parse_tile_directory(&dir, total).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[1].start, dir.len() as u64 + (3u64 << 30));
        assert!(
            entries[1].end > u32::MAX as u64,
            "tail tile sits past 4 GiB"
        );
        // a total smaller than the tiles must still reject the directory
        assert!(parse_tile_directory(&dir, 1 << 20).is_err());
    }

    #[test]
    fn tile_synopsis_trailer_round_trips() {
        let mut ib = GraphIndexBuilder::new().with_tile_budget(64);
        for i in 0..200u32 {
            ib.push((i, i % 7, i % 13));
        }
        let index = ib.build();
        let tiles = index.tile_sections()[0];
        assert!(tiles.len() > 3, "tiny budget forces many tiles");

        let payload = encode_tiled_section(tiles, CODEC_NONE);
        let dir = parse_tile_directory(&payload, payload.len() as u64).unwrap();
        assert_eq!(dir.len(), tiles.len());
        // The trailer sits past the last tile; the old directory parse stops there.
        let trailer_start = dir.iter().map(|e| e.end).max().unwrap();
        assert!(
            trailer_start < payload.len() as u64,
            "a trailer follows the tiles"
        );
        for e in &dir {
            assert!(
                e.end <= payload.len() as u64,
                "tiles still located within the payload"
            );
        }
        let syn = parse_tile_synopsis(&payload, trailer_start as usize, dir.len()).unwrap();
        for (e, (min_b, max_b, min_c, max_c)) in dir.iter().zip(syn) {
            let block = decompress(CODEC_NONE, &payload[e.start as usize..e.end as usize]).unwrap();
            let z = *crate::triples::TripleBlock::parse(&block).unwrap().zone();
            assert_eq!(
                (min_b, max_b, min_c, max_c),
                (z.min_b, z.max_b, z.min_c, z.max_c),
                "synopsis equals the tile's own zone"
            );
        }
    }

    /// End-to-end safety: a synopsis-carrying file, opened **lazily** (range
    /// reads), must return exactly the brute-force answer for every pattern shape
    /// — the synopsis prune may never drop a real match.
    #[test]
    fn tile_synopsis_lazy_matches_reference_every_shape() {
        use crate::reader::{CountingReader, SliceReader};
        let triples: Vec<(String, String, String)> = (0..200u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    format!("<http://ex/p/{}>", i % 7),
                    format!("<http://ex/o/{:04}>", i % 13),
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
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        let eager = Rete::open(&bytes).unwrap();
        assert!(
            eager.header().has_tile_synopsis(),
            "new files set the synopsis flag"
        );

        // `open_ranged_lazy` needs a `'static` reader; leak the image (the test
        // process exits straight after).
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let lazy = Rete::open_ranged_lazy(reader).unwrap();

        let brute = |s: Option<&str>, p: Option<&str>, o: Option<&str>| {
            let mut v: Vec<(String, String, String)> = triples
                .iter()
                .filter(|(a, b, c)| {
                    s.is_none_or(|x| x == a) && p.is_none_or(|x| x == b) && o.is_none_or(|x| x == c)
                })
                .cloned()
                .collect();
            v.sort();
            v
        };
        // Existing + absent terms in every position (and unbound) — 4×4×4 shapes.
        let sv = [
            None,
            Some("<http://ex/s/0007>"),
            Some("<http://ex/s/0130>"),
            Some("<http://ex/s/9999>"),
        ];
        let pv = [
            None,
            Some("<http://ex/p/3>"),
            Some("<http://ex/p/6>"),
            Some("<http://ex/p/999>"),
        ];
        let ov = [
            None,
            Some("<http://ex/o/0000>"),
            Some("<http://ex/o/0012>"),
            Some("<http://ex/o/9999>"),
        ];
        for &s in &sv {
            for &p in &pv {
                for &o in &ov {
                    let mut e = eager.query(s, p, o);
                    e.sort();
                    let mut l = lazy.query(s, p, o);
                    l.sort();
                    let r = brute(s, p, o);
                    assert_eq!(e, r, "eager {s:?} {p:?} {o:?}");
                    assert_eq!(l, r, "lazy {s:?} {p:?} {o:?} — synopsis over-pruned");
                }
            }
        }
        assert!(!lazy.index_incomplete(), "no lazy fetch failed");
    }

    /// Byte-0 / polyglot experiment: a real `.rete` embedded behind a large HTML
    /// shell (so byte 0 is `<`, not `RETE`) still opens and queries LAZILY through
    /// an `OffsetReader`, touching only the graph's bytes and never the shell.
    #[test]
    fn polyglot_offset_reads_lazily() {
        use crate::reader::{
            detect_polyglot_base, CountingReader, OffsetReader, SliceReader, POLYGLOT_DIGITS,
            POLYGLOT_MARKER,
        };

        // A real .rete image, hidden behind a 50 KB HTML shell that carries the
        // polyglot base-offset marker in its first bytes (a browser-ignored
        // comment). The .rete begins right after the shell.
        let image = build_image();
        let mut shell = Vec::new();
        shell.extend_from_slice(b"<!DOCTYPE html><html><head><!--");
        shell.extend_from_slice(POLYGLOT_MARKER);
        let digits_at = shell.len();
        shell.extend_from_slice(&[b'0'; POLYGLOT_DIGITS]); // patched below
        shell.extend_from_slice(b"--></head><body>a web page</body></html>\n");
        shell.resize(50_000, b' '); // >> the .rete, to prove the shell is untouched
        let base = shell.len() as u64;
        let digits = format!("{base:0width$}", width = POLYGLOT_DIGITS);
        shell[digits_at..digits_at + POLYGLOT_DIGITS].copy_from_slice(digits.as_bytes());

        let mut poly = shell;
        poly.extend_from_slice(&image);

        // byte 0 is a web page, not a .rete...
        assert_ne!(
            &poly[0..4],
            b"RETE",
            "polyglot must not start with the magic"
        );
        // ...but the marker in the first header window points at the embedded .rete.
        let detected = detect_polyglot_base(&poly[..crate::header::HEADER_LEN]).unwrap();
        assert_eq!(detected, base);

        // Open it LAZILY through the offset shim and query it.
        let leaked: &'static [u8] = Box::leak(poly.into_boxed_slice());
        let counting = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let lazy = Rete::open_ranged_lazy(OffsetReader::new(counting.clone(), detected)).unwrap();

        // The polyglot must answer exactly like the plain .rete it embeds.
        let plain = Rete::open(&image).unwrap();
        let mut want = plain.query(None, None, None);
        want.sort();
        let mut got = lazy.query(None, None, None);
        got.sort();
        assert_eq!(
            got, want,
            "polyglot lazy query differs from the plain .rete"
        );
        assert!(!got.is_empty(), "expected some triples");
        assert!(!lazy.index_incomplete(), "a lazy fetch failed");

        // It was LAZY: it read fewer bytes than the HTML shell alone, i.e. it
        // never touched the prefix — only the embedded .rete's own bytes.
        let read = counting.bytes_read();
        eprintln!(
            "polyglot lazy read: HTML shell {base} B + .rete image {} B; \
             the query touched only {read} B (never the shell).",
            image.len()
        );
        assert!(
            read < base,
            "read {read} bytes; the HTML shell alone is {base} — the reader \
             touched the prefix instead of only the embedded .rete"
        );
    }

    /// Build a small file whose objects are string literals, **with** a text
    /// index, and return `(image, triples)`. Shared by the text-index tests.
    #[cfg(test)]
    fn build_text_indexed(triples: &[(String, String, String)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(64);
        let mut id_triples: Vec<(u32, u32, u32)> = Vec::with_capacity(triples.len());
        for (s, p, o) in triples {
            let t = dict.encode(s, p, o).unwrap();
            ib.push(t);
            id_triples.push(t);
        }
        let index = ib.build();
        let text_index = compute_text_index(&dict, &id_triples);
        assert!(
            !text_index.is_empty(),
            "literals should produce a text index"
        );
        write_dataset_with_metadata(&dict, &index, &[], false, &[], 0, &[], &text_index)
    }

    /// A `--text-index` build round-trips: `text_search` returns exactly the
    /// subjects whose literals contain the queried word(s), with AND across words
    /// and token-prefix — matching a brute-force scan of the literals.
    #[test]
    fn text_index_eager_matches_brute_force() {
        let triples: Vec<(String, String, String)> = vec![
            (
                "<http://ex/s0>",
                "<http://ex/label>",
                "\"alpha glucose phosphate\"",
            ),
            ("<http://ex/s1>", "<http://ex/label>", "\"beta Glucose\""),
            ("<http://ex/s2>", "<http://ex/label>", "\"gamma fructose\""),
            (
                "<http://ex/s3>",
                "<http://ex/note>",
                "\"einstein relativity\"",
            ),
            (
                "<http://ex/s4>",
                "<http://ex/ref>",
                "<http://ex/not-a-literal>",
            ),
        ]
        .into_iter()
        .map(|(s, p, o)| (s.to_string(), p.to_string(), o.to_string()))
        .collect();
        let bytes = build_text_indexed(&triples);
        let rete = Rete::open(&bytes).unwrap();
        assert!(rete.has_text_index());

        // Brute-force reference: subjects whose literal objects contain all words.
        let brute = |words: &[&str]| -> Vec<String> {
            let mut v: Vec<String> = triples
                .iter()
                .filter(|(_, _, o)| {
                    crate::terms::is_literal(o)
                        && words.iter().all(|w| {
                            let wl = w.to_lowercase();
                            crate::terms::literal_lexical(o)
                                .unwrap()
                                .split(|c: char| !c.is_alphanumeric())
                                .any(|t| t.to_lowercase() == wl)
                        })
                })
                .map(|(s, _, _)| s.clone())
                .collect();
            v.sort();
            v.dedup();
            v
        };

        let mut got = rete.text_search(&["glucose"], None, 0);
        got.sort();
        assert_eq!(got, brute(&["glucose"]), "case-insensitive single word");

        // AND across two words: only s0 has both.
        let mut got = rete.text_search(&["glucose", "phosphate"], None, 0);
        got.sort();
        assert_eq!(got, brute(&["glucose", "phosphate"]));

        // A word nobody has → empty.
        assert!(rete.text_search(&["zzznope"], None, 0).is_empty());

        // Token-prefix: "ein…" matches "einstein".
        let got = rete.text_search(&[], Some("ein"), 0);
        assert_eq!(got, vec!["<http://ex/s3>".to_string()]);

        // No text index → empty, has_text_index() false.
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let plain = write_dataset(&dict, &ib.build(), &[], false, &[], 0);
        let plain_rete = Rete::open(&plain).unwrap();
        assert!(!plain_rete.has_text_index());
        assert!(plain_rete.text_search(&["glucose"], None, 0).is_empty());
    }

    /// The lazy/remote path returns the same subjects as the eager path **and**
    /// faults only the token table + the queried posting list — never the whole
    /// postings blob. A `CountingReader` proves the byte budget stays small.
    #[test]
    fn text_index_lazy_faults_only_queried_postings() {
        use crate::reader::{CountingReader, SliceReader};
        // Many subjects so the postings blob is large relative to one posting:
        // every subject carries "common", but only a few carry "rare".
        let mut triples: Vec<(String, String, String)> = (0..300u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/label>".to_string(),
                    format!("\"common word number {i}\""),
                )
            })
            .collect();
        for i in [3u32, 77, 250] {
            triples.push((
                format!("<http://ex/s/{i:04}>"),
                "<http://ex/tag>".to_string(),
                "\"raretoken\"".to_string(),
            ));
        }
        let bytes = build_text_indexed(&triples);
        let eager = Rete::open(&bytes).unwrap();
        let mut want = eager.text_search(&["raretoken"], None, 0);
        want.sort();
        assert_eq!(want.len(), 3);

        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
        // Bytes pulled by the open itself (dict dirs, index dirs, named graphs) —
        // the text index is deferred and not touched yet.
        let before = reader.bytes_read();
        let mut got = lazy.text_search(&["raretoken"], None, 0);
        got.sort();
        assert_eq!(got, want, "lazy search matches eager");
        let pulled = reader.bytes_read() - before;
        // The search faulted the token table + the one "raretoken" posting; it must
        // be far less than the whole text-index section (300 "common" postings).
        let ti_len = eager.header().text_index_len;
        assert!(
            pulled < ti_len,
            "search pulled {pulled} B but the section is {ti_len} B — faulted too much"
        );
        assert!(!lazy.index_incomplete());
    }

    /// `SearchView` answers exactly what a full open answers, having read
    /// strictly less: it skips the permutation tile directories and — the part
    /// that actually costs on a literal-heavy graph — the object-only
    /// dictionary's chunk directory, which carries every chunk's first term.
    #[test]
    fn search_view_matches_full_open_for_far_fewer_bytes() {
        use crate::reader::{CountingReader, SliceReader};
        // Long object literals so the object-only dictionary (and its chunk
        // directory) dominates the file, as on a graph that stores abstracts.
        let filler = "lorem ipsum dolor sit amet consectetur adipiscing elit ".repeat(20);
        let mut triples: Vec<(String, String, String)> = (0..300u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/abstract>".to_string(),
                    format!("\"{filler} number {i}\""),
                )
            })
            .collect();
        for i in [3u32, 77, 250] {
            triples.push((
                format!("<http://ex/s/{i:04}>"),
                "<http://ex/tag>".to_string(),
                "\"raretoken\"".to_string(),
            ));
        }
        let bytes = build_text_indexed(&triples);
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());

        let full_reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let full = Rete::open_ranged_lazy(full_reader.clone()).unwrap();
        let mut want = full.text_search(&["raretoken"], None, 0);
        want.sort();
        assert_eq!(want.len(), 3);

        let search_reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let view = SearchView::open_ranged(search_reader.clone()).unwrap();
        assert!(view.has_text_index());
        let mut got = view.text_search(&["raretoken"], None, 0);
        got.sort();
        assert_eq!(got, want, "search view matches the full open");
        assert!(
            search_reader.bytes_read() < full_reader.bytes_read(),
            "search read {} B, full open read {} B — the narrow open bought nothing",
            search_reader.bytes_read(),
            full_reader.bytes_read()
        );

        // A file without a text index says so from the header alone, and the
        // label-prefix mode still works through the same view.
        let mut prefix_hits = view.prefix_search("", 5);
        prefix_hits.sort();
        assert!(!prefix_hits.is_empty() || !view.has_pyramid());
    }

    /// Nothing records a dictionary chunk directory's byte length, so a ranged
    /// open probes for it. That probe must cost about ONE directory, not several
    /// — it used to re-read the whole prefix on every round, so the sum was
    /// ~2x the last (already over-sized) read: 537 MB of range reads to fetch
    /// epfl-infoscience's 234 MB object-only directory. Here the same probe runs
    /// against a file whose object literals are long enough that its directory
    /// is well past the 8 KiB header prefix, and must (a) stay inside the
    /// append-only bound of 2x the directory and (b) beat what the old
    /// re-reading loop would have spent.
    #[test]
    fn dict_chunk_directory_probe_costs_about_one_directory() {
        use crate::reader::{CountingReader, SliceReader};
        // ~4 KiB literals: one restart run (16 terms) far exceeds the 64 KiB
        // chunk budget, so every chunk stores a full 4 KiB term in the directory
        // — the shape that makes this directory expensive on real graphs.
        let filler = "x".repeat(4000);
        let triples: Vec<(String, String, String)> = (0..800u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/abstract>".to_string(),
                    format!("\"{i:04} {filler}\""),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        let reader = CountingReader::new(SliceReader::new(&bytes));
        let header = Header::from_bytes(&reader.read_at(0, HEADER_LEN as u64).unwrap()).unwrap();
        // Section 2 is object-only: the one carrying the long literals.
        let section = locate_container_section_ranged(
            &reader,
            header.dictionary_offset,
            header.dictionary_len,
            2,
            4,
        )
        .unwrap();

        let before = reader.bytes_read();
        let (_meta, entries) = read_dict_dir_ranged(&reader, section).unwrap();
        let spent = reader.bytes_read() - before;

        // The first chunk body starts where the directory ends.
        let dir_end = entries[0].start;
        let head = reader.read_at(section.offset, 64).unwrap();
        let (header_len, n0) = read_uvarint(&head).unwrap();
        let dir_start = n0 as u64 + header_len;
        let dir_len = dir_end - dir_start;
        let dir_total = section.len - dir_start;
        assert!(
            dir_start + dir_len > 8192,
            "directory fits the header prefix ({dir_len} B) — the probe never runs"
        );

        // What the previous loop spent: every round re-read from the start.
        let mut old = 0u64;
        let mut p = 4096u64.min(dir_total).max(1);
        loop {
            let got = p.min(dir_total);
            old += got;
            if got >= dir_len || p >= dir_total {
                break;
            }
            p = p.saturating_mul(2).min(dir_total);
        }
        assert!(
            spent <= dir_len * 2,
            "probe read {spent} B for a {dir_len} B directory — past the append-only bound"
        );
        assert!(
            spent < old,
            "probe read {spent} B; the re-reading loop it replaced spent {old} B"
        );
    }

    /// The TEXT_INDEX section is inside the content hash: a freshly built
    /// text-indexed file must pass `verify()`, and flipping a byte inside the
    /// section must break it. (Regression: `verify()` once rebuilt the hash
    /// without the text index, so every `--text-index` file failed as corrupt.)
    #[test]
    fn text_index_is_tamper_evident_and_verifies() {
        let triples: Vec<(String, String, String)> = vec![(
            "<http://ex/s0>".to_string(),
            "<http://ex/label>".to_string(),
            "\"alpha glucose phosphate\"".to_string(),
        )];
        let bytes = build_text_indexed(&triples);
        let header = Rete::open(&bytes).unwrap().header().clone();
        assert!(header.text_index_len > 0);
        assert!(verify(&bytes).unwrap(), "a text-indexed build must verify");

        let mut tampered = bytes.clone();
        tampered[header.text_index_offset as usize] ^= 0xff;
        assert!(
            !verify(&tampered).unwrap(),
            "tampering with the text index must break verify()"
        );
    }

    /// End-to-end win: on a remote (range-read) file, a lookup whose routed tile
    /// is ruled out by a bound secondary fetches **fewer bytes** with the synopsis
    /// than without it — and the answer is identical (empty) either way.
    #[test]
    fn synopsis_cuts_remote_fetch_bytes() {
        use crate::header::FLAG_TILE_SYNOPSIS;
        use crate::reader::{CountingReader, SliceReader};

        // Zero-padded terms ⇒ dictionary ids are monotonic in i; subject s_i pairs
        // only with object o_i, so an OSP tile (routed by object) holds a
        // contiguous subject range — a subject from a far tile is provably absent.
        let triples: Vec<(String, String, String)> = (0..400u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/p>".to_string(),
                    format!("<http://ex/o/{i:04}>"),
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
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        // (s_0395, ?, o_0005): routes OSP by the (early) object, secondary = the
        // (late) subject — outside that tile's subject range, so the synopsis
        // prunes the one routed tile.
        let q = (Some("<http://ex/s/0395>"), None, Some("<http://ex/o/0005>"));
        // Measure the bytes the QUERY pulls (after open) — isolating the per-query
        // saving from the one-time synopsis trailer reads done at open, which a
        // persistent remote session amortizes over many queries.
        let query_bytes = |image: &[u8]| -> (u64, usize) {
            let leaked: &'static [u8] = Box::leak(image.to_vec().into_boxed_slice());
            let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
            let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
            let before = reader.bytes_read(); // after open (incl. trailer reads)
            let n = rete.query(q.0, q.1, q.2).len();
            assert!(!rete.index_incomplete());
            (reader.bytes_read() - before, n)
        };

        let (on_bytes, on_n) = query_bytes(&bytes);
        // Same file with the synopsis flag cleared = an older reader's behavior.
        let mut off = bytes.clone();
        off[5] &= !FLAG_TILE_SYNOPSIS;
        let (off_bytes, off_n) = query_bytes(&off);

        assert_eq!(on_n, 0, "the pair never co-occurs");
        assert_eq!(off_n, 0, "same answer without the synopsis");
        // Both pay the same dictionary-resolution bytes; the difference is the one
        // routed index tile that the synopsis skips (and the no-synopsis path
        // fetches only to have its zone map reject it).
        assert!(
            on_bytes < off_bytes,
            "synopsis skips the routed tile fetch: {on_bytes} < {off_bytes}"
        );
    }

    /// A double-bound-object intersection (`?p P o1 ; P o2 ; label ?l`) — the
    /// shape whose REMOTE join strategy changed (scan + hash-join instead of
    /// probing each prefix row) — must return the SAME rows opened eagerly (in
    /// memory) and lazily (remote-style, `is_remote()` true). Strategy is a
    /// performance choice; the result multiset is invariant.
    #[test]
    fn double_bound_object_join_eager_matches_lazy() {
        use crate::reader::SliceReader;
        let occ = "<http://ex/occ>";
        let phys = "<http://ex/physicist>";
        let phil = "<http://ex/philosopher>";
        let label = "<http://www.w3.org/2000/01/rdf-schema#label>";
        // p00..p19 are physicists; p00..p09 are also philosophers (the answer).
        let mut triples: Vec<(String, String, String)> = Vec::new();
        for i in 0..20u32 {
            triples.push((format!("<http://ex/p/{i:02}>"), occ.into(), phys.into()));
            if i < 10 {
                triples.push((format!("<http://ex/p/{i:02}>"), occ.into(), phil.into()));
            }
            triples.push((
                format!("<http://ex/p/{i:02}>"),
                label.into(),
                format!("\"Name {i:02}\""),
            ));
        }
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(16);
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        let q = "SELECT ?l WHERE { \
            ?p <http://ex/occ> <http://ex/physicist> ; \
               <http://ex/occ> <http://ex/philosopher> ; \
               <http://www.w3.org/2000/01/rdf-schema#label> ?l }";
        let run = |rete: &Rete| -> Vec<String> {
            let (_, sols) = crate::eval_sparql(rete, q).unwrap();
            let mut v: Vec<String> = sols.iter().filter_map(|b| b.get("l").cloned()).collect();
            v.sort();
            v
        };

        let eager_rows = run(&Rete::open(&bytes).unwrap());
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let lazy = Rete::open_ranged_lazy(std::sync::Arc::new(SliceReader::new(leaked))).unwrap();
        let lazy_rows = run(&lazy);
        assert!(!lazy.index_incomplete());

        assert_eq!(eager_rows.len(), 10, "the 10 physicist∩philosopher labels");
        assert_eq!(eager_rows, lazy_rows, "eager and lazy must agree exactly");
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
        write_dataset_with_metadata(&dict, &ib.build(), &[], false, &pmeta, levels, meta, &[])
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

    /// The contract `replace_metadata` has to meet to be usable at the end of a
    /// build: swapping the card must land on **exactly** the bytes the writer
    /// would have produced with that card in the first place — shorter, longer,
    /// and gone — so a spliced file and a rebuilt one are the same file.
    #[test]
    fn replace_metadata_matches_a_direct_write() {
        let first = br#"{"title":"My Dataset","queries":[1,2,3]}"#;
        let bytes = build_with_metadata(first);
        for target in [
            br#"{"title":"My Dataset","queries":[1,2]}"#.as_slice(), // shorter
            br#"{"title":"My Dataset","queries":[1,2,3,4,5,6,7]}"#.as_slice(), // longer
            b"".as_slice(),                                          // removed
            first.as_slice(),                                        // unchanged
        ] {
            let spliced = replace_metadata(&bytes, target).unwrap();
            assert_eq!(
                spliced,
                build_with_metadata(target),
                "splicing {} bytes of metadata must equal a direct write",
                target.len()
            );
            assert!(
                verify(&spliced).unwrap(),
                "the new hash covers the new card"
            );
            let rete = Rete::open(&spliced).unwrap();
            assert_eq!(
                rete.metadata(),
                (!target.is_empty()).then_some(target),
                "the new payload reads back verbatim"
            );
            // The data is untouched by the surgery.
            assert_eq!(
                rete.query(Some("Bob"), Some("knows"), Some("Carol")).len(),
                1
            );
        }
    }

    /// A build-info section survives the splice, still adjacent to the card, and
    /// still outside the content hash.
    #[test]
    fn replace_metadata_keeps_build_info_adjacent() {
        let with_info =
            attach_build_info(&build_with_metadata(br#"{"a":1}"#), br#"{"builder":"x"}"#).unwrap();
        let spliced = replace_metadata(&with_info, br#"{"a":1,"b":2}"#).unwrap();
        let h = Header::from_bytes(&spliced).unwrap();
        assert_eq!(h.metadata_offset, HEADER_LEN as u64);
        assert_eq!(h.build_info_offset, HEADER_LEN as u64 + h.metadata_len);
        assert_eq!(
            read_build_info(&spliced).unwrap().as_deref(),
            Some(br#"{"builder":"x"}"#.as_slice())
        );
        assert!(verify(&spliced).unwrap());
        // Same card, same content hash, whatever the build-info says.
        assert_eq!(
            h.content_hash,
            Header::from_bytes(&build_with_metadata(br#"{"a":1,"b":2}"#))
                .unwrap()
                .content_hash
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
    fn build_info_attaches_outside_the_hash() {
        let card = br#"{"title":"My Dataset"}"#;
        let base = build_with_metadata(card);
        let base_hash = Header::from_bytes(&base).unwrap().content_hash;

        let info = br#"{"built_at":"2026-08-04T00:00:00Z","builder":"rete-cli 0.3.2"}"#;
        let with = attach_build_info(&base, info).unwrap();

        // The section reads back verbatim, adjacent to the metadata.
        assert_eq!(read_build_info(&with).unwrap().unwrap(), info);
        let h = Header::from_bytes(&with).unwrap();
        assert_eq!(h.build_info_offset, HEADER_LEN as u64 + card.len() as u64);
        assert_eq!(h.build_info_len, info.len() as u64);
        assert_eq!(h.expected_file_len(), Some(with.len() as u64));

        // OUTSIDE the hash: the content hash is unchanged and the file still
        // verifies — two builds of identical data stay hash-equal even though
        // their build-info differs.
        assert_eq!(h.content_hash, base_hash);
        assert!(verify(&with).unwrap());
        let other = attach_build_info(&base, br#"{"built_at":"1999-01-01T00:00:00Z"}"#).unwrap();
        assert_eq!(Header::from_bytes(&other).unwrap().content_hash, base_hash);

        // The graph still opens and answers at its shifted offsets.
        let rete = Rete::open(&with).unwrap();
        assert_eq!(
            rete.query(Some("Bob"), Some("knows"), Some("Carol")).len(),
            1
        );
        assert_eq!(rete.metadata(), Some(card.as_slice()));

        // Replacing an existing section works and removing it restores the
        // original image byte-for-byte (the strip-equality determinism proof).
        let replaced = attach_build_info(&with, br#"{"builder":"other"}"#).unwrap();
        assert_eq!(
            read_build_info(&replaced).unwrap().unwrap(),
            br#"{"builder":"other"}"#
        );
        assert!(verify(&replaced).unwrap());
        let stripped = attach_build_info(&replaced, &[]).unwrap();
        assert_eq!(stripped, base, "stripping build-info restores the image");
    }

    #[test]
    fn build_info_attaches_on_a_metadata_free_file() {
        // No card: the section slots in right after the header.
        let base = build_image();
        let info = b"{\"builder\":\"x\"}";
        let with = attach_build_info(&base, info).unwrap();
        let h = Header::from_bytes(&with).unwrap();
        assert_eq!(h.build_info_offset, HEADER_LEN as u64);
        assert_eq!(h.metadata_len, 0);
        assert_eq!(read_build_info(&with).unwrap().unwrap(), info);
        assert!(verify(&with).unwrap());
        assert_eq!(
            h.content_hash,
            Header::from_bytes(&base).unwrap().content_hash
        );
        Rete::open(&with).unwrap();
    }

    /// The contract a **streaming** rewriter needs: a plan is enough to rebuild
    /// the file from three spans — the header it hands back, the untouched
    /// `[HEADER_LEN, insert)` prefix, the new section, and the untouched
    /// `[tail_start, len)` tail — and the result is byte-identical to the
    /// in-memory splice. That is what lets a 17 GB file gain a build record
    /// with a 4 MiB buffer instead of 34 GB of RAM.
    #[test]
    fn a_plan_rebuilds_exactly_what_the_in_memory_splice_produces() {
        let card = br#"{"title":"My Dataset"}"#;
        let base = build_with_metadata(card);
        let base_hash = Header::from_bytes(&base).unwrap().content_hash;
        for start in [
            base.clone(),
            attach_build_info(&base, b"{\"a\":1}").unwrap(),
        ] {
            for info in [
                br#"{"builder":"rete-cli 0.3.2","query_costs":{}}"#.as_slice(), // longer
                b"{}".as_slice(),                                               // shorter
                b"".as_slice(),                                                 // removed
            ] {
                let plan = plan_build_info(&start, start.len() as u64, info.len() as u64).unwrap();
                let mut streamed = Vec::new();
                streamed.extend_from_slice(&plan.header);
                streamed.extend_from_slice(&start[HEADER_LEN..plan.insert as usize]);
                streamed.extend_from_slice(info);
                streamed.extend_from_slice(&start[plan.tail_start as usize..]);
                assert_eq!(streamed.len() as u64, plan.new_len);
                assert_eq!(
                    streamed,
                    attach_build_info(&start, info).unwrap(),
                    "a {}-byte section streamed must equal the same section spliced",
                    info.len()
                );
                // The point of the exercise: the file's identity is untouched.
                assert!(verify(&streamed).unwrap());
                let h = Header::from_bytes(&streamed).unwrap();
                assert_eq!(h.content_hash, base_hash);
                assert_eq!(h.expected_file_len(), Some(plan.new_len));
                let rete = Rete::open(&streamed).unwrap();
                assert_eq!(rete.metadata(), Some(card.as_slice()));
                assert_eq!(
                    rete.query(Some("Bob"), Some("knows"), Some("Carol")).len(),
                    1
                );
            }
        }
    }

    /// A plan is refused rather than trusted when the header's own numbers put
    /// the section outside the file — the header is attacker-controlled input.
    #[test]
    fn a_plan_refuses_a_header_that_overruns_the_file() {
        let base = build_with_metadata(br#"{"title":"x"}"#);
        assert!(plan_build_info(&base, 8, 16).is_err());
        assert!(plan_build_info(&base[..HEADER_LEN], base.len() as u64, 16).is_ok());
    }

    #[test]
    fn card_and_build_info_ranged_is_one_header_plus_one_range() {
        use crate::reader::{CountingReader, SliceReader};
        let card = vec![0xCDu8; 384];
        let info = vec![0xEFu8; 200];
        let bytes = attach_build_info(&build_with_metadata(&card), &info).unwrap();

        // Both sections in ONE coalesced range after the header — the CARD
        // tier's 1 header + 1 range budget holds with build info included.
        let r = CountingReader::new(SliceReader::new(&bytes));
        let (m, b) = read_card_and_build_info_ranged(&r).unwrap();
        assert_eq!(m.unwrap(), card);
        assert_eq!(b.unwrap(), info);
        assert_eq!(r.requests(), 2, "header + one coalesced range");
        assert_eq!(
            r.bytes_read(),
            HEADER_LEN as u64 + card.len() as u64 + info.len() as u64
        );

        // Card only: same two requests, no build info.
        let plain = build_with_metadata(&card);
        let rp = CountingReader::new(SliceReader::new(&plain));
        let (m, b) = read_card_and_build_info_ranged(&rp).unwrap();
        assert_eq!(m.unwrap(), card);
        assert!(b.is_none());
        assert_eq!(rp.requests(), 2);
        // Stated as the comparison every client depends on: carrying a build
        // record costs a reader NOTHING in requests. Any client that shows the
        // build record — the CLI, the browser card modal — can therefore do it
        // in the card's own budget, and does not need a second call.
        assert_eq!(
            r.requests(),
            rp.requests(),
            "build info must not cost an extra request",
        );

        // Neither: the header read alone answers.
        let none = build_image();
        let rn = CountingReader::new(SliceReader::new(&none));
        let (m, b) = read_card_and_build_info_ranged(&rn).unwrap();
        assert!(m.is_none() && b.is_none());
        assert_eq!(rn.requests(), 1);
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

    /// A named graph whose container exceeds [`NAMED_GRAPH_RESIDENT_MAX`]
    /// opens through [`open_index_container_lazy`] — the default graph's own
    /// machinery, at the container's offset. Drive that helper directly over a
    /// named graph's container range (the threshold itself is too large to
    /// cross cheaply in a unit test) and require identical matches.
    #[test]
    fn named_graph_container_opens_tile_lazily_like_the_root() {
        let mut db = DictionaryBuilder::new();
        let node = |n: u32| format!("<http://ex/n{n}>");
        let p = "<http://ex/p>".to_string();
        db.observe(&node(0), &p, &node(1));
        for i in 0..500u32 {
            db.observe(&node(1000 + i), &p, &node(2000 + i));
        }
        let dict = db.build();
        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode(&node(0), &p, &node(1)).unwrap());
        let mut g = GraphIndexBuilder::new().with_tile_budget(256); // many tiles
        for i in 0..500u32 {
            g.push(dict.encode(&node(1000 + i), &p, &node(2000 + i)).unwrap());
        }
        let named = vec![("<http://ex/g>".to_string(), g.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);
        let rete = Rete::open(&bytes).unwrap();
        let header = rete.header();

        // The single entry's container range: count varint, iri_len varint,
        // iri, container_len varint, then the container.
        let soff = header.named_graphs_offset as usize;
        let send = soff + header.named_graphs_len as usize;
        let sec = &bytes[soff..send];
        let (n, mut pos) = read_uvarint(sec).unwrap();
        assert_eq!(n, 1);
        let (ilen, u1) = read_uvarint(&sec[pos..]).unwrap();
        pos += u1 + ilen as usize;
        let (clen, u2) = read_uvarint(&sec[pos..]).unwrap();
        pos += u2;
        let container = ByteRange {
            offset: (soff + pos) as u64,
            len: clen,
        };

        use crate::reader::SliceReader;
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let reader: std::sync::Arc<dyn RangeReader + Send + Sync> =
            std::sync::Arc::new(SliceReader::new(leaked));
        let (lazy_idx, _, _) = open_index_container_lazy(
            &reader,
            container,
            header.block_codec,
            header.has_tile_synopsis(),
            1,
            header.perms,
        )
        .unwrap();
        let mut want = rete
            .graph_index("<http://ex/g>")
            .unwrap()
            .match_pattern((None, None, None));
        let mut got = lazy_idx.match_pattern((None, None, None));
        want.sort_unstable();
        got.sort_unstable();
        assert_eq!(got.len(), 500);
        assert_eq!(got, want);
        assert!(!lazy_idx.load_incomplete());
    }

    /// `dump_iter` must agree with `dump` exactly — and must be *lazy*: taking
    /// the first triple of a lazily range-read file may not drag the whole
    /// index across the reader, or the JS client's streaming quad cursor (which
    /// is this iterator, suspended between wasm calls) would be a materializing
    /// dump wearing a cursor's clothes.
    #[test]
    fn dump_iter_matches_dump_and_stops_early() {
        use crate::reader::{CountingReader, SliceReader};

        // Enough triples over tiny tiles to force many tiles per permutation.
        let triples: Vec<(String, String, String)> = (0..2000u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/p>".to_string(),
                    format!("<http://ex/o/{i:04}>"),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        // Same triples, same order, as the eager dump.
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.dump_iter(None).collect::<Vec<_>>(), rete.dump(None));
        // A missing named graph yields nothing rather than the default graph.
        assert_eq!(rete.dump_iter(Some("http://ex/nope")).count(), 0);

        // Lazily: taking one triple must cost far fewer bytes than draining.
        let read_bytes = |take: usize| -> u64 {
            let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
            let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
            let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
            let before = reader.bytes_read();
            let n = rete.dump_iter(None).take(take).count();
            assert_eq!(n, take.min(triples.len()));
            assert!(!rete.index_incomplete());
            reader.bytes_read() - before
        };
        let first_only = read_bytes(1);
        let everything = read_bytes(triples.len());
        // Always true: stopping after one triple cannot cost a full scan.
        assert!(
            first_only < everything,
            "dump_iter is not lazy: one triple read {first_only} B, a full scan {everything} B"
        );
        // The STRONG claim — under half — only holds once the payload dwarfs the
        // fixed open cost. This fixture is ~32 KB, and with `compression` off the
        // dictionary is essentially the whole file, so opening it at all already
        // pays most of `everything` (21 KB of 33 KB) and the ratio says nothing
        // about laziness. Asserting it unconditionally is how this test failed on
        // `--no-default-features` while passing everywhere else.
        #[cfg(feature = "compression")]
        assert!(
            first_only * 2 < everything,
            "dump_iter is not lazy: one triple read {first_only} B of the {everything} B a full scan reads"
        );

        // A FRESH open: these assertions fault tiles and the dictionary in,
        // and the laziness measurement above counts bytes on a COLD reader —
        // sharing one `rete` makes first_only and everything both ~0, so the
        // comparison silently stops testing anything.
        let fresh = Rete::open(&bytes).unwrap();
        // `dump_batch` is the resumable form the language clients pull through.
        // Driven to exhaustion it must reproduce the eager dump EXACTLY — same
        // triples, same order — for any batch size, including one so small that
        // every call ends on a subject boundary before reaching it.
        let drain = |max_quads: usize| {
            let (mut out, mut cursor, mut calls) = (Vec::new(), 0u32, 0);
            loop {
                let (batch, next, done) = fresh.dump_batch(None, cursor, max_quads);
                out.extend(batch);
                cursor = next;
                calls += 1;
                assert!(calls < 10_000, "cursor failed to advance");
                if done {
                    break out;
                }
            }
        };
        let eager = fresh.dump(None);
        for max in [1usize, 7, 128, 100_000] {
            assert_eq!(drain(max), eager, "batch size {max} changed the dump");
        }
        // An unknown graph is an empty, already-finished dump — not an error and
        // not the default graph leaking through.
        let (t, _, done) = fresh.dump_batch(Some("http://ex/nope"), 0, 16);
        assert!(t.is_empty() && done);
    }

    /// `query_batch` is `dump_batch` generalized to a PATTERN — the primitive the
    /// Java client's RDF4J cursor pulls through, one bounded batch per wasm call.
    /// Three things have to hold or the cursor is a materializing scan wearing a
    /// cursor's clothes: driven to exhaustion it must reproduce the eager
    /// `query_in_graph` for every pattern shape and every batch size; taking one
    /// row must cost far fewer bytes than draining; and every call must either
    /// yield a row or say `done`, so the caller can never spin.
    #[test]
    fn query_batch_matches_query_in_graph_and_stops_early() {
        use crate::reader::{CountingReader, SliceReader};

        // Enough triples over tiny tiles that a scan spans many tiles, with two
        // predicates and a repeated subject so bound patterns have real fanout.
        let triples: Vec<(String, String, String)> = (0..2000u32)
            .map(|i| {
                (
                    format!("<http://ex/s/{:04}>", i / 2),
                    format!("<http://ex/p/{}>", i % 2),
                    format!("<http://ex/o/{i:04}>"),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);
        let rete = Rete::open(&bytes).unwrap();

        let drain = |s: Option<&str>, p: Option<&str>, o: Option<&str>, max: usize| {
            let (mut out, mut cursor, mut calls) = (Vec::new(), 0u64, 0);
            loop {
                let (batch, next, done) = rete.query_batch(None, s, p, o, cursor, max);
                // The contract the client relies on: rows, or done. Never
                // "nothing yet, call again" — that is an unbounded spin.
                assert!(!batch.is_empty() || done, "empty non-final batch");
                out.extend(batch);
                cursor = next;
                calls += 1;
                assert!(calls < 20_000, "cursor failed to advance");
                if done {
                    break out;
                }
            }
        };
        let shapes: [(Option<&str>, Option<&str>, Option<&str>); 6] = [
            (None, None, None),
            (Some("<http://ex/s/0007>"), None, None),
            (None, Some("<http://ex/p/1>"), None),
            (None, None, Some("<http://ex/o/0042>")),
            (Some("<http://ex/s/0007>"), Some("<http://ex/p/0>"), None),
            (
                Some("<http://ex/s/0007>"),
                Some("<http://ex/p/0>"),
                Some("<http://ex/o/0014>"),
            ),
        ];
        for (s, p, o) in shapes {
            let eager = rete.query_in_graph(None, s, p, o);
            assert!(
                !eager.is_empty(),
                "fixture shape {s:?} {p:?} {o:?} matches nothing"
            );
            // The lazy pull form agrees exactly, order included.
            assert_eq!(
                rete.query_iter(None, s, p, o).collect::<Vec<_>>(),
                eager,
                "query_iter disagrees for {s:?} {p:?} {o:?}"
            );
            for max in [1usize, 3, 64, 100_000] {
                let mut got = drain(s, p, o, max);
                let mut want = eager.clone();
                // A batch is re-sorted canonically per batch, not globally, so
                // a bound pattern's batches can arrive in permutation order.
                got.sort();
                want.sort();
                assert_eq!(got, want, "batch size {max} changed {s:?} {p:?} {o:?}");
            }
            // The fully unbound scan routes to SPO, so batching is order-exact.
            if s.is_none() && p.is_none() && o.is_none() {
                assert_eq!(drain(s, p, o, 7), eager, "unbound batching reordered rows");
            }
        }

        // A bound term the dictionary has never seen, and an unknown graph, are
        // both finished-and-empty rather than errors or a leak of everything.
        let (t, _, done) = rete.query_batch(None, Some("<http://ex/nope>"), None, None, 0, 16);
        assert!(t.is_empty() && done);
        let (t, _, done) = rete.query_batch(Some("http://ex/nope"), None, None, None, 0, 16);
        assert!(t.is_empty() && done);
        assert_eq!(
            rete.query_iter(None, Some("<http://ex/nope>"), None, None)
                .count(),
            0
        );

        // Lazily: one row must not drag the whole scan across the reader. This
        // is the property that makes `LIMIT 1` bounded on a 48 GiB file.
        let read_bytes = |take: usize| -> u64 {
            let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
            let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
            let rete = Rete::open_ranged_lazy(reader.clone()).unwrap();
            let before = reader.bytes_read();
            let (mut n, mut cursor) = (0usize, 0u64);
            loop {
                let (batch, next, done) = rete.query_batch(None, None, None, None, cursor, 1);
                n += batch.len();
                cursor = next;
                if done || n >= take {
                    break;
                }
            }
            assert!(n >= take.min(triples.len()));
            assert!(!rete.index_incomplete());
            reader.bytes_read() - before
        };
        let first_only = read_bytes(1);
        let everything = read_bytes(triples.len());
        assert!(
            first_only < everything,
            "query_batch is not lazy: one row read {first_only} B, a full scan {everything} B"
        );
        #[cfg(feature = "compression")]
        assert!(
            first_only * 2 < everything,
            "query_batch is not lazy: one row read {first_only} B of the {everything} B a full scan reads"
        );
    }

    /// A batched cursor over a QUADS file must stay inside the graph it was
    /// given: the default graph and each named graph resume independently.
    #[test]
    fn query_batch_is_graph_scoped() {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in [
            ("Alice", "knows", "Bob"),
            ("Alice", "knows", "Carol"),
            ("Alice", "knows", "Dave"),
            ("Erin", "knows", "Frank"),
            ("Alice", "knows", "Zoe"),
        ] {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode("Alice", "knows", "Bob").unwrap());
        def.push(dict.encode("Alice", "knows", "Carol").unwrap());
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode("Alice", "knows", "Dave").unwrap());
        g1.push(dict.encode("Erin", "knows", "Frank").unwrap());
        let mut g2 = GraphIndexBuilder::new();
        g2.push(dict.encode("Alice", "knows", "Zoe").unwrap());
        let named = vec![
            ("http://ex/g1".to_string(), g1.build()),
            ("http://ex/g2".to_string(), g2.build()),
        ];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);
        let rete = Rete::open(&bytes).unwrap();
        for graph in [None, Some("http://ex/g1"), Some("http://ex/g2")] {
            let eager = rete.query_in_graph(graph, None, None, None);
            let (mut out, mut cursor) = (Vec::new(), 0u64);
            loop {
                let (batch, next, done) = rete.query_batch(graph, None, None, None, cursor, 1);
                out.extend(batch);
                cursor = next;
                if done {
                    break;
                }
            }
            assert_eq!(out, eager, "graph {graph:?} batched differently");
        }
    }

    #[test]
    fn query_in_graph_is_graph_scoped() {
        // Default graph: Alice knows Bob, Alice knows Carol.
        // Named g1: Alice knows Dave (same predicate, different graph).
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in [
            ("Alice", "knows", "Bob"),
            ("Alice", "knows", "Carol"),
            ("Alice", "knows", "Dave"),
        ] {
            db.observe(s, p, o);
        }
        let dict = db.build();

        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode("Alice", "knows", "Bob").unwrap());
        def.push(dict.encode("Alice", "knows", "Carol").unwrap());
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode("Alice", "knows", "Dave").unwrap());

        let named = vec![("http://ex/g1".to_string(), g1.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);
        let rete = Rete::open(&bytes).unwrap();

        // Default graph only: the two default-graph objects, not Dave.
        let mut def_objs: Vec<String> = rete
            .query_in_graph(None, Some("Alice"), Some("knows"), None)
            .into_iter()
            .map(|(_, _, o)| o)
            .collect();
        def_objs.sort();
        assert_eq!(def_objs, vec!["Bob".to_string(), "Carol".to_string()]);

        // Named graph only: just Dave.
        assert_eq!(
            rete.query_in_graph(Some("http://ex/g1"), Some("Alice"), None, None),
            vec![("Alice".into(), "knows".into(), "Dave".into())]
        );

        // A wildcard-everything scan is scoped to its graph.
        assert_eq!(rete.query_in_graph(None, None, None, None).len(), 2);
        assert_eq!(
            rete.query_in_graph(Some("http://ex/g1"), None, None, None)
                .len(),
            1
        );

        // An unknown graph IRI is empty, not an error.
        assert!(rete
            .query_in_graph(Some("http://ex/missing"), None, None, None)
            .is_empty());
    }

    /// A **bounded** `query_in_graph` — and a dump of one named graph — must not
    /// fault the whole dictionary on a lazily range-read file.
    ///
    /// Both used to call `Dictionary::prefetch_all`, so the graph-scoped pattern
    /// primitive an RDF4J `Sail` calls once per named graph paid for every term
    /// in the file to answer a handful of rows. Measured on `cordis.rete`
    /// (801 MB, a 417 MB dictionary) before the fix: a 15-row answer read
    /// 416 MB and peaked at 2.5 GB RSS. The fixture below is the same shape in
    /// miniature — long literals so the dictionary dominates the file — and the
    /// assertion is the invariant that regressed: bytes read must stay well
    /// under the dictionary section. Lazy answers are compared against eager
    /// throughout: cheaper is only interesting if it is also correct.
    #[test]
    fn bounded_query_in_graph_does_not_fault_the_whole_dictionary() {
        use crate::reader::{CountingReader, SliceReader};

        // Long, distinct, poorly-compressible object literals: the dictionary is
        // the payload here, exactly as on a graph that stores abstracts or
        // embedded media. Repeating one filler string would not do — front
        // coding plus zstd shrink 2000 near-identical literals to ~6 KB, which
        // is smaller than the index and makes the measurement meaningless.
        let payload = |i: u32| -> String {
            let mut s = String::with_capacity(800);
            let mut x = i as u64 ^ 0x9e37_79b9_7f4a_7c15;
            while s.len() < 800 {
                x = x
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.push_str(&format!("{:016x}", x ^ (x >> 29)));
            }
            s
        };
        // Half the quads go to the named graph, and the two halves are kept
        // lexically apart (`a…` / `b…`) so each lands in its own dictionary
        // chunks — the real layout on a quads file whose graphs use different
        // IRI namespaces, e.g. `cordis.rete`. Interleaving them instead would
        // put a term of both graphs in every chunk, and then no per-graph
        // access pattern could avoid faulting the whole dictionary.
        let quads: Vec<(String, String, String, bool)> = (0..2000u32)
            .map(|i| {
                let named = i % 2 == 0;
                let ns = if named { 'a' } else { 'b' };
                (
                    format!("<http://ex/{ns}/s/{i:04}>"),
                    "<http://ex/abstract>".to_string(),
                    format!("\"{ns}{} number {i}\"", payload(i)),
                    named,
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o, _) in &quads {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut def = GraphIndexBuilder::new().with_tile_budget(256);
        let mut g1 = GraphIndexBuilder::new().with_tile_budget(256);
        for (s, p, o, named) in &quads {
            let t = dict.encode(s, p, o).unwrap();
            if *named {
                g1.push(t);
            } else {
                def.push(t);
            }
        }
        let named = vec![("http://ex/g1".to_string(), g1.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);

        let eager = Rete::open(&bytes).unwrap();
        let dict_len = eager.header().dictionary_len;

        // `probe` runs one closure against a COLD lazy open and reports the
        // bytes it faulted beyond the open itself.
        let leaked: &'static [u8] = Box::leak(bytes.clone().into_boxed_slice());
        let probe = |f: &dyn Fn(&Rete) -> Vec<TermTriple>| -> (Vec<TermTriple>, u64) {
            let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
            let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
            let before = reader.bytes_read();
            let out = f(&lazy);
            assert!(!lazy.index_incomplete(), "a chunk fetch failed");
            (out, reader.bytes_read() - before)
        };

        // 1. One bound subject in the named graph.
        let want = eager.query_in_graph(
            Some("http://ex/g1"),
            Some("<http://ex/a/s/0100>"),
            None,
            None,
        );
        assert_eq!(want.len(), 1, "fixture: expected exactly one match");
        let (got, pulled) = probe(&|r| {
            r.query_in_graph(
                Some("http://ex/g1"),
                Some("<http://ex/a/s/0100>"),
                None,
                None,
            )
        });
        assert_eq!(got, want, "lazy query_in_graph disagrees with eager");
        assert!(
            pulled * 4 < dict_len,
            "a 1-row query_in_graph faulted {pulled} B of a {dict_len} B dictionary — \
             it is prefetching the whole thing again"
        );

        // 2. A graph with NOTHING to resolve must not fault the dictionary.
        //    Both shapes below used to pull every chunk before returning an
        //    empty answer, and `rete export` hits the second one first on a
        //    quads file whose default graph is empty — on `cordis.rete` that
        //    was 415 MB read and 2.5 GB of peak RSS to emit zero triples. What
        //    is left is the named-graph directory walk it takes to learn the
        //    graph is absent at all; that is index, not dictionary.
        let (empty, q_pulled) =
            probe(&|r| r.query_in_graph(Some("http://ex/nope"), None, None, None));
        assert!(empty.is_empty());
        let (_, d_pulled) = probe(&|r| {
            let mut n = 0usize;
            r.dump_each(Some("http://ex/nope"), |_, _, _| n += 1);
            assert_eq!(n, 0, "an absent graph yielded triples");
            Vec::new()
        });
        assert_eq!(
            q_pulled, d_pulled,
            "query_in_graph and dump_each disagree on what an absent graph costs"
        );
        assert!(
            q_pulled * 4 < dict_len,
            "an absent graph faulted {q_pulled} B of a {dict_len} B dictionary"
        );

        // 3. Dumping ONE named graph costs that graph, not the whole file: the
        //    default graph's half of the dictionary must stay unfaulted.
        let want: Vec<TermTriple> = eager.dump(Some("http://ex/g1"));
        assert_eq!(want.len(), 1000);
        let (got, pulled) = probe(&|r| {
            let mut out = Vec::new();
            r.dump_each(Some("http://ex/g1"), |s, p, o| {
                out.push((s.to_string(), p.to_string(), o.to_string()))
            });
            out
        });
        assert_eq!(got, want, "lazy dump_each disagrees with eager dump");
        assert!(
            pulled * 3 < dict_len * 2,
            "dumping half the graph faulted {pulled} B of a {dict_len} B dictionary — \
             the other graph's chunks are being pulled in too"
        );
    }

    #[test]
    fn query_quads_tags_every_graph() {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in [("Alice", "knows", "Bob"), ("Alice", "knows", "Dave")] {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode("Alice", "knows", "Bob").unwrap());
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode("Alice", "knows", "Dave").unwrap());
        let named = vec![("http://ex/g1".to_string(), g1.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);
        let rete = Rete::open(&bytes).unwrap();

        // `Alice knows ?` spans both graphs; each match carries its graph tag.
        let quads = rete.query_quads(Some("Alice"), Some("knows"), None);
        assert_eq!(quads.len(), 2);
        assert_eq!(
            quads[0],
            (("Alice".into(), "knows".into(), "Bob".into()), None)
        );
        assert_eq!(
            quads[1],
            (
                ("Alice".into(), "knows".into(), "Dave".into()),
                Some("http://ex/g1".to_string())
            )
        );

        // A bound term absent from the dictionary yields nothing, in any graph.
        assert!(rete.query_quads(Some("Nobody"), None, None).is_empty());
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
            crate::ingest::assemble_dataset_with_opts(quads, true, false, None, |_, _| Vec::new());

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

    /// Build an in-memory `.rete` with `n` labeled subjects: each carries an
    /// `rdfs:label` literal drawn from `WORDS` (a word prefix selects ~1/|WORDS|
    /// of them) plus one extra edge so the subject has a degree to rank by.
    fn build_labeled(n: usize) -> Vec<u8> {
        const LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
        const WORDS: &[&str] = &[
            "alanine",
            "benzene",
            "glucose",
            "dextrose",
            "ethanol",
            "formate",
            "heptane",
            "isoleucine",
        ];
        let triples: Vec<(String, String, String)> = (0..n)
            .flat_map(|i| {
                let s = format!("<http://ex/e{i}>");
                let w = WORDS[i % WORDS.len()];
                [
                    (s.clone(), LABEL.to_string(), format!("\"{w}-{i:06}\"")),
                    (
                        s,
                        "<http://ex/p>".to_string(),
                        format!("<http://ex/c{}>", i % 64),
                    ),
                ]
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<(u32, u32, u32)> = triples
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
    fn prefix_search_matches_a_filter_scan() {
        // 800 < the 8192 label-index cap, so the index is COMPLETE — every label
        // is present and the two paths must return the exact same subject set.
        let bytes = build_labeled(800);
        let rete = Rete::open(&bytes).unwrap();
        let idx_subjects: std::collections::BTreeSet<String> = rete
            .prefix_search("glucose", 10_000)
            .into_iter()
            .map(|(_label, subject)| subject)
            .collect();
        // The same selection via a SPARQL FILTER scan over every label literal.
        let q = "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l \
                 FILTER(STRSTARTS(LCASE(?l), \"glucose\")) }";
        let crate::QueryOutput::Select(_, rows) = crate::eval_query(&rete, q).unwrap() else {
            panic!("expected SELECT");
        };
        let scan_subjects: std::collections::BTreeSet<String> =
            rows.iter().map(|r| r.get("s").cloned().unwrap()).collect();
        assert_eq!(idx_subjects, scan_subjects, "index agrees with the scan");
        assert_eq!(
            idx_subjects.len(),
            100,
            "800/8 words = 100 glucose-* labels"
        );
    }

    /// Latency: the binary-search label index vs the FILTER scan it replaces.
    /// Ignored by default (timing-sensitive); run with
    /// `cargo test -p rete-core -- --ignored --nocapture bench_prefix_search`.
    #[test]
    #[ignore]
    fn bench_prefix_search_vs_filter_scan() {
        use std::time::Instant;
        let n = 6000; // < the 8192 cap, so both paths return identical sets
        let bytes = build_labeled(n);
        let rete = Rete::open(&bytes).unwrap();
        let q = "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l \
                 FILTER(STRSTARTS(LCASE(?l), \"glucose\")) }";
        let reps = 200;
        let idx_n = rete.prefix_search("glucose", 100_000).len();
        let t = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(rete.prefix_search("glucose", 100_000));
        }
        let idx_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        let t = Instant::now();
        for _ in 0..reps {
            let _ = std::hint::black_box(crate::eval_query(&rete, q).unwrap());
        }
        let scan_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        println!(
            "label prefix search over {n} labeled subjects ({idx_n} matches): \
             index {idx_ms:.4} ms vs FILTER scan {scan_ms:.3} ms ({:.0}× faster)",
            scan_ms / idx_ms
        );
    }

    /// Latency: the TEXT_INDEX word search vs the `FILTER(CONTAINS(?l, …))` scan
    /// it replaces. Ignored by default (timing-sensitive); run with
    /// `cargo test -p rete-core -- --ignored --nocapture bench_text_search`.
    #[test]
    #[ignore]
    fn bench_text_search_vs_contains_scan() {
        use std::time::Instant;
        const LABEL: &str = "<http://www.w3.org/2000/01/rdf-schema#label>";
        const WORDS: &[&str] = &[
            "alanine",
            "benzene",
            "glucose",
            "dextrose",
            "ethanol",
            "formate",
            "heptane",
            "isoleucine",
        ];
        let n = 6000;
        let triples: Vec<(String, String, String)> = (0..n)
            .map(|i| {
                (
                    format!("<http://ex/e{i}>"),
                    LABEL.to_string(),
                    format!("\"{} sample number {i:06}\"", WORDS[i % WORDS.len()]),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<(u32, u32, u32)> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let ti = compute_text_index(&dict, &ids);
        let bytes = write_dataset_with_metadata(&dict, &ib.build(), &[], false, &[], 0, &[], &ti);
        let rete = Rete::open(&bytes).unwrap();

        let q = "SELECT ?s WHERE { ?s <http://www.w3.org/2000/01/rdf-schema#label> ?l \
                 FILTER(CONTAINS(LCASE(?l), \"glucose\")) }";
        let reps = 200;
        let idx_n = rete.text_search(&["glucose"], None, 100_000).len();
        let t = Instant::now();
        for _ in 0..reps {
            std::hint::black_box(rete.text_search(&["glucose"], None, 100_000));
        }
        let idx_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        let t = Instant::now();
        for _ in 0..reps {
            let _ = std::hint::black_box(crate::eval_query(&rete, q).unwrap());
        }
        let scan_ms = t.elapsed().as_secs_f64() * 1000.0 / reps as f64;
        println!(
            "text search over {n} literals ({idx_n} matches): \
             index {idx_ms:.4} ms vs FILTER(CONTAINS) scan {scan_ms:.3} ms ({:.0}× faster)",
            scan_ms / idx_ms
        );
    }

    /// Operational debugging harness for a REAL on-disk file (ignored; driven
    /// by env vars): step-by-step dump of a bound (p, o) POS routing.
    ///   RETE_DEBUG_FILE=<path.rete> RETE_DEBUG_P=<iri> RETE_DEBUG_O=<iri>
    #[test]
    #[ignore = "operational tool, driven by RETE_DEBUG_* env vars"]
    fn debug_bound_po_routing() {
        struct FR(std::fs::File);
        impl crate::RangeReader for FR {
            fn len(&self) -> u64 {
                self.0.metadata().map(|m| m.len()).unwrap_or(0)
            }
            fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
                use std::os::unix::fs::FileExt;
                let mut buf = vec![0u8; len as usize];
                self.0.read_exact_at(&mut buf, offset)?;
                Ok(buf)
            }
        }
        let path = std::env::var("RETE_DEBUG_FILE").expect("RETE_DEBUG_FILE");
        let p_iri = std::env::var("RETE_DEBUG_P").expect("RETE_DEBUG_P");
        let o_iri = std::env::var("RETE_DEBUG_O").expect("RETE_DEBUG_O");
        let rete =
            Rete::open_ranged_lazy(std::sync::Arc::new(FR(std::fs::File::open(&path).unwrap())))
                .unwrap();
        let pid = rete.dict.predicate_id(&p_iri).expect("p resolves");
        let oid = rete.dict.object_id(&o_iri).expect("o resolves");
        eprintln!("pid={pid} oid={oid}");
        let pattern = (None, Some(pid), Some(oid));
        let perm = GraphIndex::best_permutation(pattern);
        eprintln!("best_permutation = {}", perm.name());
        let si = perm.section_index();
        let tiles = &rete.index.sections[si];
        eprintln!("section {} tiles = {}", perm.name(), tiles.len());
        let [pa, pb, pc] = perm.order_pattern(pattern);
        eprintln!("permuted pattern pa={pa:?} pb={pb:?} pc={pc:?}");
        let (start, end) = rete.index.tile_span(si, pa);
        eprintln!("tile_span = [{start}, {end}) -> {} tiles", end - start);
        let mut admitted = 0usize;
        for (ti, t) in tiles.iter().enumerate().take(end).skip(start) {
            if t.syn_admits(pb, pc) {
                admitted += 1;
                if admitted <= 10 {
                    let (lo, hi) = t.leading_range();
                    eprintln!("  admit tile {ti}: a=[{lo},{hi}] syn={:?}", t.syn);
                }
            }
        }
        eprintln!("admitted {admitted} tile(s) by synopsis");
        let n = rete.index.scan_iter(pattern).count();
        eprintln!("scan_iter matches = {n}");
        let hi_res = rete.query(None, Some(&p_iri), Some(&o_iri));
        eprintln!("high-level query matches = {}", hi_res.len());
    }
}
