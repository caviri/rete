//! `.rete` file assembly and reading (SPEC.md Â§4, Â§9).
//!
//! v0 layout:
//!
//! ```text
//! [0..1024)  header ([`crate::header::HEADER_LEN`])
//! [dict]     dictionary container: 4 front-coded sections
//! [index]    permutation container: 6 triple blocks (SPO/POS/OSP/SOP/PSO/OPS)
//! [pyramid]  summary meta (and, in future, tile directories)
//! [footer]   trailing magic
//! ```
//!
//! The header points at the dictionary container (`dictionary_offset/len`) and
//! the permutation container (`root_dir_offset/len`); routed readers can fetch a
//! single permutation payload from that container.

use crate::adaptive::ReadIntent;
use crate::build_pipeline::family::{FamilyView, IndexFamily, Synopsis};
use crate::dictionary::Dictionary;
use crate::header::{
    Header, FLAG_HAS_QUADS, FLAG_HAS_QUOTED_TRIPLES, FLAG_TILE_SYNOPSIS, HEADER_LEN,
    LEGACY_FORMAT_VERSION, MAGIC, NEXT_FORMAT_VERSION,
};
use crate::index::{GraphIndex, IndexPermutation, Pattern, PermSet, NUM_PERMS};
use crate::meta::{ClassNode, CommunityDescriptor, LevelLinks, LevelRollup, PyramidMeta};
use crate::pyramid::{build_dendrogram, project_graph, PyramidAlgo};
use crate::reader::{checked_resident_range, materializable_len, RangeReader};
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
    // The community partition — the only step that differs by algorithm.
    let louvain = || {
        let g = project_graph(dict, triples);
        build_dendrogram(&g)
    };
    let dend = match algo {
        PyramidAlgo::Louvain => louvain(),
        PyramidAlgo::Types => {
            match crate::schema_pyramid::build_type_dendrogram(dict, triples, type_override) {
                Some(d) => d,
                None => {
                    eprintln!(
                        "  [pyramid] --pyramid-algo types: no usable rdf:type \
                         predicate — falling back to louvain"
                    );
                    louvain()
                }
            }
        }
    };
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
    let predicate_stats = compute_predicate_stats(triples);
    let char_sets = compute_char_sets(triples);
    let label_index = compute_label_index(dict, triples);
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
    (meta.encode(), dend.rounds() as u16)
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

/// Fetch a set of ascending, disjoint byte ranges, coalescing ranges according
/// to the physical source's session-local adaptive plan. Readers without a
/// controller retain the legacy static-gap behavior. Returns each requested
/// range's bytes in order; `None` if any read fails or the input ranges are not
/// ascending and disjoint.
fn read_coalesced<R: RangeReader + ?Sized>(
    reader: &R,
    ranges: &[ByteRange],
    static_gap: u64,
    intent: ReadIntent,
) -> Option<Vec<Vec<u8>>> {
    // Every requested range is later returned as its own Vec. Reject a length
    // this target cannot represent before asking a cache or network reader to
    // enumerate/materialize it.
    for range in ranges {
        materializable_len(range.len).ok()?;
    }
    let known_bytes = ranges
        .iter()
        .try_fold(0u64, |sum, range| sum.checked_add(range.len))?;
    let adaptive = reader.adaptive_controller();
    let plan = adaptive
        .as_ref()
        .map(|controller| controller.plan(intent, known_bytes, static_gap, reader.concurrency()));
    let coalesce_gap = plan.as_ref().map_or(static_gap, |plan| plan.coalesce_gap);
    let max_span = plan.as_ref().map_or(u64::MAX, |plan| plan.max_span);
    let gap_budget = plan
        .as_ref()
        .map_or(u64::MAX, |_| (known_bytes / 4).min(256 * 1024));

    // Build the coalesced spans and remember which span each input range maps
    // into, so the fetched span blobs can be sliced back apart in order.
    let mut spans: Vec<(u64, u64)> = Vec::new();
    let mut span_of: Vec<usize> = Vec::with_capacity(ranges.len());
    let mut gap_bytes = 0u64;
    let mut i = 0;
    while i < ranges.len() {
        let start = ranges[i].offset;
        let mut end = ranges[i].offset.checked_add(ranges[i].len)?;
        let mut j = i + 1;
        while j < ranges.len() {
            let r = &ranges[j];
            if r.offset < end {
                return None;
            }
            let next_end = r.offset.checked_add(r.len)?;
            let next_gap = r.offset - end;
            let next_gap_bytes = gap_bytes.checked_add(next_gap)?;
            let next_span = next_end.checked_sub(start)?;
            if next_gap > coalesce_gap || next_gap_bytes > gap_budget || next_span > max_span {
                break;
            }
            gap_bytes = next_gap_bytes;
            end = next_end;
            j += 1;
        }
        let si = spans.len();
        let span_len = end.checked_sub(start)?;
        materializable_len(span_len).ok()?;
        spans.push((start, span_len));
        for _ in i..j {
            span_of.push(si);
        }
        i = j;
    }
    let blobs = reader.read_many_with_intent(&spans, intent).ok()?;
    if blobs.len() != spans.len() {
        return None;
    }
    let mut out = Vec::with_capacity(ranges.len());
    for (k, r) in ranges.iter().enumerate() {
        let (span_start, _) = spans[span_of[k]];
        let blob = &blobs[span_of[k]];
        let lo = usize::try_from(r.offset.checked_sub(span_start)?).ok()?;
        let hi = lo.checked_add(materializable_len(r.len).ok()?)?;
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

/// What a filtered dump will cost, before it runs — see
/// [`Rete::dump_plan`](crate::Rete::dump_plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DumpPlan {
    /// The routed scan this dump will run. `None` when it provably matches
    /// nothing without touching the index: an absent graph IRI, or a bound term
    /// the dictionary does not contain.
    pub scan: Option<crate::index::ScanPlan>,
    /// The file's whole dictionary section — the **ceiling** on what resolving
    /// the matched rows' terms can fault, and on a literal-heavy file the
    /// dominant term in a dump's real cost.
    ///
    /// It is deliberately not an estimate. A dump that matches one row faults
    /// only the chunks that row's three terms live in, but nothing in the tile
    /// directories says which chunks those are or how big they are, and a
    /// preview that guessed would be worse than one that states the bound.
    pub dictionary_bytes: u64,
}

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
fn decode_container(
    bytes: &[u8],
    codec: u8,
    expected_sections: usize,
) -> Result<Vec<Vec<u8>>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated count"))?;
    if n != u64::try_from(expected_sections)
        .map_err(|_| FileError::Container("expected section count too large"))?
    {
        return Err(FileError::Container("unexpected container section count"));
    }
    // Dictionary and index containers have fixed production section counts.
    // Validate that count before reserving even one output element.
    let mut out = Vec::with_capacity(expected_sections);
    for _ in 0..expected_sections {
        let (len, used) = read_uvarint(bytes.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated length"))?;
        pos = pos
            .checked_add(used)
            .ok_or(FileError::Container("section offset overflows"))?;
        let len = materializable_len(len)
            .map_err(|_| FileError::Container("section length too large"))?;
        let end = pos
            .checked_add(len)
            .ok_or(FileError::Container("section range overflows"))?;
        let payload = bytes
            .get(pos..end)
            .ok_or(FileError::Container("section overruns buffer"))?;
        out.push(decompress(codec, payload)?);
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
///  [num_chunks; per chunk: Î”first_run, key_len, key, comp_len]
///  [individually compressed run-aligned body slices]`.
/// The header keeps its original encoding, so restart offsets stay valid in
/// the section's coordinate space.
///
/// `key` is the chunk's **routing separator**, not its first term: the
/// shortest string strictly above the previous chunk's last term and at most
/// the chunk's own first term (empty for chunk 0). See
/// [`crate::dict::SectionChunk::key`] for the invariant and
/// [`crate::dict::shortest_separator`] for the choice. Storing the first term
/// verbatim — what this wrote before — is the degenerate case of the same
/// invariant, so readers of either vintage route both correctly; the
/// separator is simply orders of magnitude smaller when the boundary term is a
/// long literal.
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
    // The previous chunk's last term — the separator's lower bound. Decoding it
    // walks at most `restart_interval` front-coded entries of one chunk, which
    // is nothing beside the zstd pass over the same 64 KiB.
    let mut prev_last: Option<Vec<u8>> = None;
    for (i, (&(first_run, start, end), comp)) in bounds.iter().zip(&compressed).enumerate() {
        let key = if i == 0 {
            // Everything routes to chunk 0; it needs no separator at all.
            Vec::new()
        } else {
            let first = crate::dict::run_first_term(raw, start as usize).unwrap_or_default();
            match &prev_last {
                // Undecodable predecessor: the verbatim first term is always a
                // valid separator, just not a short one.
                None => first,
                Some(pl) => crate::dict::shortest_separator(pl, &first),
            }
        };
        if i + 1 < bounds.len() {
            // The last run of THIS chunk starts one run before the next chunk's.
            let last_run_off = meta.restart_offsets[bounds[i + 1].0 - 1] as usize;
            prev_last = crate::dict::run_last_term(raw, last_run_off, end as usize);
        }
        write_uvarint(&mut out, (first_run - prev_run) as u64);
        write_uvarint(&mut out, key.len() as u64);
        out.extend_from_slice(&key);
        write_uvarint(&mut out, comp.len() as u64);
        prev_run = first_run;
    }
    for comp in &compressed {
        out.extend_from_slice(comp);
    }
    out
}

/// A parsed chunked-dict-section directory entry: the chunk's run/key/body
/// coordinates plus its compressed byte range *within the payload*. `key` is a
/// routing separator, not a term — see [`crate::dict::SectionChunk::key`].
struct DictChunkEntry {
    first_run: usize,
    key: Vec<u8>,
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
        *pos = pos
            .checked_add(n)
            .ok_or(FileError::Container("dict directory offset overflows"))?;
        Ok(v)
    };
    let header_len = usize::try_from(take(&mut pos)?)
        .map_err(|_| FileError::Container("dict header length too large"))?;
    let header_end = pos
        .checked_add(header_len)
        .ok_or(FileError::Container("dict directory offset overflows"))?;
    let header = bytes
        .get(pos..header_end)
        .ok_or(FileError::Container("truncated dict header"))?;
    let meta = crate::dict::parse_meta_header_fragment(header)
        .map_err(|_| FileError::Container("malformed dict header"))?;
    pos = header_end;

    let num_chunks = usize::try_from(take(&mut pos)?)
        .map_err(|_| FileError::Container("dict chunk count too large"))?;
    let expected_runs = u64::from(meta.term_count).div_ceil(u64::from(meta.restart_interval));
    if (expected_runs == 0) != (num_chunks == 0) {
        return Err(FileError::Container(
            "dict chunk count disagrees with restart table",
        ));
    }
    if num_chunks > bytes.len().saturating_sub(pos) / 3 {
        return Err(FileError::Container("dict chunk count exceeds directory"));
    }
    let mut entries = Vec::with_capacity(num_chunks.min(bytes.len()));
    let mut lens = Vec::with_capacity(num_chunks.min(bytes.len()));
    let mut prev_run = None;
    for chunk_index in 0..num_chunks {
        let drun = take(&mut pos)?;
        let klen = usize::try_from(take(&mut pos)?)
            .map_err(|_| FileError::Container("dict chunk term length too large"))?;
        let key_end = pos
            .checked_add(klen)
            .ok_or(FileError::Container("dict directory offset overflows"))?;
        let key = bytes
            .get(pos..key_end)
            .ok_or(FileError::Container("truncated dict chunk key"))?
            .to_vec();
        pos = key_end;
        let clen = take(&mut pos)?;
        materializable_len(clen)
            .map_err(|_| FileError::Container("dict chunk length too large"))?;
        let first_run = prev_run
            .unwrap_or(0u64)
            .checked_add(drun)
            .ok_or(FileError::Container("dict chunk run overflows"))?;
        if (chunk_index == 0 && first_run != 0)
            || first_run >= expected_runs
            || prev_run.is_some_and(|previous| first_run <= previous)
        {
            return Err(FileError::Container("dict chunk run outside restart table"));
        }
        let first_run = u32::try_from(first_run)
            .map_err(|_| FileError::Container("dict chunk run exceeds u32"))?;
        let first_run = usize::try_from(first_run)
            .map_err(|_| FileError::Container("dict chunk run exceeds usize"))?;
        let body_start = meta
            .restart_offsets
            .get(first_run)
            .copied()
            .ok_or(FileError::Container("dict chunk run out of range"))?;
        entries.push(DictChunkEntry {
            first_run,
            key,
            body_start,
            start: 0,
            end: 0,
        });
        lens.push(clen);
        prev_run = Some(
            u64::try_from(first_run)
                .map_err(|_| FileError::Container("dict chunk run exceeds u64"))?,
        );
    }
    let mut start =
        u64::try_from(pos).map_err(|_| FileError::Container("dict directory offset too large"))?;
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
    let interval_start = hbase
        .checked_add(n1)
        .ok_or(FileError::Container("dict header offset overflows"))?;
    let (restart_interval, n2) = read_uvarint(head.get(interval_start..).unwrap_or(&[]))
        .ok_or(FileError::Container("truncated dict interval"))?;
    if restart_interval == 0 {
        return Err(FileError::Container("zero restart interval"));
    }
    let header_len_usize = usize::try_from(header_len)
        .map_err(|_| FileError::Container("dict header length too large"))?;
    let header_end = hbase
        .checked_add(header_len_usize)
        .ok_or(FileError::Container("dict header offset overflows"))?;
    let restart_count_start = interval_start
        .checked_add(n2)
        .ok_or(FileError::Container("dict header offset overflows"))?;
    let (num_restarts, n3) = read_uvarint(head.get(restart_count_start..).unwrap_or(&[]))
        .ok_or(FileError::Container("truncated dict restart count"))?;
    let restart_count_end = restart_count_start
        .checked_add(n3)
        .filter(|&end| end <= header_end)
        .ok_or(FileError::Container("dict restart count overruns header"))?;
    if u64::try_from(header_end - restart_count_end)
        .ok()
        .filter(|&available| available >= num_restarts)
        .is_none()
    {
        return Err(FileError::Container("dict restart table overruns header"));
    }
    // The chunk directory begins right after the header body — i.e. past the
    // `header_len` bytes, which include the restart table we never materialize.
    let dir_start = (hbase as u64)
        .checked_add(header_len)
        .filter(|&d| d <= total)
        .ok_or(FileError::Container("dict header overruns section"))?;
    let dir_total = total - dir_start;
    let meta = crate::dict::SectionMeta {
        term_count: u32::try_from(term_count)
            .map_err(|_| FileError::Container("dict term count exceeds u32"))?,
        restart_interval: u32::try_from(restart_interval)
            .map_err(|_| FileError::Container("dict restart interval exceeds u32"))?,
        restart_offsets: Vec::new(),
    };
    let expected_restarts = u64::from(meta.term_count).div_ceil(u64::from(meta.restart_interval));
    if num_restarts != expected_restarts {
        return Err(FileError::Container(
            "dict restart count disagrees with term count",
        ));
    }
    let finish = |mut entries: Vec<DictChunkEntry>| {
        for e in &mut entries {
            e.start = e
                .start
                .checked_add(dir_start)
                .ok_or(FileError::Container("dict chunk offset overflows"))?;
            e.end = e
                .end
                .checked_add(dir_start)
                .ok_or(FileError::Container("dict chunk offset overflows"))?;
        }
        Ok((meta.clone(), entries))
    };
    // Fast path: the directory already sits in the prefix we read (small section
    // — its restart table is tiny, so the ~few KiB over-read is negligible).
    // A short prefix is not wasted: those bytes seed the probe below.
    let mut have: Vec<u8> = Vec::new();
    if dir_start < head.len() as u64 {
        let dir_start_usize = usize::try_from(dir_start)
            .map_err(|_| FileError::Container("dict directory offset too large"))?;
        match parse_chunk_dir_only(&head[dir_start_usize..], dir_total, expected_restarts)? {
            ChunkDirParse::Done(entries) => return finish(entries),
            ChunkDirParse::Truncated { .. } => {
                have = head[dir_start_usize..].to_vec();
            }
        }
    }
    // Big section: range-read the directory on its own, skipping the table.
    let directory_offset = section
        .offset
        .checked_add(dir_start)
        .ok_or(FileError::Container("dict directory offset overflows"))?;
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
            let read_offset = directory_offset
                .checked_add(held)
                .ok_or(FileError::Container("dict directory offset overflows"))?;
            let read_len = want - held;
            materializable_len(read_len)
                .map_err(|_| FileError::Container("dict directory length too large"))?;
            let extra = reader.read_at(read_offset, read_len)?;
            if extra.is_empty() {
                return Err(FileError::Container("truncated dict chunk directory"));
            }
            have.extend_from_slice(&extra);
        }
        let held = have.len() as u64;
        match parse_chunk_dir_only(&have, dir_total, expected_restarts)? {
            ChunkDirParse::Done(entries) => return finish(entries),
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
/// `[num_chunks][per chunk: Δfirst_run, key_len, key, comp_len]`.
/// Chunk byte ranges (`start`/`end`) come back relative to the directory's own
/// start; `body_start` is 0 (a lite section never uses it — lookups derive run
/// offsets per chunk). Bodies aren't needed here, so `dir` may end at the first
/// body as long as it covers the whole directory. A `dir` that stops mid-entry
/// is reported as [`ChunkDirParse::Truncated`], not an error: only bytes that
/// cannot be a directory at any length (a chunk range overrunning the section)
/// fail.
fn parse_chunk_dir_only(
    dir: &[u8],
    dir_total: u64,
    expected_runs: u64,
) -> Result<ChunkDirParse, FileError> {
    let mut pos = 0usize;
    let take = |pos: &mut usize| -> Result<Option<u64>, FileError> {
        let remaining = dir.get(*pos..).unwrap_or(&[]);
        let Some((v, n)) = read_uvarint(remaining) else {
            let terminates = remaining.iter().take(10).any(|byte| byte & 0x80 == 0);
            if terminates || remaining.len() >= 10 {
                return Err(FileError::Container("malformed dict chunk directory"));
            }
            return Ok(None);
        };
        *pos = pos
            .checked_add(n)
            .ok_or(FileError::Container("dict directory offset overflows"))?;
        Ok(Some(v))
    };
    let Some(num_chunks) = take(&mut pos)? else {
        return Ok(ChunkDirParse::Truncated {
            parsed: 0,
            used: 0,
            total: 0,
        });
    };
    let num_chunks = usize::try_from(num_chunks)
        .map_err(|_| FileError::Container("dict chunk count too large"))?;
    if (expected_runs == 0) != (num_chunks == 0) {
        return Err(FileError::Container(
            "dict chunk count disagrees with restart table",
        ));
    }
    if u64::try_from(num_chunks)
        .ok()
        .filter(|count| count.saturating_mul(3) <= dir_total)
        .is_none()
    {
        return Err(FileError::Container("dict chunk count exceeds directory"));
    }
    let initial = num_chunks.min(dir.len().saturating_sub(pos) / 3);
    let mut entries = Vec::with_capacity(initial);
    let mut lens = Vec::with_capacity(initial);
    let mut prev_run = None;
    for chunk_index in 0..num_chunks {
        let entry_start = pos;
        let short = ChunkDirParse::Truncated {
            parsed: entries.len(),
            used: entry_start,
            total: num_chunks,
        };
        let Some(drun) = take(&mut pos)? else {
            return Ok(short);
        };
        let Some(klen) = take(&mut pos)? else {
            return Ok(short);
        };
        let klen = usize::try_from(klen)
            .map_err(|_| FileError::Container("dict chunk term length too large"))?;
        let key_end = pos
            .checked_add(klen)
            .ok_or(FileError::Container("dict directory offset overflows"))?;
        if u64::try_from(key_end)
            .ok()
            .filter(|&end| end <= dir_total)
            .is_none()
        {
            return Err(FileError::Container("dict chunk key overruns section"));
        }
        let Some(key) = dir.get(pos..key_end) else {
            return Ok(short);
        };
        let key = key.to_vec();
        pos = key_end;
        let Some(clen) = take(&mut pos)? else {
            return Ok(short);
        };
        materializable_len(clen)
            .map_err(|_| FileError::Container("dict chunk length too large"))?;
        let first_run = prev_run
            .unwrap_or(0u64)
            .checked_add(drun)
            .ok_or(FileError::Container("dict chunk run overflows"))?;
        if (chunk_index == 0 && first_run != 0)
            || first_run >= expected_runs
            || prev_run.is_some_and(|previous| first_run <= previous)
        {
            return Err(FileError::Container("dict chunk run outside restart table"));
        }
        let first_run = u32::try_from(first_run)
            .map_err(|_| FileError::Container("dict chunk run exceeds u32"))?;
        let first_run = usize::try_from(first_run)
            .map_err(|_| FileError::Container("dict chunk run exceeds usize"))?;
        entries.push(DictChunkEntry {
            first_run,
            key,
            body_start: 0,
            start: 0,
            end: 0,
        });
        lens.push(clen);
        prev_run = Some(first_run as u64);
    }
    let mut start =
        u64::try_from(pos).map_err(|_| FileError::Container("dict directory offset too large"))?;
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
                e.key,
                e.body_start,
                decompress(
                    codec,
                    payload
                        .get(
                            usize::try_from(e.start)
                                .map_err(|_| FileError::Container("dict chunk offset too large"))?
                                ..usize::try_from(e.end).map_err(|_| {
                                    FileError::Container("dict chunk offset too large")
                                })?,
                        )
                        .ok_or(FileError::Container("dict chunk overruns section"))?,
                )?,
            ))
        })
        .collect::<Result<Vec<_>, FileError>>()?;
    Ok(crate::dict::ChunkedSection::from_parts(meta, chunks, None))
}

fn decode_dictionary_container(bytes: &[u8], codec: u8) -> Result<Dictionary, FileError> {
    let dsecs = decode_container(bytes, CODEC_NONE, 4)?;
    let mut sections = Vec::with_capacity(4);
    for sec in &dsecs {
        sections.push(decode_chunked_dict_section(sec, codec)?);
    }
    let arr: [crate::dict::ChunkedSection; 4] = sections
        .try_into()
        .map_err(|_| FileError::Container("expected 4 dictionary sections"))?;
    Ok(Dictionary::from_chunked_sections(arr))
}

// --- 0x06 paired-family container -------------------------------------------
//
// Ordinary six-permutation writers emit this layout. Transitional readers
// retain the independent six-section 0x05 dispatch for existing and external
// memory-bounded files.

#[allow(dead_code)]
const PREFIX2_FORMAT_BUDGET: usize = 64 * 1024;
/// A staged family record is one physical tile.  This independent cap keeps a
/// hostile compressed record from growing without bound before `TripleBlock`
/// gets a chance to validate it.
#[allow(dead_code)]
const FAMILY_TILE_DECOMPRESSED_MAX: usize = 64 * 1024;
#[allow(dead_code)]
const FAMILY_FLAG_CONTINUES_PREVIOUS: u8 = 0b0000_0001;
#[allow(dead_code)]
const FAMILY_FLAG_CONTINUES_NEXT: u8 = 0b0000_0010;
#[allow(dead_code)]
const FAMILY_FLAG_MASK: u8 = FAMILY_FLAG_CONTINUES_PREVIOUS | FAMILY_FLAG_CONTINUES_NEXT;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) struct Prefix2Group {
    pub a: u32,
    pub a_body_offset: u32,
    pub b_entries: Vec<(u32, u32, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) struct Prefix2Meta {
    pub groups: Vec<Prefix2Group>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[allow(dead_code)]
pub(crate) struct FamilyDirectory {
    pub ranges: Vec<(u32, u32)>,
    pub first_flags: Vec<u8>,
    pub second_flags: Vec<u8>,
    pub first_lengths: Vec<u64>,
    pub second_lengths: Vec<u64>,
    pub first_prefix2_lengths: Vec<u64>,
    pub second_prefix2_lengths: Vec<u64>,
    pub first_prefix2: Vec<Prefix2Meta>,
    pub second_prefix2: Vec<Prefix2Meta>,
    pub first_synopses: Vec<Synopsis>,
    pub second_synopses: Vec<Synopsis>,
    pub first_records_offset: u64,
    pub second_records_offset: u64,
}

#[allow(dead_code)]
pub(crate) struct DecodedFamily {
    pub first: Vec<crate::index::Tile>,
    pub second: Vec<crate::index::Tile>,
    pub directory: FamilyDirectory,
}

#[allow(dead_code)]
fn family_compress(codec: u8, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
    match codec {
        CODEC_NONE => Ok(bytes.to_vec()),
        CODEC_ZSTD => {
            #[cfg(feature = "compression")]
            {
                use std::io::Write;
                let mut encoder =
                    zstd::stream::Encoder::new(Vec::new(), ZSTD_LEVEL).map_err(FileError::Io)?;
                // Match the staged decoder's fixed physical-tile window.
                encoder.window_log(16).map_err(FileError::Io)?;
                encoder.write_all(bytes).map_err(FileError::Io)?;
                encoder.finish().map_err(FileError::Io)
            }
            #[cfg(not(feature = "compression"))]
            {
                let _ = bytes;
                Err(FileError::UnknownCodec(codec))
            }
        }
        other => Err(FileError::UnknownCodec(other)),
    }
}

#[allow(dead_code)]
fn family_continuation_flags(ranges: &[(u32, u32)]) -> Vec<u8> {
    ranges
        .iter()
        .enumerate()
        .map(|(i, &(min_a, max_a))| {
            let repeated_previous = i > 0 && ranges[i - 1] == (min_a, max_a);
            let repeated_next = ranges.get(i + 1).copied() == Some((min_a, max_a));
            let mut flags = 0;
            if repeated_previous {
                flags |= FAMILY_FLAG_CONTINUES_PREVIOUS;
            }
            if repeated_next {
                flags |= FAMILY_FLAG_CONTINUES_NEXT;
            }
            flags
        })
        .collect()
}

#[allow(dead_code)]
fn encode_prefix2(tile: &crate::index::Tile) -> Result<Vec<u8>, FileError> {
    let block = crate::triples::TripleBlock::parse(tile.bytes())
        .map_err(|_| FileError::Container("malformed tile for prefix-2"))?;
    block
        .validate_complete()
        .map_err(|_| FileError::Container("malformed tile for prefix-2"))?;
    let Some(groups) = block.group_directory().complete_prefix2() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    write_uvarint(&mut out, groups.len() as u64);
    let mut previous_a = 0u32;
    for (a, a_body_offset, b_entries) in groups {
        let a_delta = a
            .checked_sub(previous_a)
            .ok_or(FileError::Container("non-monotone prefix-2 a"))?;
        write_uvarint(&mut out, a_delta as u64);
        write_uvarint(&mut out, a_body_offset as u64);
        write_uvarint(&mut out, b_entries.len() as u64);
        previous_a = a;
        let mut previous_b = 0u32;
        for (b, c_body_offset, c_count) in b_entries {
            let b_delta = b
                .checked_sub(previous_b)
                .ok_or(FileError::Container("non-monotone prefix-2 b"))?;
            write_uvarint(&mut out, b_delta as u64);
            write_uvarint(&mut out, c_body_offset as u64);
            write_uvarint(&mut out, c_count as u64);
            previous_b = b;
        }
    }
    if out.len() > PREFIX2_FORMAT_BUDGET {
        return Ok(Vec::new());
    }
    Ok(out)
}

#[allow(dead_code)]
fn synopsis_for(tile: &crate::index::Tile) -> Result<Synopsis, FileError> {
    let block = crate::triples::TripleBlock::parse(tile.bytes())
        .map_err(|_| FileError::Container("malformed family tile"))?;
    block
        .validate_complete()
        .map_err(|_| FileError::Container("malformed family tile"))?;
    let zone = block.zone();
    Ok((zone.min_b, zone.max_b, zone.min_c, zone.max_c))
}

/// Encode one paired physical family. The payload contains its own shared
/// ranges, sibling directories, records, and synopsis trailers.
#[allow(dead_code)]
pub(crate) fn encode_family_container(
    family: FamilyView<'_>,
    codec: u8,
) -> Result<Vec<u8>, FileError> {
    debug_assert_eq!(NEXT_FORMAT_VERSION, LEGACY_FORMAT_VERSION + 1);
    if family.first.len() != family.second.len() {
        return Err(FileError::Container("family tile count mismatch"));
    }
    let count = family.first.len();
    let mut ranges = Vec::with_capacity(count);
    for (first, second) in family.first.iter().zip(family.second) {
        let range = first.leading_range();
        if range != second.leading_range() {
            return Err(FileError::Container("family sibling ranges differ"));
        }
        ranges.push(range);
    }
    for pair in ranges.windows(2) {
        if pair[0] == pair[1] {
            if pair[1].0 != pair[1].1 {
                return Err(FileError::Container(
                    "continuation range is not a single leading group",
                ));
            }
        } else if pair[1].0 <= pair[0].1 {
            return Err(FileError::Container("family ranges overlap"));
        }
    }
    let flags = family_continuation_flags(&ranges);
    let mut first_prefix2 = Vec::with_capacity(count);
    let mut second_prefix2 = Vec::with_capacity(count);
    let mut first_payloads = Vec::with_capacity(count);
    let mut second_payloads = Vec::with_capacity(count);
    let mut first_synopses = Vec::with_capacity(count);
    let mut second_synopses = Vec::with_capacity(count);
    for tile in family.first {
        if tile.bytes().len() > FAMILY_TILE_DECOMPRESSED_MAX {
            return Err(FileError::Container(
                "family tile exceeds fixed decompressed limit",
            ));
        }
        first_prefix2.push(encode_prefix2(tile)?);
        first_payloads.push(family_compress(codec, tile.bytes())?);
        first_synopses.push(synopsis_for(tile)?);
    }
    for tile in family.second {
        if tile.bytes().len() > FAMILY_TILE_DECOMPRESSED_MAX {
            return Err(FileError::Container(
                "family tile exceeds fixed decompressed limit",
            ));
        }
        second_prefix2.push(encode_prefix2(tile)?);
        second_payloads.push(family_compress(codec, tile.bytes())?);
        second_synopses.push(synopsis_for(tile)?);
    }

    let mut directory_bytes = Vec::new();
    let mut previous_min = 0u32;
    for &(min_a, max_a) in &ranges {
        let delta = min_a
            .checked_sub(previous_min)
            .ok_or(FileError::Container("family ranges are not monotone"))?;
        let span = max_a
            .checked_sub(min_a)
            .ok_or(FileError::Container("family range underflows"))?;
        write_uvarint(&mut directory_bytes, delta as u64);
        write_uvarint(&mut directory_bytes, span as u64);
        previous_min = min_a;
    }
    for ((&flag, payload), prefix2) in flags.iter().zip(&first_payloads).zip(&first_prefix2) {
        write_uvarint(&mut directory_bytes, flag as u64);
        write_uvarint(&mut directory_bytes, payload.len() as u64);
        write_uvarint(&mut directory_bytes, prefix2.len() as u64);
    }
    for ((&flag, payload), prefix2) in flags.iter().zip(&second_payloads).zip(&second_prefix2) {
        write_uvarint(&mut directory_bytes, flag as u64);
        write_uvarint(&mut directory_bytes, payload.len() as u64);
        write_uvarint(&mut directory_bytes, prefix2.len() as u64);
    }

    let mut trailer_bytes = Vec::new();
    for &(min_b, max_b, min_c, max_c) in &first_synopses {
        write_uvarint(&mut trailer_bytes, min_b as u64);
        write_uvarint(&mut trailer_bytes, (max_b - min_b) as u64);
        write_uvarint(&mut trailer_bytes, min_c as u64);
        write_uvarint(&mut trailer_bytes, (max_c - min_c) as u64);
    }
    for &(min_b, max_b, min_c, max_c) in &second_synopses {
        write_uvarint(&mut trailer_bytes, min_b as u64);
        write_uvarint(&mut trailer_bytes, (max_b - min_b) as u64);
        write_uvarint(&mut trailer_bytes, min_c as u64);
        write_uvarint(&mut trailer_bytes, (max_c - min_c) as u64);
    }

    // Length-frame both metadata regions.  A ranged opener can fetch them in
    // two precise reads without crossing into either sibling's tile payloads.
    let mut out = Vec::new();
    write_uvarint(&mut out, count as u64);
    write_uvarint(&mut out, directory_bytes.len() as u64);
    write_uvarint(&mut out, trailer_bytes.len() as u64);
    out.extend_from_slice(&directory_bytes);
    for (prefix2, payload) in first_prefix2.iter().zip(&first_payloads) {
        out.extend_from_slice(prefix2);
        out.extend_from_slice(payload);
    }
    for (prefix2, payload) in second_prefix2.iter().zip(&second_payloads) {
        out.extend_from_slice(prefix2);
        out.extend_from_slice(payload);
    }
    out.extend_from_slice(&trailer_bytes);
    Ok(out)
}

/// Encode the internal root container: exactly Subject, Predicate, Object
/// family payloads, individually length-framed and never independently
/// compressed. The production `0x05` root container remains untouched.
#[allow(dead_code)]
pub(crate) fn encode_family_index_container(
    index: &GraphIndex,
    codec: u8,
) -> Result<Vec<u8>, FileError> {
    let payloads = [
        encode_family_container(index.family_view(IndexFamily::Subject), codec)?,
        encode_family_container(index.family_view(IndexFamily::Predicate), codec)?,
        encode_family_container(index.family_view(IndexFamily::Object), codec)?,
    ];
    let mut out = Vec::new();
    write_uvarint(&mut out, payloads.len() as u64);
    for payload in &payloads {
        write_uvarint(&mut out, payload.len() as u64);
        out.extend_from_slice(payload);
    }
    Ok(out)
}

#[allow(dead_code)]
fn family_varint(bytes: &[u8], pos: &mut usize, what: &'static str) -> Result<u64, FileError> {
    let mut value = 0u64;
    for byte_index in 0..10usize {
        let index = pos
            .checked_add(byte_index)
            .ok_or(FileError::Container("family offset overflows"))?;
        let byte = *bytes.get(index).ok_or(FileError::Container(what))?;
        let payload = u64::from(byte & 0x7f);
        if byte_index == 9 && payload > 1 {
            return Err(FileError::Container("family varint overflows u64"));
        }
        value |= payload << (byte_index * 7);
        if byte & 0x80 == 0 {
            let used = byte_index + 1;
            let canonical = if value < (1u64 << 7) {
                1
            } else {
                (64usize - value.leading_zeros() as usize).div_ceil(7)
            };
            if used != canonical {
                return Err(FileError::Container("non-canonical family varint"));
            }
            *pos = index
                .checked_add(1)
                .ok_or(FileError::Container("family offset overflows"))?;
            return Ok(value);
        }
    }
    Err(FileError::Container("family varint exceeds ten bytes"))
}

#[allow(dead_code)]
fn family_u32(bytes: &[u8], pos: &mut usize, what: &'static str) -> Result<u32, FileError> {
    u32::try_from(family_varint(bytes, pos, what)?).map_err(|_| FileError::Container(what))
}

#[allow(dead_code)]
fn family_bytes<'a>(
    bytes: &'a [u8],
    pos: &mut usize,
    len: u64,
    what: &'static str,
) -> Result<&'a [u8], FileError> {
    let len = usize::try_from(len).map_err(|_| FileError::Container(what))?;
    let end = pos
        .checked_add(len)
        .ok_or(FileError::Container("family offset overflows"))?;
    let result = bytes.get(*pos..end).ok_or(FileError::Container(what))?;
    *pos = end;
    Ok(result)
}

#[allow(dead_code)]
fn decode_prefix2(bytes: &[u8]) -> Result<Prefix2Meta, FileError> {
    let mut pos = 0usize;
    let group_count = usize::try_from(family_varint(bytes, &mut pos, "truncated prefix-2 count")?)
        .map_err(|_| FileError::Container("prefix-2 count too large"))?;
    if group_count > bytes.len() / 3 {
        return Err(FileError::Container("prefix-2 count exceeds blob"));
    }
    let mut groups = Vec::with_capacity(group_count.min(bytes.len() / 3));
    let mut previous_a = 0u32;
    for _ in 0..group_count {
        let delta = family_u32(bytes, &mut pos, "malformed prefix-2 a delta")?;
        let a = previous_a
            .checked_add(delta)
            .ok_or(FileError::Container("prefix-2 a overflows u32"))?;
        if !groups.is_empty() && a <= previous_a {
            return Err(FileError::Container(
                "prefix-2 a is not strictly increasing",
            ));
        }
        let a_body_offset = family_u32(bytes, &mut pos, "malformed prefix-2 a offset")?;
        let b_count = usize::try_from(family_varint(
            bytes,
            &mut pos,
            "malformed prefix-2 b count",
        )?)
        .map_err(|_| FileError::Container("prefix-2 b count too large"))?;
        if b_count > bytes.len().saturating_sub(pos) / 3 {
            return Err(FileError::Container("prefix-2 b count exceeds blob"));
        }
        let mut b_entries = Vec::with_capacity(b_count.min(bytes.len().saturating_sub(pos) / 3));
        let mut previous_b = 0u32;
        for _ in 0..b_count {
            let delta = family_u32(bytes, &mut pos, "malformed prefix-2 b delta")?;
            let b = previous_b
                .checked_add(delta)
                .ok_or(FileError::Container("prefix-2 b overflows u32"))?;
            if !b_entries.is_empty() && b <= previous_b {
                return Err(FileError::Container(
                    "prefix-2 b is not strictly increasing",
                ));
            }
            let c_body_offset = family_u32(bytes, &mut pos, "malformed prefix-2 c offset")?;
            let c_count = family_u32(bytes, &mut pos, "malformed prefix-2 c count")?;
            b_entries.push((b, c_body_offset, c_count));
            previous_b = b;
        }
        groups.push(Prefix2Group {
            a,
            a_body_offset,
            b_entries,
        });
        previous_a = a;
    }
    if pos != bytes.len() {
        return Err(FileError::Container("trailing prefix-2 bytes"));
    }
    Ok(Prefix2Meta { groups })
}

#[allow(dead_code)]
fn validate_prefix2(meta: &Prefix2Meta, payload: &[u8]) -> Result<(), FileError> {
    let block = crate::triples::TripleBlock::parse(payload)
        .map_err(|_| FileError::Container("malformed family tile payload"))?;
    block
        .validate_complete()
        .map_err(|_| FileError::Container("malformed family tile payload"))?;
    for group in &meta.groups {
        if usize::try_from(group.a_body_offset)
            .ok()
            .filter(|&offset| offset < payload.len())
            .is_none()
        {
            return Err(FileError::Container("prefix-2 a offset outside tile"));
        }
        for &(_, c_body_offset, _) in &group.b_entries {
            if usize::try_from(c_body_offset)
                .ok()
                .filter(|&offset| offset < payload.len())
                .is_none()
            {
                return Err(FileError::Container("prefix-2 c offset outside tile"));
            }
        }
    }
    let Some(expected) = block.group_directory().complete_prefix2() else {
        return Err(FileError::Container(
            "prefix-2 exceeds complete directory budget",
        ));
    };
    let expected = Prefix2Meta {
        groups: expected
            .into_iter()
            .map(|(a, a_body_offset, b_entries)| Prefix2Group {
                a,
                a_body_offset,
                b_entries,
            })
            .collect(),
    };
    if &expected != meta {
        return Err(FileError::Container(
            "prefix-2 metadata does not match tile",
        ));
    }
    Ok(())
}

#[allow(dead_code)]
fn decode_family_record(
    bytes: &[u8],
    pos: &mut usize,
    prefix2_len: u64,
    compressed_len: u64,
    codec: u8,
) -> Result<(Prefix2Meta, Vec<u8>), FileError> {
    let prefix2 = family_bytes(bytes, pos, prefix2_len, "prefix-2 blob overruns family")?;
    let compressed = family_bytes(bytes, pos, compressed_len, "tile payload overruns family")?;
    let payload = decompress_family_tile_exact(codec, compressed)?;
    let meta = if prefix2.is_empty() {
        Prefix2Meta::default()
    } else {
        let meta = decode_prefix2(prefix2)?;
        validate_prefix2(&meta, &payload)?;
        meta
    };
    let block = crate::triples::TripleBlock::parse(&payload)
        .map_err(|_| FileError::Container("malformed family tile payload"))?;
    block
        .validate_complete()
        .map_err(|_| FileError::Container("malformed family tile payload"))?;
    Ok((meta, payload))
}

/// Family records are deliberately stricter than legacy compressed sections:
/// one declared slice must contain exactly one zstd frame and may decode to at
/// most one fixed-size physical tile.
#[allow(dead_code)]
fn decompress_family_tile_exact(codec: u8, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
    match codec {
        CODEC_NONE => {
            if bytes.len() > FAMILY_TILE_DECOMPRESSED_MAX {
                return Err(FileError::Container(
                    "family tile exceeds fixed decompressed limit",
                ));
            }
            Ok(bytes.to_vec())
        }
        CODEC_ZSTD => {
            use std::io::Read;
            // ruzstd allocates its declared window while constructing the
            // streaming decoder, so inspect the tiny frame header first.
            let (frame, _) = ruzstd::frame::read_frame_header(bytes)
                .map_err(|e| FileError::Decompress(std::io::Error::other(e.to_string())))?;
            let window = frame
                .header
                .window_size()
                .map_err(|e| FileError::Decompress(std::io::Error::other(e.to_string())))?;
            if window > FAMILY_TILE_DECOMPRESSED_MAX as u64 {
                return Err(FileError::Container(
                    "family zstd window exceeds fixed limit",
                ));
            }
            let content_size = frame.header.frame_content_size();
            if content_size != 0 && content_size > FAMILY_TILE_DECOMPRESSED_MAX as u64 {
                return Err(FileError::Container(
                    "family zstd content size exceeds fixed limit",
                ));
            }
            let mut decoder = ruzstd::StreamingDecoder::new(bytes)
                .map_err(|e| FileError::Decompress(std::io::Error::other(e.to_string())))?;
            let mut out = Vec::with_capacity(FAMILY_TILE_DECOMPRESSED_MAX.min(bytes.len()));
            let mut buf = [0u8; 8192];
            loop {
                let remaining = FAMILY_TILE_DECOMPRESSED_MAX
                    .checked_add(1)
                    .and_then(|limit| limit.checked_sub(out.len()))
                    .ok_or(FileError::Container(
                        "family tile exceeds fixed decompressed limit",
                    ))?;
                if remaining == 0 {
                    return Err(FileError::Container(
                        "family tile exceeds fixed decompressed limit",
                    ));
                }
                let read_len = remaining.min(buf.len());
                let read = decoder
                    .read(&mut buf[..read_len])
                    .map_err(FileError::Decompress)?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&buf[..read]);
            }
            if out.len() > FAMILY_TILE_DECOMPRESSED_MAX {
                return Err(FileError::Container(
                    "family tile exceeds fixed decompressed limit",
                ));
            }
            // `StreamingDecoder` can read ahead in its source; the frame
            // decoder's own consumed-source counter is the exact boundary.
            if decoder.decoder.bytes_read_from_source()
                != u64::try_from(bytes.len())
                    .map_err(|_| FileError::Container("family payload too large"))?
            {
                return Err(FileError::Container(
                    "family zstd payload has trailing bytes",
                ));
            }
            Ok(out)
        }
        other => Err(FileError::UnknownCodec(other)),
    }
}

#[allow(dead_code)]
fn decode_family_synopses(
    bytes: &[u8],
    pos: &mut usize,
    count: usize,
) -> Result<Vec<Synopsis>, FileError> {
    let mut synopses = Vec::with_capacity(count.min(bytes.len().saturating_sub(*pos) / 4));
    for _ in 0..count {
        let min_b = family_u32(bytes, pos, "truncated family synopsis")?;
        let span_b = family_u32(bytes, pos, "truncated family synopsis")?;
        let min_c = family_u32(bytes, pos, "truncated family synopsis")?;
        let span_c = family_u32(bytes, pos, "truncated family synopsis")?;
        synopses.push((
            min_b,
            min_b
                .checked_add(span_b)
                .ok_or(FileError::Container("family synopsis b overflows u32"))?,
            min_c,
            min_c
                .checked_add(span_c)
                .ok_or(FileError::Container("family synopsis c overflows u32"))?,
        ));
    }
    Ok(synopses)
}

/// Decode and fully validate one paired family payload. All count-dependent
/// allocations are bounded by bytes already present in this payload.
#[allow(dead_code)]
pub(crate) fn decode_family_container(bytes: &[u8], codec: u8) -> Result<DecodedFamily, FileError> {
    if !matches!(codec, CODEC_NONE | CODEC_ZSTD) {
        return Err(FileError::UnknownCodec(codec));
    }
    let mut pos = 0usize;
    let count = usize::try_from(family_varint(
        bytes,
        &mut pos,
        "truncated family tile count",
    )?)
    .map_err(|_| FileError::Container("family tile count too large"))?;
    let directory_len = family_varint(bytes, &mut pos, "truncated family directory length")?;
    let trailer_len = family_varint(bytes, &mut pos, "truncated family trailer length")?;
    let minimum_directory = u64::try_from(
        count
            .checked_mul(8)
            .ok_or(FileError::Container("family directory size overflows"))?,
    )
    .map_err(|_| FileError::Container("family directory size exceeds u64"))?;
    let minimum_trailer = u64::try_from(
        count
            .checked_mul(8)
            .ok_or(FileError::Container("family trailer size overflows"))?,
    )
    .map_err(|_| FileError::Container("family trailer size exceeds u64"))?;
    if directory_len < minimum_directory {
        return Err(FileError::Container("family tile count exceeds directory"));
    }
    if trailer_len < minimum_trailer {
        return Err(FileError::Container("family tile count exceeds trailer"));
    }
    let maximum_directory = minimum_directory
        .checked_mul(10)
        .ok_or(FileError::Container("family directory size overflows"))?;
    let maximum_trailer = minimum_trailer
        .checked_mul(10)
        .ok_or(FileError::Container("family trailer size overflows"))?;
    if directory_len > maximum_directory {
        return Err(FileError::Container(
            "family directory exceeds varint framing",
        ));
    }
    if trailer_len > maximum_trailer {
        return Err(FileError::Container(
            "family trailer exceeds varint framing",
        ));
    }
    let directory_bytes =
        family_bytes(bytes, &mut pos, directory_len, "truncated family directory")?;
    let mut directory_pos = 0usize;
    let mut directory = FamilyDirectory {
        ranges: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        first_flags: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        second_flags: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        first_lengths: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        second_lengths: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        first_prefix2_lengths: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        second_prefix2_lengths: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        first_prefix2: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        second_prefix2: Vec::with_capacity(count.min(directory_bytes.len() / 8)),
        first_synopses: Vec::new(),
        second_synopses: Vec::new(),
        first_records_offset: 0,
        second_records_offset: 0,
    };
    let mut previous_min = 0u32;
    for _ in 0..count {
        let delta = family_u32(
            directory_bytes,
            &mut directory_pos,
            "malformed family minimum delta",
        )?;
        let span = family_u32(
            directory_bytes,
            &mut directory_pos,
            "malformed family range span",
        )?;
        let min_a = previous_min
            .checked_add(delta)
            .ok_or(FileError::Container("family minimum overflows u32"))?;
        let max_a = min_a
            .checked_add(span)
            .ok_or(FileError::Container("family maximum overflows u32"))?;
        directory.ranges.push((min_a, max_a));
        previous_min = min_a;
    }
    for _ in 0..count {
        let flags = u8::try_from(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated first family directory",
        )?)
        .map_err(|_| FileError::Container("first family flags exceed u8"))?;
        if flags & !FAMILY_FLAG_MASK != 0 {
            return Err(FileError::Container("reserved first family flags"));
        }
        directory.first_flags.push(flags);
        directory.first_lengths.push(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated first family length",
        )?);
        directory.first_prefix2_lengths.push(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated first prefix-2 length",
        )?);
        if directory
            .first_prefix2_lengths
            .last()
            .copied()
            .unwrap_or_default()
            > PREFIX2_FORMAT_BUDGET as u64
        {
            return Err(FileError::Container("prefix-2 blob exceeds fixed budget"));
        }
    }
    for _ in 0..count {
        let flags = u8::try_from(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated second family directory",
        )?)
        .map_err(|_| FileError::Container("second family flags exceed u8"))?;
        if flags & !FAMILY_FLAG_MASK != 0 {
            return Err(FileError::Container("reserved second family flags"));
        }
        directory.second_flags.push(flags);
        directory.second_lengths.push(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated second family length",
        )?);
        directory.second_prefix2_lengths.push(family_varint(
            directory_bytes,
            &mut directory_pos,
            "truncated second prefix-2 length",
        )?);
        if directory
            .second_prefix2_lengths
            .last()
            .copied()
            .unwrap_or_default()
            > PREFIX2_FORMAT_BUDGET as u64
        {
            return Err(FileError::Container("prefix-2 blob exceeds fixed budget"));
        }
    }
    for i in 0..count {
        if directory.first_flags[i] != directory.second_flags[i] {
            return Err(FileError::Container("sibling continuation flags differ"));
        }
        let repeated_previous = i > 0 && directory.ranges[i - 1] == directory.ranges[i];
        let repeated_next = i + 1 < count && directory.ranges[i + 1] == directory.ranges[i];
        if repeated_previous && directory.ranges[i].0 != directory.ranges[i].1 {
            return Err(FileError::Container(
                "continuation range is not a single leading group",
            ));
        }
        let flags = directory.first_flags[i];
        if (flags & FAMILY_FLAG_CONTINUES_PREVIOUS != 0) != repeated_previous
            || (flags & FAMILY_FLAG_CONTINUES_NEXT != 0) != repeated_next
        {
            return Err(FileError::Container("impossible family continuation"));
        }
        if i > 0 && !repeated_previous && directory.ranges[i].0 <= directory.ranges[i - 1].1 {
            return Err(FileError::Container("family ranges overlap"));
        }
    }
    if directory_pos != directory_bytes.len() {
        return Err(FileError::Container("trailing family directory bytes"));
    }
    directory.first_records_offset =
        u64::try_from(pos).map_err(|_| FileError::Container("family offset too large"))?;
    let mut first_records = Vec::with_capacity(count.min(bytes.len() / 16));
    for i in 0..count {
        first_records.push(decode_family_record(
            bytes,
            &mut pos,
            directory.first_prefix2_lengths[i],
            directory.first_lengths[i],
            codec,
        )?);
    }
    directory.second_records_offset =
        u64::try_from(pos).map_err(|_| FileError::Container("family offset too large"))?;
    let mut second_records = Vec::with_capacity(count.min(bytes.len() / 16));
    for i in 0..count {
        second_records.push(decode_family_record(
            bytes,
            &mut pos,
            directory.second_prefix2_lengths[i],
            directory.second_lengths[i],
            codec,
        )?);
    }
    let trailer_bytes = family_bytes(
        bytes,
        &mut pos,
        trailer_len,
        "truncated family synopsis trailer",
    )?;
    if pos != bytes.len() {
        return Err(FileError::Container("trailing family bytes"));
    }
    let mut trailer_pos = 0usize;
    directory.first_synopses = decode_family_synopses(trailer_bytes, &mut trailer_pos, count)?;
    directory.second_synopses = decode_family_synopses(trailer_bytes, &mut trailer_pos, count)?;
    if trailer_pos != trailer_bytes.len() {
        return Err(FileError::Container("trailing family synopsis bytes"));
    }
    for i in 0..count {
        for ((_, payload), synopsis) in [
            (&first_records[i], directory.first_synopses[i]),
            (&second_records[i], directory.second_synopses[i]),
        ] {
            let block = crate::triples::TripleBlock::parse(payload)
                .map_err(|_| FileError::Container("malformed family tile payload"))?;
            let zone = block.zone();
            if (zone.min_a, zone.max_a) != directory.ranges[i] {
                return Err(FileError::Container(
                    "family directory range disagrees with tile",
                ));
            }
            if (zone.min_b, zone.max_b, zone.min_c, zone.max_c) != synopsis {
                return Err(FileError::Container("family synopsis disagrees with tile"));
            }
        }
    }
    let first = first_records
        .into_iter()
        .zip(directory.ranges.iter().copied())
        .zip(directory.first_synopses.iter().copied())
        .map(|(((meta, payload), (min_a, max_a)), syn)| {
            let mut tile = crate::index::Tile::local(min_a, max_a, payload);
            tile.syn = Some(syn);
            (meta, tile)
        })
        .collect::<Vec<_>>();
    let second = second_records
        .into_iter()
        .zip(directory.ranges.iter().copied())
        .zip(directory.second_synopses.iter().copied())
        .map(|(((meta, payload), (min_a, max_a)), syn)| {
            let mut tile = crate::index::Tile::local(min_a, max_a, payload);
            tile.syn = Some(syn);
            (meta, tile)
        })
        .collect::<Vec<_>>();
    directory.first_prefix2 = first.iter().map(|(meta, _)| meta.clone()).collect();
    directory.second_prefix2 = second.iter().map(|(meta, _)| meta.clone()).collect();
    Ok(DecodedFamily {
        first: first.into_iter().map(|(_, tile)| tile).collect(),
        second: second.into_iter().map(|(_, tile)| tile).collect(),
        directory,
    })
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
type LogicalTileRanges = [Vec<(u32, u32, ByteRange)>; NUM_PERMS];
type LazyIndexOpen = (GraphIndex, [ByteRange; NUM_PERMS], LogicalTileRanges);

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

/// Fetch a tiled directory without crossing its exact byte boundary. Once the
/// tile count is known, every unfinished field requires at least one byte, so
/// bounded batches of that minimum size can advance without touching payloads.
fn read_tile_directory_ranged_exact<R: RangeReader>(
    reader: &R,
    section: ByteRange,
) -> Result<Vec<TileDirEntry>, FileError> {
    const DIRECTORY_BATCH: u64 = 4096;

    let section_end = checked_end(section.offset, section.len)?;
    let mut read_offset = section.offset;
    let mut count_bytes = Vec::with_capacity(10);
    let num_tiles = loop {
        if read_offset >= section_end || count_bytes.len() == 10 {
            return Err(FileError::Container("truncated tile directory"));
        }
        let one = reader.read_at_precise(read_offset, 1)?;
        if one.len() != 1 {
            return Err(FileError::Container("truncated tile directory"));
        }
        let byte = one
            .first()
            .copied()
            .ok_or(FileError::Container("truncated tile directory"))?;
        read_offset = checked_end(read_offset, 1)?;
        count_bytes.push(byte);
        if byte & 0x80 == 0 {
            if count_bytes.len() == 10 && byte > 1 {
                return Err(FileError::Container("tile count overflows u64"));
            }
            break read_uvarint(&count_bytes)
                .ok_or(FileError::Container("malformed tile count"))?
                .0;
        }
    };
    let num_tiles =
        usize::try_from(num_tiles).map_err(|_| FileError::Container("tile count too large"))?;
    let total_fields = num_tiles
        .checked_mul(3)
        .ok_or(FileError::Container("tile directory field count overflows"))?;
    let section_capacity = usize::try_from(section.len).unwrap_or(usize::MAX);
    // Neither the tile count nor a virtual section length is an allocation
    // bound. Keep only a small proven-safe initial reserve; grow in proportion
    // to directory records that were actually fetched and parsed.
    let initial_capacity = num_tiles
        .min(section_capacity)
        .min(DIRECTORY_BATCH as usize);
    let mut entries = Vec::with_capacity(initial_capacity);
    let mut lens = Vec::with_capacity(initial_capacity);
    let mut prev_min = 0u32;
    let mut field_index = 0usize;
    let mut pending = Vec::new();
    let mut pending_pos = 0usize;
    let mut fields = [0u64; 3];

    while field_index < total_fields {
        let available = &pending[pending_pos..];
        let parsed = read_uvarint(available)
            .and_then(|(value, used)| (used < 10 || available[9] <= 1).then_some((value, used)));
        if let Some((value, used)) = parsed {
            fields[field_index % 3] = value;
            pending_pos += used;
            field_index += 1;
            if field_index.is_multiple_of(3) {
                let dmin = u32::try_from(fields[0])
                    .map_err(|_| FileError::Container("tile minimum delta too large"))?;
                let span = u32::try_from(fields[1])
                    .map_err(|_| FileError::Container("tile leading span too large"))?;
                let min_a = prev_min
                    .checked_add(dmin)
                    .ok_or(FileError::Container("tile minimum overflows u32"))?;
                let max_a = min_a
                    .checked_add(span)
                    .ok_or(FileError::Container("tile maximum overflows u32"))?;
                entries.push(TileDirEntry {
                    min_a,
                    max_a,
                    start: 0,
                    end: 0,
                });
                lens.push(fields[2]);
                prev_min = min_a;
            }
            continue;
        }
        if available.len() >= 10 {
            return Err(FileError::Container("tile directory varint overflows"));
        }

        pending.drain(..pending_pos);
        pending_pos = 0;
        let unfinished_fields = total_fields - field_index;
        let fetch_len = (unfinished_fields as u64).min(DIRECTORY_BATCH);
        let fetch_end = checked_end(read_offset, fetch_len)?;
        if fetch_end > section_end {
            return Err(FileError::Container("truncated tile directory"));
        }
        let bytes = reader.read_at_precise(read_offset, fetch_len)?;
        if bytes.len() as u64 != fetch_len {
            return Err(FileError::Container("truncated tile directory"));
        }
        pending.extend_from_slice(&bytes);
        read_offset = fetch_end;
    }

    if pending_pos != pending.len() {
        return Err(FileError::Container("tile directory framing is ambiguous"));
    }
    let body_start = read_offset - section.offset;
    let mut start = body_start;
    for (entry, len) in entries.iter_mut().zip(lens) {
        let end = start
            .checked_add(len)
            .filter(|&end| end <= section.len)
            .ok_or(FileError::Container("tile overruns section"))?;
        entry.start = start;
        entry.end = end;
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
            Err(error) => return Err(error),
        }
    }
}

/// Fetch and parse a remote section's **tile-synopsis trailer** (only when the
/// header's [`FLAG_TILE_SYNOPSIS`] is set): one targeted, format-bounded range
/// read past the last tile, parsed into one synopsis per tile (directory order).
/// Each synopsis contains four uvarints, at most ten bytes each. A
/// missing/short/garbled trailer degrades to all-`None` — pruning simply doesn't
/// fire, never a wrong result. The directory gives the trailer's start (the last
/// tile's end).
fn read_tile_synopsis_ranged<R: RangeReader>(
    reader: &R,
    section: ByteRange,
    dir: &[TileDirEntry],
    precise: bool,
) -> Vec<Option<TileSynopsis>> {
    let n = dir.len();
    let none = vec![None; n];
    let trailer_start = dir.iter().map(|e| e.end).max().unwrap_or(0);
    let total = section.len;
    if n == 0 || trailer_start >= total {
        return none; // no trailer bytes present
    }
    let Some(max_synopsis_len) = u64::try_from(n)
        .ok()
        .and_then(|count| count.checked_mul(4 * 10))
    else {
        return none;
    };
    let trailer_len = (total - trailer_start).min(max_synopsis_len);
    let Ok(trailer_offset) = checked_end(section.offset, trailer_start) else {
        return none;
    };
    let bytes = if precise {
        reader.read_at_precise(trailer_offset, trailer_len)
    } else {
        reader.read_at(trailer_offset, trailer_len)
    };
    let Ok(bytes) = bytes else {
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
    let mut isecs = decode_container(bytes, CODEC_NONE, perms.len())?;
    let mut sections: [Vec<(u32, u32, Vec<u8>)>; NUM_PERMS] = Default::default();
    for (perm, sec) in perms.iter().zip(isecs.iter_mut()) {
        sections[perm.section_index()] = decode_tiled_section(sec, codec)?;
    }
    Ok(GraphIndex::from_tiles(sections, perms))
}

fn decode_family_index_container(bytes: &[u8], codec: u8) -> Result<GraphIndex, FileError> {
    let families = decode_container(bytes, CODEC_NONE, 3)?;
    let mut sections: [Vec<(u32, u32, Vec<u8>)>; NUM_PERMS] = Default::default();
    for (family, payload) in [
        IndexFamily::Subject,
        IndexFamily::Predicate,
        IndexFamily::Object,
    ]
    .into_iter()
    .zip(families)
    {
        let decoded = decode_family_container(&payload, codec)?;
        let slot = family.slot();
        sections[slot] = decoded
            .first
            .into_iter()
            .map(|tile| {
                let (min_a, max_a) = tile.leading_range();
                (min_a, max_a, tile.bytes().to_vec())
            })
            .collect();
        sections[slot + 3] = decoded
            .second
            .into_iter()
            .map(|tile| {
                let (min_a, max_a) = tile.leading_range();
                (min_a, max_a, tile.bytes().to_vec())
            })
            .collect();
    }
    Ok(GraphIndex::from_tiles(sections, PermSet::ALL))
}

fn decode_index_for_version(
    bytes: &[u8],
    codec: u8,
    perms: PermSet,
    version: u8,
) -> Result<GraphIndex, FileError> {
    match version {
        LEGACY_FORMAT_VERSION => decode_index_container(bytes, codec, perms),
        NEXT_FORMAT_VERSION if perms == PermSet::ALL => decode_family_index_container(bytes, codec),
        NEXT_FORMAT_VERSION => Err(FileError::Container(
            "paired-family generation requires all six logical permutations",
        )),
        _ => Err(FileError::Container("unsupported index generation")),
    }
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

fn decode_index_section_ranges_for_version(
    bytes: &[u8],
    container_offset: u64,
    perms: PermSet,
    version: u8,
) -> Result<[ByteRange; NUM_PERMS], FileError> {
    if version == LEGACY_FORMAT_VERSION {
        return decode_index_section_ranges(bytes, container_offset, perms);
    }
    let ranges = container_section_payload_ranges(bytes, container_offset, 3)?;
    let mut out = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
    for (family, range) in [
        IndexFamily::Subject,
        IndexFamily::Predicate,
        IndexFamily::Object,
    ]
    .into_iter()
    .zip(ranges)
    {
        let slot = family.slot();
        out[slot] = range;
        out[slot + 3] = range;
    }
    Ok(out)
}

fn family_tile_file_ranges(
    bytes: &[u8],
    container_offset: u64,
) -> Result<LogicalTileRanges, FileError> {
    let family_ranges = container_section_payload_ranges(bytes, container_offset, 3)?;
    let mut out: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    for (family, physical) in [
        IndexFamily::Subject,
        IndexFamily::Predicate,
        IndexFamily::Object,
    ]
    .into_iter()
    .zip(family_ranges)
    {
        let relative = physical
            .offset
            .checked_sub(container_offset)
            .ok_or(FileError::Container("family range precedes root"))?;
        let start = usize::try_from(relative)
            .map_err(|_| FileError::Container("family range too large"))?;
        let len = usize::try_from(physical.len)
            .map_err(|_| FileError::Container("family range too large"))?;
        let end = start
            .checked_add(len)
            .ok_or(FileError::Container("family range overflows"))?;
        let payload = bytes
            .get(start..end)
            .ok_or(FileError::Container("family range overruns root"))?;
        // The graph index was fully decoded and validated immediately before
        // this provenance pass.  Re-read only its small framed directory here;
        // decompressing all six payload streams a second time would double the
        // eager open cost merely to recover their physical byte ranges.
        let mut pos = 0usize;
        let count = usize::try_from(family_varint(
            payload,
            &mut pos,
            "truncated family tile count",
        )?)
        .map_err(|_| FileError::Container("family tile count too large"))?;
        let directory_len = family_varint(payload, &mut pos, "truncated family directory length")?;
        let trailer_len = family_varint(payload, &mut pos, "truncated family trailer length")?;
        let directory_bytes = family_bytes(
            payload,
            &mut pos,
            directory_len,
            "truncated family directory",
        )?;
        let mut directory_pos = 0usize;
        let mut ranges = Vec::with_capacity(count.min(directory_bytes.len() / 8));
        let mut previous_min = 0u32;
        for _ in 0..count {
            let delta = family_u32(
                directory_bytes,
                &mut directory_pos,
                "malformed family minimum delta",
            )?;
            let span = family_u32(
                directory_bytes,
                &mut directory_pos,
                "malformed family range span",
            )?;
            let min_a = previous_min
                .checked_add(delta)
                .ok_or(FileError::Container("family minimum overflows u32"))?;
            let max_a = min_a
                .checked_add(span)
                .ok_or(FileError::Container("family maximum overflows u32"))?;
            ranges.push((min_a, max_a));
            previous_min = min_a;
        }
        let mut parse_records = || {
            (0..count)
                .map(|_| {
                    let _flags = family_varint(
                        directory_bytes,
                        &mut directory_pos,
                        "truncated family flags",
                    )?;
                    let compressed = family_varint(
                        directory_bytes,
                        &mut directory_pos,
                        "truncated family length",
                    )?;
                    let prefix = family_varint(
                        directory_bytes,
                        &mut directory_pos,
                        "truncated family prefix-2 length",
                    )?;
                    Ok((prefix, compressed))
                })
                .collect::<Result<Vec<_>, FileError>>()
        };
        let first_directory = parse_records()?;
        let second_directory = parse_records()?;
        if directory_pos != directory_bytes.len() {
            return Err(FileError::Container("trailing family directory bytes"));
        }
        let records = |offset: u64, entries: &[(u64, u64)]| {
            let mut cursor = checked_end(physical.offset, offset)?;
            entries
                .iter()
                .copied()
                .zip(ranges.iter().copied())
                .map(|((prefix, compressed), (min_a, max_a))| {
                    let len = checked_end(prefix, compressed)?;
                    let range = ByteRange {
                        offset: cursor,
                        len,
                    };
                    cursor = checked_end(cursor, len)?;
                    Ok((min_a, max_a, range))
                })
                .collect::<Result<Vec<_>, FileError>>()
        };
        let slot = family.slot();
        let first_offset =
            u64::try_from(pos).map_err(|_| FileError::Container("family offset too large"))?;
        out[slot] = records(first_offset, &first_directory)?;
        let first_bytes = first_directory
            .iter()
            .try_fold(0u64, |total, &(prefix, compressed)| {
                checked_end(total, checked_end(prefix, compressed)?)
            })?;
        let second_offset = checked_end(first_offset, first_bytes)?;
        out[slot + 3] = records(second_offset, &second_directory)?;
        let second_bytes = second_directory
            .iter()
            .try_fold(0u64, |total, &(prefix, compressed)| {
                checked_end(total, checked_end(prefix, compressed)?)
            })?;
        let framed_end = checked_end(checked_end(second_offset, second_bytes)?, trailer_len)?;
        if framed_end != physical.len {
            return Err(FileError::Container(
                "family record lengths disagree with framing",
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
#[allow(dead_code)]
struct RemoteGraphIndex {
    index: GraphIndex,
    section_ranges: [ByteRange; NUM_PERMS],
    tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS],
}

/// View of a reader that preserves exact physical metadata ranges through
/// wrappers which normally widen/cache [`RangeReader::read_at`] calls. Payload
/// loaders keep the original reader and therefore retain ordinary caching.
struct PreciseMetadataReader<'a, R: RangeReader + ?Sized> {
    inner: &'a R,
}

impl<R: RangeReader + ?Sized> RangeReader for PreciseMetadataReader<'_, R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_at_precise(offset, len)
    }

    fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.inner.read_at_precise(offset, len)
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// Construct a tile-faulting graph index from on-disk framing. Named graphs set
/// `exact_payload_boundaries`: their lazy-open contract permits no payload
/// fetch at all. The established default-graph path keeps its 4 KiB directory
/// prefix to avoid adding dozens of high-latency metadata round trips.
#[cfg(test)]
fn open_remote_graph_index<R: RangeReader + Send + Sync + 'static>(
    reader: std::sync::Arc<R>,
    container: ByteRange,
    codec: u8,
    has_tile_synopsis: bool,
    read_concurrency: usize,
    exact_payload_boundaries: bool,
) -> Result<RemoteGraphIndex, FileError> {
    let adaptive_controller = reader.adaptive_controller();
    let section_ranges = if exact_payload_boundaries {
        locate_container_sections_ranged_exact::<R, NUM_PERMS>(reader.as_ref(), container)?
    } else {
        let mut ranges = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
        for (si, range) in ranges.iter_mut().enumerate() {
            *range = locate_container_section_ranged(
                reader.as_ref(),
                container.offset,
                container.len,
                si,
                NUM_PERMS as u64,
            )?;
        }
        ranges
    };
    let mut tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    #[allow(clippy::type_complexity)]
    let mut directories: [Vec<(u32, u32, Option<TileSynopsis>)>; NUM_PERMS] = Default::default();

    for si in 0..NUM_PERMS {
        let section = section_ranges[si];
        let dir = if exact_payload_boundaries {
            read_tile_directory_ranged_exact(reader.as_ref(), section)?
        } else {
            read_tile_directory_ranged(reader.as_ref(), section)?
        };
        let synopsis = if has_tile_synopsis {
            read_tile_synopsis_ranged(reader.as_ref(), section, &dir, exact_payload_boundaries)
        } else {
            vec![None; dir.len()]
        };
        directories[si] = dir
            .iter()
            .zip(synopsis)
            .map(|(entry, synopsis)| (entry.min_a, entry.max_a, synopsis))
            .collect();
        tile_ranges[si] = dir
            .into_iter()
            .map(|entry| {
                Ok((
                    entry.min_a,
                    entry.max_a,
                    ByteRange {
                        offset: checked_end(section.offset, entry.start)?,
                        len: entry.end - entry.start,
                    },
                ))
            })
            .collect::<Result<Vec<_>, FileError>>()?;
    }

    let loader_ranges = tile_ranges.clone();
    let loader_reader = reader.clone();
    let loader: crate::index::TileLoader = Box::new(move |si, ti| {
        let (_, _, range) = loader_ranges.get(si)?.get(ti)?;
        let compressed = loader_reader.read_at(range.offset, range.len).ok()?;
        if compressed.len() as u64 != range.len {
            return None;
        }
        let bytes = decompress(codec, &compressed).ok()?;
        crate::triples::TripleBlock::parse(&bytes).ok()?;
        Some(bytes)
    });
    let bulk_ranges = tile_ranges.clone();
    let bulk_reader = reader;
    let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis, intent| {
        let section = bulk_ranges.get(si)?;
        let want: Option<Vec<ByteRange>> = tis
            .iter()
            .map(|&ti| section.get(ti).map(|&(_, _, range)| range))
            .collect();
        let blobs = read_coalesced(bulk_reader.as_ref(), &want?, TILE_COALESCE_GAP, intent)?;
        blobs
            .iter()
            .map(|blob| {
                let bytes = decompress(codec, blob).ok()?;
                crate::triples::TripleBlock::parse(&bytes).ok()?;
                Some(bytes)
            })
            .collect()
    });
    let mut index = GraphIndex::from_remote_directories(directories, PermSet::ALL, loader)
        .with_bulk_loader(bulk);
    index.set_tile_lens(std::array::from_fn(|si| {
        tile_ranges[si]
            .iter()
            .map(|&(_, _, range)| range.len.min(u32::MAX as u64) as u32)
            .collect()
    }));
    index.set_read_concurrency(read_concurrency);
    index.set_adaptive_controller(adaptive_controller);

    Ok(RemoteGraphIndex {
        index,
        section_ranges,
        tile_ranges,
    })
}

#[cfg(test)]
fn open_named_graphs_ranged_lazy<R: RangeReader + Send + Sync + 'static>(
    reader: std::sync::Arc<R>,
    section: ByteRange,
    codec: u8,
    has_tile_synopsis: bool,
    read_concurrency: usize,
) -> Result<Vec<(String, GraphIndex)>, FileError> {
    let section_end = checked_end(section.offset, section.len)?;
    let (graph_count, count_len) =
        read_uvarint_at_exact(reader.as_ref(), section.offset, section_end)?;
    let graph_count = usize::try_from(graph_count)
        .map_err(|_| FileError::Container("named-graph count too large"))?;
    // The count is untrusted and the section may advertise a virtual u64-sized
    // range. Grow only after each complete, bounded graph record is parsed.
    let mut graphs = Vec::new();
    let mut pos = checked_end(section.offset, count_len)?;

    for _ in 0..graph_count {
        let (iri_len, iri_len_used) = read_uvarint_at_exact(reader.as_ref(), pos, section_end)?;
        pos = checked_end(pos, iri_len_used)?;
        let iri_end = checked_end(pos, iri_len)?;
        if iri_end > section_end {
            return Err(FileError::Container("named-graph IRI overruns section"));
        }
        let iri_len_usize = usize::try_from(iri_len)
            .map_err(|_| FileError::Container("named-graph IRI too large"))?;
        let iri_bytes = reader.read_at_precise(pos, iri_len)?;
        if iri_bytes.len() != iri_len_usize {
            return Err(FileError::Container("truncated named-graph IRI"));
        }
        let iri = String::from_utf8_lossy(&iri_bytes).into_owned();
        pos = iri_end;

        let (container_len, container_len_used) =
            read_uvarint_at_exact(reader.as_ref(), pos, section_end)?;
        pos = checked_end(pos, container_len_used)?;
        let container_end = checked_end(pos, container_len)?;
        if container_end > section_end {
            return Err(FileError::Container(
                "named-graph index container overruns section",
            ));
        }
        let remote = open_remote_graph_index(
            reader.clone(),
            ByteRange {
                offset: pos,
                len: container_len,
            },
            codec,
            has_tile_synopsis,
            read_concurrency,
            true,
        )?;
        graphs.push((iri, remote.index));
        pos = container_end;
    }

    if pos != section_end {
        return Err(FileError::Container("trailing named-graph bytes"));
    }

    Ok(graphs)
}

fn read_uvarint_at_exact<R: RangeReader + ?Sized>(
    reader: &R,
    absolute_offset: u64,
    container_end: u64,
) -> Result<(u64, u64), FileError> {
    if absolute_offset >= container_end {
        return Err(FileError::Container("truncated container varint"));
    }
    // Framing precedes opaque payload bytes with no encoded framing length.
    // Read one byte at a time until the varint terminates: even a 10-byte
    // speculative probe could otherwise cross a tiny tile directory and fetch
    // compressed tile bytes during a supposedly metadata-only lazy open. This
    // costs at most ten tiny reads per framing value; tile directories use the
    // bounded batched reader above once their field count is known.
    let mut bytes = Vec::with_capacity(10);
    for used in 0..10u64 {
        let offset = checked_end(absolute_offset, used)?;
        if offset >= container_end {
            return Err(FileError::Container("truncated container varint"));
        }
        let one = reader.read_at_precise(offset, 1)?;
        if one.len() != 1 {
            return Err(FileError::Container("truncated container varint"));
        }
        let byte = one
            .first()
            .copied()
            .ok_or(FileError::Container("truncated container varint"))?;
        bytes.push(byte);
        if byte & 0x80 == 0 {
            if used == 9 && byte > 1 {
                return Err(FileError::Container("container varint overflows u64"));
            }
            let (value, _) =
                read_uvarint(&bytes).ok_or(FileError::Container("malformed container varint"))?;
            return Ok((value, used + 1));
        }
    }
    Err(FileError::Container("container varint overflows u64"))
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

#[cfg(test)]
fn locate_container_sections_ranged_exact<R: RangeReader, const SECTION_COUNT: usize>(
    reader: &R,
    container: ByteRange,
) -> Result<[ByteRange; SECTION_COUNT], FileError> {
    locate_container_sections_ranged_exact_dynamic(reader, container, SECTION_COUNT)?
        .try_into()
        .map_err(|_| FileError::Container("unexpected container section count"))
}

fn locate_container_sections_ranged_exact_dynamic<R: RangeReader + ?Sized>(
    reader: &R,
    container: ByteRange,
    expected_sections: usize,
) -> Result<Vec<ByteRange>, FileError> {
    let container_end = checked_end(container.offset, container.len)?;
    let (section_count, used) = read_uvarint_at_exact(reader, container.offset, container_end)?;
    if section_count != expected_sections as u64 {
        return Err(FileError::Container("unexpected container section count"));
    }

    let mut ranges = Vec::with_capacity(expected_sections);
    let mut pos = checked_end(container.offset, used)?;
    for _ in 0..expected_sections {
        let (payload_len, len_used) = read_uvarint_at_exact(reader, pos, container_end)?;
        pos = checked_end(pos, len_used)?;
        let payload_end = checked_end(pos, payload_len)?;
        if payload_end > container_end {
            return Err(FileError::Container("section overruns buffer"));
        }
        ranges.push(ByteRange {
            offset: pos,
            len: payload_len,
        });
        pos = payload_end;
    }
    if pos != container_end {
        return Err(FileError::Container("trailing container bytes"));
    }
    Ok(ranges)
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
#[derive(Clone, Copy)]
struct LazyIndexConfig {
    block_codec: u8,
    has_synopsis: bool,
    read_concurrency: usize,
    perms: PermSet,
    exact_payload_boundaries: bool,
    version: u8,
}

fn open_index_container_lazy(
    reader: &std::sync::Arc<dyn RangeReader + Send + Sync>,
    container: ByteRange,
    config: LazyIndexConfig,
) -> Result<LazyIndexOpen, FileError> {
    let LazyIndexConfig {
        block_codec,
        has_synopsis,
        read_concurrency,
        perms,
        exact_payload_boundaries,
        version,
    } = config;
    if version == NEXT_FORMAT_VERSION {
        if perms != PermSet::ALL {
            return Err(FileError::Container(
                "paired-family generation requires all six logical permutations",
            ));
        }
        return open_family_index_container_lazy(reader, container, block_codec, read_concurrency);
    }
    let mut index_section_ranges = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
    let mut tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    #[allow(clippy::type_complexity)]
    let mut directories: [Vec<(u32, u32, Option<TileSynopsis>)>; NUM_PERMS] = Default::default();
    let exact_ranges = if exact_payload_boundaries {
        Some(locate_container_sections_ranged_exact_dynamic(
            reader.as_ref(),
            container,
            perms.len(),
        )?)
    } else {
        None
    };
    for (pos, perm) in perms.iter().enumerate() {
        let si = perm.section_index();
        let section = if let Some(ranges) = &exact_ranges {
            *ranges
                .get(pos)
                .ok_or(FileError::Container("container section not found"))?
        } else {
            locate_container_section_ranged(
                reader,
                container.offset,
                container.len,
                pos,
                perms.len() as u64,
            )?
        };
        index_section_ranges[si] = section;
        let dir = if exact_payload_boundaries {
            read_tile_directory_ranged_exact(reader, section)?
        } else {
            read_tile_directory_ranged(reader, section)?
        };
        // Tile synopses (one extra small tail read per section) let a routed
        // scan prune a tile by a bound secondary component before faulting it.
        let syn = if has_synopsis {
            read_tile_synopsis_ranged(reader, section, &dir, exact_payload_boundaries)
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
                Ok((
                    e.min_a,
                    e.max_a,
                    ByteRange {
                        offset: checked_end(section.offset, e.start)?,
                        len: (e.end - e.start),
                    },
                ))
            })
            .collect::<Result<Vec<_>, FileError>>()?;
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
        if bytes.len() as u64 != range.len {
            return None;
        }
        let decoded = decompress(codec, &bytes).ok()?;
        crate::triples::TripleBlock::parse(&decoded).ok()?;
        Some(decoded)
    });
    let bulk_ranges = tile_ranges.clone();
    let bulk_reader = reader.clone();
    let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis, intent| {
        let section = bulk_ranges.get(si)?;
        let want: Option<Vec<ByteRange>> = tis
            .iter()
            .map(|&ti| section.get(ti).map(|&(_, _, r)| r))
            .collect();
        let blobs = read_coalesced(bulk_reader.as_ref(), &want?, TILE_COALESCE_GAP, intent)?;
        blobs
            .iter()
            .map(|bytes| {
                let decoded = decompress(codec, bytes).ok()?;
                crate::triples::TripleBlock::parse(&decoded).ok()?;
                Some(decoded)
            })
            .collect()
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
    index.set_adaptive_controller(reader.adaptive_controller());
    Ok((index, index_section_ranges, tile_ranges))
}

fn read_family_fields_exact(
    reader: &dyn RangeReader,
    start: u64,
    end: u64,
    field_count: usize,
) -> Result<(Vec<u64>, u64), FileError> {
    const BATCH: u64 = 4096;
    let mut values = Vec::with_capacity(field_count.min(BATCH as usize));
    let mut pending = Vec::new();
    let mut pending_pos = 0usize;
    let mut read_offset = start;
    while values.len() < field_count {
        let available = &pending[pending_pos..];
        let mut parsed_pos = 0usize;
        if let Ok(value) = family_varint(available, &mut parsed_pos, "truncated family metadata") {
            pending_pos += parsed_pos;
            values.push(value);
            continue;
        }
        if available.len() >= 10 {
            return Err(FileError::Container("malformed family metadata varint"));
        }
        pending.drain(..pending_pos);
        pending_pos = 0;
        let unfinished = field_count - values.len();
        let fetch_len = (unfinished as u64).min(BATCH);
        let fetch_end = checked_end(read_offset, fetch_len)?;
        if fetch_end > end {
            return Err(FileError::Container("truncated family metadata"));
        }
        let bytes = reader.read_at_precise(read_offset, fetch_len)?;
        if bytes.len() as u64 != fetch_len {
            return Err(FileError::Container("truncated family metadata"));
        }
        pending.extend_from_slice(&bytes);
        read_offset = fetch_end;
    }
    let unread_pending = u64::try_from(pending.len().saturating_sub(pending_pos))
        .map_err(|_| FileError::Container("family metadata offset too large"))?;
    debug_assert_eq!(
        unread_pending, 0,
        "minimum-field reads cannot cross payloads"
    );
    Ok((values, read_offset))
}

#[derive(Clone)]
struct RangedFamilyMeta {
    ranges: Vec<(u32, u32)>,
    first_records: Vec<(ByteRange, u64, u64)>,
    second_records: Vec<(ByteRange, u64, u64)>,
    first_synopses: Vec<Synopsis>,
    second_synopses: Vec<Synopsis>,
}

#[derive(Clone)]
struct FamilyRecordMeta {
    range: ByteRange,
    prefix_len: u64,
    compressed_len: u64,
    leading: (u32, u32),
    synopsis: Synopsis,
}

fn read_family_directory_ranged(
    reader: &dyn RangeReader,
    section: ByteRange,
) -> Result<RangedFamilyMeta, FileError> {
    let end = checked_end(section.offset, section.len)?;
    let (header, directory_start) = read_family_fields_exact(reader, section.offset, end, 3)?;
    let count = usize::try_from(header[0])
        .map_err(|_| FileError::Container("family tile count too large"))?;
    let directory_len = header[1];
    let trailer_len = header[2];
    let directory_fields = count.checked_mul(8).ok_or(FileError::Container(
        "family directory field count overflows",
    ))?;
    let trailer_fields = count.checked_mul(8).ok_or(FileError::Container(
        "family synopsis field count overflows",
    ))?;
    let minimum_directory = u64::try_from(directory_fields)
        .map_err(|_| FileError::Container("family directory size exceeds u64"))?;
    let minimum_trailer = u64::try_from(trailer_fields)
        .map_err(|_| FileError::Container("family trailer size exceeds u64"))?;
    if directory_len < minimum_directory {
        return Err(FileError::Container("family tile count exceeds directory"));
    }
    if trailer_len < minimum_trailer {
        return Err(FileError::Container("family tile count exceeds trailer"));
    }
    let maximum_directory = minimum_directory
        .checked_mul(10)
        .ok_or(FileError::Container("family directory size overflows"))?;
    let maximum_trailer = minimum_trailer
        .checked_mul(10)
        .ok_or(FileError::Container("family trailer size overflows"))?;
    if directory_len > maximum_directory {
        return Err(FileError::Container(
            "family directory exceeds varint framing",
        ));
    }
    if trailer_len > maximum_trailer {
        return Err(FileError::Container(
            "family trailer exceeds varint framing",
        ));
    }
    materializable_len(directory_len)
        .map_err(|_| FileError::Container("family directory too large"))?;
    materializable_len(trailer_len)
        .map_err(|_| FileError::Container("family trailer too large"))?;
    let records_start = checked_end(directory_start, directory_len)?;
    if records_start > end {
        return Err(FileError::Container("truncated family directory"));
    }
    let directory_bytes = reader.read_at_precise(directory_start, directory_len)?;
    if directory_bytes.len() as u64 != directory_len {
        return Err(FileError::Container("truncated family directory"));
    }
    let parse_fields = |bytes: &[u8], count: usize| -> Result<Vec<u64>, FileError> {
        let mut values = Vec::with_capacity(count.min(bytes.len()));
        let mut pos = 0usize;
        for _ in 0..count {
            values.push(family_varint(bytes, &mut pos, "truncated family metadata")?);
        }
        if pos != bytes.len() {
            return Err(FileError::Container("trailing family metadata bytes"));
        }
        Ok(values)
    };
    let fields = parse_fields(&directory_bytes, directory_fields)?;
    let mut ranges = Vec::with_capacity(count.min(4096));
    let mut previous_min = 0u32;
    for pair in fields[..count * 2].chunks_exact(2) {
        let delta = u32::try_from(pair[0])
            .map_err(|_| FileError::Container("family minimum delta too large"))?;
        let span = u32::try_from(pair[1])
            .map_err(|_| FileError::Container("family range span too large"))?;
        let min_a = previous_min
            .checked_add(delta)
            .ok_or(FileError::Container("family minimum overflows u32"))?;
        let max_a = min_a
            .checked_add(span)
            .ok_or(FileError::Container("family maximum overflows u32"))?;
        ranges.push((min_a, max_a));
        previous_min = min_a;
    }
    let parse_dirs = |slice: &[u64]| -> Result<Vec<(u8, u64, u64)>, FileError> {
        slice
            .chunks_exact(3)
            .map(|entry| {
                let flags = u8::try_from(entry[0])
                    .map_err(|_| FileError::Container("family flags exceed u8"))?;
                if flags & !FAMILY_FLAG_MASK != 0 {
                    return Err(FileError::Container("reserved family flags"));
                }
                if entry[2] > PREFIX2_FORMAT_BUDGET as u64 {
                    return Err(FileError::Container("prefix-2 blob exceeds fixed budget"));
                }
                let record_len = checked_end(entry[1], entry[2])?;
                materializable_len(record_len)
                    .map_err(|_| FileError::Container("family payload too large"))?;
                Ok((flags, entry[1], entry[2]))
            })
            .collect()
    };
    let first_dirs = parse_dirs(&fields[count * 2..count * 5])?;
    let second_dirs = parse_dirs(&fields[count * 5..count * 8])?;
    for i in 0..count {
        if first_dirs[i].0 != second_dirs[i].0 {
            return Err(FileError::Container("sibling continuation flags differ"));
        }
        let repeated_previous = i > 0 && ranges[i - 1] == ranges[i];
        let repeated_next = i + 1 < count && ranges[i + 1] == ranges[i];
        if repeated_previous && ranges[i].0 != ranges[i].1 {
            return Err(FileError::Container(
                "continuation range is not a single leading group",
            ));
        }
        let flags = first_dirs[i].0;
        if (flags & FAMILY_FLAG_CONTINUES_PREVIOUS != 0) != repeated_previous
            || (flags & FAMILY_FLAG_CONTINUES_NEXT != 0) != repeated_next
        {
            return Err(FileError::Container("impossible family continuation"));
        }
        if i > 0 && !repeated_previous && ranges[i].0 <= ranges[i - 1].1 {
            return Err(FileError::Container("family ranges overlap"));
        }
    }
    let make_records = |dirs: &[(u8, u64, u64)], cursor: &mut u64| {
        dirs.iter()
            .map(|&(_, compressed, prefix)| {
                let len = checked_end(prefix, compressed)?;
                let range = ByteRange {
                    offset: *cursor,
                    len,
                };
                *cursor = checked_end(*cursor, len)?;
                Ok((range, prefix, compressed))
            })
            .collect::<Result<Vec<_>, FileError>>()
    };
    let mut cursor = records_start;
    let first_records = make_records(&first_dirs, &mut cursor)?;
    let second_records = make_records(&second_dirs, &mut cursor)?;
    let trailer_end = checked_end(cursor, trailer_len)?;
    if trailer_end != end {
        return Err(FileError::Container(
            "family record lengths disagree with framing",
        ));
    }
    let trailer_bytes = reader.read_at_precise(cursor, trailer_len)?;
    if trailer_bytes.len() as u64 != trailer_len {
        return Err(FileError::Container("truncated family synopsis trailer"));
    }
    let trailers = parse_fields(&trailer_bytes, trailer_fields)?;
    let decode_synopses = |slice: &[u64]| -> Result<Vec<Synopsis>, FileError> {
        slice
            .chunks_exact(4)
            .map(|entry| {
                let min_b = u32::try_from(entry[0])
                    .map_err(|_| FileError::Container("family synopsis b exceeds u32"))?;
                let span_b = u32::try_from(entry[1])
                    .map_err(|_| FileError::Container("family synopsis b span exceeds u32"))?;
                let min_c = u32::try_from(entry[2])
                    .map_err(|_| FileError::Container("family synopsis c exceeds u32"))?;
                let span_c = u32::try_from(entry[3])
                    .map_err(|_| FileError::Container("family synopsis c span exceeds u32"))?;
                Ok((
                    min_b,
                    min_b
                        .checked_add(span_b)
                        .ok_or(FileError::Container("family synopsis b overflows u32"))?,
                    min_c,
                    min_c
                        .checked_add(span_c)
                        .ok_or(FileError::Container("family synopsis c overflows u32"))?,
                ))
            })
            .collect()
    };
    Ok(RangedFamilyMeta {
        ranges,
        first_records,
        second_records,
        first_synopses: decode_synopses(&trailers[..count * 4])?,
        second_synopses: decode_synopses(&trailers[count * 4..])?,
    })
}

fn open_family_index_container_lazy(
    reader: &std::sync::Arc<dyn RangeReader + Send + Sync>,
    container: ByteRange,
    codec: u8,
    read_concurrency: usize,
) -> Result<LazyIndexOpen, FileError> {
    let families = locate_container_sections_ranged_exact_dynamic(reader.as_ref(), container, 3)?;
    let mut section_ranges = [ByteRange { offset: 0, len: 0 }; NUM_PERMS];
    let mut tile_ranges: [Vec<(u32, u32, ByteRange)>; NUM_PERMS] = Default::default();
    let mut directories: [Vec<(u32, u32, Option<TileSynopsis>)>; NUM_PERMS] = Default::default();
    let mut record_meta: [Vec<FamilyRecordMeta>; NUM_PERMS] = Default::default();
    for (family, section) in [
        IndexFamily::Subject,
        IndexFamily::Predicate,
        IndexFamily::Object,
    ]
    .into_iter()
    .zip(families)
    {
        let meta = read_family_directory_ranged(reader.as_ref(), section)?;
        let slot = family.slot();
        for (logical, records, synopses) in [
            (slot, meta.first_records, meta.first_synopses),
            (slot + 3, meta.second_records, meta.second_synopses),
        ] {
            section_ranges[logical] = section;
            directories[logical] = meta
                .ranges
                .iter()
                .copied()
                .zip(synopses.iter().copied())
                .map(|((min_a, max_a), syn)| (min_a, max_a, Some(syn)))
                .collect();
            tile_ranges[logical] = meta
                .ranges
                .iter()
                .copied()
                .zip(records.iter().map(|record| record.0))
                .map(|((min_a, max_a), range)| (min_a, max_a, range))
                .collect();
            record_meta[logical] = records
                .into_iter()
                .zip(meta.ranges.iter().copied())
                .zip(synopses)
                .map(
                    |(((range, prefix_len, compressed_len), leading), synopsis)| FamilyRecordMeta {
                        range,
                        prefix_len,
                        compressed_len,
                        leading,
                        synopsis,
                    },
                )
                .collect();
        }
    }
    let decode_record = move |blob: &[u8], meta: &FamilyRecordMeta| {
        let mut pos = 0usize;
        let (_, payload) =
            decode_family_record(blob, &mut pos, meta.prefix_len, meta.compressed_len, codec)
                .ok()?;
        if pos != blob.len() {
            return None;
        }
        let block = crate::triples::TripleBlock::parse(&payload).ok()?;
        let zone = block.zone();
        if (zone.min_a, zone.max_a) != meta.leading
            || (zone.min_b, zone.max_b, zone.min_c, zone.max_c) != meta.synopsis
        {
            return None;
        }
        Some(payload)
    };
    let loader_meta = record_meta.clone();
    let loader_reader = reader.clone();
    let loader: crate::index::TileLoader = Box::new(move |si, ti| {
        let meta = loader_meta.get(si)?.get(ti)?;
        let blob = loader_reader
            .read_at(meta.range.offset, meta.range.len)
            .ok()?;
        if blob.len() as u64 != meta.range.len {
            return None;
        }
        decode_record(&blob, meta)
    });
    let bulk_meta = record_meta.clone();
    let bulk_reader = reader.clone();
    let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis, intent| {
        let section = bulk_meta.get(si)?;
        let metas: Option<Vec<_>> = tis.iter().map(|&ti| section.get(ti).cloned()).collect();
        let metas = metas?;
        let ranges: Vec<_> = metas.iter().map(|meta| meta.range).collect();
        let blobs = read_coalesced(bulk_reader.as_ref(), &ranges, TILE_COALESCE_GAP, intent)?;
        blobs
            .iter()
            .zip(&metas)
            .map(|(blob, meta)| decode_record(blob, meta))
            .collect()
    });
    let mut index = GraphIndex::from_remote_directories(directories, PermSet::ALL, loader)
        .with_bulk_loader(bulk);
    index.set_tile_lens(std::array::from_fn(|si| {
        tile_ranges[si]
            .iter()
            .map(|&(_, _, range)| range.len.min(u32::MAX as u64) as u32)
            .collect()
    }));
    index.set_read_concurrency(read_concurrency);
    index.set_adaptive_controller(reader.adaptive_controller());
    Ok((index, section_ranges, tile_ranges))
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
pub(crate) fn encode_index_container(index: &GraphIndex, codec: u8) -> Vec<u8> {
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

fn encode_named_graph_families(
    named: &[(String, GraphIndex)],
    codec: u8,
) -> Result<Vec<u8>, FileError> {
    let mut out = Vec::new();
    write_uvarint(&mut out, named.len() as u64);
    for (iri, index) in named {
        if index.perms() != PermSet::ALL {
            return Err(FileError::Container(
                "paired-family named graph requires all permutations",
            ));
        }
        write_uvarint(&mut out, iri.len() as u64);
        out.extend_from_slice(iri.as_bytes());
        let container = encode_family_index_container(index, codec)?;
        write_uvarint(&mut out, container.len() as u64);
        out.extend_from_slice(&container);
    }
    Ok(out)
}

fn decode_named_graphs(
    bytes: &[u8],
    codec: u8,
    perms: PermSet,
    version: u8,
) -> Result<Vec<(String, GraphIndex)>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated graph count"))?;
    // Bounds-checked slice within this (already bounded) section. Lengths read
    // below are untrusted, so every range is validated before indexing.
    let bound = |start: usize, len: u64| -> Result<usize, FileError> {
        let len = usize::try_from(len)
            .map_err(|_| FileError::Container("named-graph field too large"))?;
        start
            .checked_add(len)
            .filter(|&e| e <= bytes.len())
            .ok_or(FileError::Container("named-graph field overruns buffer"))
    };
    let initial_capacity = usize::try_from(n).unwrap_or(bytes.len()).min(bytes.len());
    let mut out = Vec::with_capacity(initial_capacity);
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
        let index = decode_index_for_version(&bytes[pos..cend], codec, perms, version)?;
        out.push((iri, index));
        pos = cend;
    }
    if pos != bytes.len() {
        return Err(FileError::Container("trailing named-graph bytes"));
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
    let paired = if default_index.perms() == PermSet::ALL {
        encode_family_index_container(default_index, codec)
            .and_then(|index| encode_named_graph_families(named, codec).map(|named| (index, named)))
    } else {
        Err(FileError::Container(
            "paired-family generation requires all permutations",
        ))
    };
    let (version, index_container, named_section) = match paired {
        Ok((index, named)) => (NEXT_FORMAT_VERSION, index, named),
        Err(_) => (
            LEGACY_FORMAT_VERSION,
            encode_index_container(default_index, codec),
            encode_named_graphs(named, codec),
        ),
    };

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
        version,
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
/// [`RangeReader`]: read the 1 KiB header, then the metadata byte range â€”
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
    version: u8,
    /// `(count, slab table)` — set once the leading count varint is read.
    /// Slabs allocate on first touch; boxed slices never move, so `&` handed
    /// out to entries stay valid for `&self`'s lifetime with no unsafe.
    #[allow(clippy::type_complexity)]
    dir: std::sync::OnceLock<(usize, Box<[std::sync::OnceLock<Box<[NamedEntry]>>]>)>,
    walk: std::sync::Mutex<NamedWalk>,
    failed: std::sync::atomic::AtomicBool,
    #[cfg(feature = "unsafe-decode-bench")]
    unchecked_decode: std::sync::atomic::AtomicBool,
}

impl LazyNamedGraphs {
    fn new(
        reader: std::sync::Arc<dyn RangeReader + Send + Sync>,
        section: ByteRange,
        codec: u8,
        has_synopsis: bool,
        read_concurrency: usize,
        perms: PermSet,
        version: u8,
    ) -> Self {
        LazyNamedGraphs {
            reader,
            section,
            codec,
            has_synopsis,
            read_concurrency,
            perms,
            version,
            dir: std::sync::OnceLock::new(),
            walk: std::sync::Mutex::new(NamedWalk::default()),
            failed: std::sync::atomic::AtomicBool::new(false),
            #[cfg(feature = "unsafe-decode-bench")]
            unchecked_decode: std::sync::atomic::AtomicBool::new(false),
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
        let n = match usize::try_from(n) {
            Ok(n) => n,
            Err(_) => {
                self.fail();
                return None;
            }
        };
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
        let total = self.directory()?.0;
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
            let have = |b: &[u8], off: u64, need: u64| {
                let Some(need_end) = pos.checked_add(need) else {
                    return false;
                };
                let Some(buf_end) = off.checked_add(b.len() as u64) else {
                    return false;
                };
                pos >= off && need_end <= buf_end
            };
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
                if !have(&w.buf, w.buf_off, len) {
                    self.fail();
                    return None;
                }
            }
            let rel = (pos - w.buf_off) as usize;
            let Some((ilen, u1)) = read_uvarint(&w.buf[rel..]) else {
                self.fail();
                return None;
            };
            let iri_end = match pos
                .checked_add(u1 as u64)
                .and_then(|start| start.checked_add(ilen))
            {
                Some(iri_end) if iri_end <= end => iri_end,
                _ => {
                    self.fail();
                    return None;
                }
            };
            let header_need = match iri_end
                .checked_sub(pos)
                .and_then(|used| used.checked_add(10))
            {
                Some(need) => need,
                None => {
                    self.fail();
                    return None;
                }
            };
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
                if !have(&w.buf, w.buf_off, header_need.min(end - pos)) {
                    self.fail();
                    return None;
                }
            }
            let rel = (pos - w.buf_off) as usize;
            let ilen = match usize::try_from(ilen) {
                Ok(ilen) => ilen,
                Err(_) => {
                    self.fail();
                    return None;
                }
            };
            let istart = match rel.checked_add(u1) {
                Some(start) => start,
                None => {
                    self.fail();
                    return None;
                }
            };
            let iend = match istart.checked_add(ilen) {
                Some(end) if end <= w.buf.len() => end,
                _ => {
                    self.fail();
                    return None;
                }
            };
            let iri = String::from_utf8_lossy(&w.buf[istart..iend]).into_owned();
            let Some((clen, u2)) = read_uvarint(&w.buf[iend..]) else {
                self.fail();
                return None;
            };
            let cstart = match iri_end.checked_add(u2 as u64) {
                Some(start) => start,
                None => {
                    self.fail();
                    return None;
                }
            };
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
            if w.next == total && w.pos != end {
                self.fail();
                return None;
            }
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
        let graph = if range.len <= NAMED_GRAPH_RESIDENT_MAX {
            let bytes = self.container_bytes(range)?;
            match decode_index_for_version(&bytes, self.codec, self.perms, self.version) {
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
                LazyIndexConfig {
                    block_codec: self.codec,
                    has_synopsis: self.has_synopsis,
                    read_concurrency: self.read_concurrency,
                    perms: self.perms,
                    exact_payload_boundaries: true,
                    version: self.version,
                },
            ) {
                Ok((g, _, _)) => Some(g),
                Err(_) => {
                    self.fail();
                    None
                }
            }
        }?;
        #[cfg(feature = "unsafe-decode-bench")]
        if self
            .unchecked_decode
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            // SAFETY: the caller of `assume_valid_blocks` established this
            // invariant for every graph opened later by this lazy catalog.
            unsafe { graph.assume_valid_blocks() };
        }
        Some(graph)
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

    #[cfg(feature = "unsafe-decode-bench")]
    unsafe fn assume_valid_blocks(&self) {
        self.unchecked_decode
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.for_each_opened(|graph| {
            // SAFETY: the caller established the contract for every already
            // opened graph, and `open_graph` applies it to future graphs.
            unsafe { graph.assume_valid_blocks() };
        });
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

    #[cfg(feature = "unsafe-decode-bench")]
    unsafe fn assume_valid_blocks(&self) {
        match self {
            NamedGraphsSlot::Resident(graphs) => {
                for (_, graph) in graphs {
                    // SAFETY: forwarded from this method's caller.
                    unsafe { graph.assume_valid_blocks() };
                }
            }
            NamedGraphsSlot::Lazy(graphs) => {
                // SAFETY: forwarded from this method's caller.
                unsafe { graphs.assume_valid_blocks() };
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
            let range = checked_resident_range(off, len, bytes.len())
                .map_err(|_| FileError::Container("section range out of bounds"))?;
            bytes
                .get(range)
                .ok_or(FileError::Container("section range out of bounds"))
        };

        let dict = decode_dictionary_container(
            region(header.dictionary_offset, header.dictionary_len)?,
            header.dict_codec,
        )?;

        let index_bytes = region(header.root_dir_offset, header.root_dir_len)?;
        let index = decode_index_for_version(
            index_bytes,
            header.block_codec,
            header.perms,
            header.version,
        )?;
        let index_section_ranges = decode_index_section_ranges_for_version(
            index_bytes,
            header.root_dir_offset,
            header.perms,
            header.version,
        )?;

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
                header.version,
            )?
        } else {
            Vec::new()
        });

        let metadata = if header.metadata_len > 0 {
            region(header.metadata_offset, header.metadata_len)?.to_vec()
        } else {
            Vec::new()
        };

        let tile_ranges = if header.version == LEGACY_FORMAT_VERSION {
            tile_file_ranges(index_bytes, header.root_dir_offset, &index_section_ranges)
        } else {
            family_tile_file_ranges(index_bytes, header.root_dir_offset)?
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
        // The size in the label is DERIVED from the constant that defines the
        // span, never typed out: this line read "fixed 128 bytes" — the
        // pre-v0.3 header size — while emitting a 1024-byte span, and it is
        // what the file explorer shows a reader inspecting a real file.
        // `layout_header_label_matches_its_span` pins the two together.
        let mut out = vec![seg(
            "header",
            format!("header (fixed {} bytes)", crate::header::HEADER_LEN),
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

    /// Permanently select unchecked triple-block decoding for every graph in
    /// this file instance. This API is compiled only for controlled benchmarks.
    ///
    /// # Safety
    ///
    /// Every default and named-graph index block, including every block later
    /// returned by a lazy range loader, must be a complete immutable image
    /// produced by rete's encoder. Malformed or truncated input can cause an
    /// out-of-bounds read. Normal applications must not enable this mode.
    #[cfg(feature = "unsafe-decode-bench")]
    pub unsafe fn assume_valid_index_blocks(&mut self) {
        // SAFETY: this method's caller establishes the same invariant for the
        // default index and for every named graph below.
        unsafe { self.index.assume_valid_blocks() };
        // SAFETY: covered by this method's all-index-block contract, including
        // named graphs that the lazy catalog opens after this call.
        unsafe { self.named_graphs.assume_valid_blocks() };
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
    ///
    /// To dump a *slice* — one predicate, one subject, one object — call
    /// [`dump_filtered_each`](Self::dump_filtered_each), which is this method
    /// with a triple pattern and prunes tiles instead of filtering rows.
    pub fn dump_each<F: FnMut(&str, &str, &str)>(&self, graph: Option<&str>, f: F) {
        self.dump_filtered_each(graph, None, None, None, f)
    }

    /// [`dump_each`](Self::dump_each) restricted to a triple **pattern** — the
    /// filtered dump. Any of `s` / `p` / `o` may be bound (canonical N-Triples
    /// term tokens); all three unbound is exactly `dump_each`.
    ///
    /// # Why this is cheap, and how much
    ///
    /// It is the same access path a routed *query* takes, not a full scan with a
    /// predicate test bolted on: [`GraphIndex::scan_iter`] picks the permutation
    /// with the longest bound prefix, binary-searches that section's tile
    /// directory down to the span a bound leading id can live in, and then drops
    /// every remaining tile whose recorded synopsis proves it cannot hold a
    /// bound secondary component — **all from directories, without fetching a
    /// tile**. So a filtered dump pays for the slice, which is the whole point
    /// of issue #117.
    ///
    /// Measured on `cordis.rete` (801 MB, a 417 MB dictionary, 26.4M quads,
    /// six named graphs), lazily opened; "before" is the only way to get a
    /// predicate slice out of a dump previously — dump the graph and throw away
    /// what does not match, in the consumer:
    ///
    /// ```text
    ///   graph=results  p=s66#doi     -> 337,811 rows
    ///     before  375.8 MB · 1797 req · 2105.8 MB RSS · 12.8 s · first row 409 ms
    ///     after    16.0 MB ·  105 req ·  182.6 MB RSS ·  0.41 s · first row  14 ms
    ///   graph=results  p=s66#isbn    ->  40,761 rows
    ///     before  375.8 MB · 1797 req · 2105.7 MB RSS · 12.8 s
    ///     after    15.5 MB ·  104 req ·  155.2 MB RSS ·  0.23 s
    /// ```
    ///
    /// # Where it does NOT help
    ///
    /// The index is pruned; the **dictionary is not**. Term resolution still
    /// faults whichever chunks the surviving rows' terms live in, and on a file
    /// whose payload *is* the literals those chunks are most of the file. Same
    /// file, a predicate whose objects are the long abstracts:
    ///
    /// ```text
    ///   graph=projects p=s66#abstract ->  80,206 rows
    ///     before  260.7 MB · 1142 req · 1496.9 MB RSS · 5.97 s
    ///     after   213.4 MB · 1606 req · 1144.4 MB RSS · 2.91 s   (1.2x, not 23x)
    /// ```
    ///
    /// An *unfiltered* dump is unchanged by construction — it is the floor, and
    /// its peak is the resident dictionary, not this. Two ceilings sit above
    /// both: a faulted dictionary chunk is a `OnceCell` nothing ever evicts, and
    /// on a literal-heavy file the chunk directory alone can be a third of the
    /// file before any dump work at all (#198).
    ///
    /// # Permutations
    ///
    /// Every one of the eight bound/unbound shapes routes inside
    /// [`PermSet::CORE`] — `{SPO, POS, OSP}`, the minimum a legal file carries
    /// (see `perm_routing_never_leaves_core`). A file built with
    /// `--permutations 3` therefore prunes *identically* to a six-permutation
    /// one: same tiles, same rows, no fallback path to get wrong.
    ///
    /// A bound term the dictionary does not know, or an absent graph IRI, yields
    /// nothing without touching the index.
    ///
    /// # Order
    ///
    /// Rows arrive in the **routed permutation's** order, which is the price of
    /// routing at all. Unfiltered (and subject-bound) that is SPO — canonical
    /// `(s, p, o)`, unchanged from [`dump_each`](Self::dump_each). A bound
    /// predicate routes to POS and streams `(p, o, s)`; a bound object routes to
    /// OSP. The *set* is identical to the unfiltered dump filtered; the order is
    /// not, so a consumer that needs canonical order must sort. N-Quads, the
    /// format `rete export` writes, is order-independent.
    ///
    /// [`PermSet::CORE`]: crate::index::PermSet::CORE
    pub fn dump_filtered_each<F: FnMut(&str, &str, &str)>(
        &self,
        graph: Option<&str>,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
        mut f: F,
    ) {
        let index = match graph {
            None => Some(&self.index),
            Some(g) => self.graph_index(g),
        };
        let Some(index) = index else { return };
        // A bound term the dictionary does not know can match nothing, and the
        // index is never touched — same contract as `query_iter`/`query_batch`.
        let Some(pattern) = self.resolve_query_pattern(s, p, o) else {
            return;
        };
        let dict = &self.dict;
        // Resolve in fixed-size batches with ONE coalesced dictionary fault per
        // batch — the same `prefetch_terms` call `dump_batch` makes, for the
        // same reason.
        //
        // This used to be `Dictionary::prefetch_all` up front, on the argument
        // that a full dump reaches every term anyway so the bytes are owed
        // either way. That argument only holds for a dump of EVERYTHING, and
        // this takes a graph AND a pattern. Measured on `cordis.rete` (801 MB,
        // a 417 MB dictionary, six named graphs), lazily opened:
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
        for t in index.scan_iter(pattern) {
            ids.push(t);
            if ids.len() == RESOLVE_BATCH {
                flush(&mut ids, &mut f);
            }
        }
        flush(&mut ids, &mut f);
    }

    /// What a [`dump_filtered_each`](Self::dump_filtered_each) over this graph
    /// and pattern will fetch, **before** it starts — the dump twin of
    /// `rete cost`'s query preview.
    ///
    /// Costed from the tile directories, so it fetches no tile. It does read the
    /// dictionary far enough to resolve the bound terms (that is a real,
    /// unavoidable cost of the dump too) and to learn whether the graph exists.
    pub fn dump_plan(
        &self,
        graph: Option<&str>,
        s: Option<&str>,
        p: Option<&str>,
        o: Option<&str>,
    ) -> DumpPlan {
        let index = match graph {
            None => Some(&self.index),
            Some(g) => self.graph_index(g),
        };
        let scan = index
            .zip(self.resolve_query_pattern(s, p, o))
            .map(|(ix, pattern)| ix.scan_plan(pattern));
        DumpPlan {
            scan,
            dictionary_bytes: self.header.dictionary_len,
        }
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
        let index = decode_index_for_version(
            &index_bytes,
            header.block_codec,
            header.perms,
            header.version,
        )?;
        let index_section_ranges = decode_index_section_ranges_for_version(
            &index_bytes,
            header.root_dir_offset,
            header.perms,
            header.version,
        )?;

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
            decode_named_graphs(&nb, header.block_codec, header.perms, header.version)?
        } else {
            Vec::new()
        });

        // The metadata section (Dataset Card) is deliberately NOT fetched here:
        // a ranged query open keeps to its small range budget. Use `Rete::open`
        // (or a dedicated card fetch) when the card is actually needed.
        let tile_ranges = if header.version == LEGACY_FORMAT_VERSION {
            tile_file_ranges(&index_bytes, header.root_dir_offset, &index_section_ranges)
        } else {
            family_tile_file_ranges(&index_bytes, header.root_dir_offset)?
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

    /// Open via an **owned** [`RangeReader`] with lazy tile faulting (tiled
    /// v0.2 files): fetches the header plus dictionary/index directory metadata;
    /// pyramid and text data stay deferred. When named graphs exist, every
    /// opening metadata read uses exact physical boundaries (including the
    /// shared dictionary and default graph), so even a widening block-cache
    /// wrapper cannot fetch a named tile payload. Default-only files retain
    /// their bounded cached directory prefixes. No tile image is decompressed
    /// until a scan touches it, so a selective SPARQL query fetches O(touched
    /// tiles) bytes instead of the whole index.
    ///
    /// **Failure contract:** scans are infallible by design, so a failed tile
    /// fetch yields an empty tile and sets a sticky flag â€” after evaluating,
    /// callers MUST check [`index_incomplete`](Self::index_incomplete) and
    /// surface an error instead of the (possibly partial) results.
    pub fn open_ranged_lazy<R: RangeReader + Send + Sync + 'static>(
        reader: R,
    ) -> Result<Self, FileError> {
        // The header tells us whether named graphs exist, so it must be precise
        // before that decision: a block-aligned cache read could otherwise
        // swallow a tiny file's named tile payload during the opening probe.
        let head = reader.read_at_precise(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        let reader = std::sync::Arc::new(reader);
        // Captured before the loader closures take the Arc: the reader's
        // concurrent-range fan-out, stamped onto the index for the planner.
        let read_concurrency = reader.concurrency();

        // Lazily-chunked dictionary: locate the four sections, fetch each
        // section's header + restart table + chunk directory (small), and
        // fault the chunk bodies in on first term lookup.
        let has_named_graphs = header.named_graphs_len > 0;
        let dict = ranged_chunked_dictionary(&reader, &header, [true; 4], has_named_graphs)?;

        // Locate the carried index section payloads (container framing only) and
        // fetch just their tile directories — shared with the per-named-graph
        // lazy opener, which opens a large graph's container the same way.
        let reader_dyn: std::sync::Arc<dyn RangeReader + Send + Sync> = reader.clone();
        let (index, index_section_ranges, tile_ranges) = open_index_container_lazy(
            &reader_dyn,
            ByteRange {
                offset: header.root_dir_offset,
                len: header.root_dir_len,
            },
            LazyIndexConfig {
                block_codec: header.block_codec,
                has_synopsis: header.has_tile_synopsis(),
                read_concurrency,
                perms: header.perms,
                exact_payload_boundaries: has_named_graphs,
                version: header.version,
            },
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
                header.version,
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
    let (position, sections) = if header.version == NEXT_FORMAT_VERSION {
        (permutation.section_index() % 3, 3)
    } else {
        (
            header
                .perms
                .position(permutation)
                .ok_or(FileError::Container("routed to an absent permutation"))?,
            header.perms.len(),
        )
    };
    let section = locate_container_section_ranged(
        reader,
        header.root_dir_offset,
        header.root_dir_len,
        position,
        sections as u64,
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
    if routed.header.version == NEXT_FORMAT_VERSION {
        let family = read_family_directory_ranged(reader, routed.section)?;
        let second = routed.permutation.section_index() >= 3;
        let (records, synopses) = if second {
            (&family.second_records, &family.second_synopses)
        } else {
            (&family.first_records, &family.first_synopses)
        };
        let [pa, pb, pc] = routed.permutation.order_pattern(routed.pattern);
        let admitted = |i: usize| {
            let (min_a, max_a) = family.ranges[i];
            let (min_b, max_b, min_c, max_c) = synopses[i];
            pa.is_none_or(|a| min_a <= a && a <= max_a)
                && pb.is_none_or(|b| min_b <= b && b <= max_b)
                && pc.is_none_or(|c| min_c <= c && c <= max_c)
        };
        let mut out = Vec::new();
        for (i, (range, prefix, compressed)) in records.iter().enumerate() {
            if !admitted(i) {
                continue;
            }
            let blob = reader.read_at(range.offset, range.len)?;
            if blob.len() as u64 != range.len {
                return Err(FileError::Container("truncated family record"));
            }
            let mut pos = 0usize;
            let (_, tile) = decode_family_record(
                &blob,
                &mut pos,
                *prefix,
                *compressed,
                routed.header.block_codec,
            )?;
            if pos != blob.len() {
                return Err(FileError::Container("trailing family record bytes"));
            }
            let block = crate::triples::TripleBlock::parse(&tile)
                .map_err(|_| FileError::Container("malformed family tile payload"))?;
            let zone = block.zone();
            if (zone.min_a, zone.max_a) != family.ranges[i]
                || (zone.min_b, zone.max_b, zone.min_c, zone.max_c) != synopses[i]
            {
                return Err(FileError::Container("family metadata disagrees with tile"));
            }
            out.extend(GraphIndex::match_serialized_block(
                &tile,
                routed.permutation,
                routed.pattern,
            ));
        }
        out.sort_unstable();
        return Ok(out);
    }
    let dir = read_tile_directory_ranged(reader, routed.section)?;
    let [pa, _, _] = routed.permutation.order_pattern(routed.pattern);
    let codec = routed.header.block_codec;
    let mut out = Vec::new();
    match pa {
        // Bound leading id: the run of covering tiles (several for a split
        // mega-group; one otherwise).
        Some(a) => {
            for e in dir.iter().filter(|e| e.min_a <= a && a <= e.max_a) {
                let offset = checked_end(routed.section.offset, e.start)?;
                let len = e
                    .end
                    .checked_sub(e.start)
                    .ok_or(FileError::Container("tile range overflows"))?;
                materializable_len(len)
                    .map_err(|_| FileError::Container("tile length too large"))?;
                let bytes = reader.read_at(offset, len)?;
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
                let body_offset = checked_end(routed.section.offset, base)?;
                let body_len = last
                    .end
                    .checked_sub(base)
                    .ok_or(FileError::Container("tile range overflows"))?;
                materializable_len(body_len)
                    .map_err(|_| FileError::Container("tile body too large"))?;
                let body = reader.read_at(body_offset, body_len)?;
                for e in &dir {
                    let start = usize::try_from(
                        e.start
                            .checked_sub(base)
                            .ok_or(FileError::Container("tile range overflows"))?,
                    )
                    .map_err(|_| FileError::Container("tile offset too large"))?;
                    let end = usize::try_from(
                        e.end
                            .checked_sub(base)
                            .ok_or(FileError::Container("tile range overflows"))?,
                    )
                    .map_err(|_| FileError::Container("tile offset too large"))?;
                    let tile = decompress(
                        codec,
                        body.get(start..end)
                            .ok_or(FileError::Container("tile overruns body"))?,
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
/// section whose terms resolve to `None`. On a file written before #198 a
/// **directory is not small**: it carried each chunk's first term verbatim, so
/// the object-only section of a dataset that stores abstracts can run to
/// hundreds of megabytes, and fetching it is most of what a remote open costs.
/// Current builds key it by the shortest separator instead (a few bytes per
/// chunk), but that is write-side, so every already-published file still pays.
/// A reader that only ever resolves subjects (see [`SearchView`]) skips
/// sections 2 and 3 and pays none of it. [`Rete::open_ranged_lazy`] wants all
/// four.
fn ranged_chunked_dictionary<R: RangeReader + Send + Sync + 'static>(
    reader: &std::sync::Arc<R>,
    header: &Header,
    want: [bool; 4],
    precise_metadata: bool,
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
        let (section, meta, entries) = if precise_metadata {
            let metadata_reader = PreciseMetadataReader {
                inner: reader.as_ref(),
            };
            let section = locate_container_section_ranged(
                &metadata_reader,
                header.dictionary_offset,
                header.dictionary_len,
                si,
                4,
            )?;
            let (meta, entries) = read_dict_dir_ranged(&metadata_reader, section)?;
            (section, meta, entries)
        } else {
            let section = locate_container_section_ranged(
                reader.as_ref(),
                header.dictionary_offset,
                header.dictionary_len,
                si,
                4,
            )?;
            let (meta, entries) = read_dict_dir_ranged(reader.as_ref(), section)?;
            (section, meta, entries)
        };
        let ranges: Vec<ByteRange> = entries
            .iter()
            .map(|e| {
                let offset = checked_end(section.offset, e.start)?;
                let len = e
                    .end
                    .checked_sub(e.start)
                    .ok_or(FileError::Container("dict chunk range overflows"))?;
                materializable_len(len)
                    .map_err(|_| FileError::Container("dict chunk length too large"))?;
                Ok(ByteRange { offset, len })
            })
            .collect::<Result<_, FileError>>()?;
        let chunks: Vec<crate::dict::SectionChunk> = entries
            .into_iter()
            .map(|e| crate::dict::SectionChunk::remote(e.first_run, e.key, e.body_start))
            .collect();
        let chunk_reader = reader.clone();
        let codec = header.dict_codec;
        let loader_ranges = ranges.clone();
        let loader: crate::dict::ChunkLoader = Box::new(move |ci| {
            let range = loader_ranges.get(ci)?;
            let bytes = chunk_reader.read_at(range.offset, range.len).ok()?;
            if bytes.len() as u64 != range.len {
                return None;
            }
            decompress(codec, &bytes).ok()
        });
        // Full-section sweeps (export/dump) batch their chunk fetches:
        // adjacent chunk ranges coalesce into a handful of range reads.
        let bulk_reader = reader.clone();
        let bulk: crate::dict::ChunkBulkLoader = Box::new(move |cis, intent| {
            let want: Option<Vec<ByteRange>> =
                cis.iter().map(|&ci| ranges.get(ci).copied()).collect();
            let blobs = read_coalesced(bulk_reader.as_ref(), &want?, DICT_COALESCE_GAP, intent)?;
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
/// object-only chunk directory alone is 234,400,728 B, because that file stores
/// every chunk's first term verbatim. Files built since #198 key it by the
/// shortest separator (~600 KB on this graph); the published one still does
/// not, and only a rebuild changes that. This view opens sections 0 and 1
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
        let dict = ranged_chunked_dictionary(&reader, &header, [true, true, false, false], false)?;
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

/// Every dictionary section's chunk **routing keys**, exactly as stored, from a
/// finished file image. Test-only: it exists so a test in another module of
/// this crate (`extbuild`) can assert on the directory the writers produce
/// without any of this becoming public surface.
#[cfg(test)]
pub(crate) fn dict_chunk_keys_for_test(image: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let header = Header::from_bytes(&image[..HEADER_LEN]).unwrap();
    let s = header.dictionary_offset as usize;
    let e = s + header.dictionary_len as usize;
    decode_container(&image[s..e], CODEC_NONE, 4)
        .unwrap()
        .iter()
        .map(|payload| {
            let (_meta, entries) = parse_chunked_dict_dir(payload, payload.len() as u64).unwrap();
            entries.into_iter().map(|c| c.key).collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adaptive::{AdaptiveReadController, ReadIntent, ReadObservation};
    use crate::dictionary::DictionaryBuilder;
    use crate::index::GraphIndexBuilder;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    struct SparseReader {
        len: u64,
        bytes: BTreeMap<u64, u8>,
    }

    struct SynopsisProbeReader {
        normal_reads: std::sync::Mutex<Vec<(u64, u64)>>,
        precise_reads: std::sync::Mutex<Vec<(u64, u64)>>,
    }

    struct AdaptiveRecordingReader {
        bytes: Vec<u8>,
        controller: Option<Arc<AdaptiveReadController>>,
        reads: Mutex<Vec<(u64, u64)>>,
    }

    struct NoMaterializeReader {
        calls: std::sync::atomic::AtomicU64,
    }

    impl RangeReader for NoMaterializeReader {
        fn len(&self) -> u64 {
            u64::MAX
        }

        fn read_at(&self, _offset: u64, _len: u64) -> std::io::Result<Vec<u8>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(std::io::Error::other("must not materialize hostile range"))
        }

        fn read_many_with_intent(
            &self,
            _ranges: &[(u64, u64)],
            _intent: ReadIntent,
        ) -> std::io::Result<Vec<Vec<u8>>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Err(std::io::Error::other("must not materialize hostile ranges"))
        }
    }

    impl AdaptiveRecordingReader {
        fn new(len: usize, controller: Option<Arc<AdaptiveReadController>>) -> Self {
            Self {
                bytes: vec![0x5e; len],
                controller,
                reads: Mutex::new(Vec::new()),
            }
        }

        fn reads(&self) -> Vec<(u64, u64)> {
            self.reads.lock().unwrap().clone()
        }
    }

    impl RangeReader for AdaptiveRecordingReader {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            self.reads.lock().unwrap().push((offset, len));
            Ok(self.bytes[offset as usize..(offset + len) as usize].to_vec())
        }

        fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
            ranges
                .iter()
                .map(|&(offset, len)| self.read_at(offset, len))
                .collect()
        }

        fn concurrency(&self) -> usize {
            8
        }

        fn adaptive_controller(&self) -> Option<Arc<AdaptiveReadController>> {
            self.controller.clone()
        }
    }

    fn train_network(controller: &AdaptiveReadController, bytes: u64, elapsed_micros: u64) {
        for _ in 0..2 {
            controller.observe(ReadObservation {
                requested_bytes: bytes,
                returned_bytes: bytes,
                physical_ranges: 1,
                elapsed_micros: Some(elapsed_micros),
                success: true,
            });
        }
    }

    impl SynopsisProbeReader {
        fn new() -> Self {
            Self {
                normal_reads: std::sync::Mutex::new(Vec::new()),
                precise_reads: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn response(len: u64) -> std::io::Result<Vec<u8>> {
            if len > 40 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "oversized synopsis request",
                ));
            }
            Ok(vec![0; len as usize])
        }
    }

    impl SparseReader {
        fn with_uvarint(value: u64, len: u64) -> Self {
            let mut encoded = Vec::new();
            write_uvarint(&mut encoded, value);
            Self {
                len,
                bytes: encoded
                    .into_iter()
                    .enumerate()
                    .map(|(offset, byte)| (offset as u64, byte))
                    .collect(),
            }
        }
    }

    impl RangeReader for SparseReader {
        fn len(&self) -> u64 {
            self.len
        }

        fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            let end = offset
                .checked_add(len)
                .filter(|&end| end <= self.len)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "sparse range out of bounds",
                    )
                })?;
            (offset..end)
                .map(|at| {
                    self.bytes.get(&at).copied().ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "unmapped sparse byte",
                        )
                    })
                })
                .collect()
        }
    }

    impl RangeReader for SynopsisProbeReader {
        fn len(&self) -> u64 {
            u64::MAX
        }

        fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            self.normal_reads.lock().unwrap().push((offset, len));
            Self::response(len)
        }

        fn read_at_precise(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
            self.precise_reads.lock().unwrap().push((offset, len));
            Self::response(len)
        }
    }

    #[test]
    fn synopsis_reads_are_capped_by_format_for_normal_and_precise_paths() {
        let section = ByteRange {
            offset: 0,
            len: u64::MAX,
        };
        // A zero-length tile is syntactically representable. Its directory end
        // puts the synopsis at byte 4 while the hostile virtual section claims
        // that virtually the entire u64 address space remains.
        let directory = [TileDirEntry {
            min_a: 0,
            max_a: 0,
            start: 4,
            end: 4,
        }];

        for precise in [false, true] {
            let reader = SynopsisProbeReader::new();
            assert_eq!(
                read_tile_synopsis_ranged(&reader, section, &directory, precise),
                vec![Some((0, 0, 0, 0))]
            );
            let normal = reader.normal_reads.lock().unwrap().clone();
            let exact = reader.precise_reads.lock().unwrap().clone();
            if precise {
                assert!(normal.is_empty());
                assert_eq!(exact, vec![(4, 40)]);
            } else {
                assert_eq!(normal, vec![(4, 40)]);
                assert!(exact.is_empty());
            }
        }
    }

    #[test]
    fn huge_untrusted_named_graph_count_is_a_clean_error() {
        let len = usize::MAX as u64;
        let reader = SparseReader::with_uvarint(usize::MAX as u64, len);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            open_named_graphs_ranged_lazy(
                std::sync::Arc::new(reader),
                ByteRange { offset: 0, len },
                CODEC_NONE,
                false,
                1,
            )
        }));

        assert!(result.is_ok(), "untrusted graph count must not panic");
        assert!(result.unwrap().is_err(), "huge graph count must fail open");
    }

    #[test]
    fn huge_untrusted_tile_count_is_a_clean_error() {
        let len = usize::MAX as u64;
        let reader = SparseReader::with_uvarint(usize::MAX as u64, len);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_tile_directory_ranged_exact(&reader, ByteRange { offset: 0, len })
        }));

        assert!(result.is_ok(), "untrusted tile count must not panic");
        assert!(result.unwrap().is_err(), "huge tile count must fail open");
    }

    #[test]
    fn chunked_dictionary_directory_rejects_untrusted_u64_ids_and_counts() {
        let mut overlong_header = Vec::new();
        write_uvarint(&mut overlong_header, 3);
        write_uvarint(&mut overlong_header, u32::MAX as u64 + 1);
        write_uvarint(&mut overlong_header, 1);
        write_uvarint(&mut overlong_header, 0);
        assert!(parse_chunked_dict_dir(&overlong_header, overlong_header.len() as u64).is_err());

        let reader = crate::reader::SliceReader::new(&overlong_header);
        assert!(read_dict_dir_ranged(
            &reader,
            ByteRange {
                offset: 0,
                len: overlong_header.len() as u64,
            },
        )
        .is_err());

        let mut overlong_count = Vec::new();
        write_uvarint(&mut overlong_count, u64::MAX);
        assert!(parse_chunk_dir_only(&overlong_count, overlong_count.len() as u64, 1).is_err());

        let mut overlong_run = Vec::new();
        write_uvarint(&mut overlong_run, 1);
        write_uvarint(&mut overlong_run, 0);
        write_uvarint(&mut overlong_run, u64::MAX);
        write_uvarint(&mut overlong_run, 0);
        assert!(parse_chunk_dir_only(&overlong_run, overlong_run.len() as u64, 1).is_err());

        // The physical dictionary namespace is u32 even on a 64-bit host:
        // accepting this run would later alias an ID during lookup.
        let mut first_run_over_u32 = Vec::new();
        write_uvarint(&mut first_run_over_u32, 2);
        for delta in [0, u32::MAX as u64 + 1] {
            write_uvarint(&mut first_run_over_u32, delta);
            write_uvarint(&mut first_run_over_u32, 0);
            write_uvarint(&mut first_run_over_u32, 0);
        }
        assert!(parse_chunk_dir_only(
            &first_run_over_u32,
            first_run_over_u32.len() as u64,
            u64::MAX,
        )
        .is_err());

        let mut overflowing_sum = Vec::new();
        write_uvarint(&mut overflowing_sum, 2);
        for delta in [u64::MAX, 1] {
            write_uvarint(&mut overflowing_sum, delta);
            write_uvarint(&mut overflowing_sum, 0);
            write_uvarint(&mut overflowing_sum, 0);
        }
        assert!(
            parse_chunk_dir_only(&overflowing_sum, overflowing_sum.len() as u64, u64::MAX,)
                .is_err()
        );
    }

    #[cfg(feature = "compression")]
    #[test]
    fn compressed_multi_run_dictionary_keeps_raw_restart_coordinates_valid() {
        let mut builder = crate::dict::DictSectionBuilder::new().with_restart_interval(1);
        for i in 0..2048u32 {
            builder.push(format!(
                "<http://example.org/repetitive/{i:06}/{}>",
                "x".repeat(48)
            ));
        }
        let raw = builder.build();
        let encoded = encode_chunked_dict_section(&raw, CODEC_ZSTD);
        assert!(encoded.len() < raw.len(), "fixture must be compressed");
        let (meta, entries) = parse_chunked_dict_dir(&encoded, encoded.len() as u64).unwrap();
        assert_eq!(meta.term_count, 2048);
        assert!(!entries.is_empty());
        let decoded = decode_chunked_dict_section(&encoded, CODEC_ZSTD).unwrap();
        assert_eq!(decoded.term_count(), 2048);
    }

    #[test]
    fn ranged_dictionary_rejects_wrong_restart_count_like_eager_decode() {
        let mut builder = crate::dict::DictSectionBuilder::new().with_restart_interval(1);
        for i in 0..300u32 {
            builder.push(format!("<http://example.org/{i:03}>"));
        }
        let raw = builder.build();
        let mut encoded = encode_chunked_dict_section(&raw, CODEC_NONE);
        let (header_len, header_start) = read_uvarint(&encoded).unwrap();
        assert!(header_len >= 300);
        assert!(parse_chunked_dict_dir(&encoded, encoded.len() as u64).is_ok());
        let reader = crate::reader::SliceReader::new(&encoded);
        assert!(read_dict_dir_ranged(
            &reader,
            ByteRange {
                offset: 0,
                len: encoded.len() as u64
            },
        )
        .is_ok());
        let (_, term_used) = read_uvarint(&encoded[header_start..]).unwrap();
        let (_, interval_used) = read_uvarint(&encoded[header_start + term_used..]).unwrap();
        let restart_count_pos = header_start + term_used + interval_used;
        // 300 and 301 have the same two-byte LEB128 width, preserving framing.
        encoded[restart_count_pos] = 0xad;
        assert!(parse_chunked_dict_dir(&encoded, encoded.len() as u64).is_err());
        let reader = crate::reader::SliceReader::new(&encoded);
        assert!(read_dict_dir_ranged(
            &reader,
            ByteRange {
                offset: 0,
                len: encoded.len() as u64
            },
        )
        .is_err());
    }

    #[test]
    fn ranged_dictionary_rejects_restart_table_shorter_than_declared_count() {
        // `[header_len=3][term_count=3][interval=1][restart_count=3]` names
        // three restart offsets but has no room for even one. The following
        // bytes form a superficially valid one-chunk directory.
        let malformed = [3, 3, 1, 3, 1, 0, 0, 0];
        assert!(parse_chunked_dict_dir(&malformed, malformed.len() as u64).is_err());
        let reader = crate::reader::SliceReader::new(&malformed);
        assert!(read_dict_dir_ranged(
            &reader,
            ByteRange {
                offset: 0,
                len: malformed.len() as u64
            },
        )
        .is_err());
    }

    #[test]
    fn allocation_sized_untrusted_tile_count_is_a_clean_error() {
        let count = (isize::MAX as usize / std::mem::size_of::<TileDirEntry>()) + 1;
        assert!(count.checked_mul(3).is_some());
        let reader = SparseReader::with_uvarint(count as u64, u64::MAX);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            read_tile_directory_ranged_exact(
                &reader,
                ByteRange {
                    offset: 0,
                    len: u64::MAX,
                },
            )
        }));

        assert!(result.is_ok(), "untrusted tile count must not panic");
        assert!(result.unwrap().is_err(), "huge tile count must fail open");
    }

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
        let out = read_coalesced(&r, &ranges, 16, ReadIntent::SelectiveProbe).unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(r.requests(), 3);
        // Gap 64 merges A+B (gap 32) but not C (gap 1024) → two reads.
        let r = CountingReader::new(SliceReader::new(&bytes));
        read_coalesced(&r, &ranges, 64, ReadIntent::SelectiveProbe).unwrap();
        assert_eq!(r.requests(), 2);
        // Gap 4096 merges all three into one read, over-fetching the gaps.
        let r = CountingReader::new(SliceReader::new(&bytes));
        read_coalesced(&r, &ranges, 4096, ReadIntent::SelectiveProbe).unwrap();
        assert_eq!(r.requests(), 1);
    }

    #[test]
    fn adaptive_coalescing_uses_the_observed_network_break_even() {
        let ranges = [
            ByteRange {
                offset: 0,
                len: 64 * 1024,
            },
            ByteRange {
                offset: 96 * 1024,
                len: 64 * 1024,
            },
            ByteRange {
                offset: 672 * 1024,
                len: 64 * 1024,
            },
        ];

        let fast_high_rtt = Arc::new(AdaptiveReadController::new());
        train_network(&fast_high_rtt, 1024 * 1024, 120_000);
        let high = AdaptiveRecordingReader::new(1024 * 1024, Some(fast_high_rtt));
        let out = read_coalesced(&high, &ranges, 4096, ReadIntent::SelectiveProbe).unwrap();
        assert_eq!(out, vec![vec![0x5e; 64 * 1024]; 3]);
        assert_eq!(high.reads(), vec![(0, 160 * 1024), (672 * 1024, 64 * 1024)]);

        let slow_low_rtt = Arc::new(AdaptiveReadController::new());
        train_network(&slow_low_rtt, 4 * 1024, 20_000);
        let low = AdaptiveRecordingReader::new(1024 * 1024, Some(slow_low_rtt));
        read_coalesced(&low, &ranges, 4096, ReadIntent::SelectiveProbe).unwrap();
        assert_eq!(
            low.reads(),
            vec![
                (0, 64 * 1024),
                (96 * 1024, 64 * 1024),
                (672 * 1024, 64 * 1024),
            ]
        );
    }

    #[test]
    fn adaptive_coalescing_honors_gap_and_span_caps() {
        let controller = Arc::new(AdaptiveReadController::new());
        train_network(&controller, 1024 * 1024, 120_000);
        let reader = AdaptiveRecordingReader::new(4 * 1024 * 1024, Some(controller));
        let ranges = [
            ByteRange {
                offset: 0,
                len: 1024 * 1024,
            },
            ByteRange {
                offset: 1024 * 1024,
                len: 1024 * 1024,
            },
            ByteRange {
                offset: 2 * 1024 * 1024,
                len: 1024 * 1024,
            },
        ];

        read_coalesced(&reader, &ranges, 64 * 1024, ReadIntent::FullScan).unwrap();

        let reads = reader.reads();
        assert_eq!(
            reads,
            vec![(0, 2 * 1024 * 1024), (2 * 1024 * 1024, 1024 * 1024)]
        );
        assert!(reads.iter().all(|&(_, len)| len <= 2 * 1024 * 1024));
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

    fn tiny_family_index() -> GraphIndex {
        let mut builder = GraphIndexBuilder::new();
        builder.push((1, 1, 1));
        builder.build()
    }

    #[test]
    fn family_container_matches_literal_bytes() {
        assert_eq!(NEXT_FORMAT_VERSION, 0x06);
        let index = tiny_family_index();
        assert_eq!(
            encode_family_container(
                index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
                CODEC_NONE,
            )
            .unwrap(),
            vec![
                1, 8, 8, // pair count; directory length; trailer length
                1, 0, // shared leading range delta/span
                0, 13, 7, // first order: flags, compressed length, prefix-2 length
                0, 13, 7, // second order: flags, compressed length, prefix-2 length
                1, 1, 10, 1, 1, 12, 1, // first complete prefix-2 directory
                1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // first TripleBlock
                1, 1, 10, 1, 1, 12, 1, // second complete prefix-2 directory
                1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, // second TripleBlock
                1, 0, 1, 0, // first synopsis trailer
                1, 0, 1, 0, // second synopsis trailer
            ]
        );
    }

    #[test]
    fn eager_container_rejects_unmaterializable_lengths_without_panicking() {
        let mut hostile = vec![1];
        write_uvarint(&mut hostile, u64::MAX);
        let result = std::panic::catch_unwind(|| decode_container(&hostile, CODEC_NONE, 1));
        assert!(
            result.is_ok(),
            "hostile eager container length must not panic"
        );
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn eager_container_rejects_unmaterializable_counts_without_panicking() {
        let mut hostile = Vec::new();
        write_uvarint(&mut hostile, u64::MAX);
        let result = std::panic::catch_unwind(|| decode_container(&hostile, CODEC_NONE, 1));
        assert!(
            result.is_ok(),
            "hostile eager container count must not panic"
        );
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn fixed_container_count_rejects_hostile_empty_sections_before_decode() {
        let declared = 1024usize;
        let mut hostile = Vec::new();
        write_uvarint(&mut hostile, declared as u64);
        hostile.extend(std::iter::repeat_n(0, declared));

        let error = decode_container(&hostile, CODEC_NONE, 4).unwrap_err();
        assert!(error
            .to_string()
            .contains("unexpected container section count"));
    }

    #[test]
    fn eager_open_rejects_unrepresentable_resident_section_offsets() {
        let mut bytes = build_image();
        let mut header = Header::from_bytes(&bytes).unwrap();
        header.dictionary_offset = u64::from(u32::MAX) + 1;
        header.dictionary_len = 1;
        bytes[..HEADER_LEN].copy_from_slice(&header.to_bytes());

        let result = std::panic::catch_unwind(|| Rete::open(&bytes));
        assert!(result.is_ok());
        assert!(result.unwrap().is_err());
    }

    #[test]
    fn coalesced_reads_reject_unmaterializable_range_before_physical_io() {
        let reader = NoMaterializeReader {
            calls: std::sync::atomic::AtomicU64::new(0),
        };
        assert!(read_coalesced(
            &reader,
            &[ByteRange {
                offset: 0,
                len: u64::MAX,
            }],
            0,
            ReadIntent::SelectiveProbe,
        )
        .is_none());
        assert_eq!(reader.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn coalescing_rejects_an_unmaterializable_merged_span_before_io() {
        let reader = NoMaterializeReader {
            calls: std::sync::atomic::AtomicU64::new(0),
        };
        let ranges = [
            ByteRange { offset: 0, len: 1 },
            ByteRange {
                offset: isize::MAX as u64,
                len: 1,
            },
        ];
        assert!(read_coalesced(&reader, &ranges, u64::MAX, ReadIntent::SelectiveProbe,).is_none());
        assert_eq!(reader.calls.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn dictionary_directory_rejects_unmaterializable_chunk_before_loading() {
        let mut directory = Vec::new();
        write_uvarint(&mut directory, 1); // one chunk for one restart run
        write_uvarint(&mut directory, 0); // first run delta
        write_uvarint(&mut directory, 1); // first-term length
        directory.push(b'x');
        write_uvarint(&mut directory, isize::MAX as u64 + 1);

        assert!(parse_chunk_dir_only(&directory, u64::MAX, 1).is_err());
    }

    #[test]
    fn eager_dictionary_directory_rejects_unmaterializable_chunk_before_loading() {
        let mut header = Vec::new();
        write_uvarint(&mut header, 1); // one term
        write_uvarint(&mut header, 1); // restart interval
        write_uvarint(&mut header, 1); // one restart offset
        write_uvarint(&mut header, 0); // raw body begins immediately after header

        let mut payload = Vec::new();
        write_uvarint(&mut payload, header.len() as u64);
        payload.extend_from_slice(&header);
        write_uvarint(&mut payload, 1); // one chunk
        write_uvarint(&mut payload, 0); // first run delta
        write_uvarint(&mut payload, 1); // first term length
        payload.push(b'x');
        write_uvarint(&mut payload, isize::MAX as u64 + 1);

        assert!(parse_chunked_dict_dir(&payload, u64::MAX).is_err());
    }

    #[test]
    fn empty_family_root_is_three_zero_count_families() {
        let index = GraphIndexBuilder::new().build();
        assert_eq!(
            encode_family_index_container(&index, CODEC_NONE).unwrap(),
            vec![3, 3, 0, 0, 0, 3, 0, 0, 0, 3, 0, 0, 0]
        );
    }

    #[test]
    fn corrupt_lazy_sop_is_isolated_from_spo_and_marks_selected_scan_incomplete() {
        let triple = ("<http://ex/s>", "<http://ex/p>", "<http://ex/o>");
        let mut db = crate::DictionaryBuilder::new();
        db.observe(triple.0, triple.1, triple.2);
        let dict = db.build();
        let ids = dict.encode(triple.0, triple.1, triple.2).unwrap();
        let mut builder = GraphIndexBuilder::new().with_tile_budget(64);
        builder.push(ids);
        let mut image = write_file(&dict, &builder.build(), false, &[], 0);
        let header = Header::from_bytes(&image).unwrap();
        assert_eq!(header.version, NEXT_FORMAT_VERSION);

        let subject = {
            let reader = crate::reader::SliceReader::new(&image);
            locate_container_sections_ranged_exact_dynamic(
                &reader,
                ByteRange {
                    offset: header.root_dir_offset,
                    len: header.root_dir_len,
                },
                3,
            )
            .unwrap()[0]
        };
        let second_records = {
            let reader = crate::reader::SliceReader::new(&image);
            read_family_directory_ranged(&reader, subject)
                .unwrap()
                .second_records
        };
        for (record, prefix_len, compressed_len) in second_records {
            let start = (record.offset + prefix_len) as usize;
            let end = (record.offset + prefix_len + compressed_len) as usize;
            image[start..end].fill(0xff);
        }

        let leaked: &'static [u8] = Box::leak(image.into_boxed_slice());
        let rete = Rete::open_ranged_lazy(crate::reader::SliceReader::new(leaked)).unwrap();
        assert_eq!(
            rete.default_index()
                .match_pattern((Some(ids.0), None, None)),
            vec![ids]
        );
        assert!(
            !rete.index_incomplete(),
            "SPO must not depend on corrupt SOP"
        );

        rete.reset_load_failures();
        let rows: Vec<_> = rete
            .default_index()
            .scan_iter_sorted_on((Some(ids.0), None, None), 2)
            .unwrap()
            .collect();
        assert!(rows.is_empty());
        assert!(
            rete.index_incomplete(),
            "selecting the corrupt SOP sibling must mark the scan incomplete"
        );
    }

    #[test]
    fn family_container_round_trips_prefix2_and_both_orders() {
        let index = tiny_family_index();
        let bytes = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();
        let decoded = decode_family_container(&bytes, CODEC_NONE).unwrap();
        assert_eq!(decoded.first.len(), 1);
        assert_eq!(decoded.second.len(), 1);
        assert_eq!(decoded.first[0].bytes(), &[1; 13]);
        assert_eq!(decoded.second[0].bytes(), &[1; 13]);
        assert_eq!(
            decoded.directory.first_prefix2[0].groups[0].a_body_offset,
            10
        );
        assert_eq!(
            decoded.directory.second_prefix2[0].groups[0].b_entries[0],
            (1, 12, 1)
        );
    }

    #[test]
    fn family_encoder_rejects_raw_tile_over_fixed_decoder_limit() {
        let mut builder = GraphIndexBuilder::new().with_tile_budget(256 * 1024);
        for id in 1..20_000u32 {
            builder.push((id, id, id));
        }
        let index = builder.build();
        assert!(matches!(
            encode_family_container(
                index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
                CODEC_NONE,
            ),
            Err(FileError::Container(
                "family tile exceeds fixed decompressed limit"
            ))
        ));
    }

    #[test]
    fn family_literal_directory_and_trailer_cannot_create_false_pruning() {
        let index = tiny_family_index();
        let literal = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();

        // Literal byte 3 is the shared leading-range delta.  If accepted, the
        // route claims a=2 while the parsed blocks actually contain a=1.
        let mut wrong_route = literal.clone();
        wrong_route[3] = 2;
        assert!(decode_family_container(&wrong_route, CODEC_NONE).is_err());

        // Literal byte 51 is the first trailer's min-b.  If accepted, this
        // forged synopsis would let `syn_admits(Some(1), _)` prune its own row.
        let mut wrong_synopsis = literal;
        wrong_synopsis[51] = 2;
        assert!(decode_family_container(&wrong_synopsis, CODEC_NONE).is_err());
    }

    #[test]
    fn family_varints_are_canonical_and_bounded() {
        for malformed in [
            vec![0x80, 0x00],
            vec![0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x02],
        ] {
            assert!(decode_family_container(&malformed, CODEC_NONE).is_err());
        }
    }

    #[test]
    fn family_rejects_prefix2_length_over_fixed_budget_before_record_decode() {
        let mut directory = vec![1, 0, 0, 0];
        write_uvarint(&mut directory, PREFIX2_FORMAT_BUDGET as u64 + 1);
        directory.extend_from_slice(&[0, 0, 0]);
        let trailer = [0; 8];
        let mut malformed = Vec::new();
        write_uvarint(&mut malformed, 1);
        write_uvarint(&mut malformed, directory.len() as u64);
        write_uvarint(&mut malformed, trailer.len() as u64);
        malformed.extend_from_slice(&directory);
        malformed.extend_from_slice(&trailer);
        assert!(matches!(
            decode_family_container(&malformed, CODEC_NONE),
            Err(FileError::Container("prefix-2 blob exceeds fixed budget"))
        ));
    }

    #[cfg(feature = "compression")]
    #[test]
    fn family_zstd_requires_one_bounded_exact_frame() {
        use std::io::Write;

        let mut high_window_encoder = zstd::stream::Encoder::new(Vec::new(), 0).unwrap();
        high_window_encoder.include_contentsize(false).unwrap();
        high_window_encoder.window_log(20).unwrap();
        high_window_encoder.write_all(&[1; 13]).unwrap();
        let high_window = high_window_encoder.finish().unwrap();
        assert!(matches!(
            decompress_family_tile_exact(CODEC_ZSTD, &high_window),
            Err(FileError::Container(
                "family zstd window exceeds fixed limit"
            ))
        ));

        let compressed = compress(CODEC_ZSTD, &[1; 13]);
        let mut garbage = compressed.clone();
        garbage.extend_from_slice(b"garbage");
        assert!(decompress_family_tile_exact(CODEC_ZSTD, &garbage).is_err());

        let mut second_frame = compressed;
        second_frame.extend_from_slice(&compress(CODEC_ZSTD, &[2; 13]));
        assert!(decompress_family_tile_exact(CODEC_ZSTD, &second_frame).is_err());

        let huge = compress(CODEC_ZSTD, &vec![0; FAMILY_TILE_DECOMPRESSED_MAX + 1]);
        assert!(decompress_family_tile_exact(CODEC_ZSTD, &huge).is_err());

        let boundary = vec![0x5a; FAMILY_TILE_DECOMPRESSED_MAX];
        let boundary_frame = family_compress(CODEC_ZSTD, &boundary).unwrap();
        assert_eq!(
            decompress_family_tile_exact(CODEC_ZSTD, &boundary_frame).unwrap(),
            boundary
        );
    }

    #[test]
    fn family_container_rejects_malformed_directory_before_payload_allocation() {
        let malformed = [
            &[0x80][..],                                  // truncated count
            &[1, 1, 0, 4, 0, 0, 0, 0, 0][..],             // reserved first-order flag
            &[1, 1, 0, 0x80, 0x80, 0x80, 0x80, 0x10][..], // compressed length overflows u32
            &[0xff, 0xff, 0xff, 0xff, 0x0f][..],          // huge count with no directory bytes
        ];
        for bytes in malformed {
            let result = std::panic::catch_unwind(|| decode_family_container(bytes, CODEC_NONE));
            assert!(result.is_ok(), "malformed family bytes must not panic");
            assert!(result.unwrap().is_err(), "malformed family bytes must fail");
        }

        // One tile has exactly eight directory varints and eight trailer
        // varints. Even at ten bytes each, a declared region larger than 80
        // bytes is impossible and must fail before it is materialized.
        let impossible_directory = [1, 81, 8];
        assert!(matches!(
            decode_family_container(&impossible_directory, CODEC_NONE),
            Err(FileError::Container(
                "family directory exceeds varint framing"
            ))
        ));
        let impossible_trailer = [1, 8, 81];
        assert!(matches!(
            decode_family_container(&impossible_trailer, CODEC_NONE),
            Err(FileError::Container(
                "family trailer exceeds varint framing"
            ))
        ));
    }

    #[test]
    fn family_container_rejects_bad_continuations_payloads_prefix2_and_trailers() {
        let index = tiny_family_index();
        let mut bytes = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();

        bytes[5] = 4; // reserved flag
        assert!(decode_family_container(&bytes, CODEC_NONE).is_err());

        let mut bytes = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();
        bytes[7] = 6; // prefix-2 blob now ends before its final c-count
        assert!(decode_family_container(&bytes, CODEC_NONE).is_err());

        let mut bytes = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();
        let first_payload = 11 + 7;
        bytes[first_payload] = 0; // corrupt the leading zone map, conflicting with the directory range
        assert!(decode_family_container(&bytes, CODEC_NONE).is_err());

        let mut bytes = encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE,
        )
        .unwrap();
        let last = bytes.len() - 1;
        bytes[last] = 0x80; // truncated second synopsis trailer
        assert!(decode_family_container(&bytes, CODEC_NONE).is_err());

        let mut directory = Vec::new();
        write_uvarint(&mut directory, 1);
        write_uvarint(&mut directory, 0);
        write_uvarint(&mut directory, 0);
        write_uvarint(&mut directory, 0);
        for flags in [2u8, 1] {
            write_uvarint(&mut directory, flags as u64);
            write_uvarint(&mut directory, 13);
            write_uvarint(&mut directory, 0);
        }
        for flags in [2u8, 1] {
            write_uvarint(&mut directory, flags as u64);
            write_uvarint(&mut directory, 13);
            write_uvarint(&mut directory, 0);
        }
        let trailer = [1, 0, 1, 0].repeat(4);
        let mut continuation = Vec::new();
        write_uvarint(&mut continuation, 2);
        write_uvarint(&mut continuation, directory.len() as u64);
        write_uvarint(&mut continuation, trailer.len() as u64);
        continuation.extend_from_slice(&directory);
        for _ in 0..4 {
            continuation.extend_from_slice(&[1; 13]);
        }
        continuation.extend_from_slice(&trailer);
        assert!(decode_family_container(&continuation, CODEC_NONE).is_ok());
        continuation[13] = 0; // second-order first flag no longer matches its sibling.
        assert!(decode_family_container(&continuation, CODEC_NONE).is_err());
    }

    #[test]
    fn family_container_codec_rules_cover_none_and_zstd_without_encoder_feature() {
        let index = tiny_family_index();
        assert!(encode_family_container(
            index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
            CODEC_NONE
        )
        .is_ok());
        if cfg!(feature = "compression") {
            let encoded = encode_family_container(
                index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
                CODEC_ZSTD,
            )
            .unwrap();
            assert!(decode_family_container(&encoded, CODEC_ZSTD).is_ok());
        } else {
            assert!(encode_family_container(
                index.family_view(crate::build_pipeline::family::IndexFamily::Subject),
                CODEC_ZSTD
            )
            .is_err());
        }
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
    /// dictionary's chunk directory.
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
        // Long literals that share a 3 KiB prefix with their neighbour, each
        // ending in a 1 KiB tail of its own: the WKT-polygon / JSON-blob shape
        // (#198's weak case). The directory stores the SHORTEST SEPARATOR per
        // chunk, and a separator has to reproduce the whole shared prefix — so
        // these keys are still ~3 KiB and the directory still runs well past
        // the 8 KiB header prefix, which is what gives the probe something to
        // measure.
        //
        // This fixture used to put the varying bytes FIRST (`"{i:04} xxxx…"`).
        // That shape is the one separators annihilate: its keys collapse to ~4
        // bytes, the whole directory to 192 B, it fits the header prefix, and
        // the probe never runs — the assertion below then fails naming itself.
        // Keep the shared prefix.
        let head = "x".repeat(3000);
        let triples: Vec<(String, String, String)> = (0..2000u32)
            .map(|i| {
                let tail = char::from(b'a' + (i % 26) as u8).to_string().repeat(1000);
                (
                    format!("<http://ex/s/{i:04}>"),
                    "<http://ex/abstract>".to_string(),
                    format!("\"{head}{i:04}{tail}\""),
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

    // ---------------------------------------------------------------------
    // Separator routing keys (#198 option (d))
    // ---------------------------------------------------------------------

    /// A verbatim-keyed twin of a chunked dictionary section: same header, same
    /// chunking, byte-identical compressed bodies — only every directory key
    /// replaced by the chunk's actual first term. That is exactly what
    /// `encode_chunked_dict_section` wrote before separator keys, so it is the
    /// control for every comparison below (and for the proptest).
    fn verbatim_keyed_twin(payload: &[u8], codec: u8) -> Vec<u8> {
        let (_meta, entries) = parse_chunked_dict_dir(payload, payload.len() as u64).unwrap();
        let (header_len, n0) = read_uvarint(payload).unwrap();
        let mut out = Vec::new();
        write_uvarint(&mut out, header_len);
        out.extend_from_slice(&payload[n0..n0 + header_len as usize]);
        write_uvarint(&mut out, entries.len() as u64);
        let mut prev_run = 0usize;
        for e in &entries {
            let body = decompress(codec, &payload[e.start as usize..e.end as usize]).unwrap();
            let first = crate::dict::run_first_term(&body, 0).unwrap();
            write_uvarint(&mut out, (e.first_run - prev_run) as u64);
            write_uvarint(&mut out, first.len() as u64);
            out.extend_from_slice(&first);
            write_uvarint(&mut out, e.end - e.start);
            prev_run = e.first_run;
        }
        for e in &entries {
            out.extend_from_slice(&payload[e.start as usize..e.end as usize]);
        }
        out
    }

    /// Each chunk's (first term, last term), decoded from the bodies.
    fn chunk_bounds_of(payload: &[u8], codec: u8) -> Vec<(Vec<u8>, Vec<u8>)> {
        let (_meta, entries) = parse_chunked_dict_dir(payload, payload.len() as u64).unwrap();
        entries
            .iter()
            .map(|e| {
                let body = decompress(codec, &payload[e.start as usize..e.end as usize]).unwrap();
                (
                    crate::dict::run_first_term(&body, 0).unwrap(),
                    crate::dict::run_last_term(&body, 0, body.len()).unwrap(),
                )
            })
            .collect()
    }

    /// A term set built to hit every shape the corpus has: plain IRIs, literals
    /// sharing a long prefix with their neighbour (separators are LONG there —
    /// the WKT/`ohm-full` weak case), literals that diverge in their first bytes
    /// (separators are tiny — the `epfl-infoscience` case), literals larger than
    /// `DICT_CHUNK_BUDGET` on their own, and multibyte UTF-8.
    fn adversarial_terms() -> Vec<String> {
        let mut terms: Vec<String> = Vec::new();
        for i in 0..6_000 {
            terms.push(format!("<http://example.org/work/{i:06}>"));
        }
        let shared = "X".repeat(900);
        for i in 0..4_000 {
            terms.push(format!("\"{shared}-{i:05}-tail\""));
        }
        let deep = "Y".repeat(3_000); // shared prefix far past any short key
        for i in 0..120 {
            terms.push(format!("\"{deep}{i:03}\""));
        }
        for i in 0..6 {
            // one literal larger than the 64 KiB budget: a chunk of one run
            terms.push(format!("\"BIG-{i:02}-{}\"", "Z".repeat(90_000)));
        }
        for i in 0..800 {
            terms.push(format!(
                "\"h\u{e9}llo-{}-\u{1F600}{i:04}\"",
                "\u{e9}".repeat(40)
            ));
        }
        for i in 0..2_000 {
            terms.push(format!("\"{i:05}-{}\"", "T".repeat(1_800)));
        }
        terms.sort();
        terms.dedup();
        terms
    }

    fn chunked_section_of(terms: &[String]) -> Vec<u8> {
        let mut b = crate::dict::DictSectionBuilder::new().with_restart_interval(16);
        for t in terms {
            b.push(t.clone());
        }
        b.build()
    }

    /// The invariant, asserted against the real chunk contents: the stored key
    /// of chunk `i` is strictly above chunk `i-1`'s LAST term and at most chunk
    /// `i`'s own first term, chunk 0's key is empty, and the keys ascend — the
    /// three facts `ChunkedSection::id`'s `partition_point` relies on.
    ///
    /// Plus the anti-regression the rename exists for: on a section like this
    /// the key must NOT be the first term. A writer that quietly went back to
    /// storing terms would pass every other test in this file — dumps and
    /// `term(id)` route by `first_run` and never read the key.
    #[test]
    fn chunk_directory_keys_are_separators_not_terms() {
        let terms = adversarial_terms();
        let raw = chunked_section_of(&terms);
        let payload = encode_chunked_dict_section(&raw, CODEC_NONE);
        let (_meta, entries) = parse_chunked_dict_dir(&payload, payload.len() as u64).unwrap();
        let bounds = chunk_bounds_of(&payload, CODEC_NONE);
        assert!(
            entries.len() > 20,
            "need a many-chunk section, got {}",
            entries.len()
        );

        assert!(entries[0].key.is_empty(), "chunk 0 needs no separator");
        let mut key_bytes = 0usize;
        let mut term_bytes = 0usize;
        let mut differ = 0usize;
        for (i, e) in entries.iter().enumerate() {
            key_bytes += e.key.len();
            term_bytes += bounds[i].0.len();
            if e.key != bounds[i].0 {
                differ += 1;
            }
            assert!(
                e.key <= bounds[i].0,
                "chunk {i}: key must be <= its own first term"
            );
            if i > 0 {
                assert!(
                    e.key.as_slice() > bounds[i - 1].1.as_slice(),
                    "chunk {i}: key must be > the previous chunk's last term"
                );
                assert!(
                    e.key > entries[i - 1].key,
                    "chunk {i}: keys must ascend for partition_point"
                );
                // shortest: one byte past the divergence from the predecessor
                assert_eq!(
                    e.key,
                    crate::dict::shortest_separator(&bounds[i - 1].1, &bounds[i].0),
                    "chunk {i}: key is not the SHORTEST separator"
                );
            }
        }
        assert!(
            differ > 0,
            "every key equalled its first term — the writer stores terms again"
        );
        assert!(
            key_bytes * 4 < term_bytes,
            "separators bought almost nothing: {key_bytes} B vs {term_bytes} B of first terms"
        );
        eprintln!(
            "chunks={} first-term keys={term_bytes} B  separator keys={key_bytes} B ({:.1}x)",
            entries.len(),
            term_bytes as f64 / key_bytes.max(1) as f64
        );
    }

    /// A separator-keyed section answers **identically** to the verbatim-keyed
    /// one built over the same bodies, under the same unmodified reader: every
    /// id, every term, and every boundary probe — the first and last term of
    /// each chunk, one byte either side of each, strings that fall strictly
    /// *between* two chunks, truncations, the empty string, and probes below
    /// and above the whole section.
    ///
    /// Probing `id(term)` is the entire point. A broken key (a truncation, a
    /// hash) leaves `term(id)`, `dump` and `export` byte-perfect — they route by
    /// `first_run` — and only lookups lie.
    #[test]
    fn separator_keys_route_identically_to_verbatim_first_terms() {
        let terms = adversarial_terms();
        let raw = chunked_section_of(&terms);
        let sep_payload = encode_chunked_dict_section(&raw, CODEC_NONE);
        let ver_payload = verbatim_keyed_twin(&sep_payload, CODEC_NONE);

        // Same chunking, byte-identical bodies — only the keys differ.
        let (_ms, es) = parse_chunked_dict_dir(&sep_payload, sep_payload.len() as u64).unwrap();
        let (_mv, ev) = parse_chunked_dict_dir(&ver_payload, ver_payload.len() as u64).unwrap();
        assert_eq!(es.len(), ev.len());
        for (a, b) in es.iter().zip(ev.iter()) {
            assert_eq!(a.first_run, b.first_run);
            assert_eq!(
                &sep_payload[a.start as usize..a.end as usize],
                &ver_payload[b.start as usize..b.end as usize],
                "chunk bodies must be identical"
            );
        }
        assert!(
            sep_payload.len() < ver_payload.len(),
            "separator payload is not smaller"
        );

        let sec_s = decode_chunked_dict_section(&sep_payload, CODEC_NONE).unwrap();
        let sec_v = decode_chunked_dict_section(&ver_payload, CODEC_NONE).unwrap();

        // 1. every present term → the same, correct id
        for (i, t) in terms.iter().enumerate() {
            let want = Some(i as u32 + 1);
            assert_eq!(sec_v.id(t), want, "verbatim id() lost a term");
            assert_eq!(sec_s.id(t), want, "separator id() lost a term");
        }
        // 2. every id → the same term
        for i in 0..terms.len() as u32 {
            assert_eq!(sec_s.term(i + 1), sec_v.term(i + 1));
            assert_eq!(
                sec_s.term(i + 1).as_deref(),
                Some(terms[i as usize].as_str())
            );
        }
        // 3. boundary probes, from the real chunk bounds
        let bounds = chunk_bounds_of(&sep_payload, CODEC_NONE);
        let mut probes: Vec<String> = Vec::new();
        let push = |bytes: &[u8], probes: &mut Vec<String>| {
            if let Ok(s) = std::str::from_utf8(bytes) {
                probes.push(s.to_string());
            }
        };
        for (first, last) in &bounds {
            for base in [first, last] {
                push(base, &mut probes); // present
                let mut after = base.clone();
                after.push(0x01); // immediately after — between chunks when base = last
                push(&after, &mut probes);
                let mut far = base.clone();
                far.push(b'~');
                push(&far, &mut probes);
                if !base.is_empty() {
                    push(&base[..base.len() - 1], &mut probes); // truncated
                    let mut down = base.clone();
                    *down.last_mut().unwrap() = down.last().unwrap().wrapping_sub(1);
                    push(&down, &mut probes);
                    let mut up = base.clone();
                    *up.last_mut().unwrap() = up.last().unwrap().wrapping_add(1);
                    push(&up, &mut probes);
                }
            }
        }
        // 4. the stored separators themselves, and the ends of the world
        for e in &es {
            push(&e.key, &mut probes);
            let mut plus = e.key.clone();
            plus.push(0);
            push(&plus, &mut probes);
        }
        probes.push(String::new()); // the empty key, i.e. chunk 0's
        probes.push("\u{1}".to_string()); // below every term
        probes.push("\u{10FFFF}\u{10FFFF}".to_string()); // above every term
        probes.push(terms.first().unwrap().clone());
        probes.push(terms.last().unwrap().clone());

        let mut hits = 0usize;
        for p in &probes {
            let (a, b) = (sec_v.id(p), sec_s.id(p));
            assert_eq!(
                a,
                b,
                "probe diverged: {:?}",
                p.get(..p.len().min(48)).unwrap_or(p.as_str())
            );
            hits += usize::from(a.is_some());
        }
        assert!(
            probes.len() > 500 && hits > 50,
            "probe set is too thin: {} probes, {hits} of them present",
            probes.len()
        );
        eprintln!(
            "{} boundary probes, {hits} present, all identical",
            probes.len()
        );
    }

    /// The same claim as a **property**: for arbitrary term sets and arbitrary
    /// probes, a separator-keyed section and the verbatim-keyed twin built from
    /// the same input are indistinguishable through the reader. The fixed test
    /// above picks the shapes I thought of; this one shrinks a counterexample
    /// out of the ones I did not.
    mod separator_props {
        use super::*;
        use proptest::prelude::*;

        /// Sorted, deduplicated term sets mixing the three shapes that decide a
        /// separator's length: short IRIs, literals whose payload comes BEFORE
        /// the discriminating bytes (long shared prefixes ⇒ long separators),
        /// and literals that diverge immediately (⇒ one-byte separators). Sizes
        /// straddle `DICT_CHUNK_BUDGET`, so single- and many-chunk sections both
        /// occur.
        fn term_set() -> impl Strategy<Value = Vec<String>> {
            prop::collection::vec(("[a-c]{1,6}", 0u8..3, 300usize..1200), 60..250).prop_map(
                |specs| {
                    let mut terms: Vec<String> = specs
                        .into_iter()
                        .enumerate()
                        .map(|(i, (core, kind, pad))| {
                            let block = "z".repeat(pad);
                            match kind {
                                0 => format!("<http://ex/{core}/{i:05}>"),
                                1 => format!("\"{block}{core}{i:05}\""),
                                _ => format!("\"{core}{i:05}{block}\""),
                            }
                        })
                        .collect();
                    terms.sort();
                    terms.dedup();
                    terms
                },
            )
        }

        proptest! {
            #[test]
            fn prop_separator_keyed_section_answers_like_verbatim(
                terms in term_set(),
                extra in prop::collection::vec(any::<String>(), 0..8),
            ) {
                let raw = chunked_section_of(&terms);
                let sep = encode_chunked_dict_section(&raw, CODEC_NONE);
                let ver = verbatim_keyed_twin(&sep, CODEC_NONE);
                prop_assert!(sep.len() <= ver.len());

                // The separator invariant, against the real chunk contents.
                let (_m, es) = parse_chunked_dict_dir(&sep, sep.len() as u64).unwrap();
                let bounds = chunk_bounds_of(&sep, CODEC_NONE);
                prop_assert!(es[0].key.is_empty());
                for i in 1..es.len() {
                    prop_assert!(es[i].key.as_slice() > bounds[i - 1].1.as_slice());
                    prop_assert!(es[i].key <= bounds[i].0);
                    prop_assert!(es[i].key > es[i - 1].key);
                }

                let sec_s = decode_chunked_dict_section(&sep, CODEC_NONE).unwrap();
                let sec_v = decode_chunked_dict_section(&ver, CODEC_NONE).unwrap();

                for (i, t) in terms.iter().enumerate() {
                    prop_assert_eq!(sec_s.id(t), Some(i as u32 + 1));
                    prop_assert_eq!(sec_v.id(t), Some(i as u32 + 1));
                    prop_assert_eq!(sec_s.term(i as u32 + 1), sec_v.term(i as u32 + 1));
                }

                // Probes: arbitrary strings, plus mutations of every boundary.
                let mut probes: Vec<String> = extra;
                probes.push(String::new());
                for (first, last) in &bounds {
                    for base in [first, last] {
                        if let Ok(s) = std::str::from_utf8(base) {
                            probes.push(s.to_string());
                            probes.push(format!("{s}\u{1}"));
                            probes.push(s[..s.len() - s.chars().next_back()
                                .map(char::len_utf8).unwrap_or(0)].to_string());
                        }
                    }
                }
                for e in &es {
                    if let Ok(s) = std::str::from_utf8(&e.key) {
                        probes.push(s.to_string());
                    }
                }
                for p in &probes {
                    prop_assert_eq!(sec_s.id(p), sec_v.id(p), "probe diverged: {:?}", p);
                }
            }
        }
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
        let mut ib = GraphIndexBuilder::new()
            .with_tile_budget(64)
            // This test mutates the legacy optional-synopsis header flag. In
            // 0x06 family trailers are mandatory physical metadata.
            .with_perms(PermSet::CORE);
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
            bytes.len() < raw * 2 / 3,
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
            LazyIndexConfig {
                block_codec: header.block_codec,
                has_synopsis: header.has_tile_synopsis(),
                read_concurrency: 1,
                perms: header.perms,
                exact_payload_boundaries: false,
                version: header.version,
            },
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

    /// **The filtered-dump correctness invariant** (#117 point 1): for every
    /// graph and every one of the eight bound/unbound shapes,
    /// `dump_filtered_each` must return *exactly* the quads `dump_each` returns,
    /// filtered — no more, and above all no fewer.
    ///
    /// "No fewer" is the whole risk. The speedup comes from *not fetching* tiles
    /// a synopsis says cannot match, and a pruning bug does not crash, it
    /// silently returns a shorter answer. So the oracle here is the unfiltered
    /// dump with a plain Rust `retain` over it — the thing the filter is meant
    /// to be a cheaper spelling of — and it is checked on every open path, since
    /// the eager path prunes with a different code path than the lazy one.
    ///
    /// It runs on a `PermSet::CORE` (`--permutations 3`) file as well as a
    /// six-permutation one. Every shape routes inside CORE — `perm_routing_never_leaves_core`
    /// proves the routing table does — so the two must agree row for row, and if
    /// a future shape ever routed outside CORE this test fails on the three-perm
    /// build rather than silently returning nothing from an absent section.
    #[test]
    fn filtered_dump_is_exactly_the_unfiltered_dump_filtered() {
        use crate::index::PermSet;
        use crate::reader::SliceReader;

        let terms = |i: u32| {
            (
                format!("<http://ex/s{}>", i % 11),
                format!("<http://ex/p{}>", i % 3),
                // Mix IRIs and literals in the object column: a literal lives in
                // a different dictionary section than a node, and the object
                // filter has to resolve through the right one.
                if i.is_multiple_of(4) {
                    format!("\"lit {}\"@en", i % 5)
                } else {
                    format!("<http://ex/o{}>", i % 7)
                },
            )
        };
        let build = |perms: PermSet| {
            let mut db = DictionaryBuilder::new();
            let all: Vec<_> = (0..90u32).map(terms).collect();
            for (s, p, o) in &all {
                db.observe(s, p, o);
            }
            let dict = db.build();
            let mut def = GraphIndexBuilder::new()
                .with_tile_budget(16)
                .with_perms(perms);
            let mut g1 = GraphIndexBuilder::new()
                .with_tile_budget(16)
                .with_perms(perms);
            for (i, (s, p, o)) in all.iter().enumerate() {
                let t = dict.encode(s, p, o).unwrap();
                // Deliberately overlapping, not disjoint: every second triple is
                // in both graphs, so a filter that leaked across graphs would
                // still look plausible on counts alone.
                if i % 2 == 0 {
                    g1.push(t);
                }
                def.push(t);
            }
            write_dataset(
                &dict,
                &def.build(),
                &[("<http://ex/g1>".to_string(), g1.build())],
                true,
                &[],
                0,
            )
        };

        let shapes: Vec<(Option<&str>, Option<&str>, Option<&str>)> = vec![
            (None, None, None),
            (Some("<http://ex/s3>"), None, None),
            (None, Some("<http://ex/p1>"), None),
            (None, None, Some("<http://ex/o5>")),
            (None, None, Some("\"lit 0\"@en")),
            (Some("<http://ex/s3>"), Some("<http://ex/p0>"), None),
            (Some("<http://ex/s4>"), None, Some("<http://ex/o4>")),
            (None, Some("<http://ex/p1>"), Some("<http://ex/o5>")),
            (
                Some("<http://ex/s3>"),
                Some("<http://ex/p0>"),
                Some("<http://ex/o3>"),
            ),
            // Terms the dictionary has never seen, in each position: these must
            // return nothing, not everything (an unresolvable bound term must
            // not silently degrade to "unbound").
            (Some("<http://ex/nope>"), None, None),
            (None, Some("<http://ex/nope>"), None),
            (None, None, Some("<http://ex/nope>")),
        ];

        let mut any_pruned = false;
        for perms in [PermSet::ALL, PermSet::CORE] {
            let image = build(perms);
            let leaked: &'static [u8] = Box::leak(image.clone().into_boxed_slice());
            let opens: Vec<(&str, Rete)> = vec![
                ("resident", Rete::open(&image).unwrap()),
                (
                    "ranged",
                    Rete::open_ranged(&SliceReader::new(leaked)).unwrap(),
                ),
                (
                    "lazy",
                    Rete::open_ranged_lazy(SliceReader::new(leaked)).unwrap(),
                ),
            ];
            for graph in [None, Some("<http://ex/g1>"), Some("<http://ex/absent>")] {
                // The oracle: the unfiltered dump, filtered in Rust.
                let full: Vec<TermTriple> = {
                    let mut v = Vec::new();
                    Rete::open(&image).unwrap().dump_each(graph, |s, p, o| {
                        v.push((s.to_string(), p.to_string(), o.to_string()))
                    });
                    v.sort();
                    v
                };
                for &(s, p, o) in &shapes {
                    let mut want = full.clone();
                    want.retain(|(ts, tp, to)| {
                        s.is_none_or(|x| x == ts)
                            && p.is_none_or(|x| x == tp)
                            && o.is_none_or(|x| x == to)
                    });
                    for (path, rete) in &opens {
                        let mut got = Vec::new();
                        rete.dump_filtered_each(graph, s, p, o, |s, p, o| {
                            got.push((s.to_string(), p.to_string(), o.to_string()))
                        });
                        got.sort();
                        assert_eq!(
                            got,
                            want,
                            "{path}/{perms:?} graph={graph:?} pattern={:?} — filtered dump \
                             is not the unfiltered dump filtered",
                            (s, p, o)
                        );
                        assert!(!rete.index_incomplete(), "{path}: a fetch failed");
                    }
                    // …and the plan must agree with what the dump actually did:
                    // an empty answer means either no scan at all, or a scan
                    // whose admitted tiles simply held nothing.
                    let plan = opens[2].1.dump_plan(graph, s, p, o);
                    if graph == Some("<http://ex/absent>")
                        || [s, p, o].contains(&Some("<http://ex/nope>"))
                    {
                        assert!(
                            plan.scan.is_none(),
                            "an unmatchable dump still planned a scan: {:?}",
                            (graph, s, p, o)
                        );
                    } else {
                        let scan = plan.scan.expect("a matchable dump plans a scan");
                        assert!(
                            PermSet::CORE.contains(scan.permutation),
                            "a dump routed to {:?}, outside PermSet::CORE — a \
                             --permutations 3 file has no such section",
                            scan.permutation
                        );
                        assert!(scan.tiles_admitted <= scan.tiles_routed);
                        assert!(scan.tiles_routed <= scan.tiles_total);
                        assert!(scan.tile_bytes <= scan.section_bytes);
                        if scan.tiles_admitted < scan.tiles_total {
                            any_pruned = true;
                        }
                    }
                }
            }
        }
        assert!(
            any_pruned,
            "no shape pruned a single tile — the fixture is too small to be \
             measuring anything"
        );
    }

    /// The point of the filter: a predicate-scoped dump of a lazily range-read
    /// file must **fetch less** than the same graph's full dump, not merely
    /// return less.
    ///
    /// The fixture is the shape the win shows on — many tiles, and a predicate
    /// whose triples live in a minority of them — so the assertion can be about
    /// bytes rather than about the code path being taken on faith. Measured on
    /// the real thing (`cordis.rete`, 801 MB): one predicate of one named graph
    /// read 16.0 MB where the graph's full dump read 375.8 MB.
    #[test]
    fn a_predicate_scoped_dump_fetches_less_than_the_graph() {
        use crate::reader::{CountingReader, SliceReader};

        // 200 subjects × 20 predicates, tiled small so there are many tiles, and
        // one distinct object per cell so that a predicate slice needs a
        // twentieth of the object dictionary rather than all of it — otherwise
        // the dictionary, which this change does not prune, swamps the index
        // saving the test is about.
        let mut db = DictionaryBuilder::new();
        let mut all = Vec::new();
        for s in 0..200u32 {
            for p in 0..20u32 {
                let t = (
                    format!("<http://ex/s/{s:04}>"),
                    format!("<http://ex/p/{p:02}>"),
                    format!("\"object {s:04}/{p:02}\""),
                );
                db.observe(&t.0, &t.1, &t.2);
                all.push(t);
            }
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new().with_tile_budget(64);
        for (s, p, o) in &all {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let image = write_dataset(&dict, &ib.build(), &[], false, &[], 0);
        let leaked: &'static [u8] = Box::leak(image.clone().into_boxed_slice());

        // Bytes faulted past the open, for one dump.
        let probe = |s: Option<&str>, p: Option<&str>, o: Option<&str>| -> (usize, u64) {
            let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
            let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
            let before = reader.bytes_read();
            let mut n = 0usize;
            lazy.dump_filtered_each(None, s, p, o, |_, _, _| n += 1);
            assert!(!lazy.index_incomplete());
            (n, reader.bytes_read() - before)
        };

        let (full_rows, full_bytes) = probe(None, None, None);
        let (pred_rows, pred_bytes) = probe(None, Some("<http://ex/p/07>"), None);
        let (subj_rows, subj_bytes) = probe(Some("<http://ex/s/0021>"), None, None);
        assert_eq!(full_rows, 4000);
        assert_eq!(pred_rows, 200, "one predicate covers every subject once");
        assert_eq!(subj_rows, 20);
        // True in any build: a slice fetches strictly less than the graph.
        assert!(
            pred_bytes < full_bytes && subj_bytes < full_bytes,
            "a slice fetched {pred_bytes} / {subj_bytes} B where the whole graph fetched \
             {full_bytes} B — the scan is not being pruned at all"
        );
        // The SIZE of the win is a property of the FILE, not of this change: it is
        // the index share of what a dump reads, and the dictionary — which no
        // filter prunes — is the rest. With the default `compression` feature the
        // dictionary chunk bodies are zstd-compressed and the slice costs about a
        // tenth of the graph (measured here: 1,565 B for the predicate and 864 B
        // for the subject, against 15,714 B). Built `--no-default-features` those
        // same chunks are stored RAW, so resolving 200 rows' terms dominates and
        // the identical, identically-pruned scan costs 22,225 B of 40,165 — 55%.
        // Asserting the strong ratio unconditionally would be asserting that zstd
        // is enabled; the index-side prune is checked below and holds either way.
        #[cfg(feature = "compression")]
        assert!(
            pred_bytes * 4 < full_bytes && subj_bytes * 4 < full_bytes,
            "a 1-in-20 predicate slice fetched {pred_bytes} B and one subject \
             {subj_bytes} B where the whole graph fetched {full_bytes} B"
        );

        // And the plan predicted the index side of it without fetching a tile.
        let reader = std::sync::Arc::new(CountingReader::new(SliceReader::new(leaked)));
        let lazy = Rete::open_ranged_lazy(reader.clone()).unwrap();
        let before = reader.bytes_read();
        let plan = lazy
            .dump_plan(None, None, Some("<http://ex/p/07>"), None)
            .scan
            .expect("a known predicate plans a scan");
        assert!(
            reader.bytes_read() - before < 64 * 1024,
            "planning fetched {} B — it is supposed to read directories, not tiles",
            reader.bytes_read() - before
        );
        assert!(plan.tiles_admitted < plan.tiles_total);
        // The index side on its own, with the dictionary's share taken out: one
        // predicate of twenty routes to a handful of POS tiles. (Measured here:
        // the whole dump faults 15,714 B, the predicate slice 1,565 B and one
        // subject 864 B — a 10x and an 18x, most of what is left being the
        // dictionary this change does not prune.)
        assert!(
            plan.tile_bytes * 5 < plan.section_bytes,
            "one predicate of twenty planned {} B of a {} B section",
            plan.tile_bytes,
            plan.section_bytes
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

    /// The header segment's LABEL and the span it describes must agree. They
    /// did not from v0.3 (which grew the header to 1 KiB) until this test: the
    /// label still said "fixed 128 bytes", the pre-v0.3 size, while the segment
    /// covered `HEADER_LEN` = 1024 — and that label is exactly what the file
    /// explorer shows someone inspecting a real file. Reading the size back out
    /// of the string is the point: it fails if anyone retypes the number
    /// instead of deriving it from `HEADER_LEN`.
    #[test]
    fn layout_header_label_matches_its_span() {
        let mut db = DictionaryBuilder::new();
        let mut triples = Vec::new();
        for i in 0..24u32 {
            let (s, p, o) = (
                format!("<http://ex/s{}>", i % 5),
                format!("<http://ex/p{}>", i % 3),
                format!("<http://ex/o{i}>"),
            );
            db.observe(&s, &p, &o);
            triples.push((s, p, o));
        }
        let dict = db.build();
        let ids: Vec<(u32, u32, u32)> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let index = GraphIndexBuilder::from_triples(ids).build();
        let image = write_dataset_with_metadata(
            &dict,
            &index,
            &[],
            false,
            &[],
            0,
            br#"{"name":"layout-label-fixture"}"#,
            &[],
        );
        let rete = Rete::open(&image).unwrap();

        let layout = rete.file_layout();
        let header = layout
            .iter()
            .find(|s| s.kind == "header")
            .expect("every layout starts with a header segment");

        assert_eq!(
            header.offset, 0,
            "the header is the first thing in the file"
        );
        assert_eq!(
            header.len, HEADER_LEN as u64,
            "the header segment must span exactly HEADER_LEN"
        );

        // Pull the number back out of the human-readable label and hold it to
        // the span. A hard-coded literal that drifts from HEADER_LEN dies here.
        let digits: String = header
            .label
            .chars()
            .skip_while(|c| !c.is_ascii_digit())
            .take_while(|c| c.is_ascii_digit())
            .collect();
        assert!(
            !digits.is_empty(),
            "header label {:?} states no size at all",
            header.label
        );
        assert_eq!(
            digits.parse::<u64>().unwrap(),
            header.len,
            "header label {:?} disagrees with the {}-byte span it describes",
            header.label,
            header.len
        );

        // The layout must also stay sorted and start at byte zero, which is
        // what makes "the first segment is the header" meaningful.
        assert!(
            layout.windows(2).all(|w| w[0].offset <= w[1].offset),
            "file_layout must return segments sorted by offset"
        );
    }
}
