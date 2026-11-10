//! Writer tuning knobs: index tile size, dictionary chunk size, zstd level and
//! frame shape, and frequency-promoted object ids. The measurements behind the
//! defaults are in `docs/BENCHMARK.md` ("Compression (0.4)").
//!
//! None of these change the file format. Every combination writes a file any
//! reader of the same format version opens unchanged: tiles and dictionary
//! chunks of any size are routed by their directories, zstd frames of any level
//! or shape decode with any conformant decoder, and a promoted object is an
//! ordinary entry of the shared dictionary section.
//!
//! The settings are **process-wide**, like `RETE_DICT_RESTART_INTERVAL`: the
//! CLI sets them once, before a build, and every writer path (the in-memory and
//! streaming assemblers and the external build) reads the same values, so the
//! byte-identity between those paths holds for every setting. Each knob falls
//! back to an environment variable, then to the built-in default:
//!
//! | knob | env var | default |
//! |---|---|---|
//! | index tile budget | `RETE_TILE_KIB` | 64 KiB |
//! | dictionary chunk budget | `RETE_DICT_CHUNK_KIB` | 64 KiB |
//! | zstd level | `RETE_ZSTD_LEVEL` | 9 |
//! | zstd frame shape | `RETE_ZSTD_FRAME` (`sized` / `stream`) | `sized` |
//! | frequency-promoted objects | `RETE_FREQUENT_OBJECTS` | 0 (off) |

use std::sync::atomic::{AtomicI32, AtomicU8, AtomicUsize, Ordering};

/// Default per-tile budget of an index permutation, in uncompressed bytes.
pub const DEFAULT_TILE_BUDGET_BYTES: usize = 64 * 1024;
/// Default per-chunk budget of a dictionary section body, in uncompressed bytes.
pub const DEFAULT_DICT_CHUNK_BYTES: usize = 64 * 1024;
/// Default zstd level of the writer.
pub const DEFAULT_ZSTD_LEVEL: i32 = 9;
/// Smallest accepted tile or chunk budget (KiB). Below this the per-unit
/// directory entry and frame header dominate.
pub const MIN_BUDGET_KIB: usize = 4;
/// Largest accepted tile or chunk budget (KiB). A tile is decoded whole on
/// every touch, so very large tiles turn a point lookup into a large decode.
pub const MAX_BUDGET_KIB: usize = 4096;

/// How each tile / chunk is framed by zstd.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameShape {
    /// A streaming frame without a content size: what every writer up to 0.3
    /// produced, and still available (`RETE_ZSTD_FRAME=stream`) to reproduce a
    /// 0.3 file byte for byte. Not knowing the input size, zstd compresses each
    /// 64 KiB unit with the parameters it tunes for large inputs, and declares
    /// the level's whole window (4 MiB at level 9) in the frame header.
    Stream,
    /// A single-segment frame that records its content size (the default since
    /// 0.4). zstd then picks the parameter set it tunes for inputs of this size:
    /// at level 9 the index comes out 8% smaller on the OpenCitations sample,
    /// and encoding is faster; the frame declares the tile's own size instead
    /// of a 4 MiB window. Any conformant decoder reads both shapes, so this is
    /// not a format change (released 0.3.5 and 0.3.7 readers verified).
    Sized,
}

/// A complete set of writer settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Index tile budget in uncompressed bytes.
    pub tile_budget: usize,
    /// Dictionary chunk budget in uncompressed bytes.
    pub dict_chunk_budget: usize,
    /// zstd compression level (1..=22).
    pub zstd_level: i32,
    /// zstd frame shape.
    pub frame: FrameShape,
    /// How many of the most frequent object-only terms to promote into the
    /// shared dictionary section (0 = off).
    pub frequent_objects: usize,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            tile_budget: DEFAULT_TILE_BUDGET_BYTES,
            dict_chunk_budget: DEFAULT_DICT_CHUNK_BYTES,
            zstd_level: DEFAULT_ZSTD_LEVEL,
            frame: FrameShape::Sized,
            frequent_objects: 0,
        }
    }
}

// 0 / -1 / u8::MAX = "not set yet": resolve from the environment on first read.
static TILE_BUDGET: AtomicUsize = AtomicUsize::new(0);
static DICT_CHUNK_BUDGET: AtomicUsize = AtomicUsize::new(0);
static ZSTD_LEVEL: AtomicI32 = AtomicI32::new(0);
static FRAME: AtomicU8 = AtomicU8::new(u8::MAX);
static FREQUENT_OBJECTS: AtomicUsize = AtomicUsize::new(usize::MAX);

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

fn kib_env(name: &str, default: usize) -> usize {
    env_parse::<usize>(name)
        .filter(|k| (MIN_BUDGET_KIB..=MAX_BUDGET_KIB).contains(k))
        .map(|k| k * 1024)
        .unwrap_or(default)
}

/// The index tile budget in force (bytes).
pub fn tile_budget() -> usize {
    match TILE_BUDGET.load(Ordering::Relaxed) {
        0 => {
            let v = kib_env("RETE_TILE_KIB", DEFAULT_TILE_BUDGET_BYTES);
            TILE_BUDGET.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// The dictionary chunk budget in force (bytes).
pub fn dict_chunk_budget() -> usize {
    match DICT_CHUNK_BUDGET.load(Ordering::Relaxed) {
        0 => {
            let v = kib_env("RETE_DICT_CHUNK_KIB", DEFAULT_DICT_CHUNK_BYTES);
            DICT_CHUNK_BUDGET.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// The zstd level in force.
pub fn zstd_level() -> i32 {
    match ZSTD_LEVEL.load(Ordering::Relaxed) {
        0 => {
            let v = env_parse::<i32>("RETE_ZSTD_LEVEL")
                .filter(|l| (1..=22).contains(l))
                .unwrap_or(DEFAULT_ZSTD_LEVEL);
            ZSTD_LEVEL.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// The zstd frame shape in force.
pub fn frame_shape() -> FrameShape {
    let code = match FRAME.load(Ordering::Relaxed) {
        u8::MAX => {
            let v = match std::env::var("RETE_ZSTD_FRAME").ok().as_deref() {
                Some("stream") => 0,
                _ => 1,
            };
            FRAME.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    };
    if code == 1 {
        FrameShape::Sized
    } else {
        FrameShape::Stream
    }
}

#[cfg(test)]
thread_local! {
    /// [`frequent_objects`] for this thread alone: the crate's unit tests run
    /// in parallel threads, so one that promotes must not change what every
    /// other test's builder does.
    pub(crate) static TEST_FREQUENT_OBJECTS: std::cell::Cell<Option<usize>> =
        const { std::cell::Cell::new(None) };
}

/// How many frequent object-only terms the dictionary builder promotes.
pub fn frequent_objects() -> usize {
    #[cfg(test)]
    if let Some(n) = TEST_FREQUENT_OBJECTS.with(|c| c.get()) {
        return n;
    }
    match FREQUENT_OBJECTS.load(Ordering::Relaxed) {
        usize::MAX => {
            let v = env_parse::<usize>("RETE_FREQUENT_OBJECTS").unwrap_or(0);
            FREQUENT_OBJECTS.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// The full set of settings in force.
pub fn write_options() -> WriteOptions {
    WriteOptions {
        tile_budget: tile_budget(),
        dict_chunk_budget: dict_chunk_budget(),
        zstd_level: zstd_level(),
        frame: frame_shape(),
        frequent_objects: frequent_objects(),
    }
}

/// Replace the settings in force for this process (the CLI calls this once,
/// from its flags, before a build). Values outside the accepted ranges are
/// clamped.
pub fn set_write_options(o: WriteOptions) {
    let clamp_budget = |b: usize| b.clamp(MIN_BUDGET_KIB * 1024, MAX_BUDGET_KIB * 1024);
    TILE_BUDGET.store(clamp_budget(o.tile_budget), Ordering::Relaxed);
    DICT_CHUNK_BUDGET.store(clamp_budget(o.dict_chunk_budget), Ordering::Relaxed);
    ZSTD_LEVEL.store(o.zstd_level.clamp(1, 22), Ordering::Relaxed);
    FRAME.store(
        match o.frame {
            FrameShape::Stream => 0,
            FrameShape::Sized => 1,
        },
        Ordering::Relaxed,
    );
    FREQUENT_OBJECTS.store(o.frequent_objects, Ordering::Relaxed);
}

/// Default rows of one sort run of the `0x07` quad orders: 100 M rows are 1.6 GB
/// of `(u32, u32, u32, u32)`, which with the tile buffers stays under a 4 GB
/// container. A file with at most this many named quads is sorted in RAM.
pub const DEFAULT_QUAD_RUN_ROWS: usize = 100_000_000;

static QUAD_RUN_ROWS: AtomicUsize = AtomicUsize::new(0);

/// Rows per sort run of the `0x07` quad orders (`RETE_QUAD_RUN_ROWS`, default
/// [`DEFAULT_QUAD_RUN_ROWS`]). Changes how a build uses RAM and disk, never the
/// bytes it writes.
pub fn quad_run_rows() -> usize {
    match QUAD_RUN_ROWS.load(Ordering::Relaxed) {
        0 => {
            let v = env_parse::<usize>("RETE_QUAD_RUN_ROWS")
                .filter(|&n| n > 0)
                .unwrap_or(DEFAULT_QUAD_RUN_ROWS);
            QUAD_RUN_ROWS.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// Set the rows per sort run (0 = back to the default).
pub fn set_quad_run_rows(n: usize) {
    QUAD_RUN_ROWS.store(n, Ordering::Relaxed);
}

#[cfg(not(target_arch = "wasm32"))]
static SCRATCH_DIR: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);

/// Where a writer spills its sort runs when it has no directory of its own (the
/// CLI's `--tmp-dir`); `None` = the system temporary directory.
#[cfg(not(target_arch = "wasm32"))]
pub fn scratch_dir() -> Option<std::path::PathBuf> {
    SCRATCH_DIR.lock().ok().and_then(|g| g.clone())
}

/// Set (or clear) the spill directory of [`scratch_dir`].
#[cfg(not(target_arch = "wasm32"))]
pub fn set_scratch_dir(dir: Option<std::path::PathBuf>) {
    if let Ok(mut g) = SCRATCH_DIR.lock() {
        *g = dir;
    }
}

/// Default size above which a `0x07` graph view does not build its POS and OSP
/// orders from its own GSPO slice (see `quad::view`).
pub const DEFAULT_QUAD_SYNTH_MAX: u64 = 1 << 20;

static QUAD_SYNTH_MAX: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Quads of a named graph up to which a `0x07` view builds its POS and OSP
/// orders from the graph's GSPO tiles (`RETE_QUAD_SYNTH_MAX`, default
/// [`DEFAULT_QUAD_SYNTH_MAX`]); a bigger graph reads them from the graph-last
/// POSG and OSPG tiles. Changes what a query reads, never what it returns.
pub fn quad_view_synth_max() -> u64 {
    match QUAD_SYNTH_MAX.load(Ordering::Relaxed) {
        0 => {
            let v = env_parse::<u64>("RETE_QUAD_SYNTH_MAX")
                .filter(|&n| n > 0)
                .unwrap_or(DEFAULT_QUAD_SYNTH_MAX);
            QUAD_SYNTH_MAX.store(v, Ordering::Relaxed);
            v
        }
        v => v,
    }
}

/// Set [`quad_view_synth_max`] (0 = back to the default).
pub fn set_quad_view_synth_max(n: u64) {
    QUAD_SYNTH_MAX.store(n, Ordering::Relaxed);
}

/// Class clustering of the subject-leading permutations (`0x07`,
/// `--cluster-subjects-by-class`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterOpts {
    /// The most classes that get a run of their own (at most
    /// [`crate::v06::MAX_CLUSTERS`]).
    pub max_classes: usize,
    /// A class needs at least this many subjects to get a run: a smaller class
    /// would not fill a tile, and every run costs a point lookup one more tile.
    pub min_subjects: u64,
}

/// Default of [`ClusterOpts::min_subjects`].
pub const DEFAULT_CLUSTER_MIN_SUBJECTS: u64 = 1000;

impl Default for ClusterOpts {
    fn default() -> Self {
        ClusterOpts {
            max_classes: crate::v06::MAX_CLUSTERS,
            min_subjects: DEFAULT_CLUSTER_MIN_SUBJECTS,
        }
    }
}

static CLUSTER: std::sync::Mutex<Option<ClusterOpts>> = std::sync::Mutex::new(None);

/// The clustering the writers apply, `None` = off (the default). Process-wide
/// like the other writer settings, so every writer path agrees.
pub fn cluster() -> Option<ClusterOpts> {
    CLUSTER.lock().ok().and_then(|g| *g)
}

/// Turn class clustering on (with these limits) or off.
pub fn set_cluster(c: Option<ClusterOpts>) {
    if let Ok(mut g) = CLUSTER.lock() {
        *g = c;
    }
}
