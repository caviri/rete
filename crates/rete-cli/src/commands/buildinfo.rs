//! Build conditions — the **when / by what / how** record of a `.rete` build,
//! stored in the file's kind-7 `BuildInfo` section (see
//! `rete_core::header::SectionKind::BuildInfo`).
//!
//! The Dataset Card answers *what the data is*; this section answers *how this
//! particular file came to be*: a build timestamp, the builder version, the
//! build parameters that shaped the result, and measured cost figures for the
//! card's starter queries. All of that is per-build — two builds of identical
//! data legitimately differ here — so the section lives **outside the content
//! hash** and never perturbs the reproducible blake3 the card folds into.
//!
//! Cost figures come in two kinds, deliberately kept together but labelled
//! apart (issue #153): `bytes`/`requests` are **portable** — a property of the
//! file layout and the query, identical from disk, R2 or GitHub Pages — while
//! `debug_ms` is a **reference measurement from one machine** and is named and
//! contextualized so it cannot be quoted as a property of the file.

use serde::{Deserialize, Serialize};

use rete_core::{eval_query, CountingReader, QueryOutput, RangeReader, Rete};

use super::card::ExampleQuery;

/// Schema version of the build-info JSON (bumped on incompatible change).
pub(crate) const BUILD_INFO_SCHEMA: u8 = 1;

/// The build-conditions record. Every field is `#[serde(default)]`-friendly so
/// newer records keep deserializing in older readers and vice versa.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BuildInfo {
    /// Build-info schema version (this file: [`BUILD_INFO_SCHEMA`]).
    #[serde(default)]
    pub schema: u8,
    /// When the build started (RFC 3339 UTC, whole seconds; `SOURCE_DATE_EPOCH`
    /// honored like `built_at`). Absent before 0.4.0.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// When the build wrote the file (RFC 3339 UTC, whole seconds). Honors
    /// `SOURCE_DATE_EPOCH` for reproducible pipelines. Distinct from the
    /// card's curated `created`/`source_date`, which describe the *data*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built_at: Option<String>,
    /// The binary that wrote the file, e.g. `rete-cli 0.3.2`. The gap this
    /// closes: a misbehaving external file used to carry no way to tell which
    /// `rete` had written it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub builder: Option<String>,
    /// The same binary in full: version, git commit, target, profile, features.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    /// The operating system and architecture the build ran on — and nothing
    /// else about the machine: no host name, user, path or environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
    /// The build parameters that shaped the result.
    #[serde(default, skip_serializing_if = "BuildParams::is_empty")]
    pub params: BuildParams,
    /// What the build read: each input's file name (never its path), format,
    /// size and BLAKE3, hashed as it streamed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<InputRecord>,
    /// Inputs past [`MAX_INPUTS_LISTED`], counted but not listed, so a build
    /// over thousands of shards does not grow the CARD-tier read without bound.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub inputs_omitted: u64,
    /// Statement counts: read, unique, duplicates dropped, graphs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<Counts>,
    /// Debugging and performance figures of this one run: wall time per phase,
    /// peak RSS, the memory limit and CPUs it saw. Volatile — omitted under
    /// `RETE_BUILD_RECORD=reproducible`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunRecord>,
    /// What the memory-bounded build's external sort did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill: Option<SpillRecord>,
    /// Measured starter-query costs, run against the finished file (absent with
    /// `--no-card-costs`, and in memory-bounded and merged files written before
    /// 0.4.1, whose counts-only cards had no starter queries to measure).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query_costs: Option<QueryCosts>,
    /// Starter queries this build generated, **ran**, and then refused to ship
    /// because the run showed they were worthless on this very file. Empty on
    /// a healthy build — and the only surviving record of the drop, so the
    /// evidence outlives the terminal the build scrolled past.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dropped_queries: Vec<DroppedQuery>,
    /// Set when `rete recard` replaced this file's card instead of a rebuild.
    /// Every other field then still describes the **build** that wrote the
    /// data sections — they were copied byte for byte, so its timestamp,
    /// builder and parameters are still the truth about them — except
    /// `query_costs` / `dropped_queries` and `params.card_top_n`, which belong
    /// to the new card and were measured by the re-card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recarded: Option<Recarded>,
}

/// When, by what, and from which file a card was replaced in place of a
/// rebuild.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Recarded {
    /// RFC 3339 UTC, like [`BuildInfo::built_at`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    /// The binary that re-carded it, e.g. `rete-cli 0.4.0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// The content hash (hex) of the file it was re-carded from — the identity
    /// the data sections had before the card changed it.
    pub from_content_hash: String,
    /// The binary that re-carded it, in full.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<Engine>,
    /// Where the re-card ran (OS and architecture only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<Host>,
    /// The file it re-carded: its name and size. Its identity is
    /// `from_content_hash`, so it is not hashed again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<InputRecord>,
    /// The re-card's own flags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<RecardParams>,
    /// The re-card's wall time per phase and peak RSS (volatile, like
    /// [`BuildInfo::run`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunRecord>,
}

/// `rete recard`'s flags, as the re-card record keeps them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecardParams {
    #[serde(default)]
    pub keep_curated: bool,
    /// `--in-place`: the card was appended to the file itself, not copied.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub in_place: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_file: Option<FileDigest>,
    #[serde(default)]
    pub card_costs: bool,
    #[serde(default)]
    pub memory_budget_mb: u64,
}

/// The engine that wrote a file — enough to rebuild the same binary.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Engine {
    /// `rete-cli`.
    pub name: String,
    /// The crate version, e.g. `0.4.0`.
    pub version: String,
    /// The git commit the binary was built from, or `unknown` (see
    /// `crates/rete-cli/build.rs` for where it comes from).
    pub commit: String,
    /// The Rust target triple, e.g. `x86_64-unknown-linux-gnu`.
    pub target: String,
    /// The cargo profile: `release`, or `debug` for a development binary —
    /// whose timings are not comparable with a release build's.
    pub profile: String,
    /// The rete-core features compiled in (`compression`, `parallel`, …).
    #[serde(default)]
    pub features: Vec<String>,
}

/// The machine a build ran on: OS and architecture, deliberately nothing more.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Host {
    /// `std::env::consts::OS`: `linux`, `macos`, `windows`, …
    pub os: String,
    /// `std::env::consts::ARCH`: `x86_64`, `aarch64`, …
    pub arch: String,
}

/// One build input.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct InputRecord {
    /// The file name without its directory (paths name users and machines),
    /// or `stdin`.
    pub name: String,
    /// The syntax it was parsed as (`nt`, `nq`, `ttl`, `trig`, `rdfxml`,
    /// `rete`).
    pub format: String,
    /// `gzip` when the bytes were gzip, detected from their magic number.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compression: Option<String>,
    /// Bytes read, as stored (compressed when the input was).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// BLAKE3 (hex) of exactly those bytes, hashed as they streamed in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blake3: Option<String>,
}

/// A small file the build read whole (`--card-file`): its name and digest.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileDigest {
    pub name: String,
    pub bytes: u64,
    pub blake3: String,
}

impl FileDigest {
    pub(crate) fn of(path: &str, bytes: &[u8]) -> Self {
        FileDigest {
            name: file_name(path),
            bytes: bytes.len() as u64,
            blake3: blake3::hash(bytes).to_hex().to_string(),
        }
    }
}

/// Statement counts. `statements_read` is what the parser handed over;
/// `statements_unique` is what the file holds (the header's `quad_count`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Counts {
    pub statements_read: u64,
    /// `--materialize`: entailed triples the reasoner added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub statements_inferred: Option<u64>,
    pub statements_unique: u64,
    /// `read + inferred - unique`: the duplicates the index sort dropped.
    pub duplicates_dropped: u64,
    pub default_graph_triples: u64,
    pub named_graphs: u64,
    pub named_graph_quads: u64,
    pub terms: u64,
    /// Invalid IRIs the tolerant parse stored verbatim (absent when there were
    /// none; a `--strict` build that finished had none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invalid_iris: Option<InvalidIris>,
}

/// The `--strict` audit's tally (`rete_core::IriReport`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct InvalidIris {
    pub statements: u64,
    pub occurrences: u64,
    pub repairable: u64,
    pub unrepairable: u64,
}

/// One run's debugging and performance figures.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct RunRecord {
    /// Wall time from the start of the command to the record being sealed —
    /// everything, the final write included.
    pub wall_ms: u64,
    /// Wall time per phase, in order. The phases partition the run; their sum
    /// is `wall_ms` less the setup before the first one.
    #[serde(default)]
    pub phases: Vec<PhaseTime>,
    /// Peak resident set size (`VmHWM`); `null` off Linux.
    pub peak_rss_bytes: Option<u64>,
    /// The memory limit the process saw (its cgroup's); `null` when none.
    pub memory_limit_bytes: Option<u64>,
    /// Logical CPUs available to the process (affinity and cgroup quota
    /// applied — what the parallel phases size themselves to).
    pub cpus: Option<u64>,
}

/// One phase's wall time — and, for the phases measured on their own (the
/// card's derivation and its starter queries), that phase's peak RSS.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct PhaseTime {
    pub phase: String,
    pub ms: u64,
    /// The peak resident set size **during this phase alone** (`VmHWM`, reset
    /// when the phase began). Only on the phases measured that way; the run's
    /// own `peak_rss_bytes` is the peak over every phase. Absent before 0.4.1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak_rss_bytes: Option<u64>,
}

/// The memory-bounded build's external sort.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SpillRecord {
    /// Chunks the input was cut into.
    pub chunks: u64,
    /// k-way merges over spilled files (dictionary, permutations, named graphs,
    /// text index, labels/schema): each reads every one of its runs once.
    pub merge_passes: u64,
    /// The spilled sorted runs (and chunk term files) those merges read.
    pub sort_runs: u64,
    /// The spill directory's size at its fullest, sampled at phase boundaries
    /// and after each sort's runs are written — a lower bound on the peak.
    pub scratch_peak_bytes: u64,
}

/// The flags in force for this build. `command` names the CLI path taken.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct BuildParams {
    /// CLI entry point: `build`, `build --memory-budget-mb`, `merge`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_pyramid: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text_index: bool,
    /// `--label-index`: a LABEL_INDEX section (kind 8) was written.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub label_index: bool,
    /// `--no-schema`: the SCHEMA section (kind 9) was left out.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_schema: bool,
    /// `--text-index-graphs`: `all` when the named graphs' literals are indexed
    /// too; absent for the default graph only (the pre-0.4.0 scope).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_index_graphs: Option<String>,
    /// `--text-index-include`: the only predicates whose literals are indexed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_index_include: Vec<String>,
    /// `--text-index-exclude`: predicates whose literals are left out.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub text_index_exclude: Vec<String>,
    /// `--materialize`: OWL RL/RDFS entailments were baked into the file.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub materialize: bool,
    /// `--reason`: the coherence verdict was stamped into the card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reason: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pyramid_algo: Option<String>,
    /// `--memory-budget-mb` for the external build (None = in-memory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_budget_mb: Option<u64>,
    /// Section codec the writer used (`zstd` / `none`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// The `CARD_TOP_N` cap in force when the card's lists were derived — the
    /// number `truncated: true` was hinting at without stating.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_top_n: Option<u32>,
    /// The writer knobs in force (`--tile-kib`, `--dict-chunk-kib`,
    /// `--zstd-level`, the frame shape, `--frequent-objects`), so a file says
    /// how it was compressed. Absent in files written before 0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub write: Option<WriteRecord>,
    // The fields below record the EFFECTIVE value of a flag, default or not,
    // so the record reads the same whatever the defaults of a later version.
    // Absent in files written before 0.4.0.
    /// `--permutations`: 6 or 3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permutations: Option<u8>,
    /// `--named-graphs`: `per-graph` or `quad`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named_graphs: Option<String>,
    /// `--collapse-graphs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collapse_graphs: Option<bool>,
    /// `--strict`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict: Option<bool>,
    /// `--quoted-triple-syntax` for Turtle/TriG input: `rdf-star` or `rdf12`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quoted_triple_syntax: Option<String>,
    /// `--type-predicate`, when one replaced `rdf:type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_predicate: Option<String>,
    /// Not `--no-card-costs`: the starter queries were run and measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_costs: Option<bool>,
    /// `--card-file`: its file name (no path) and BLAKE3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card_file: Option<FileDigest>,
}

/// [`rete_core::write_opts::WriteOptions`] as recorded in the build info.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct WriteRecord {
    pub tile_kib: u32,
    pub dict_chunk_kib: u32,
    pub zstd_level: i32,
    /// `sized` (single-segment frames, the 0.4 default) or `stream`.
    pub zstd_frame: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub frequent_objects: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The writer settings this process is building with.
pub(crate) fn write_record() -> Option<WriteRecord> {
    let o = rete_core::write_opts::write_options();
    Some(WriteRecord {
        tile_kib: (o.tile_budget / 1024) as u32,
        dict_chunk_kib: (o.dict_chunk_budget / 1024) as u32,
        zstd_level: o.zstd_level,
        zstd_frame: match o.frame {
            rete_core::write_opts::FrameShape::Sized => "sized",
            rete_core::write_opts::FrameShape::Stream => "stream",
        }
        .to_string(),
        frequent_objects: o.frequent_objects as u64,
    })
}

impl BuildParams {
    fn is_empty(&self) -> bool {
        *self == BuildParams::default()
    }

    /// Record the text index and its scope (`None` = no text index). The scope
    /// is also in the section itself, which is what readers act on; this is
    /// the build's account of the flags that produced it.
    pub(crate) fn set_text_index(&mut self, scope: Option<&rete_core::TextIndexScope>) {
        use rete_core::{TextIndexGraphs, TextIndexPredicates};
        self.text_index = scope.is_some();
        self.text_index_graphs = None;
        self.text_index_include.clear();
        self.text_index_exclude.clear();
        let Some(scope) = scope else {
            return;
        };
        if scope.graphs == TextIndexGraphs::All {
            self.text_index_graphs = Some("all".to_string());
        }
        match &scope.predicates {
            TextIndexPredicates::All => {}
            TextIndexPredicates::Only(l) => self.text_index_include = l.clone(),
            TextIndexPredicates::Except(l) => self.text_index_exclude = l.clone(),
        }
    }
}

/// Measured costs for the card's starter queries, with the context that makes
/// the timing interpretable (engine, transport, one machine).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct QueryCosts {
    /// What produced the numbers — without this the `debug_ms` column is the
    /// misleading form the issue warns about.
    pub context: CostContext,
    /// Per-query figures, in the card's query order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queries: Vec<QueryCost>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CostContext {
    /// Engine that ran the measurement (same binary as `builder`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    /// Transport the reads went through. Build-time measurement runs against
    /// the in-memory image with **logical range reads and no block cache**, so
    /// `bytes`/`requests` are pure layout properties — a block-caching remote
    /// client coalesces requests and rounds bytes up to blocks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    /// The caveat, stored where the numbers are.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One starter query's measured cost. Each run is **cold**: a fresh lazy open
/// per query, so the figures are what a stateless remote client would pay to
/// answer just that query.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct QueryCost {
    /// The starter query's stable id (`ov-triples`, …).
    pub id: String,
    /// Bytes fetched (logical range reads), open included. Portable.
    pub bytes: u64,
    /// Logical range requests, open included. Portable.
    pub requests: u64,
    /// Result rows (SELECT rows; 1 for ASK; constructed triples).
    pub rows: u64,
    /// Wall-clock milliseconds on the build machine — a debug reference, NOT a
    /// property of the file. Interpret with `context` and the byte figures
    /// ("N ms for B bytes in R requests" travels; "N ms" does not).
    pub debug_ms: u64,
}

/// A starter query the build measured and then did not ship.
///
/// The generator decides what a card *should* carry from the profile; the
/// build then runs each of those queries against the finished file, so for a
/// carded build emptiness stops being an inference and becomes an observation.
/// An observation of "this answers nothing" is the end of the argument: the
/// query is dropped from the card, and this row is what says so afterwards.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct DroppedQuery {
    /// The starter query's stable id (`sp-within`, …).
    pub id: String,
    /// What the run showed, in the words that justify the drop.
    pub why: String,
    /// True when the template behind `id` *claimed* the query could not come
    /// back empty and it did anyway. That is a defect in the generator's static
    /// rule, not a fact about the data — flagged here so it is findable in a
    /// published file long after the build log is gone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub contradicts_claim: bool,
}

impl BuildInfo {
    pub(crate) fn to_json_bytes(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("BuildInfo serializes")
    }

    pub(crate) fn from_json_bytes(b: &[u8]) -> anyhow::Result<Self> {
        serde_json::from_slice(b).map_err(|e| anyhow::anyhow!("malformed build info: {e}"))
    }
}

/// `rete-cli <version>` — the builder identity stamped into every build info.
pub(crate) fn builder_version() -> String {
    format!("rete-cli {}", env!("CARGO_PKG_VERSION"))
}

/// Human name of a section codec id from a file header — the truthful source
/// for the `codec` param is the built image itself, not a compile-time guess.
pub(crate) fn codec_name(codec: u8) -> &'static str {
    match codec {
        rete_core::CODEC_ZSTD => "zstd",
        rete_core::CODEC_NONE => "none",
        _ => "unknown",
    }
}

/// A fresh [`BuildInfo`] carrying the timestamp, builder, engine, host and
/// parameters — query costs are measured separately ([`measure_query_costs`])
/// because they need the finished image, and a [`Recorder`] adds what the run
/// measured.
pub(crate) fn new_build_info(params: BuildParams) -> BuildInfo {
    BuildInfo {
        schema: BUILD_INFO_SCHEMA,
        built_at: timestamp(),
        builder: Some(builder_version()),
        engine: Some(engine()),
        host: Some(host()),
        params,
        ..Default::default()
    }
}

/// How many inputs a record lists one by one; the rest are counted in
/// [`BuildInfo::inputs_omitted`]. Each listed input costs ~150 bytes of the
/// CARD-tier read.
pub(crate) const MAX_INPUTS_LISTED: usize = 64;

/// `RETE_BUILD_RECORD=reproducible`: leave every volatile fact out of the
/// record — the `run` block (wall times, peak RSS, the memory limit and CPUs
/// seen), the starter queries' `debug_ms` (zeroed: older readers require the
/// field), and the timestamps unless `SOURCE_DATE_EPOCH` pins them. Two such
/// builds of the same input by the same binary are byte-identical, build
/// record included. Without it, the comparison to make is with the build-info
/// section stripped (`rete_core::attach_build_info(image, &[])`), which is what
/// the test suite does.
pub(crate) fn reproducible() -> bool {
    std::env::var("RETE_BUILD_RECORD").is_ok_and(|v| v.trim() == "reproducible")
}

/// The timestamp to stamp now: [`now_rfc3339`], except that a reproducible
/// record carries one only when `SOURCE_DATE_EPOCH` provides it.
pub(crate) fn timestamp() -> Option<String> {
    if reproducible() && std::env::var("SOURCE_DATE_EPOCH").is_err() {
        None
    } else {
        Some(now_rfc3339())
    }
}

/// This binary, in full.
pub(crate) fn engine() -> Engine {
    Engine {
        name: "rete-cli".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        commit: env!("RETE_GIT_COMMIT").into(),
        target: env!("RETE_TARGET").into(),
        profile: env!("RETE_PROFILE").into(),
        features: rete_core::build_trace::enabled_features()
            .into_iter()
            .map(String::from)
            .collect(),
    }
}

/// OS and architecture — the whole of what the record says about the machine.
pub(crate) fn host() -> Host {
    Host {
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
    }
}

/// The last component of `path`, or `stdin` for `-`: what a record may say
/// about where an input came from. A directory names a user, a machine, a
/// project layout; the file name is what a reader needs to match it.
pub(crate) fn file_name(path: &str) -> String {
    if path == "-" {
        return "stdin".into();
    }
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.rsplit(['/', '\\']).next().unwrap_or(path).to_string())
}

/// One input as the build reads it: its byte count and running BLAKE3, fed by
/// the reader underneath any gzip decoder ([`super::build::open_reader_tapped`]),
/// so the digest is of the bytes as stored and costs no second pass. Clones
/// share the same state.
#[derive(Clone)]
pub(crate) struct InputTap {
    pub name: String,
    pub format: &'static str,
    pub bytes: std::rc::Rc<std::cell::Cell<u64>>,
    pub hasher: std::rc::Rc<std::cell::RefCell<blake3::Hasher>>,
    pub gzip: std::rc::Rc<std::cell::Cell<bool>>,
}

impl InputTap {
    pub(crate) fn new(path: &str, format: &'static str) -> Self {
        InputTap {
            name: file_name(path),
            format,
            bytes: Default::default(),
            hasher: Default::default(),
            gzip: Default::default(),
        }
    }

    fn record(&self) -> InputRecord {
        InputRecord {
            name: self.name.clone(),
            format: self.format.into(),
            compression: self.gzip.get().then(|| "gzip".into()),
            bytes: Some(self.bytes.get()),
            blake3: Some(self.hasher.borrow().finalize().to_hex().to_string()),
        }
    }
}

/// Everything a build measures about itself while it runs, for the record.
///
/// Created at the start of the command: it starts the core's
/// [`rete_core::build_trace`] lap timer on this thread, holds one
/// [`InputTap`] per input, and counts statements as the stream hands them over.
pub(crate) struct Recorder {
    collector: rete_core::build_trace::Collector,
    started: std::time::Instant,
    started_at: Option<String>,
    reproducible: bool,
    pub taps: Vec<InputTap>,
    pub statements_read: std::rc::Rc<std::cell::Cell<u64>>,
    /// The peak RSS of each phase measured on its own ([`Self::lap_measured`]).
    phase_peaks: std::cell::RefCell<Vec<(rete_core::build_trace::Phase, u64)>>,
    /// The sampler watching the phase being measured, if one is.
    sampler: std::cell::RefCell<Option<RssSampler>>,
}

/// Watches one phase's resident set size: `VmHWM` when the phase began, and a
/// thread reading `VmRSS` every few milliseconds until it ends.
///
/// The kernel's high-water mark is never reset (`/proc/self/clear_refs` would
/// do it, and would also make `/usr/bin/time` and every other observer of the
/// process report the wrong peak). So the phase's peak is the high-water mark
/// when the phase raised it — then it is exact — and the sampled maximum
/// otherwise, which a spike shorter than the interval can escape.
struct RssSampler {
    hwm_before: Option<u64>,
    max: std::sync::Arc<std::sync::atomic::AtomicU64>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RssSampler {
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

    fn start() -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;
        let max = Arc::new(AtomicU64::new(current_rss_bytes().unwrap_or(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = current_rss_bytes().is_some().then(|| {
            let (max, stop) = (max.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if let Some(rss) = current_rss_bytes() {
                        max.fetch_max(rss, Ordering::Relaxed);
                    }
                    std::thread::sleep(Self::INTERVAL);
                }
            })
        });
        RssSampler {
            hwm_before: peak_rss_bytes(),
            max,
            stop,
            thread,
        }
    }

    /// The phase's peak, or `None` where there is no `/proc`.
    fn finish(mut self) -> Option<u64> {
        use std::sync::atomic::Ordering;
        self.stop.store(true, Ordering::Relaxed);
        let sampled = self
            .thread
            .take()
            .and_then(|t| t.join().ok())
            .map(|()| self.max.load(Ordering::Relaxed))?;
        let sampled = sampled.max(current_rss_bytes().unwrap_or(0));
        match (self.hwm_before, peak_rss_bytes()) {
            (Some(before), Some(after)) if after > before => Some(after),
            _ => Some(sampled),
        }
    }
}

impl Drop for RssSampler {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Resident set size of this process now: `VmRSS` from `/proc/self/status`.
fn current_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

impl Recorder {
    /// Start timing now. `inputs` pairs each input path with its format.
    pub(crate) fn start(inputs: &[(&str, &'static str)]) -> Self {
        Recorder {
            collector: rete_core::build_trace::Collector::start(),
            started: std::time::Instant::now(),
            started_at: timestamp(),
            reproducible: reproducible(),
            taps: inputs.iter().map(|(p, f)| InputTap::new(p, f)).collect(),
            statements_read: Default::default(),
            phase_peaks: Default::default(),
            sampler: Default::default(),
        }
    }

    /// End `phase` (see [`rete_core::build_trace::lap`]).
    pub(crate) fn lap(&self, phase: rete_core::build_trace::Phase) {
        rete_core::build_trace::lap(phase);
    }

    /// Begin a phase whose own peak RSS is to be recorded ([`RssSampler`]).
    pub(crate) fn begin_measured(&self) {
        *self.sampler.borrow_mut() = Some(RssSampler::start());
    }

    /// End `phase` like [`Self::lap`] and record its own peak RSS (since
    /// [`Self::begin_measured`]); the next phase starts measured too.
    pub(crate) fn lap_measured(&self, phase: rete_core::build_trace::Phase) {
        self.lap(phase);
        if let Some(peak) = self
            .sampler
            .borrow_mut()
            .take()
            .and_then(RssSampler::finish)
        {
            self.phase_peaks.borrow_mut().push((phase, peak));
        }
        self.begin_measured();
    }

    /// The peak RSS recorded for `phase`, if it was measured on its own.
    fn phase_peak(&self, phase: rete_core::build_trace::Phase) -> Option<u64> {
        self.phase_peaks
            .borrow()
            .iter()
            .filter(|(p, _)| *p == phase)
            .map(|&(_, b)| b)
            .max()
    }

    /// Stamp the parts of `info` known when the build starts: `started_at`,
    /// and the inputs' names and formats.
    pub(crate) fn begin(&self, info: &mut BuildInfo) {
        info.started_at = self.started_at.clone();
        info.inputs_omitted = self.taps.len().saturating_sub(MAX_INPUTS_LISTED) as u64;
        info.inputs = self
            .taps
            .iter()
            .take(MAX_INPUTS_LISTED)
            .map(|t| InputRecord {
                name: t.name.clone(),
                format: t.format.into(),
                ..Default::default()
            })
            .collect();
    }

    /// The inputs as read so far.
    pub(crate) fn inputs(&self) -> Vec<InputRecord> {
        self.taps
            .iter()
            .take(MAX_INPUTS_LISTED)
            .map(InputTap::record)
            .collect()
    }

    /// The memory-bounded build's spill figures, from the core's counters.
    pub(crate) fn spill(&self) -> SpillRecord {
        let t = self.collector.snapshot();
        SpillRecord {
            chunks: t.counter("chunks").unwrap_or(0),
            merge_passes: t.counter("merge_passes").unwrap_or(0),
            sort_runs: t.counter("sort_runs").unwrap_or(0),
            scratch_peak_bytes: t.counter("scratch_peak_bytes").unwrap_or(0),
        }
    }

    /// This run's figures as of now, or `None` in a reproducible record.
    pub(crate) fn run(&self) -> Option<RunRecord> {
        if self.reproducible {
            return None;
        }
        Some(RunRecord {
            wall_ms: self.started.elapsed().as_millis() as u64,
            phases: self
                .collector
                .snapshot()
                .phases
                .into_iter()
                .map(|(p, d)| PhaseTime {
                    phase: p.as_str().into(),
                    ms: d.as_millis() as u64,
                    peak_rss_bytes: self.phase_peak(p),
                })
                .collect(),
            peak_rss_bytes: peak_rss_bytes(),
            memory_limit_bytes: memory_limit_bytes(),
            cpus: std::thread::available_parallelism()
                .ok()
                .map(|n| n.get() as u64),
        })
    }

    /// Whether this record leaves the volatile facts out.
    pub(crate) fn is_reproducible(&self) -> bool {
        self.reproducible
    }

    /// Seal `info` with what the run measured: the inputs, `built_at`, and
    /// (unless reproducible) the `run` block. Call it last — the wall time and
    /// peak RSS are read here.
    pub(crate) fn seal(&self, info: &mut BuildInfo) {
        if !self.taps.is_empty() {
            info.inputs = self.inputs();
        }
        info.built_at = timestamp();
        info.run = self.run();
        if self.reproducible {
            zero_debug_ms(info);
        }
    }
}

/// A reproducible record keeps the cost table's portable figures and zeroes
/// its one wall-clock column (removing it would break older readers, which
/// require the field).
pub(crate) fn zero_debug_ms(info: &mut BuildInfo) {
    if let Some(c) = info.query_costs.as_mut() {
        for q in &mut c.queries {
            q.debug_ms = 0;
        }
    }
}

/// Peak resident set size of this process: `VmHWM` from `/proc/self/status`.
/// `None` where there is no procfs.
pub(crate) fn peak_rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

/// The memory limit of this process's cgroup (v2 `memory.max`, else v1
/// `memory.limit_in_bytes`), `None` when unlimited or not on Linux.
pub(crate) fn memory_limit_bytes() -> Option<u64> {
    let v2 = std::fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|c| {
            c.lines()
                .find_map(|l| l.strip_prefix("0::").map(str::to_string))
        })
        .map(|rel| format!("/sys/fs/cgroup{}/memory.max", rel.trim_end_matches('/')));
    for path in v2
        .into_iter()
        .chain(["/sys/fs/cgroup/memory.max".to_string()])
        .chain(["/sys/fs/cgroup/memory/memory.limit_in_bytes".to_string()])
    {
        if let Ok(text) = std::fs::read_to_string(&path) {
            let t = text.trim();
            if t == "max" {
                return None;
            }
            if let Ok(n) = t.parse::<u64>() {
                // cgroup v1 reports "unlimited" as a page-rounded i64::MAX.
                return (n < (1u64 << 60)).then_some(n);
            }
        }
    }
    None
}

/// The record's serialized form, padded with spaces to `len` (JSON allows
/// trailing whitespace; every reader of the section — serde_json, the
/// browser's `JSON.parse`, Python's `json.loads` — accepts it).
fn padded(json: Vec<u8>, len: usize) -> Vec<u8> {
    let mut out = json;
    if out.len() < len {
        out.resize(len, b' ');
    }
    out
}

impl BuildInfo {
    /// A copy of this record with every field the run has **not yet measured**
    /// at its widest serialization: the size the sealed record can grow to and
    /// no more.
    ///
    /// A record is written into the file before the run it describes has
    /// finished — an in-RAM build's and a copying re-card's last phase is
    /// writing the file — so the writer reserves this many bytes and
    /// [`patch_build_info`] fills them in place once the run is over: no second
    /// copy of the file, and the wall time and peak RSS cover the write
    /// itself. Every measured number is a `u64` and every measured string has
    /// a bounded width (a phase name from a closed set), so the bound is
    /// exact, not a guess.
    ///
    /// `pending` says what is still to come: [`Pending::Write`], the final
    /// write, so the run's phases are those already timed plus `write`. (The
    /// memory-bounded build does not reserve: its record goes after the data
    /// with its card, sealed, once everything else is on disk.) A
    /// reproducible record ([`reproducible`]) has no run block to reserve.
    /// Fields already final are left as they are, which is what keeps the
    /// padding small.
    pub(crate) fn widest(&self, pending: Pending, rec: &Recorder) -> BuildInfo {
        use rete_core::build_trace::Phase;
        const MAX: u64 = u64::MAX;
        let phases: Vec<Phase> = match pending {
            Pending::Write => {
                let mut p: Vec<Phase> = rec
                    .collector
                    .snapshot()
                    .phases
                    .iter()
                    .map(|(p, _)| *p)
                    .collect();
                if !p.contains(&Phase::Write) {
                    p.push(Phase::Write);
                }
                p
            }
        };
        let widest_run = || RunRecord {
            wall_ms: MAX,
            phases: phases
                .iter()
                .map(|p| PhaseTime {
                    phase: p.as_str().into(),
                    ms: MAX,
                    // Only a phase already measured on its own has a peak.
                    peak_rss_bytes: rec.phase_peak(*p).map(|_| MAX),
                })
                .collect(),
            peak_rss_bytes: Some(MAX),
            memory_limit_bytes: Some(MAX),
            cpus: Some(MAX),
        };
        let run = !rec.is_reproducible();
        let mut w = self.clone();
        if let Some(r) = w.recarded.as_mut() {
            // A re-card measures only itself; the build's own figures are
            // copied as they are and do not change.
            if run {
                r.run = Some(widest_run());
            }
            return w;
        }
        if run {
            w.run = Some(widest_run());
        }
        w
    }

    /// [`widest`](Self::widest), serialized: the bytes to reserve.
    pub(crate) fn reserve_bytes(&self, pending: Pending, rec: &Recorder) -> Vec<u8> {
        self.widest(pending, rec).to_json_bytes()
    }
}

/// What a record still has to measure when its section is reserved.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Pending {
    /// Only the final write: the inputs, counts and costs are final.
    Write,
}

/// Write a finished in-memory `image` to `path` with `info` spliced in as its
/// build-info section — streamed from the image, so the file is never copied
/// in memory (what [`rete_core::attach_build_info`] does, and on a multi-GB
/// in-RAM build, a second image at the very end of the build is the peak).
/// The bytes are exactly `attach_build_info(image, info)`'s.
pub(crate) fn write_image_with_build_info(
    path: &std::path::Path,
    image: &[u8],
    info: &[u8],
) -> anyhow::Result<u64> {
    use std::io::Write;
    let plan = rete_core::plan_build_info(image, image.len() as u64, info.len() as u64)?;
    let mut out = std::io::BufWriter::with_capacity(8 << 20, std::fs::File::create(path)?);
    out.write_all(&plan.header)?;
    out.write_all(&image[rete_core::format::HEADER_LEN..plan.insert as usize])?;
    out.write_all(info)?;
    out.write_all(&image[plan.tail_start as usize..])?;
    out.flush()?;
    Ok(plan.new_len)
}

/// The statement counts of a finished build. `read` is what the parser handed
/// over, `inferred` what `--materialize` added; `stats` is the written file.
pub(crate) fn counts(
    stats: &rete_core::ingest::BuildStats,
    read: u64,
    inferred: Option<u64>,
    iri: &rete_core::IriReport,
) -> Counts {
    let unique = stats.statements as u64;
    let default = stats.default_triples as u64;
    Counts {
        statements_read: read,
        statements_inferred: inferred,
        statements_unique: unique,
        duplicates_dropped: (read + inferred.unwrap_or(0)).saturating_sub(unique),
        default_graph_triples: default,
        named_graphs: stats.named_graphs as u64,
        named_graph_quads: unique.saturating_sub(default),
        terms: stats.terms as u64,
        invalid_iris: (!iri.is_empty()).then(|| InvalidIris {
            statements: iri.statements(),
            occurrences: iri.occurrences(),
            repairable: iri.repairable(),
            unrepairable: iri.unrepairable(),
        }),
    }
}

/// Fill a file's reserved build-info section in place with `info`, padded with
/// spaces to the reserved length. One small write at the section's offset; the
/// rest of the file is not touched, and the content hash does not cover the
/// section. Falls back to [`write_build_info_streaming`] (a full rewrite) if the
/// record somehow outgrew its reservation — which [`BuildInfo::widest`] is
/// there to make impossible.
pub(crate) fn patch_build_info(path: &std::path::Path, info: &[u8]) -> anyhow::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;
    let mut head = vec![0u8; rete_core::format::HEADER_LEN];
    f.read_exact(&mut head)?;
    let h = rete_core::format::Header::from_bytes(&head)?;
    let reserved = h.build_info_len as usize;
    if reserved == 0 || info.len() > reserved {
        drop(f);
        eprintln!(
            "warning: the build record ({} bytes) outgrew its reservation ({reserved}); \
             rewriting the file to fit it",
            info.len()
        );
        write_build_info_streaming(path, info)?;
        return Ok(());
    }
    f.seek(SeekFrom::Start(h.build_info_offset))?;
    f.write_all(&padded(info.to_vec(), reserved))?;
    f.sync_all()?;
    Ok(())
}

/// RFC 3339 UTC (`2026-08-04T12:34:56Z`), honoring `SOURCE_DATE_EPOCH` so a
/// reproducibility-minded pipeline can pin it. No chrono dependency: days →
/// civil date via the standard Gregorian algorithm.
pub(crate) fn now_rfc3339() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0)
        });
    epoch_to_rfc3339(secs)
}

/// Format an epoch second as RFC 3339 UTC. (Howard Hinnant's `civil_from_days`.)
pub(crate) fn epoch_to_rfc3339(secs: i64) -> String {
    let (y, mo, d, h, m, s) = civil_from_epoch(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// An epoch second as UTC `(year, month, day, hour, minute, second)`.
pub(crate) fn civil_from_epoch(secs: i64) -> (i64, i64, i64, i64, i64, i64) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, m, s) = (rem / 3600, (rem / 60) % 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d, h, m, s)
}

/// An owning in-memory [`RangeReader`] — `open_ranged_lazy` needs `'static`,
/// which the borrowing `SliceReader` cannot provide.
struct ImageReader(std::sync::Arc<Vec<u8>>);

impl RangeReader for ImageReader {
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let start = offset as usize;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= self.0.len())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "range out of bounds")
            })?;
        Ok(self.0[start..end].to_vec())
    }
}

/// A [`RangeReader`] that refuses to serve more than `budget` bytes.
///
/// The measurement has to actually run the query, and on a remote file that
/// means paying for every byte it reads. `switzerland-fedlex`'s own build
/// record says eight of its ten starter queries read ~1.02 GB each — measuring
/// that card over HTTP unguarded is an 8 GB download nobody asked for. Past the
/// budget the read fails, the query fails with it, and the failure is reported
/// **with the bytes spent**: "costs more than N MB" is itself a usable answer.
///
/// Two ways to count. [`new`](Self::new) charges from the first byte — what
/// `card-audit --max-mb` promises a remote caller. [`after_open`](Self::after_open)
/// charges only what the query reads once the file is open, which is what the
/// starter budget means: opening a 26 GB file reads ~44 MB of section
/// directories whatever is asked of it, a cost every client pays once per
/// session and no starter query can change, so charging it to the query would
/// make every starter on a large enough file "too expensive".
pub(crate) struct BudgetReader<R> {
    inner: R,
    state: std::sync::Arc<BudgetState>,
}

struct BudgetState {
    budget: u64,
    /// Reads are refused once `spent` would pass this; `u64::MAX` until a
    /// [`Leash`] arms it.
    limit: std::sync::atomic::AtomicU64,
    spent: std::sync::atomic::AtomicU64,
}

/// Arms an [`BudgetReader::after_open`] budget: from the moment of the call,
/// the reader serves at most `budget` more bytes.
pub(crate) struct Leash(std::sync::Arc<BudgetState>);

impl Leash {
    pub(crate) fn arm(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let spent = self.0.spent.load(Relaxed);
        self.0
            .limit
            .store(spent.saturating_add(self.0.budget), Relaxed);
    }
}

impl<R: RangeReader> BudgetReader<R> {
    pub(crate) fn new(inner: R, budget: u64) -> Self {
        Self {
            inner,
            state: std::sync::Arc::new(BudgetState {
                budget,
                limit: std::sync::atomic::AtomicU64::new(budget),
                spent: std::sync::atomic::AtomicU64::new(0),
            }),
        }
    }

    /// A budget of `budget` bytes that starts counting when the returned
    /// [`Leash`] is armed — after the open, see the type note.
    pub(crate) fn after_open(inner: R, budget: u64) -> (Self, Leash) {
        let state = std::sync::Arc::new(BudgetState {
            budget,
            limit: std::sync::atomic::AtomicU64::new(u64::MAX),
            spent: std::sync::atomic::AtomicU64::new(0),
        });
        (
            Self {
                inner,
                state: state.clone(),
            },
            Leash(state),
        )
    }

    /// Charged BEFORE the read, so a single oversized range cannot slip
    /// through by being the one that would have crossed the line.
    fn charge(&self, len: u64) -> std::io::Result<()> {
        use std::sync::atomic::Ordering::Relaxed;
        let spent = self.state.spent.fetch_add(len, Relaxed) + len;
        if spent > self.state.limit.load(Relaxed) {
            return Err(std::io::Error::other(format!(
                "byte budget exhausted: this query wanted more than {} bytes",
                self.state.budget
            )));
        }
        Ok(())
    }
}

impl<R: RangeReader> RangeReader for BudgetReader<R> {
    fn len(&self) -> u64 {
        self.inner.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        self.charge(len)?;
        self.inner.read_at(offset, len)
    }

    fn read_many(&self, ranges: &[(u64, u64)]) -> std::io::Result<Vec<Vec<u8>>> {
        self.charge(ranges.iter().map(|&(_, l)| l).sum())?;
        self.inner.read_many(ranges)
    }

    fn concurrency(&self) -> usize {
        self.inner.concurrency()
    }
}

/// What one cold run of one starter query produced: what it cost, whether it
/// finished, and whether it was worth anything.
///
/// `error` and `useless` answer different questions and are both needed.
/// `error` is *did the run complete* — the file did not open, the query failed,
/// a byte budget stopped it — and `cost` then still holds the bytes and
/// requests spent getting that far, which is what makes "costs more than N MB"
/// a usable answer for `card-audit`. `useless` is *is the answer worth
/// shipping*, which is what `rete build` drops on. A run that did not complete
/// is both.
pub(crate) struct Measured {
    pub cost: QueryCost,
    /// Set when the run did not finish. See the type note above.
    pub error: Option<String>,
    /// `None` when the query answered with something. `Some(reason)` when the
    /// run showed the query is worthless on this file — the reason is the text
    /// the build prints and stores in [`DroppedQuery::why`].
    pub useless: Option<String>,
    /// Rows came back, and every one of them binds nothing at all — the
    /// un-grouped aggregate over an empty solution sequence. SPARQL returns
    /// exactly one row there whatever happens, so no row count catches it, and
    /// the row carries no answer. Only a run can see this; a card cannot.
    ///
    /// It is also the one shape of uselessness that contradicts **no**
    /// template: the non-emptiness claims are about row *count*, and
    /// `NonEmpty::Aggregate` says in so many words that the row's values may be
    /// unbound. So `useless.is_some() && !vacuous` is exactly "measured zero
    /// rows", which is what can prove a claim wrong.
    pub vacuous: bool,
}

/// What a run's *result* decides, before its cost is tallied — the output of
/// [`grade`], folded into a [`Measured`] by [`measure_one`].
struct Graded {
    rows: u64,
    error: Option<String>,
    useless: Option<String>,
    vacuous: bool,
}

impl Graded {
    /// A run that never produced a result. It is an error, and — since a query
    /// that cannot be run cannot be shown to answer — it is useless too.
    fn failed(why: String) -> Self {
        Self {
            rows: 0,
            error: Some(why.clone()),
            useless: Some(why),
            vacuous: false,
        }
    }
}

/// Grade one query's result. Two things make a starter query worthless, and a
/// run can see both:
///
/// * **no rows** — the failure the query library exists to prevent ("a starter
///   query that answers nothing is worse than no starter query: the reader
///   concludes the *file* is broken");
/// * **rows that bind nothing** — the un-grouped aggregate over an empty
///   solution sequence. SPARQL returns exactly one row there whatever happens,
///   so no row count can catch it, and the row carries zero information:
///   `sp-bbox` over a file where no subject holds both `wgs:lat` and
///   `wgs:long` returns one row of four unbound variables. Its template says
///   the card "cannot do better" than that — the *measurement* can.
///
/// A `COUNT` is not caught by either: it binds, to 0. See the module notes on
/// `cmp-coverage` in `docs/dataset-cards.md`.
fn grade(out: &QueryOutput) -> Graded {
    let answers = |rows: u64| Graded {
        rows,
        error: None,
        useless: None,
        vacuous: false,
    };
    let empty = |why: &str| Graded {
        rows: 0,
        error: None,
        useless: Some(why.to_string()),
        vacuous: false,
    };
    match out {
        QueryOutput::Select(_, rows) if rows.is_empty() => empty("measured 0 rows on the file"),
        QueryOutput::Select(_, rows) if rows.iter().all(|b| b.is_empty()) => Graded {
            rows: rows.len() as u64,
            error: None,
            useless: Some(format!(
                "measured {} row(s) binding no variable at all — an aggregate over an empty \
                 solution sequence, which is a row without an answer in it",
                rows.len()
            )),
            vacuous: true,
        },
        QueryOutput::Select(_, rows) => answers(rows.len() as u64),
        QueryOutput::Ask(false) => empty("measured ASK false on the file"),
        QueryOutput::Ask(true) => answers(1),
        QueryOutput::Construct(ts) if ts.is_empty() => empty("constructed no triples on the file"),
        QueryOutput::Construct(ts) => answers(ts.len() as u64),
        // `QueryOutput` is non-exhaustive; the library emits only the forms
        // above, so anything else is a shape this run cannot vouch for.
        other => Graded::failed(format!("returned an unexpected result form: {other:?}")),
    }
}

/// **The** measurement, in one place: open the file cold through `reader`, run
/// `q`, tally what it cost, and grade what came back.
///
/// Everything that reports a query cost goes through here — `rete build` when
/// it writes the build record (and when it decides which starter queries the
/// card may keep), `rete card-audit --measure` when it re-measures a file that
/// already exists. Two implementations would drift, and a cost that drifts from
/// the cost a build recorded cannot be compared against it, which is the only
/// thing anyone wants to do with it. The same goes for the grade: the audit
/// reports emptiness with the words the build dropped on.
///
/// `reader` is a fresh counting reader per query on purpose: the figures are
/// what a **stateless** client pays to answer just this query, with no state
/// carried over from the last one. What the reader is wrapped around — an
/// in-memory image, a file handle, an HTTP range client — decides the transport
/// but not the arithmetic.
fn measure_one<R: RangeReader + Send + Sync + 'static>(
    reader: std::sync::Arc<CountingReader<R>>,
    q: &ExampleQuery,
    leash: Option<Leash>,
) -> Measured {
    let start = std::time::Instant::now();
    let open = Rete::open_ranged_lazy(reader.clone());
    // The open is paid whatever the query; a starter budget counts from here.
    if let (Ok(_), Some(l)) = (&open, &leash) {
        l.arm();
    }
    let graded = match open {
        Ok(rete) => match eval_query(&rete, &q.sparql) {
            // A lazy open turns a failed range read into an empty tile and a
            // flag, so the result of a run whose reads failed — a byte budget
            // ran out, a request errored — is a partial answer that looks
            // complete. It is a failed run, and saying so is what makes
            // "costs more than N MB" an answer rather than a wrong row count.
            Ok(_) if rete.index_incomplete() => Graded::failed(format!(
                "did not finish: a range read was refused after {} bytes (byte budget \
                 exhausted, or the source failed)",
                reader.bytes_read()
            )),
            Ok(result) => grade(&result),
            Err(e) => Graded::failed(format!("failed to run: {e}")),
        },
        Err(e) => Graded::failed(format!("the file did not open: {e}")),
    };
    Measured {
        cost: QueryCost {
            id: q.id.clone(),
            bytes: reader.bytes_read(),
            requests: reader.requests(),
            rows: graded.rows,
            debug_ms: start.elapsed().as_millis() as u64,
        },
        error: graded.error,
        useless: graded.useless,
        vacuous: graded.vacuous,
    }
}

/// Measure a starter query cold through a caller-supplied transport. The
/// closure is called once per query and must hand back a **fresh** reader —
/// a new file handle, a new HTTP client — so no query is warmed by the last.
pub(crate) fn measure_query<R, F>(open: F, q: &ExampleQuery) -> anyhow::Result<Measured>
where
    R: RangeReader + Send + Sync + 'static,
    F: FnOnce() -> anyhow::Result<R>,
{
    Ok(measure_one(
        std::sync::Arc::new(CountingReader::new(open()?)),
        q,
        None,
    ))
}

/// Measure one **starter** query cold over `src`, inside the starter byte
/// budget ([`rete_core::card::STARTER_MAX_BYTES`]) counted from the moment the
/// file is open — what `rete build` and `rete recard` decide a card's queries
/// on. The reported `bytes` still include the open, as every cost figure does.
pub(crate) fn measure_starter<R: RangeReader + Send + Sync + 'static>(
    src: R,
    q: &ExampleQuery,
) -> Measured {
    let (reader, leash) = BudgetReader::after_open(src, rete_core::card::STARTER_MAX_BYTES);
    measure_one(
        std::sync::Arc::new(CountingReader::new(reader)),
        q,
        Some(leash),
    )
}

/// Measure every starter query **cold** against the finished image: a fresh
/// lazy ranged open per query (what a stateless remote client pays), counting
/// logical range reads. `bytes`/`requests` are deterministic properties of
/// layout + query; `debug_ms` is this machine's reference timing.
///
/// The image measured here lacks the build-info section itself, but that does
/// not skew the figures: inserting it only shifts later sections by a constant,
/// and logical reads are section-relative, so sizes and counts are invariant.
///
/// The run also **grades** each answer ([`Measured::useless`]). That costs
/// nothing extra — the query had to run to be costed — and it is the whole
/// point: for a carded build, "does this query answer?" is measured, not
/// reasoned about.
///
/// Every run goes through [`measure_starter`]'s budget of
/// [`rete_core::card::STARTER_MAX_BYTES`] past the open: a starter query that
/// asks for more is a query that does not belong in a card, and the run stops
/// there rather than finishing it. That cap is what keeps this phase from being the build's
/// memory peak — measuring the old unbounded starters against the
/// OpenCitations sample peaked at 8.7 GiB, above the 7.3 GiB of serializing it.
pub(crate) fn measure_query_costs(
    image: std::sync::Arc<Vec<u8>>,
    queries: &[ExampleQuery],
) -> Vec<Measured> {
    queries
        .iter()
        .map(|q| {
            let m = measure_starter(ImageReader(image.clone()), q);
            rete_core::build_progress::advance(1);
            m
        })
        .collect()
}

/// Sort a measurement into the figures to record and the queries to drop, and
/// remove the dropped ones from `card` — the one decision `rete build` and
/// `rete recard` both make once every starter query has been run.
///
/// A query is dropped when the run shows it is worthless on this file: it
/// answered nothing, it answered rows that bind nothing, or it did not finish
/// inside [`rete_core::card::STARTER_MAX_BYTES`]. Only the first can
/// contradict a template — the non-emptiness claims are about row *count*;
/// `NonEmpty::Aggregate` says outright that the row's values may be unbound,
/// and a run that ran out of budget says the query is too expensive to be a
/// starter, nothing about its rows. The claim is read off the body that
/// shipped ([`rete_core::card::claim_of_query`]), since a template's bounded
/// form claims less than its exact one.
pub(crate) fn keep_answering(
    card: &mut super::card::DatasetCard,
    measured: Vec<Measured>,
) -> (Vec<QueryCost>, Vec<DroppedQuery>) {
    use rete_core::card::{claim_of_query, Claim};
    let mut kept = Vec::with_capacity(measured.len());
    let mut dropped = Vec::new();
    for m in measured {
        match m.useless {
            None => kept.push(m.cost),
            Some(why) => {
                let sparql = card
                    .queries
                    .iter()
                    .find(|q| q.id == m.cost.id)
                    .map(|q| q.sparql.as_str())
                    .unwrap_or_default();
                let contradicts_claim = !m.vacuous
                    && m.error.is_none()
                    && claim_of_query(&m.cost.id, sparql) == Claim::CannotBeEmpty;
                dropped.push(DroppedQuery {
                    id: m.cost.id,
                    why,
                    contradicts_claim,
                });
            }
        }
    }
    if !dropped.is_empty() {
        eprintln!(
            "dropping {} starter quer{} the measurement showed useless on this file:",
            dropped.len(),
            if dropped.len() == 1 { "y" } else { "ies" }
        );
        for d in &dropped {
            eprintln!("  {:<14} {}", d.id, d.why);
            if d.contradicts_claim {
                // The static rule and the measurement disagree, and the
                // measurement is the one that ran. A template that declares its
                // emitted query cannot be empty and then measures zero is a bug
                // in `card_queries.rs`, not a property of this dataset.
                eprintln!(
                    "  {:<14} ^ GENERATOR DEFECT: its template claims this query cannot come back                      empty. Fix the rule in crates/rete-core/src/card_queries.rs.",
                    ""
                );
            }
        }
        let ids: std::collections::BTreeSet<&str> = dropped.iter().map(|d| d.id.as_str()).collect();
        card.queries.retain(|q| !ids.contains(q.id.as_str()));
    }
    (kept, dropped)
}

/// Wrap the kept per-query figures in the context that makes them
/// interpretable.
pub(crate) fn query_costs(queries: Vec<QueryCost>) -> QueryCosts {
    QueryCosts {
        context: cost_context(IN_MEMORY_TRANSPORT),
        queries,
    }
}

/// How `rete build` reads the file it has just written.
pub(crate) const IN_MEMORY_TRANSPORT: &str =
    "local in-memory image; cold lazy open per query; logical range reads, no block cache";

/// The caveat that travels with every cost figure — stored in the file, and
/// printed by anything that shows one. One string, so the two cannot diverge.
pub(crate) const COST_NOTE: &str =
    "bytes/requests are properties of file layout + query (portable); debug_ms is one machine's \
     reference timing, not a guarantee";

/// The context every set of cost figures travels with. `transport` is the one
/// part a caller varies, and it is not optional: "1.4 MB in 76 requests" means
/// nothing without knowing what did the requesting.
pub(crate) fn cost_context(transport: &str) -> CostContext {
    CostContext {
        engine: Some(builder_version()),
        transport: Some(transport.to_string()),
        note: Some(COST_NOTE.to_string()),
    }
}

/// How big a chunk the streaming rewriter moves at a time. 4 MiB is enough to
/// keep a spinning disk streaming and small enough that peak RSS stays flat
/// whatever the file weighs.
const REWRITE_CHUNK: usize = 4 << 20;

/// Write `info` into `path`'s build-info section, **streaming** — the file is
/// never held in memory, so a 17 GB `.rete` costs [`REWRITE_CHUNK`] of RAM and
/// one pass of I/O rather than 34 GB of `Vec`. (The command that calls this
/// still has to *run* the queries first, and that is where its memory goes; the
/// copy below is the cheap half.)
///
/// Why a rewrite at all: the section sits immediately after the card, near the
/// front of the file, so growing it shifts every byte behind it. The identity
/// is preserved (build info is outside the content hash — see
/// [`rete_core::plan_build_info`]) but the bytes are not, which is the trade
/// this function exists to make explicit rather than hide.
///
/// Crash-safety: the new image is built beside the target and renamed over it
/// only once it is complete and fsynced, so an interrupted run leaves the
/// original intact.
///
/// Returns the new file length.
pub(crate) fn write_build_info_streaming(
    path: &std::path::Path,
    info: &[u8],
) -> anyhow::Result<u64> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut src = std::fs::File::open(path)?;
    let file_len = src.metadata()?.len();
    let mut head = vec![0u8; rete_core::format::HEADER_LEN];
    src.read_exact(&mut head)?;
    let before = rete_core::format::Header::from_bytes(&head)?;
    let plan = rete_core::plan_build_info(&head, file_len, info.len() as u64)?;

    let tmp = path.with_extension("rete.costs-tmp");
    {
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        out.write_all(&plan.header)?;
        // The `[HEADER_LEN, insert)` prefix is the card, which is KBs; the
        // `[tail_start, len)` tail is the rest of the file, which is not.
        let mut copy = |from: u64, to: u64, out: &mut dyn Write| -> anyhow::Result<()> {
            src.seek(SeekFrom::Start(from))?;
            let mut left = to - from;
            let mut buf = vec![0u8; REWRITE_CHUNK.min(left.max(1) as usize)];
            while left > 0 {
                let n = buf.len().min(left as usize);
                src.read_exact(&mut buf[..n])?;
                out.write_all(&buf[..n])?;
                left -= n as u64;
            }
            Ok(())
        };
        copy(rete_core::format::HEADER_LEN as u64, plan.insert, &mut out)?;
        out.write_all(info)?;
        copy(plan.tail_start, file_len, &mut out)?;
        out.flush()?;
        out.into_inner()
            .map_err(|e| anyhow::anyhow!("flushing {}: {e}", tmp.display()))?
            .sync_all()?;
    }
    drop(src);

    // Read the new header back before committing: the section must be there,
    // and the content hash — the file's identity — must be the byte-for-byte
    // same 16 bytes it was. A rewrite that moved it is a rewrite that produced
    // a different file, and the caller was promised it would not.
    let mut written_head = vec![0u8; rete_core::format::HEADER_LEN];
    std::fs::File::open(&tmp)?.read_exact(&mut written_head)?;
    let after = rete_core::format::Header::from_bytes(&written_head)
        .map_err(|e| anyhow::anyhow!("the rewritten file has no readable header: {e}"))?;
    if after.content_hash != before.content_hash {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("refusing to commit: the rewrite changed the content hash");
    }
    if after.build_info_len != info.len() as u64 {
        let _ = std::fs::remove_file(&tmp);
        anyhow::bail!("refusing to commit: the rewritten build-info section is the wrong length");
    }
    std::fs::rename(&tmp, path)?;
    Ok(plan.new_len)
}

/// `1234567` → `1,234,567`.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Bytes as a human size (binary units, one decimal).
fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// Milliseconds as `850 ms`, `12.3 s` or `4 min 05 s`.
fn human_ms(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 120_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        let s = ms / 1000;
        format!("{} min {:02} s", s / 60, s % 60)
    }
}

/// The record's flags as the command line that would reproduce them, defaults
/// included where the record states them (it records effective values).
fn command_line(p: &BuildParams) -> String {
    let mut f: Vec<String> = vec![format!(
        "rete {}",
        p.command
            .as_deref()
            .map(|c| c.trim_end_matches(" --memory-budget-mb"))
            .unwrap_or("build")
    )];
    if let Some(v) = &p.format {
        f.push(format!("--format {v}"));
    }
    if let Some(mb) = p.memory_budget_mb {
        f.push(format!("--memory-budget-mb {mb}"));
    }
    if let Some(n) = p.permutations {
        f.push(format!("--permutations {n}"));
    }
    if let Some(g) = &p.named_graphs {
        f.push(format!("--named-graphs {g}"));
    }
    if p.collapse_graphs == Some(true) {
        f.push("--collapse-graphs".into());
    }
    if p.no_pyramid {
        f.push("--no-pyramid".into());
    }
    if let Some(a) = &p.pyramid_algo {
        f.push(format!("--pyramid-algo {a}"));
    }
    if let Some(t) = &p.type_predicate {
        f.push(format!("--type-predicate {t}"));
    }
    if p.label_index {
        f.push("--label-index".into());
    }
    if p.no_schema {
        f.push("--no-schema".into());
    }
    if p.text_index {
        f.push("--text-index".into());
    }
    if let Some(g) = &p.text_index_graphs {
        f.push(format!("--text-index-graphs {g}"));
    }
    for iri in &p.text_index_include {
        f.push(format!("--text-index-include {iri}"));
    }
    for iri in &p.text_index_exclude {
        f.push(format!("--text-index-exclude {iri}"));
    }
    if p.materialize {
        f.push("--materialize".into());
    }
    if p.reason {
        f.push("--reason".into());
    }
    if p.strict == Some(true) {
        f.push("--strict".into());
    }
    if let Some(s) = &p.quoted_triple_syntax {
        f.push(format!("--quoted-triple-syntax {s}"));
    }
    if let Some(w) = &p.write {
        f.push(format!("--tile-kib {}", w.tile_kib));
        f.push(format!("--dict-chunk-kib {}", w.dict_chunk_kib));
        f.push(format!("--zstd-level {}", w.zstd_level));
        if w.frequent_objects > 0 {
            f.push(format!("--frequent-objects {}", w.frequent_objects));
        }
    }
    if let Some(c) = &p.card_file {
        f.push(format!("--card-file {}", c.name));
    }
    if p.card_costs == Some(false) {
        f.push("--no-card-costs".into());
    }
    f.join(" ")
}

fn short_hash(h: &str) -> &str {
    &h[..h.len().min(16)]
}

fn format_run(out: &mut String, run: &RunRecord, indent: &str) {
    use std::fmt::Write;
    let mut line = human_ms(run.wall_ms);
    if let Some(c) = run.cpus {
        let _ = write!(line, " on {c} CPU(s)");
    }
    let _ = writeln!(out, "{indent}{:<12}: {line}", "wall time");
    let peak = run
        .peak_rss_bytes
        .map(human_bytes)
        .unwrap_or_else(|| "not measured (no /proc on this OS)".into());
    let limit = run
        .memory_limit_bytes
        .map(|l| format!(" of a {} limit", human_bytes(l)))
        .unwrap_or_else(|| " (no memory limit)".into());
    let _ = writeln!(out, "{indent}{:<12}: {peak}{limit}", "peak RAM");
    if !run.phases.is_empty() {
        let phases: Vec<String> = run
            .phases
            .iter()
            .map(|p| match p.peak_rss_bytes {
                Some(b) => format!("{} {} (peak {})", p.phase, human_ms(p.ms), human_bytes(b)),
                None => format!("{} {}", p.phase, human_ms(p.ms)),
            })
            .collect();
        // Several per line: a record has up to a dozen phases.
        for (i, chunk) in phases.chunks(4).enumerate() {
            let label = if i == 0 { "phases" } else { "" };
            let sep = if i == 0 { ":" } else { " " };
            let _ = writeln!(out, "{indent}{label:<12}{sep} {}", chunk.join(" · "));
        }
    }
}

/// Render the build record for the human `rete card` view: how the file was
/// made (command line, engine, host, inputs), what came of it (statement
/// counts), and what it cost (wall time per phase, peak RAM, spill).
pub(crate) fn format_build_info(info: &BuildInfo) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    const I: &str = "      ";
    let _ = writeln!(out, "  build:");
    let opt = |out: &mut String, label: &str, v: &Option<String>| {
        if let Some(v) = v {
            let _ = writeln!(out, "{I}{label:<12}: {v}");
        }
    };
    match (&info.started_at, &info.built_at) {
        (Some(s), Some(b)) => {
            let _ = writeln!(out, "{I}{:<12}: {b} (started {s})", "built at");
        }
        _ => opt(&mut out, "built at", &info.built_at),
    }
    match &info.engine {
        Some(e) => {
            let mut line = format!("{} {}", e.name, e.version);
            let _ = write!(line, ", commit {}", short_hash(&e.commit));
            let _ = write!(line, ", {} ({})", e.target, e.profile);
            if !e.features.is_empty() {
                let _ = write!(line, "; features: {}", e.features.join(", "));
            }
            let _ = writeln!(out, "{I}{:<12}: {line}", "engine");
        }
        None => opt(&mut out, "builder", &info.builder),
    }
    if let Some(h) = &info.host {
        let _ = writeln!(out, "{I}{:<12}: {} {}", "host", h.os, h.arch);
    }
    let p = &info.params;
    if !p.is_empty() {
        let _ = writeln!(out, "{I}{:<12}: {}", "command", command_line(p));
        if let Some(c) = &p.card_file {
            let _ = writeln!(
                out,
                "{I}{:<12}: {} ({} B, blake3 {}…)",
                "card file",
                c.name,
                thousands(c.bytes),
                short_hash(&c.blake3)
            );
        }
        if let Some(w) = &p.write {
            let _ = writeln!(
                out,
                "{I}{:<12}: {}, {} KiB tiles, {} KiB dictionary chunks, zstd level {} ({} frames)",
                "writer",
                p.codec.as_deref().unwrap_or("?"),
                w.tile_kib,
                w.dict_chunk_kib,
                w.zstd_level,
                w.zstd_frame
            );
        } else if let Some(c) = &p.codec {
            let _ = writeln!(out, "{I}{:<12}: {c}", "codec");
        }
        if let Some(n) = p.card_top_n {
            let _ = writeln!(out, "{I}{:<12}: {n}", "card top-N");
        }
    }
    for (n, i) in info.inputs.iter().enumerate() {
        let mut line = format!("{} ({}", i.name, i.format);
        if let Some(c) = &i.compression {
            let _ = write!(line, ", {c}");
        }
        line.push(')');
        if let Some(b) = i.bytes {
            let _ = write!(line, ", {} B", thousands(b));
        }
        if let Some(h) = &i.blake3 {
            let _ = write!(line, ", blake3 {}…", short_hash(h));
        }
        let label = if n == 0 { "inputs" } else { "" };
        let sep = if n == 0 { ":" } else { " " };
        let _ = writeln!(out, "{I}{label:<12}{sep} {line}");
    }
    if info.inputs_omitted > 0 {
        let _ = writeln!(
            out,
            "{I}{:<12}  … and {} more input(s), not listed",
            "", info.inputs_omitted
        );
    }
    if let Some(c) = &info.counts {
        let mut line = format!("{} read", thousands(c.statements_read));
        if let Some(n) = c.statements_inferred {
            let _ = write!(line, " + {} inferred", thousands(n));
        }
        let _ = write!(
            line,
            ", {} unique ({} duplicate(s) dropped)",
            thousands(c.statements_unique),
            thousands(c.duplicates_dropped)
        );
        let _ = writeln!(out, "{I}{:<12}: {line}", "statements");
        let _ = writeln!(
            out,
            "{I}{:<12}  {} in the default graph, {} in {} named graph(s); {} terms",
            "",
            thousands(c.default_graph_triples),
            thousands(c.named_graph_quads),
            thousands(c.named_graphs),
            thousands(c.terms)
        );
        if let Some(b) = &c.invalid_iris {
            let _ = writeln!(
                out,
                "{I}{:<12}: {} occurrence(s) in {} statement(s), stored verbatim ({} not \
                 repairable by `export --sanitize-iris`)",
                "invalid IRIs",
                thousands(b.occurrences),
                thousands(b.statements),
                thousands(b.unrepairable)
            );
        }
    }
    if let Some(run) = &info.run {
        format_run(&mut out, run, I);
    }
    if let Some(s) = &info.spill {
        let _ = writeln!(
            out,
            "{I}{:<12}: {} chunk(s); {} k-way merge(s) over {} sorted run(s); scratch peak {}",
            "spill",
            thousands(s.chunks),
            thousands(s.merge_passes),
            thousands(s.sort_runs),
            human_bytes(s.scratch_peak_bytes)
        );
    }
    if let Some(r) = &info.recarded {
        let _ = writeln!(
            out,
            "{I}{:<12}: {} by {} (from content hash {})",
            "re-carded",
            r.at.as_deref().unwrap_or("?"),
            r.engine
                .as_ref()
                .map(|e| format!("{} {}, commit {}", e.name, e.version, short_hash(&e.commit)))
                .or_else(|| r.by.clone())
                .unwrap_or_else(|| "?".into()),
            r.from_content_hash
        );
        if let Some(i) = &r.input {
            let _ = writeln!(
                out,
                "{I}  {:<10}: {}{}",
                "from",
                i.name,
                i.bytes
                    .map(|b| format!(" ({} B)", thousands(b)))
                    .unwrap_or_default()
            );
        }
        if let Some(run) = &r.run {
            format_run(&mut out, run, &format!("{I}  "));
        }
    }
    if let Some(costs) = &info.query_costs {
        let _ = writeln!(
            out,
            "{I}query costs (cold; bytes/requests portable, ms = one machine's debug ref):"
        );
        // The transport belongs WITH the numbers: "1.4 MB in 76 requests" is
        // only a cost once you know what did the requesting, and these figures
        // can now come either from the build (an in-memory image) or from a
        // later `card-audit --measure` (a file handle, or HTTP).
        if let Some(t) = &costs.context.transport {
            let _ = writeln!(out, "        measured over: {t}");
        }
        for c in &costs.queries {
            let _ = writeln!(
                out,
                "        {:<14} {:>9} B in {:>2} req · {:>5} row(s) · {} ms",
                c.id, c.bytes, c.requests, c.rows, c.debug_ms
            );
        }
    }
    if !info.dropped_queries.is_empty() {
        let _ = writeln!(
            out,
            "{I}starter queries generated, measured, and NOT shipped:"
        );
        for d in &info.dropped_queries {
            let _ = writeln!(
                out,
                "        {:<14} {}{}",
                d.id,
                d.why,
                if d.contradicts_claim {
                    " [generator defect: its template claimed this could not happen]"
                } else {
                    ""
                }
            );
        }
    }
    while out.ends_with('\n') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_formats_known_epochs() {
        assert_eq!(epoch_to_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(1_785_801_600), "2026-08-04T00:00:00Z");
        assert_eq!(epoch_to_rfc3339(86_399), "1970-01-01T23:59:59Z");
    }

    #[test]
    fn build_info_round_trips_and_omits_absent_fields() {
        let info = new_build_info(BuildParams {
            command: Some("build".into()),
            no_pyramid: true,
            codec: Some(codec_name(rete_core::CODEC_ZSTD).into()),
            card_top_n: Some(100),
            ..Default::default()
        });
        let bytes = info.to_json_bytes();
        let back = BuildInfo::from_json_bytes(&bytes).unwrap();
        assert_eq!(info, back);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("built_at"));
        assert!(text.contains("rete-cli"));
        assert!(text.contains("no_pyramid"));
        // Absent options are omitted, keeping the section small.
        assert!(!text.contains("memory_budget_mb"));
        assert!(!text.contains("query_costs"));
        assert!(!text.contains("text_index"));
    }

    #[test]
    fn empty_build_info_parses_from_empty_object() {
        // Forward-compat: an empty or minimal JSON object still deserializes.
        let b = BuildInfo::from_json_bytes(b"{}").unwrap();
        assert!(b.built_at.is_none() && b.builder.is_none());
        assert!(b.params.is_empty());
    }

    /// The reservation is an exact upper bound: a record sealed with every
    /// measured field at its largest — every phase timed, every counter at
    /// `u64::MAX`, a gzip input — still fits the bytes reserved before the run.
    #[test]
    fn a_sealed_record_always_fits_its_reservation() {
        use rete_core::build_trace::{lap, Phase};
        let rec = Recorder::start(&[("/home/someone/data/in.nq.gz", "nq"), ("-", "nq")]);
        let mut info = new_build_info(BuildParams {
            command: Some("build".into()),
            ..Default::default()
        });
        rec.begin(&mut info);
        // Everything but the final write has run — two phases measured on
        // their own, as a card's derivation and starter queries are — and the
        // inputs and counts are final when the section is reserved.
        for p in Phase::ALL.iter().rev().filter(|p| **p != Phase::Write) {
            if matches!(p, Phase::CardDerive | Phase::StarterQueries) {
                rec.begin_measured();
                rec.lap_measured(*p);
            } else {
                lap(*p);
            }
        }
        for t in &rec.taps {
            t.bytes.set(u64::MAX);
            t.gzip.set(true);
        }
        info.inputs = rec.inputs();
        info.counts = Some(Counts {
            statements_read: u64::MAX,
            statements_inferred: Some(u64::MAX),
            statements_unique: u64::MAX,
            duplicates_dropped: u64::MAX,
            default_graph_triples: u64::MAX,
            named_graphs: u64::MAX,
            named_graph_quads: u64::MAX,
            terms: u64::MAX,
            invalid_iris: Some(InvalidIris {
                statements: u64::MAX,
                occurrences: u64::MAX,
                repairable: u64::MAX,
                unrepairable: u64::MAX,
            }),
        });
        info.params.codec = Some("zstd".into());
        let reserve = info.reserve_bytes(Pending::Write, &rec);
        lap(Phase::Write);
        rec.seal(&mut info);
        if let Some(r) = info.run.as_mut() {
            r.wall_ms = u64::MAX;
            r.peak_rss_bytes = Some(u64::MAX);
            r.memory_limit_bytes = Some(u64::MAX);
            r.cpus = Some(u64::MAX);
            for p in &mut r.phases {
                p.ms = u64::MAX;
                if p.peak_rss_bytes.is_some() {
                    p.peak_rss_bytes = Some(u64::MAX);
                }
            }
        }
        let sealed = info.to_json_bytes();
        assert!(
            sealed.len() <= reserve.len(),
            "sealed {} > reserved {}",
            sealed.len(),
            reserve.len()
        );
        // Names only, never a directory.
        assert_eq!(info.inputs[0].name, "in.nq.gz");
        assert_eq!(info.inputs[1].name, "stdin");
    }

    /// The in-place seal: the reserved section is overwritten with the record
    /// padded by spaces, which every JSON reader accepts, and nothing else in
    /// the file moves — the content hash and every other byte stay put.
    #[test]
    fn patching_fills_the_reservation_in_place() {
        let (image, _) = rete_core::ingest::assemble_dataset(
            vec![(
                "<http://x/a>".into(),
                "<http://x/p>".into(),
                "<http://x/b>".into(),
                None,
            )],
            br#"{"title":"t"}"#,
        );
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("p.rete");
        let reserve = vec![b' '; 1024];
        write_image_with_build_info(&path, &image, &reserve).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            rete_core::attach_build_info(&image, &reserve).unwrap(),
            "the streamed splice is attach_build_info's bytes"
        );
        let info = new_build_info(BuildParams {
            command: Some("build".into()),
            ..Default::default()
        });
        patch_build_info(&path, &info.to_json_bytes()).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let section = rete_core::read_build_info(&bytes).unwrap().unwrap();
        assert_eq!(section.len(), 1024, "patched in place, not rewritten");
        assert!(section.ends_with(b" "));
        assert_eq!(BuildInfo::from_json_bytes(&section).unwrap(), info);
        assert!(rete_core::verify(&bytes).unwrap());
        assert_eq!(
            rete_core::attach_build_info(&bytes, &[]).unwrap(),
            rete_core::attach_build_info(&image, &[]).unwrap()
        );
    }

    /// The grade is read by two callers that ask different questions of it, so
    /// every result shape is pinned against **both**.
    ///
    /// `rete build` drops on `useless` and then asks "was this a *zero-row*
    /// emptiness?" — the only shape that can contradict a template's
    /// non-emptiness claim — which it reads as `useless && !vacuous`.
    /// `rete card-audit` reports an `outcome` and reads `error` to tell a run
    /// that could not finish from one that finished with nothing. Those two
    /// readings must stay consistent with each other and with the shapes below,
    /// because a starter query silently kept (or silently dropped) is exactly
    /// the failure both features exist to prevent.
    #[test]
    fn every_result_shape_grades_the_same_way_for_both_callers() {
        use rete_core::Binding;

        let unbound = Binding::default();
        let bound = Binding::from([("s".to_string(), "http://x/a".to_string())]);
        let vars = vec!["s".to_string()];
        let triple = || ("s".to_string(), "p".to_string(), "o".to_string());

        // output, rows, useless, vacuous, error
        let cases = [
            (
                QueryOutput::Select(vars.clone(), vec![]),
                0,
                true,
                false,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![unbound.clone()]),
                1,
                true,
                true,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![bound.clone(), unbound.clone()]),
                2,
                false,
                false,
                false,
            ),
            (
                QueryOutput::Select(vars.clone(), vec![bound]),
                1,
                false,
                false,
                false,
            ),
            (QueryOutput::Ask(false), 0, true, false, false),
            (QueryOutput::Ask(true), 1, false, false, false),
            (QueryOutput::Construct(vec![]), 0, true, false, false),
            (
                QueryOutput::Construct(vec![triple()]),
                1,
                false,
                false,
                false,
            ),
        ];

        for (out, rows, useless, vacuous, error) in cases {
            let g = grade(&out);
            let what = format!("{out:?}");
            assert_eq!(g.rows, rows, "rows for {what}");
            assert_eq!(g.useless.is_some(), useless, "useless for {what}");
            assert_eq!(g.vacuous, vacuous, "vacuous for {what}");
            assert_eq!(g.error.is_some(), error, "error for {what}");
            // What `build.rs` reads as "measured zero rows".
            assert_eq!(
                g.useless.is_some() && !g.vacuous,
                useless && !vacuous,
                "zero-row emptiness for {what}"
            );
        }

        // A run that never produced a result is both: nothing to ship, and a
        // failure the audit has to name rather than report as an honest zero.
        let f = Graded::failed("the file did not open: nope".into());
        assert_eq!(f.rows, 0);
        assert_eq!(f.error.as_deref(), f.useless.as_deref());
        assert!(
            !f.vacuous,
            "a failure is a zero-row emptiness, not a vacuous one"
        );
    }
}
