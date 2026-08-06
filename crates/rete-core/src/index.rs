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

use std::sync::OnceLock;

use crate::triples::{encode_sorted_unique, GroupDirectory, Triple, TripleBlock};
use crate::varint::uvarint_len;

#[cfg(test)]
use crate::triples::TripleBlockBuilder;
#[cfg(feature = "unsafe-decode-bench")]
use crate::triples::{BlockCursor, UncheckedBlockCursor};

/// A triple pattern: `None` is an unbound variable, `Some(id)` a bound term.
pub type Pattern = (Option<u32>, Option<u32>, Option<u32>);

/// Default per-tile budget in (uncompressed) encoded bytes. Tiles are the
/// independently-compressed, independently-fetchable units of a permutation
/// section: ~64 KiB matches both zstd's sweet spot and one HTTP range read.
pub const INDEX_TILE_BUDGET: usize = 64 * 1024;

/// Geometric tile-prefetch ramp for an unbound (multi-tile) scan (see
/// [`GraphIndex::scan_iter`]). The first coalesced batch faults this many
/// tiles; each subsequent batch doubles up to [`PREFETCH_WINDOW_MAX`]. Small
/// enough that `LIMIT 1` fetches only a few tiles, large enough that a full
/// scan still coalesces into a handful of range reads.
const PREFETCH_WINDOW_START: usize = 4;
const PREFETCH_WINDOW_MAX: usize = 512;

#[cfg(feature = "unsafe-decode-bench")]
enum DecodeCursor<'a> {
    Safe(BlockCursor<'a>),
    Unchecked(UncheckedBlockCursor<'a>),
}

#[cfg(feature = "unsafe-decode-bench")]
impl Iterator for DecodeCursor<'_> {
    type Item = Triple;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Safe(cursor) => cursor.next(),
            Self::Unchecked(cursor) => cursor.next(),
        }
    }
}

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
    data: OnceLock<Vec<u8>>,
    dir: OnceLock<GroupDirectory>,
}

impl Tile {
    fn local(min_a: u32, max_a: u32, bytes: Vec<u8>) -> Self {
        let len = bytes.len().min(u32::MAX as usize) as u32;
        let data = OnceLock::new();
        let _ = data.set(bytes);
        Tile {
            min_a,
            max_a,
            syn: None,
            len,
            data,
            dir: OnceLock::new(),
        }
    }

    fn remote(min_a: u32, max_a: u32, syn: Option<(u32, u32, u32, u32)>) -> Self {
        Tile {
            min_a,
            max_a,
            syn,
            len: 0,
            data: OnceLock::new(),
            dir: OnceLock::new(),
        }
    }

    /// Encoded byte length (see the field doc); falls back to the loaded image
    /// size when the directory didn't provide one.
    pub(crate) fn encoded_len(&self) -> u64 {
        if self.len > 0 {
            self.len as u64
        } else {
            self.data.get().map_or(0, |d| d.len() as u64)
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
    pub(crate) fn syn_admits(&self, pb: Option<u32>, pc: Option<u32>) -> bool {
        match self.syn {
            None => true,
            Some((min_b, max_b, min_c, max_c)) => {
                let ok = |v: Option<u32>, lo: u32, hi: u32| v.is_none_or(|x| lo <= x && x <= hi);
                ok(pb, min_b, max_b) && ok(pc, min_c, max_c)
            }
        }
    }

    /// The tile's serialized (uncompressed) [`TripleBlock`] image, if present
    /// locally (always, for built/opened indexes; empty for an unfaulted
    /// remote tile — the writer never sees those).
    pub fn bytes(&self) -> &[u8] {
        self.data.get().map(Vec::as_slice).unwrap_or(&[])
    }
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

/// The number of stored permutations.
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
    fn back(self, abc: Triple) -> Triple {
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
}

impl Default for GraphIndexBuilder {
    fn default() -> Self {
        Self {
            triples: Vec::new(),
            tile_budget: INDEX_TILE_BUDGET,
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
            tile_budget: INDEX_TILE_BUDGET,
        }
    }

    /// Override the per-tile byte budget (tests use tiny budgets to force
    /// multi-tile sections on small data).
    pub fn with_tile_budget(mut self, bytes: usize) -> Self {
        self.tile_budget = bytes.max(1);
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
            build_tiles(permuted, budget)
        };
        #[cfg(feature = "parallel")]
        let sections: [Vec<Tile>; NUM_PERMS] = {
            use rayon::prelude::*;
            let mut built: Vec<Vec<Tile>> = Vec::with_capacity(NUM_PERMS);
            for chunk in ALL_PERMS.chunks(2) {
                built.extend(
                    chunk
                        .to_vec()
                        .into_par_iter()
                        .map(build_one)
                        .collect::<Vec<_>>(),
                );
            }
            built.try_into().ok().expect("six permutations")
        };
        #[cfg(not(feature = "parallel"))]
        let sections: [Vec<Tile>; NUM_PERMS] = ALL_PERMS.map(build_one);
        GraphIndex::from_sections(sections)
    }

    pub fn build(self) -> GraphIndex {
        let perms = ALL_PERMS;
        let triples = &self.triples;
        let budget = self.tile_budget;
        // The six permutations are independent — permute + sort + tile each.
        let build_one = move |perm: IndexPermutation| -> Vec<Tile> {
            let permuted: Vec<Triple> = triples.iter().map(|&t| perm.forward(t)).collect();
            build_tiles(permuted, budget)
        };
        // Build the permutations concurrently when the `parallel` feature is on
        // (they share no state); the per-permutation sort inside is also
        // parallel. Output is byte-identical to the serial path.
        #[cfg(feature = "parallel")]
        let sections: [Vec<Tile>; NUM_PERMS] = {
            use rayon::iter::{IntoParallelIterator, ParallelIterator};
            let built: Vec<Vec<Tile>> = perms.into_par_iter().map(build_one).collect();
            // `.ok()` drops the (non-Debug) Vec error so `expect` compiles.
            built.try_into().ok().expect("six permutations")
        };
        #[cfg(not(feature = "parallel"))]
        let sections: [Vec<Tile>; NUM_PERMS] = perms.map(build_one);
        GraphIndex::from_sections(sections)
    }
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
            size: uvarint_len((a - prev_a) as u64),
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
                self.size += uvarint_len(b as u64); // first b-run: delta from 0
            } else {
                self.size += uvarint_len(self.num_c); // close the previous b-run
                self.size += uvarint_len((b - self.cur_b) as u64);
            }
            self.cur_b = b;
            self.num_c = 0;
            self.prev_c = 0;
            self.num_b += 1;
            self.empty = false;
        }
        self.size += uvarint_len((c - self.prev_c) as u64);
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
                uvarint_len(self.num_c)
            }
            + uvarint_len(self.num_b)
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
        Tile::local(run[0].0, run[run.len() - 1].0, encode_sorted_unique(run))
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
    pub(crate) sections: [Vec<Tile>; NUM_PERMS],
    /// Faults in remote tiles on first scan (`None` for local indexes).
    loader: Option<TileLoader>,
    /// Optional batched fetch for multi-tile scans (`None` falls back to
    /// one [`TileLoader`] call per tile).
    bulk: Option<TileBulkLoader>,
    /// Set when the loader failed for some tile: results may be incomplete and
    /// the caller must surface an error rather than the partial answer.
    load_failed: std::sync::atomic::AtomicBool,
    /// The reader's concurrent-range fan-out (1 = strictly sequential) — see
    /// [`set_read_concurrency`](Self::set_read_concurrency).
    read_concurrency: usize,
    /// Research-only selection of the unchecked cursor. Default artifacts do
    /// not contain this field or its decoder.
    #[cfg(feature = "unsafe-decode-bench")]
    unchecked_decode: bool,
}

impl GraphIndex {
    fn from_sections(sections: [Vec<Tile>; NUM_PERMS]) -> Self {
        GraphIndex {
            sections,
            loader: None,
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            #[cfg(feature = "unsafe-decode-bench")]
            unchecked_decode: false,
        }
    }

    /// Rebuild from tiled sections: per permutation, `(min_a, max_a, block
    /// bytes)` per tile in ascending leading-id order — the v0.2 layout.
    pub fn from_tiles(sections: [Vec<(u32, u32, Vec<u8>)>; NUM_PERMS]) -> Self {
        let sections = sections.map(|tiles| {
            tiles
                .into_iter()
                .map(|(min_a, max_a, bytes)| Tile::local(min_a, max_a, bytes))
                .collect()
        });
        Self::from_sections(sections)
    }

    /// A **remote** index: only the tile directories (leading-id ranges per
    /// permutation, ascending) are known; tile payloads fault in through
    /// `loader` on first scan. Check [`load_incomplete`](Self::load_incomplete)
    /// after evaluating — a failed fetch must become an error, never a
    /// silently smaller result.
    #[allow(clippy::type_complexity)]
    pub fn from_remote_directories(
        directories: [Vec<(u32, u32, Option<(u32, u32, u32, u32)>)>; NUM_PERMS],
        loader: TileLoader,
    ) -> Self {
        let sections = directories.map(|dir| {
            dir.into_iter()
                .map(|(min_a, max_a, syn)| Tile::remote(min_a, max_a, syn))
                .collect()
        });
        GraphIndex {
            sections,
            loader: Some(loader),
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            #[cfg(feature = "unsafe-decode-bench")]
            unchecked_decode: false,
        }
    }

    /// Permanently select unchecked decoding for this index instance.
    ///
    /// # Safety
    ///
    /// Every local block and every block subsequently returned by the remote
    /// loaders must be a complete immutable image produced by rete's encoder.
    /// A malformed, truncated, or concurrently mutated block can cause an
    /// out-of-bounds read. This research mode must never be enabled for an
    /// untrusted file.
    #[cfg(feature = "unsafe-decode-bench")]
    pub unsafe fn assume_valid_blocks(&mut self) {
        self.unchecked_decode = true;
    }

    /// Attach a batched tile fetcher (see [`TileBulkLoader`]): multi-tile
    /// scans prefetch their span through it instead of faulting tile by tile.
    pub fn with_bulk_loader(mut self, bulk: TileBulkLoader) -> Self {
        self.bulk = Some(bulk);
        self
    }

    /// Record each remote tile's encoded (on-disk) byte length, per section —
    /// known to the ranged opener from the tile directory. Powers the join
    /// planner's fatness gates; never triggers a fetch.
    pub(crate) fn set_tile_lens(&mut self, lens: [Vec<u32>; NUM_PERMS]) {
        for (section, ls) in self.sections.iter_mut().zip(lens) {
            for (tile, l) in section.iter_mut().zip(ls) {
                tile.len = l;
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

    /// The reader's concurrent-range fan-out (1 = strictly sequential).
    pub(crate) fn read_concurrency(&self) -> usize {
        self.read_concurrency
    }

    /// Did any tile fetch fail since this index was opened — or since the last
    /// [`reset_load_failure`](Self::reset_load_failure)?
    pub fn load_incomplete(&self) -> bool {
        self.load_failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Forget recorded fetch failures — the start-of-evaluation reset for a
    /// RESIDENT session, making the incompleteness verdict per-query instead of
    /// per-open (one transient network blip used to fail every later query on
    /// the session). Safe because failed tiles are never cached: the next scan
    /// simply retries them.
    pub fn reset_load_failure(&self) {
        self.load_failed
            .store(false, std::sync::atomic::Ordering::Relaxed);
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
    fn prefetch_span(&self, section: usize, start: usize, end: usize) {
        let missing: Vec<usize> = (start..end)
            .filter(|&ti| self.sections[section][ti].data.get().is_none())
            .collect();
        self.bulk_fault(section, &missing);
    }

    /// Bulk-fault a set of (possibly scattered, ascending) missing tile indices in
    /// `section` through the bulk loader in one coalesced read, storing each image.
    /// No-op without a bulk loader or for fewer than two tiles (a single fault
    /// costs the same via the per-tile loader). A failed batch leaves the tiles
    /// unloaded for the per-tile loader to retry. Shared by the consecutive-span
    /// scan prefetch and the scattered batch-probe prefetch.
    fn bulk_fault(&self, section: usize, tiles: &[usize]) {
        if tiles.len() < 2 {
            return;
        }
        let Some(bulk) = &self.bulk else { return };
        if let Some(images) = bulk(section, tiles) {
            if images.len() == tiles.len() {
                for (&ti, img) in tiles.iter().zip(images) {
                    let _ = self.sections[section][ti].data.set(img);
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
    pub(crate) fn prefetch_probe_tiles(&self, patterns: &[Pattern]) {
        if self.bulk.is_none() {
            return;
        }
        let mut want: [std::collections::BTreeSet<usize>; NUM_PERMS] = Default::default();
        for &pat in patterns {
            let perm = Self::best_permutation(pat);
            let [pa, pb, pc] = perm.order_pattern(pat);
            let si = perm.section_index();
            let (start, end) = self.tile_span(si, pa);
            for ti in start..end {
                if self.sections[si][ti].syn_admits(pb, pc)
                    && self.sections[si][ti].data.get().is_none()
                {
                    want[si].insert(ti);
                }
            }
        }
        for (si, set) in want.iter().enumerate() {
            let tiles: Vec<usize> = set.iter().copied().collect();
            self.bulk_fault(si, &tiles);
        }
    }

    /// The tile's block image, faulting it in through the loader if remote.
    /// A FAILED fetch records the failure and returns an empty slice WITHOUT
    /// caching it, so a later evaluation retries the tile — a transient
    /// network error must not permanently poison a long-lived (resident)
    /// session with an empty tile masquerading as data.
    fn tile_data(&self, section: usize, tile: usize) -> &[u8] {
        let cell = &self.sections[section][tile].data;
        if let Some(d) = cell.get() {
            return d;
        }
        match &self.loader {
            Some(load) => match load(section, tile) {
                Some(bytes) => cell.get_or_init(|| bytes),
                None => {
                    self.load_failed
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    &[]
                }
            },
            // A local tile with no data was constructed empty on purpose.
            None => cell.get_or_init(Vec::new),
        }
    }

    /// Total triple count (sum of the SPO tiles' zone counts). For a remote
    /// index this faults in the SPO tiles — prefer the header's quad count.
    pub fn triple_count(&self) -> u32 {
        self.prefetch_span(0, 0, self.sections[0].len());
        (0..self.sections[0].len())
            .filter_map(|ti| TripleBlock::parse(self.tile_data(0, ti)).ok())
            .map(|b| b.zone().count)
            .sum()
    }

    /// The tiles of each permutation section (in `ALL_PERMS` order), for the
    /// file writer.
    pub fn tile_sections(&self) -> [&[Tile]; NUM_PERMS] {
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
    pub fn best_permutation(pattern: Pattern) -> IndexPermutation {
        let mut best = IndexPermutation::Spo;
        let mut best_score = best.leading_bound(pattern);
        for perm in ALL_PERMS {
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
    pub fn permutation_sorted_on(pattern: Pattern, sort_col: usize) -> Option<IndexPermutation> {
        let bound = [
            pattern.0.is_some(),
            pattern.1.is_some(),
            pattern.2.is_some(),
        ];
        if bound[sort_col] {
            return None;
        }
        let mut best: Option<(IndexPermutation, usize)> = None;
        for perm in ALL_PERMS {
            let roles = perm.roles();
            let lead = perm.leading_bound(pattern);
            // The component at slot `lead` is the first free one; it must be the
            // sort column (so the stream is sorted on it after the bound prefix).
            if lead < 3 && roles[lead] == sort_col && best.map(|(_, s)| lead > s).unwrap_or(true) {
                best = Some((perm, lead));
            }
        }
        best.map(|(p, _)| p)
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
        self.scan_iter_with(pattern, Self::best_permutation(pattern))
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
        Some(self.scan_iter_with(pattern, Self::permutation_sorted_on(pattern, sort_col)?))
    }

    /// Stream `pattern`'s matches using a **specific** permutation `perm`; the
    /// stream is sorted in `perm`'s `(a, b, c)` order. Shared core of `scan_iter`
    /// and `scan_iter_sorted_on`.
    fn scan_iter_with(
        &self,
        pattern: Pattern,
        perm: IndexPermutation,
    ) -> impl Iterator<Item = Triple> + '_ {
        // Route: a bound leading id binary-searches the tile directory to exactly
        // one tile (groups are never split across tiles); an unbound one chains
        // every tile's cursor. Within a tile, a bound leading scan jumps to its
        // a-group via the tile's lazily-built group directory — built on first
        // use, costing one walk of that (budget-sized) tile.
        let [pa, pb, pc] = perm.order_pattern(pattern);
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, pa);
        // Coalesce tile faults, but ramp the prefetch window geometrically as
        // the scan advances rather than fetching the whole span up front: a
        // small `LIMIT` (which stops pulling early) then never faults tiles
        // past the rows it needs, while a full scan still batches into a
        // handful of coalesced reads (4, 8, 16, … tiles). A bound leading scan
        // spans a single tile, so the prefetch no-ops and it faults just that
        // one tile, unchanged.
        let window = std::cell::Cell::new(PREFETCH_WINDOW_START);
        (start..end)
            // Synopsis pre-fault prune: drop a routed tile the directory proves
            // can't match a bound secondary component, **without fetching it**
            // (the remote win — a negative/sparse lookup costs zero tile reads).
            // A bound leading id routes to a single tile, so this is where it
            // bites; a fully-unbound leading scan leaves `pb`/`pc` unbound, so
            // `syn_admits` keeps every tile, unchanged.
            .filter(move |&ti| self.sections[si][ti].syn_admits(pb, pc))
            .flat_map(move |ti| {
                // Fault in (if remote), parse (untrusted bytes ⇒ `None` on
                // malformed), and zone-prune per tile, then stream the
                // matching groups.
                if self.sections[si][ti].data.get().is_none() {
                    let w = window.get();
                    self.prefetch_span(si, ti, (ti + w).min(end));
                    window.set(w.saturating_mul(2).min(PREFETCH_WINDOW_MAX));
                }
                let tile = &self.sections[si][ti];
                TripleBlock::parse(self.tile_data(si, ti))
                    .ok()
                    .filter(|b| b.zone().may_contain(pa, pb, pc))
                    .map(|b| {
                        #[cfg(feature = "unsafe-decode-bench")]
                        {
                            if self.unchecked_decode {
                                return match pa {
                                    Some(a) => {
                                        // SAFETY: enabling this mode requires every
                                        // loader result to be a complete immutable
                                        // builder-produced block. The directory and
                                        // cursor borrow this exact tile allocation.
                                        let dir = tile.dir.get_or_init(|| unsafe {
                                            b.group_directory_unchecked()
                                        });
                                        // SAFETY: the mode's contract and directory
                                        // construction above satisfy the cursor's
                                        // validity, lifetime, and provenance rules.
                                        DecodeCursor::Unchecked(unsafe {
                                            b.scan_from_unchecked(dir, a, pb, pc)
                                        })
                                    }
                                    // SAFETY: the mode's contract guarantees this is
                                    // a complete immutable builder-produced block.
                                    None => DecodeCursor::Unchecked(unsafe {
                                        b.scan_unchecked(pa, pb, pc)
                                    }),
                                };
                            }
                            DecodeCursor::Safe(match pa {
                                Some(a) => {
                                    let dir = tile.dir.get_or_init(|| b.group_directory());
                                    b.scan_from(dir, a, pb, pc)
                                }
                                None => b.scan(pa, pb, pc),
                            })
                        }
                        #[cfg(not(feature = "unsafe-decode-bench"))]
                        match pa {
                            Some(a) => {
                                let dir = tile.dir.get_or_init(|| b.group_directory());
                                b.scan_from(dir, a, pb, pc)
                            }
                            None => b.scan(pa, pb, pc),
                        }
                    })
                    .into_iter()
                    .flatten()
            })
            .map(move |abc| perm.back(abc))
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
            None => (0, tiles.len()),
            Some(a) => {
                let i = tiles.partition_point(|t| t.max_a < a);
                let mut j = i;
                while j < tiles.len() && tiles[j].min_a <= a {
                    j += 1;
                }
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

    #[cfg(feature = "unsafe-decode-bench")]
    #[test]
    fn unchecked_index_matches_safe_every_pattern() {
        let data = graph().1;
        let build = |budget| {
            let mut builder = GraphIndexBuilder::new().with_tile_budget(budget);
            for &triple in &data {
                builder.push(triple);
            }
            builder.build()
        };
        let values = [None, Some(1), Some(2), Some(3), Some(99)];

        for budget in [1usize, INDEX_TILE_BUDGET] {
            let safe = build(budget);
            let mut unchecked = build(budget);
            // SAFETY: both indexes were built in-process from the same valid
            // triples and their immutable block images have not been modified.
            unsafe { unchecked.assume_valid_blocks() };
            for s in values {
                for p in values {
                    for o in values {
                        let pattern = (s, p, o);
                        assert_eq!(
                            unchecked.match_pattern(pattern),
                            safe.match_pattern(pattern),
                            "budget {budget}, pattern {pattern:?}"
                        );
                    }
                }
            }
        }
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
            for w in idx.tile_sections()[0].windows(2) {
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
        let idx = GraphIndex::from_remote_directories(dirs, loader);

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
        let idx = GraphIndex::from_remote_directories(dirs, loader);
        // Predicate 99 absent, but with no synopsis the tile is fetched (then the
        // zone map yields no match) — correctness preserved, just no fetch saved.
        assert!(idx.match_pattern((Some(5), Some(99), None)).is_empty());
        assert_eq!(fetches.load(SeqCst), 1, "no synopsis ⇒ no early prune");
    }
}
