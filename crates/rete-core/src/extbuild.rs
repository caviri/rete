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
//!        ├─ chunk 1: …          (+ graph₀ name file + quads₀ when named graphs exist)
//!        └─ chunk K: …
//! merge:  k-way term merge over all chunks
//!           → global front-coded dict sections (streamed to disk)
//!           → per-chunk id remap tables (local id → global id)
//!           → k-way merge of the graph-name files → global graph ordinals
//! remap:  each chunk's default triples remapped to global ids → global.tri
//!         each chunk's named quads remapped → global.qtri (graph ordinal first)
//! index:  per permutation: budget-sized sorted runs → k-way merge (dedup)
//!           → streaming tiler → independently compressed tiles on disk
//! named:  external sort of global.qtri by (g, s, p, o) — so every graph is one
//!           CONTIGUOUS RUN — then one index container per graph, in graph-name
//!           order, appended to a single spill file
//! write:  header | metadata | dict | index | named graphs | footer, hashed
//!           incrementally
//! ```
//!
//! Peak RAM ≈ max(one chunk's working set, remap tables + merge buffers, one
//! sort run, one named graph's index) — all sized from `memory_budget`, never
//! from the dataset.
//!
//! ## Named graphs
//!
//! Graph names are **not** dictionary terms (the format stores them verbatim in
//! the named-graphs section), so they get their own per-chunk name file and
//! their own k-way merge, yielding a global **graph ordinal**. That ordinal is
//! the leading column of the named-quad spill, which makes the external sort do
//! the grouping for free: after the merge, each graph's triples arrive together
//! and its index is built from that run alone. One dictionary, one extra spill,
//! no per-graph files.
//!
//! The default graph keeps its own 12-byte spill and its own sort, untouched —
//! so an input with no named graphs runs exactly the code (and the record
//! widths) it ran before this existed.
//!
//! v1 limits (clear errors, not silent degradation): no community pyramid, no
//! full-text index. SPARQL/joins/verify are unaffected (they never require the
//! pyramid).

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::build_pipeline::spool::{BuildTemp, TripleSpool};
use crate::build_pipeline::timing::{BuildCounters, BuildPhase, BuildTiming};
use crate::build_pipeline::BuildPipelineError;
use crate::dict::env_restart_interval;

use crate::index::{GroupSizer, INDEX_TILE_BUDGET};
use crate::ingest::{BuildStats, IngestError, RawQuad};
use crate::triples::TripleBlockBuilder;
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
    /// Build-conditions payload ([`crate::header::SectionKind::BuildInfo`]),
    /// written verbatim after the metadata section and **excluded from the
    /// content hash** — per-build facts (timestamp, builder, parameters) that
    /// must not make two builds of identical data hash differently. Empty =
    /// no section (byte-identical to a pre-build-info file).
    pub build_info: Vec<u8>,
    /// Which permutations to sort and write ([`crate::index::PermSet::ALL`] by
    /// default). Each one is a full external sort of every triple plus its own
    /// section on disk, so this is the single biggest lever on both build time
    /// and file size that does not touch the data.
    pub perms: crate::index::PermSet,
}

impl Default for ExternalBuildOptions {
    fn default() -> Self {
        Self {
            memory_budget: 4 << 30,
            tmp_dir: None,
            metadata: Box::new(|_| Vec::new()),
            build_info: Vec::new(),
            perms: crate::index::PermSet::ALL,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ExtBuildError {
    #[error("ingest: {0}")]
    Ingest(#[from] IngestError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal: {0}")]
    Internal(&'static str),
    #[error("{0} count does not fit this target")]
    CountOverflow(&'static str),
}

impl ExtBuildError {
    pub(crate) fn into_pipeline(self) -> BuildPipelineError {
        match self {
            Self::Ingest(error) => BuildPipelineError::Ingest(error),
            Self::Io(error) => BuildPipelineError::Io(error),
            Self::Internal(message) => BuildPipelineError::InvalidSpool(message),
            Self::CountOverflow(message) => BuildPipelineError::Overflow(message),
        }
    }
}

fn pipeline_error_to_ext(error: BuildPipelineError) -> ExtBuildError {
    match error {
        BuildPipelineError::Io(error) => ExtBuildError::Io(error),
        BuildPipelineError::Ingest(error) => ExtBuildError::Ingest(error),
        BuildPipelineError::InvalidSpool(message) | BuildPipelineError::Overflow(message) => {
            ExtBuildError::Internal(message)
        }
        BuildPipelineError::File(_)
        | BuildPipelineError::TooManyTerms
        | BuildPipelineError::NamedGraph(_) => {
            ExtBuildError::Internal("external spill allocation failed")
        }
        #[cfg(test)]
        BuildPipelineError::InjectedFailure(message) => ExtBuildError::Internal(message),
    }
}

fn count_to_usize_with_limit(
    value: u64,
    limit: usize,
    name: &'static str,
) -> Result<usize, ExtBuildError> {
    if value > limit as u64 {
        return Err(ExtBuildError::CountOverflow(name));
    }
    usize::try_from(value).map_err(|_| ExtBuildError::CountOverflow(name))
}

fn count_to_usize(value: u64, name: &'static str) -> Result<usize, ExtBuildError> {
    count_to_usize_with_limit(value, usize::MAX, name)
}

/// Bytes charged against the budget per triple of a named graph held resident
/// while its index is built: the triple itself (12 B), the six permuted copies
/// `GraphIndexBuilder::build` keeps in flight, and the tiles they turn into.
/// Charged against **half** the budget, so the graph coexists with the merge
/// readers. A graph over the cap spills and takes the same external
/// per-permutation path the default graph always takes.
const NAMED_GRAPH_BYTES_PER_TRIPLE: u64 = 160;

/// Hard ceiling on a resident named graph, whatever the budget says. Mirrors
/// `ingest::LOWMEM_TRIPLE_THRESHOLD`: past it the in-RAM assembler stops
/// building all six permutations at once, and so does this — by spilling the
/// graph rather than by switching builders, which keeps one resident path.
const NAMED_GRAPH_RAM_TRIPLE_CAP: usize = 30_000_000;

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
    let tmp = TmpDir::create(&tmp_parent)?;
    let budget = opts.memory_budget.max(64 << 20); // floor: 64 MiB

    // ---- Phase 1: chunk the input ------------------------------------------
    let chunk_budget = (budget as f64 * CHUNK_BUDGET_FRACTION) as u64;
    eprintln!(
        "extbuild: budget {} MiB -> chunk target {} MiB",
        budget >> 20,
        chunk_budget >> 20
    );
    let mut chunker = Chunker::new_tmp(&tmp, chunk_budget);
    stream(&mut |q: RawQuad| chunker.push(q))?;
    let chunks = chunker.finish()?;
    let statements: u64 = chunks
        .iter()
        .map(|c| c.triple_count + c.quad_count)
        .sum::<u64>();
    let has_named = chunks.iter().any(|c| c.quad_count > 0);
    eprintln!(
        "extbuild: {} chunk(s), {} statement(s) spilled",
        chunks.len(),
        statements
    );
    timing.lap(BuildPhase::ParseIngest);

    // ---- Phase 2: merge chunk dictionaries into the global dictionary -------
    let mut merged = merge_dictionaries(&tmp, &chunks)?;
    eprintln!(
        "extbuild: merged dictionary — {} term(s)",
        merged.term_count
    );
    if merged.graph_count > 0 {
        eprintln!("extbuild: {} named graph(s)", merged.graph_count);
    }
    timing.lap(BuildPhase::Canonicalize);

    // ---- Phase 3: remap chunk triples to global ids --------------------------
    let remaps = std::mem::take(&mut merged.remaps);
    let global_tri = tmp.path("global.tri")?;
    let global_qtri = tmp.path("global.qtri")?;
    {
        let mut out = BufWriter::new(File::create(&global_tri)?);
        // The named-quad spill is opened only when the input actually carries
        // named graphs, so a default-graph build writes and sorts exactly the
        // bytes it did before quads existed here.
        let mut qout = if has_named {
            Some(BufWriter::new(File::create(&global_qtri)?))
        } else {
            None
        };
        for (ci, chunk) in chunks.iter().enumerate() {
            let maps = &remaps[ci];
            let mut rd = BufReader::new(File::open(tmp.path(&format!("c{ci}.tri"))?)?);
            let mut buf = [0u8; 12];
            loop {
                match rd.read_exact(&mut buf) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e.into()),
                }
                let s = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                let p = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                let o = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                let gs = maps.subj[(s - 1) as usize];
                let gp = maps.pred[(p - 1) as usize];
                let go = maps.obj[(o - 1) as usize];
                out.write_all(&gs.to_le_bytes())?;
                out.write_all(&gp.to_le_bytes())?;
                out.write_all(&go.to_le_bytes())?;
            }
            let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.tri"))?);
            if chunk.quad_count > 0 {
                let qout = qout
                    .as_mut()
                    .ok_or(ExtBuildError::Internal("named quads without a spill"))?;
                let mut rd = BufReader::new(File::open(tmp.path(&format!("c{ci}.qtri"))?)?);
                let mut buf = [0u8; 16];
                loop {
                    match rd.read_exact(&mut buf) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                        Err(e) => return Err(e.into()),
                    }
                    let g = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let s = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let p = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                    let o = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                    qout.write_all(&maps.graph[(g - 1) as usize].to_le_bytes())?;
                    qout.write_all(&maps.subj[(s - 1) as usize].to_le_bytes())?;
                    qout.write_all(&maps.pred[(p - 1) as usize].to_le_bytes())?;
                    qout.write_all(&maps.obj[(o - 1) as usize].to_le_bytes())?;
                }
            }
            let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.qtri"))?);
        }
        out.flush()?;
        if let Some(mut q) = qout {
            q.flush()?;
        }
    }
    drop(remaps); // free the remap tables before the sort phase
    timing.lap(BuildPhase::Remap);

    // ---- Phase 4: per-permutation external sort + streaming tiler -----------
    let codec = crate::file::writer_codec();
    // A run holds R triples resident twice over during sort (Vec + sort scratch).
    let run_len = ((budget / 2) / 24).clamp(1 << 16, u32::MAX as u64) as usize;
    let mut perm_sections: Vec<SectionFile> = Vec::with_capacity(opts.perms.len());
    let mut deduped_count: Option<u64> = None;
    for perm in opts.perms.iter() {
        let (section, n) = build_permutation_section(&tmp, &global_tri, perm, run_len, codec, "")?;
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
    let _ = std::fs::remove_file(&global_tri);
    let default_count = deduped_count.unwrap_or(0);

    // ---- Phase 4b: one index container per named graph ----------------------
    // The graph ordinal leads the sort key, so the merge hands back one
    // contiguous run per graph, in graph-name order — exactly the order the
    // named-graphs section stores them in.
    let named = if has_named {
        // Same accounting as `run_len`, at the quad record's 16 bytes rather
        // than the triple's 12: a run holds Q quads resident twice over, so
        // Q·16·2 = budget/2. Reusing `run_len` here would have quietly spent
        // 4/3 of the intended half-budget on the quad sort.
        let quad_run_len = ((budget / 2) / 32).clamp(1 << 16, u32::MAX as u64) as usize;
        let ram_triples = (((budget / 2) / NAMED_GRAPH_BYTES_PER_TRIPLE).max(1) as usize)
            .min(NAMED_GRAPH_RAM_TRIPLE_CAP);
        let section = build_named_graphs(
            &tmp,
            &global_qtri,
            merged
                .graph_names
                .as_deref()
                .ok_or(ExtBuildError::Internal("named quads without graph names"))?,
            merged.graph_count,
            quad_run_len,
            run_len,
            ram_triples,
            opts.perms,
            codec,
        )?;
        let _ = std::fs::remove_file(&global_qtri);
        eprintln!(
            "extbuild: {} named graph(s) indexed ({} unique quad(s))",
            section.count, section.triples
        );
        Some(section)
    } else {
        None
    };
    let named_triples = named.as_ref().map(|n| n.triples).unwrap_or(0);
    let named_graphs = named.as_ref().map(|n| n.count).unwrap_or(0);
    let quad_count = default_count + named_triples;
    timing.lap(BuildPhase::SubjectFamily);

    // ---- Phase 5: stream the final file -------------------------------------
    // `statements` is what was INGESTED; `quad_count` is what the permutation
    // merge actually kept after dedup, and it is what the header records. The
    // metadata callback (the Dataset Card) has to see the latter, or a build
    // from input containing duplicates publishes a card that disagrees with its
    // own file. They are equal whenever the input has no duplicates, which is
    // why the gap went unnoticed.
    let mut stats = BuildStats {
        statements: count_to_usize(quad_count, "statements")?,
        default_triples: count_to_usize(default_count, "default triples")?,
        named_graphs: count_to_usize(named_graphs, "named graphs")?,
        terms: count_to_usize(merged.term_count, "terms")?,
        pyramid_levels: 0,
    };
    let metadata = (opts.metadata)(&stats);
    write_final_file(
        output,
        &metadata,
        &opts.build_info,
        &merged,
        &perm_sections,
        named.as_ref(),
        quad_count,
        codec,
        opts.perms,
    )?;
    timing.lap(BuildPhase::FinalWrite);
    let bits = opts.perms.bits();
    let family_runs = [
        u64::from((bits & ((1 << 0) | (1 << 3))).count_ones()),
        u64::from((bits & ((1 << 1) | (1 << 4))).count_ones()),
        u64::from((bits & ((1 << 2) | (1 << 5))).count_ones()),
    ];
    timing.set_counters(BuildCounters {
        statements,
        input_bytes: None,
        spill_bytes: spill_file_bytes(&tmp.dir)?,
        output_bytes: std::fs::metadata(output)?.len(),
        family_runs,
    });
    timing.finish();
    stats.pyramid_levels = 0;
    Ok(stats)
}

// ---------------------------------------------------------------------------
// tmp-dir guard
// ---------------------------------------------------------------------------

fn spill_file_bytes(dir: &Path) -> Result<u64, std::io::Error> {
    let mut total = 0u64;
    for entry in std::fs::read_dir(dir)? {
        let metadata = entry?.metadata()?;
        if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

struct TmpDir {
    temp: BuildTemp,
    dir: PathBuf,
}

impl TmpDir {
    fn create(parent: &Path) -> Result<Self, ExtBuildError> {
        let temp = BuildTemp::new_named(parent, "extbuild").map_err(pipeline_error_to_ext)?;
        Ok(Self::from_build_temp(temp))
    }

    fn from_build_temp(temp: BuildTemp) -> Self {
        let dir = temp.root().to_path_buf();
        Self { temp, dir }
    }

    fn path(&self, name: &str) -> Result<PathBuf, ExtBuildError> {
        self.temp.path(name).map_err(pipeline_error_to_ext)
    }
}

// ---------------------------------------------------------------------------
// Phase 1: chunking
// ---------------------------------------------------------------------------

pub(crate) struct ChunkInfo {
    /// default-graph statements in this chunk
    pub(crate) triple_count: u64,
    /// named-graph statements in this chunk
    quad_count: u64,
    /// term counts per section file, in file order (shared, subj, obj, pred)
    section_terms: [u32; 4],
    /// distinct graph names in this chunk (`c<i>.graph` line count)
    graph_terms: u32,
}

pub(crate) struct Chunker {
    tmp: PathBuf,
    chunk_budget: u64,
    acc_bytes: u64,
    quads: Vec<(String, String, String)>,
    /// named-graph statements: `(chunk-local graph id, s, p, o)`. The graph is
    /// interned rather than cloned per quad — fedlex averages ~113 quads per
    /// graph, so storing the name once instead of 113 times is the difference
    /// between the buffer being bounded by distinct graphs and by statements.
    named: Vec<(u32, String, String, String)>,
    /// graph name → chunk-local *insertion* id (ranked at seal)
    graph_ids: std::collections::BTreeMap<String, u32>,
    chunks: Vec<ChunkInfo>,
    has_quoted: bool,
}

impl Chunker {
    fn new_tmp(tmp: &TmpDir, chunk_budget: u64) -> Self {
        Self::new_at(tmp.dir.clone(), chunk_budget)
    }

    pub(crate) fn new(tmp: &BuildTemp, chunk_budget: u64) -> Self {
        Self::new_at(tmp.root().to_path_buf(), chunk_budget)
    }

    fn new_at(tmp: PathBuf, chunk_budget: u64) -> Self {
        Chunker {
            tmp,
            chunk_budget,
            acc_bytes: 0,
            quads: Vec::new(),
            named: Vec::new(),
            graph_ids: std::collections::BTreeMap::new(),
            chunks: Vec::new(),
            has_quoted: false,
        }
    }

    pub(crate) fn push(&mut self, q: RawQuad) -> Result<(), ExtBuildError> {
        let (s, p, o, g) = q;
        self.acc_bytes += (s.len() + p.len() + o.len()) as u64 + PER_QUAD_OVERHEAD;
        match g {
            None => self.quads.push((s, p, o)),
            Some(graph) => {
                let next = self.graph_ids.len() as u32 + 1;
                let gid = match self.graph_ids.get(&graph) {
                    Some(&id) => id,
                    None => {
                        // first sight in this chunk: the name itself is charged
                        // once, the per-quad cost is the 4-byte id above
                        self.acc_bytes += graph.len() as u64 + PER_QUAD_OVERHEAD;
                        self.graph_ids.insert(graph, next);
                        next
                    }
                };
                self.named.push((gid, s, p, o));
            }
        }
        if self.acc_bytes >= self.chunk_budget {
            self.seal()?;
        }
        Ok(())
    }

    /// Build this chunk's private dictionary, spill its four sorted section term
    /// files + local-id triples (+ the graph name file and named quads when the
    /// chunk saw any), then drop everything.
    fn seal(&mut self) -> Result<(), ExtBuildError> {
        if self.quads.is_empty() && self.named.is_empty() {
            return Ok(());
        }
        let ci = self.chunks.len();
        let quads = std::mem::take(&mut self.quads);
        let named = std::mem::take(&mut self.named);
        let graph_ids = std::mem::take(&mut self.graph_ids);
        self.acc_bytes = 0;

        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &quads {
            db.observe(s, p, o);
        }
        // Graph NAMES are not dictionary terms — the format stores them verbatim
        // in the named-graphs section — but the s/p/o of a named quad are, and
        // the in-RAM builder observes them from the same single pass.
        for (_, s, p, o) in &named {
            db.observe(s, p, o);
        }
        let dict = db.build();
        if dict.has_quoted_triples() {
            self.has_quoted = true;
        }

        // local id-triples (subject-space id, predicate id, object-space id)
        let mut tri = BufWriter::new(File::create(self.tmp.join(format!("c{ci}.tri")))?);
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

        // Named quads: the chunk-local graph id is the name's RANK in this
        // chunk's sorted name file, so `c<i>.graph` and the ids in `c<i>.qtri`
        // agree and the same k-way machinery that merges terms merges graphs.
        // (Graph names are NT tokens — an IRI or a blank node — so they never
        // contain a raw \n, the same property the term files rely on.)
        let quad_count = named.len() as u64;
        let graph_terms = graph_ids.len() as u32;
        if !named.is_empty() {
            let mut rank_of = vec![0u32; graph_ids.len() + 1];
            let mut gw = BufWriter::new(File::create(self.tmp.join(format!("c{ci}.graph")))?);
            for (rank, (name, insertion_id)) in graph_ids.iter().enumerate() {
                rank_of[*insertion_id as usize] = rank as u32 + 1;
                gw.write_all(name.as_bytes())?;
                gw.write_all(b"\n")?;
            }
            gw.flush()?;
            let mut qw = BufWriter::new(File::create(self.tmp.join(format!("c{ci}.qtri")))?);
            for (gid, s, p, o) in &named {
                let (si, pi, oi) = dict
                    .encode(s, p, o)
                    .ok_or(ExtBuildError::Internal("chunk term missing from own dict"))?;
                qw.write_all(&rank_of[*gid as usize].to_le_bytes())?;
                qw.write_all(&si.to_le_bytes())?;
                qw.write_all(&pi.to_le_bytes())?;
                qw.write_all(&oi.to_le_bytes())?;
            }
            qw.flush()?;
        }
        drop(named);

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
            let mut w = BufWriter::new(File::create(self.tmp.join(name))?);
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
            "extbuild: chunk {ci} sealed — {} statement(s), {} term(s), {graph_terms} graph(s)",
            triple_count + quad_count,
            shared + subj_only + obj_only + preds
        );
        self.chunks.push(ChunkInfo {
            triple_count,
            quad_count,
            section_terms: [shared, subj_only, obj_only, preds],
            graph_terms,
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
                quad_count: 0,
                section_terms: [0, 0, 0, 0],
                graph_terms: 0,
            });
            for name in ["c0.tri", "c0.shared", "c0.subj", "c0.obj", "c0.pred"] {
                File::create(self.tmp.join(name))?;
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
    /// chunk-local graph rank → global graph ordinal (empty when the chunk saw
    /// no named graphs)
    graph: Vec<u32>,
}

pub(crate) struct MergedDict {
    /// The four chunked-encoded dict section payloads, spilled to tmp files
    /// (shared, subjects, objects, predicates — the container order).
    section_files: [SectionFile; 4],
    pub(crate) term_count: u64,
    pub(crate) has_quoted: bool,
    remaps: Vec<ChunkRemap>,
    /// Every distinct graph name, ascending, one per line — spilled rather than
    /// resident because fedlex has 497,905 of them. `None` when the input had
    /// no named graphs.
    graph_names: Option<PathBuf>,
    graph_count: u32,
}

impl MergedDict {
    pub(crate) fn section_paths(&self) -> [PathBuf; 4] {
        self.section_files
            .each_ref()
            .map(|section| section.path.clone())
    }
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

fn merge_dictionaries(tmp: &TmpDir, chunks: &[ChunkInfo]) -> Result<MergedDict, ExtBuildError> {
    merge_dictionaries_at(&tmp.dir, chunks)
}

pub(crate) fn merge_pipeline_dictionaries(
    tmp: &BuildTemp,
    chunks: &[ChunkInfo],
) -> Result<MergedDict, ExtBuildError> {
    merge_dictionaries_at(tmp.root(), chunks)
}

fn merge_dictionaries_at(tmp: &Path, chunks: &[ChunkInfo]) -> Result<MergedDict, ExtBuildError> {
    let k = chunks.len();

    // remap tables, sized by each chunk's spaces
    let mut remaps: Vec<ChunkRemap> = chunks
        .iter()
        .map(|c| ChunkRemap {
            subj: vec![0; (c.section_terms[0] + c.section_terms[1]) as usize],
            obj: vec![0; (c.section_terms[0] + c.section_terms[2]) as usize],
            pred: vec![0; c.section_terms[3] as usize],
            graph: vec![0; c.graph_terms as usize],
        })
        .collect();

    // subject-space + object-space k-way streams
    let mut subj_streams = Vec::with_capacity(k);
    let mut obj_streams = Vec::with_capacity(k);
    for (ci, c) in chunks.iter().enumerate() {
        subj_streams.push(SpaceStream::new(
            TermFileReader::open(&tmp.join(format!("c{ci}.shared")))?,
            TermFileReader::open(&tmp.join(format!("c{ci}.subj")))?,
            c.section_terms[0],
        )?);
        obj_streams.push(SpaceStream::new(
            TermFileReader::open(&tmp.join(format!("c{ci}.shared")))?,
            TermFileReader::open(&tmp.join(format!("c{ci}.obj")))?,
            c.section_terms[0],
        )?);
    }
    let mut subjects = KWayTerms::new(subj_streams)?;
    let mut objects = KWayTerms::new(obj_streams)?;

    // The three node sections are written as raw front-coded bodies first; the
    // final chunked encoding needs term counts, so bodies spill to tmp.
    let mut shared_sec = RawSectionWriter::create(tmp.join("g.shared.raw"))?;
    let mut subj_sec = RawSectionWriter::create(tmp.join("g.subj.raw"))?;
    let mut obj_sec = RawSectionWriter::create(tmp.join("g.obj.raw"))?;
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
    let mut pred_sec = RawSectionWriter::create(tmp.join("g.pred.raw"))?;
    {
        let mut streams = Vec::with_capacity(k);
        for (ci, _) in chunks.iter().enumerate() {
            // empty shared reader + real pred reader => a plain single stream
            streams.push(SpaceStream::new(
                TermFileReader::open(&tmp.join(format!("c{ci}.pred")))?,
                TermFileReader::open(&tmp.join(format!("c{ci}.pred.empty",))).or_else(
                    |_| -> Result<TermFileReader, std::io::Error> {
                        // create-once empty file per chunk
                        let p = tmp.join(format!("c{ci}.pred.empty"));
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

    // graph names: the same k-way merge, into a plain sorted name list rather
    // than a front-coded dict section — the format stores graph names verbatim.
    let mut graph_count = 0u32;
    let graph_names = if chunks.iter().any(|c| c.graph_terms > 0) {
        let path = tmp.join("g.graphs");
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let mut streams = Vec::with_capacity(k);
        for (ci, c) in chunks.iter().enumerate() {
            let real = tmp.join(format!("c{ci}.graph"));
            if !real.exists() {
                File::create(&real)?;
            }
            let empty = tmp.join(format!("c{ci}.graph.empty"));
            File::create(&empty)?;
            streams.push(SpaceStream::new(
                TermFileReader::open(&real)?,
                TermFileReader::open(&empty)?,
                c.graph_terms,
            )?);
        }
        let mut kway = KWayTerms::new(streams)?;
        while let Some((name, carriers)) = kway.next()? {
            graph_count += 1;
            for (ci, lid) in carriers {
                remaps[ci].graph[(lid - 1) as usize] = graph_count;
            }
            w.write_all(name.as_bytes())?;
            w.write_all(b"\n")?;
        }
        w.flush()?;
        Some(path)
    } else {
        None
    };

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
        for suffix in [
            "shared",
            "subj",
            "obj",
            "pred",
            "pred.empty",
            "graph",
            "graph.empty",
        ] {
            let _ = std::fs::remove_file(tmp.join(format!("c{ci}.{suffix}")));
        }
    }

    let term_count =
        n_shared as u64 + n_subj as u64 + n_obj as u64 + section_files[3].term_count as u64;

    Ok(MergedDict {
        section_files: section_files.map(|s| s.file),
        term_count,
        has_quoted,
        remaps,
        graph_names,
        graph_count,
    })
}

/// Remap each default-graph chunk into one canonical replayable triple spool.
/// The staged pipeline rejects named graphs before they reach this helper; the
/// production external builder keeps its separate named-quad path unchanged.
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
    let path = temp.path("global.tri").map_err(|error| match error {
        BuildPipelineError::Io(error) => ExtBuildError::Io(error),
        _ => ExtBuildError::Internal("triple spool path failed"),
    })?;
    let mut output = BufWriter::new(File::create(&path)?);
    let remaps = std::mem::take(&mut merged.remaps);
    for (chunk_index, _) in chunks.iter().enumerate() {
        let maps = remaps
            .get(chunk_index)
            .ok_or(ExtBuildError::Internal("chunk remap missing"))?;
        let chunk_path = temp
            .path(&format!("c{chunk_index}.tri"))
            .map_err(|_| ExtBuildError::Internal("chunk triple path failed"))?;
        let mut input = BufReader::new(File::open(&chunk_path)?);
        loop {
            let mut record = [0u8; 12];
            match input.read_exact(&mut record) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    if std::fs::metadata(&chunk_path)?.len() % 12 != 0 {
                        return Err(ExtBuildError::Internal("partial chunk triple record"));
                    }
                    break;
                }
                Err(error) => return Err(error.into()),
            }
            let read_id = |bytes: &[u8], message| {
                bytes
                    .try_into()
                    .map(u32::from_le_bytes)
                    .map_err(|_| ExtBuildError::Internal(message))
            };
            let subject = read_id(&record[0..4], "subject record width")?;
            let predicate = read_id(&record[4..8], "predicate record width")?;
            let object = read_id(&record[8..12], "object record width")?;
            let remap = |ids: &[u32], id: u32, message| {
                id.checked_sub(1)
                    .and_then(|index| usize::try_from(index).ok())
                    .and_then(|index| ids.get(index).copied())
                    .filter(|mapped| *mapped != 0)
                    .ok_or(ExtBuildError::Internal(message))
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
        BuildPipelineError::Ingest(error) => ExtBuildError::Ingest(error),
        BuildPipelineError::InvalidSpool(message) | BuildPipelineError::Overflow(message) => {
            ExtBuildError::Internal(message)
        }
        BuildPipelineError::File(_)
        | BuildPipelineError::TooManyTerms
        | BuildPipelineError::NamedGraph(_) => {
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
        tmp: &Path,
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
        let comp_path = tmp.join(format!("{out_name}.chunks"));
        let mut comp_out = BufWriter::new(File::create(&comp_path)?);
        let mut dir: Vec<(usize, Vec<u8>, u64)> = Vec::with_capacity(bounds.len());
        // The previous chunk's last term — the separator's lower bound. Kept in
        // lockstep with `encode_chunked_dict_section`; the two writers must emit
        // byte-identical directories for the same input.
        let mut prev_last: Option<Vec<u8>> = None;
        for (i, &(first_run, start, end)) in bounds.iter().enumerate() {
            body.seek(SeekFrom::Start(start))?;
            let mut raw = vec![0u8; (end - start) as usize];
            body.read_exact(&mut raw)?;
            // the chunk's first run entry is a restart: [0][len][full term]
            let first_term = read_restart_term(&raw)
                .ok_or(ExtBuildError::Internal("restart entry unreadable"))?;
            // Routing key = the shortest separator, not the first term; chunk 0
            // needs none. See `crate::dict::SectionChunk::key`.
            let key = if i == 0 {
                Vec::new()
            } else {
                match &prev_last {
                    None => first_term,
                    Some(pl) => crate::dict::shortest_separator(pl, &first_term),
                }
            };
            if i + 1 < bounds.len() {
                // This chunk's last run starts one run before the next chunk's;
                // `raw` already holds the bytes, so decoding it is free of I/O.
                let last_run_rel = (offs[bounds[i + 1].0 - 1] - start) as usize;
                prev_last = crate::dict::run_last_term(&raw, last_run_rel, raw.len());
            }
            let comp = crate::file::compress(codec, &raw);
            comp_out.write_all(&comp)?;
            dir.push((first_run, key, comp.len() as u64));
        }
        comp_out.flush()?;
        drop(body);
        let _ = std::fs::remove_file(&self.path);

        // assemble: [header_len][header][num_chunks][dir…][compressed chunks…]
        let out_path = tmp.join(out_name);
        let mut out = BufWriter::new(File::create(&out_path)?);
        let mut head = Vec::new();
        write_uvarint(&mut head, header.len() as u64);
        head.extend_from_slice(&header);
        write_uvarint(&mut head, dir.len() as u64);
        let mut prev_run = 0usize;
        for (first_run, key, comp_len) in &dir {
            write_uvarint(&mut head, (*first_run - prev_run) as u64);
            write_uvarint(&mut head, key.len() as u64);
            head.extend_from_slice(key);
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
    tmp: &TmpDir,
    global_tri: &Path,
    perm: crate::index::IndexPermutation,
    run_len: usize,
    codec: u8,
    prefix: &str,
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
                let path = tmp.path(&format!("{prefix}{}.run{}", perm.name(), runs.len()))?;
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
    let mut tiler = StreamingTiler::new(tmp, &format!("{prefix}{}", perm.name()), codec)?;
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

// ---------------------------------------------------------------------------
// Phase 4b: named graphs — the graph ordinal leads the sort key
// ---------------------------------------------------------------------------

/// A spilled quad in `(graph ordinal, s, p, o)` order.
type IdQuad = (u32, u32, u32, u32);
/// Quad merge-heap entry: a quad tagged with the run it came from.
type QuadMergeEntry = std::cmp::Reverse<(IdQuad, usize)>;

/// The finished named-graphs section, spilled: a `count` and the concatenated
/// per-graph `(iri, index container)` records the section prefixes it with.
struct NamedSection {
    body: PathBuf,
    body_len: u64,
    /// number of named graphs
    count: u64,
    /// deduplicated statements across every named graph
    triples: u64,
}

struct QuadRunReader {
    rd: BufReader<File>,
}
impl QuadRunReader {
    fn open(path: &Path) -> Result<Self, std::io::Error> {
        Ok(QuadRunReader {
            rd: BufReader::with_capacity(1 << 20, File::open(path)?),
        })
    }
    fn next(&mut self) -> Result<Option<IdQuad>, std::io::Error> {
        let mut buf = [0u8; 16];
        match self.rd.read_exact(&mut buf) {
            Ok(()) => Ok(Some((
                u32::from_le_bytes(buf[0..4].try_into().unwrap()),
                u32::from_le_bytes(buf[4..8].try_into().unwrap()),
                u32::from_le_bytes(buf[8..12].try_into().unwrap()),
                u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

#[cfg(feature = "parallel")]
fn sort_quads(v: &mut [IdQuad]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
}
#[cfg(not(feature = "parallel"))]
fn sort_quads(v: &mut [IdQuad]) {
    v.sort_unstable();
}

fn uvarint_len(v: u64) -> u64 {
    let mut b = Vec::new();
    write_uvarint(&mut b, v);
    b.len() as u64
}

/// Sort `global_qtri` by `(graph, s, p, o)` — so each graph is one contiguous
/// run — and write one index container per graph, in graph-name order, into a
/// single spill file. `ram_triples` caps how many of a graph's triples may be
/// held resident: a graph over the cap spills and is indexed by the same
/// external per-permutation path the default graph uses, so a single 2-billion
/// quad graph is no worse than the same data in the default graph.
#[allow(clippy::too_many_arguments)]
fn build_named_graphs(
    tmp: &TmpDir,
    global_qtri: &Path,
    graph_names: &Path,
    graph_count: u32,
    quad_run_len: usize,
    tri_run_len: usize,
    ram_triples: usize,
    perms: crate::index::PermSet,
    codec: u8,
) -> Result<NamedSection, ExtBuildError> {
    // 1. sorted runs over (g, s, p, o)
    let mut runs: Vec<PathBuf> = Vec::new();
    {
        let mut rd = BufReader::with_capacity(1 << 20, File::open(global_qtri)?);
        let mut buf = [0u8; 16];
        let mut run: Vec<IdQuad> = Vec::with_capacity(quad_run_len.min(1 << 22));
        loop {
            let eof = match rd.read_exact(&mut buf) {
                Ok(()) => false,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => true,
                Err(e) => return Err(e.into()),
            };
            if !eof {
                run.push((
                    u32::from_le_bytes(buf[0..4].try_into().unwrap()),
                    u32::from_le_bytes(buf[4..8].try_into().unwrap()),
                    u32::from_le_bytes(buf[8..12].try_into().unwrap()),
                    u32::from_le_bytes(buf[12..16].try_into().unwrap()),
                ));
            }
            if run.len() >= quad_run_len || (eof && !run.is_empty()) {
                sort_quads(&mut run);
                run.dedup();
                let path = tmp.path(&format!("q.run{}", runs.len()))?;
                let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
                for &(g, s, p, o) in &run {
                    w.write_all(&g.to_le_bytes())?;
                    w.write_all(&s.to_le_bytes())?;
                    w.write_all(&p.to_le_bytes())?;
                    w.write_all(&o.to_le_bytes())?;
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

    // 2. k-way merge (dedup) → one contiguous run per graph
    let body_path = tmp.path("named.body")?;
    let mut body = BufWriter::with_capacity(1 << 20, File::create(&body_path)?);
    let mut names = TermFileReader::open(graph_names)?;
    let mut emitted = 0u32;
    let mut total: u64 = 0;
    let mut body_len: u64 = 0;

    let mut readers: Vec<QuadRunReader> = runs
        .iter()
        .map(|p| QuadRunReader::open(p))
        .collect::<Result<_, _>>()?;
    let mut heap: BinaryHeap<QuadMergeEntry> = BinaryHeap::new();
    for (i, r) in readers.iter_mut().enumerate() {
        if let Some(q) = r.next()? {
            heap.push(std::cmp::Reverse((q, i)));
        }
    }
    let mut cur = GraphRun::new();
    let mut last: Option<IdQuad> = None;
    while let Some(std::cmp::Reverse((q, i))) = heap.pop() {
        if let Some(n) = readers[i].next()? {
            heap.push(std::cmp::Reverse((n, i)));
        }
        if last == Some(q) {
            continue;
        }
        last = Some(q);
        if cur.graph != 0 && cur.graph != q.0 {
            // The name file is read line by line in lockstep with the ordinals,
            // so a skipped or out-of-order ordinal would silently pair a graph's
            // triples with another graph's IRI. Ordinals are dense and ascending
            // by construction; check it rather than trust it.
            if cur.graph != emitted + 1 {
                return Err(ExtBuildError::Internal(
                    "graph ordinals are not dense and ascending",
                ));
            }
            body_len += cur.finish(
                tmp,
                &mut body,
                &mut names,
                tri_run_len,
                perms,
                codec,
                &mut total,
            )?;
            emitted += 1;
            cur = GraphRun::new();
        }
        cur.graph = q.0;
        cur.push(tmp, (q.1, q.2, q.3), ram_triples)?;
    }
    if cur.graph != 0 {
        if cur.graph != emitted + 1 {
            return Err(ExtBuildError::Internal(
                "graph ordinals are not dense and ascending",
            ));
        }
        body_len += cur.finish(
            tmp,
            &mut body,
            &mut names,
            tri_run_len,
            perms,
            codec,
            &mut total,
        )?;
        emitted += 1;
    }
    body.flush()?;
    drop(body);
    for p in runs {
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_file(graph_names);

    if emitted != graph_count {
        return Err(ExtBuildError::Internal(
            "named-graph count diverges from the merged graph names",
        ));
    }
    Ok(NamedSection {
        body: body_path,
        body_len,
        count: emitted as u64,
        triples: total,
    })
}

/// One graph's contiguous run: resident until it outgrows `ram_triples`, then
/// spilled to a 12-byte `(s, p, o)` file and indexed externally.
struct GraphRun {
    graph: u32,
    resident: Vec<IdTriple>,
    spill: Option<(PathBuf, BufWriter<File>)>,
    n: u64,
}

impl GraphRun {
    fn new() -> Self {
        GraphRun {
            graph: 0,
            resident: Vec::new(),
            spill: None,
            n: 0,
        }
    }

    fn push(&mut self, tmp: &TmpDir, t: IdTriple, ram_triples: usize) -> Result<(), ExtBuildError> {
        self.n += 1;
        if self.spill.is_none() && self.resident.len() < ram_triples {
            self.resident.push(t);
            return Ok(());
        }
        if self.spill.is_none() {
            let path = tmp.path(&format!("ng{}.tri", self.graph))?;
            let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
            for &(a, b, c) in &self.resident {
                w.write_all(&a.to_le_bytes())?;
                w.write_all(&b.to_le_bytes())?;
                w.write_all(&c.to_le_bytes())?;
            }
            self.resident = Vec::new();
            self.spill = Some((path, w));
        }
        let (_, w) = self.spill.as_mut().unwrap();
        w.write_all(&t.0.to_le_bytes())?;
        w.write_all(&t.1.to_le_bytes())?;
        w.write_all(&t.2.to_le_bytes())?;
        Ok(())
    }

    /// Emit `[iri_len][iri][container_len][container]` and return its byte length.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        self,
        tmp: &TmpDir,
        body: &mut BufWriter<File>,
        names: &mut TermFileReader,
        tri_run_len: usize,
        perms: crate::index::PermSet,
        codec: u8,
        total: &mut u64,
    ) -> Result<u64, ExtBuildError> {
        let iri = names
            .next()?
            .ok_or(ExtBuildError::Internal("graph name file exhausted"))?;
        *total += self.n;

        let mut head = Vec::new();
        write_uvarint(&mut head, iri.len() as u64);
        head.extend_from_slice(iri.as_bytes());

        match self.spill {
            // Small graph (the overwhelming majority — fedlex averages ~113
            // quads): build it with the very call the in-RAM builder makes for a
            // graph this size, so byte-identity is not a second
            // implementation's promise — and so a dataset with half a million
            // tiny graphs is not half a million trips through the low-RAM
            // builder's nested rayon, which for a 60-triple graph is all
            // overhead and no parallelism.
            None => {
                let index = crate::GraphIndexBuilder::from_triples(self.resident)
                    .with_perms(perms)
                    .build();
                let container = crate::file::encode_index_container(&index, codec);
                write_uvarint(&mut head, container.len() as u64);
                body.write_all(&head)?;
                body.write_all(&container)?;
                Ok(head.len() as u64 + container.len() as u64)
            }
            // A graph too big for the budget takes the same external path the
            // default graph always takes, section by section.
            Some((path, mut w)) => {
                w.flush()?;
                drop(w);
                let prefix = format!("ng{}.", self.graph);
                let mut sections = Vec::with_capacity(perms.len());
                for perm in perms.iter() {
                    let (sec, _) =
                        build_permutation_section(tmp, &path, perm, tri_run_len, codec, &prefix)?;
                    sections.push(sec);
                }
                let _ = std::fs::remove_file(&path);
                let mut clen = uvarint_len(sections.len() as u64);
                for s in &sections {
                    clen += uvarint_len(s.len) + s.len;
                }
                write_uvarint(&mut head, clen);
                body.write_all(&head)?;
                write_uvarint_to(body, sections.len() as u64)?;
                for s in &sections {
                    write_uvarint_to(body, s.len)?;
                    let mut rd = File::open(&s.path)?;
                    std::io::copy(&mut rd, body)?;
                    drop(rd);
                    let _ = std::fs::remove_file(&s.path);
                }
                Ok(head.len() as u64 + clen)
            }
        }
    }
}

fn write_uvarint_to(w: &mut BufWriter<File>, v: u64) -> Result<(), std::io::Error> {
    let mut b = Vec::new();
    write_uvarint(&mut b, v);
    w.write_all(&b)
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
    fn new(tmp: &TmpDir, name: &str, codec: u8) -> Result<Self, ExtBuildError> {
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
            let mut b = TripleBlockBuilder::new();
            for &t in run {
                b.push(t);
            }
            let bytes = b.build();
            let syn = match crate::triples::TripleBlock::parse(&bytes) {
                Ok(blk) => {
                    let z = blk.zone();
                    (z.min_b, z.max_b, z.min_c, z.max_c)
                }
                Err(_) => (0, u32::MAX, 0, u32::MAX),
            };
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
    fn finish(mut self, tmp: &TmpDir) -> Result<(SectionFile, u64), ExtBuildError> {
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
#[allow(clippy::too_many_arguments)]
fn write_final_file(
    output: &Path,
    metadata: &[u8],
    build_info: &[u8],
    dict: &MergedDict,
    perm_sections: &[SectionFile],
    named: Option<&NamedSection>,
    quad_count: u64,
    codec: u8,
    perms: crate::index::PermSet,
) -> Result<(), ExtBuildError> {
    use crate::header::{
        Header, FLAG_HAS_QUADS, FLAG_HAS_QUOTED_TRIPLES, FLAG_TILE_SYNOPSIS, HEADER_LEN,
    };

    // container framings are tiny; build them in RAM
    let mut dict_frame = Vec::new();
    write_uvarint(&mut dict_frame, 4);
    let mut dict_len = dict_frame.len() as u64;
    let mut dict_section_heads = Vec::with_capacity(4);
    for s in &dict.section_files {
        let mut h = Vec::new();
        write_uvarint(&mut h, s.len);
        dict_len += h.len() as u64 + s.len;
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

    // The named-graphs section is `[count][per graph: iri, container]`; only the
    // count prefix is built here, the records stream from their spill file.
    let mut named_frame = Vec::new();
    let named_len = match named {
        Some(n) => {
            write_uvarint(&mut named_frame, n.count);
            named_frame.len() as u64 + n.body_len
        }
        None => 0,
    };

    let meta_len = metadata.len() as u64;
    let build_len = build_info.len() as u64;
    let dict_offset = HEADER_LEN as u64 + meta_len + build_len;
    let index_offset = dict_offset + dict_len;
    // pyramid and text index are absent (length 0), so the named graphs follow
    // the index directly — the same arithmetic `write_dataset_from_parts` does.
    let named_offset = index_offset + index_len;

    let mut out = BufWriter::with_capacity(1 << 20, File::create(output)?);
    let mut hasher = blake3::Hasher::new();

    // header placeholder
    out.write_all(&[0u8; HEADER_LEN])?;

    // metadata (hashed only when present — mirrors write_dataset_from_parts)
    if meta_len > 0 {
        out.write_all(metadata)?;
        hasher.update(metadata);
    }
    // build-info: written adjacent to the metadata (so a card reader fetches
    // both in one coalesced range) but NOT hashed — it records per-build facts
    // that must not perturb the reproducible content hash.
    if build_len > 0 {
        out.write_all(build_info)?;
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
    for (s, head) in dict.section_files.iter().zip(&dict_section_heads) {
        write_hashed(&mut out, &mut hasher, head)?;
        copy_hashed(&s.path, &mut out, &mut hasher)?;
    }

    // index container
    write_hashed(&mut out, &mut hasher, &index_frame)?;
    for (s, head) in perm_sections.iter().zip(&index_section_heads) {
        write_hashed(&mut out, &mut hasher, head)?;
        copy_hashed(&s.path, &mut out, &mut hasher)?;
    }

    // pyramid section is empty (hash update of nothing = no-op, same as the
    // in-RAM writer pushing an empty slice); the text index is absent.

    // named-graphs container
    if let Some(n) = named {
        write_hashed(&mut out, &mut hasher, &named_frame)?;
        copy_hashed(&n.body, &mut out, &mut hasher)?;
    }

    out.write_all(&crate::header::MAGIC)?; // footer marker
    out.flush()?;

    let mut hash = [0u8; 16];
    hash.copy_from_slice(&hasher.finalize().as_bytes()[..16]);

    let header = Header {
        version: crate::header::CURRENT_FORMAT_VERSION,
        flags: FLAG_TILE_SYNOPSIS
            | if named_len > 0 { FLAG_HAS_QUADS } else { 0 }
            | if dict.has_quoted {
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
        perms,
        quad_count,
        term_count: dict.term_count,
        content_hash: hash,
        named_graphs_offset: if named_len > 0 { named_offset } else { 0 },
        named_graphs_len: named_len,
        schema_meta_len: 0,
        text_index_offset: 0,
        text_index_len: 0,
        build_info_offset: if build_len > 0 {
            HEADER_LEN as u64 + meta_len
        } else {
            0
        },
        build_info_len: build_len,
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
                build_info: Vec::new(),
                perms: crate::index::PermSet::ALL,
            },
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (bytes, stats)
    }

    #[test]
    fn external_build_counts_reject_target_width_overflow() {
        assert_eq!(
            count_to_usize_with_limit(u32::MAX as u64, u32::MAX as usize, "statements").unwrap(),
            u32::MAX as usize
        );
        assert!(matches!(
            count_to_usize_with_limit(u32::MAX as u64 + 1, u32::MAX as usize, "statements"),
            Err(ExtBuildError::CountOverflow("statements"))
        ));
    }

    #[test]
    fn external_tmp_retries_an_occupied_candidate_and_contains_paths() {
        let parent =
            std::env::temp_dir().join(format!("rete-extbuild-collision-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir(&parent).unwrap();
        let occupied = parent.join(format!(".rete-extbuild-{}-41", std::process::id()));
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("marker"), b"owned by somebody else").unwrap();

        let temp =
            BuildTemp::new_named_with_sequence_for_test(&parent, "extbuild", &[41, 42]).unwrap();
        let tmp = TmpDir::from_build_temp(temp);
        assert_ne!(tmp.dir, occupied);
        assert_eq!(
            std::fs::read(occupied.join("marker")).unwrap(),
            b"owned by somebody else"
        );
        assert!(tmp.path("../escape").is_err());
        drop(tmp);
        assert!(occupied.exists());
        std::fs::remove_dir_all(&parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn external_tmp_never_follows_an_occupied_symlink_candidate() {
        use std::os::unix::fs::symlink;

        let parent =
            std::env::temp_dir().join(format!("rete-extbuild-symlink-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir(&parent).unwrap();
        let outside = parent.join("outside");
        std::fs::create_dir(&outside).unwrap();
        let occupied = parent.join(format!(".rete-extbuild-{}-51", std::process::id()));
        symlink(&outside, &occupied).unwrap();

        let temp =
            BuildTemp::new_named_with_sequence_for_test(&parent, "extbuild", &[51, 52]).unwrap();
        let tmp = TmpDir::from_build_temp(temp);
        std::fs::write(tmp.path("scratch").unwrap(), b"safe").unwrap();
        assert!(!outside.join("scratch").exists());
        drop(tmp);
        std::fs::remove_dir_all(&parent).unwrap();
    }

    /// A build-info payload is written as a kind-7 section, readable back, and
    /// leaves the content hash identical to a build without one — the external
    /// builder's version of the outside-the-hash property.
    #[test]
    fn external_build_info_is_outside_the_hash() {
        let quads = test_quads(500);
        let (plain, _) = build_ext(quads.clone(), 0);

        let dir = std::env::temp_dir().join(format!("rete-extbuild-bi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let info = br#"{"built_at":"2026-08-04T00:00:00Z"}"#.to_vec();
        build_external(
            |visit| {
                for q in quads.iter().cloned() {
                    visit(q)?;
                }
                Ok(())
            },
            &out,
            ExternalBuildOptions {
                memory_budget: 0,
                tmp_dir: Some(dir.clone()),
                metadata: Box::new(|_| Vec::new()),
                build_info: info.clone(),
                perms: crate::index::PermSet::ALL,
            },
        )
        .unwrap();
        let with = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(crate::read_build_info(&with).unwrap().unwrap(), info);
        let h_with = crate::Header::from_bytes(&with).unwrap();
        let h_plain = crate::Header::from_bytes(&plain).unwrap();
        assert_eq!(h_with.content_hash, h_plain.content_hash);
        assert!(crate::verify(&with).unwrap());
        // Stripping the section restores the plain image byte-for-byte.
        assert_eq!(crate::attach_build_info(&with, &[]).unwrap(), plain);
        // And the graph still opens.
        crate::Rete::open(&with).unwrap();
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
        // `test_quads` repeats some statements, so this fixture is exactly the
        // case #128 was about: the reported count is what the build WROTE (and
        // what the header records), not what it read.
        let header = crate::Header::from_bytes(&bytes_floor).unwrap();
        assert_eq!(stats.statements as u64, header.quad_count);
        assert!(
            stats.statements < quads.len(),
            "fixture must contain duplicates for this to mean anything"
        );
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
        let tmp = TmpDir::create(&dir).unwrap();
        let mut chunker = Chunker::new_tmp(&tmp, 4 * 1024); // ~4 KiB chunks
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
            let (sec, n) =
                build_permutation_section(&tmp, &global_tri, perm, 256, codec, "").unwrap();
            if let Some(prev) = count {
                assert_eq!(prev, n);
            }
            count = Some(n);
            sections.push(sec);
        }
        write_final_file(
            &out,
            &[],
            &[],
            &merged,
            &sections,
            None,
            count.unwrap(),
            codec,
            crate::index::PermSet::ALL,
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

    /// The two dictionary writers must agree on the **separator keys**, not
    /// just on everything else. `finish_chunked` and
    /// `file::encode_chunked_dict_section` compute the previous chunk's last
    /// term from different sources — a spill file read back in slices, versus
    /// one contiguous buffer — so this is the place they can drift.
    ///
    /// The literals here are long enough that the object-only section really
    /// splits into chunks; a short-IRI graph would compare two one-chunk
    /// directories and prove nothing.
    #[test]
    fn separator_keyed_dictionary_is_byte_identical_across_writers() {
        let blob = "w".repeat(1200);
        let quads: Vec<RawQuad> = (0..900usize)
            .map(|i| {
                (
                    format!("<http://ex/s{i:04}>"),
                    "<http://ex/note>".to_string(),
                    // half share a long prefix (long separators), half diverge
                    // in their first bytes (one-byte separators)
                    if i % 2 == 0 {
                        format!("\"{blob}{i:04}\"")
                    } else {
                        format!("\"{i:04}{blob}\"")
                    },
                    None,
                )
            })
            .collect();

        let reference = build_reference(quads.clone());
        let (bytes, _) = build_ext(quads, 0);
        assert_eq!(
            bytes, reference,
            "the external builder's chunk directory drifted from the in-RAM one"
        );

        // …and the directory they agree on really is multi-chunk and keyed by
        // separators, not by the 1,206-byte literals it used to copy.
        let keys = crate::file::dict_chunk_keys_for_test(&bytes);
        let obj = &keys[2]; // section 2 = object-only, the one with the literals
        assert!(
            obj.len() > 4,
            "object-only section has {} chunks",
            obj.len()
        );
        assert!(obj[0].is_empty(), "chunk 0 must carry no separator");
        let longest = obj.iter().map(Vec::len).max().unwrap();
        assert!(
            longest < 1_200,
            "longest key is {longest} B — that is a stored term, not a separator"
        );

        // Every object term still resolves: the routing works in the merged file.
        let rete = crate::Rete::open(&bytes).unwrap();
        for i in (0..900usize).step_by(7) {
            let o = if i % 2 == 0 {
                format!("\"{blob}{i:04}\"")
            } else {
                format!("\"{i:04}{blob}\"")
            };
            assert!(
                rete.dictionary().object_id(&o).is_some(),
                "object {i} lost by the chunk directory"
            );
        }
    }

    /// …and the same, with the terms spread across **named graphs**. The graph
    /// column changes which chunk a term is first seen in, which changes what
    /// the per-chunk dictionaries look like — so the separator directory the two
    /// writers agree on has to survive quads, not just triples. Same shape as
    /// `separator_keyed_dictionary_is_byte_identical_across_writers` (#222),
    /// with the literals distributed over 30 graphs.
    #[test]
    fn separator_keyed_dictionary_survives_named_graphs() {
        let blob = "w".repeat(1200);
        let quads: Vec<RawQuad> = (0..900usize)
            .map(|i| {
                (
                    format!("<http://ex/s{i:04}>"),
                    "<http://ex/note>".to_string(),
                    if i % 2 == 0 {
                        format!("\"{blob}{i:04}\"")
                    } else {
                        format!("\"{i:04}{blob}\"")
                    },
                    // graph names generated out of order, so the graph merge has
                    // to sort them and the ordinals are not the insertion order
                    Some(format!("<http://ex/g{:02}>", (i * 7) % 30)),
                )
            })
            .collect();

        let reference = build_reference(quads.clone());
        let (bytes, stats) = build_ext(quads, 0);
        assert_eq!(
            bytes, reference,
            "the external builder's chunk directory drifted from the in-RAM one \
             once named graphs were in the input"
        );
        assert_eq!(stats.named_graphs, 30);

        let keys = crate::file::dict_chunk_keys_for_test(&bytes);
        let obj = &keys[2]; // section 2 = object-only, the one with the literals
        assert!(
            obj.len() > 4,
            "object-only section has {} chunks",
            obj.len()
        );
        assert!(obj[0].is_empty(), "chunk 0 must carry no separator");
        let longest = obj.iter().map(Vec::len).max().unwrap();
        assert!(
            longest < 1_200,
            "longest key is {longest} B — that is a stored term, not a separator"
        );
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
    ///
    /// **Default graph only.** A spill from a build that had named graphs also
    /// carries `global.qtri` and `g.graphs`, and this harness ignores both — it
    /// would resume as a file with the named graphs silently dropped. It is
    /// checked rather than commented: `global.qtri` present is a hard stop.
    #[test]
    #[ignore = "operational tool, driven by RETE_RESUME_* env vars"]
    fn resume_from_spill() {
        let spill = PathBuf::from(std::env::var("RETE_RESUME_SPILL").expect("RETE_RESUME_SPILL"));
        assert!(
            !spill.join("global.qtri").exists(),
            "this spill has named quads (global.qtri); resuming it here would \
             write a file with the named graphs silently missing — rerun the \
             build instead"
        );
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
            graph_names: None,
            graph_count: 0,
        };
        let tmp = TmpDir::from_build_temp(BuildTemp::adopt_existing_for_resume(spill.clone()));
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
                build_permutation_section(&tmp, &global_tri, perm, run_len, codec, "").unwrap();
            assert_eq!(n, quad_count, "permutation count must match");
            eprintln!("resume: {} done", perm.name());
            sections.push(s);
        }
        write_final_file(
            &out,
            &metadata,
            &[],
            &merged,
            &sections,
            None,
            quad_count,
            codec,
            crate::index::PermSet::ALL,
        )
        .unwrap();
        eprintln!("resume: wrote {}", out.display());
        std::mem::forget(tmp); // keep the spill until the file is verified
    }

    /// A faithful reduction of the one published named-graph dataset
    /// (`switzerland-fedlex`: 56.3 M quads across 497,905 graphs, most of them
    /// tiny). Includes the three awkward shapes on purpose:
    ///   * default-graph triples living alongside named ones,
    ///   * the same triple asserted in two different graphs,
    ///   * a graph holding exactly one statement.
    fn fedlex_shaped_quads(graphs: usize, per_graph: usize) -> Vec<RawQuad> {
        let mut quads: Vec<RawQuad> = Vec::new();
        // the ontology triples fedlex keeps in the default graph
        for i in 0..40usize {
            quads.push((
                format!("<http://data.europa.eu/eli/ontology#p{i}>"),
                "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>".to_string(),
                "<http://www.w3.org/2002/07/owl#FunctionalProperty>".to_string(),
                None,
            ));
        }
        for g in 0..graphs {
            // the graph names are NOT generated in sorted order — the k-way
            // merge really has to order them
            let name = format!(
                "<https://fedlex.data.admin.ch/eli/cc/{:05}/graph>",
                g * 7 % 9973
            );
            let n = if g % 11 == 0 { 1 } else { per_graph };
            for i in 0..n {
                quads.push((
                    format!("<https://fedlex.data.admin.ch/eli/cc/{g}/art_{i}>"),
                    format!("<http://data.europa.eu/eli/ontology#pred{}>", i % 6),
                    match i % 4 {
                        0 => format!("<https://fedlex.data.admin.ch/eli/cc/{g}>"),
                        1 => format!("\"Artikel {i}\"@de"),
                        2 => format!("\"{i}\"^^<http://www.w3.org/2001/XMLSchema#integer>"),
                        _ => format!("_:b{}", i % 13),
                    },
                    Some(name.clone()),
                ));
            }
            // the SAME triple in a second graph — must survive in both
            quads.push((
                "<https://fedlex.data.admin.ch/eli/cc/shared>".to_string(),
                "<http://data.europa.eu/eli/ontology#in_force>".to_string(),
                "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>".to_string(),
                Some(name.clone()),
            ));
            // …and in the default graph too
            if g == 0 {
                quads.push((
                    "<https://fedlex.data.admin.ch/eli/cc/shared>".to_string(),
                    "<http://data.europa.eu/eli/ontology#in_force>".to_string(),
                    "\"true\"^^<http://www.w3.org/2001/XMLSchema#boolean>".to_string(),
                    None,
                ));
            }
        }
        // duplicates, far apart, so dedup has something to do in both builders
        for i in 0..(quads.len() / 20) {
            let j = i * 17 % quads.len();
            quads.push(quads[j].clone());
        }
        quads
    }

    /// The acceptance criterion from #139: the same named-graph input built both
    /// ways must be **byte-identical**, not merely equivalent.
    #[test]
    fn named_graph_external_build_is_byte_identical() {
        let quads = fedlex_shaped_quads(60, 9);
        let reference = build_reference(quads.clone());
        let (bytes, stats) = build_ext(quads.clone(), 0);
        assert_eq!(
            bytes, reference,
            "named-graph external build must be byte-identical"
        );

        let header = crate::Header::from_bytes(&bytes).unwrap();
        assert_eq!(stats.statements as u64, header.quad_count);
        assert!(
            stats.statements < quads.len(),
            "fixture must contain duplicates for this to mean anything"
        );
        assert!(stats.named_graphs > 1);
        assert!(stats.default_triples > 0, "default graph must be populated");

        // …and the file really carries the graphs, not just the same bytes.
        let rete = crate::Rete::open(&bytes).unwrap();
        assert_eq!(rete.named_graph_count(), stats.named_graphs);
    }

    /// Many chunks + many sort runs + named graphs at once: the graph ordinals
    /// are merged across chunk-local rankings, so a graph first seen in chunk 7
    /// must land in the same slot as the same graph seen in chunk 0.
    #[test]
    fn multi_chunk_named_graph_build_is_byte_identical() {
        let quads = fedlex_shaped_quads(80, 7);
        let reference = build_reference(quads.clone());

        let dir = std::env::temp_dir().join(format!("rete-extbuild-mcng-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let tmp = TmpDir::create(&dir).unwrap();
        let mut chunker = Chunker::new_tmp(&tmp, 4 * 1024); // ~4 KiB chunks
        for q in quads.iter().cloned() {
            chunker.push(q).unwrap();
        }
        let chunks = chunker.finish().unwrap();
        assert!(
            chunks.len() >= 8,
            "expected many chunks, got {}",
            chunks.len()
        );
        let bytes = finish_from_chunks(&tmp, &chunks, &out, 256, 4).unwrap();
        assert_eq!(
            bytes, reference,
            "multi-chunk named-graph build must be byte-identical"
        );
        drop(tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A graph too large for the budget takes the external per-permutation path
    /// instead of `GraphIndexBuilder`. Both must produce the same container, so
    /// force the spill with a 4-triple cap and compare against the in-RAM build.
    #[test]
    fn oversized_named_graph_spills_and_stays_identical() {
        let mut quads: Vec<RawQuad> = Vec::new();
        for i in 0..4000usize {
            quads.push((
                format!("<http://ex/s{}>", i % 400),
                "<http://ex/cites>".to_string(),
                format!("<http://ex/o{i}>"),
                Some("<http://ex/big>".to_string()),
            ));
        }
        for i in 0..30usize {
            quads.push((
                format!("<http://ex/s{i}>"),
                "<http://ex/p>".to_string(),
                format!("\"lit {i}\""),
                Some("<http://ex/tiny>".to_string()),
            ));
        }
        quads.push((
            "<http://ex/d>".into(),
            "<http://ex/p>".into(),
            "<http://ex/o>".into(),
            None,
        ));
        let reference = build_reference(quads.clone());

        let dir = std::env::temp_dir().join(format!("rete-extbuild-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let tmp = TmpDir::create(&dir).unwrap();
        let mut chunker = Chunker::new_tmp(&tmp, 64 * 1024);
        for q in quads.iter().cloned() {
            chunker.push(q).unwrap();
        }
        let chunks = chunker.finish().unwrap();
        // ram_triples = 4 forces `<http://ex/big>` (4,000 quads) down the spill
        // path while `<http://ex/tiny>` stays resident — both in one file.
        let bytes = finish_from_chunks(&tmp, &chunks, &out, 512, 4).unwrap();
        assert_eq!(
            bytes, reference,
            "a spilled named graph must encode identically to an in-RAM one"
        );
        drop(tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Named graphs with an EMPTY default graph: the default index is still
    /// written (zero triples) and the header still says quads.
    #[test]
    fn named_only_build_is_byte_identical() {
        let quads: Vec<RawQuad> = (0..50usize)
            .map(|i| {
                (
                    format!("<http://ex/s{i}>"),
                    "<http://ex/p>".to_string(),
                    format!("\"v{i}\""),
                    Some(format!("<http://ex/g{}>", i % 5)),
                )
            })
            .collect();
        let reference = build_reference(quads.clone());
        let (bytes, stats) = build_ext(quads, 0);
        assert_eq!(bytes, reference, "named-only build must be byte-identical");
        assert_eq!(stats.default_triples, 0);
        assert_eq!(stats.named_graphs, 5);
        let rete = crate::Rete::open(&bytes).unwrap();
        assert_eq!(rete.named_graph_count(), 5);
    }

    /// Named graphs survive the trip: every distinct quad comes back out of the
    /// file, in the right graph, and `GRAPH ?g` still discriminates.
    #[test]
    fn named_graph_output_is_queryable() {
        let quads = fedlex_shaped_quads(12, 5);
        let (bytes, _) = build_ext(quads.clone(), 0);
        let rete = crate::Rete::open(&bytes).unwrap();

        type Quad = (String, String, String, Option<String>);
        let expected: std::collections::BTreeSet<Quad> = quads.iter().cloned().collect();
        let mut got: std::collections::BTreeSet<Quad> = std::collections::BTreeSet::new();
        for (s, p, o) in rete.dump(None) {
            got.insert((s, p, o, None));
        }
        let names: Vec<String> = rete.graph_names().iter().map(|g| g.to_string()).collect();
        for name in &names {
            for (s, p, o) in rete.dump(Some(name)) {
                got.insert((s, p, o, Some(name.clone())));
            }
        }
        assert_eq!(
            got, expected,
            "quads lost or misfiled by the external build"
        );

        let out = crate::eval_query(
            &rete,
            "SELECT (COUNT(DISTINCT ?g) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }",
        )
        .unwrap();
        match out {
            crate::QueryOutput::Select(_, rows) => assert_eq!(rows.len(), 1),
            other => panic!("expected select, got {other:?}"),
        }
    }

    /// Drive phases 2-5 from an already-chunked spill with test-sized run and
    /// residency caps, returning the finished file image.
    fn finish_from_chunks(
        tmp: &TmpDir,
        chunks: &[ChunkInfo],
        out: &Path,
        run_len: usize,
        ram_triples: usize,
    ) -> Result<Vec<u8>, ExtBuildError> {
        let mut merged = merge_dictionaries(tmp, chunks)?;
        let remaps = std::mem::take(&mut merged.remaps);
        let has_named = chunks.iter().any(|c| c.quad_count > 0);
        let global_tri = tmp.path("global.tri")?;
        let global_qtri = tmp.path("global.qtri")?;
        {
            let mut w = BufWriter::new(File::create(&global_tri)?);
            let mut qw = if has_named {
                Some(BufWriter::new(File::create(&global_qtri)?))
            } else {
                None
            };
            for (ci, chunk) in chunks.iter().enumerate() {
                let maps = &remaps[ci];
                let mut rd = BufReader::new(File::open(tmp.path(&format!("c{ci}.tri"))?)?);
                let mut buf = [0u8; 12];
                while rd.read_exact(&mut buf).is_ok() {
                    let s = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                    let p = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                    let o = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                    w.write_all(&maps.subj[(s - 1) as usize].to_le_bytes())?;
                    w.write_all(&maps.pred[(p - 1) as usize].to_le_bytes())?;
                    w.write_all(&maps.obj[(o - 1) as usize].to_le_bytes())?;
                }
                if chunk.quad_count > 0 {
                    let qw = qw.as_mut().unwrap();
                    let mut rd = BufReader::new(File::open(tmp.path(&format!("c{ci}.qtri"))?)?);
                    let mut buf = [0u8; 16];
                    while rd.read_exact(&mut buf).is_ok() {
                        let g = u32::from_le_bytes(buf[0..4].try_into().unwrap());
                        let s = u32::from_le_bytes(buf[4..8].try_into().unwrap());
                        let p = u32::from_le_bytes(buf[8..12].try_into().unwrap());
                        let o = u32::from_le_bytes(buf[12..16].try_into().unwrap());
                        qw.write_all(&maps.graph[(g - 1) as usize].to_le_bytes())?;
                        qw.write_all(&maps.subj[(s - 1) as usize].to_le_bytes())?;
                        qw.write_all(&maps.pred[(p - 1) as usize].to_le_bytes())?;
                        qw.write_all(&maps.obj[(o - 1) as usize].to_le_bytes())?;
                    }
                }
            }
            w.flush()?;
            if let Some(mut q) = qw {
                q.flush()?;
            }
        }
        drop(remaps);

        let codec = crate::file::writer_codec();
        let perms = crate::index::PermSet::ALL;
        let mut sections = Vec::new();
        let mut default_count = 0u64;
        for perm in perms.iter() {
            let (sec, n) = build_permutation_section(tmp, &global_tri, perm, run_len, codec, "")?;
            default_count = n;
            sections.push(sec);
        }
        let named = if has_named {
            Some(build_named_graphs(
                tmp,
                &global_qtri,
                merged.graph_names.as_deref().unwrap(),
                merged.graph_count,
                run_len,
                run_len,
                ram_triples,
                perms,
                codec,
            )?)
        } else {
            None
        };
        let quads = default_count + named.as_ref().map(|n| n.triples).unwrap_or(0);
        write_final_file(
            out,
            &[],
            &[],
            &merged,
            &sections,
            named.as_ref(),
            quads,
            codec,
            perms,
        )?;
        Ok(std::fs::read(out)?)
    }
}
