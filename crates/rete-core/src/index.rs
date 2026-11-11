//! Permutation index set (SPO / POS / OSP) over integer triples (SPEC.md §6).
//!
//! The three orders together answer every triple-pattern shape: for any subset
//! of bound `(s, p, o)` components, at least one permutation sorts those bound
//! components into a leading prefix, turning the lookup into a range scan.
//!
//! Each permutation is **tiled** (format v0.2): consecutive runs of whole
//! a-groups packed to a byte budget ([`INDEX_TILE_BUDGET`]), each tile a
//! self-contained [`TripleBlock`]. A bound leading id routes to exactly one
//! tile (binary search over the tile ranges), then jumps to its a-group via
//! the tile's lazily-built [`GroupDirectory`]; unbound scans chain all tiles
//! with per-tile zone-map pruning. On disk each tile is compressed
//! independently, so a ranged reader can fetch just the tiles a query needs.
//! v0.1 single-block sections are still read (one tile per permutation).

use std::sync::Arc;

use crate::chunk_cache::{CacheEntry, ChunkCache};
use crate::triples::{GroupDirectory, Triple, TripleBlock, TripleBlockBuilder};

/// A triple pattern: `None` is an unbound variable, `Some(id)` a bound term.
pub type Pattern = (Option<u32>, Option<u32>, Option<u32>);

/// The most tiles one probe-prefetch window faults ([`GraphIndex::prefetch_probe_tiles`]).
pub(crate) const PROBE_WINDOW_TILES: usize = 1024;

/// The tiles a window of upcoming probes will route to, gathered one pattern
/// at a time ([`GraphIndex::probe_batch_add`]) so the window can grow until it
/// is full, then faulted together ([`GraphIndex::probe_batch_fault`]).
///
/// Bounded in tiles ([`PROBE_WINDOW_TILES`]) and in stored bytes (an eighth
/// of the tile cache, within 256 KiB–64 MiB): faulting every tile of a long probe
/// list at once overran the cache, which evicted the early tiles before their
/// probes ran, and each then came back as a request of its own, one round trip
/// at a time (G50a).
#[derive(Default)]
pub(crate) struct ProbeBatch {
    want: [std::collections::BTreeSet<usize>; NUM_PERMS],
    tiles: usize,
    bytes: u64,
    /// Patterns accepted so far.
    patterns: usize,
}

/// Default per-tile budget in (uncompressed) encoded bytes. Tiles are the
/// independently-compressed, independently-fetchable units of a permutation
/// section: ~64 KiB matches both zstd's sweet spot and one HTTP range read.
/// The budget a build uses is [`crate::write_opts::tile_budget`] (`--tile-kib`).
pub const INDEX_TILE_BUDGET: usize = crate::write_opts::DEFAULT_TILE_BUDGET_BYTES;

/// Geometric tile-prefetch ramp for an unbound (multi-tile) scan (see
/// [`GraphIndex::scan_iter`]). The first coalesced batch faults this many
/// tiles; each subsequent batch doubles up to [`PREFETCH_WINDOW_MAX`]. Small
/// enough that `LIMIT 1` fetches only a few tiles, large enough that a full
/// scan still coalesces into a handful of range reads.
const PREFETCH_WINDOW_START: usize = 4;
const PREFETCH_WINDOW_MAX: usize = 512;

/// Fetches one tile's (uncompressed) block image on demand: the bridge that
/// lets a remote `GraphIndex` fault tiles in over a `RangeReader` without this
/// module knowing about I/O. `None` = the fetch failed; the index records the
/// failure ([`GraphIndex::load_incomplete`]) and the scan sees an empty tile —
/// callers running over remote data MUST check the flag after evaluating.
pub type TileLoader = Box<dyn Fn(usize, usize) -> Option<Vec<u8>> + Send + Sync>;

/// Fetches **many** tiles of one section in one round trip: given the section
/// and the (ascending) indices of the tiles wanted, returns each tile's
/// uncompressed block image in the same order. The ranged reader implements
/// this by coalescing byte-adjacent tile ranges into single range reads, so a
/// full-section scan costs a handful of requests instead of one per tile.
/// `None` = the batch failed as a whole; callers fall back to the per-tile
/// [`TileLoader`] (which records per-tile failures).
pub type TileBulkLoader = Box<dyn Fn(usize, &[usize]) -> Option<Vec<Vec<u8>>> + Send + Sync>;

/// One tile of a permutation section: a fully self-contained [`TripleBlock`]
/// over a consecutive run of whole a-groups, plus the leading-id range it
/// covers (for routing without parsing or fetching) and its lazily-built group
/// directory (for intra-tile point lookups). The block image itself may be
/// local (built/decoded eagerly) or faulted in by the index's [`TileLoader`].
pub struct Tile {
    min_a: u32,
    max_a: u32,
    /// Optional **tile synopsis**: the inclusive min/max of the two non-leading
    /// columns `(min_b, max_b, min_c, max_c)`, read from the directory trailer of
    /// a synopsis-carrying file. Lets a scan prune this tile by a bound secondary
    /// component *before* faulting it (remote reads). `None` = no synopsis (an
    /// older file, or a locally-built/opened tile) — then nothing is pruned early
    /// and the in-tile zone map prunes after the tile is in hand, as before.
    pub(crate) syn: Option<(u32, u32, u32, u32)>,
    /// Encoded byte length of this tile (compressed on-disk size for a remote
    /// tile, in-memory image size for a local one; 0 = unknown). Feeds the join
    /// planner's fatness gates without faulting any data.
    len: u32,
    /// Build-time staging only: a locally-built or locally-decoded tile's body
    /// waits here until [`GraphIndex::from_sections`] moves it into the shared
    /// [`ChunkCache`] (keyed by `(section, tile)`) and clears it. `None`
    /// afterward, and for a remote tile whose body faults in through the loader.
    /// The long-lived retention is the cache, not this field.
    staged: Option<Vec<u8>>,
}

impl Tile {
    fn local(min_a: u32, max_a: u32, bytes: Vec<u8>) -> Self {
        let len = bytes.len().min(u32::MAX as usize) as u32;
        Tile {
            min_a,
            max_a,
            syn: None,
            len,
            staged: Some(bytes),
        }
    }

    /// A remote tile's descriptor with its stored length.
    pub(crate) fn remote_with_len(
        min_a: u32,
        max_a: u32,
        syn: Option<(u32, u32, u32, u32)>,
        len: u32,
    ) -> Self {
        Tile {
            len,
            ..Tile::remote(min_a, max_a, syn)
        }
    }

    fn remote(min_a: u32, max_a: u32, syn: Option<(u32, u32, u32, u32)>) -> Self {
        Tile {
            min_a,
            max_a,
            syn,
            len: 0,
            staged: None,
        }
    }

    /// Encoded byte length (see the field doc); falls back to a still-staged
    /// body's size when the directory didn't provide one.
    pub(crate) fn encoded_len(&self) -> u64 {
        if self.len > 0 {
            self.len as u64
        } else {
            self.staged.as_ref().map_or(0, |d| d.len() as u64)
        }
    }

    /// Leading-component (permuted `a`) range this tile covers, inclusive.
    pub fn leading_range(&self) -> (u32, u32) {
        (self.min_a, self.max_a)
    }

    /// Could this tile hold a triple with the given bound non-leading components,
    /// per its synopsis? `true` when there is no synopsis (can't rule it out) or
    /// the bound `b`/`c` fall inside the recorded ranges. Conservative: a `false`
    /// is a *proven* miss (the in-tile zone map would reject the same tile), so
    /// skipping the fetch never drops a result.
    /// The inclusive range of this tile's column `col` (0 = the leading one),
    /// from the directory: `None` for a non-leading column of a tile without
    /// a synopsis. It covers every row of the tile, so it may be wider than
    /// the rows of one leading id.
    pub(crate) fn column_range(&self, col: usize) -> Option<(u32, u32)> {
        match col {
            0 => Some((self.min_a, self.max_a)),
            1 => self.syn.map(|s| (s.0, s.1)),
            _ => self.syn.map(|s| (s.2, s.3)),
        }
    }

    pub(crate) fn syn_admits(&self, pb: Option<u32>, pc: Option<u32>) -> bool {
        match self.syn {
            None => true,
            Some((min_b, max_b, min_c, max_c)) => {
                let ok = |v: Option<u32>, lo: u32, hi: u32| v.is_none_or(|x| lo <= x && x <= hi);
                ok(pb, min_b, max_b) && ok(pc, min_c, max_c)
            }
        }
    }
}

/// Fetches stored leaf blocks of a two-level directory (format `0x06`, SPEC.md
/// §4.4): absolute `(offset, len)` ranges in, their bytes out, in order — one
/// batched read. `None` = the batch failed.
pub type LeafFetch = Arc<dyn Fn(&[(u64, u64)]) -> Option<Vec<Vec<u8>>> + Send + Sync>;

/// A two-level directory whose leaves fault on demand (a tile directory, a
/// chunk directory): what [`ensure_leaves_across`] needs to fetch the leaves of
/// several directories in **one** batched read instead of one read each.
pub(crate) trait LeafDir {
    /// Leaf `li`'s absolute stored range, when it exists and is not resident.
    fn missing_leaf(&self, li: usize) -> Option<(u64, u64)>;
    /// Decode `stored` as leaf `li` and keep it (recording a failure).
    fn install_leaf(&self, li: usize, stored: &[u8]);
    /// Record that a batch carrying this directory's leaves failed.
    fn fail_leaves(&self);
    /// The batched reader the directory's leaves come through.
    fn leaf_fetch(&self) -> &LeafFetch;
}

/// Fault the not-yet-resident leaves `lis` of each directory in `dirs` in one
/// batched read, through the first directory's fetcher. Every directory of one
/// open reads through the same reader, so this is a single round trip where one
/// read per directory was one per directory in sequence (a lookup that names
/// terms in three dictionary sections, a pattern estimated on four
/// permutations). A failed batch marks every directory it carried.
pub(crate) fn ensure_leaves_across(dirs: &[(&dyn LeafDir, &[usize])]) {
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let mut slots: Vec<(usize, usize)> = Vec::new();
    for (di, (dir, lis)) in dirs.iter().enumerate() {
        // The same leaf can come many times over (a join probing 460 k subjects
        // of one leaf): fetch each once.
        let mut lis: Vec<usize> = lis.to_vec();
        lis.sort_unstable();
        lis.dedup();
        for li in lis {
            if let Some(r) = dir.missing_leaf(li) {
                ranges.push(r);
                slots.push((di, li));
            }
        }
    }
    let Some((first, _)) = dirs.first() else {
        return;
    };
    if ranges.is_empty() {
        return;
    }
    match (first.leaf_fetch())(&ranges) {
        Some(blobs) if blobs.len() == slots.len() => {
            for (&(di, li), b) in slots.iter().zip(&blobs) {
                dirs[di].0.install_leaf(li, b);
            }
        }
        _ => {
            for &(di, _) in &slots {
                dirs[di].0.fail_leaves();
            }
        }
    }
}

/// One leaf of a permutation's tile directory, as the `0x06` root describes it.
#[derive(Debug, Clone)]
pub struct TileLeaf {
    /// Index of the leaf's first tile in the permutation.
    pub first: usize,
    /// Tiles in the leaf.
    pub n: usize,
    /// Leading-id range of the leaf (first tile's min, last tile's max).
    pub min_a: u32,
    /// See `min_a`.
    pub max_a: u32,
    /// Absolute range of the stored leaf block.
    pub leaf: (u64, u64),
    /// Absolute offset of the leaf's first tile.
    pub tile_base: u64,
}

/// A permutation's tile directory whose entries live in leaf blocks faulted
/// on demand and kept for the session (format `0x06`). Routing narrows by the
/// leaves' leading-id ranges first, so a bound lookup faults one leaf.
/// A faulted tile-directory leaf: each tile and its absolute offset.
type TileCell = std::sync::OnceLock<Box<[(Tile, u64)]>>;

pub struct LeafedTiles {
    leaves: Vec<TileLeaf>,
    cells: Vec<TileCell>,
    total: usize,
    total_bytes: u64,
    fetch: LeafFetch,
    codec: u8,
    /// Decompression bound on a leaf.
    max_block: u64,
    failed: std::sync::atomic::AtomicBool,
}

/// The tile standing in for one whose leaf could not be read: it routes
/// nowhere and its synopsis admits nothing. The failure is recorded.
static UNREADABLE_TILE: Tile = Tile {
    min_a: u32::MAX,
    max_a: u32::MAX,
    syn: Some((1, 0, 1, 0)),
    len: 0,
    staged: None,
};

impl LeafDir for LeafedTiles {
    fn missing_leaf(&self, li: usize) -> Option<(u64, u64)> {
        (li < self.cells.len() && self.cells[li].get().is_none()).then(|| self.leaves[li].leaf)
    }

    fn install_leaf(&self, li: usize, stored: &[u8]) {
        match self.decode_leaf(li, stored) {
            Some(v) => {
                let _ = self.cells[li].set(v);
            }
            None => self
                .failed
                .store(true, std::sync::atomic::Ordering::Relaxed),
        }
    }

    fn fail_leaves(&self) {
        self.failed
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }

    fn leaf_fetch(&self) -> &LeafFetch {
        &self.fetch
    }
}

impl LeafedTiles {
    /// Bound every leaf decompression at `max` bytes (from the `0x06` root).
    pub fn with_max_block(mut self, max: u64) -> Self {
        self.max_block = max;
        self
    }

    /// A leafed directory of `total` tiles and `total_bytes` stored bytes.
    pub fn new(
        leaves: Vec<TileLeaf>,
        total: usize,
        total_bytes: u64,
        fetch: LeafFetch,
        codec: u8,
    ) -> Self {
        let cells = leaves.iter().map(|_| std::sync::OnceLock::new()).collect();
        LeafedTiles {
            leaves,
            cells,
            total,
            total_bytes,
            fetch,
            codec,
            max_block: u64::MAX,
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// How many tiles hold leading id `a`, from the root alone and without
    /// reading anything (the `rete cost` preview). A resident leaf is counted
    /// exactly; one that is not is estimated from what the root records of it:
    /// a leaf whose whole range is `a` holds `n` such tiles, and a leaf shared
    /// with other ids holds its tiles' share of the range (at least one). Skewed
    /// ids make the shared-leaf figure rough; [`Self::admitted_estimate`] reads
    /// instead.
    pub(crate) fn admitted_unfaulted(&self, a: u32) -> f64 {
        let mut tiles = 0.0;
        for (li, lf) in self.leaves.iter().enumerate() {
            if a < lf.min_a || a > lf.max_a {
                continue;
            }
            tiles += match self.cells[li].get() {
                Some(v) => v
                    .iter()
                    .filter(|(t, _)| t.min_a <= a && a <= t.max_a)
                    .count() as f64,
                None if lf.min_a == lf.max_a => lf.n as f64,
                None => {
                    let width = (lf.max_a as f64 - lf.min_a as f64) + 1.0;
                    (lf.n as f64 / width).max(1.0)
                }
            };
        }
        tiles
    }

    /// The `(first tile's min_b, last tile's max_b)` of leaf `li`, faulting it;
    /// `None` when the leaf cannot be read or a tile carries no synopsis.
    fn leaf_b_span(&self, li: usize) -> Option<(u32, u32)> {
        self.ensure_leaves(&[li]);
        let v = self.cells[li].get()?;
        let first = v.first()?.0.syn?;
        let last = v.last()?.0.syn?;
        Some((first.0, last.1))
    }

    /// The leaves of the run of tiles holding leading id `a` that [`admitted_estimate`](Self::admitted_estimate)
    /// counts exactly: the first and the last, when shared with other ids.
    pub(crate) fn run_end_leaves(&self, a: u32) -> Vec<usize> {
        let first = self.leaves.partition_point(|lf| lf.max_a < a);
        let mut last = first;
        while last < self.leaves.len() && self.leaves[last].min_a <= a {
            last += 1;
        }
        let shared = |li: usize| !(self.leaves[li].min_a == a && self.leaves[li].max_a == a);
        let mut out = Vec::new();
        if first < last {
            if shared(first) {
                out.push(first);
            }
            if last - 1 != first && shared(last - 1) {
                out.push(last - 1);
            }
        }
        out
    }

    /// Exact count of leaf `li`'s tiles that hold `a` and admit `pb`/`pc`,
    /// faulting the leaf; the unfaulted figure if it cannot be read.
    fn count_in_leaf(&self, li: usize, a: u32, pb: Option<u32>, pc: Option<u32>) -> f64 {
        self.ensure_leaves(&[li]);
        match self.cells[li].get() {
            Some(v) => v
                .iter()
                .filter(|(t, _)| t.min_a <= a && a <= t.max_a && t.syn_admits(pb, pc))
                .count() as f64,
            None => {
                let lf = &self.leaves[li];
                let width = (lf.max_a as f64 - lf.min_a as f64) + 1.0;
                (lf.n as f64 / width).max(1.0)
            }
        }
    }

    /// The tiles that hold leading id `a` and admit the bound second and third
    /// keys `pb`/`pc`, counted from the root plus a few leaf reads (all kept
    /// for the session; the lookup that follows needs them anyway).
    ///
    /// The planner's row estimates stand on this, so it must neither say "one
    /// tile" for a predicate that fills a leaf nor ignore a bound class:
    /// `?s rdf:type <small class>` is a few tiles of an 851 M-row predicate.
    /// - A leaf shared with other ids (at most two per id) is read and counted
    ///   exactly, synopsis included.
    /// - Leaves wholly inside `a` cost nothing when only `a` is bound (each is
    ///   `n` tiles). With `pb` bound they are ordered by `pb` (tiles are cut in
    ///   `(a, b, c)` order), so the admitted tiles are a contiguous run that two
    ///   binary searches over the leaves' `b` spans bound: O(log leaves) reads,
    ///   then the run's end leaves are counted exactly and the leaves between
    ///   them are all `n`.
    pub(crate) fn admitted_estimate(&self, a: u32, pb: Option<u32>, pc: Option<u32>) -> f64 {
        let first = self.leaves.partition_point(|lf| lf.max_a < a);
        let mut last = first;
        while last < self.leaves.len() && self.leaves[last].min_a <= a {
            last += 1;
        }
        if first >= last {
            return 0.0;
        }
        let mut tiles = 0.0;
        // Pure leaves are contiguous: every shared one is first or last.
        let pure = |li: usize| self.leaves[li].min_a == a && self.leaves[li].max_a == a;
        let (mut p0, mut p1) = (first, last);
        if !pure(first) {
            tiles += self.count_in_leaf(first, a, pb, pc);
            p0 += 1;
        }
        if p1 > p0 && !pure(p1 - 1) {
            tiles += self.count_in_leaf(p1 - 1, a, pb, pc);
            p1 -= 1;
        }
        if p0 >= p1 {
            return tiles;
        }
        // Wholly-`a` leaves p0..p1.
        let Some(b) = pb else {
            return tiles + self.leaves[p0..p1].iter().map(|l| l.n as f64).sum::<f64>();
        };
        // First leaf whose last tile reaches `b`; first leaf that starts past it.
        let search = |past: bool| -> Option<usize> {
            let (mut lo, mut hi) = (p0, p1);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                let (min_b, max_b) = self.leaf_b_span(mid)?;
                if if past { min_b > b } else { max_b >= b } {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            Some(lo)
        };
        let (Some(f), Some(g)) = (search(false), search(true)) else {
            // A leaf could not be read: no narrowing, count them all.
            return tiles + self.leaves[p0..p1].iter().map(|l| l.n as f64).sum::<f64>();
        };
        if f >= g {
            return tiles;
        }
        // Leaves f..g may hold `b`; those strictly between the ends are all `b`.
        tiles += self.count_in_leaf(f, a, pb, pc);
        if g - 1 > f {
            tiles += self.count_in_leaf(g - 1, a, pb, pc);
            tiles += self.leaves[f + 1..g - 1]
                .iter()
                .map(|l| l.n as f64)
                .sum::<f64>();
        }
        tiles
    }

    fn leaf_of(&self, ti: usize) -> usize {
        self.leaves
            .partition_point(|l| l.first <= ti)
            .saturating_sub(1)
    }

    fn decode_leaf(&self, li: usize, stored: &[u8]) -> Option<Box<[(Tile, u64)]>> {
        let l = &self.leaves[li];
        let raw = crate::file::decompress_bounded(self.codec, stored, self.max_block).ok()?;
        let ents = crate::v06::decode_tile_leaf(&raw, l.n).ok()?;
        let mut at = l.tile_base;
        let mut out = Vec::with_capacity(ents.len());
        for e in ents {
            let mut t = Tile::remote(e.min_a, e.max_a, Some(e.syn));
            t.len = e.clen.min(u32::MAX as u64) as u32;
            out.push((t, at));
            at = at.checked_add(e.clen)?;
        }
        Some(out.into_boxed_slice())
    }

    /// Fault every not-yet-resident leaf of `lis` in one batched read.
    pub fn ensure_leaves(&self, lis: &[usize]) {
        ensure_leaves_across(&[(self, lis)]);
    }

    /// Fault the leaves covering tiles `[start, end)`.
    pub fn ensure_span(&self, start: usize, end: usize) {
        if start >= end || start >= self.total {
            return;
        }
        let a = self.leaf_of(start);
        let b = self.leaf_of(end.min(self.total) - 1);
        self.ensure_leaves(&(a..=b).collect::<Vec<_>>());
    }

    fn entry(&self, ti: usize) -> Option<&(Tile, u64)> {
        if ti >= self.total {
            return None;
        }
        let li = self.leaf_of(ti);
        if self.cells[li].get().is_none() {
            self.ensure_leaves(&[li]);
        }
        self.cells[li].get()?.get(ti - self.leaves[li].first)
    }

    /// Tile `ti`'s absolute stored range, faulting its leaf.
    pub fn range(&self, ti: usize) -> Option<(u64, u64)> {
        self.entry(ti).map(|(t, off)| (*off, t.len as u64))
    }

    /// Did a leaf fault fail?
    pub fn load_failed(&self) -> bool {
        self.failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Forget a recorded leaf-fault failure.
    pub fn reset_load_failure(&self) {
        self.failed
            .store(false, std::sync::atomic::Ordering::Relaxed);
    }

    /// Bytes held by the leaves faulted so far.
    fn resident_bytes(&self) -> u64 {
        let n: usize = self
            .cells
            .iter()
            .filter_map(|c| c.get())
            .map(|v| v.len())
            .sum();
        (n * (std::mem::size_of::<Tile>() + 8)) as u64
            + (self.leaves.len() * std::mem::size_of::<TileLeaf>()) as u64
    }
}

/// A tile list produced on first use by a closure (see [`TileList::Deferred`]).
pub struct DeferredTiles {
    cell: std::sync::OnceLock<Vec<Tile>>,
    /// Stored bytes the planner may assume before the descriptors exist.
    approx_bytes: u64,
    load: Box<dyn Fn() -> Option<Vec<Tile>> + Send + Sync>,
    failed: std::sync::atomic::AtomicBool,
}

impl DeferredTiles {
    /// Descriptors built by `load` on first use and kept. A failed load is recorded
    /// (see [`GraphIndex::load_incomplete`]) and reads as no tiles until retried.
    pub fn new(approx_bytes: u64, load: Box<dyn Fn() -> Option<Vec<Tile>> + Send + Sync>) -> Self {
        DeferredTiles {
            cell: std::sync::OnceLock::new(),
            approx_bytes,
            load,
            failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn get(&self) -> &[Tile] {
        if let Some(v) = self.cell.get() {
            return v;
        }
        match (self.load)() {
            Some(v) => {
                // A racing thread may have built it first; both are the same.
                let _ = self.cell.set(v);
                self.cell.get().map_or(&[][..], Vec::as_slice)
            }
            None => {
                // Not cached: the next use retries.
                self.failed
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                &[]
            }
        }
    }
}

/// The class runs of a clustered permutation (format `0x07`, SPEC.md §6.6):
/// its tiles are consecutive runs, each sorted by subject; a subject's tiles
/// are in the one run of its primary class.
pub struct ClusterRuns {
    /// The tile range of each run, run 0 (no listed class) first.
    pub runs: Vec<(usize, usize)>,
    /// The class (object id) of run `i + 1`.
    pub classes: Vec<u32>,
    /// The run the last subject was found in: the first place the next is
    /// looked for, so a join over one class's subjects reads one run.
    pub(crate) hint: std::sync::atomic::AtomicUsize,
    /// Subjects already placed (`ABSENT_RUN`: not in any run).
    memo: std::sync::Mutex<std::collections::HashMap<u32, u8>>,
}

/// Memo value for a subject no run holds.
const ABSENT_RUN: u8 = u8::MAX;
/// Subjects the run memo keeps before it starts over.
const RUN_MEMO_MAX: usize = 1 << 20;

impl ClusterRuns {
    /// Runs from the cluster root's per-run tile counts.
    pub fn new(classes: Vec<u32>, run_tiles: &[usize]) -> Self {
        let mut at = 0usize;
        let runs = run_tiles
            .iter()
            .map(|&n| {
                at += n;
                (at - n, at)
            })
            .collect();
        ClusterRuns {
            runs,
            classes,
            hint: std::sync::atomic::AtomicUsize::new(0),
            memo: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub(crate) fn remember(&self, a: u32, run: Option<usize>) {
        if let Ok(mut m) = self.memo.lock() {
            if m.len() >= RUN_MEMO_MAX {
                m.clear();
            }
            m.insert(a, run.map_or(ABSENT_RUN, |r| r as u8));
        }
        if let Some(r) = run {
            self.hint.store(r, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(crate) fn recall(&self, a: u32) -> Option<Option<usize>> {
        let m = self.memo.lock().ok()?;
        m.get(&a).map(|&r| {
            if r == ABSENT_RUN {
                None
            } else {
                Some(r as usize)
            }
        })
    }
}

/// A permutation's tile directory: resident in full (`0x05`, built indexes),
/// or two-level with leaves faulted on demand (`0x06`).
pub enum TileList {
    /// Every tile descriptor resident.
    Full(Vec<Tile>),
    /// Root resident, leaves on demand.
    Leafed(Arc<LeafedTiles>),
    /// Descriptors built on first use (a quad-index view's slice of one order's
    /// directory: an order a query never touches is never fetched).
    Deferred(Arc<DeferredTiles>),
}

impl Default for TileList {
    fn default() -> Self {
        TileList::Full(Vec::new())
    }
}

impl std::ops::Index<usize> for TileList {
    type Output = Tile;
    fn index(&self, ti: usize) -> &Tile {
        match self {
            TileList::Full(v) => &v[ti],
            TileList::Deferred(d) => d.get().get(ti).unwrap_or(&UNREADABLE_TILE),
            TileList::Leafed(l) => match l.entry(ti) {
                Some((t, _)) => t,
                None => {
                    l.failed.store(true, std::sync::atomic::Ordering::Relaxed);
                    &UNREADABLE_TILE
                }
            },
        }
    }
}

impl TileList {
    /// Tiles known without building anything (0 for a deferred list that has
    /// not been built).
    fn known_len(&self) -> usize {
        match self {
            TileList::Deferred(d) => d.cell.get().map_or(0, Vec::len),
            other => other.len(),
        }
    }

    /// Number of tiles.
    pub fn len(&self) -> usize {
        match self {
            TileList::Full(v) => v.len(),
            TileList::Deferred(d) => d.get().len(),
            TileList::Leafed(l) => l.total,
        }
    }

    /// No tiles?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The resident descriptors of a full list (`None` for a leafed one).
    pub fn as_full(&self) -> Option<&[Tile]> {
        match self {
            TileList::Full(v) => Some(v),
            TileList::Deferred(d) => Some(d.get()),
            TileList::Leafed(_) => None,
        }
    }

    /// Every tile, in order, faulting all leaves first (one batch).
    pub fn iter(&self) -> impl Iterator<Item = &Tile> {
        if let TileList::Leafed(l) = self {
            l.ensure_span(0, l.total);
        }
        (0..self.len()).map(move |i| &self[i])
    }

    /// The first tile, if any.
    pub fn first(&self) -> Option<&Tile> {
        (!self.is_empty()).then(|| &self[0])
    }

    /// The last tile, if any.
    pub fn last(&self) -> Option<&Tile> {
        (!self.is_empty()).then(|| &self[self.len() - 1])
    }

    /// Fault the leaves covering `[start, end)` in one batch (a no-op on a full
    /// list).
    pub fn ensure_span(&self, start: usize, end: usize) {
        if let TileList::Leafed(l) = self {
            l.ensure_span(start, end);
        }
    }

    /// Index of the first tile whose `max_a >= a` (`len()` if none). A leafed
    /// list narrows to the one leaf that can hold it before faulting.
    pub fn first_max_ge(&self, a: u32) -> usize {
        match self {
            TileList::Full(v) => v.partition_point(|t| t.max_a < a),
            TileList::Deferred(d) => d.get().partition_point(|t| t.max_a < a),
            TileList::Leafed(l) => {
                let li = l.leaves.partition_point(|lf| lf.max_a < a);
                if li >= l.leaves.len() {
                    return l.total;
                }
                let lf = &l.leaves[li];
                l.ensure_leaves(&[li]);
                // Binary search inside the faulted leaf: stepping with `self[i]`
                // costs a leaf lookup per tile, and a lookup runs this per probe
                // row (94,700 probes x a leaf of thousands of tiles was 126 s).
                match l.cells[li].get() {
                    Some(v) => lf.first + v.partition_point(|(t, _)| t.max_a < a),
                    None => lf.first,
                }
            }
        }
    }

    /// Index of the first tile whose `min_a > a` (`len()` if none): the end of
    /// the run of tiles that can hold leading id `a`. Faults one leaf at most.
    pub fn first_min_gt(&self, a: u32) -> usize {
        match self {
            TileList::Full(v) => v.partition_point(|t| t.min_a <= a),
            TileList::Deferred(d) => d.get().partition_point(|t| t.min_a <= a),
            TileList::Leafed(l) => {
                let next = l.leaves.partition_point(|lf| lf.min_a <= a);
                if next == 0 {
                    return 0;
                }
                let li = next - 1;
                let lf = &l.leaves[li];
                l.ensure_leaves(&[li]);
                match l.cells[li].get() {
                    Some(v) => lf.first + v.partition_point(|(t, _)| t.min_a <= a),
                    None => lf.first,
                }
            }
        }
    }

    /// [`first_max_ge`](Self::first_max_ge) within the tiles `[lo, hi)` (one
    /// class run of a clustered permutation): `hi` when none reaches `a`.
    pub(crate) fn first_max_ge_in(&self, a: u32, lo: usize, hi: usize) -> usize {
        if lo >= hi {
            return lo;
        }
        match self {
            TileList::Full(v) => lo + v[lo..hi].partition_point(|t| t.max_a < a),
            TileList::Deferred(d) => lo + d.get()[lo..hi].partition_point(|t| t.max_a < a),
            TileList::Leafed(l) => {
                let (l0, l1) = (l.leaf_of(lo), l.leaf_of(hi - 1) + 1);
                let li = l0 + l.leaves[l0..l1].partition_point(|lf| lf.max_a < a);
                if li >= l1 {
                    return hi;
                }
                let lf = &l.leaves[li];
                l.ensure_leaves(&[li]);
                match l.cells[li].get() {
                    Some(v) => lf.first + v.partition_point(|(t, _)| t.max_a < a),
                    None => lf.first,
                }
            }
        }
    }

    /// [`first_min_gt`](Self::first_min_gt) within the tiles `[lo, hi)`: `lo`
    /// when every tile starts past `a`.
    pub(crate) fn first_min_gt_in(&self, a: u32, lo: usize, hi: usize) -> usize {
        if lo >= hi {
            return lo;
        }
        match self {
            TileList::Full(v) => lo + v[lo..hi].partition_point(|t| t.min_a <= a),
            TileList::Deferred(d) => lo + d.get()[lo..hi].partition_point(|t| t.min_a <= a),
            TileList::Leafed(l) => {
                let (l0, l1) = (l.leaf_of(lo), l.leaf_of(hi - 1) + 1);
                let next = l0 + l.leaves[l0..l1].partition_point(|lf| lf.min_a <= a);
                if next == l0 {
                    return lo;
                }
                let li = next - 1;
                let lf = &l.leaves[li];
                l.ensure_leaves(&[li]);
                match l.cells[li].get() {
                    Some(v) => lf.first + v.partition_point(|(t, _)| t.min_a <= a),
                    None => lf.first,
                }
            }
        }
    }

    /// The directory leaves of tiles `[lo, hi)` the two searches above read for
    /// `a` (none for a list that is not leafed), so a caller can fault them in
    /// one batch.
    pub(crate) fn leaves_for_in(&self, a: u32, lo: usize, hi: usize) -> Vec<usize> {
        let TileList::Leafed(l) = self else {
            return Vec::new();
        };
        if lo >= hi {
            return Vec::new();
        }
        let (l0, l1) = (l.leaf_of(lo), l.leaf_of(hi - 1) + 1);
        let mut out = Vec::new();
        let li = l0 + l.leaves[l0..l1].partition_point(|lf| lf.max_a < a);
        if li < l1 {
            out.push(li);
        }
        let next = l0 + l.leaves[l0..l1].partition_point(|lf| lf.min_a <= a);
        if next > l0 && !out.contains(&(next - 1)) {
            out.push(next - 1);
        }
        out
    }

    /// Stored bytes of every tile (from the root on a leafed list).
    pub fn total_bytes(&self) -> u64 {
        match self {
            TileList::Full(v) => v.iter().map(Tile::encoded_len).sum(),
            TileList::Deferred(d) => d.get().iter().map(Tile::encoded_len).sum(),
            TileList::Leafed(l) => l.total_bytes,
        }
    }

    /// Stored bytes of the tiles whose leading range holds `a`, **without
    /// faulting**: exact where the leaf is resident, else the mean tile size times the
    /// tiles the root says hold it — the planner's cost
    /// estimate must not cost a round trip.
    pub fn routed_bytes(&self, a: u32) -> u64 {
        match self {
            TileList::Full(v) => v
                .iter()
                .filter(|t| t.min_a <= a && a <= t.max_a)
                .map(Tile::encoded_len)
                .sum(),
            // No fetch for an estimate: exact once built, else a rough size.
            TileList::Deferred(d) => match d.cell.get() {
                Some(v) => v
                    .iter()
                    .filter(|t| t.min_a <= a && a <= t.max_a)
                    .map(Tile::encoded_len)
                    .sum(),
                None => d.approx_bytes,
            },
            TileList::Leafed(l) => {
                let lis: Vec<usize> = l
                    .leaves
                    .iter()
                    .enumerate()
                    .filter(|(_, lf)| lf.min_a <= a && a <= lf.max_a)
                    .map(|(i, _)| i)
                    .collect();
                if lis.is_empty() {
                    return 0;
                }
                if lis.iter().all(|&li| l.cells[li].get().is_some()) {
                    lis.iter()
                        .flat_map(|&li| l.cells[li].get().into_iter().flat_map(|v| v.iter()))
                        .filter(|(t, _)| t.min_a <= a && a <= t.max_a)
                        .map(|(t, _)| t.encoded_len())
                        .sum()
                } else {
                    // The mean tile size times the tiles the root says hold
                    // `a`: a predicate filling many tiles is many tiles' bytes.
                    let mean = l.total_bytes as f64 / l.total.max(1) as f64;
                    (l.admitted_unfaulted(a).max(1.0) * mean) as u64
                }
            }
        }
    }

    /// Is the leaf holding tile `ti` resident (always, for a full list)?
    pub fn resident_at(&self, ti: usize) -> bool {
        match self {
            TileList::Full(_) => true,
            TileList::Deferred(d) => d.cell.get().is_some(),
            TileList::Leafed(l) => ti < l.total && l.cells[l.leaf_of(ti)].get().is_some(),
        }
    }

    /// The absolute stored range of tile `ti` (leafed lists only).
    pub fn range(&self, ti: usize) -> Option<(u64, u64)> {
        match self {
            TileList::Full(_) | TileList::Deferred(_) => None,
            TileList::Leafed(l) => l.range(ti),
        }
    }

    fn load_failed(&self) -> bool {
        match self {
            TileList::Leafed(l) => l.load_failed(),
            TileList::Deferred(d) => d.failed.load(std::sync::atomic::Ordering::Relaxed),
            TileList::Full(_) => false,
        }
    }

    fn reset_load_failure(&self) {
        match self {
            TileList::Leafed(l) => l.reset_load_failure(),
            TileList::Deferred(d) => d.failed.store(false, std::sync::atomic::Ordering::Relaxed),
            TileList::Full(_) => {}
        }
    }

    fn directory_bytes(&self) -> u64 {
        match self {
            TileList::Full(v) => (v.capacity() * std::mem::size_of::<Tile>()) as u64,
            TileList::Deferred(d) => d
                .cell
                .get()
                .map_or(0, |v| (v.capacity() * std::mem::size_of::<Tile>()) as u64),
            TileList::Leafed(l) => l.resident_bytes(),
        }
    }
}

/// What a routed scan of one pattern will touch, from the tile directories
/// alone — see [`GraphIndex::scan_plan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanPlan {
    /// The permutation the scan routes to (always inside [`PermSet::CORE`]).
    pub permutation: IndexPermutation,
    /// Tiles in that permutation's section — the denominator.
    pub tiles_total: usize,
    /// Tiles left after routing on the bound leading component.
    pub tiles_routed: usize,
    /// Tiles left after the synopsis prune — what the scan will actually fetch.
    pub tiles_admitted: usize,
    /// Encoded (on-disk) bytes of the admitted tiles: the index-side upper
    /// bound on the scan's range reads.
    pub tile_bytes: u64,
    /// Encoded bytes of the whole section — what an unpruned scan would read.
    pub section_bytes: u64,
}

/// Which stored permutation to scan. The full **six** orders (a SPARQL engine's
/// classic set, as in QLever): together they sort the triples on *every* prefix
/// of `(s, p, o)` columns, so for any bound-component prefix and any free
/// component there is a permutation that routes on the prefix **and** yields the
/// stream sorted on that free component — the precondition for a merge join.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPermutation {
    Spo,
    Sop,
    Pso,
    Pos,
    Osp,
    Ops,
}

/// The number of permutations the format can address (the width of every
/// per-permutation array). How many a *given file* stores is its [`PermSet`].
pub(crate) const NUM_PERMS: usize = 6;

/// The six permutations in section order — the canonical iteration list. The
/// original three (SPO, POS, OSP) lead, so a default scan's tie-break (and its
/// provenance) is unchanged from the 3-permutation format; the three added orders
/// (SOP, PSO, OPS) follow and exist to give a merge join a stream sorted on the
/// join key.
pub(crate) const ALL_PERMS: [IndexPermutation; NUM_PERMS] = [
    IndexPermutation::Spo,
    IndexPermutation::Pos,
    IndexPermutation::Osp,
    IndexPermutation::Sop,
    IndexPermutation::Pso,
    IndexPermutation::Ops,
];

/// **Which** permutations a file stores — a 6-bit set over
/// [`IndexPermutation::section_index`], recorded in the header so a reader
/// never has to assume six.
///
/// Every legal set is a superset of [`PermSet::CORE`] = {SPO, POS, OSP}. That
/// is not a convention: those three **tie the longest bound prefix on all eight
/// triple-pattern shapes**, so [`GraphIndex::best_permutation`] never selects
/// outside them and a lean file routes every pattern exactly where a full one
/// does — same tiles, same rows. What the other three (SOP, PSO, OPS) add is a
/// *sort order*: they let [`GraphIndex::permutation_sorted_on`] hand a
/// sort-merge join a co-sorted stream for three of the twelve
/// (bound-set, join-column) shapes it otherwise has to hash-join. Absent, the
/// merge seed declines and the hash path answers — the cost is a fast path,
/// never correctness (`bgp::try_merge_join` already returns `Option`).
///
/// `CORE` is the smallest legal set, [`PermSet::ALL`] is the default and what
/// every file written before this existed carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PermSet(u8);

impl Default for PermSet {
    fn default() -> Self {
        PermSet::ALL
    }
}

impl PermSet {
    /// All six permutations — the default, and what a `0` header byte means.
    pub const ALL: PermSet = PermSet(0b0011_1111);
    /// The three routing permutations {SPO, POS, OSP}: every pattern shape
    /// routes as well as it does with six, no merge-join sort orders.
    pub const CORE: PermSet = PermSet(0b0000_0111);

    /// Parse a raw mask. `Err` when it is not a superset of [`PermSet::CORE`]
    /// or addresses a permutation this build has no section index for — a
    /// pattern would then route to a permutation the file does not carry, which
    /// is the one failure mode that must never be silent.
    pub fn from_bits(bits: u8) -> Result<PermSet, &'static str> {
        if bits & !PermSet::ALL.0 != 0 {
            return Err("permutation mask addresses more than six permutations");
        }
        if bits & PermSet::CORE.0 != PermSet::CORE.0 {
            return Err("permutation mask must contain SPO, POS and OSP");
        }
        Ok(PermSet(bits))
    }

    /// The raw 6-bit mask.
    pub fn bits(self) -> u8 {
        self.0
    }

    /// Is `perm` stored in this file?
    pub fn contains(self, perm: IndexPermutation) -> bool {
        self.0 & (1 << perm.section_index()) != 0
    }

    /// How many permutation sections the file's index container holds.
    pub fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Never true: every legal set contains [`PermSet::CORE`]. Present because
    /// a public `len` without it is a clippy error, and the honest answer is
    /// more useful than an `allow`.
    pub fn is_empty(self) -> bool {
        false
    }

    /// The stored permutations in [`ALL_PERMS`] (= container) order.
    pub fn iter(self) -> impl Iterator<Item = IndexPermutation> {
        ALL_PERMS.into_iter().filter(move |p| self.contains(*p))
    }

    /// `perm`'s position **inside the index container**, which is its rank
    /// among the stored permutations — not [`IndexPermutation::section_index`],
    /// which is its slot in the format's fixed six-wide addressing. The two
    /// coincide only for [`PermSet::ALL`].
    pub fn position(self, perm: IndexPermutation) -> Option<usize> {
        self.contains(perm)
            .then(|| (self.0 & ((1u8 << perm.section_index()) - 1)).count_ones() as usize)
    }

    /// Names in container order, e.g. `["SPO", "POS", "OSP"]`.
    pub fn names(self) -> Vec<&'static str> {
        self.iter().map(|p| p.name()).collect()
    }

    /// Does this set carry the three merge-join orders?
    pub fn has_merge_orders(self) -> bool {
        self == PermSet::ALL
    }
}

impl IndexPermutation {
    /// Stable display name used in CLI diagnostics and provenance records.
    pub fn name(self) -> &'static str {
        match self {
            IndexPermutation::Spo => "SPO",
            IndexPermutation::Sop => "SOP",
            IndexPermutation::Pso => "PSO",
            IndexPermutation::Pos => "POS",
            IndexPermutation::Osp => "OSP",
            IndexPermutation::Ops => "OPS",
        }
    }

    /// Section number inside the stored permutation container.
    pub fn section_index(self) -> usize {
        match self {
            IndexPermutation::Spo => 0,
            IndexPermutation::Pos => 1,
            IndexPermutation::Osp => 2,
            IndexPermutation::Sop => 3,
            IndexPermutation::Pso => 4,
            IndexPermutation::Ops => 5,
        }
    }

    /// The canonical `(s, p, o)` roles in this permutation's `(a, b, c)` slots,
    /// as indices (0=s, 1=p, 2=o). The single source of truth for `forward` /
    /// `back` / `order_pattern`.
    pub(crate) const fn roles(self) -> [usize; 3] {
        match self {
            IndexPermutation::Spo => [0, 1, 2],
            IndexPermutation::Sop => [0, 2, 1],
            IndexPermutation::Pso => [1, 0, 2],
            IndexPermutation::Pos => [1, 2, 0],
            IndexPermutation::Osp => [2, 0, 1],
            IndexPermutation::Ops => [2, 1, 0],
        }
    }

    /// Map a canonical `(s, p, o)` triple into this permutation's `(a, b, c)`.
    pub(crate) fn forward(self, t: Triple) -> Triple {
        let c = [t.0, t.1, t.2];
        let r = self.roles();
        (c[r[0]], c[r[1]], c[r[2]])
    }

    /// Map a stored `(a, b, c)` back to canonical `(s, p, o)`.
    pub(crate) fn back(self, abc: Triple) -> Triple {
        let a = [abc.0, abc.1, abc.2];
        let r = self.roles();
        let mut out = [0u32; 3];
        out[r[0]] = a[0];
        out[r[1]] = a[1];
        out[r[2]] = a[2];
        (out[0], out[1], out[2])
    }

    /// The pattern's bound components in this permutation's component order.
    pub(crate) fn order_pattern(self, p: Pattern) -> [Option<u32>; 3] {
        let c = [p.0, p.1, p.2];
        let r = self.roles();
        [c[r[0]], c[r[1]], c[r[2]]]
    }

    /// Length of the leading run of bound components — higher is more selective.
    fn leading_bound(self, pat: Pattern) -> usize {
        self.order_pattern(pat)
            .iter()
            .take_while(|c| c.is_some())
            .count()
    }
}

/// Build a [`GraphIndex`] from canonical `(s, p, o)` integer triples.
pub struct GraphIndexBuilder {
    triples: Vec<Triple>,
    tile_budget: usize,
    perms: PermSet,
    /// Report each finished permutation to [`crate::build_progress`].
    progress: bool,
}

impl Default for GraphIndexBuilder {
    fn default() -> Self {
        Self {
            triples: Vec::new(),
            tile_budget: crate::write_opts::tile_budget(),
            perms: PermSet::ALL,
            progress: false,
        }
    }
}

impl GraphIndexBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build directly from an owned triple vector — moves it in, avoiding the
    /// per-triple `push` copy when the caller already has the id-triples in a
    /// `Vec` (the assembly path, where they double as the pyramid input).
    pub fn from_triples(triples: Vec<Triple>) -> Self {
        Self {
            triples,
            ..Self::default()
        }
    }

    /// Override the per-tile byte budget (tests use tiny budgets to force
    /// multi-tile sections on small data).
    pub fn with_tile_budget(mut self, bytes: usize) -> Self {
        self.tile_budget = bytes.max(1);
        self
    }

    /// Build only the permutations in `perms` (default [`PermSet::ALL`]).
    /// Sections outside the set come out empty and are not written to the file.
    pub fn with_perms(mut self, perms: PermSet) -> Self {
        self.perms = perms;
        self
    }

    /// Advance [`crate::build_progress`] by the triple count as each
    /// permutation is built (the in-RAM build's `index_permutations` phase).
    pub(crate) fn with_progress(mut self) -> Self {
        self.progress = true;
        self
    }

    pub fn push(&mut self, t: Triple) {
        self.triples.push(t);
    }

    /// Build the six permutations **one at a time** (each permutation's internal
    /// sort still uses every core), so only a single permuted copy of the triples
    /// is resident at once instead of all six concurrently. Trades the cross-
    /// permutation parallelism of [`Self::build`] for a much lower peak RAM — the
    /// large-graph low-memory path. Output is byte-identical to [`Self::build`].
    pub fn build_seq(self) -> GraphIndex {
        let triples = &self.triples;
        let budget = self.tile_budget;
        let progress = self.progress;
        let wanted: Vec<IndexPermutation> = self.perms.iter().collect();
        // Permute (parallel) + sort (parallel inside `build_tiles`) one permutation
        // at a time, so only a single permuted copy of the triples is resident — but
        // every core is still busy. Process permutations in batches of two to
        // overlap one permutation's tile-building with the next one's sort while
        // keeping the resident permuted copies to two. Byte-identical to `build`.
        let build_one = |perm: IndexPermutation| -> Vec<Tile> {
            #[cfg(feature = "parallel")]
            let permuted: Vec<Triple> = {
                use rayon::prelude::*;
                triples.par_iter().map(|&t| perm.forward(t)).collect()
            };
            #[cfg(not(feature = "parallel"))]
            let permuted: Vec<Triple> = triples.iter().map(|&t| perm.forward(t)).collect();
            let tiles = build_tiles(permuted, budget);
            if progress {
                crate::build_progress::advance(triples.len() as u64);
            }
            tiles
        };
        #[cfg(feature = "parallel")]
        let built: Vec<Vec<Tile>> = {
            use rayon::prelude::*;
            let mut built: Vec<Vec<Tile>> = Vec::with_capacity(wanted.len());
            for chunk in wanted.chunks(2) {
                built.extend(
                    chunk
                        .to_vec()
                        .into_par_iter()
                        .map(build_one)
                        .collect::<Vec<_>>(),
                );
            }
            built
        };
        #[cfg(not(feature = "parallel"))]
        let built: Vec<Vec<Tile>> = wanted.iter().copied().map(build_one).collect();
        GraphIndex::from_sections(scatter(&wanted, built), self.perms)
    }

    pub fn build(self) -> GraphIndex {
        let wanted: Vec<IndexPermutation> = self.perms.iter().collect();
        let triples = &self.triples;
        let budget = self.tile_budget;
        let progress = self.progress;
        // The permutations are independent — permute + sort + tile each.
        let build_one = move |perm: IndexPermutation| -> Vec<Tile> {
            let permuted: Vec<Triple> = triples.iter().map(|&t| perm.forward(t)).collect();
            let tiles = build_tiles(permuted, budget);
            if progress {
                crate::build_progress::advance(triples.len() as u64);
            }
            tiles
        };
        // Build the permutations concurrently when the `parallel` feature is on
        // (they share no state); the per-permutation sort inside is also
        // parallel. Output is byte-identical to the serial path.
        #[cfg(feature = "parallel")]
        let built: Vec<Vec<Tile>> = {
            use rayon::iter::{IntoParallelIterator, ParallelIterator};
            wanted.clone().into_par_iter().map(build_one).collect()
        };
        #[cfg(not(feature = "parallel"))]
        let built: Vec<Vec<Tile>> = wanted.iter().copied().map(build_one).collect();
        GraphIndex::from_sections(scatter(&wanted, built), self.perms)
    }
}

/// Place each built section at its permutation's fixed [`IndexPermutation::section_index`]
/// slot, leaving unbuilt permutations empty. The array stays six wide however
/// few permutations a file carries, so every `sections[perm.section_index()]`
/// in the engine keeps working unchanged.
fn scatter(wanted: &[IndexPermutation], built: Vec<Vec<Tile>>) -> [Vec<Tile>; NUM_PERMS] {
    let mut sections: [Vec<Tile>; NUM_PERMS] = Default::default();
    for (perm, tiles) in wanted.iter().zip(built) {
        sections[perm.section_index()] = tiles;
    }
    sections
}

/// The encoded varint length of `v` (LEB128).
fn varint_len(mut v: u64) -> usize {
    let mut n = 1;
    while v >= 0x80 {
        v >>= 7;
        n += 1;
    }
    n
}

/// Incremental encoded-size accounting for one a-group of a tiled section —
/// the running-delta chain of [`build_tiles`], streamable triple by triple.
/// Shared by the in-memory tiler here and the external build's streaming
/// tiler ([`crate::extbuild`]) so both choose IDENTICAL tile boundaries and
/// their outputs stay byte-identical.
pub(crate) struct GroupSizer {
    /// finalized contributions: the a-delta plus every closed b-run
    size: usize,
    num_b: u64,
    cur_b: u32,
    /// the open b-run's c count / last c
    num_c: u64,
    prev_c: u32,
    empty: bool,
}

impl GroupSizer {
    /// Start a group for leading id `a` whose delta base is `prev_a` (the
    /// previous group's leading id; `a` again for a mid-group continuation).
    pub(crate) fn start(a: u32, prev_a: u32) -> Self {
        GroupSizer {
            size: varint_len((a - prev_a) as u64),
            num_b: 0,
            cur_b: 0,
            num_c: 0,
            prev_c: 0,
            empty: true,
        }
    }

    /// Account one `(b, c)` of this group; returns the group's total encoded
    /// size so far (open count varints included).
    pub(crate) fn push(&mut self, b: u32, c: u32) -> usize {
        if self.empty || b != self.cur_b {
            if self.empty {
                self.size += varint_len(b as u64); // first b-run: delta from 0
            } else {
                self.size += varint_len(self.num_c); // close the previous b-run
                self.size += varint_len((b - self.cur_b) as u64);
            }
            self.cur_b = b;
            self.num_c = 0;
            self.prev_c = 0;
            self.num_b += 1;
            self.empty = false;
        }
        self.size += varint_len((c - self.prev_c) as u64);
        self.prev_c = c;
        self.num_c += 1;
        self.total()
    }

    /// The group's total encoded size so far.
    pub(crate) fn total(&self) -> usize {
        self.size
            + if self.empty {
                0
            } else {
                varint_len(self.num_c)
            }
            + varint_len(self.num_b)
    }
}

/// Split sorted, deduped permuted triples into size-targeted tiles: append
/// whole a-groups until the (estimated, near-exact) encoded size would exceed
/// `budget`, then flush — and additionally cut WITHIN an a-group whose own
/// running size exceeds the budget (a mega-group: one predicate or class
/// carrying a large share of the graph, e.g. a 2B-triple `cites` predicate in
/// POS). Split slices become consecutive tiles sharing the leading id — a
/// bound leading id routes to the whole run of covering tiles (see
/// [`GraphIndex::tile_span`]) — which bounds both builder memory and the
/// bytes a remote reader faults for one lookup. Boundary accounting lives in
/// [`GroupSizer`], shared with the external build for byte-identity.
fn build_tiles(mut triples: Vec<Triple>, budget: usize) -> Vec<Tile> {
    #[cfg(feature = "parallel")]
    {
        use rayon::slice::ParallelSliceMut;
        triples.par_sort_unstable();
    }
    #[cfg(not(feature = "parallel"))]
    triples.sort_unstable();
    triples.dedup();
    if triples.is_empty() {
        return Vec::new();
    }

    let make_tile = |run: &[Triple]| -> Tile {
        let mut b = TripleBlockBuilder::new();
        for &t in run {
            b.push(t);
        }
        Tile::local(run[0].0, run[run.len() - 1].0, b.build())
    };

    let mut tiles = Vec::new();
    let mut tile_start = 0usize;
    let mut tile_size = 0usize; // completed groups in the current tile
    let mut prev_a = 0u32;
    let mut i = 0usize;
    while i < triples.len() {
        let a = triples[i].0;
        let mut slice_start = i; // group start, or the last mid-group cut
        let mut sizer = GroupSizer::start(a, prev_a);
        let mut gtotal = 0usize;
        while i < triples.len() && triples[i].0 == a {
            gtotal = sizer.push(triples[i].1, triples[i].2);
            i += 1;
            if gtotal > budget {
                // Mega-group cut: everything buffered — completed groups plus
                // the slice measured so far — becomes one tile; the group
                // continues in a fresh chain with `a` as its own delta base.
                tiles.push(make_tile(&triples[tile_start..i]));
                tile_start = i;
                tile_size = 0;
                prev_a = a;
                slice_start = i;
                sizer = GroupSizer::start(a, a);
                gtotal = 0;
            }
        }
        if slice_start == i {
            continue; // the cut landed exactly on the group's end
        }
        if slice_start > tile_start && tile_size + gtotal > budget {
            tiles.push(make_tile(&triples[tile_start..slice_start]));
            tile_start = slice_start;
            tile_size = 0;
        }
        tile_size += gtotal;
        prev_a = a;
    }
    if tile_start < triples.len() {
        tiles.push(make_tile(&triples[tile_start..]));
    }
    tiles
}

/// The six tiled permutation sections, queryable by triple pattern.
pub struct GraphIndex {
    /// Tiles per permutation (SPO, SOP, PSO, POS, OSP, OPS — see [`ALL_PERMS`]),
    /// ascending in their leading-id ranges; consecutive tiles may share a
    /// leading id when a mega-group was split (see [`build_tiles`]).
    pub(crate) sections: [TileList; NUM_PERMS],
    /// Which of those six slots this file actually carries. Sections outside
    /// the set are empty and must never be consulted — an empty section is
    /// indistinguishable from an empty *graph*, which is why the planner asks
    /// this rather than the section lengths.
    perms: PermSet,
    /// Faults in remote tiles on first scan (`None` for local indexes).
    loader: Option<TileLoader>,
    /// Optional batched fetch for multi-tile scans (`None` falls back to
    /// one [`TileLoader`] call per tile).
    bulk: Option<TileBulkLoader>,
    /// The batched fetch for tiles known to lie far apart (a window of probes):
    /// exact ranges, not rounded out to cache blocks or merged across wide
    /// gaps. `None` falls back to [`bulk`](Self::bulk).
    scattered: Option<TileBulkLoader>,
    /// Set when the loader failed for some tile: results may be incomplete and
    /// the caller must surface an error rather than the partial answer.
    load_failed: std::sync::atomic::AtomicBool,
    /// The reader's concurrent-range fan-out (1 = strictly sequential) — see
    /// [`set_read_concurrency`](Self::set_read_concurrency).
    read_concurrency: usize,
    /// Decompressed tile bodies live here, keyed by `(section, tile)`, faulted
    /// in on first scan and evictable. Phase 0 caps it at unlimited, so a
    /// scanned permutation stays resident exactly as it did in the old per-tile
    /// `OnceLock`s.
    cache: Arc<ChunkCache>,
    /// The number of triples this index holds, when known without reading a
    /// tile (0 = unknown). A named graph's view takes it from the quad
    /// directory, so the planner sizes patterns inside `GRAPH <g>` against the
    /// graph rather than the whole file — see [`Self::size_hint`].
    size_hint: std::sync::atomic::AtomicU64,
    /// Class runs of the subject-leading permutations of a clustered `0x07`
    /// file; `None` for every other permutation.
    clusters: [Option<Arc<ClusterRuns>>; NUM_PERMS],
}

impl GraphIndex {
    fn from_sections(mut sections: [Vec<Tile>; NUM_PERMS], perms: PermSet) -> Self {
        // Move every locally-built/decoded tile body into the shared cache and
        // clear its staging slot, so the descriptors hold routing metadata only.
        let cache = ChunkCache::unlimited_arc();
        for (si, tiles) in sections.iter_mut().enumerate() {
            for (ti, tile) in tiles.iter_mut().enumerate() {
                if let Some(body) = tile.staged.take() {
                    cache.insert((si as u8, ti as u32), Arc::from(body));
                }
            }
        }
        GraphIndex {
            sections: sections.map(TileList::Full),
            perms,
            loader: None,
            bulk: None,
            scattered: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            cache,
            size_hint: std::sync::atomic::AtomicU64::new(0),
            clusters: Default::default(),
        }
    }

    /// Rebuild from tiled sections: per permutation, `(min_a, max_a, block
    /// bytes)` per tile in ascending leading-id order — the v0.2 layout.
    /// Slots outside `perms` must be empty.
    pub fn from_tiles(sections: [Vec<(u32, u32, Vec<u8>)>; NUM_PERMS], perms: PermSet) -> Self {
        let sections = sections.map(|tiles| {
            tiles
                .into_iter()
                .map(|(min_a, max_a, bytes)| Tile::local(min_a, max_a, bytes))
                .collect()
        });
        Self::from_sections(sections, perms)
    }

    /// The permutations this index carries.
    pub fn perms(&self) -> PermSet {
        self.perms
    }

    /// Set the byte cap of this index's tile cache (bounded-export phase 2).
    /// `u64::MAX` (the open-time default) is unlimited — a scanned permutation
    /// stays resident exactly as before. A finite cap evicts least-recently-used
    /// tiles, so a full scan's tile residency is bounded.
    ///
    /// Only safe on an index whose tiles can be **re-faulted** (a remote/ranged
    /// index with a loader). A locally-built index has no loader — an evicted
    /// tile could never be reloaded — so the caller must not cap those; the
    /// bounded-export path caps only the ranged default-graph index and leaves
    /// resident named-graph indexes unlimited (released per slot instead).
    pub fn set_cache_cap(&self, cap: u64) {
        self.cache.set_cap(cap);
    }

    /// The current tile-cache byte cap (`u64::MAX` = unlimited).
    pub fn cache_cap(&self) -> u64 {
        self.cache.cap()
    }

    /// Does this index fault its tiles through a loader (a remote/ranged open)?
    /// Only such an index is safe to give a finite tile-cache cap: a locally
    /// decoded index has no loader, so an evicted tile could never be reloaded.
    pub fn is_lazy(&self) -> bool {
        self.loader.is_some()
    }

    /// A snapshot of the tile cache's observability counters.
    pub fn cache_stats(&self) -> crate::chunk_cache::CacheStats {
        self.cache.stats()
    }

    /// Heap bytes this index holds for as long as it is open, outside its tile
    /// cache: one descriptor per tile of every permutation the file carries
    /// (plus a tile's group directory, if a bound-leading lookup built one),
    /// and — on a ranged open — the two copies of the per-tile byte ranges the
    /// loaders capture. Grows with the tile count, never with what a scan
    /// touches.
    pub fn directory_bytes(&self) -> u64 {
        let tiles: u64 = self.sections.iter().map(|s| s.known_len() as u64).sum();
        let caps: u64 = self.sections.iter().map(TileList::directory_bytes).sum();
        let ranges = if self.loader.is_some() {
            2 * 24 * tiles
        } else {
            0
        };
        caps + ranges
    }

    /// A **remote** index: only the tile directories (leading-id ranges per
    /// permutation, ascending) are known; tile payloads fault in through
    /// `loader` on first scan. Check [`load_incomplete`](Self::load_incomplete)
    /// after evaluating — a failed fetch must become an error, never a
    /// silently smaller result.
    #[allow(clippy::type_complexity)]
    pub fn from_remote_directories(
        directories: [Vec<(u32, u32, Option<(u32, u32, u32, u32)>)>; NUM_PERMS],
        perms: PermSet,
        loader: TileLoader,
    ) -> Self {
        let sections = directories.map(|dir| {
            TileList::Full(
                dir.into_iter()
                    .map(|(min_a, max_a, syn)| Tile::remote(min_a, max_a, syn))
                    .collect(),
            )
        });
        GraphIndex {
            sections,
            perms,
            loader: Some(loader),
            bulk: None,
            scattered: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            cache: ChunkCache::unlimited_arc(),
            size_hint: std::sync::atomic::AtomicU64::new(0),
            clusters: Default::default(),
        }
    }

    /// A remote index whose per-order tile lists are built on first use: an
    /// order the query never routes to costs nothing. `lists[si]` is `None` for
    /// an order the file does not carry. The loaders index the tiles the lists
    /// return.
    pub fn from_remote_deferred(
        lists: [Option<Arc<DeferredTiles>>; NUM_PERMS],
        perms: PermSet,
        loader: TileLoader,
    ) -> Self {
        GraphIndex {
            sections: lists.map(|l| match l {
                Some(l) => TileList::Deferred(l),
                None => TileList::Full(Vec::new()),
            }),
            perms,
            loader: Some(loader),
            bulk: None,
            scattered: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            cache: ChunkCache::unlimited_arc(),
            size_hint: std::sync::atomic::AtomicU64::new(0),
            clusters: Default::default(),
        }
    }

    /// Attach a batched tile fetcher (see [`TileBulkLoader`]): multi-tile
    /// scans prefetch their span through it instead of faulting tile by tile.
    pub fn with_bulk_loader(mut self, bulk: TileBulkLoader) -> Self {
        self.bulk = Some(bulk);
        self
    }

    /// Attach the fetcher for scattered tiles ([`GraphIndex::probe_batch_fault`]):
    /// a probe window's tiles are far apart, so it asks for each one's exact
    /// range instead of the block-aligned, gap-merged spans a scan wants.
    pub fn with_scattered_loader(mut self, scattered: TileBulkLoader) -> Self {
        self.scattered = Some(scattered);
        self
    }

    /// Record each remote tile's encoded (on-disk) byte length, per section —
    /// known to the ranged opener from the tile directory. Powers the join
    /// planner's fatness gates; never triggers a fetch.
    pub(crate) fn set_tile_lens(&mut self, lens: [Vec<u32>; NUM_PERMS]) {
        for (section, ls) in self.sections.iter_mut().zip(lens) {
            if let TileList::Full(section) = section {
                for (tile, l) in section.iter_mut().zip(ls) {
                    tile.len = l;
                }
            }
        }
    }

    /// Record the reader's concurrent-range fan-out (see
    /// [`RangeReader::concurrency`](crate::reader::RangeReader::concurrency)) —
    /// the join planner widens its remote probe budget when round trips
    /// overlap instead of serializing.
    pub(crate) fn set_read_concurrency(&mut self, c: usize) {
        self.read_concurrency = c.max(1);
    }

    /// Record how many triples this index holds (see [`Self::size_hint`]).
    pub(crate) fn set_size_hint(&self, triples: u64) {
        self.size_hint
            .store(triples, std::sync::atomic::Ordering::Relaxed);
    }

    /// The triples this index holds, when it was told without a tile read
    /// (a quad-index view's graph count, a resident per-graph index).
    pub(crate) fn size_hint(&self) -> Option<u64> {
        match self.size_hint.load(std::sync::atomic::Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        }
    }

    /// The reader's concurrent-range fan-out (1 = strictly sequential).
    pub(crate) fn read_concurrency(&self) -> usize {
        self.read_concurrency
    }

    /// Did any tile fetch fail since this index was opened — or since the last
    /// [`reset_load_failure`](Self::reset_load_failure)?
    pub fn load_incomplete(&self) -> bool {
        self.load_failed.load(std::sync::atomic::Ordering::Relaxed)
            || self.sections.iter().any(TileList::load_failed)
    }

    /// Forget recorded fetch failures — the start-of-evaluation reset for a
    /// RESIDENT session, making the incompleteness verdict per-query instead of
    /// per-open (one transient network blip used to fail every later query on
    /// the session). Safe because failed tiles are never cached: the next scan
    /// simply retries them.
    pub fn reset_load_failure(&self) {
        self.load_failed
            .store(false, std::sync::atomic::Ordering::Relaxed);
        for s in &self.sections {
            s.reset_load_failure();
        }
    }

    /// True for a remote/lazy index (tiles fault in over a `RangeReader`). The
    /// join planner uses this to pick read-aware strategies: here a per-row index
    /// probe is a network round-trip, not a memory lookup, so a left-deep scan +
    /// hash join often beats probing a moderately-sized prefix.
    pub fn is_remote(&self) -> bool {
        self.loader.is_some()
    }

    /// Batch-fault the unloaded tiles in `[start, end)` of `section` through
    /// the bulk loader, if one is attached and at least two tiles are missing
    /// (a single missing tile costs the same either way). A failed batch is
    /// not an error here: the tiles stay unloaded and the per-tile loader
    /// retries each one (recording failures) when the scan reaches it.
    /// Fault, in one batch per permutation, the tile-directory leaves the
    /// bound leading ids of `patterns` route to (format `0x06`; a no-op on a
    /// resident directory).
    pub(crate) fn prefault_leaves(&self, patterns: &[Pattern]) {
        let mut want: [std::collections::BTreeSet<usize>; NUM_PERMS] = Default::default();
        let mut subjects: [Vec<u32>; NUM_PERMS] = Default::default();
        for &pat in patterns {
            let perm = Self::best_permutation_in(self.perms, pat);
            let [pa, _, _] = perm.order_pattern(pat);
            let si = perm.section_index();
            if self.clusters[si].is_some() {
                subjects[si].extend(pa);
                continue;
            }
            if let (TileList::Leafed(l), Some(a)) = (&self.sections[si], pa) {
                // Leaves ascend by leading id: the run whose range holds `a`.
                let from = l.leaves.partition_point(|lf| lf.max_a < a);
                for (li, lf) in l.leaves.iter().enumerate().skip(from) {
                    if lf.min_a > a {
                        break;
                    }
                    want[si].insert(li);
                }
            }
        }
        for (si, lis) in want.iter().enumerate() {
            if let TileList::Leafed(l) = &self.sections[si] {
                if !lis.is_empty() {
                    l.ensure_leaves(&lis.iter().copied().collect::<Vec<_>>());
                }
            }
        }
        for (si, a) in subjects.iter().enumerate() {
            if !a.is_empty() {
                self.resolve_batch(si, a);
            }
        }
    }

    /// The absolute stored range of tile `(section, tile)` on a leafed
    /// (`0x06`) index, faulting its directory leaf.
    pub fn tile_range(&self, section: usize, tile: usize) -> Option<(u64, u64)> {
        self.sections.get(section)?.range(tile)
    }

    /// A remote `0x06` index: per stored permutation a two-level tile
    /// directory, tiles faulted through `loader`/`bulk`.
    pub fn from_leafed(
        lists: [Option<Arc<LeafedTiles>>; NUM_PERMS],
        perms: PermSet,
        loader: TileLoader,
    ) -> Self {
        GraphIndex {
            sections: lists.map(|l| match l {
                Some(l) => TileList::Leafed(l),
                None => TileList::Full(Vec::new()),
            }),
            perms,
            loader: Some(loader),
            bulk: None,
            scattered: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            cache: ChunkCache::unlimited_arc(),
            size_hint: std::sync::atomic::AtomicU64::new(0),
            clusters: Default::default(),
        }
    }

    /// Mark permutation section `si` as clustered into class runs (the reader of
    /// a `0x07` file with the cluster root).
    pub fn with_clusters(mut self, si: usize, runs: ClusterRuns) -> Self {
        self.clusters[si] = Some(Arc::new(runs));
        self
    }

    /// The class runs of permutation section `si`, when it is clustered.
    pub fn cluster_runs(&self, si: usize) -> Option<&ClusterRuns> {
        self.clusters.get(si)?.as_deref()
    }

    /// Stored bytes the planner assumes a bound subject reads in clustered
    /// section `si`: one mean tile.
    pub(crate) fn clustered_subject_bytes(&self, si: usize) -> u64 {
        let t = &self.sections[si];
        t.total_bytes() / t.len().max(1) as u64
    }

    /// The tiles of run `run` that can hold subject `a`.
    fn run_span(&self, si: usize, run: usize, a: u32) -> (usize, usize) {
        let Some(cr) = &self.clusters[si] else {
            return (0, 0);
        };
        let (lo, hi) = cr.runs[run];
        let t = &self.sections[si];
        let i = t.first_max_ge_in(a, lo, hi);
        let j = t.first_min_gt_in(a, lo, hi).max(i);
        (i, j)
    }

    /// Does tile `ti` of section `si` hold leading id `a` (not just a range that
    /// covers it)?
    fn tile_holds(&self, si: usize, ti: usize, a: u32) -> bool {
        let Some(entry) = self.tile_entry(si, ti) else {
            return false;
        };
        let Ok(block) = TripleBlock::parse(entry.body()) else {
            return false;
        };
        self.tile_dir(si, ti, &entry, &block).first_at_or_after(a) == Some(a)
    }

    /// Look for each of `subjects` in the runs `order` only: the directory
    /// leaves of every run in one batch, then every candidate tile in one
    /// batch, then the tiles are checked. Returns the run each was found in.
    fn find_in_runs(&self, si: usize, subjects: &[u32], order: &[usize]) -> Vec<Option<usize>> {
        let Some(cr) = &self.clusters[si] else {
            return vec![None; subjects.len()];
        };
        let t = &self.sections[si];
        // 1. The leaves (a no-op on a resident directory).
        if let TileList::Leafed(l) = t {
            let mut lis: Vec<usize> = Vec::new();
            for &a in subjects {
                for &r in order {
                    let (lo, hi) = cr.runs[r];
                    lis.extend(t.leaves_for_in(a, lo, hi));
                }
            }
            lis.sort_unstable();
            lis.dedup();
            l.ensure_leaves(&lis);
        }
        // 2. The candidate tiles.
        let spans: Vec<Vec<(usize, (usize, usize))>> = subjects
            .iter()
            .map(|&a| {
                order
                    .iter()
                    .map(|&r| (r, self.run_span(si, r, a)))
                    .filter(|(_, (i, j))| i < j)
                    .collect()
            })
            .collect();
        if self.is_remote() {
            let mut missing: Vec<usize> = spans
                .iter()
                .flatten()
                .flat_map(|(_, (i, j))| *i..*j)
                .filter(|&ti| !self.tile_resident(si, ti))
                .collect();
            missing.sort_unstable();
            missing.dedup();
            self.bulk_fault(si, &missing);
        }
        // 3. Which tile holds the subject.
        subjects
            .iter()
            .zip(&spans)
            .map(|(&a, cands)| {
                cands
                    .iter()
                    .find(|(_, (i, j))| (*i..*j).any(|ti| self.tile_holds(si, ti, a)))
                    .map(|&(r, _)| r)
            })
            .collect()
    }

    /// The run of clustered section `si` that holds subject `a` (`None`: none
    /// does, or a read failed and was recorded). The run the previous subject
    /// was found in is tried alone first: a join over the subjects of one class
    /// reads that class's run and no other. A subject in no guessed run is looked
    /// for in all the others at once, one batch of leaves and one of tiles.
    fn resolve_run(&self, si: usize, a: u32) -> Option<usize> {
        let cr = self.clusters[si].as_ref()?;
        if let Some(known) = cr.recall(a) {
            return known;
        }
        let hint = cr.hint.load(std::sync::atomic::Ordering::Relaxed);
        let mut found = self.find_in_runs(si, &[a], &[hint])[0];
        if found.is_none() {
            let rest: Vec<usize> = (0..cr.runs.len()).filter(|&r| r != hint).collect();
            found = self.find_in_runs(si, &[a], &rest)[0];
        }
        if found.is_some() || !self.load_incomplete() {
            cr.remember(a, found);
        }
        found
    }

    /// [`resolve_run`](Self::resolve_run) for a whole batch of subjects (a probe
    /// window): the first fixes the guess, the rest are looked for in that run
    /// in one coalesced read, and only the ones it does not hold fall back.
    fn resolve_batch(&self, si: usize, subjects: &[u32]) {
        let Some(cr) = self.clusters[si].as_ref() else {
            return;
        };
        let mut todo: Vec<u32> = subjects
            .iter()
            .copied()
            .filter(|&a| cr.recall(a).is_none())
            .collect();
        todo.sort_unstable();
        todo.dedup();
        let Some(&first) = todo.first() else {
            return;
        };
        let _ = self.resolve_run(si, first);
        let hint = cr.hint.load(std::sync::atomic::Ordering::Relaxed);
        let rest: Vec<u32> = todo[1..]
            .iter()
            .copied()
            .filter(|&a| cr.recall(a).is_none())
            .collect();
        let guessed = self.find_in_runs(si, &rest, &[hint]);
        let mut missed: Vec<u32> = Vec::new();
        for (&a, g) in rest.iter().zip(guessed) {
            match g {
                Some(r) => cr.remember(a, Some(r)),
                None => missed.push(a),
            }
        }
        if missed.is_empty() {
            return;
        }
        let others: Vec<usize> = (0..cr.runs.len()).filter(|&r| r != hint).collect();
        let found = self.find_in_runs(si, &missed, &others);
        if !self.load_incomplete() {
            for (&a, f) in missed.iter().zip(found) {
                cr.remember(a, f);
            }
        }
    }

    /// The least and greatest leading id any tile of section `si` covers
    /// (over every run of a clustered one); `None` for an empty section.
    pub(crate) fn leading_bounds(&self, si: usize) -> Option<(u32, u32)> {
        let t = &self.sections[si];
        let ranges: Vec<(usize, usize)> = match &self.clusters[si] {
            Some(cr) => cr.runs.clone(),
            None => vec![(0, t.len())],
        };
        let mut out: Option<(u32, u32)> = None;
        for (lo, hi) in ranges.into_iter().filter(|(l, h)| l < h) {
            let (a, b) = (t[lo].leading_range().0, t[hi - 1].leading_range().1);
            out = Some(out.map_or((a, b), |(x, y)| (x.min(a), y.max(b))));
        }
        out
    }

    /// The first leading id at or after `sid` that some tile of section `si` can
    /// hold (from the directory alone), or `None` past the last tile.
    pub(crate) fn next_tile_start(&self, si: usize, sid: u32) -> Option<u32> {
        let t = &self.sections[si];
        let ranges: Vec<(usize, usize)> = match &self.clusters[si] {
            Some(cr) => cr.runs.clone(),
            None => vec![(0, t.len())],
        };
        let mut best: Option<u32> = None;
        for (lo, hi) in ranges {
            let i = t.first_max_ge_in(sid, lo, hi);
            if i < hi {
                let x = t[i].leading_range().0.max(sid);
                best = Some(best.map_or(x, |b| b.min(x)));
            }
        }
        best
    }

    /// The tile spans that can hold leading id `pa` (every tile when unbound),
    /// from the directory alone: one span, or one per class run that has a
    /// tile covering a bound subject of a clustered section. For a reader that
    /// cannot afford the reads [`tile_span`](Self::tile_span) makes to place a
    /// subject in its run.
    pub(crate) fn routed_spans(&self, si: usize, pa: Option<u32>) -> Vec<(usize, usize)> {
        match (&self.clusters[si], pa) {
            (Some(cr), Some(a)) => {
                let spans: Vec<(usize, usize)> = (0..cr.runs.len())
                    .map(|r| self.run_span(si, r, a))
                    .filter(|(i, j)| i < j)
                    .collect();
                for &(i, j) in &spans {
                    self.sections[si].ensure_span(i, j);
                }
                spans
            }
            _ => vec![self.tile_span(si, pa)],
        }
    }

    fn prefetch_span(&self, section: usize, start: usize, end: usize) {
        let missing: Vec<usize> = (start..end)
            .filter(|&ti| !self.tile_resident(section, ti))
            .collect();
        self.bulk_fault(section, &missing);
    }

    /// Is tile `(section, tile)`'s body currently resident in the cache?
    fn tile_resident(&self, section: usize, tile: usize) -> bool {
        self.cache.contains((section as u8, tile as u32))
    }

    /// Bulk-fault a set of (possibly scattered, ascending) missing tile indices in
    /// `section` through the bulk loader in one coalesced read, storing each image.
    /// No-op without a bulk loader or for fewer than two tiles (a single fault
    /// costs the same via the per-tile loader). A failed batch leaves the tiles
    /// unloaded for the per-tile loader to retry. Shared by the consecutive-span
    /// scan prefetch and the scattered batch-probe prefetch.
    fn bulk_fault(&self, section: usize, tiles: &[usize]) {
        self.fault_with(self.bulk.as_ref(), section, tiles);
    }

    /// [`bulk_fault`](Self::bulk_fault) through `loader`.
    fn fault_with(&self, loader: Option<&TileBulkLoader>, section: usize, tiles: &[usize]) {
        if tiles.len() < 2 {
            return;
        }
        let Some(bulk) = loader else { return };
        if let Some(images) = bulk(section, tiles) {
            if images.len() == tiles.len() {
                for (&ti, img) in tiles.iter().zip(images) {
                    self.cache
                        .insert((section as u8, ti as u32), Arc::from(img));
                }
            }
        }
    }

    /// Batch-fault the tiles a set of upcoming **probe** patterns will route to,
    /// before they are probed one at a time. Each probe (a bound leading id)
    /// routes to a single tile; gathered across the batch those tiles are
    /// scattered, so faulting them together turns N sequential remote round trips
    /// into a few coalesced parallel reads — the read-amplification win for a
    /// label-heavy join over a lazy reader. Honors the synopsis prune (never
    /// fetches a tile a bound secondary proves can't match). No-op for a local
    /// index or when fewer than two tiles per section need faulting.
    ///
    /// The batch is **bounded** ([`ProbeBatch`]): it covers the leading
    /// patterns whose tiles fit, and returns how many that is (at least one for
    /// a non-empty list; all of them for a local index). A caller with more
    /// patterns asks again for the rest after probing these.
    pub(crate) fn prefetch_probe_tiles(&self, patterns: &[Pattern]) -> usize {
        if self.bulk.is_none() {
            return patterns.len();
        }
        self.prefault_leaves(patterns);
        let mut batch = ProbeBatch::default();
        let mut covered = 0;
        for &pat in patterns {
            if !self.probe_batch_add(&mut batch, pat) {
                break;
            }
            covered += 1;
        }
        self.probe_batch_fault(batch);
        covered
    }

    /// Add `pat`'s tiles to `batch` if the batch stays within its bounds
    /// (tiles, stored bytes); `false` leaves the batch as it was. The first
    /// pattern of a batch is always accepted: if it alone is wider than a
    /// batch (a bound predicate with nothing else routes to its whole span) it
    /// adds no tiles, and its own scan faults what it reads. The directory
    /// leaves of `pat` should be resident
    /// ([`prefault_leaves`](Self::prefault_leaves)), or this faults them one
    /// at a time.
    pub(crate) fn probe_batch_add(&self, batch: &mut ProbeBatch, pat: Pattern) -> bool {
        let perm = Self::best_permutation(pat);
        let [pa, pb, pc] = perm.order_pattern(pat);
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, pa);
        // This pattern's tiles that are not already wanted or resident.
        let mut fresh: Vec<usize> = Vec::new();
        for ti in start..end {
            if self.sections[si][ti].syn_admits(pb, pc)
                && !self.tile_resident(si, ti)
                && !batch.want[si].contains(&ti)
            {
                fresh.push(ti);
            }
        }
        let fresh_bytes: u64 = fresh
            .iter()
            .map(|&ti| self.sections[si][ti].len as u64)
            .sum();
        if batch.tiles + fresh.len() > PROBE_WINDOW_TILES
            || batch.bytes + fresh_bytes > self.probe_window_bytes()
        {
            if batch.patterns > 0 {
                return false;
            }
            batch.patterns = 1;
            return true;
        }
        batch.tiles += fresh.len();
        batch.bytes += fresh_bytes;
        batch.want[si].extend(fresh);
        batch.patterns += 1;
        true
    }

    /// Fault every tile of `batch`: per section one coalesced, parallel read.
    pub(crate) fn probe_batch_fault(&self, batch: ProbeBatch) {
        for (si, set) in batch.want.iter().enumerate() {
            let tiles: Vec<usize> = set.iter().copied().collect();
            self.fault_with(self.scattered.as_ref().or(self.bulk.as_ref()), si, &tiles);
        }
    }

    /// The stored bytes one [`ProbeBatch`] may fault: an eighth of the tile
    /// cache's cap, within 256 KiB–64 MiB. A tile decodes to a few times its
    /// stored size and the window must stay resident until its probes have
    /// run, so a window that outgrew the cache would evict its own early
    /// tiles. With the CLI's default budget (a 128 MiB tile cache) that is
    /// 16 MiB: tens of coalesced requests, enough to keep the reader's
    /// fan-out busy.
    fn probe_window_bytes(&self) -> u64 {
        const KIB: u64 = 1 << 10;
        match self.cache.cap() {
            u64::MAX => 64 * KIB * KIB,
            cap => (cap / 8).clamp(256 * KIB, 64 * KIB * KIB),
        }
    }

    /// The tile's cache entry, faulting its block image in through the loader if
    /// remote. Returns an owned [`CacheEntry`] handle (an `Arc`) the caller
    /// holds for the duration of one parse/scan — valid even if a later eviction
    /// drops the map slot (the handout invariant).
    ///
    /// A FAILED fetch records the failure and caches nothing (returns `None`),
    /// so a later evaluation retries the tile — a transient network error must
    /// not permanently poison a long-lived (resident) session. A local tile
    /// with no body returns an empty entry (constructed empty on purpose).
    fn tile_entry(&self, section: usize, tile: usize) -> Option<Arc<CacheEntry>> {
        // A query run under a memory ceiling ([`crate::guard`]) stops here.
        crate::guard::check();
        let key = (section as u8, tile as u32);
        if let Some(e) = self.cache.get(key) {
            return Some(e);
        }
        match &self.loader {
            Some(load) => match load(section, tile) {
                Some(bytes) => Some(self.cache.insert(key, Arc::from(bytes))),
                None => {
                    self.load_failed
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    None
                }
            },
            None => Some(self.cache.insert(key, Arc::from(Vec::new()))),
        }
    }

    /// Tile `(section, tile)`'s a-group directory, held in its cache entry and
    /// built from `block` (that entry's parsed body) on first use. Its bytes
    /// are charged to the tile cache when built, so a capped cache bounds the
    /// directories too, and they are evicted with their tile.
    fn tile_dir<'e>(
        &self,
        section: usize,
        tile: usize,
        entry: &'e Arc<CacheEntry>,
        block: &TripleBlock<'_>,
    ) -> &'e GroupDirectory {
        let (dir, built) = entry.group_dir(|| block.group_directory());
        if built {
            self.cache.charge_extra(
                (section as u8, tile as u32),
                entry,
                dir.heap_bytes() + std::mem::size_of::<GroupDirectory>() as u64,
            );
        }
        dir
    }

    /// The tile's decompressed block image as a shared handle, for the file
    /// writer (local indexes, all tiles resident). Empty on a miss.
    pub(crate) fn tile_body(&self, section: usize, tile: usize) -> Arc<[u8]> {
        self.tile_entry(section, tile)
            .map(|e| e.body_arc())
            .unwrap_or_else(|| Arc::from(Vec::new()))
    }

    /// Total triple count (sum of the SPO tiles' zone counts). For a remote
    /// index this faults in the SPO tiles — prefer the header's quad count.
    pub fn triple_count(&self) -> u32 {
        self.prefetch_span(0, 0, self.sections[0].len());
        (0..self.sections[0].len())
            .filter_map(|ti| {
                let entry = self.tile_entry(0, ti)?;
                TripleBlock::parse(entry.body())
                    .ok()
                    .map(|b| b.zone().count)
            })
            // A hostile tile can claim any count: saturate, never overflow.
            .fold(0u32, u32::saturating_add)
    }

    /// Stream **every** triple of one stored permutation in that section's own
    /// order, as `(a, b, c)` in the permutation's column order (SPO gives
    /// `(s, p, o)`, OSP gives `(o, s, p)`, …).
    ///
    /// The access a whole-graph pass needs — `rete recard`'s streaming card
    /// derivation reads SPO for out-degrees and OSP for in-degrees and literal
    /// objects — and one [`scan_iter`](Self::scan_iter) cannot give, because a
    /// fully unbound pattern always routes to SPO. Tiles fault with the same
    /// geometric prefetch as a scan and are decoded one at a time; nothing is
    /// retained beyond the tile cache, so memory follows its cap, not the
    /// section. A failed fault is recorded in
    /// [`load_incomplete`](Self::load_incomplete) and the tile is skipped, as
    /// everywhere else. Returns `false` when the file does not store `perm`.
    pub fn for_each_in_order(
        &self,
        perm: IndexPermutation,
        f: &mut dyn FnMut(u32, u32, u32),
    ) -> bool {
        if !self.perms.contains(perm) {
            return false;
        }
        let si = perm.section_index();
        let end = self.sections[si].len();
        let mut window = PREFETCH_WINDOW_START;
        for ti in 0..end {
            if !self.tile_resident(si, ti) {
                self.prefetch_span(si, ti, (ti + window).min(end));
                window = window.saturating_mul(2).min(PREFETCH_WINDOW_MAX);
            }
            if let Some(entry) = self.tile_entry(si, ti) {
                if let Ok(block) = TripleBlock::parse(entry.body()) {
                    for (a, b, c) in block.scan(None, None, None) {
                        f(a, b, c);
                    }
                }
            }
        }
        true
    }

    /// The tiles of each permutation section (in `ALL_PERMS` order), for the
    /// file writer.
    pub fn tile_sections(&self) -> [&TileList; NUM_PERMS] {
        [
            &self.sections[0],
            &self.sections[1],
            &self.sections[2],
            &self.sections[3],
            &self.sections[4],
            &self.sections[5],
        ]
    }

    /// The permutation selected for a pattern: the one with the longest bound
    /// prefix. Ties keep the canonical SPO order (then POS, then OSP), which
    /// makes provenance stable for unbound or equally selective shapes and routes
    /// a fully unbound pattern to the SPO block rather than fetching all three.
    ///
    /// Six-permutation shorthand for [`best_permutation_in`]. Its answer is
    /// always inside [`PermSet::CORE`] (see `perm_routing_never_leaves_core`),
    /// so a file with any legal [`PermSet`] routes identically — but call the
    /// `_in` form wherever the file's set is at hand, so that stays true by
    /// construction rather than by argument.
    ///
    /// [`best_permutation_in`]: GraphIndex::best_permutation_in
    pub fn best_permutation(pattern: Pattern) -> IndexPermutation {
        Self::best_permutation_in(PermSet::ALL, pattern)
    }

    /// [`best_permutation`](GraphIndex::best_permutation) restricted to the
    /// permutations a file carries.
    pub fn best_permutation_in(perms: PermSet, pattern: Pattern) -> IndexPermutation {
        let mut best = IndexPermutation::Spo;
        let mut best_score = best.leading_bound(pattern);
        for perm in perms.iter() {
            let score = perm.leading_bound(pattern);
            if score > best_score {
                best = perm;
                best_score = score;
            }
        }
        best
    }

    /// Choose a permutation that **routes** on `pattern`'s bound prefix *and*
    /// streams sorted on the canonical column `sort_col` (0=subject, 1=predicate,
    /// 2=object) — i.e. `sort_col` is the leading *free* component after the bound
    /// prefix. Returns `None` if `sort_col` is itself bound (then every row shares
    /// that value — any permutation is "sorted" on it) or no permutation qualifies.
    /// This is the precondition a merge join needs: both inputs sorted on the join
    /// key. Among qualifiers, the longest bound prefix (best routing) wins.
    ///
    /// Six-permutation shorthand for [`permutation_sorted_on_in`].
    ///
    /// [`permutation_sorted_on_in`]: GraphIndex::permutation_sorted_on_in
    pub fn permutation_sorted_on(pattern: Pattern, sort_col: usize) -> Option<IndexPermutation> {
        Self::permutation_sorted_on_in(PermSet::ALL, pattern, sort_col)
    }

    /// [`permutation_sorted_on`](GraphIndex::permutation_sorted_on) restricted
    /// to the permutations a file carries — **and it will not trade routing for
    /// sort order**.
    ///
    /// A qualifier must match the bound prefix that
    /// [`best_permutation_in`](GraphIndex::best_permutation_in) achieves.
    /// With all six that costs nothing: every ordering of `(s, p, o)` exists, so
    /// "the bound components first, then `sort_col`" is always available at the
    /// maximal prefix. With [`PermSet::CORE`] it is what stops a merge join from
    /// buying its co-sorted stream with a **whole-section scan** — e.g.
    /// `?s :p ?o` sorted on `?s` has no `P*S` order left, and the only remaining
    /// sorted-on-subject stream is an unrouted SPO scan of the entire graph. The
    /// merge seed then declines and the hash/probe path answers, which is
    /// strictly the cheaper plan.
    pub fn permutation_sorted_on_in(
        perms: PermSet,
        pattern: Pattern,
        sort_col: usize,
    ) -> Option<IndexPermutation> {
        let bound = [
            pattern.0.is_some(),
            pattern.1.is_some(),
            pattern.2.is_some(),
        ];
        if bound[sort_col] {
            return None;
        }
        let routed = Self::best_permutation_in(perms, pattern).leading_bound(pattern);
        let mut best: Option<(IndexPermutation, usize)> = None;
        for perm in perms.iter() {
            let roles = perm.roles();
            let lead = perm.leading_bound(pattern);
            // The component at slot `lead` is the first free one; it must be the
            // sort column (so the stream is sorted on it after the bound prefix).
            if lead < 3 && roles[lead] == sort_col && best.map(|(_, s)| lead > s).unwrap_or(true) {
                best = Some((perm, lead));
            }
        }
        best.filter(|&(_, lead)| lead >= routed).map(|(p, _)| p)
    }

    /// Match one already-decoded serialized permutation block (a single v0.1
    /// section or one v0.2 tile). This is the core primitive for range-routed
    /// readers: the caller fetches only the selected payload, then this scans
    /// it as if it came from a full [`GraphIndex`].
    pub fn match_serialized_block(
        bytes: &[u8],
        permutation: IndexPermutation,
        pattern: Pattern,
    ) -> Vec<Triple> {
        let [pa, pb, pc] = permutation.order_pattern(pattern);
        let mut out: Vec<Triple> = TripleBlock::parse(bytes)
            .ok()
            .filter(|b| b.zone().may_contain(pa, pb, pc))
            .map(|b| b.scan(pa, pb, pc))
            .into_iter()
            .flatten()
            .map(move |abc| permutation.back(abc))
            .collect();
        out.sort_unstable();
        out
    }

    /// All triples matching `pattern`, returned in canonical `(s, p, o)` order.
    ///
    /// Thin eager wrapper over [`scan_iter`](Self::scan_iter): collect the lazy
    /// stream and restore the canonical sort (the stream is sorted in the chosen
    /// permutation's order, which differs from canonical once `perm.back`
    /// permutes the free components).
    pub fn match_pattern(&self, pattern: Pattern) -> Vec<Triple> {
        let mut out: Vec<Triple> = self.scan_iter(pattern).collect();
        out.sort_unstable();
        out
    }

    /// Lazily stream the triples matching `pattern` in canonical `(s, p, o)`
    /// order *within the chosen permutation* — the streaming entry point for
    /// callers that can stop early (ASK, `LIMIT`, BGP probes) or that don't need
    /// the canonical re-sort. Decodes only the matching groups (see
    /// [`TripleBlock::scan`]); a malformed/absent block yields nothing rather
    /// than panicking. The permutation is chosen for the longest bound prefix.
    pub fn scan_iter(&self, pattern: Pattern) -> impl Iterator<Item = Triple> + '_ {
        self.scan_iter_with(pattern, Self::best_permutation(pattern), None, None)
    }

    /// [`scan_iter`](Self::scan_iter) over a slice of the tiles it would
    /// visit: `cut` is given the routed tile span `[start, end)` and returns
    /// the part to keep. A triple lies in one tile, so scans over disjoint
    /// slices covering the span yield every match exactly once.
    pub(crate) fn scan_iter_cut<'a>(
        &'a self,
        pattern: Pattern,
        cut: &dyn Fn(usize, usize) -> (usize, usize),
    ) -> impl Iterator<Item = Triple> + 'a {
        self.scan_iter_with(pattern, Self::best_permutation(pattern), Some(cut), None)
    }

    /// What [`scan_iter`](Self::scan_iter) *would* fetch for `pattern`, computed
    /// from the tile directories alone — **no tile is fetched, decoded, or even
    /// looked at**. This is the cost-preview half of the routed scan: it walks
    /// the same three decisions the scan makes (permutation, routed tile span,
    /// synopsis prune) and reports what survives, so a caller can say "this dump
    /// will pull 16 MB of index, not 376 MB" before starting it.
    ///
    /// It is a preview, not a promise: `tile_bytes` is an **upper bound** on the
    /// index bytes the scan reads (an admitted tile can still be rejected by its
    /// in-tile zone map once in hand, and a re-scan of an already-faulted tile
    /// reads nothing), and it says nothing about the dictionary, which is where
    /// a term-resolving dump's real cost usually lives.
    pub fn scan_plan(&self, pattern: Pattern) -> ScanPlan {
        let perm = Self::best_permutation_in(self.perms, pattern);
        let [_, pb, pc] = perm.order_pattern(pattern);
        let si = perm.section_index();
        let tiles = &self.sections[si];
        let (start, end) = self.tile_span(si, perm.order_pattern(pattern)[0]);
        let mut admitted = 0usize;
        let mut tile_bytes = 0u64;
        for ti in start..end {
            let t = &tiles[ti];
            if t.syn_admits(pb, pc) {
                admitted += 1;
                tile_bytes += t.encoded_len();
            }
        }
        ScanPlan {
            permutation: perm,
            tiles_total: tiles.len(),
            tiles_routed: end - start,
            tiles_admitted: admitted,
            tile_bytes,
            section_bytes: tiles.total_bytes(),
        }
    }

    /// One bounded, **resumable** slice of `pattern`'s matches: the pull form of
    /// [`scan_iter`](Self::scan_iter), for a caller that cannot hold a borrow
    /// across calls.
    ///
    /// Returns `(triples, next_cursor, done)`; start at `cursor = 0` and feed the
    /// returned cursor back until `done`. The whole resume state is one opaque
    /// `u64` — `(tile index, next a-group id)` — so no iterator, no thread and no
    /// self-referential struct has to survive between calls. That is the shape a
    /// foreign-function boundary needs: a wasm module cannot suspend a Rust
    /// iterator mid-scan and hand control back to its host, but it can be handed
    /// a `u64` and told to carry on.
    ///
    /// `max_rows` is a **floor**, not a hard cut: a batch always ends on an
    /// a-group boundary of the chosen permutation, so no group is ever split
    /// across two calls and the overshoot is one group's fanout. Nothing is ever
    /// rescanned ([`TripleBlock::scan_resume`] jumps straight to the group
    /// through the tile's directory), so a full drain is O(n) overall — unlike
    /// `(offset, limit)`, which is O(n²/limit).
    ///
    /// Every call either yields at least one row or reports `done`, so a caller
    /// never has to guard against an empty non-final batch.
    ///
    /// **Order.** Rows arrive in the chosen permutation's order and each batch is
    /// re-sorted canonically, exactly as [`match_pattern`](Self::match_pattern)
    /// does for a whole result. For a fully unbound pattern (routed to SPO) that
    /// makes the concatenation of every batch identical to `match_pattern`'s
    /// output; for a bound pattern the *set* is identical but the canonical
    /// re-sort is per batch rather than global.
    ///
    /// **Memory** is O(`max_rows` + one a-group + the tiles faulted so far) —
    /// never O(matches). That is the whole point: an unbounded `(?s ?p ?o)` over
    /// a 26-million-quad graph used to materialize every match before the caller
    /// saw one row.
    pub fn scan_batch(
        &self,
        pattern: Pattern,
        cursor: u64,
        max_rows: usize,
    ) -> (Vec<Triple>, u64, bool) {
        let max_rows = max_rows.max(1);
        let perm = Self::best_permutation(pattern);
        let [pa, pb, pc] = perm.order_pattern(pattern);
        let si = perm.section_index();
        // Same routing as `scan_iter_with`: the tile directory alone decides the
        // span, so this is free on a lazy/remote open.
        let (start, end) = self.tile_span(si, pa);
        let mut ti = ((cursor >> 32) as usize).max(start);
        let mut from_a = cursor as u32;
        let mut out: Vec<Triple> = Vec::new();
        // Same geometric prefetch ramp as the streaming scan: a batch that stops
        // after one tile never fetches the ones behind it.
        let mut window = PREFETCH_WINDOW_START;
        while ti < end {
            // Synopsis pre-fault prune — no tile is fetched to reject it.
            if !self.sections[si][ti].syn_admits(pb, pc) {
                ti += 1;
                from_a = 0;
                continue;
            }
            if !self.tile_resident(si, ti) {
                self.prefetch_span(si, ti, (ti + window).min(end));
                window = window.saturating_mul(2).min(PREFETCH_WINDOW_MAX);
            }
            let entry = self.tile_entry(si, ti);
            let mut resume_at: Option<u32> = None;
            if let Some(block) = entry
                .as_deref()
                .and_then(|e| TripleBlock::parse(e.body()).ok())
                .filter(|b| b.zone().may_contain(pa, pb, pc))
            {
                let rows = match pa {
                    // A bound leading id is one a-group; it cannot be split
                    // inside a tile, only across tiles (a mega-group split), so
                    // there is nothing finer to resume at than the tile.
                    Some(a) => {
                        let dir = self.tile_dir(si, ti, entry.as_ref().unwrap(), &block);
                        block.scan_from(dir, a, pb, pc)
                    }
                    // Fresh tile: no directory needed, so a straight drain never
                    // pays for one.
                    None if from_a == 0 => block.scan(None, pb, pc),
                    None => {
                        let dir = self.tile_dir(si, ti, entry.as_ref().unwrap(), &block);
                        block.scan_resume(dir, from_a, pb, pc)
                    }
                };
                // Only a leading-unbound scan can be resumed mid-tile: with `pa`
                // bound there is one a-group and `scan_from` always restarts it
                // from the top, so cutting inside it would duplicate rows.
                let splittable = pa.is_none();
                let mut last_a: Option<u32> = None;
                for abc in rows {
                    // Full, and this row opens a new a-group ⇒ cut here and
                    // resume at this group. The row is dropped, not lost: the
                    // next call re-enters at exactly this id.
                    if splittable && out.len() >= max_rows && last_a != Some(abc.0) {
                        resume_at = Some(abc.0);
                        break;
                    }
                    last_a = Some(abc.0);
                    out.push(perm.back(abc));
                }
            }
            if let Some(a) = resume_at {
                out.sort_unstable();
                return (out, ((ti as u64) << 32) | a as u64, false);
            }
            ti += 1;
            from_a = 0;
            if out.len() >= max_rows {
                break;
            }
        }
        let done = ti >= end;
        out.sort_unstable();
        (out, (ti as u64) << 32, done)
    }

    /// Stream `pattern`'s matches **sorted on the canonical column `sort_col`**
    /// (0=subject, 1=predicate, 2=object): the stream's `sort_col` values are
    /// ascending. Chooses a permutation that routes on the bound prefix *and*
    /// orders by `sort_col` ([`permutation_sorted_on`](Self::permutation_sorted_on));
    /// `None` when none qualifies (e.g. `sort_col` is itself bound). The
    /// precondition for feeding a [merge join](crate::bgp): both inputs sorted on
    /// the shared join key.
    pub(crate) fn scan_iter_sorted_on(
        &self,
        pattern: Pattern,
        sort_col: usize,
    ) -> Option<impl Iterator<Item = Triple> + '_> {
        let perm = Self::permutation_sorted_on_in(self.perms, pattern, sort_col)?;
        // A clustered permutation is sorted by subject only within a run: with no
        // bound prefix the stream is not sorted on its leading column.
        if self.clusters[perm.section_index()].is_some() && perm.leading_bound(pattern) == 0 {
            return None;
        }
        Some(self.scan_iter_with(pattern, perm, None, None))
    }

    /// The permutation [`scan_iter_sorted_on`](Self::scan_iter_sorted_on)
    /// streams `pattern` from, and where the sort column sits in it (the
    /// number of bound leading columns).
    fn sorted_route(&self, pattern: Pattern, sort_col: usize) -> Option<(IndexPermutation, usize)> {
        let perm = Self::permutation_sorted_on_in(self.perms, pattern, sort_col)?;
        let lead = perm.leading_bound(pattern);
        if self.clusters[perm.section_index()].is_some() && lead == 0 {
            return None;
        }
        Some((perm, lead))
    }

    /// [`scan_iter_sorted_on`](Self::scan_iter_sorted_on), keeping the triples
    /// whose `sort_col` value `v` has `lo <= v < hi`: a slice of the sorted
    /// stream, cut by value. Tiles the directory proves outside the range are
    /// not read. Slices over consecutive ranges yield every triple of the
    /// whole stream once, in its order.
    pub(crate) fn scan_iter_sorted_between(
        &self,
        pattern: Pattern,
        sort_col: usize,
        lo: u32,
        hi: u64,
    ) -> Option<impl Iterator<Item = Triple> + '_> {
        let (perm, lead) = self.sorted_route(pattern, sort_col)?;
        let key = move |t: &Triple| match sort_col {
            0 => t.0,
            1 => t.1,
            _ => t.2,
        };
        Some(
            self.scan_iter_with(pattern, perm, None, Some((lead, lo, hi)))
                .filter(move |t| {
                    let v = key(t);
                    lo <= v && u64::from(v) < hi
                }),
        )
    }

    /// Values that cut `pattern`'s stream sorted on `sort_col` into `n` slices
    /// of about as many tiles each (`n - 1` ascending values: slice `i` is
    /// `cuts[i - 1] <= v < cuts[i]`), and how many tiles the stream routes to.
    /// From the directory alone. `None` when the stream cannot be sorted on
    /// the column, or a tile has no range for it.
    pub(crate) fn sorted_cuts(
        &self,
        pattern: Pattern,
        sort_col: usize,
        n: usize,
    ) -> Option<(Vec<u32>, usize)> {
        let (perm, lead) = self.sorted_route(pattern, sort_col)?;
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, perm.order_pattern(pattern)[0]);
        let tiles = end - start;
        let mut cuts = Vec::with_capacity(n.saturating_sub(1));
        for k in 1..n.max(1) {
            let ti = start + (tiles as u128 * k as u128 / n as u128) as usize;
            if ti >= end {
                break;
            }
            cuts.push(self.sections[si][ti].column_range(lead)?.0);
        }
        cuts.sort_unstable();
        Some((cuts, tiles))
    }

    /// Stream `pattern`'s matches using a **specific** permutation `perm`; the
    /// stream is sorted in `perm`'s `(a, b, c)` order. Shared core of `scan_iter`
    /// and `scan_iter_sorted_on`.
    fn scan_iter_with(
        &self,
        pattern: Pattern,
        perm: IndexPermutation,
        cut: Option<&dyn Fn(usize, usize) -> (usize, usize)>,
        // `(column, lo, hi)`: skip the tiles whose range of that column the
        // directory proves outside `lo <= v < hi` (the caller filters rows).
        between: Option<(usize, u32, u64)>,
    ) -> impl Iterator<Item = Triple> + '_ {
        // Route: a bound leading id binary-searches the tile directory to exactly
        // one tile (groups are never split across tiles); an unbound one chains
        // every tile's cursor. Within a tile, a bound leading scan jumps to its
        // a-group via the tile's lazily-built group directory — built on first
        // use, costing one walk of that (budget-sized) tile.
        let [pa, pb, pc] = perm.order_pattern(pattern);
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, pa);
        let (start, end) = match cut {
            Some(cut) => {
                let (lo, hi) = cut(start, end);
                let lo = lo.clamp(start, end);
                (lo, hi.clamp(lo, end))
            }
            None => (start, end),
        };
        // Coalesce tile faults, but ramp the prefetch window geometrically as
        // the scan advances rather than fetching the whole span up front: a
        // small `LIMIT` (which stops pulling early) then never faults tiles
        // past the rows it needs, while a full scan still batches into a
        // handful of coalesced reads (4, 8, 16, … tiles). A bound leading scan
        // spans a single tile, so the prefetch no-ops and it faults just that
        // one tile, unchanged.
        // A cut scan (a shard of a grouped query) is read to its end: it starts
        // at the largest batch, so a slice over a network is a request or two,
        // not a ramp of its own.
        let window = std::cell::Cell::new(if cut.is_some() {
            PREFETCH_WINDOW_MAX
        } else {
            PREFETCH_WINDOW_START
        });
        (start..end)
            // Synopsis pre-fault prune: drop a routed tile the directory proves
            // can't match a bound secondary component, **without fetching it**
            // (the remote win — a negative/sparse lookup costs zero tile reads).
            // A bound leading id routes to a single tile, so this is where it
            // bites; a fully-unbound leading scan leaves `pb`/`pc` unbound, so
            // `syn_admits` keeps every tile, unchanged.
            .filter(move |&ti| {
                let t = &self.sections[si][ti];
                t.syn_admits(pb, pc)
                    && between.is_none_or(|(col, lo, hi)| {
                        t.column_range(col)
                            .is_none_or(|(min, max)| max >= lo && u64::from(min) < hi)
                    })
            })
            .flat_map(move |ti| {
                // Fault in (if remote), parse (untrusted bytes ⇒ `None` on
                // malformed), and zone-prune per tile, then stream the
                // matching groups.
                if !self.tile_resident(si, ti) {
                    let w = window.get();
                    self.prefetch_span(si, ti, (ti + w).min(end));
                    window.set(w.saturating_mul(2).min(PREFETCH_WINDOW_MAX));
                }
                // A failed fetch is recorded by the index and scans as empty.
                let rows: Vec<Triple> = self.tile_rows(si, ti, pa, pb, pc).unwrap_or_default();
                rows.into_iter()
            })
            .map(move |abc| perm.back(abc))
    }

    /// The rows of tile `ti` of section `si` matching `(pa, pb, pc)` (in that
    /// section's `a, b, c` order). The tile is decoded once and its matching
    /// rows materialized into an owned `Vec`, so the borrowed block/cursor never
    /// outlives the tile's cache handle (an `Arc` dropped on return); the buffer
    /// is bounded to one tile's rows. `None` = the tile could not be fetched
    /// (the failure is recorded, see [`Self::load_incomplete`]); a malformed or
    /// zone-pruned tile yields no rows.
    fn tile_rows(
        &self,
        si: usize,
        ti: usize,
        pa: Option<u32>,
        pb: Option<u32>,
        pc: Option<u32>,
    ) -> Option<Vec<Triple>> {
        let entry = self.tile_entry(si, ti)?;
        Some(
            TripleBlock::parse(entry.body())
                .ok()
                .filter(|b| b.zone().may_contain(pa, pb, pc))
                .map(|b| match pa {
                    Some(a) => {
                        let dir = self.tile_dir(si, ti, &entry, &b);
                        b.scan_from(dir, a, pb, pc).collect()
                    }
                    None => b.scan(pa, pb, pc).collect(),
                })
                .unwrap_or_default(),
        )
    }

    /// Is the object of every triple with predicate `p` inside an object-id
    /// range `covers` accepts? Read off the predicate-leading tiles (POS, or PSO):
    /// a tile whose synopsis object range `covers` accepts is proven without
    /// being read — the synopsis bounds every row of the tile, whatever its
    /// predicate, so a tile shared with other predicates only makes the test
    /// stricter. The tiles left (no synopsis, or a range it does not accept) are
    /// read and their rows of `p` checked one by one: on a lazy index only when
    /// there are at most `max_read` of them, in one batch, else the answer is
    /// `false`; on a resident one always, in memory. `false` also when a tile
    /// cannot be read. An index without `p` proves it trivially.
    pub(crate) fn predicate_objects_within(
        &self,
        p: u32,
        covers: impl Fn(u32, u32) -> bool,
        max_read: usize,
    ) -> bool {
        // (perm, which synopsis/row column holds the object)
        let (perm, object_is_b) = if self.perms.contains(IndexPermutation::Pos) {
            (IndexPermutation::Pos, true)
        } else if self.perms.contains(IndexPermutation::Pso) {
            (IndexPermutation::Pso, false)
        } else {
            return false;
        };
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, Some(p));
        let unproven: Vec<usize> = (start..end)
            .filter(|&ti| match self.sections[si][ti].syn {
                Some((min_b, max_b, min_c, max_c)) => {
                    let (lo, hi) = if object_is_b {
                        (min_b, max_b)
                    } else {
                        (min_c, max_c)
                    };
                    !covers(lo, hi)
                }
                None => true,
            })
            .collect();
        if unproven.is_empty() {
            return true;
        }
        if self.is_lazy() {
            if unproven.len() > max_read {
                return false;
            }
            let missing: Vec<usize> = unproven
                .iter()
                .copied()
                .filter(|&ti| !self.tile_resident(si, ti))
                .collect();
            self.bulk_fault(si, &missing);
        }
        for ti in unproven {
            let Some(rows) = self.tile_rows(si, ti, Some(p), None, None) else {
                return false;
            };
            let all = rows.iter().all(|&(_, b, c)| {
                let o = if object_is_b { b } else { c };
                covers(o, o)
            });
            if !all {
                return false;
            }
        }
        true
    }

    /// Fault, in one batched read across every permutation, the directory leaves
    /// [`estimate_matches`](Self::estimate_matches) reads for `patterns`: for each
    /// permutation that leads with a bound id, the leaves its run of tiles starts
    /// and ends in (a leaf wholly inside the run is counted from the root). The
    /// estimate then reads them from memory instead of one permutation at a time.
    /// A no-op on a resident directory.
    pub(crate) fn prefault_estimate_leaves(&self, patterns: &[Pattern]) {
        let mut want: [std::collections::BTreeSet<usize>; NUM_PERMS] = Default::default();
        for &pattern in patterns {
            for perm in self.perms.iter() {
                let [Some(a), _, _] = perm.order_pattern(pattern) else {
                    continue;
                };
                let si = perm.section_index();
                if self.clusters[si].is_some() {
                    continue;
                }
                if let TileList::Leafed(l) = &self.sections[si] {
                    want[si].extend(l.run_end_leaves(a));
                }
            }
        }
        let wanted: Vec<(usize, Vec<usize>)> = want
            .into_iter()
            .enumerate()
            .filter(|(_, lis)| !lis.is_empty())
            .map(|(si, lis)| (si, lis.into_iter().collect()))
            .collect();
        let across: Vec<(&dyn LeafDir, &[usize])> = wanted
            .iter()
            .filter_map(|(si, lis)| match &self.sections[*si] {
                TileList::Leafed(l) => Some((&**l as &dyn LeafDir, lis.as_slice())),
                _ => None,
            })
            .collect();
        ensure_leaves_across(&across);
    }

    /// A planner estimate of how many triples match `pattern`, read off the
    /// tile directories alone — no tile is fetched or decoded, so it costs the
    /// same on a lazy 50 GB file as on a resident one. `total` is the number
    /// of triples the index holds (an upper bound is fine).
    ///
    /// For every carried permutation that leads with a bound component, the
    /// routed tiles the synopsis admits are counted; a tile holds about
    /// `total / tiles` triples, and the routed run is charged half a tile less
    /// than its length, since a key rarely starts and ends on tile boundaries.
    /// The smallest figure wins, so `?s a :Rare` (a few tiles of OPS) beats
    /// `?s :p ?o` (every tile of `:p` in PSO). Its resolution is one tile:
    /// patterns inside one tile tie, which is all a join order needs. A
    /// pattern with no bound component estimates `total`; one the synopsis
    /// rules out estimates 0.
    pub(crate) fn estimate_matches(&self, pattern: Pattern, total: u64) -> f64 {
        let mut best = total as f64;
        for perm in self.perms.iter() {
            let [pa, pb, pc] = perm.order_pattern(pattern);
            if pa.is_none() {
                continue;
            }
            let si = perm.section_index();
            let tiles = &self.sections[si];
            if tiles.is_empty() {
                return 0.0;
            }
            let per_tile = total as f64 / tiles.len() as f64;
            // A leafed list whose leaf is not resident must still be estimated
            // from the root alone (a cost estimate must not cost a read), and
            // must give the figure the same directory gives when laid out flat
            // (format 0x05) to within the unavoidable: a predicate that fills
            // thousands of tiles is thousands of tiles whichever layout stores
            // them. Counting "one tile" here made the planner see a 2 M-row
            // predicate as 7 k rows and hash-join it (see [`LeafedTiles::admitted_estimate`]).
            // A clustered subject-leading permutation does not know which run a
            // subject is in without reading: a subject is one tile, as in an
            // unclustered file.
            let admitted = match (tiles, pa) {
                (_, Some(_)) if self.clusters[si].is_some() => 1.0,
                (TileList::Leafed(l), Some(a))
                    if !l
                        .leaves
                        .iter()
                        .enumerate()
                        .filter(|(_, lf)| lf.min_a <= a && a <= lf.max_a)
                        .all(|(li, _)| l.cells[li].get().is_some()) =>
                {
                    l.admitted_estimate(a, pb, pc)
                }
                _ => {
                    let (start, end) = self.tile_span(si, pa);
                    (start..end)
                        .filter(|&ti| tiles[ti].syn_admits(pb, pc))
                        .count() as f64
                }
            };
            let est = if admitted <= 0.0 {
                0.0
            } else {
                (admitted - 0.5).max(0.5) * per_tile
            };
            best = best.min(est);
        }
        best
    }

    /// The smallest leading id `>= a` present in permutation section `section`,
    /// or `None` past the last one. Routes by the tile directory, then reads the
    /// tile's own group directory — so it skips the ids a tile's *range* covers
    /// but its body does not hold. That matters where a range is conservative:
    /// a quad-index view's tile shared with other graphs is `0..=u32::MAX`, and
    /// stepping through it id by id would never finish. Faults the tiles it
    /// reads; a failed fault ends the search (`load_incomplete` says so).
    pub(crate) fn next_leading(&self, section: usize, a: u32) -> Option<u32> {
        if let Some(cr) = &self.clusters[section] {
            // Each run holds its own ascending ids: the least one over the runs.
            let mut best: Option<u32> = None;
            for &(lo, hi) in &cr.runs {
                if let Some(x) = self.next_leading_in(section, a, lo, hi) {
                    best = Some(best.map_or(x, |b| b.min(x)));
                }
            }
            return best;
        }
        let n = self.sections[section].len();
        self.next_leading_in(section, a, 0, n)
    }

    /// [`next_leading`](Self::next_leading) within the tiles `[lo, hi)` of one run.
    fn next_leading_in(&self, section: usize, a: u32, lo: usize, hi: usize) -> Option<u32> {
        let tiles = &self.sections[section];
        let mut ti = tiles.first_max_ge_in(a, lo, hi);
        while ti < hi {
            let from = a.max(tiles[ti].min_a);
            let entry = self.tile_entry(section, ti)?;
            if let Ok(block) = TripleBlock::parse(entry.body()) {
                let dir = self.tile_dir(section, ti, &entry, &block);
                if let Some(x) = dir.first_at_or_after(from) {
                    return Some(x);
                }
            }
            ti += 1;
        }
        None
    }

    /// The tile index span a scan must visit: every tile when the leading
    /// component is unbound, else the run of tiles whose leading-id ranges
    /// cover it. Tile ranges are ascending; consecutive tiles may SHARE a
    /// leading id when a mega-group was split across tiles (see
    /// [`build_tiles`]), so the span is a range scan, not a single hit —
    /// files without splits still yield a span of at most one tile.
    pub(crate) fn tile_span(&self, section: usize, pa: Option<u32>) -> (usize, usize) {
        let tiles = &self.sections[section];
        match pa {
            None => {
                // An unbound scan visits every tile: fault every leaf at once.
                tiles.ensure_span(0, tiles.len());
                (0, tiles.len())
            }
            Some(a) if self.clusters[section].is_some() => {
                let (i, j) = match self.resolve_run(section, a) {
                    Some(run) => self.run_span(section, run, a),
                    None => (0, 0),
                };
                tiles.ensure_span(i, j);
                (i, j)
            }
            Some(a) => {
                let i = tiles.first_max_ge(a);
                let j = tiles.first_min_gt(a).max(i);
                // The callers walk the run: fault its leaves in one batch, as
                // the tile-by-tile walk this replaced did one by one.
                tiles.ensure_span(i, j);
                (i, j)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (GraphIndex, Vec<Triple>) {
        let data = vec![
            (1, 10, 100),
            (1, 10, 101),
            (1, 11, 100),
            (2, 10, 100),
            (2, 12, 200),
            (3, 11, 300),
        ];
        let mut b = GraphIndexBuilder::new();
        for &t in &data {
            b.push(t);
        }
        (b.build(), data)
    }

    /// Brute-force reference for a pattern.
    fn reference(data: &[Triple], (s, p, o): Pattern) -> Vec<Triple> {
        let mut v: Vec<Triple> = data
            .iter()
            .copied()
            .filter(|&(a, b, c)| {
                s.is_none_or(|x| x == a) && p.is_none_or(|x| x == b) && o.is_none_or(|x| x == c)
            })
            .collect();
        v.sort_unstable();
        v
    }

    /// The tile-directory estimate orders patterns the way their true sizes
    /// do, never fetches anything, and is exact at its extremes: unbound is the
    /// total, a key no tile can hold is zero.
    #[test]
    fn tile_estimates_rank_patterns_by_size() {
        // Predicate 1 on every subject, predicate 2 on a few; object 50 is a
        // "class" with 4000 instances, object 60 one with 3.
        let mut b = GraphIndexBuilder::new().with_tile_budget(256);
        let mut n = 0u64;
        for s in 0..4000u32 {
            b.push((s, 1, 50));
            b.push((s, 3, 1000 + s));
            n += 2;
            if s % 1000 == 0 {
                b.push((s, 2, 60));
                n += 1;
            }
        }
        let idx = b.build();
        let est = |p: Pattern| idx.estimate_matches(p, n);
        assert_eq!(est((None, None, None)), n as f64);
        assert!(est((None, Some(2), None)) < est((None, Some(1), None)));
        assert!(est((None, None, Some(60))) < est((None, None, Some(50))));
        assert!(est((Some(7), None, None)) < est((None, Some(3), None)));
        // A big predicate spans many tiles; the rare one fits in at most one.
        let per_tile = n as f64 / idx.tile_sections()[0].len() as f64;
        assert!(est((None, Some(2), None)) <= 1.5 * per_tile);
        assert!(est((None, Some(1), None)) > 10.0 * per_tile);
        // A leading id beyond every tile's range is provably absent.
        assert_eq!(est((Some(99_999), None, None)), 0.0);
        // An empty index estimates nothing for a bound pattern.
        let empty = GraphIndexBuilder::new().build();
        assert_eq!(empty.estimate_matches((Some(1), None, None), 0), 0.0);
    }

    #[test]
    fn every_pattern_shape_matches_reference() {
        let (idx, data) = graph();
        let vals = |opts: &[u32]| {
            let mut v: Vec<Option<u32>> = opts.iter().map(|&x| Some(x)).collect();
            v.push(None);
            v
        };
        // Exercise all 8 bound/unbound shapes across representative values.
        for s in vals(&[1, 2, 9]) {
            for p in vals(&[10, 11, 99]) {
                for o in vals(&[100, 300, 999]) {
                    let pat = (s, p, o);
                    assert_eq!(
                        idx.match_pattern(pat),
                        reference(&data, pat),
                        "pattern {pat:?}"
                    );
                    // The lazy stream must match the eager result once sorted —
                    // same triples, just without the up-front canonical re-sort.
                    let mut streamed: Vec<Triple> = idx.scan_iter(pat).collect();
                    streamed.sort_unstable();
                    assert_eq!(streamed, reference(&data, pat), "scan_iter {pat:?}");
                }
            }
        }
    }

    #[test]
    fn unbound_returns_everything_sorted() {
        let (idx, data) = graph();
        let mut sorted = data.clone();
        sorted.sort_unstable();
        assert_eq!(idx.match_pattern((None, None, None)), sorted);
    }

    /// A tiny tile budget must split sections into many tiles — including
    /// MID-GROUP cuts (budget 1/16 makes every a-group oversized) — and every
    /// pattern shape must still match the brute-force reference: bound
    /// leading ids route to the run of covering tiles, unbound scans chain
    /// all tiles.
    #[test]
    fn multi_tile_sections_match_reference_every_shape() {
        // Enough distinct leading ids to split under a tiny budget.
        let mut data: Vec<Triple> = Vec::new();
        for s in 1..=40u32 {
            for p in [10u32, 11] {
                for o in [100u32, 100 + s] {
                    data.push((s, p, o));
                }
            }
        }
        for budget in [1usize, 16, 64, 1 << 20] {
            let mut b = GraphIndexBuilder::new().with_tile_budget(budget);
            for &t in &data {
                b.push(t);
            }
            let idx = b.build();
            let spo_tiles = idx.tile_sections()[0].len();
            if budget <= 16 {
                assert!(spo_tiles > 1, "budget {budget} should force tiling");
            }
            // Tile ranges must be ascending; a split mega-group may leave
            // consecutive tiles SHARING a leading id (never overlapping past it).
            for w in idx.tile_sections()[0].as_full().unwrap().windows(2) {
                assert!(w[0].leading_range().1 <= w[1].leading_range().0);
            }
            assert_eq!(idx.triple_count() as usize, data.len(), "budget {budget}");

            let vals = |opts: &[u32]| {
                let mut v: Vec<Option<u32>> = opts.iter().map(|&x| Some(x)).collect();
                v.push(None);
                v
            };
            for _round in 0..2 {
                for s in vals(&[1, 20, 40, 99]) {
                    for p in vals(&[10, 11, 99]) {
                        for o in vals(&[100, 120, 999]) {
                            let pat = (s, p, o);
                            assert_eq!(
                                idx.match_pattern(pat),
                                reference(&data, pat),
                                "budget {budget} pattern {pat:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The Crossref shape: ONE mega-predicate carrying most of the graph. At
    /// the real 64 KiB budget its P-leading a-group must be split across
    /// multiple tiles (bounding builder memory), and a bound leading id must
    /// route to the whole run of covering tiles with complete results.
    #[test]
    fn mega_group_splits_across_tiles_and_lookups_stay_complete() {
        let hot_p = 7u32;
        let mut data: Vec<Triple> = Vec::new();
        for i in 0..40_000u32 {
            data.push((1_000 + i % 200, hot_p, 50_000 + i));
        }
        data.push((1, 1, 1));
        data.push((2, 2, 2));
        let mut b = GraphIndexBuilder::new(); // default INDEX_TILE_BUDGET
        for &t in &data {
            b.push(t);
        }
        let idx = b.build();

        // PSO (a = predicate): the hot_p group must span several tiles.
        let pso = ALL_PERMS
            .iter()
            .position(|p| matches!(p, IndexPermutation::Pso))
            .unwrap();
        let covering = idx.tile_sections()[pso]
            .iter()
            .filter(|t| {
                let (lo, hi) = t.leading_range();
                lo <= hot_p && hot_p <= hi
            })
            .count();
        assert!(
            covering > 1,
            "expected the hot predicate split across tiles, got {covering}"
        );

        // Bound-p lookup must still return every triple of the mega-group —
        // probing bound objects in the FIRST, MIDDLE and LAST slices of the
        // split run (a mid-run object once returned 0 on the first split file).
        for pat in [
            (None, Some(hot_p), None),
            (Some(1_050), Some(hot_p), None),
            (None, Some(hot_p), Some(50_123)), // first slice
            (None, Some(hot_p), Some(70_000)), // middle of the run
            (None, Some(hot_p), Some(89_999)), // last slice
            (None, Some(hot_p), Some(49_000)), // below the range → empty
            (None, Some(hot_p), Some(95_000)), // above the range → empty
        ] {
            assert_eq!(idx.match_pattern(pat), reference(&data, pat), "{pat:?}");
        }
    }

    /// Scans must yield identical results across the directory lifecycle: the
    /// first leading-bound scan walks linearly, the second builds the
    /// directory, and later ones jump through it — including absent ids
    /// (between, below, and above the stored groups).
    #[test]
    fn directory_backed_scans_match_reference_every_shape() {
        let (idx, data) = graph();
        let vals = |opts: &[u32]| {
            let mut v: Vec<Option<u32>> = opts.iter().map(|&x| Some(x)).collect();
            v.push(None);
            v
        };
        for round in 0..3 {
            for s in vals(&[1, 2, 3, 0, 9]) {
                for p in vals(&[10, 11, 12, 99]) {
                    for o in vals(&[100, 200, 300, 999]) {
                        let pat = (s, p, o);
                        let mut got: Vec<Triple> = idx.scan_iter(pat).collect();
                        got.sort_unstable();
                        assert_eq!(got, reference(&data, pat), "round {round} scan {pat:?}");
                    }
                }
            }
        }
    }

    /// A tile synopsis must let a routed scan **skip the fetch** of a tile its
    /// secondary-column range rules out — and never skip one that could match.
    #[test]
    fn synopsis_prunes_the_routed_tile_before_fetch() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;

        // One real SPO tile: subject a=5, predicates b∈{10,11}, object c=100.
        let block = {
            let mut b = TripleBlockBuilder::new();
            b.push((5, 10, 100));
            b.push((5, 11, 100));
            b.build()
        };
        let fetches = Arc::new(AtomicUsize::new(0));
        let (blk, fc) = (block.clone(), fetches.clone());
        // The loader returns the (already-decompressed) tile image and counts calls.
        let loader: TileLoader = Box::new(move |_si, _ti| {
            fc.fetch_add(1, SeqCst);
            Some(blk.clone())
        });
        // Remote SPO directory: one tile, leading a∈[5,5], synopsis b∈[10,11], c∈[100,100].
        let dirs = [
            vec![(5u32, 5u32, Some((10u32, 11u32, 100u32, 100u32)))],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ];
        let idx = GraphIndex::from_remote_directories(dirs, PermSet::ALL, loader);

        // Predicate 99 is outside the tile's b-range → prune, zero fetches, empty.
        assert!(idx.match_pattern((Some(5), Some(99), None)).is_empty());
        assert_eq!(fetches.load(SeqCst), 0, "synopsis must skip the fetch");

        // Object 999 outside the tile's c-range → prune, still zero fetches.
        assert!(idx.match_pattern((Some(5), None, Some(999))).is_empty());
        assert_eq!(
            fetches.load(SeqCst),
            0,
            "secondary-c prune also skips the fetch"
        );

        // Predicate 10 is in range → the tile is fetched and the match returned.
        assert_eq!(
            idx.match_pattern((Some(5), Some(10), None)),
            vec![(5, 10, 100)]
        );
        assert_eq!(
            fetches.load(SeqCst),
            1,
            "an admissible secondary still fetches"
        );
    }

    /// Without a synopsis (`None`), nothing is pruned early — the tile is always
    /// fetched and the in-tile zone map does the (correct) pruning, as before.
    #[test]
    fn absent_synopsis_never_prunes() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;
        let block = {
            let mut b = TripleBlockBuilder::new();
            b.push((5, 10, 100));
            b.build()
        };
        let fetches = Arc::new(AtomicUsize::new(0));
        let (blk, fc) = (block.clone(), fetches.clone());
        let loader: TileLoader = Box::new(move |_si, _ti| {
            fc.fetch_add(1, SeqCst);
            Some(blk.clone())
        });
        let dirs = [
            vec![(5u32, 5u32, None)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ];
        let idx = GraphIndex::from_remote_directories(dirs, PermSet::ALL, loader);
        // Predicate 99 absent, but with no synopsis the tile is fetched (then the
        // zone map yields no match) — correctness preserved, just no fetch saved.
        assert!(idx.match_pattern((Some(5), Some(99), None)).is_empty());
        assert_eq!(fetches.load(SeqCst), 1, "no synopsis ⇒ no early prune");
    }

    /// The eight triple-pattern shapes, as `(s, p, o)` boundness.
    fn all_patterns() -> Vec<Pattern> {
        (0..8u8)
            .map(|mask| {
                (
                    (mask & 1 != 0).then_some(1u32),
                    (mask & 2 != 0).then_some(2u32),
                    (mask & 4 != 0).then_some(3u32),
                )
            })
            .collect()
    }

    /// The claim the whole option rests on: **{SPO, POS, OSP} ties the longest
    /// bound prefix on all eight pattern shapes**, so the other three never win
    /// routing — only sort order. Enumerated and printed rather than asserted in
    /// prose, because if it were ever false a lean file would answer a pattern
    /// from a worse index without anything failing.
    #[test]
    fn perm_routing_never_leaves_core() {
        for pat in all_patterns() {
            let best_all = ALL_PERMS
                .iter()
                .map(|p| p.leading_bound(pat))
                .max()
                .unwrap();
            let best_core = PermSet::CORE
                .iter()
                .map(|p| p.leading_bound(pat))
                .max()
                .unwrap();
            let scores: Vec<String> = ALL_PERMS
                .iter()
                .map(|p| format!("{}={}", p.name(), p.leading_bound(pat)))
                .collect();
            eprintln!(
                "s={} p={} o={} | {} | best6={best_all} best3={best_core} chosen={}",
                pat.0.is_some() as u8,
                pat.1.is_some() as u8,
                pat.2.is_some() as u8,
                scores.join(" "),
                GraphIndex::best_permutation(pat).name(),
            );
            assert_eq!(
                best_core, best_all,
                "the three routing permutations must tie the best of six on {pat:?}"
            );
            let chosen = GraphIndex::best_permutation(pat);
            assert!(
                PermSet::CORE.contains(chosen),
                "best_permutation chose {} for {pat:?}, outside the routing three",
                chosen.name()
            );
            assert_eq!(
                chosen,
                GraphIndex::best_permutation_in(PermSet::CORE, pat),
                "routing must be identical with three permutations and with six"
            );
        }
    }

    /// Restricting the set may never make a merge join pick a *worse-routed*
    /// stream than the plain lookup would use: with six the co-sorted
    /// permutation always ties the best prefix, and with three the guard
    /// declines rather than buying sort order with a whole-section scan.
    #[test]
    fn sorted_permutation_never_sacrifices_routing() {
        let mut declined = Vec::new();
        for pat in all_patterns() {
            for col in 0..3usize {
                let routed_all = GraphIndex::best_permutation(pat).leading_bound(pat);
                if let Some(p) = GraphIndex::permutation_sorted_on(pat, col) {
                    assert_eq!(
                        p.leading_bound(pat),
                        routed_all,
                        "six permutations must always co-sort at the best prefix"
                    );
                    assert_eq!(p.roles()[p.leading_bound(pat)], col);
                }
                match GraphIndex::permutation_sorted_on_in(PermSet::CORE, pat, col) {
                    Some(p) => {
                        assert!(PermSet::CORE.contains(p));
                        assert_eq!(
                            p.leading_bound(pat),
                            GraphIndex::best_permutation_in(PermSet::CORE, pat).leading_bound(pat)
                        );
                    }
                    None => {
                        if GraphIndex::permutation_sorted_on(pat, col).is_some() {
                            declined.push((pat, col));
                        }
                    }
                }
            }
        }
        // Exactly the three shapes SOP / PSO / OPS exist for: one bound
        // component, sorted on a column no remaining order can lead with after
        // it — subject-bound sorted on object, predicate-bound sorted on
        // subject, object-bound sorted on predicate.
        for (pat, col) in &declined {
            let bound: Vec<usize> = (0..3)
                .filter(|&i| [pat.0, pat.1, pat.2][i].is_some())
                .collect();
            assert_eq!(
                bound.len(),
                1,
                "only single-bound shapes lose a merge order"
            );
            assert_ne!(bound[0], *col);
        }
        eprintln!("three-permutation merge-join declines: {declined:?}");
        assert_eq!(declined.len(), 3, "exactly three (bound, sort) shapes lost");
    }

    #[test]
    fn perm_set_masks_and_positions() {
        assert_eq!(PermSet::ALL.len(), 6);
        assert_eq!(PermSet::CORE.len(), 3);
        assert_eq!(PermSet::CORE.bits(), 0b0000_0111);
        assert_eq!(PermSet::CORE.names(), vec!["SPO", "POS", "OSP"]);
        assert_eq!(
            PermSet::ALL.names(),
            vec!["SPO", "POS", "OSP", "SOP", "PSO", "OPS"]
        );
        // Container position is rank among the STORED permutations, which is
        // why the routing three had to lead ALL_PERMS.
        for (i, p) in PermSet::CORE.iter().enumerate() {
            assert_eq!(PermSet::CORE.position(p), Some(i));
            assert_eq!(p.section_index(), i);
        }
        assert_eq!(PermSet::CORE.position(IndexPermutation::Sop), None);
        assert_eq!(PermSet::ALL.position(IndexPermutation::Ops), Some(5));
        // Only supersets of the routing three are legal.
        assert!(PermSet::from_bits(0b0000_0111).is_ok());
        assert!(PermSet::from_bits(0b0011_1111).is_ok());
        assert!(PermSet::from_bits(0b0000_1111).is_ok());
        assert!(PermSet::from_bits(0b0000_0011).is_err());
        assert!(PermSet::from_bits(0b0011_1000).is_err());
        assert!(PermSet::from_bits(0b1000_0111).is_err());
    }

    /// A lean index answers every pattern shape with exactly the rows a full
    /// one does — the property that makes the option safe at all.
    #[test]
    fn three_and_six_agree_on_every_pattern() {
        let triples: Vec<Triple> = (0..200u32)
            .map(|i| (i % 17 + 1, i % 5 + 1, i % 23 + 1))
            .collect();
        let six = GraphIndexBuilder::from_triples(triples.clone())
            .with_tile_budget(128)
            .build();
        let three = GraphIndexBuilder::from_triples(triples)
            .with_tile_budget(128)
            .with_perms(PermSet::CORE)
            .build();
        assert_eq!(six.perms(), PermSet::ALL);
        assert_eq!(three.perms(), PermSet::CORE);
        for perm in [
            IndexPermutation::Sop,
            IndexPermutation::Pso,
            IndexPermutation::Ops,
        ] {
            assert!(three.tile_sections()[perm.section_index()].is_empty());
        }
        for a in [None, Some(3u32)] {
            for b in [None, Some(2u32)] {
                for c in [None, Some(9u32)] {
                    let mut x = six.match_pattern((a, b, c));
                    let mut y = three.match_pattern((a, b, c));
                    x.sort_unstable();
                    y.sort_unstable();
                    assert_eq!(x, y, "pattern {:?} disagreed", (a, b, c));
                }
            }
        }
    }
}
