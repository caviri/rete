//! **External (chunked, memory-bounded) build**: assemble an arbitrarily large
//! single `.rete` file inside a caller-chosen memory budget by spilling sorted
//! intermediate artifacts to disk and merging them, instead of holding the whole
//! dictionary + id-triples + permutations in RAM (SPEC-compatible output;
//! byte-identical to [`crate::ingest::assemble_dataset_streaming_algo`] with the
//! pyramid disabled, text index included or not).
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
//!           → per-chunk id remap tables (local id → global id), appended to
//!             disk as the merge reaches each local id (`extbuild_remap`)
//!           → k-way merge of the graph-name files → global graph ordinals
//! remap:  one chunk's remap tables loaded at a time:
//!         its default triples remapped to global ids → global.tri
//!         its named quads remapped → global.qtri (graph ordinal first)
//! index:  per permutation: budget-sized sorted runs → k-way merge (dedup)
//!           → streaming tiler → independently compressed tiles on disk
//! named:  external sort of global.qtri by (g, s, p, o) — so every graph is one
//!           CONTIGUOUS RUN — then one index container per graph, in graph-name
//!           order, appended to a single spill file
//! text:   (`--text-index` only) each chunk's literal words → c<i>.tok, remapped
//!           with the same tables → global.tok, external sort by (token,
//!           subject) → the TEXT_INDEX section (`crate::text_index_spill`)
//! write:  header | metadata | dict | index | text index | named graphs |
//!           footer, hashed incrementally; the spilled sections are deleted
//! labels: (`--label-index`) the remap tees (object, slot, subject) ids; once
//!           the file is written its own dictionary names them, and the
//!           LABEL_INDEX and SCHEMA sections are appended (`extbuild_sections`)
//! card:   (the CLI: `build --memory-budget-mb --card…`, `merge`) the full card is
//!           derived by streaming the finished file's sorted sections and
//!           appended after them (`crate::recard::plan_card_append`); this
//!           module then writes no metadata at all
//! ```
//!
//! Spill: the merge deletes each chunk's term files once it has read them,
//! the remap each chunk's statements and remap tables once rewritten. The
//! peak is at the end of the merge, while every chunk's statements are on
//! disk; nothing of the label pass is there yet (up to 0.4.0 its string
//! records were, +20% on a data-only graph).
//!
//! Peak RAM ≈ max(one chunk's working set, the merge's read and write buffers,
//! one chunk's remap tables, one sort run, one named graph's index) — all
//! sized from `memory_budget`, never from the dataset:
//!
//! - a chunk is sealed at half the budget; its remap tables, 4 B per local id,
//!   are at most a quarter of that (`extbuild_remap`);
//! - the merge reads every chunk's term files at once and appends to every
//!   chunk's remap files at once, so those buffers shrink as the chunk count
//!   grows: an eighth and a sixteenth of the budget, floored at 64 KiB per
//!   reader and 4 KiB per writer;
//! - a dictionary section's restart offsets (8 B per 16 terms) and chunk
//!   directory are spilled too, and its header streamed from them;
//! - a sort run holds a quarter of the budget in records (the text index's
//!   vectors may grow to half);
//! - what does grow with the data are the tile directories, about 30 B per
//!   64 KiB tile — tens of MB at billions of statements.
//!
//! Up to 0.4.0 every chunk's remap tables stayed resident from the merge to
//! the end of the remap, 4 B per chunk-local id: 15.0 GB at 1.73 B chunk-local
//! terms, which capped one OpenCitations Meta build at half the graph.
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
//! ## Quad index (`GraphLayout::Quad`)
//!
//! Graph names **are** dictionary terms there (subject role), so the chunker
//! observes each name into its chunk dictionary and spills a named quad as
//! `(local subject id of g, s, p, o)`: no name file, no graph-ordinal merge;
//! the remap phase maps `g` like `s`. Then, per graph-leading order, one
//! external sort of 16-byte `(g, a, b, c)` records, a k-way merge with dedup,
//! and the same `quad::QuadTiler` the in-RAM builder uses — so the two write
//! identical bytes. The `G+SPO` pass also emits the graph directory. The
//! section is assembled in a spill file (its digest goes into the index
//! container's seal) and streamed after the index. Peak RAM: one sort run plus
//! one tile batch.
//!
//!
//! Limits (clear errors, not silent degradation): no community pyramid.
//! SPARQL/joins/verify are unaffected (they never require the pyramid). The
//! full-text index is supported, in any scope, and its section is byte-identical
//! to the in-RAM build's.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::build_trace::{self as trace, note_merge, Phase};
use crate::dict::env_restart_interval;

use crate::extbuild_remap::{
    remap_buf_bytes, role_only_id, shared_id, RemapStore, RemapWriter, Seg, SegCounts,
};
use crate::extbuild_sections::{ExtLabels, ExtSchema};
use crate::index::GroupSizer;
use crate::ingest::{BuildStats, GraphLayout, IngestError, RawQuad};
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
    /// Write the LABEL_INDEX section (kind 8). See `extbuild_sections.rs`.
    pub label_index: bool,
    /// Write the SCHEMA section (kind 9). See `extbuild_sections.rs`.
    pub schema: bool,
    /// The schema's type predicate; `rdf:type` when `None`.
    pub type_override: Option<String>,
    /// Build the full-text index over the literals of this scope (`None` = no
    /// TEXT_INDEX section). A second external sort, on the literal words, inside
    /// the same budget; see `text_index_spill`.
    pub text_index: Option<crate::text_index::TextIndexScope>,
    /// How the named graphs are stored ([`GraphLayout::PerGraph`] by default).
    /// [`GraphLayout::Quad`] writes the quad index (SPEC.md §6.5) through its
    /// own external sorts — see "Quad index" in the module docs.
    pub graphs: GraphLayout,
}

impl Default for ExternalBuildOptions {
    fn default() -> Self {
        Self {
            memory_budget: 4 << 30,
            tmp_dir: None,
            metadata: Box::new(|_| Vec::new()),
            build_info: Vec::new(),
            perms: crate::index::PermSet::ALL,
            label_index: false,
            schema: false,
            type_override: None,
            text_index: None,
            graphs: GraphLayout::PerGraph,
        }
    }
}

#[cfg(test)]
thread_local! {
    /// `(chunk budget, text-index run bytes)` for tests that drive a small
    /// graph through [`build_external`] as many chunks and many sort runs —
    /// the public budget floor would otherwise make it one of each.
    pub(crate) static TEST_SPLIT: std::cell::Cell<Option<(u64, usize)>> = const { std::cell::Cell::new(None) };
}

#[derive(Debug, thiserror::Error)]
pub enum ExtBuildError {
    #[error("ingest: {0}")]
    Ingest(#[from] IngestError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal: {0}")]
    Internal(&'static str),
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
const CHUNK_BUDGET_FRACTION: f64 = 0.5;
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
    let tmp_parent = opts
        .tmp_dir
        .clone()
        .or_else(|| output.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let tmp = TmpDir::create(&tmp_parent)?;
    let budget = opts.memory_budget.max(64 << 20); // floor: 64 MiB

    // ---- Phase 1: chunk the input ------------------------------------------
    let chunk_budget = (budget as f64 * CHUNK_BUDGET_FRACTION) as u64;
    // A text-index sort run holds at most a quarter of the budget in records;
    // its vectors may grow to twice that, which is the half the phase may use.
    let text_run_bytes = (budget / 4) as usize;
    #[cfg(test)]
    let (chunk_budget, text_run_bytes) = TEST_SPLIT
        .with(|c| c.get())
        .unwrap_or((chunk_budget, text_run_bytes));
    eprintln!(
        "extbuild: budget {} MiB -> chunk target {} MiB",
        budget >> 20,
        chunk_budget >> 20
    );
    let quad_layout = opts.graphs == GraphLayout::Quad;
    if quad_layout {
        crate::quad::check_perms(opts.perms).map_err(|_| {
            ExtBuildError::Internal("the quad index needs the 3- or 6-permutation set")
        })?;
    }
    let mut text = opts
        .text_index
        .clone()
        .map(crate::text_index_spill::TextSpill::new);
    let mut chunker = Chunker::new(&tmp, chunk_budget, opts.graphs);
    chunker.text = text.as_mut();
    // LABEL_INDEX / SCHEMA (kinds 8, 9): the passes live in extbuild_sections;
    // each phase below calls one hook. Both work on global ids from the remap
    // on, so nothing of them is spilled beside the chunk files. A sort run
    // gets an eighth of the budget while the remap runs, a quarter once the
    // file is written.
    let mut ext_labels = opts
        .label_index
        .then(|| ExtLabels::new(&tmp.dir, (budget / 8) as usize));
    let mut ext_schema = opts.schema.then(|| {
        ExtSchema::new(
            &tmp.dir,
            (budget / 8) as usize,
            opts.type_override.as_deref(),
        )
    });
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
    trace::add("chunks", chunks.len() as u64);
    note_scratch(&tmp);
    trace::lap(Phase::ParseChunk);

    // ---- Phase 2: merge chunk dictionaries into the global dictionary -------
    let schema_preds = crate::extbuild_sections::schema_predicates(opts.type_override.as_deref());
    let (merged, remaps) = merge_dictionaries_with(&tmp, &chunks, &schema_preds, budget)?;
    if let Some(x) = ext_schema.as_mut() {
        x.set_merge_facts(&merged.schema_facts);
    }
    if let Some(l) = ext_labels.as_mut() {
        l.set_predicates(merged.schema_facts.label_preds.clone());
    }
    note_scratch(&tmp);
    trace::lap(Phase::Dictionary);
    eprintln!(
        "extbuild: merged dictionary — {} term(s)",
        merged.term_count
    );
    if merged.graph_count > 0 {
        eprintln!("extbuild: {} named graph(s)", merged.graph_count);
    }

    // ---- Phase 3: remap chunk triples to global ids --------------------------
    let global_tri = tmp.path("global.tri");
    let global_qtri = tmp.path("global.qtri");
    if let Some(text) = text.as_mut() {
        text.begin_remap(tmp.path("global.tok"))?;
    }
    // The named-quad spill is opened only when the input actually carries
    // named graphs, so a default-graph build writes and sorts exactly the
    // bytes it did before quads existed here.
    remap_chunks(
        &tmp,
        &chunks,
        &remaps,
        quad_layout,
        &global_tri,
        has_named.then_some(global_qtri.as_path()),
        text.as_mut(),
        ext_schema.as_mut(),
        ext_labels.as_mut(),
    )?;
    note_scratch(&tmp);
    trace::lap(Phase::Encode);

    // ---- Phase 3b: the full-text index (`--text-index`) -----------------------
    let codec = crate::file::writer_codec();
    let text_section = match text {
        Some(t) => {
            let s = t.finish(&|name| tmp.path(name), text_run_bytes, codec)?;
            note_scratch(&tmp);
            trace::lap(Phase::TextIndex);
            s
        }
        None => None,
    };

    // ---- Phase 4: per-permutation external sort + streaming tiler -----------
    // A run holds R triples resident twice over during sort (Vec + sort scratch).
    let run_len = ((budget / 2) / 24).clamp(1 << 16, u32::MAX as u64) as usize;
    let mut perm_sections: Vec<SectionFile> = Vec::with_capacity(opts.perms.len());
    let mut deduped_count: Option<u64> = None;
    if let Some(x) = ext_schema.as_mut() {
        x.begin_spo()?;
    }
    for perm in opts.perms.iter() {
        // The schema pass reads the deduplicated SPO and POS merge streams.
        let mut observe = |t: IdTriple| -> std::io::Result<()> {
            match (ext_schema.as_mut(), perm) {
                (Some(x), crate::index::IndexPermutation::Spo) => x.observe_spo(t),
                (Some(x), crate::index::IndexPermutation::Pos) => {
                    x.observe_pos(t);
                    Ok(())
                }
                _ => Ok(()),
            }
        };
        let (section, n) = build_permutation_section_observed(
            &tmp,
            &global_tri,
            perm,
            run_len,
            codec,
            "",
            &mut observe,
        )?;
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
        trace::lap(Phase::IndexPermutations);
    }
    let _ = std::fs::remove_file(&global_tri);
    let default_count = deduped_count.unwrap_or(0);
    if let Some(x) = ext_schema.as_mut() {
        x.finish_links()?;
    }

    // ---- Phase 4b: one index container per named graph ----------------------
    // The graph ordinal leads the sort key, so the merge hands back one
    // contiguous run per graph, in graph-name order — exactly the order the
    // named-graphs section stores them in.
    // Same accounting as `run_len`, at the quad record's 16 bytes rather
    // than the triple's 12: a run holds Q quads resident twice over, so
    // Q·16·2 = budget/2. Reusing `run_len` here would have quietly spent
    // 4/3 of the intended half-budget on the quad sort.
    let quad_run_len = ((budget / 2) / 32).clamp(1 << 16, u32::MAX as u64) as usize;
    // ---- Phase 4b (quad index): graph-leading orders over the quad spill ----
    let quad = if has_named && quad_layout {
        let q = build_quad_section(&tmp, &global_qtri, quad_run_len, opts.perms, codec)?;
        let _ = std::fs::remove_file(&global_qtri);
        eprintln!(
            "extbuild: quad index — {} named graph(s), {} unique quad(s), {} byte(s)",
            q.graph_count, q.quad_count, q.len
        );
        Some(q)
    } else {
        None
    };
    let named = if has_named && !quad_layout {
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
    if has_named {
        trace::lap(Phase::NamedGraphs);
    }
    let named_triples = named.as_ref().map(|n| n.triples).unwrap_or(0)
        + quad.as_ref().map(|q| q.quad_count).unwrap_or(0);
    let named_graphs = named.as_ref().map(|n| n.count).unwrap_or(0)
        + quad.as_ref().map(|q| q.graph_count).unwrap_or(0);
    let quad_count = default_count + named_triples;

    // ---- Phase 5: stream the final file -------------------------------------
    // `statements` is what was INGESTED; `quad_count` is what the permutation
    // merge actually kept after dedup, and it is what the header records. The
    // metadata callback (the Dataset Card) has to see the latter, or a build
    // from input containing duplicates publishes a card that disagrees with its
    // own file. They are equal whenever the input has no duplicates, which is
    // why the gap went unnoticed.
    let mut stats = BuildStats {
        statements: quad_count as usize,
        default_triples: default_count as usize,
        named_graphs: named_graphs as usize,
        terms: merged.term_count as usize,
        pyramid_levels: 0,
    };
    let metadata = (opts.metadata)(&stats);
    if !metadata.is_empty() {
        // A caller that derives its card from the finished file (the CLI's
        // full card, appended after the data) laps the phase itself, later.
        trace::lap(Phase::CardDerive);
    }
    write_final_file(
        output,
        &metadata,
        &opts.build_info,
        &merged,
        &perm_sections,
        text_section.as_ref(),
        named.as_ref(),
        quad.as_ref(),
        quad_count,
        codec,
        opts.perms,
    )?;
    stats.pyramid_levels = 0;
    note_scratch(&tmp);
    trace::lap(Phase::Write);
    // The sections now live in the output: drop their spill copies before the
    // label pass sorts, so the two never stack.
    for p in merged
        .section_files
        .iter()
        .chain(perm_sections.iter())
        .map(|s| &s.path)
        .chain(text_section.iter().flat_map(|t| [&t.table, &t.postings]))
        .chain(named.iter().map(|n| &n.body))
        .chain(quad.iter().map(|q| &q.path))
    {
        let _ = std::fs::remove_file(p);
    }

    // ---- Phase 6: LABEL_INDEX / SCHEMA, appended (outside the content hash) --
    // Both read the strings they name through the written file's dictionary.
    let label_section = match ext_labels {
        Some(l) => {
            let s = l
                .finish(output, (budget / 4) as usize, budget / 8, codec)
                .map_err(|e| ExtBuildError::Io(std::io::Error::other(e.to_string())))?;
            note_scratch(&tmp);
            s
        }
        None => None,
    };
    let schema_section = match ext_schema {
        Some(x) => x
            .finish(output)
            .map_err(|e| ExtBuildError::Io(std::io::Error::other(e.to_string())))?,
        None => Vec::new(),
    };
    if label_section.is_some() || !schema_section.is_empty() {
        use crate::file::PayloadPart;
        use crate::header::SectionKind;
        let mut sections: Vec<(SectionKind, Vec<PayloadPart<'_>>)> = Vec::new();
        if let Some(l) = &label_section {
            sections.push((
                SectionKind::LabelIndex,
                vec![
                    PayloadPart::Bytes(&l.digest_and_head),
                    PayloadPart::File(&l.body),
                ],
            ));
            eprintln!("extbuild: label index — {} entries", l.entries);
        }
        if !schema_section.is_empty() {
            sections.push((
                SectionKind::Schema,
                vec![PayloadPart::Bytes(&schema_section)],
            ));
            eprintln!("extbuild: schema section — {} bytes", schema_section.len());
        }
        crate::file::append_sections_to_file(output, &sections)
            .map_err(|e| ExtBuildError::Io(std::io::Error::other(e.to_string())))?;
        trace::lap(Phase::LabelsSchema);
    }
    Ok(stats)
}

/// Raise the `scratch_peak_bytes` counter to what the spill directory holds
/// now. Sampled at phase boundaries (and once each external sort has written
/// its runs, its high-water mark), so it is a lower bound on the true peak.
/// Skipped — no directory walk — when no build trace is recording.
fn note_scratch(tmp: &TmpDir) {
    if !trace::active() {
        return;
    }
    fn walk(dir: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => walk(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    trace::max("scratch_peak_bytes", walk(&tmp.dir));
}

// ---------------------------------------------------------------------------
// tmp-dir guard
// ---------------------------------------------------------------------------

struct TmpDir {
    dir: PathBuf,
}

impl TmpDir {
    fn create(parent: &Path) -> Result<Self, std::io::Error> {
        // pid + a process-wide counter: two concurrent external builds in the
        // same process (or parallel tests) must never share a spill directory.
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = parent.join(format!(".rete-extbuild-{}-{}", std::process::id(), seq));
        std::fs::create_dir_all(&dir)?;
        Ok(TmpDir { dir })
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
}

impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ---------------------------------------------------------------------------
// Phase 1: chunking
// ---------------------------------------------------------------------------

struct ChunkInfo {
    /// default-graph statements in this chunk
    triple_count: u64,
    /// named-graph statements in this chunk
    quad_count: u64,
    /// term counts per section file, in file order (shared, subj, obj, pred)
    section_terms: [u32; 4],
    /// distinct graph names in this chunk (`c<i>.graph` line count)
    graph_terms: u32,
}

impl ChunkInfo {
    /// Local ids per remap segment ([`Seg`]): the section files' term counts,
    /// then the graph names.
    fn seg_counts(&self) -> SegCounts {
        let [shared, subj, obj, pred] = self.section_terms;
        [shared, subj, obj, pred, self.graph_terms]
    }
}

struct Chunker<'a> {
    tmp: &'a TmpDir,
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
    /// `--text-index`: each sealed chunk spills its literal words here.
    text: Option<&'a mut crate::text_index_spill::TextSpill>,
    /// Quad index: graph names are subject terms of the chunk dictionary and
    /// a named quad's graph id is the name's local subject id.
    quad: bool,
}

impl<'a> Chunker<'a> {
    fn new(tmp: &'a TmpDir, chunk_budget: u64, graphs: GraphLayout) -> Self {
        Chunker {
            tmp,
            chunk_budget,
            acc_bytes: 0,
            quads: Vec::new(),
            named: Vec::new(),
            graph_ids: std::collections::BTreeMap::new(),
            chunks: Vec::new(),
            has_quoted: false,
            text: None,
            quad: graphs == GraphLayout::Quad,
        }
    }

    fn push(&mut self, q: RawQuad) -> Result<(), ExtBuildError> {
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

        // Chunk-local dictionaries are merged by role, so they must classify
        // terms by role alone: frequent-object promotion is an in-memory-build
        // feature (the CLI refuses it with `--memory-budget-mb`).
        let mut db = DictionaryBuilder::new().with_frequent_objects(0);
        for (s, p, o) in &quads {
            db.observe(s, p, o);
        }
        // Graph NAMES are not dictionary terms — the format stores them verbatim
        // in the named-graphs section — but the s/p/o of a named quad are, and
        // the in-RAM builder observes them from the same single pass.
        for (_, s, p, o) in &named {
            db.observe(s, p, o);
        }
        // ...except in the quad index, where they are subject terms.
        if self.quad {
            for name in graph_ids.keys() {
                db.observe_graph(name);
            }
        }
        let dict = db.build();
        if dict.has_quoted_triples() {
            self.has_quoted = true;
        }
        // `--text-index`: the chunk's literal words, keyed by chunk-local subject
        // ids, while the raw strings and their dictionary are both in hand.
        if let Some(text) = self.text.as_mut() {
            text.spill_chunk(&self.tmp.path(&format!("c{ci}.tok")), &dict, &quads, &named)?;
        }

        // local id-triples (subject-space id, predicate id, object-space id)
        let mut tri = BufWriter::new(File::create(self.tmp.path(&format!("c{ci}.tri")))?);
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
        let graphs_seen = graph_ids.len();
        let graph_terms = if self.quad { 0 } else { graph_ids.len() as u32 };
        if !named.is_empty() && self.quad {
            // Quad index: the graph column is the name's local subject id,
            // remapped later exactly like a subject.
            let mut sid_of = vec![0u32; graph_ids.len() + 1];
            for (name, insertion_id) in &graph_ids {
                sid_of[*insertion_id as usize] = dict
                    .subject_id(name)
                    .ok_or(ExtBuildError::Internal("graph name missing from own dict"))?;
            }
            let mut qw = BufWriter::new(File::create(self.tmp.path(&format!("c{ci}.qtri")))?);
            for (gid, s, p, o) in &named {
                let (si, pi, oi) = dict
                    .encode(s, p, o)
                    .ok_or(ExtBuildError::Internal("chunk term missing from own dict"))?;
                qw.write_all(&sid_of[*gid as usize].to_le_bytes())?;
                qw.write_all(&si.to_le_bytes())?;
                qw.write_all(&pi.to_le_bytes())?;
                qw.write_all(&oi.to_le_bytes())?;
            }
            qw.flush()?;
        } else if !named.is_empty() {
            let mut rank_of = vec![0u32; graph_ids.len() + 1];
            let mut gw = BufWriter::new(File::create(self.tmp.path(&format!("c{ci}.graph")))?);
            for (rank, (name, insertion_id)) in graph_ids.iter().enumerate() {
                rank_of[*insertion_id as usize] = rank as u32 + 1;
                gw.write_all(name.as_bytes())?;
                gw.write_all(b"\n")?;
            }
            gw.flush()?;
            let mut qw = BufWriter::new(File::create(self.tmp.path(&format!("c{ci}.qtri")))?);
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
            let mut w = BufWriter::new(File::create(self.tmp.path(name))?);
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
            "extbuild: chunk {ci} sealed — {} statement(s), {} term(s), {graphs_seen} graph(s)",
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

    fn finish(mut self) -> Result<Vec<ChunkInfo>, ExtBuildError> {
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
                File::create(self.tmp.path(name))?;
            }
        }
        Ok(self.chunks)
    }
}

// ---------------------------------------------------------------------------
// Phase 2: global dictionary merge
// ---------------------------------------------------------------------------

struct MergedDict {
    /// The four chunked-encoded dict section payloads, spilled to tmp files
    /// (shared, subjects, objects, predicates — the container order).
    section_files: [SectionFile; 4],
    term_count: u64,
    has_quoted: bool,
    /// Every distinct graph name, ascending, one per line — spilled rather than
    /// resident because fedlex has 497,905 of them. `None` when the input had
    /// no named graphs.
    graph_names: Option<PathBuf>,
    graph_count: u32,
    /// What the SCHEMA pass needs from the merge.
    schema_facts: crate::extbuild_sections::MergeFacts,
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
    /// `None` once the file has been read to its end
    rd: Option<BufReader<File>>,
    buf: String,
    /// deleted at the end of the file (the merge's chunk term files)
    consume: Option<PathBuf>,
}

impl TermFileReader {
    fn open(path: &Path) -> Result<Self, std::io::Error> {
        Ok(TermFileReader {
            rd: Some(BufReader::with_capacity(1 << 20, File::open(path)?)),
            buf: String::new(),
            consume: None,
        })
    }

    /// With a `capacity`-byte read buffer, deleting the file once it has been
    /// read to its end: the dictionary merge opens every chunk's term files at
    /// once, sizes their buffers from the budget, and gives a chunk's space
    /// back as soon as the merge is past its last term rather than when every
    /// chunk is done. (A shared-section file is read twice, by the subject and
    /// the object stream; its space comes back when the second one closes.)
    fn open_consuming(path: &Path, capacity: usize) -> Result<Self, std::io::Error> {
        Ok(TermFileReader {
            rd: Some(BufReader::with_capacity(capacity, File::open(path)?)),
            buf: String::new(),
            consume: Some(path.to_path_buf()),
        })
    }

    fn next(&mut self) -> Result<Option<String>, std::io::Error> {
        let Some(rd) = self.rd.as_mut() else {
            return Ok(None);
        };
        self.buf.clear();
        let n = rd.read_line(&mut self.buf)?;
        if n == 0 {
            self.rd = None; // close before deleting
            if let Some(p) = self.consume.take() {
                let _ = std::fs::remove_file(p);
            }
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
        note_merge(streams.len());
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

#[cfg(test)]
fn merge_dictionaries(
    tmp: &TmpDir,
    chunks: &[ChunkInfo],
) -> Result<(MergedDict, RemapStore), ExtBuildError> {
    merge_dictionaries_with(tmp, chunks, &[], 64 << 20)
}

/// [`merge_dictionaries`], also recording what the SCHEMA pass needs: the
/// global ids of `schema_preds`, the shared-term count and the literal
/// prefixes of the shared and object-only sections. `budget` sizes the
/// buffers of the merge's per-chunk readers and remap writers.
///
/// Returns the merged dictionary and every chunk's id remap tables, which are
/// written to disk while the merge reaches each chunk-local id (see
/// [`crate::extbuild_remap`]): within one remap segment of one chunk the
/// merge meets the local ids in increasing order, so each table is a file of
/// sequential appends.
fn merge_dictionaries_with(
    tmp: &TmpDir,
    chunks: &[ChunkInfo],
    schema_preds: &[String],
    budget: u64,
) -> Result<(MergedDict, RemapStore), ExtBuildError> {
    let k = chunks.len();
    let mut schema_facts = crate::extbuild_sections::MergeFacts {
        pred_ids: vec![None; schema_preds.len()],
        ..Default::default()
    };

    // The node merge reads four term files per chunk at once (shared twice,
    // subject-only, object-only): their buffers share an eighth of the budget.
    let read_buf = ((budget / 8) / (4 * k.max(1) as u64)).clamp(64 << 10, 1 << 20) as usize;
    let counts: Vec<SegCounts> = chunks.iter().map(ChunkInfo::seg_counts).collect();
    let mut remaps = RemapWriter::new(&tmp.dir, &counts, remap_buf_bytes(budget, k));
    // A chunk-local id of the subject or object space at or below this is in
    // the chunk's shared section, the same id in both spaces.
    let shared_of = |ci: usize| chunks[ci].section_terms[0];

    // subject-space + object-space k-way streams
    let mut subj_streams = Vec::with_capacity(k);
    let mut obj_streams = Vec::with_capacity(k);
    for (ci, c) in chunks.iter().enumerate() {
        let open = |suffix: &str| {
            TermFileReader::open_consuming(&tmp.path(&format!("c{ci}.{suffix}")), read_buf)
        };
        // Both readers of the shared file open before either stream reads:
        // a stream deletes its file at the end, and an empty one ends at once.
        let (shared_s, shared_o) = (open("shared")?, open("shared")?);
        subj_streams.push(SpaceStream::new(
            shared_s,
            open("subj")?,
            c.section_terms[0],
        )?);
        obj_streams.push(SpaceStream::new(
            shared_o,
            open("obj")?,
            c.section_terms[0],
        )?);
    }
    let mut subjects = KWayTerms::new(subj_streams)?;
    let mut objects = KWayTerms::new(obj_streams)?;

    // The three node sections are written as raw front-coded bodies first; the
    // final chunked encoding needs term counts, so bodies spill to tmp.
    let mut shared_sec = RawSectionWriter::create(tmp.path("g.shared.raw"))?;
    let mut subj_sec = RawSectionWriter::create(tmp.path("g.subj.raw"))?;
    let mut obj_sec = RawSectionWriter::create(tmp.path("g.obj.raw"))?;
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
                // A shared id is final at once: shared ids lead both spaces.
                let id = shared_id(ranks[0])?;
                for (ci, lid) in s_carriers {
                    match lid.checked_sub(shared_of(ci)) {
                        None | Some(0) => remaps.push(ci, Seg::Shared, lid, id)?,
                        Some(rel) => remaps.push(ci, Seg::SubjOnly, rel, id)?,
                    }
                }
                for (ci, lid) in o_carriers {
                    match lid.checked_sub(shared_of(ci)) {
                        // shared in its chunk too: written from the subject side
                        None | Some(0) => {}
                        Some(rel) => remaps.push(ci, Seg::ObjOnly, rel, id)?,
                    }
                }
                has_quoted |= term.starts_with("<<");
                if term.starts_with('"') {
                    schema_facts.shared_literals += 1;
                }
                shared_sec.push(&term)?;
                ranks[0] += 1;
                s_item = subjects.next()?;
                o_item = objects.next()?;
            }
            Class::SubjOnly => {
                let (term, carriers) = s_item.take().unwrap();
                let id = role_only_id(ranks[1])?;
                for (ci, lid) in carriers {
                    match lid.checked_sub(shared_of(ci)) {
                        Some(rel) if rel > 0 => remaps.push(ci, Seg::SubjOnly, rel, id)?,
                        _ => {
                            return Err(ExtBuildError::Internal(
                                "a chunk-shared term merged as subject-only",
                            ))
                        }
                    }
                }
                has_quoted |= term.starts_with("<<");
                subj_sec.push(&term)?;
                ranks[1] += 1;
                s_item = subjects.next()?;
            }
            Class::ObjOnly => {
                let (term, carriers) = o_item.take().unwrap();
                let id = role_only_id(ranks[2])?;
                for (ci, lid) in carriers {
                    match lid.checked_sub(shared_of(ci)) {
                        Some(rel) if rel > 0 => remaps.push(ci, Seg::ObjOnly, rel, id)?,
                        _ => {
                            return Err(ExtBuildError::Internal(
                                "a chunk-shared term merged as object-only",
                            ))
                        }
                    }
                }
                has_quoted |= term.starts_with("<<");
                if term.starts_with('"') {
                    schema_facts.objonly_literals += 1;
                }
                obj_sec.push(&term)?;
                ranks[2] += 1;
                o_item = objects.next()?;
            }
        }
    }
    // The node merge is over: release its readers and write buffers.
    drop((subjects, objects));
    for seg in [Seg::Shared, Seg::SubjOnly, Seg::ObjOnly] {
        remaps.flush(seg)?;
    }
    let (n_shared, n_subj, n_obj) = (ranks[0], ranks[1], ranks[2]);
    if n_shared.checked_add(n_subj.max(n_obj)).is_none() {
        return Err(ExtBuildError::Internal(
            "more than 2^32 subject or object terms: the dictionary is too large for one file",
        ));
    }
    schema_facts.n_shared = n_shared;

    // predicates: plain k-way merge over the pred term files
    let mut pred_sec = RawSectionWriter::create(tmp.path("g.pred.raw"))?;
    {
        let mut streams = Vec::with_capacity(k);
        for (ci, c) in chunks.iter().enumerate() {
            // real pred reader + empty role-only reader => a plain single stream
            let empty = tmp.path(&format!("c{ci}.pred.empty"));
            File::create(&empty)?;
            streams.push(SpaceStream::new(
                TermFileReader::open_consuming(&tmp.path(&format!("c{ci}.pred")), read_buf)?,
                TermFileReader::open_consuming(&empty, read_buf)?,
                c.section_terms[3],
            )?);
        }
        let mut kway = KWayTerms::new(streams)?;
        let mut rank = 0u32;
        while let Some((term, carriers)) = kway.next()? {
            for (ci, lid) in carriers {
                remaps.push(ci, Seg::Pred, lid, rank + 1)?; // pred ids are final now
            }
            if let Some(i) = schema_preds.iter().position(|w| *w == term) {
                schema_facts.pred_ids[i] = Some(rank + 1);
            }
            if let Some(slot) = crate::label_index::slot_of_predicate(&term) {
                schema_facts.label_preds.push((rank + 1, slot));
            }
            pred_sec.push(&term)?;
            rank += 1;
        }
    }
    remaps.flush(Seg::Pred)?;

    // graph names: the same k-way merge, into a plain sorted name list rather
    // than a front-coded dict section — the format stores graph names verbatim.
    let mut graph_count = 0u32;
    let graph_names = if chunks.iter().any(|c| c.graph_terms > 0) {
        let path = tmp.path("g.graphs");
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        let mut streams = Vec::with_capacity(k);
        for (ci, c) in chunks.iter().enumerate() {
            let real = tmp.path(&format!("c{ci}.graph"));
            if !real.exists() {
                File::create(&real)?;
            }
            let empty = tmp.path(&format!("c{ci}.graph.empty"));
            File::create(&empty)?;
            streams.push(SpaceStream::new(
                TermFileReader::open_consuming(&real, read_buf)?,
                TermFileReader::open_consuming(&empty, read_buf)?,
                c.graph_terms,
            )?);
        }
        let mut kway = KWayTerms::new(streams)?;
        while let Some((name, carriers)) = kway.next()? {
            graph_count += 1;
            for (ci, lid) in carriers {
                remaps.push(ci, Seg::Graph, lid, graph_count)?;
            }
            w.write_all(name.as_bytes())?;
            w.write_all(b"\n")?;
        }
        w.flush()?;
        Some(path)
    } else {
        None
    };
    // Subject- and object-only ids stay provisional until `n_shared` is
    // known: they are fixed up as each chunk's tables are loaded.
    let remaps = remaps.finish(n_shared)?;

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
            let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.{suffix}")));
        }
    }

    let term_count =
        n_shared as u64 + n_subj as u64 + n_obj as u64 + section_files[3].term_count as u64;

    Ok((
        MergedDict {
            section_files: section_files.map(|s| s.file),
            term_count,
            has_quoted,
            graph_names,
            graph_count,
            schema_facts,
        },
        remaps,
    ))
}

/// A raw front-coded dict-section body being streamed to a tmp file, exactly as
/// [`crate::dict::DictSectionBuilder::build`] would lay it out — plus the
/// restart-offset bookkeeping needed to emit the header and (later) the chunked
/// encoding without ever holding the body in RAM.
///
/// The restart offsets go to disk too (8 B per `restart_interval` terms: a
/// section of a billion terms has 62.5 M of them), and so does the chunk
/// directory [`finish_chunked`](Self::finish_chunked) builds from them, so a
/// section of any size costs this writer one 64 KiB chunk of RAM.
struct RawSectionWriter {
    w: BufWriter<File>,
    path: PathBuf,
    prev: String,
    n: u64,
    body_len: u64,
    /// every restart offset, `u64` little-endian, in order
    restarts: BufWriter<File>,
    restarts_path: PathBuf,
    n_restarts: u64,
    /// the restart offsets' varint bytes, summed: the header's length,
    /// known without reading them back
    restart_varint_bytes: u64,
}

struct FinishedSection {
    file: SectionFile,
    term_count: u32,
}

/// Reads back the `u64` restart offsets a [`RawSectionWriter`] spilled.
struct OffsetReader {
    rd: BufReader<File>,
}

impl OffsetReader {
    fn open(path: &Path) -> Result<Self, std::io::Error> {
        Ok(OffsetReader {
            rd: BufReader::with_capacity(1 << 16, File::open(path)?),
        })
    }

    fn next(&mut self) -> Result<Option<u64>, std::io::Error> {
        let mut b = [0u8; 8];
        match self.rd.read_exact(&mut b) {
            Ok(()) => Ok(Some(u64::from_le_bytes(b))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl RawSectionWriter {
    fn create(path: PathBuf) -> Result<Self, std::io::Error> {
        let restarts_path = path.with_extension("restarts");
        Ok(RawSectionWriter {
            w: BufWriter::with_capacity(1 << 20, File::create(&path)?),
            path,
            prev: String::new(),
            n: 0,
            body_len: 0,
            restarts: BufWriter::with_capacity(1 << 16, File::create(&restarts_path)?),
            restarts_path,
            n_restarts: 0,
            restart_varint_bytes: 0,
        })
    }

    fn push(&mut self, term: &str) -> Result<(), std::io::Error> {
        let r = env_restart_interval() as u64;
        let mut entry = Vec::with_capacity(term.len() + 8);
        if self.n.is_multiple_of(r) {
            self.restarts.write_all(&self.body_len.to_le_bytes())?;
            self.n_restarts += 1;
            self.restart_varint_bytes += uvarint_len(self.body_len);
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
    /// tmp file, deleting the raw body. Streams: the restart offsets are read
    /// back once to cut the chunks and once more to write the header, and the
    /// chunk directory is spilled as it is built.
    fn finish_chunked(
        mut self,
        tmp: &TmpDir,
        out_name: &str,
        codec: u8,
    ) -> Result<FinishedSection, ExtBuildError> {
        self.w.flush()?;
        drop(self.w);
        self.restarts.flush()?;
        drop(self.restarts);
        let term_count = self.n as u32;
        let interval = env_restart_interval() as u64;

        // the raw header exactly as DictSectionBuilder writes it:
        // [n][restart interval][restart count][restart offsets…]
        let header_len = uvarint_len(self.n)
            + uvarint_len(interval)
            + uvarint_len(self.n_restarts)
            + self.restart_varint_bytes;

        // Chunk bounds over the body by run offsets (whole runs, 64 KiB
        // budget), exactly as `encode_chunked_dict_section` cuts them: a chunk
        // starts at a run and takes the following runs while they start less
        // than the budget past its own start.
        let budget = crate::write_opts::dict_chunk_budget() as u64;
        let mut offs = OffsetReader::open(&self.restarts_path)?;
        let mut body = File::open(&self.path)?;
        // compressed chunks and the encoded directory, each spilled
        let comp_path = tmp.path(&format!("{out_name}.chunks"));
        let mut comp_out = BufWriter::with_capacity(1 << 20, File::create(&comp_path)?);
        let dir_path = tmp.path(&format!("{out_name}.dir"));
        let mut dir_out = BufWriter::with_capacity(1 << 16, File::create(&dir_path)?);
        let mut dir_len = 0u64;
        let mut num_chunks = 0u64;
        let mut prev_run = 0u64;
        // The previous chunk's last term — the separator's lower bound. Kept in
        // lockstep with `encode_chunked_dict_section`; the two writers must emit
        // byte-identical directories for the same input.
        let mut prev_last: Option<Vec<u8>> = None;
        let mut entry = Vec::new();
        // `next` is the offset of run `run`, the first run not yet in a chunk
        let mut run = 0u64;
        let mut next = offs.next()?;
        while let Some(start) = next {
            let first_run = run;
            let mut last_run_start = start;
            run += 1;
            next = offs.next()?;
            while let Some(o) = next {
                if o - start >= budget {
                    break;
                }
                last_run_start = o;
                run += 1;
                next = offs.next()?;
            }
            let end = next.unwrap_or(self.body_len);
            body.seek(SeekFrom::Start(start))?;
            let mut raw = vec![0u8; (end - start) as usize];
            body.read_exact(&mut raw)?;
            // the chunk's first run entry is a restart: [0][len][full term]
            let first_term = read_restart_term(&raw)
                .ok_or(ExtBuildError::Internal("restart entry unreadable"))?;
            // Routing key = the shortest separator, not the first term; chunk 0
            // needs none. See `crate::dict::SectionChunk::key`.
            let key = if num_chunks == 0 {
                Vec::new()
            } else {
                match &prev_last {
                    None => first_term,
                    Some(pl) => crate::dict::shortest_separator(pl, &first_term),
                }
            };
            if next.is_some() {
                // This chunk's last run is the one before the next chunk's
                // first; `raw` already holds the bytes, so decoding it is free
                // of I/O.
                let last_run_rel = (last_run_start - start) as usize;
                prev_last = crate::dict::run_last_term(&raw, last_run_rel, raw.len());
            }
            let comp = crate::file::compress(codec, &raw);
            comp_out.write_all(&comp)?;
            entry.clear();
            write_uvarint(&mut entry, first_run - prev_run);
            write_uvarint(&mut entry, key.len() as u64);
            entry.extend_from_slice(&key);
            write_uvarint(&mut entry, comp.len() as u64);
            dir_out.write_all(&entry)?;
            dir_len += entry.len() as u64;
            prev_run = first_run;
            num_chunks += 1;
        }
        comp_out.flush()?;
        drop(comp_out);
        dir_out.flush()?;
        drop(dir_out);
        drop(body);
        let _ = std::fs::remove_file(&self.path);

        // assemble: [header_len][header][num_chunks][dir…][compressed chunks…]
        let out_path = tmp.path(out_name);
        let mut out = BufWriter::with_capacity(1 << 20, File::create(&out_path)?);
        let mut head = Vec::new();
        write_uvarint(&mut head, header_len);
        write_uvarint(&mut head, self.n);
        write_uvarint(&mut head, interval);
        write_uvarint(&mut head, self.n_restarts);
        out.write_all(&head)?;
        let mut written = head.len() as u64;
        let mut offs = OffsetReader::open(&self.restarts_path)?;
        while let Some(off) = offs.next()? {
            head.clear();
            write_uvarint(&mut head, off);
            out.write_all(&head)?;
            written += head.len() as u64;
        }
        drop(offs);
        let _ = std::fs::remove_file(&self.restarts_path);
        if written != uvarint_len(header_len) + header_len {
            return Err(ExtBuildError::Internal(
                "dictionary section header length mismatch",
            ));
        }
        head.clear();
        write_uvarint(&mut head, num_chunks);
        out.write_all(&head)?;
        written += head.len() as u64;
        let mut dir_in = File::open(&dir_path)?;
        written += std::io::copy(&mut dir_in, &mut out)?;
        drop(dir_in);
        let _ = std::fs::remove_file(&dir_path);
        let mut comp_in = File::open(&comp_path)?;
        let copied = std::io::copy(&mut comp_in, &mut out)?;
        drop(comp_in);
        out.flush()?;
        let _ = std::fs::remove_file(&comp_path);
        let len = written + copied;
        debug_assert_eq!(
            len,
            uvarint_len(header_len) + header_len + uvarint_len(num_chunks) + dir_len + copied
        );
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
// Phase 3: remap every chunk to global ids
// ---------------------------------------------------------------------------

/// Rewrite every chunk's statements in global ids, loading one chunk's remap
/// tables at a time: default triples to `global_tri`, named quads to
/// `global_qtri` (graph first: a subject id in the quad layout, an ordinal in
/// the per-graph one). The default triples are teed into the SCHEMA and
/// LABEL_INDEX passes and each chunk's text-index words are remapped onto the
/// same subject ids. The
/// chunk spills are deleted as they are consumed.
#[allow(clippy::too_many_arguments)]
fn remap_chunks(
    tmp: &TmpDir,
    chunks: &[ChunkInfo],
    remaps: &RemapStore,
    quad_layout: bool,
    global_tri: &Path,
    global_qtri: Option<&Path>,
    mut text: Option<&mut crate::text_index_spill::TextSpill>,
    mut schema: Option<&mut ExtSchema>,
    mut labels: Option<&mut ExtLabels>,
) -> Result<(), ExtBuildError> {
    let mut out = BufWriter::with_capacity(1 << 20, File::create(global_tri)?);
    let mut qout = match global_qtri {
        Some(p) => Some(BufWriter::with_capacity(1 << 20, File::create(p)?)),
        None => None,
    };
    for (ci, chunk) in chunks.iter().enumerate() {
        let maps = remaps.load(ci)?;
        // The chunk's literal words, onto the same global subject ids.
        if let Some(text) = text.as_deref_mut() {
            text.remap_chunk(&tmp.path(&format!("c{ci}.tok")), &maps.subj)?;
        }
        let mut rd =
            BufReader::with_capacity(1 << 20, File::open(tmp.path(&format!("c{ci}.tri")))?);
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
            if let Some(x) = schema.as_deref_mut() {
                x.tee(gs, gp, go)?;
            }
            if let Some(x) = labels.as_deref_mut() {
                x.tee(gs, gp, go)?;
            }
        }
        drop(rd);
        let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.tri")));
        if chunk.quad_count > 0 {
            let qout = qout
                .as_mut()
                .ok_or(ExtBuildError::Internal("named quads without a spill"))?;
            let mut rd =
                BufReader::with_capacity(1 << 20, File::open(tmp.path(&format!("c{ci}.qtri")))?);
            // Quad index: the graph is a subject id; else an ordinal.
            let gmap = if quad_layout { &maps.subj } else { &maps.graph };
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
                qout.write_all(&gmap[(g - 1) as usize].to_le_bytes())?;
                qout.write_all(&maps.subj[(s - 1) as usize].to_le_bytes())?;
                qout.write_all(&maps.pred[(p - 1) as usize].to_le_bytes())?;
                qout.write_all(&maps.obj[(o - 1) as usize].to_le_bytes())?;
            }
        }
        let _ = std::fs::remove_file(tmp.path(&format!("c{ci}.qtri")));
    }
    out.flush()?;
    if let Some(mut q) = qout {
        q.flush()?;
    }
    let total = remaps.total_table_bytes();
    trace::add("remap_total_bytes", total);
    eprintln!(
        "extbuild: remapped {} chunk(s) one at a time — {} B of id tables in all, \
         at most {} B resident",
        chunks.len(),
        total,
        remaps.largest_table_bytes()
    );
    Ok(())
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
    build_permutation_section_observed(tmp, global_tri, perm, run_len, codec, prefix, &mut |_| {
        Ok(())
    })
}

/// [`build_permutation_section`], handing every deduplicated triple of the
/// merge, in this permutation's order, to `observe` (the SCHEMA pass).
fn build_permutation_section_observed(
    tmp: &TmpDir,
    global_tri: &Path,
    perm: crate::index::IndexPermutation,
    run_len: usize,
    codec: u8,
    prefix: &str,
    observe: &mut dyn FnMut(IdTriple) -> std::io::Result<()>,
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
                let path = tmp.path(&format!("{prefix}{}.run{}", perm.name(), runs.len()));
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
    note_scratch(tmp);
    note_merge(runs.len());
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
                observe(t)?;
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

/// Bytes [`write_uvarint`] takes for `v`.
fn uvarint_len(mut v: u64) -> u64 {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
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
                let path = tmp.path(&format!("q.run{}", runs.len()));
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
    note_scratch(tmp);
    note_merge(runs.len());
    let body_path = tmp.path("named.body");
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
            let path = tmp.path(&format!("ng{}.tri", self.graph));
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

// ---------------------------------------------------------------------------
// Phase 4b (quad index): the graph-leading orders of `GraphLayout::Quad`
// ---------------------------------------------------------------------------

/// The finished QUAD_INDEX section, spilled, with the digest the index
/// container's seal carries.
struct QuadSectionFile {
    path: PathBuf,
    len: u64,
    digest: [u8; 16],
    graph_count: u64,
    quad_count: u64,
}

/// Spill `input` (16-byte `(g, s, p, o)` records) as sorted, deduplicated runs
/// of `map`ped quads.
fn quad_runs(
    tmp: &TmpDir,
    input: &Path,
    run_len: usize,
    name: &str,
    map: impl Fn(IdQuad) -> IdQuad,
) -> Result<Vec<PathBuf>, ExtBuildError> {
    let mut runs = Vec::new();
    let mut rd = QuadRunReader::open(input)?;
    let mut run: Vec<IdQuad> = Vec::with_capacity(run_len.min(1 << 22));
    loop {
        let next = rd.next()?;
        if let Some(q) = next {
            run.push(map(q));
        }
        if run.len() >= run_len || (next.is_none() && !run.is_empty()) {
            sort_quads(&mut run);
            run.dedup();
            let path = tmp.path(&format!("{name}.run{}", runs.len()));
            let mut w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
            for &(g, a, b, c) in &run {
                w.write_all(&g.to_le_bytes())?;
                w.write_all(&a.to_le_bytes())?;
                w.write_all(&b.to_le_bytes())?;
                w.write_all(&c.to_le_bytes())?;
            }
            w.flush()?;
            runs.push(path);
            run.clear();
        }
        if next.is_none() {
            break;
        }
    }
    Ok(runs)
}

/// k-way merge of sorted runs, handing each distinct quad to `f` in order.
fn merge_quad_runs(
    runs: &[PathBuf],
    mut f: impl FnMut(IdQuad) -> Result<(), ExtBuildError>,
) -> Result<(), ExtBuildError> {
    note_merge(runs.len());
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
    let mut last: Option<IdQuad> = None;
    while let Some(std::cmp::Reverse((q, i))) = heap.pop() {
        if let Some(n) = readers[i].next()? {
            heap.push(std::cmp::Reverse((n, i)));
        }
        if last != Some(q) {
            f(q)?;
            last = Some(q);
        }
    }
    Ok(())
}

/// Encode a batch of finished quad tiles into an order's directory + payload.
fn drain_quad_tiles(
    pending: &mut Vec<Vec<crate::quad::QuadRow>>,
    codec: u8,
    perm: crate::index::IndexPermutation,
    dir: &mut crate::quad::PermDirWriter,
    payload: &mut BufWriter<File>,
) -> Result<(), ExtBuildError> {
    for t in crate::quad::encode_tiles(pending, codec, perm) {
        dir.add(&t);
        payload.write_all(&t.bytes)?;
    }
    pending.clear();
    Ok(())
}

/// Build the QUAD_INDEX section from the globally remapped quad spill: per
/// graph-leading order, an external sort of `(g, a, b, c)` and the shared
/// [`crate::quad::QuadTiler`] — byte-identical to `quad::encode_quad_index`.
fn build_quad_section(
    tmp: &TmpDir,
    global_qtri: &Path,
    run_len: usize,
    perms: crate::index::PermSet,
    codec: u8,
) -> Result<QuadSectionFile, ExtBuildError> {
    let gblocks_path = tmp.path("quad.gblocks");
    let mut gblocks = BufWriter::with_capacity(1 << 20, File::create(&gblocks_path)?);
    let mut gdir = crate::quad::GraphDirWriter::new(codec);
    let mut dirs: Vec<Vec<u8>> = Vec::with_capacity(perms.len());
    let mut payloads: Vec<(PathBuf, u64)> = Vec::with_capacity(perms.len());
    let mut quad_count: Option<u64> = None;
    for perm in perms.iter() {
        let spo = perm == crate::index::IndexPermutation::Spo;
        let name = format!("q{}", perm.name());
        let runs = quad_runs(tmp, global_qtri, run_len, &name, |(g, s, p, o)| {
            let (a, b, c) = perm.forward((s, p, o));
            (g, a, b, c)
        })?;
        let ppath = tmp.path(&format!("{name}.tiles"));
        let mut payload = BufWriter::with_capacity(1 << 20, File::create(&ppath)?);
        let mut dir = crate::quad::PermDirWriter::default();
        let mut tiler = crate::quad::QuadTiler::new();
        let mut pending: Vec<Vec<crate::quad::QuadRow>> = Vec::new();
        // G+SPO also yields the graph directory: (graph, deduplicated count).
        let mut cur: Option<(u32, u64)> = None;
        merge_quad_runs(&runs, |q| {
            tiler.push(q, &mut |t| pending.push(t));
            if pending.len() >= TILE_COMPRESS_BATCH {
                drain_quad_tiles(&mut pending, codec, perm, &mut dir, &mut payload)?;
            }
            if spo {
                cur = match cur {
                    Some((g, n)) if g == q.0 => Some((g, n + 1)),
                    Some((g, n)) => {
                        gdir.push(g, n, &mut gblocks)?;
                        Some((q.0, 1))
                    }
                    None => Some((q.0, 1)),
                };
            }
            Ok(())
        })?;
        let n = tiler.finish(&mut |t| pending.push(t));
        drain_quad_tiles(&mut pending, codec, perm, &mut dir, &mut payload)?;
        if let (true, Some((g, k))) = (spo, cur) {
            gdir.push(g, k, &mut gblocks)?;
        }
        payload.flush()?;
        drop(payload);
        for r in runs {
            let _ = std::fs::remove_file(r);
        }
        if quad_count.is_some_and(|prev| prev != n) {
            return Err(ExtBuildError::Internal("quad orders' dedup counts diverge"));
        }
        quad_count = Some(n);
        payloads.push((ppath, dir.payload_len()));
        dirs.push(dir.finish());
        eprintln!(
            "extbuild: quad order G+{} indexed ({n} unique quad(s))",
            perm.name()
        );
    }
    let (gindex, gblocks_len, graph_count) = gdir.finish(&mut gblocks)?;
    gblocks.flush()?;
    drop(gblocks);
    let quad_count = quad_count.unwrap_or(0);

    let mut part_lens = vec![gindex.len() as u64];
    part_lens.extend(dirs.iter().map(|d| d.len() as u64));
    part_lens.push(gblocks_len);
    part_lens.extend(payloads.iter().map(|(_, l)| *l));
    let preamble = crate::quad::encode_preamble(perms, graph_count, quad_count, &part_lens);

    let path = tmp.path("quad.sec");
    let mut out = BufWriter::with_capacity(1 << 20, File::create(&path)?);
    let mut hasher = blake3::Hasher::new();
    let mut len = 0u64;
    for bytes in std::iter::once(&preamble)
        .chain(std::iter::once(&gindex))
        .chain(dirs.iter())
    {
        out.write_all(bytes)?;
        hasher.update(bytes);
        len += bytes.len() as u64;
    }
    for p in std::iter::once(&gblocks_path).chain(payloads.iter().map(|(p, _)| p)) {
        let mut rd = File::open(p)?;
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let k = rd.read(&mut buf)?;
            if k == 0 {
                break;
            }
            out.write_all(&buf[..k])?;
            hasher.update(&buf[..k]);
            len += k as u64;
        }
        drop(rd);
        let _ = std::fs::remove_file(p);
    }
    out.flush()?;
    let mut digest = [0u8; 16];
    digest.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Ok(QuadSectionFile {
        path,
        len,
        digest,
        graph_count,
        quad_count,
    })
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
        let comp_path = tmp.path(&format!("{name}.tiles"));
        Ok(StreamingTiler {
            tile_budget: crate::write_opts::tile_budget(),
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
        let out_path = tmp.path(&format!("{name}.sec"));
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

/// Stream `header | metadata | dict container | index container | text index |
/// named graphs | footer` to `output`, hashing the payload sections
/// incrementally (same part order as `write_dataset_from_parts`), then patch the
/// finished header at offset 0.
#[allow(clippy::too_many_arguments)]
fn write_final_file(
    output: &Path,
    metadata: &[u8],
    build_info: &[u8],
    dict: &MergedDict,
    perm_sections: &[SectionFile],
    text: Option<&crate::text_index_spill::TextSection>,
    named: Option<&NamedSection>,
    quad: Option<&QuadSectionFile>,
    quad_count: u64,
    codec: u8,
    perms: crate::index::PermSet,
) -> Result<(), ExtBuildError> {
    use crate::header::{
        Header, FLAG_HAS_QUADS, FLAG_HAS_QUOTED_TRIPLES, FLAG_TEXT_INDEX_NAMED_GRAPHS,
        FLAG_TILE_SYNOPSIS, HEADER_LEN,
    };
    // Quad index: the index container ends with the seal (the section's
    // digest), which every pre-quad-index reader refuses on the section count.
    let seal = quad.map(|q| crate::quad::encode_seal(q.digest));

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
    write_uvarint(
        &mut index_frame,
        (perm_sections.len() + usize::from(seal.is_some())) as u64,
    );
    let mut index_len = index_frame.len() as u64;
    let mut index_section_heads = Vec::with_capacity(perm_sections.len());
    for s in perm_sections {
        let mut h = Vec::new();
        write_uvarint(&mut h, s.len);
        index_len += h.len() as u64 + s.len;
        index_section_heads.push(h);
    }
    let mut seal_frame = Vec::new();
    if let Some(seal) = &seal {
        write_uvarint(&mut seal_frame, seal.len() as u64);
        seal_frame.extend_from_slice(seal);
        index_len += seal_frame.len() as u64;
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
    // The pyramid is absent (length 0), so the text index follows the index
    // directly and the named graphs follow it — the same arithmetic
    // `write_dataset_from_parts` does.
    let text_offset = index_offset + index_len;
    let text_len = text.map_or(0, |t| t.len());
    let named_offset = text_offset + text_len;

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
    write_hashed(&mut out, &mut hasher, &seal_frame)?;

    // pyramid section is empty (hash update of nothing = no-op, same as the
    // in-RAM writer pushing an empty slice).

    // text index: `varint table_len | token table | postings`. A kind-11
    // (predicate-restricted) section is outside the content hash and starts
    // with its own digest, patched in below once the payload has gone past —
    // the bytes `write_dataset_from_parts` writes.
    let mut text_digest: Option<blake3::Hash> = None;
    if let Some(t) = text {
        if t.partial {
            out.write_all(&[0u8; crate::header::TEXT_INDEX_PARTIAL_DIGEST_LEN as usize])?;
            let mut digest = blake3::Hasher::new();
            write_hashed(&mut out, &mut digest, &t.head)?;
            copy_hashed(&t.table, &mut out, &mut digest)?;
            copy_hashed(&t.postings, &mut out, &mut digest)?;
            text_digest = Some(digest.finalize());
        } else {
            write_hashed(&mut out, &mut hasher, &t.head)?;
            copy_hashed(&t.table, &mut out, &mut hasher)?;
            copy_hashed(&t.postings, &mut out, &mut hasher)?;
        }
    }

    // named-graphs container
    if let Some(n) = named {
        write_hashed(&mut out, &mut hasher, &named_frame)?;
        copy_hashed(&n.body, &mut out, &mut hasher)?;
    }

    // quad index: where the named graphs would be; not a hash part of its
    // own (the seal above carries its digest), exactly as the in-RAM writer.
    if let Some(q) = quad {
        let mut rd = File::open(&q.path)?;
        std::io::copy(&mut rd, &mut out)?;
    }

    out.write_all(&crate::header::MAGIC)?; // footer marker
    out.flush()?;

    let mut hash = [0u8; 16];
    hash.copy_from_slice(&hasher.finalize().as_bytes()[..16]);

    let header = Header {
        version: crate::header::CURRENT_FORMAT_VERSION,
        flags: FLAG_TILE_SYNOPSIS
            | if named_len > 0 || quad.is_some() {
                FLAG_HAS_QUADS
            } else {
                0
            }
            | if quad.is_some() {
                crate::header::FLAG_QUAD_INDEX
            } else {
                0
            }
            | if dict.has_quoted {
                FLAG_HAS_QUOTED_TRIPLES
            } else {
                0
            }
            | if text.is_some_and(|t| t.named) {
                FLAG_TEXT_INDEX_NAMED_GRAPHS
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
        text_index_offset: if text_len > 0 { text_offset } else { 0 },
        text_index_len: text_len,
        text_index_partial: text.is_some_and(|t| t.partial),
        build_info_offset: if build_len > 0 {
            HEADER_LEN as u64 + meta_len
        } else {
            0
        },
        build_info_len: build_len,
        extra_sections: match quad {
            Some(q) => vec![crate::header::Section {
                kind: crate::header::SectionKind::QuadIndex,
                flags: 0,
                offset: named_offset,
                length: q.len,
            }],
            None => Vec::new(),
        },
    };
    let mut f = out.into_inner().map_err(|e| e.into_error())?;
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&header.to_bytes())?;
    if let Some(d) = text_digest {
        f.seek(SeekFrom::Start(text_offset))?;
        f.write_all(&d.as_bytes()[..crate::header::TEXT_INDEX_PARTIAL_DIGEST_LEN as usize])?;
    }
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
                ..Default::default()
            },
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (bytes, stats)
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
                ..Default::default()
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
        let mut chunker = Chunker::new(&tmp, 4 * 1024, GraphLayout::PerGraph); // ~4 KiB chunks
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
        let (merged, remaps) = merge_dictionaries(&tmp, &chunks).unwrap();

        let global_tri = tmp.path("global.tri");
        remap_chunks(
            &tmp,
            &chunks,
            &remaps,
            false,
            &global_tri,
            None,
            None,
            None,
            None,
        )
        .unwrap();

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
            None,
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

    /// [`build_ext`] with the chunk budget forced down to `chunk` bytes, so a
    /// small graph becomes many chunks.
    fn build_ext_split(quads: &[RawQuad], chunk: u64) -> Vec<u8> {
        TEST_SPLIT.with(|c| c.set(Some((chunk, 1 << 10))));
        let (bytes, _) = build_ext(quads.to_vec(), 0);
        TEST_SPLIT.with(|c| c.set(None));
        bytes
    }

    /// The id remap tables were the part of the memory-bounded build that grew
    /// with the dataset: every chunk's stayed resident from the dictionary
    /// merge to the end of the remap. Now they go to disk as the merge writes
    /// them and one chunk's are loaded at a time. Cutting four times the
    /// graph into four times the chunks quadruples what the old build held
    /// and leaves what this one holds where it was: one chunk's tables, at
    /// most a quarter of the chunk budget. The files stay byte-identical.
    #[test]
    fn remap_memory_does_not_grow_with_the_chunk_count() {
        const CHUNK: u64 = 4 * 1024;
        let run = |n: usize| -> (u64, u64, u64) {
            let quads = test_quads(n);
            let reference = build_reference(quads.clone());
            let collector = crate::build_trace::Collector::start();
            let bytes = build_ext_split(&quads, CHUNK);
            let t = collector.snapshot();
            drop(collector);
            assert_eq!(bytes, reference, "{n} quads: external build drifted");
            (
                t.counter("chunks").unwrap(),
                t.counter("remap_total_bytes").unwrap(),
                t.counter("remap_peak_bytes").unwrap(),
            )
        };
        let (k1, total1, peak1) = run(1_000);
        let (k4, total4, peak4) = run(4_000);
        assert!(k1 >= 20 && k4 >= 3 * k1, "chunks: {k1}, then {k4}");
        // What the old merge held resident grows with the chunks…
        assert!(total4 >= 3 * total1, "tables: {total1} B, then {total4} B");
        assert!(
            total4 >= 50 * peak4,
            "{total4} B in all, {peak4} B resident"
        );
        // …what this one holds does not: every buffered statement is charged
        // at least 96 B against the chunk budget and brings at most six ids.
        assert!(
            peak4 <= peak1 * 3 / 2,
            "resident: {peak1} B, then {peak4} B"
        );
        assert!(
            peak4 <= (CHUNK + 1024) / 4,
            "{peak4} B for a {CHUNK} B chunk"
        );
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
    ///
    /// **A text index is carried over when it was finished.** Phase 3b leaves
    /// `tok.table` + `tok.postings` behind before any permutation is sorted, and
    /// they are reused as they are. Text-index files of an unfinished phase
    /// (`global.tok`, `tok.run*`, `c<i>.tok`) are a hard stop for the same
    /// reason as `global.qtri`.
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
        let unfinished_text = std::fs::read_dir(&spill).unwrap().flatten().any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n == "global.tok" || n.starts_with("tok.run") || n.ends_with(".tok")
        });
        assert!(
            !unfinished_text,
            "this spill has an unfinished text index (global.tok / tok.run* / c<i>.tok); \
             resuming it here would write a file without it — rerun the build instead"
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
            graph_names: None,
            graph_count: 0,
            schema_facts: Default::default(),
        };
        let tmp = TmpDir { dir: spill.clone() };
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
        let (table, postings) = (spill.join("tok.table"), spill.join("tok.postings"));
        let text = if table.exists() && postings.exists() {
            let table_len = std::fs::metadata(&table).unwrap().len();
            let mut head = Vec::new();
            write_uvarint(&mut head, table_len);
            // The scope block in the token table decides kind 6 or 11, and
            // the header's named-graphs bit.
            let mut prefix = head.clone();
            prefix.extend(std::fs::read(&table).unwrap());
            let scope = crate::text_index::TextIndex::from_token_table(
                &prefix,
                codec,
                Box::new(|_, _| None),
            )
            .expect("tok.table parses")
            .scope()
            .clone();
            eprintln!("resume: reusing the text index");
            Some(crate::text_index_spill::TextSection {
                head,
                table,
                table_len,
                postings_len: std::fs::metadata(&postings).unwrap().len(),
                postings,
                partial: scope.restricts_predicates(),
                named: scope.covers_named_graphs(),
            })
        } else {
            None
        };
        write_final_file(
            &out,
            &metadata,
            &[],
            &merged,
            &sections,
            text.as_ref(),
            None,
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
        let mut chunker = Chunker::new(&tmp, 4 * 1024, GraphLayout::PerGraph); // ~4 KiB chunks
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
        let mut chunker = Chunker::new(&tmp, 64 * 1024, GraphLayout::PerGraph);
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
        let (merged, remaps) = merge_dictionaries(tmp, chunks)?;
        let has_named = chunks.iter().any(|c| c.quad_count > 0);
        let global_tri = tmp.path("global.tri");
        let global_qtri = tmp.path("global.qtri");
        remap_chunks(
            tmp,
            chunks,
            &remaps,
            false,
            &global_tri,
            has_named.then_some(global_qtri.as_path()),
            None,
            None,
            None,
        )?;

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
            None,
            named.as_ref(),
            None,
            quads,
            codec,
            perms,
        )?;
        Ok(std::fs::read(out)?)
    }

    // ---- `--text-index` in the memory-bounded build -------------------------

    /// Literal-rich quads: titles and provenance-style descriptions in the
    /// default graph (multi-byte words included), provenance snapshots in
    /// named graphs, on top of the fedlex shape (its `"Artikel …"@de` literals
    /// live in named graphs too).
    fn text_quads() -> Vec<RawQuad> {
        let mut q = fedlex_shaped_quads(40, 9);
        for i in 0..600usize {
            let s = format!("<http://ex/w{}>", i % 150);
            q.push((
                s.clone(),
                "<http://purl.org/dc/terms/title>".to_string(),
                format!("\"Glucose Études {} on ünïcode café\"@en", i % 37),
                None,
            ));
            q.push((
                s.clone(),
                "<http://purl.org/dc/terms/description>".to_string(),
                format!("\"The entity {} has been modified.\"", i % 150),
                None,
            ));
            q.push((
                format!("<http://ex/w{}/prov/se/1>", i % 150),
                "<http://purl.org/dc/terms/description>".to_string(),
                format!("\"The entity has been created ({i}).\""),
                Some(format!("<http://ex/w{}/prov/>", i % 150)),
            ));
        }
        q
    }

    fn text_scopes() -> Vec<crate::text_index::TextIndexScope> {
        use crate::text_index::{TextIndexGraphs, TextIndexScope};
        let none: [&str; 0] = [];
        vec![
            TextIndexScope::default(),
            TextIndexScope::from_options(TextIndexGraphs::All, none, none).unwrap(),
            TextIndexScope::from_options(
                TextIndexGraphs::Default,
                none,
                ["http://purl.org/dc/terms/description"],
            )
            .unwrap(),
            TextIndexScope::from_options(
                TextIndexGraphs::All,
                ["http://purl.org/dc/terms/description"],
                none,
            )
            .unwrap(),
        ]
    }

    fn build_reference_text(
        quads: &[RawQuad],
        scope: &crate::text_index::TextIndexScope,
    ) -> Vec<u8> {
        let (bytes, _stats) = ingest::assemble_dataset_streaming_algo(
            |visit: &mut dyn FnMut(RawQuad)| {
                for q in quads.iter().cloned() {
                    visit(q);
                }
                Ok(())
            },
            false,
            scope.clone(),
            None,
            crate::PyramidAlgo::Louvain,
            |_, _, _| Vec::new(),
        )
        .unwrap();
        bytes
    }

    /// The public entry point with a text index; `split` forces many chunks and
    /// many text sort runs on a small graph.
    fn build_ext_text(
        quads: &[RawQuad],
        scope: &crate::text_index::TextIndexScope,
        split: Option<(u64, usize)>,
    ) -> Vec<u8> {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rete-extbuild-text-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        TEST_SPLIT.with(|c| c.set(split));
        let res = build_external(
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
                text_index: Some(scope.clone()),
                ..Default::default()
            },
        );
        TEST_SPLIT.with(|c| c.set(None));
        res.unwrap();
        let bytes = std::fs::read(&out).unwrap();
        // The spill directory is gone; only the output remains.
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left, [std::ffi::OsString::from("out.rete")]);
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    /// B1's requirement: with `--text-index` the memory-bounded build writes the
    /// same TEXT_INDEX section as the in-RAM build — and, the pyramid being off
    /// on both sides, the same file — in one chunk and in many, for every scope.
    #[test]
    fn external_text_index_is_byte_identical_to_in_ram() {
        let quads = text_quads();
        for scope in text_scopes() {
            let reference = build_reference_text(&quads, &scope);
            let rh = crate::Header::from_bytes(&reference).unwrap();
            assert!(
                rh.text_index_len > 0,
                "{scope:?}: the fixture must index something"
            );
            assert_eq!(rh.text_index_partial, scope.restricts_predicates());
            assert_eq!(
                rh.text_index_covers_named_graphs(),
                scope.covers_named_graphs()
            );
            for split in [None, Some((4 * 1024, 1024))] {
                let bytes = build_ext_text(&quads, &scope, split);
                assert_eq!(
                    bytes, reference,
                    "{scope:?}, split {split:?}: external build must be byte-identical"
                );
            }
        }
    }

    /// The external file's text index answers searches, verifies, and carries
    /// its scope: the provenance text is found only when named graphs are in
    /// scope, the descriptions only when their predicate is.
    #[test]
    fn external_text_index_is_searchable_in_its_scope() {
        let quads = text_quads();
        let scopes = text_scopes();
        let search = |scope: &crate::text_index::TextIndexScope, word: &str| -> usize {
            let bytes = build_ext_text(&quads, scope, Some((4 * 1024, 1024)));
            assert!(crate::verify(&bytes).unwrap());
            let rete = crate::Rete::open(&bytes).unwrap();
            assert!(rete.has_text_index());
            assert_eq!(rete.text_index().unwrap().scope(), scope);
            rete.text_search(&[word], None, 0).len()
        };
        // "glucose": 150 titled works, in every scope that keeps titles.
        assert_eq!(search(&scopes[0], "glucose"), 150);
        assert_eq!(search(&scopes[2], "glucose"), 150);
        assert_eq!(
            search(&scopes[3], "glucose"),
            0,
            "an include-only scope drops titles"
        );
        // "modified": the default-graph descriptions.
        assert_eq!(search(&scopes[0], "modified"), 150);
        assert_eq!(search(&scopes[2], "modified"), 0, "excluded predicate");
        // "created": only in the named provenance graphs.
        assert_eq!(
            search(&scopes[0], "created"),
            0,
            "named graphs not in scope"
        );
        assert_eq!(search(&scopes[1], "created"), 150);
        assert_eq!(search(&scopes[3], "created"), 150);
        // A multi-byte word survives the spill's byte-level front coding.
        assert_eq!(search(&scopes[0], "études"), 150);
    }

    /// No literal in scope: no section at all, as the in-RAM build writes none.
    #[test]
    fn external_text_index_with_nothing_in_scope_writes_no_section() {
        let quads = test_quads(400);
        let none: [&str; 0] = [];
        let scope = crate::text_index::TextIndexScope::from_options(
            crate::text_index::TextIndexGraphs::Default,
            ["http://ex/never"],
            none,
        )
        .unwrap();
        let reference = build_reference_text(&quads, &scope);
        let bytes = build_ext_text(&quads, &scope, None);
        assert_eq!(bytes, reference);
        assert_eq!(crate::Header::from_bytes(&bytes).unwrap().text_index_len, 0);
    }

    // ---- quad index (`GraphLayout::Quad`) --------------------------------

    fn build_reference_quad(quads: Vec<RawQuad>, perms: crate::index::PermSet) -> Vec<u8> {
        let (bytes, _) = ingest::assemble_dataset_streaming_with_layout(
            move |visit: &mut dyn FnMut(RawQuad)| {
                for q in quads.iter().cloned() {
                    visit(q);
                }
                Ok(())
            },
            false,
            false,
            None,
            crate::PyramidAlgo::Louvain,
            perms,
            GraphLayout::Quad,
            |_, _, _| Vec::new(),
        )
        .unwrap();
        bytes
    }

    fn build_ext_quad(quads: Vec<RawQuad>, perms: crate::index::PermSet) -> (Vec<u8>, BuildStats) {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("rete-extbuild-quad-{}-{seq}", std::process::id()));
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
                memory_budget: 0,
                tmp_dir: Some(dir.clone()),
                perms,
                graphs: GraphLayout::Quad,
                ..Default::default()
            },
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        (bytes, stats)
    }

    /// The external quad build writes the in-RAM build's bytes, for both
    /// permutation sets — the shared `QuadTiler` is what guarantees it.
    #[test]
    fn quad_external_build_is_byte_identical() {
        for perms in [crate::index::PermSet::ALL, crate::index::PermSet::CORE] {
            let quads = fedlex_shaped_quads(60, 9);
            let reference = build_reference_quad(quads.clone(), perms);
            let (bytes, stats) = build_ext_quad(quads.clone(), perms);
            assert_eq!(bytes, reference, "quad external build, {perms:?}");
            let header = crate::Header::from_bytes(&bytes).unwrap();
            assert!(header.has_quad_index());
            assert_eq!(stats.statements as u64, header.quad_count);
            let rete = crate::Rete::open(&bytes).unwrap();
            assert_eq!(rete.named_graph_count(), stats.named_graphs);
            assert!(crate::file::verify(&bytes).unwrap());
        }
    }

    /// Many chunks and many sort runs: graph names first seen in different
    /// chunks must merge into one dictionary id, and runs must merge back into
    /// the same tiles.
    #[test]
    fn multi_chunk_multi_run_quad_build_is_byte_identical() {
        let quads = fedlex_shaped_quads(80, 7);
        let reference = build_reference_quad(quads.clone(), crate::index::PermSet::ALL);

        let dir = std::env::temp_dir().join(format!("rete-extbuild-mcq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        let tmp = TmpDir::create(&dir).unwrap();
        let mut chunker = Chunker::new(&tmp, 4 * 1024, GraphLayout::Quad);
        for q in quads.iter().cloned() {
            chunker.push(q).unwrap();
        }
        let chunks = chunker.finish().unwrap();
        assert!(
            chunks.len() >= 8,
            "expected many chunks, got {}",
            chunks.len()
        );
        let (merged, remaps) = merge_dictionaries(&tmp, &chunks).unwrap();
        assert_eq!(merged.graph_count, 0, "no separate graph-name merge");
        let global_tri = tmp.path("global.tri");
        let global_qtri = tmp.path("global.qtri");
        remap_chunks(
            &tmp,
            &chunks,
            &remaps,
            true,
            &global_tri,
            Some(&global_qtri),
            None,
            None,
            None,
        )
        .unwrap();
        let codec = crate::file::writer_codec();
        let perms = crate::index::PermSet::ALL;
        let mut sections = Vec::new();
        let mut default_count = 0u64;
        for perm in perms.iter() {
            let (sec, n) =
                build_permutation_section(&tmp, &global_tri, perm, 256, codec, "").unwrap();
            default_count = n;
            sections.push(sec);
        }
        // 64-quad runs: many runs per order, merged back.
        let quad = build_quad_section(&tmp, &global_qtri, 64, perms, codec).unwrap();
        write_final_file(
            &out,
            &[],
            &[],
            &merged,
            &sections,
            None,
            None,
            Some(&quad),
            default_count + quad.quad_count,
            codec,
            perms,
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(bytes, reference, "multi-chunk, multi-run quad build");
        drop(tmp);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The quad layout needs the 3- or 6-permutation set (see `quad::check_perms`).
    #[test]
    fn quad_external_build_refuses_other_permutation_sets() {
        let dir = std::env::temp_dir().join(format!("rete-extbuild-q5-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let r = build_external(
            |visit| visit(("<a>".into(), "<p>".into(), "<b>".into(), Some("<g>".into()))),
            &dir.join("out.rete"),
            ExternalBuildOptions {
                tmp_dir: Some(dir.clone()),
                perms: crate::index::PermSet::from_bits(0b0001_1111).unwrap(),
                graphs: GraphLayout::Quad,
                ..Default::default()
            },
        );
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
