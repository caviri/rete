//! **External (chunked, memory-bounded) build**: assemble an arbitrarily large
//! single `.rete` file inside a caller-chosen memory budget by spilling sorted
//! intermediate artifacts to disk and merging them, instead of holding the whole
//! dictionary + id-triples + permutations in RAM (SPEC-compatible output;
//! byte-identical to [`crate::ingest::assemble_dataset_streaming_algo`] with the
//! pyramid and text index disabled).
//!
//! ## How the budget bounds memory
//!
//! The input is consumed **once** and cut into K *chunks*, sealed whenever the
//! buffered quads' estimated resident bytes reach a fraction of the budget — so
//! the budget directly decides K (the "number of iterations"). Each chunk is
//! assembled with the ordinary in-RAM machinery (its own small dictionary), then
//! reduced to disk artifacts and dropped:
//!
//! ```text
//! input ─┬─ chunk 0: dict₀ → {shared,subj,obj,pred}₀ term files + triples₀ (local ids)
//!        ├─ chunk 1: …
//!        └─ chunk K: …
//! merge:  k-way term merge over all chunks
//!           → global front-coded dict sections (streamed to disk)
//!           → per-chunk id remap tables (local id → global id)
//! remap:  each chunk's triples remapped to global ids → one triples file
//! index:  per permutation: budget-sized sorted runs → k-way merge (dedup)
//!           → streaming tiler → independently compressed tiles on disk
//! write:  header | metadata | dict | index | footer, hashed incrementally
//! ```
//!
//! Peak RAM ≈ max(one chunk's working set, remap tables + merge buffers, one
//! sort run) — all sized from `memory_budget`, never from the dataset.
//!
//! v1 limits (clear errors, not silent degradation): default graph only, no
//! community pyramid, no full-text index. SPARQL/joins/verify are unaffected
//! (they never require the pyramid).

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::build_pipeline::ingest::{ChunkedIngest, ChunkedStage};
use crate::build_pipeline::spool::{BuildTemp, TripleSpool};
use crate::build_pipeline::timing::{BuildCounters, BuildPhase, BuildTiming};
use crate::build_pipeline::BuildPipelineError;
use crate::dict::env_restart_interval;

use crate::index::{GroupSizer, INDEX_TILE_BUDGET};
use crate::ingest::{BuildStats, IngestError, RawQuad};
use crate::triples::encode_sorted_unique;
use crate::varint::write_uvarint;
use crate::DictionaryBuilder;

/// An id-triple in a single permutation's ordering.
type IdTriple = (u32, u32, u32);
/// A tile synopsis: `(min_b, max_b, min_c, max_c)` over the tile's triples.
type Synopsis = (u32, u32, u32, u32);
/// Builds the metadata payload (the Dataset Card) once the counts are known.
/// Return an empty `Vec` for none — byte-identical to a metadata-free build.
pub type MetadataFn = Box<dyn FnOnce(&BuildStats) -> Vec<u8>>;
/// A distinct term with the `(chunk, local id)` pairs that carry it.
type TermCarriers = (String, Vec<(usize, u32)>);
/// Merge-heap entry: an id-triple tagged with the run it came from.
type MergeEntry = std::cmp::Reverse<(IdTriple, usize)>;
/// One tile's directory entry: `(min_a, max_a, compressed length, synopsis)`.
type TileDirEntry = (u32, u32, u64, Synopsis);
/// One encoded tile: `(min_a, max_a, compressed bytes, synopsis)`.
type EncodedTile = (u32, u32, Vec<u8>, Synopsis);

/// Options for [`build_external`].
pub struct ExternalBuildOptions {
    /// Approximate peak-RAM target in bytes. Controls how many chunks the input
    /// is cut into and how large each external-sort run is. Not a hard cgroup
    /// limit — a working-set target the phases size themselves against.
    pub memory_budget: u64,
    /// Where intermediate spill files live (a `.rete-extbuild-<pid>` directory is
    /// created inside). Defaults to the output file's parent directory — same
    /// filesystem, no surprise `/tmp` exhaustion.
    pub tmp_dir: Option<PathBuf>,
    /// Metadata payload (the Dataset Card), derived after counts are known.
    pub metadata: MetadataFn,
}

impl Default for ExternalBuildOptions {
    fn default() -> Self {
        Self {
            memory_budget: 4 << 30,
            tmp_dir: None,
            metadata: Box::new(|_| Vec::new()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtBuildError {
    #[error("ingest: {0}")]
    Ingest(#[from] IngestError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "external build supports the default graph only (named graph {0} found); \
             use the standard build, or strip graph terms (.nq -> .nt) first"
    )]
    NamedGraph(String),
    #[error("internal: {0}")]
    Internal(&'static str),
}

impl ExtBuildError {
    pub(crate) fn into_pipeline(self) -> BuildPipelineError {
        match self {
            Self::Ingest(error) => BuildPipelineError::Ingest(error),
            Self::Io(error) => BuildPipelineError::Io(error),
            Self::NamedGraph(graph) => BuildPipelineError::NamedGraph(graph),
            Self::Internal(message) => BuildPipelineError::InvalidSpool(message),
        }
    }

    fn from_pipeline(error: BuildPipelineError) -> Self {
        match error {
            BuildPipelineError::Ingest(error) => Self::Ingest(error),
            BuildPipelineError::Io(error) => Self::Io(error),
            BuildPipelineError::NamedGraph(graph) => Self::NamedGraph(graph),
            BuildPipelineError::InvalidSpool(message) | BuildPipelineError::Overflow(message) => {
                Self::Internal(message)
            }
            BuildPipelineError::TooManyTerms => Self::Internal("term id space exceeds u32"),
            BuildPipelineError::File(_) => Self::Internal("shared chunk ingest file failure"),
            #[cfg(test)]
            BuildPipelineError::InjectedFailure(message) => Self::Internal(message),
        }
    }
}

impl From<BuildPipelineError> for ExtBuildError {
    fn from(error: BuildPipelineError) -> Self {
        Self::from_pipeline(error)
    }
}

/// Fraction of the budget a chunk's buffered raw quads may occupy before the
/// chunk is sealed. The other half covers the chunk's transient dictionary +
/// id-encode working set (~the same order as the buffered strings).
pub(crate) const CHUNK_BUDGET_FRACTION: f64 = 0.5;
/// Overhead charged per buffered quad on top of its term byte lengths (String
/// headers, Vec slot, allocator slack) when estimating chunk residency.
const PER_QUAD_OVERHEAD: u64 = 96;
/// Per-tile-batch size for parallel tile compression.
const TILE_COMPRESS_BATCH: usize = 512;

/// Build a `.rete` at `output` from a **single-pass** quad stream within
/// `opts.memory_budget`. Returns the same [`BuildStats`] as the in-RAM paths.
///
/// `stream` is invoked once; it must call the visitor for every quad.
pub fn build_external<S>(
    mut stream: S,
    output: &Path,
    opts: ExternalBuildOptions,
) -> Result<BuildStats, ExtBuildError>
where
    S: FnMut(&mut dyn FnMut(RawQuad) -> Result<(), ExtBuildError>) -> Result<(), ExtBuildError>,
{
    let mut timing = BuildTiming::new();
    let tmp_parent = opts
        .tmp_dir
        .clone()
        .or_else(|| output.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let tmp = BuildTemp::new(&tmp_parent).map_err(|error| match error {
        BuildPipelineError::Io(error) => ExtBuildError::Io(error),
        _ => ExtBuildError::Internal("temporary directory creation failed"),
    })?;
    let budget = opts.memory_budget.max(64 << 20); // floor: 64 MiB

    // ---- Phase 1: chunk the input ------------------------------------------
    let chunk_budget = (budget as f64 * CHUNK_BUDGET_FRACTION) as u64;
    eprintln!(
        "extbuild: budget {} MiB -> chunk target {} MiB",
        budget >> 20,
        chunk_budget >> 20
    );
    let mut chunked = ChunkedIngest::new(&tmp, budget);
    stream(&mut |q: RawQuad| chunked.push(q).map_err(ExtBuildError::from_pipeline))?;
    let canonical = chunked
        .finish_with_stages(opts.metadata, |stage| {
            timing.lap(match stage {
                ChunkedStage::ParseIngest => BuildPhase::ParseIngest,
                ChunkedStage::Canonicalize => BuildPhase::Canonicalize,
                ChunkedStage::Remap => BuildPhase::Remap,
            });
        })
        .map_err(ExtBuildError::from_pipeline)?;
    let statements = u64::try_from(canonical.stats.statements)
        .map_err(|_| ExtBuildError::Internal("statement count overflow"))?;
    eprintln!(
        "extbuild: canonical spool contains {} statement(s)",
        statements
    );

    // ---- Phase 2/3: shared chunk ingest merges dictionaries and remaps the
    // per-chunk records into the canonical file-backed spool.
    eprintln!(
        "extbuild: merged dictionary — {} term(s)",
        canonical.dictionary.term_count
    );
    let global_tri = canonical
        .triples
        .file_path()
        .ok_or(ExtBuildError::Internal(
            "external ingest did not spill triples",
        ))?;

    // ---- Phase 4: per-permutation external sort + streaming tiler -----------
    let codec = crate::file::writer_codec();
    // A run holds R triples resident twice over during sort (Vec + sort scratch).
    let run_len = ((budget / 2) / 24).clamp(1 << 16, u32::MAX as u64) as usize;
    let mut perm_sections: Vec<SectionFile> = Vec::with_capacity(6);
    let mut deduped_count: Option<u64> = None;
    for perm in crate::index::ALL_PERMS {
        let (section, n) = build_permutation_section(&tmp, global_tri, perm, run_len, codec)?;
        // every permutation dedups the same multiset — counts must agree
        if let Some(prev) = deduped_count {
            if prev != n {
                return Err(ExtBuildError::Internal("permutation dedup counts diverge"));
            }
        }
        deduped_count = Some(n);
        eprintln!(
            "extbuild: permutation {} indexed ({} unique triple(s))",
            perm.name(),
            n
        );
        perm_sections.push(section);
    }
    timing.lap(BuildPhase::SubjectFamily);
    let _ = std::fs::remove_file(global_tri);
    let quad_count = deduped_count.unwrap_or(0);

    // ---- Phase 5: stream the final file -------------------------------------
    let mut stats = canonical.stats;
    write_final_file(
        output,
        &canonical.metadata,
        &SpilledDictionaryRef {
            section_paths: canonical.dictionary.section_paths.clone(),
            term_count: canonical.dictionary.term_count,
            has_quoted: canonical.dictionary.has_quoted_triples,
        },
        &perm_sections,
        quad_count,
        codec,
    )?;
    timing.lap(BuildPhase::FinalWrite);
    let spill_bytes = canonical
        .dictionary
        .section_paths
        .iter()
        .map(|section| std::fs::metadata(section).map(|entry| entry.len()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .chain(perm_sections.iter().map(|section| section.len))
        .sum();
    timing.set_counters(BuildCounters {
        statements,
        input_bytes: None,
        spill_bytes,
        output_bytes: std::fs::metadata(output)?.len(),
        family_runs: [2, 2, 2],
    });
    timing.finish();
    stats.pyramid_levels = 0;
    Ok(stats)
}

// ---------------------------------------------------------------------------
// tmp-dir guard
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Phase 1: chunking
// ---------------------------------------------------------------------------

pub(crate) struct ChunkInfo {
    pub(crate) triple_count: u64,
    /// term counts per section file, in file order (shared, subj, obj, pred)
    section_terms: [u32; 4],
}

pub(crate) struct Chunker<'a> {
    tmp: &'a BuildTemp,
    chunk_budget: u64,
    acc_bytes: u64,
    quads: Vec<(String, String, String)>,
    chunks: Vec<ChunkInfo>,
    has_quoted: bool,
}

impl<'a> Chunker<'a> {
    pub(crate) fn new(tmp: &'a BuildTemp, chunk_budget: u64) -> Self {
        Chunker {
            tmp,
            chunk_budget,
            acc_bytes: 0,
            quads: Vec::new(),
            chunks: Vec::new(),
            has_quoted: false,
        }
    }

    pub(crate) fn push(&mut self, q: RawQuad) -> Result<(), ExtBuildError> {
        let (s, p, o, g) = q;
        if let Some(graph) = g {
            return Err(ExtBuildError::NamedGraph(graph));
        }
        self.acc_bytes += (s.len() + p.len() + o.len()) as u64 + PER_QUAD_OVERHEAD;
        self.quads.push((s, p, o));
        if self.acc_bytes >= self.chunk_budget {
            self.seal()?;
        }
        Ok(())
    }

    /// Build this chunk's private dictionary, spill its four sorted section term
    /// files + local-id triples, then drop everything.
    fn seal(&mut self) -> Result<(), ExtBuildError> {
        if self.quads.is_empty() {
            return Ok(());
        }
        let ci = self.chunks.len();
        let quads = std::mem::take(&mut self.quads);
        self.acc_bytes = 0;

        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &quads {
            db.observe(s, p, o);
        }
        let dict = db.build();
        if dict.has_quoted_triples() {
            self.has_quoted = true;
        }

        // local id-triples (subject-space id, predicate id, object-space id)
        let mut tri = BufWriter::new(File::create(self.tmp.path(&format!("c{ci}.tri"))?)?);
        for (s, p, o) in &quads {
            let (si, pi, oi) = dict
                .encode(s, p, o)
                .ok_or(ExtBuildError::Internal("chunk term missing from own dict"))?;
            tri.write_all(&si.to_le_bytes())?;
            tri.write_all(&pi.to_le_bytes())?;
            tri.write_all(&oi.to_le_bytes())?;
        }
        tri.flush()?;
        let triple_count = quads.len() as u64;
        drop(quads);

        // four sorted section term files (terms are NT tokens — never a raw \n)
        let shared = dict.shared_count();
        let subj_only = dict.subject_only_count();
        let obj_only = dict.object_only_count();
        let preds = {
            let mut n = 1u32;
            while dict.predicate_term(n).is_some() {
                n += 1;
            }
            n - 1
        };
        let write_terms = |name: &str,
                           count: u32,
                           term_of: &dyn Fn(u32) -> Option<String>|
         -> Result<(), ExtBuildError> {
            let mut w = BufWriter::new(File::create(self.tmp.path(name)?)?);
            for i in 1..=count {
                let t = term_of(i).ok_or(ExtBuildError::Internal("dict id out of range"))?;
                w.write_all(t.as_bytes())?;
                w.write_all(b"\n")?;
            }
            w.flush()?;
            Ok(())
        };
        write_terms(&format!("c{ci}.shared"), shared, &|i| dict.subject_term(i))?;
        write_terms(&format!("c{ci}.subj"), subj_only, &|i| {
            dict.subject_term(shared + i)
        })?;
        write_terms(&format!("c{ci}.obj"), obj_only, &|i| {
            dict.object_term(shared + i)
        })?;
        write_terms(&format!("c{ci}.pred"), preds, &|i| dict.predicate_term(i))?;

        eprintln!(
            "extbuild: chunk {ci} sealed — {triple_count} statement(s), {} term(s)",
            shared + subj_only + obj_only + preds
        );
        self.chunks.push(ChunkInfo {
            triple_count,
            section_terms: [shared, subj_only, obj_only, preds],
        });
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<Vec<ChunkInfo>, ExtBuildError> {
        self.seal()?;
        if self.chunks.is_empty() {
            // an empty input still produces one (empty) chunk so downstream
            // phases have a well-formed shape to merge
            self.chunks.push(ChunkInfo {
                triple_count: 0,
                section_terms: [0, 0, 0, 0],
            });
            for name in ["c0.tri", "c0.shared", "c0.subj", "c0.obj", "c0.pred"] {
                File::create(self.tmp.path(name)?)?;
            }
        }
        Ok(self.chunks)
    }
}

// ---------------------------------------------------------------------------
// Phase 2: global dictionary merge
// ---------------------------------------------------------------------------

/// Per-chunk id remap tables: local id (1-based, per space) → global id.
struct ChunkRemap {
    subj: Vec<u32>,
    obj: Vec<u32>,
    pred: Vec<u32>,
}

pub(crate) struct MergedDict {
    /// The four chunked-encoded dict section payloads, spilled to tmp files
    /// (shared, subjects, objects, predicates — the container order).
    section_files: [SectionFile; 4],
    pub(crate) term_count: u64,
    pub(crate) has_quoted: bool,
    remaps: Vec<ChunkRemap>,
}

impl MergedDict {
    pub(crate) fn section_paths(&self) -> [PathBuf; 4] {
        self.section_files
            .each_ref()
            .map(|section| section.path.clone())
    }

    #[cfg(test)]
    fn dictionary_ref(&self) -> SpilledDictionaryRef {
        SpilledDictionaryRef {
            section_paths: self.section_paths(),
            term_count: self.term_count,
            has_quoted: self.has_quoted,
        }
    }
}

struct SpilledDictionaryRef {
    section_paths: [PathBuf; 4],
    term_count: u64,
    has_quoted: bool,
}

/// A finished section payload living in a tmp file.
struct SectionFile {
    path: PathBuf,
    len: u64,
}

/// One chunk's contribution to a term-space merge: an iterator over
/// `(term, local_space_id)` in ascending term order. Subject space enumerates
/// shared then subject-only ids interleaved back into term order.
struct SpaceStream {
    a: TermFileReader, // shared section
    b: TermFileReader, // role-only section
    a_base: u32,       // shared ids are 1..=shared
    b_base: u32,       // role-only ids are shared+1..
    a_next: Option<String>,
    b_next: Option<String>,
    a_rank: u32,
    b_rank: u32,
}

impl SpaceStream {
    fn new(a: TermFileReader, b: TermFileReader, shared: u32) -> Result<Self, std::io::Error> {
        let mut s = SpaceStream {
            a,
            b,
            a_base: 0,
            b_base: shared,
            a_next: None,
            b_next: None,
            a_rank: 0,
            b_rank: 0,
        };
        s.a_next = s.a.next()?;
        s.b_next = s.b.next()?;
        Ok(s)
    }

    /// Next `(term, local_space_id)` in ascending term order.
    fn next(&mut self) -> Result<Option<(String, u32)>, std::io::Error> {
        let take_a = match (&self.a_next, &self.b_next) {
            (None, None) => return Ok(None),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            // sections are disjoint, so strict ordering decides
            (Some(x), Some(y)) => x < y,
        };
        if take_a {
            let t = self.a_next.take().unwrap();
            self.a_rank += 1;
            let id = self.a_base + self.a_rank;
            self.a_next = self.a.next()?;
            Ok(Some((t, id)))
        } else {
            let t = self.b_next.take().unwrap();
            self.b_rank += 1;
            let id = self.b_base + self.b_rank;
            self.b_next = self.b.next()?;
            Ok(Some((t, id)))
        }
    }
}

/// Line-based reader over a chunk's sorted term file.
struct TermFileReader {
    rd: BufReader<File>,
    buf: String,
}

impl TermFileReader {
    fn open(path: &Path) -> Result<Self, std::io::Error> {
        Ok(TermFileReader {
            rd: BufReader::with_capacity(1 << 20, File::open(path)?),
            buf: String::new(),
        })
    }
    fn next(&mut self) -> Result<Option<String>, std::io::Error> {
        self.buf.clear();
        let n = self.rd.read_line(&mut self.buf)?;
        if n == 0 {
            return Ok(None);
        }
        // strip exactly the trailing '\n' we wrote
        if self.buf.ends_with('\n') {
            self.buf.pop();
        }
        Ok(Some(self.buf.clone()))
    }
}

/// Heap entry for the k-way term merge (min-heap by term, then chunk index for
/// determinism).
struct HeapEntry {
    term: String,
    chunk: usize,
    local_id: u32,
}
impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.term == other.term && self.chunk == other.chunk
    }
}
impl Eq for HeapEntry {}
impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // BinaryHeap is a max-heap; invert for ascending term order
        other
            .term
            .cmp(&self.term)
            .then_with(|| other.chunk.cmp(&self.chunk))
    }
}

/// K-way merged view over one term space (subject or object) of every chunk:
/// yields each distinct term once, ascending, with every `(chunk, local_id)`
/// that carries it.
struct KWayTerms {
    heap: BinaryHeap<HeapEntry>,
    streams: Vec<SpaceStream>,
}

impl KWayTerms {
    fn new(mut streams: Vec<SpaceStream>) -> Result<Self, std::io::Error> {
        let mut heap = BinaryHeap::new();
        for (ci, s) in streams.iter_mut().enumerate() {
            if let Some((term, id)) = s.next()? {
                heap.push(HeapEntry {
                    term,
                    chunk: ci,
                    local_id: id,
                });
            }
        }
        Ok(KWayTerms { heap, streams })
    }

    /// Next distinct term with its (chunk, local_id) carriers.
    fn next(&mut self) -> Result<Option<TermCarriers>, std::io::Error> {
        let first = match self.heap.pop() {
            Some(e) => e,
            None => return Ok(None),
        };
        let term = first.term;
        let mut carriers = vec![(first.chunk, first.local_id)];
        if let Some((t, id)) = self.streams[first.chunk].next()? {
            self.heap.push(HeapEntry {
                term: t,
                chunk: first.chunk,
                local_id: id,
            });
        }
        while let Some(top) = self.heap.peek() {
            if top.term != term {
                break;
            }
            let e = self.heap.pop().unwrap();
            carriers.push((e.chunk, e.local_id));
            if let Some((t, id)) = self.streams[e.chunk].next()? {
                self.heap.push(HeapEntry {
                    term: t,
                    chunk: e.chunk,
                    local_id: id,
                });
            }
        }
        Ok(Some((term, carriers)))
    }
}

/// Provisional remap encoding while global section sizes are still unknown:
/// class in the top 2 bits, class-local rank (0-based) below. Fixed up to real
/// ids once the shared count is known.
const CLASS_SHARED: u32 = 0b00 << 30;
const CLASS_SUBJ_ONLY: u32 = 0b01 << 30;
const CLASS_OBJ_ONLY: u32 = 0b10 << 30;
const CLASS_MASK: u32 = 0b11 << 30;

pub(crate) fn merge_dictionaries(
    tmp: &BuildTemp,
    chunks: &[ChunkInfo],
) -> Result<MergedDict, ExtBuildError> {
    let k = chunks.len();

    // remap tables, sized by each chunk's spaces
    let mut remaps: Vec<ChunkRemap> = chunks
        .iter()
        .map(|c| ChunkRemap {
            subj: vec![0; (c.section_terms[0] + c.section_terms[1]) as usize],
            obj: vec![0; (c.section_terms[0] + c.section_terms[2]) as usize],
            pred: vec![0; c.section_terms[3] as usize],
        })
        .collect();

    // subject-space + object-space k-way streams
    let mut subj_streams = Vec::with_capacity(k);
    let mut obj_streams = Vec::with_capacity(k);
    for (ci, c) in chunks.iter().enumerate() {
        subj_streams.push(SpaceStream::new(
            TermFileReader::open(&tmp.path(&format!("c{ci}.shared"))?)?,
            TermFileReader::open(&tmp.path(&format!("c{ci}.subj"))?)?,
            c.section_terms[0],
        )?);
        obj_streams.push(SpaceStream::new(
            TermFileReader::open(&tmp.path(&format!("c{ci}.shared"))?)?,
            TermFileReader::open(&tmp.path(&format!("c{ci}.obj"))?)?,
            c.section_terms[0],
        )?);
    }
    let mut subjects = KWayTerms::new(subj_streams)?;
    let mut objects = KWayTerms::new(obj_streams)?;

    // The three node sections are written as raw front-coded bodies first; the
    // final chunked encoding needs term counts, so bodies spill to tmp.
    let mut shared_sec = RawSectionWriter::create(tmp.path("g.shared.raw")?)?;
    let mut subj_sec = RawSectionWriter::create(tmp.path("g.subj.raw")?)?;
    let mut obj_sec = RawSectionWriter::create(tmp.path("g.obj.raw")?)?;
    let mut has_quoted = false;

    let mut ranks = [0u32; 3]; // shared, subj-only, obj-only
    let mut s_item = subjects.next()?;
    let mut o_item = objects.next()?;
    loop {
        enum Class {
            Shared,
            SubjOnly,
            ObjOnly,
        }
        let class = match (&s_item, &o_item) {
            (None, None) => break,
            (Some(_), None) => Class::SubjOnly,
            (None, Some(_)) => Class::ObjOnly,
            (Some((st, _)), Some((ot, _))) => match st.cmp(ot) {
                std::cmp::Ordering::Less => Class::SubjOnly,
                std::cmp::Ordering::Greater => Class::ObjOnly,
                std::cmp::Ordering::Equal => Class::Shared,
            },
        };
        match class {
            Class::Shared => {
                let (term, s_carriers) = s_item.take().unwrap();
                let (_, o_carriers) = o_item.take().unwrap();
                let enc = CLASS_SHARED | ranks[0];
                for (ci, lid) in s_carriers {
                    remaps[ci].subj[(lid - 1) as usize] = enc;
                }
                for (ci, lid) in o_carriers {
                    remaps[ci].obj[(lid - 1) as usize] = enc;
                }
                has_quoted |= term.starts_with("<<");
                shared_sec.push(&term)?;
                ranks[0] += 1;
                s_item = subjects.next()?;
                o_item = objects.next()?;
            }
            Class::SubjOnly => {
                let (term, carriers) = s_item.take().unwrap();
                let enc = CLASS_SUBJ_ONLY | ranks[1];
                for (ci, lid) in carriers {
                    remaps[ci].subj[(lid - 1) as usize] = enc;
                }
                has_quoted |= term.starts_with("<<");
                subj_sec.push(&term)?;
                ranks[1] += 1;
                s_item = subjects.next()?;
            }
            Class::ObjOnly => {
                let (term, carriers) = o_item.take().unwrap();
                let enc = CLASS_OBJ_ONLY | ranks[2];
                for (ci, lid) in carriers {
                    remaps[ci].obj[(lid - 1) as usize] = enc;
                }
                has_quoted |= term.starts_with("<<");
                obj_sec.push(&term)?;
                ranks[2] += 1;
                o_item = objects.next()?;
            }
        }
    }
    let (n_shared, n_subj, n_obj) = (ranks[0], ranks[1], ranks[2]);

    // predicates: plain k-way merge over the pred term files
    let mut pred_sec = RawSectionWriter::create(tmp.path("g.pred.raw")?)?;
    {
        let mut streams = Vec::with_capacity(k);
        for (ci, _) in chunks.iter().enumerate() {
            // empty shared reader + real pred reader => a plain single stream
            streams.push(SpaceStream::new(
                TermFileReader::open(&tmp.path(&format!("c{ci}.pred"))?)?,
                TermFileReader::open(&tmp.path(&format!("c{ci}.pred.empty",))?).or_else(
                    |_| -> Result<TermFileReader, std::io::Error> {
                        // create-once empty file per chunk
                        let p = tmp.path(&format!("c{ci}.pred.empty")).map_err(|error| {
                            std::io::Error::new(std::io::ErrorKind::InvalidInput, error)
                        })?;
                        File::create(&p)?;
                        TermFileReader::open(&p)
                    },
                )?,
                chunks[ci].section_terms[3],
            )?);
        }
        let mut kway = KWayTerms::new(streams)?;
        let mut rank = 0u32;
        while let Some((term, carriers)) = kway.next()? {
            for (ci, lid) in carriers {
                remaps[ci].pred[(lid - 1) as usize] = rank + 1; // pred ids are final now
            }
            pred_sec.push(&term)?;
            rank += 1;
        }
    }

    // fix up provisional node encodings to final space ids:
    //   shared:        id = rank + 1                      (both spaces)
    //   subject-only:  id = n_shared + rank + 1           (subject space)
    //   object-only:   id = n_shared + rank + 1           (object space)
    for rm in &mut remaps {
        for v in rm.subj.iter_mut() {
            let rank = *v & !CLASS_MASK;
            *v = match *v & CLASS_MASK {
                CLASS_SHARED => rank + 1,
                CLASS_SUBJ_ONLY => n_shared + rank + 1,
                _ => return Err(ExtBuildError::Internal("subject remap class corrupt")),
            };
        }
        for v in rm.obj.iter_mut() {
            let rank = *v & !CLASS_MASK;
            *v = match *v & CLASS_MASK {
                CLASS_SHARED => rank + 1,
                CLASS_OBJ_ONLY => n_shared + rank + 1,
                _ => return Err(ExtBuildError::Internal("object remap class corrupt")),
            };
        }
    }

    // chunk-encode each raw section (identical to encode_chunked_dict_section)
    let codec = crate::file::writer_codec();
    let section_files = [
        shared_sec.finish_chunked(tmp, "g.shared.sec", codec)?,
        subj_sec.finish_chunked(tmp, "g.subj.sec", codec)?,
        obj_sec.finish_chunked(tmp, "g.obj.sec", codec)?,
        pred_sec.finish_chunked(tmp, "g.pred.sec", codec)?,
    ];
    // chunk term files are no longer needed
    for ci in 0..k {
        for suffix in ["shared", "subj", "obj", "pred", "pred.empty"] {
            let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.{suffix}"))?);
        }
    }

    let term_count =
        n_shared as u64 + n_subj as u64 + n_obj as u64 + section_files[3].term_count as u64;

    Ok(MergedDict {
        section_files: section_files.map(|s| s.file),
        term_count,
        has_quoted,
        remaps,
    })
}

/// Remap each chunk's local triple file into one canonical 12-byte spool. This
/// is deliberately sequential: the per-chunk remaps are bounded by the current
/// chunk and are released before the external sort phase begins.
pub(crate) fn remap_chunks_to_spool(
    temp: &BuildTemp,
    chunks: &[ChunkInfo],
    merged: &mut MergedDict,
) -> Result<TripleSpool, ExtBuildError> {
    let count = chunks.iter().try_fold(0u64, |total, chunk| {
        total
            .checked_add(chunk.triple_count)
            .ok_or(ExtBuildError::Internal("statement count overflow"))
    })?;
    let path = temp.path("global.tri")?;
    let mut output = BufWriter::new(File::create(&path)?);
    let remaps = std::mem::take(&mut merged.remaps);
    for (chunk_index, _) in chunks.iter().enumerate() {
        let maps = remaps
            .get(chunk_index)
            .ok_or(ExtBuildError::Internal("chunk remap missing"))?;
        let chunk_path = temp.path(&format!("c{chunk_index}.tri"))?;
        let mut input = BufReader::new(File::open(&chunk_path)?);
        loop {
            let mut record = [0u8; 12];
            match input.read_exact(&mut record) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    let length = std::fs::metadata(&chunk_path)?.len();
                    if length % 12 != 0 {
                        return Err(ExtBuildError::Internal("partial chunk triple record"));
                    }
                    break;
                }
                Err(error) => return Err(error.into()),
            }
            let subject = u32::from_le_bytes(
                record[0..4]
                    .try_into()
                    .map_err(|_| ExtBuildError::Internal("subject record width"))?,
            );
            let predicate = u32::from_le_bytes(
                record[4..8]
                    .try_into()
                    .map_err(|_| ExtBuildError::Internal("predicate record width"))?,
            );
            let object = u32::from_le_bytes(
                record[8..12]
                    .try_into()
                    .map_err(|_| ExtBuildError::Internal("object record width"))?,
            );
            let remap = |ids: &[u32], id: u32, space: &'static str| {
                id.checked_sub(1)
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| ids.get(index).copied())
                    .filter(|mapped| *mapped != 0)
                    .ok_or(ExtBuildError::Internal(space))
            };
            let subject = remap(&maps.subj, subject, "subject remap missing")?;
            let predicate = remap(&maps.pred, predicate, "predicate remap missing")?;
            let object = remap(&maps.obj, object, "object remap missing")?;
            output.write_all(&subject.to_le_bytes())?;
            output.write_all(&predicate.to_le_bytes())?;
            output.write_all(&object.to_le_bytes())?;
        }
        let _ = std::fs::remove_file(chunk_path);
    }
    output.flush()?;
    drop(output);
    TripleSpool::from_file(temp, path, count).map_err(|error| match error {
        BuildPipelineError::Io(error) => ExtBuildError::Io(error),
        BuildPipelineError::InvalidSpool(message) => ExtBuildError::Internal(message),
        BuildPipelineError::Overflow(message) => ExtBuildError::Internal(message),
        BuildPipelineError::Ingest(error) => ExtBuildError::Ingest(error),
        BuildPipelineError::NamedGraph(graph) => ExtBuildError::NamedGraph(graph),
        BuildPipelineError::File(_) | BuildPipelineError::TooManyTerms => {
            ExtBuildError::Internal("triple spool construction failed")
        }
        #[cfg(test)]
        BuildPipelineError::InjectedFailure(message) => ExtBuildError::Internal(message),
    })
}

/// A raw front-coded dict-section body being streamed to a tmp file, exactly as
/// [`crate::dict::DictSectionBuilder::build`] would lay it out — plus the
/// restart-offset bookkeeping needed to emit the header and (later) the chunked
/// encoding without ever holding the body in RAM.
struct RawSectionWriter {
    w: BufWriter<File>,
    path: PathBuf,
    prev: String,
    n: u64,
    body_len: u64,
    restart_offsets: Vec<u64>,
}

struct FinishedSection {
    file: SectionFile,
    term_count: u32,
}

impl RawSectionWriter {
    fn create(path: PathBuf) -> Result<Self, std::io::Error> {
        Ok(RawSectionWriter {
            w: BufWriter::with_capacity(1 << 20, File::create(&path)?),
            path,
            prev: String::new(),
            n: 0,
            body_len: 0,
            restart_offsets: Vec::new(),
        })
    }

    fn push(&mut self, term: &str) -> Result<(), std::io::Error> {
        let r = env_restart_interval() as u64;
        let mut entry = Vec::with_capacity(term.len() + 8);
        if self.n.is_multiple_of(r) {
            self.restart_offsets.push(self.body_len);
            write_uvarint(&mut entry, 0);
            write_uvarint(&mut entry, term.len() as u64);
            entry.extend_from_slice(term.as_bytes());
        } else {
            let shared = common_prefix_len(&self.prev, term);
            let suffix = &term.as_bytes()[shared..];
            write_uvarint(&mut entry, shared as u64);
            write_uvarint(&mut entry, suffix.len() as u64);
            entry.extend_from_slice(suffix);
        }
        self.w.write_all(&entry)?;
        self.body_len += entry.len() as u64;
        self.prev.clear();
        self.prev.push_str(term);
        self.n += 1;
        Ok(())
    }

    /// Emit the chunked section payload (byte-identical to running
    /// `encode_chunked_dict_section` over the equivalent raw section) into a new
    /// tmp file, deleting the raw body.
    fn finish_chunked(
        mut self,
        tmp: &BuildTemp,
        out_name: &str,
        codec: u8,
    ) -> Result<FinishedSection, ExtBuildError> {
        self.w.flush()?;
        drop(self.w);
        let term_count = self.n as u32;

        // the raw header exactly as DictSectionBuilder writes it
        let mut header = Vec::new();
        write_uvarint(&mut header, self.n);
        write_uvarint(&mut header, env_restart_interval() as u64);
        write_uvarint(&mut header, self.restart_offsets.len() as u64);
        for off in &self.restart_offsets {
            write_uvarint(&mut header, *off);
        }

        // chunk bounds over the body by run offsets (whole runs, 64 KiB budget)
        let budget = 64 * 1024u64; // DICT_CHUNK_BUDGET
        let offs = &self.restart_offsets;
        let mut bounds: Vec<(usize, u64, u64)> = Vec::new(); // (first_run, start, end)
        let mut r = 0usize;
        while r < offs.len() {
            let start = offs[r];
            let mut r2 = r + 1;
            while r2 < offs.len() && offs[r2] - start < budget {
                r2 += 1;
            }
            let end = if r2 < offs.len() {
                offs[r2]
            } else {
                self.body_len
            };
            bounds.push((r, start, end));
            r = r2;
        }

        let mut body = File::open(&self.path)?;
        // compress chunks (reading each body slice from disk), spill compressed
        // bytes to a scratch file, record dir entries
        let comp_path = tmp.path(&format!("{out_name}.chunks"))?;
        let mut comp_out = BufWriter::new(File::create(&comp_path)?);
        let mut dir: Vec<(usize, Vec<u8>, u64)> = Vec::with_capacity(bounds.len());
        for &(first_run, start, end) in &bounds {
            body.seek(SeekFrom::Start(start))?;
            let mut raw = vec![0u8; (end - start) as usize];
            body.read_exact(&mut raw)?;
            // the chunk's first run entry is a restart: [0][len][full term]
            let first_term = read_restart_term(&raw)
                .ok_or(ExtBuildError::Internal("restart entry unreadable"))?;
            let comp = crate::file::compress(codec, &raw);
            comp_out.write_all(&comp)?;
            dir.push((first_run, first_term, comp.len() as u64));
        }
        comp_out.flush()?;
        drop(body);
        let _ = std::fs::remove_file(&self.path);

        // assemble: [header_len][header][num_chunks][dir…][compressed chunks…]
        let out_path = tmp.path(out_name)?;
        let mut out = BufWriter::new(File::create(&out_path)?);
        let mut head = Vec::new();
        write_uvarint(&mut head, header.len() as u64);
        head.extend_from_slice(&header);
        write_uvarint(&mut head, dir.len() as u64);
        let mut prev_run = 0usize;
        for (first_run, first_term, comp_len) in &dir {
            write_uvarint(&mut head, (*first_run - prev_run) as u64);
            write_uvarint(&mut head, first_term.len() as u64);
            head.extend_from_slice(first_term);
            write_uvarint(&mut head, *comp_len);
            prev_run = *first_run;
        }
        out.write_all(&head)?;
        let mut comp_in = File::open(&comp_path)?;
        let copied = std::io::copy(&mut comp_in, &mut out)?;
        out.flush()?;
        let _ = std::fs::remove_file(&comp_path);
        let len = head.len() as u64 + copied;
        Ok(FinishedSection {
            file: SectionFile {
                path: out_path,
                len,
            },
            term_count,
        })
    }
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.as_bytes()
        .iter()
        .zip(b.as_bytes())
        .take_while(|(x, y)| x == y)
        .count()
}

/// Read the full term of the restart entry at the start of `raw`.
fn read_restart_term(raw: &[u8]) -> Option<Vec<u8>> {
    let (shared, n1) = crate::varint::read_uvarint(raw)?;
    if shared != 0 {
        return None;
    }
    let (len, n2) = crate::varint::read_uvarint(&raw[n1..])?;
    let start = n1 + n2;
    raw.get(start..start + len as usize).map(|s| s.to_vec())
}

// ---------------------------------------------------------------------------
// Phase 4: external permutation sort + streaming tiler
// ---------------------------------------------------------------------------

/// Sort `global.tri` under `perm`, dedup, tile, and spill one tiled section
/// payload (byte-identical to `encode_tiled_section` over the same triples).
/// Returns the section file + the deduped triple count.
fn build_permutation_section(
    tmp: &BuildTemp,
    global_tri: &Path,
    perm: crate::index::IndexPermutation,
    run_len: usize,
    codec: u8,
) -> Result<(SectionFile, u64), ExtBuildError> {
    // 1. sorted runs
    let mut runs: Vec<PathBuf> = Vec::new();
    {
        let mut rd = BufReader::with_capacity(1 << 20, File::open(global_tri)?);
        let mut buf = [0u8; 12];
        let mut run: Vec<(u32, u32, u32)> = Vec::with_capacity(run_len.min(1 << 22));
        loop {
            let eof = match rd.read_exact(&mut buf) {
                Ok(()) => false,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => true,
                Err(e) => return Err(e.into()),
            };
            if !eof {
                let s = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let p = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                let o = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                run.push(perm.forward((s, p, o)));
            }
            if run.len() >= run_len || (eof && !run.is_empty()) {
                sort_triples(&mut run);
                run.dedup();
                let path = tmp.path(&format!("{}.run{}", perm.name(), runs.len()))?;
                let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
                for &(a, b, c) in &run {
                    w.write_all(&a.to_le_bytes())?;
                    w.write_all(&b.to_le_bytes())?;
                    w.write_all(&c.to_le_bytes())?;
                }
                w.flush()?;
                runs.push(path);
                run.clear();
            }
            if eof {
                break;
            }
        }
    }

    // 2. k-way merge (dedup) feeding the streaming tiler
    let mut tiler = StreamingTiler::new(tmp, perm.name(), codec)?;
    {
        let mut readers: Vec<RunReader> = runs
            .iter()
            .map(|p| RunReader::open(p))
            .collect::<Result<_, _>>()?;
        let mut heap: BinaryHeap<MergeEntry> = BinaryHeap::new();
        for (i, r) in readers.iter_mut().enumerate() {
            if let Some(t) = r.next()? {
                heap.push(std::cmp::Reverse((t, i)));
            }
        }
        let mut last: Option<(u32, u32, u32)> = None;
        while let Some(std::cmp::Reverse((t, i))) = heap.pop() {
            if let Some(n) = readers[i].next()? {
                heap.push(std::cmp::Reverse((n, i)));
            }
            if last != Some(t) {
                tiler.push(t)?;
                last = Some(t);
            }
        }
    }
    let (section, count) = tiler.finish(tmp)?;
    for p in runs {
        let _ = std::fs::remove_file(p);
    }
    Ok((section, count))
}

#[cfg(feature = "parallel")]
fn sort_triples(v: &mut [IdTriple]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
}
#[cfg(not(feature = "parallel"))]
fn sort_triples(v: &mut [IdTriple]) {
    v.sort_unstable();
}

struct RunReader {
    rd: BufReader<File>,
}
impl RunReader {
    fn open(path: &Path) -> Result<Self, std::io::Error> {
        Ok(RunReader {
            rd: BufReader::with_capacity(1 << 20, File::open(path)?),
        })
    }
    fn next(&mut self) -> Result<Option<(u32, u32, u32)>, std::io::Error> {
        let mut buf = [0u8; 12];
        match self.rd.read_exact(&mut buf) {
            Ok(()) => Ok(Some((
                u32::from_le_bytes(buf[0..4].try_into().unwrap()),
                u32::from_le_bytes(buf[4..8].try_into().unwrap()),
                u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Streaming re-implementation of [`crate::index::build_tiles`]'s grouping (same
/// byte-budget accounting, same whole-a-group boundaries) that never holds more
/// than the current a-group + the current tile in RAM. Tiles are compressed in
/// parallel batches and spilled; the section payload (directory + tiles +
/// synopsis trailer) is assembled at `finish`, byte-identical to
/// `encode_tiled_section`.
struct StreamingTiler {
    tile_budget: usize,
    codec: u8,
    /// triples of the tile being accumulated
    tile: Vec<(u32, u32, u32)>,
    tile_size: usize,
    /// the a-group slice currently being measured — bounded: a group whose
    /// running encoded size alone exceeds the tile budget is cut into
    /// consecutive tiles sharing the leading id (same rule as `build_tiles`,
    /// via the shared [`GroupSizer`]), so mega-groups (a 2B-triple predicate
    /// in POS/PSO, a hot class object in OSP/OPS) can no longer grow this
    /// buffer to tens of GB
    group: Vec<(u32, u32, u32)>,
    sizer: GroupSizer,
    gtotal: usize,
    prev_a: u32,
    /// finished-but-uncompressed tiles awaiting a parallel compress batch
    pending: Vec<Vec<(u32, u32, u32)>>,
    /// spilled compressed tiles
    comp_out: BufWriter<File>,
    comp_path: PathBuf,
    /// per-tile directory: (min_a, max_a, comp_len, synopsis)
    dir: Vec<TileDirEntry>,
    count: u64,
}

impl StreamingTiler {
    fn new(tmp: &BuildTemp, name: &str, codec: u8) -> Result<Self, ExtBuildError> {
        let comp_path = tmp.path(&format!("{name}.tiles"))?;
        Ok(StreamingTiler {
            tile_budget: INDEX_TILE_BUDGET,
            codec,
            tile: Vec::new(),
            tile_size: 0,
            group: Vec::new(),
            sizer: GroupSizer::start(0, 0),
            gtotal: 0,
            prev_a: 0,
            pending: Vec::new(),
            comp_out: BufWriter::with_capacity(1 << 20, File::create(&comp_path)?),
            comp_path,
            dir: Vec::new(),
            count: 0,
        })
    }

    /// Push the next triple (already permuted, sorted, deduped). Runs the
    /// SAME boundary rules as `build_tiles` (shared `GroupSizer` accounting):
    /// whole-group packing to the byte budget, plus a mid-group cut whenever
    /// the current group's own running size exceeds the budget — the cut is
    /// what bounds this tiler's memory under mega-group skew.
    fn push(&mut self, t: (u32, u32, u32)) -> Result<(), ExtBuildError> {
        self.count += 1;
        if let Some(&(ga, _, _)) = self.group.first() {
            if t.0 != ga {
                self.close_group()?;
            }
        }
        if self.group.is_empty() {
            self.sizer = GroupSizer::start(t.0, self.prev_a);
        }
        self.group.push(t);
        self.gtotal = self.sizer.push(t.1, t.2);
        if self.gtotal > self.tile_budget {
            // Mega-group cut — mirror `build_tiles`: completed groups plus
            // the slice measured so far become ONE tile; the group continues
            // in a fresh chain with `a` as its own delta base.
            self.tile.append(&mut self.group);
            self.flush_tile()?;
            self.tile_size = 0;
            self.prev_a = t.0;
            self.gtotal = 0;
        }
        Ok(())
    }

    /// The current a-group (or its final slice after mid-group cuts) is
    /// complete: pack it into the current tile, flushing first on overflow —
    /// identical to `build_tiles`' end-of-group rule.
    fn close_group(&mut self) -> Result<(), ExtBuildError> {
        if self.group.is_empty() {
            return Ok(());
        }
        let a = self.group[0].0;
        let gsize = self.sizer.total();
        if !self.tile.is_empty() && self.tile_size + gsize > self.tile_budget {
            self.flush_tile()?;
        }
        self.tile_size += gsize;
        self.prev_a = a;
        self.tile.append(&mut self.group);
        Ok(())
    }

    fn flush_tile(&mut self) -> Result<(), ExtBuildError> {
        if self.tile.is_empty() {
            return Ok(());
        }
        self.pending.push(std::mem::take(&mut self.tile));
        self.tile_size = 0;
        if self.pending.len() >= TILE_COMPRESS_BATCH {
            self.compress_pending()?;
        }
        Ok(())
    }

    fn compress_pending(&mut self) -> Result<(), ExtBuildError> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let batch = std::mem::take(&mut self.pending);
        let codec = self.codec;
        // encode + synopsis + compress each tile; par_iter preserves order
        let encode_one = |run: &Vec<(u32, u32, u32)>| -> (u32, u32, Vec<u8>, (u32, u32, u32, u32)) {
            let bytes = encode_sorted_unique(run);
            let blk = crate::triples::TripleBlock::parse(&bytes)
                .expect("the builder's own encoded tile must parse");
            let z = blk.zone();
            let syn = (z.min_b, z.max_b, z.min_c, z.max_c);
            let comp = crate::file::compress(codec, &bytes);
            (run[0].0, run[run.len() - 1].0, comp, syn)
        };
        #[cfg(feature = "parallel")]
        let encoded: Vec<EncodedTile> = {
            use rayon::prelude::*;
            batch.par_iter().map(encode_one).collect()
        };
        #[cfg(not(feature = "parallel"))]
        let encoded: Vec<EncodedTile> = batch.iter().map(encode_one).collect();
        for (min_a, max_a, comp, syn) in encoded {
            self.comp_out.write_all(&comp)?;
            self.dir.push((min_a, max_a, comp.len() as u64, syn));
        }
        Ok(())
    }

    /// Close out and assemble the section payload file.
    fn finish(mut self, tmp: &BuildTemp) -> Result<(SectionFile, u64), ExtBuildError> {
        self.close_group()?;
        self.flush_tile()?;
        self.compress_pending()?;
        self.comp_out.flush()?;
        drop(self.comp_out);

        // [num_tiles][per tile: delta(min_a), max_a-min_a, comp_len][tiles…][synopses…]
        let name = self
            .comp_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("perm")
            .to_string();
        let out_path = tmp.path(&format!("{name}.sec"))?;
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&out_path)?);
        let mut head = Vec::new();
        write_uvarint(&mut head, self.dir.len() as u64);
        let mut prev_min = 0u32;
        for &(min_a, max_a, comp_len, _) in &self.dir {
            write_uvarint(&mut head, (min_a - prev_min) as u64);
            write_uvarint(&mut head, (max_a - min_a) as u64);
            write_uvarint(&mut head, comp_len);
            prev_min = min_a;
        }
        out.write_all(&head)?;
        let mut tiles_in = File::open(&self.comp_path)?;
        let copied = std::io::copy(&mut tiles_in, &mut out)?;
        let mut trailer = Vec::new();
        for &(_, _, _, (min_b, max_b, min_c, max_c)) in &self.dir {
            write_uvarint(&mut trailer, min_b as u64);
            write_uvarint(&mut trailer, (max_b - min_b) as u64);
            write_uvarint(&mut trailer, min_c as u64);
            write_uvarint(&mut trailer, (max_c - min_c) as u64);
        }
        out.write_all(&trailer)?;
        out.flush()?;
        let _ = std::fs::remove_file(&self.comp_path);
        let len = head.len() as u64 + copied + trailer.len() as u64;
        Ok((
            SectionFile {
                path: out_path,
                len,
            },
            self.count,
        ))
    }
}

// ---------------------------------------------------------------------------
// Phase 5: final streaming file assembly
// ---------------------------------------------------------------------------

/// Stream `header | metadata | dict container | index container | footer` to
/// `output`, hashing the payload sections incrementally (same part order as
/// `write_dataset_from_parts`), then patch the finished header at offset 0.
fn write_final_file(
    output: &Path,
    metadata: &[u8],
    dictionary: &SpilledDictionaryRef,
    perm_sections: &[SectionFile],
    quad_count: u64,
    codec: u8,
) -> Result<(), ExtBuildError> {
    use crate::header::{Header, FLAG_HAS_QUOTED_TRIPLES, FLAG_TILE_SYNOPSIS, HEADER_LEN};

    // container framings are tiny; build them in RAM
    let mut dict_frame = Vec::new();
    write_uvarint(&mut dict_frame, 4);
    let mut dict_len = dict_frame.len() as u64;
    let mut dict_section_heads = Vec::with_capacity(4);
    for s in &dictionary.section_paths {
        let mut h = Vec::new();
        let len = std::fs::metadata(s)?.len();
        write_uvarint(&mut h, len);
        dict_len += h.len() as u64 + len;
        dict_section_heads.push(h);
    }

    let mut index_frame = Vec::new();
    write_uvarint(&mut index_frame, perm_sections.len() as u64);
    let mut index_len = index_frame.len() as u64;
    let mut index_section_heads = Vec::with_capacity(perm_sections.len());
    for s in perm_sections {
        let mut h = Vec::new();
        write_uvarint(&mut h, s.len);
        index_len += h.len() as u64 + s.len;
        index_section_heads.push(h);
    }

    let meta_len = metadata.len() as u64;
    let dict_offset = HEADER_LEN as u64 + meta_len;
    let index_offset = dict_offset + dict_len;

    let mut out = BufWriter::with_capacity(1 << 20, File::create(output)?);
    let mut hasher = blake3::Hasher::new();

    // header placeholder
    out.write_all(&[0u8; HEADER_LEN])?;

    // metadata (hashed only when present — mirrors write_dataset_from_parts)
    if meta_len > 0 {
        out.write_all(metadata)?;
        hasher.update(metadata);
    }

    // dict container
    let write_hashed = |out: &mut BufWriter<File>,
                        hasher: &mut blake3::Hasher,
                        bytes: &[u8]|
     -> Result<(), std::io::Error> {
        out.write_all(bytes)?;
        hasher.update(bytes);
        Ok(())
    };
    write_hashed(&mut out, &mut hasher, &dict_frame)?;
    for (s, head) in dictionary.section_paths.iter().zip(&dict_section_heads) {
        write_hashed(&mut out, &mut hasher, head)?;
        copy_hashed(s, &mut out, &mut hasher)?;
    }

    // index container
    write_hashed(&mut out, &mut hasher, &index_frame)?;
    for (s, head) in perm_sections.iter().zip(&index_section_heads) {
        write_hashed(&mut out, &mut hasher, head)?;
        copy_hashed(&s.path, &mut out, &mut hasher)?;
    }

    // pyramid section is empty (hash update of nothing = no-op, same as the
    // in-RAM writer pushing an empty slice); text/named sections absent.

    out.write_all(&crate::header::MAGIC)?; // footer marker
    out.flush()?;

    let mut hash = [0u8; 16];
    hash.copy_from_slice(&hasher.finalize().as_bytes()[..16]);

    let header = Header {
        version: crate::header::CURRENT_FORMAT_VERSION,
        flags: FLAG_TILE_SYNOPSIS
            | if dictionary.has_quoted {
                FLAG_HAS_QUOTED_TRIPLES
            } else {
                0
            },
        metadata_offset: HEADER_LEN as u64,
        metadata_len: meta_len,
        dictionary_offset: dict_offset,
        dictionary_len: dict_len,
        root_dir_offset: index_offset,
        root_dir_len: index_len,
        pyramid_meta_offset: 0,
        pyramid_meta_len: 0,
        dict_codec: codec,
        block_codec: codec,
        pyramid_levels: 0,
        quad_count,
        term_count: dictionary.term_count,
        content_hash: hash,
        named_graphs_offset: 0,
        named_graphs_len: 0,
        schema_meta_len: 0,
        text_index_offset: 0,
        text_index_len: 0,
        extra_sections: Vec::new(),
    };
    let mut f = out.into_inner().map_err(|e| e.into_error())?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&header.to_bytes())?;
    f.flush()?;
    Ok(())
}

fn copy_hashed(
    path: &Path,
    out: &mut BufWriter<File>,
    hasher: &mut blake3::Hasher,
) -> Result<(), std::io::Error> {
    let mut rd = File::open(path)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = rd.read(&mut buf)?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest;

    /// Generate a graph that exercises the merge machinery: terms shared across
    /// chunks, subject-only/object-only/shared classification that differs
    /// per-chunk vs globally, langs, datatypes, bnodes, duplicate triples.
    fn test_quads(n: usize) -> Vec<RawQuad> {
        let mut quads = Vec::new();
        for i in 0..n {
            let s = format!("<http://ex/s{}>", i % (n / 7 + 1));
            let p = format!("<http://ex/p{}>", i % 13);
            let o = match i % 5 {
                0 => format!("<http://ex/s{}>", (i + 3) % (n / 7 + 1)), // makes shared terms
                1 => format!("\"lit {i}\""),
                2 => format!("\"{}\"^^<http://www.w3.org/2001/XMLSchema#integer>", i),
                3 => format!("\"v{}\"@en", i % 50),
                _ => format!("_:b{}", i % 97),
            };
            quads.push((s, p, o, None));
        }
        // duplicates, far apart so they land in different chunks
        for i in 0..(n / 20) {
            let j = i * 17 % n;
            quads.push(quads[j].clone());
        }
        quads
    }

    fn build_reference(quads: Vec<RawQuad>) -> Vec<u8> {
        // reference: the ordinary streaming path, no pyramid / text / metadata
        let qs = quads.clone();
        let (bytes, _stats) = ingest::assemble_dataset_streaming_algo(
            move |visit: &mut dyn FnMut(RawQuad)| {
                for q in qs.iter().cloned() {
                    visit(q);
                }
                Ok(())
            },
            false,
            false,
            None,
            crate::PyramidAlgo::Louvain,
            |_, _, _| Vec::new(),
        )
        .unwrap();
        bytes
    }

    fn build_ext(quads: Vec<RawQuad>, budget: u64) -> (Vec<u8>, BuildStats) {
        static TEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rete-extbuild-test-{}-{budget}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let stats = build_external(
            |visit| {
                for q in quads.iter().cloned() {
                    visit(q)?;
                }
                Ok(())
            },
            &out,
            ExternalBuildOptions {
                memory_budget: budget,
                tmp_dir: Some(dir.clone()),
                metadata: Box::new(|_| Vec::new()),
            },
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (bytes, stats)
    }

    /// The heart of the feature: a tiny budget (forcing many chunks, many sort
    /// runs) must produce **byte-identical** output to the in-RAM build.
    #[test]
    fn external_build_is_byte_identical_to_streaming() {
        let quads = test_quads(3000);
        let reference = build_reference(quads.clone());
        // 64 MiB is the floor; to force chunking on a small graph we can't rely
        // on the floor — instead check both a floor build (1 chunk) and verify
        // chunk-count > 1 via a direct Chunker probe below.
        let (bytes_floor, stats) = build_ext(quads.clone(), 0);
        assert_eq!(stats.statements, quads.len());
        assert_eq!(
            bytes_floor, reference,
            "single-chunk external build must be byte-identical"
        );
    }

    /// Mega-group inputs (one predicate dominating the graph — the Crossref
    /// `cites` shape) exercise the mid-group tile cuts; the external and
    /// in-RAM builds must STILL be byte-identical, proving both tilers cut at
    /// the same boundaries (shared `GroupSizer`).
    #[test]
    fn skewed_external_build_is_byte_identical() {
        let mut quads: Vec<RawQuad> = Vec::new();
        for i in 0..30_000usize {
            quads.push((
                format!("<http://ex/s{}>", i % 500),
                "<http://ex/cites>".to_string(),
                format!("<http://ex/o{i}>"),
                None,
            ));
        }
        for i in 0..500usize {
            quads.push((
                format!("<http://ex/s{i}>"),
                format!("<http://ex/p{}>", i % 7),
                format!("\"lit {i}\""),
                None,
            ));
        }
        let reference = build_reference(quads.clone());
        let (bytes, stats) = build_ext(quads.clone(), 0);
        assert_eq!(stats.statements, quads.len());
        assert_eq!(
            bytes, reference,
            "mega-group external build must be byte-identical"
        );
    }

    /// Multi-chunk path: shrink the chunk budget directly (bypassing the public
    /// floor) so a small graph is split across many chunks + runs, and the
    /// merged output must STILL be byte-identical.
    #[test]
    fn multi_chunk_external_build_is_byte_identical() {
        let quads = test_quads(3000);
        let reference = build_reference(quads.clone());

        let dir = std::env::temp_dir().join(format!("rete-extbuild-mc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");

        // drive the internals directly with a tiny chunk budget
        let tmp = BuildTemp::new(&dir).unwrap();
        let mut chunker = Chunker::new(&tmp, 4 * 1024); // ~4 KiB chunks
        for q in quads.iter().cloned() {
            chunker.push(q).unwrap();
        }
        let chunks = chunker.finish().unwrap();
        assert!(
            chunks.len() >= 4,
            "expected many chunks, got {}",
            chunks.len()
        );
        let statements: u64 = chunks.iter().map(|c| c.triple_count).sum();
        let merged = merge_dictionaries(&tmp, &chunks).unwrap();

        let global_tri = tmp.path("global.tri").unwrap();
        {
            let mut w = BufWriter::new(File::create(&global_tri).unwrap());
            for (ci, _c) in chunks.iter().enumerate() {
                let maps = &merged.remaps[ci];
                let mut rd =
                    BufReader::new(File::open(tmp.path(&format!("c{ci}.tri")).unwrap()).unwrap());
                let mut buf = [0u8; 12];
                while rd.read_exact(&mut buf).is_ok() {
                    let s = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let p = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let o = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                    w.write_all(&maps.subj[(s - 1) as usize].to_le_bytes())
                        .unwrap();
                    w.write_all(&maps.pred[(p - 1) as usize].to_le_bytes())
                        .unwrap();
                    w.write_all(&maps.obj[(o - 1) as usize].to_le_bytes())
                        .unwrap();
                }
            }
            w.flush().unwrap();
        }

        let codec = crate::file::writer_codec();
        let mut sections = Vec::new();
        let mut count = None;
        for perm in crate::index::ALL_PERMS {
            // tiny runs to force multi-run merging
            let (sec, n) = build_permutation_section(&tmp, &global_tri, perm, 256, codec).unwrap();
            if let Some(prev) = count {
                assert_eq!(prev, n);
            }
            count = Some(n);
            sections.push(sec);
        }
        write_final_file(
            &out,
            &[],
            &merged.dictionary_ref(),
            &sections,
            count.unwrap(),
            codec,
        )
        .unwrap();

        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(statements as usize, quads.len());
        assert_eq!(
            bytes, reference,
            "multi-chunk external build must be byte-identical"
        );
        drop(tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The external file must open and answer queries like any other build.
    #[test]
    fn external_build_output_is_queryable() {
        let quads = test_quads(1200);
        let (bytes, _) = build_ext(quads.clone(), 0);
        let rete = crate::Rete::open(&bytes).unwrap();
        // spot-check a known triple through the SPARQL engine
        let out = crate::eval_query(
            &rete,
            "SELECT (COUNT(*) AS ?n) WHERE { ?s <http://ex/p1> ?o }",
        )
        .unwrap();
        match out {
            crate::QueryOutput::Select(_, rows) => assert_eq!(rows.len(), 1),
            other => panic!("expected select, got {other:?}"),
        }
    }

    /// **Operational resume harness** (ignored; run explicitly with env vars):
    /// finish an external build whose process was killed after the dictionary
    /// merge, from its surviving spill directory. Requires:
    ///   RETE_RESUME_SPILL   the .rete-extbuild-* dir (g.*.sec + global.tri +
    ///                       any completed <PERM>.tiles.sec)
    ///   RETE_RESUME_OUT     output .rete path
    ///   RETE_RESUME_TERMS   merged dictionary term count
    ///   RETE_RESUME_QUADS   deduped triple count (statements when no dups)
    ///   RETE_RESUME_CARD    optional JSON card file to embed
    /// Missing permutation sections are rebuilt from global.tri; existing ones
    /// are reused as-is.
    #[test]
    #[ignore = "operational tool, driven by RETE_RESUME_* env vars"]
    fn resume_from_spill() {
        let spill = PathBuf::from(std::env::var("RETE_RESUME_SPILL").expect("RETE_RESUME_SPILL"));
        let out = PathBuf::from(std::env::var("RETE_RESUME_OUT").expect("RETE_RESUME_OUT"));
        let term_count: u64 = std::env::var("RETE_RESUME_TERMS").unwrap().parse().unwrap();
        let quad_count: u64 = std::env::var("RETE_RESUME_QUADS").unwrap().parse().unwrap();
        let metadata: Vec<u8> = std::env::var("RETE_RESUME_CARD")
            .ok()
            .map(|p| std::fs::read(p).unwrap())
            .unwrap_or_default();

        let sec = |name: &str| -> SectionFile {
            let path = spill.join(name);
            let len = std::fs::metadata(&path).expect(name).len();
            SectionFile { path, len }
        };
        let merged = MergedDict {
            section_files: [
                sec("g.shared.sec"),
                sec("g.subj.sec"),
                sec("g.obj.sec"),
                sec("g.pred.sec"),
            ],
            term_count,
            has_quoted: false,
            remaps: Vec::new(),
        };
        let tmp = BuildTemp::adopt_existing_for_resume(spill.clone());
        let global_tri = spill.join("global.tri");
        let codec = crate::file::writer_codec();
        // RETE_RESUME_BUDGET_MB (default 16384) sizes the sort runs — resume
        // with a smaller budget when the machine is busier than the build was
        let budget_mb: u64 = std::env::var("RETE_RESUME_BUDGET_MB")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(16384);
        let run_len = ((budget_mb << 20) / 2 / 24) as usize;

        let mut sections = Vec::new();
        for perm in crate::index::ALL_PERMS {
            let done = spill.join(format!("{}.tiles.sec", perm.name()));
            if done.exists() {
                eprintln!("resume: reusing {}", perm.name());
                let len = std::fs::metadata(&done).unwrap().len();
                sections.push(SectionFile { path: done, len });
                continue;
            }
            // clear partial leftovers of this permutation, then rebuild it
            for entry in std::fs::read_dir(&spill).unwrap().flatten() {
                let n = entry.file_name().to_string_lossy().into_owned();
                if n.starts_with(&format!("{}.", perm.name())) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
            eprintln!("resume: rebuilding {}", perm.name());
            let (s, n) =
                build_permutation_section(&tmp, &global_tri, perm, run_len, codec).unwrap();
            assert_eq!(n, quad_count, "permutation count must match");
            eprintln!("resume: {} done", perm.name());
            sections.push(s);
        }
        write_final_file(
            &out,
            &metadata,
            &merged.dictionary_ref(),
            &sections,
            quad_count,
            codec,
        )
        .unwrap();
        eprintln!("resume: wrote {}", out.display());
        std::mem::forget(tmp); // keep the spill until the file is verified
    }

    /// Named graphs are a clear v1 error, not silent data loss.
    #[test]
    fn named_graphs_are_rejected() {
        let dir = std::env::temp_dir().join(format!("rete-extbuild-ng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let err = build_external(
            |visit| {
                visit((
                    "<http://ex/s>".into(),
                    "<http://ex/p>".into(),
                    "<http://ex/o>".into(),
                    Some("<http://ex/g>".into()),
                ))
            },
            &out,
            ExternalBuildOptions {
                memory_budget: 0,
                tmp_dir: Some(dir.clone()),
                metadata: Box::new(|_| Vec::new()),
            },
        )
        .unwrap_err();
        assert!(matches!(err, ExtBuildError::NamedGraph(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
