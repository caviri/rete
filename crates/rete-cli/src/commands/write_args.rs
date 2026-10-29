//! The writer knobs of `rete build` (`docs/cli.md#compression-knobs`): tile and
//! dictionary-chunk size, zstd level, and frequency-promoted object ids.
//!
//! They map onto [`rete_core::write_opts`], which is process-wide: the flags
//! are applied once, before the build starts, and every writer path (in-memory,
//! streaming and external) reads the same values. A flag left out keeps the
//! value of its environment variable (`RETE_TILE_KIB`, `RETE_DICT_CHUNK_KIB`,
//! `RETE_ZSTD_LEVEL`, `RETE_FREQUENT_OBJECTS`), else the default. None of them
//! changes the file format: every combination opens in any reader of it.

use rete_core::write_opts;

#[derive(clap::Args, Debug, Clone, Default)]
pub(crate) struct WriteArgs {
    /// Index tile budget, in KiB of uncompressed tile (default 64). A tile is
    /// the unit a reader fetches and decompresses, so smaller tiles mean fewer
    /// bytes per point lookup and larger ones a smaller file (each doubling
    /// saves about 1% on the measured OpenCitations sample) at more bytes and
    /// decode work per touch.
    #[arg(long = "tile-kib", value_name = "KIB",
          value_parser = clap::value_parser!(u32).range(4..=4096))]
    pub tile_kib: Option<u32>,
    /// Dictionary chunk budget, in KiB of uncompressed section body
    /// (default 64). The dictionary's equivalent of `--tile-kib`.
    #[arg(long = "dict-chunk-kib", value_name = "KIB",
          value_parser = clap::value_parser!(u32).range(4..=4096))]
    pub dict_chunk_kib: Option<u32>,
    /// zstd level for every tile and chunk, 1–22 (default 9). Decoding speed
    /// does not depend on it; encoding does: 19 makes the files a few percent
    /// smaller for several times the compression CPU, so it suits a file that
    /// is built once and read for a long time.
    #[arg(long = "zstd-level", value_name = "LEVEL",
          value_parser = clap::value_parser!(i32).range(1..=22))]
    pub zstd_level: Option<i32>,
    /// Promote the N most frequent object-only terms (classes, controlled
    /// vocabularies, status literals) into the shared dictionary section,
    /// which gives them small ids wherever they sort ahead of the shared
    /// entities — the size effect of declaring the vocabulary in a TBox,
    /// without one (default 0 = off). Readers need nothing new. Not available
    /// with `--memory-budget-mb`.
    #[arg(long = "frequent-objects", value_name = "N")]
    pub frequent_objects: Option<usize>,
}

impl WriteArgs {
    /// Install the flags as the process-wide writer settings.
    pub(crate) fn apply(&self) {
        let mut o = write_opts::write_options();
        if let Some(k) = self.tile_kib {
            o.tile_budget = k as usize * 1024;
        }
        if let Some(k) = self.dict_chunk_kib {
            o.dict_chunk_budget = k as usize * 1024;
        }
        if let Some(l) = self.zstd_level {
            o.zstd_level = l;
        }
        if let Some(n) = self.frequent_objects {
            o.frequent_objects = n;
        }
        write_opts::set_write_options(o);
    }
}
