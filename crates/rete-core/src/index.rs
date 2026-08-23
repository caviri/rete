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

use std::cell::Cell;
use std::sync::{Arc, OnceLock};

use crate::adaptive::{AdaptiveReadController, ReadIntent};
use crate::build_pipeline::family::{
    build_family_from_slice, FamilyIndex, FamilyView, IndexFamily,
};
use crate::build_pipeline::BuildPipelineError;
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

struct ScanFeedback {
    controller: Option<Arc<AdaptiveReadController>>,
    intent: ReadIntent,
    consumed: Cell<usize>,
    offered: Cell<usize>,
}

impl ScanFeedback {
    fn new(controller: Option<Arc<AdaptiveReadController>>, intent: ReadIntent) -> Self {
        Self {
            controller,
            intent,
            consumed: Cell::new(0),
            offered: Cell::new(0),
        }
    }

    fn consume_tile(&self) {
        self.consumed.set(self.consumed.get().saturating_add(1));
    }

    fn offer_tiles(&self, count: usize) {
        self.offered.set(self.offered.get().saturating_add(count));
    }
}

impl Drop for ScanFeedback {
    fn drop(&mut self) {
        if let Some(controller) = &self.controller {
            controller.report_consumption(self.intent, self.consumed.get(), self.offered.get());
        }
    }
}

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
pub type TileBulkLoader =
    Box<dyn Fn(usize, &[usize], ReadIntent) -> Option<Vec<Vec<u8>>> + Send + Sync>;

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
    pub(crate) fn local(min_a: u32, max_a: u32, bytes: Vec<u8>) -> Self {
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

    /// The physical paired-family slot and sibling-order selector for this
    /// logical permutation. The stable six-section order remains unchanged.
    #[allow(dead_code)] // Staged paired-family metadata; exercised by crate tests.
    pub(crate) const fn family_slot(self) -> (usize, bool) {
        match self {
            Self::Spo => (0, false),
            Self::Pos => (1, false),
            Self::Osp => (2, false),
            Self::Sop => (0, true),
            Self::Pso => (1, true),
            Self::Ops => (2, true),
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
    perms: PermSet,
}

impl Default for GraphIndexBuilder {
    fn default() -> Self {
        Self {
            triples: Vec::new(),
            tile_budget: INDEX_TILE_BUDGET,
            perms: PermSet::ALL,
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
            perms: PermSet::ALL,
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

    pub fn push(&mut self, t: Triple) {
        self.triples.push(t);
    }

    /// Build the six permutations **one at a time** (each permutation's internal
    /// sort still uses every core), so only a single permuted copy of the triples
    /// is resident at once instead of all six concurrently. Trades the cross-
    /// permutation parallelism of [`Self::build`] for a much lower peak RAM — the
    /// large-graph low-memory path. Output is byte-identical to [`Self::build`].
    pub fn build_seq(self) -> GraphIndex {
        if self.perms == PermSet::ALL {
            if let Ok(index) = self.build_families_seq() {
                return index;
            }
        }
        let triples = &self.triples;
        let budget = self.tile_budget;
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
            build_tiles(permuted, budget)
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
        if self.perms == PermSet::ALL {
            if let Ok(index) = self.build_families() {
                return index;
            }
        }
        let wanted: Vec<IndexPermutation> = self.perms.iter().collect();
        let triples = &self.triples;
        let budget = self.tile_budget;
        // The permutations are independent — permute + sort + tile each.
        let build_one = move |perm: IndexPermutation| -> Vec<Tile> {
            let permuted: Vec<Triple> = triples.iter().map(|&t| perm.forward(t)).collect();
            build_tiles(permuted, budget)
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

    /// Build the three paired physical families used by format generation 0x06.
    pub(crate) fn build_families(&self) -> Result<GraphIndex, BuildPipelineError> {
        let triples = &self.triples;
        #[cfg(feature = "parallel")]
        let families = {
            let (subject, predicate_object) = rayon::join(
                || build_family_from_slice(triples, IndexFamily::Subject, self.tile_budget),
                || {
                    rayon::join(
                        || {
                            build_family_from_slice(
                                triples,
                                IndexFamily::Predicate,
                                self.tile_budget,
                            )
                        },
                        || build_family_from_slice(triples, IndexFamily::Object, self.tile_budget),
                    )
                },
            );
            [subject?, predicate_object.0?, predicate_object.1?]
        };
        #[cfg(not(feature = "parallel"))]
        let families = [
            build_family_from_slice(triples, IndexFamily::Subject, self.tile_budget)?,
            build_family_from_slice(triples, IndexFamily::Predicate, self.tile_budget)?,
            build_family_from_slice(triples, IndexFamily::Object, self.tile_budget)?,
        ];
        Ok(GraphIndex::from_families(families))
    }

    fn build_families_seq(&self) -> Result<GraphIndex, BuildPipelineError> {
        let families = [
            build_family_from_slice(&self.triples, IndexFamily::Subject, self.tile_budget)?,
            build_family_from_slice(&self.triples, IndexFamily::Predicate, self.tile_budget)?,
            build_family_from_slice(&self.triples, IndexFamily::Object, self.tile_budget)?,
        ];
        Ok(GraphIndex::from_families(families))
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
    build_tiles_sorted(&triples, budget)
}

fn build_tiles_sorted(triples: &[Triple], budget: usize) -> Vec<Tile> {
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

#[cfg(test)]
#[allow(dead_code)] // The ignored preflight harness prints the aggregate Debug view.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ReferencePermutationProfile {
    pub normalization: std::time::Duration,
    pub sort: std::time::Duration,
    pub encode: std::time::Duration,
    pub input_records: usize,
    pub output_tiles: usize,
}

/// Test-only phase split of the unchanged six-order reference construction.
/// Kept outside the production builder so it cannot affect its timing or API.
#[cfg(test)]
pub(crate) fn build_reference_profile(
    triples: &[Triple],
    budget: usize,
) -> (GraphIndex, [ReferencePermutationProfile; NUM_PERMS]) {
    let mut sections = Vec::with_capacity(NUM_PERMS);
    let mut profiles = Vec::with_capacity(NUM_PERMS);
    for permutation in ALL_PERMS {
        let normalized_started = std::time::Instant::now();
        let mut permuted: Vec<Triple> = triples
            .iter()
            .map(|&triple| permutation.forward(triple))
            .collect();
        let normalization = normalized_started.elapsed();
        let sort_started = std::time::Instant::now();
        #[cfg(feature = "parallel")]
        {
            use rayon::slice::ParallelSliceMut;
            permuted.par_sort_unstable();
        }
        #[cfg(not(feature = "parallel"))]
        permuted.sort_unstable();
        permuted.dedup();
        let sort = sort_started.elapsed();
        let encode_started = std::time::Instant::now();
        let tiles = build_tiles_sorted(&permuted, budget);
        let encode = encode_started.elapsed();
        profiles.push(ReferencePermutationProfile {
            normalization,
            sort,
            encode,
            input_records: permuted.len(),
            output_tiles: tiles.len(),
        });
        sections.push(tiles);
    }
    let sections: [Vec<Tile>; NUM_PERMS] = sections.try_into().map_err(|_| ()).unwrap();
    let profiles: [ReferencePermutationProfile; NUM_PERMS] =
        profiles.try_into().map_err(|_| ()).unwrap();
    (GraphIndex::from_sections(sections, PermSet::ALL), profiles)
}

/// The six tiled permutation sections, queryable by triple pattern.
pub struct GraphIndex {
    /// Tiles per permutation (SPO, SOP, PSO, POS, OSP, OPS — see [`ALL_PERMS`]),
    /// ascending in their leading-id ranges; consecutive tiles may share a
    /// leading id when a mega-group was split (see [`build_tiles`]).
    pub(crate) sections: [Vec<Tile>; NUM_PERMS],
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
    /// Set when the loader failed for some tile: results may be incomplete and
    /// the caller must surface an error rather than the partial answer.
    load_failed: std::sync::atomic::AtomicBool,
    /// The reader's concurrent-range fan-out (1 = strictly sequential) — see
    /// [`set_read_concurrency`](Self::set_read_concurrency).
    read_concurrency: usize,
    /// Session policy shared with sibling indexes and the dictionary reader.
    adaptive_controller: Option<Arc<AdaptiveReadController>>,
    /// Research-only selection of the unchecked cursor. Default artifacts do
    /// not contain this field or its decoder.
    #[cfg(feature = "unsafe-decode-bench")]
    unchecked_decode: std::sync::atomic::AtomicBool,
}

impl GraphIndex {
    fn from_sections(sections: [Vec<Tile>; NUM_PERMS], perms: PermSet) -> Self {
        GraphIndex {
            sections,
            perms,
            loader: None,
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            adaptive_controller: None,
            #[cfg(feature = "unsafe-decode-bench")]
            unchecked_decode: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Expand the three family pairs into `ALL_PERMS` section order: SPO, POS,
    /// OSP, SOP, PSO, OPS. Existing planners and loaders keep their APIs.
    #[allow(dead_code)] // Staged until the format writer consumes paired families.
    pub(crate) fn from_families(families: [FamilyIndex; 3]) -> Self {
        let mut sections: [Vec<Tile>; NUM_PERMS] = std::array::from_fn(|_| Vec::new());
        for family in families {
            let slot = family.family.slot();
            for tile in family.tiles {
                sections[slot].push(Tile::local(tile.min_a, tile.max_a, tile.first));
                sections[slot + 3].push(Tile::local(tile.min_a, tile.max_a, tile.second));
            }
        }
        Self::from_sections(sections, PermSet::ALL)
    }

    /// Borrow the two logical sections which make up one physical family.
    #[allow(dead_code)]
    pub(crate) fn family_view(&self, family: IndexFamily) -> FamilyView<'_> {
        let slot = family.slot();
        FamilyView {
            family,
            first: &self.sections[slot],
            second: &self.sections[slot + 3],
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
            dir.into_iter()
                .map(|(min_a, max_a, syn)| Tile::remote(min_a, max_a, syn))
                .collect()
        });
        GraphIndex {
            sections,
            perms,
            loader: Some(loader),
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
            read_concurrency: 1,
            adaptive_controller: None,
            #[cfg(feature = "unsafe-decode-bench")]
            unchecked_decode: std::sync::atomic::AtomicBool::new(false),
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
    pub unsafe fn assume_valid_blocks(&self) {
        self.unchecked_decode
            .store(true, std::sync::atomic::Ordering::Relaxed);
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

    pub(crate) fn set_adaptive_controller(
        &mut self,
        controller: Option<Arc<AdaptiveReadController>>,
    ) {
        self.adaptive_controller = controller;
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
    fn prefetch_span(&self, section: usize, start: usize, end: usize, intent: ReadIntent) -> usize {
        let missing: Vec<usize> = (start..end)
            .filter(|&ti| self.sections[section][ti].data.get().is_none())
            .collect();
        let offered = missing.len();
        self.bulk_fault(section, &missing, intent);
        offered
    }

    /// Bulk-fault a set of (possibly scattered, ascending) missing tile indices in
    /// `section` through the bulk loader in one coalesced read, storing each image.
    /// No-op without a bulk loader or for fewer than two tiles (a single fault
    /// costs the same via the per-tile loader). A failed batch leaves the tiles
    /// unloaded for the per-tile loader to retry. Shared by the consecutive-span
    /// scan prefetch and the scattered batch-probe prefetch.
    fn bulk_fault(&self, section: usize, tiles: &[usize], intent: ReadIntent) {
        if tiles.len() < 2 {
            return;
        }
        let Some(bulk) = &self.bulk else { return };
        if let Some(images) = bulk(section, tiles, intent) {
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
            self.bulk_fault(si, &tiles, ReadIntent::SelectiveProbe);
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
        self.prefetch_span(0, 0, self.sections[0].len(), ReadIntent::FullScan);
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
        let mut out: Vec<Triple> = self
            .scan_iter_with_intent(pattern, ReadIntent::FullScan)
            .collect();
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
        self.scan_iter_with_intent(pattern, ReadIntent::BoundedScan)
    }

    pub(crate) fn scan_iter_with_intent(
        &self,
        pattern: Pattern,
        intent: ReadIntent,
    ) -> impl Iterator<Item = Triple> + '_ {
        self.scan_iter_with(pattern, Self::best_permutation(pattern), intent)
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
        for t in &tiles[start..end] {
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
            section_bytes: tiles.iter().map(|t| t.encoded_len()).sum(),
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
            if self.sections[si][ti].data.get().is_none() {
                self.prefetch_span(si, ti, (ti + window).min(end), ReadIntent::BoundedScan);
                window = window.saturating_mul(2).min(PREFETCH_WINDOW_MAX);
            }
            let tile = &self.sections[si][ti];
            let mut resume_at: Option<u32> = None;
            if let Some(block) = TripleBlock::parse(self.tile_data(si, ti))
                .ok()
                .filter(|b| b.zone().may_contain(pa, pb, pc))
            {
                let rows = match pa {
                    // A bound leading id is one a-group; it cannot be split
                    // inside a tile, only across tiles (a mega-group split), so
                    // there is nothing finer to resume at than the tile.
                    Some(a) => {
                        let dir = tile.dir.get_or_init(|| block.group_directory());
                        block.scan_from(dir, a, pb, pc)
                    }
                    // Fresh tile: no directory needed, so a straight drain never
                    // pays for one.
                    None if from_a == 0 => block.scan(None, pb, pc),
                    None => {
                        let dir = tile.dir.get_or_init(|| block.group_directory());
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
        Some(self.scan_iter_with(
            pattern,
            Self::permutation_sorted_on_in(self.perms, pattern, sort_col)?,
            ReadIntent::FullScan,
        ))
    }

    /// Stream `pattern`'s matches using a **specific** permutation `perm`; the
    /// stream is sorted in `perm`'s `(a, b, c)` order. Shared core of `scan_iter`
    /// and `scan_iter_sorted_on`.
    fn scan_iter_with(
        &self,
        pattern: Pattern,
        perm: IndexPermutation,
        intent: ReadIntent,
    ) -> impl Iterator<Item = Triple> + '_ {
        let [pa, pb, pc] = perm.order_pattern(pattern);
        self.scan_permuted_with(perm, pa, pb, pc, intent)
            .map(move |abc| perm.back(abc))
    }

    /// Stream triples in their stored permutation order. Keeping this below
    /// the canonical mapping lets prefix-2 consumers project the neighbor ID
    /// directly without constructing or reordering canonical triples.
    fn scan_permuted_with(
        &self,
        perm: IndexPermutation,
        pa: Option<u32>,
        pb: Option<u32>,
        pc: Option<u32>,
        intent: ReadIntent,
    ) -> impl Iterator<Item = Triple> + '_ {
        // Route: a bound leading id binary-searches the tile directory to exactly
        // one tile (groups are never split across tiles); an unbound one chains
        // every tile's cursor. Within a tile, a bound leading scan jumps to its
        // a-group via the tile's lazily-built group directory — built on first
        // use, costing one walk of that (budget-sized) tile.
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, pa);
        // Coalesce tile faults, but ramp the prefetch window geometrically as
        // the scan advances rather than fetching the whole span up front: a
        // small `LIMIT` (which stops pulling early) then never faults tiles
        // past the rows it needs, while a full scan still batches into a
        // handful of coalesced reads (4, 8, 16, … tiles). A bound leading scan
        // spans a single tile, so the prefetch no-ops and it faults just that
        // one tile, unchanged.
        let known_bytes = self.sections[si][start..end]
            .iter()
            .fold(0u64, |total, tile| total.saturating_add(tile.len as u64));
        let plan = self
            .adaptive_controller
            .as_ref()
            .map(|controller| controller.plan(intent, known_bytes, 0, self.read_concurrency));
        let window = Cell::new(plan.map_or(PREFETCH_WINDOW_START, |plan| plan.prefetch_start));
        let window_cap = plan.map_or(PREFETCH_WINDOW_MAX, |plan| plan.prefetch_cap);
        let window_offered = Cell::new(0usize);
        let window_consumed = Cell::new(0usize);
        let feedback = ScanFeedback::new(self.adaptive_controller.clone(), intent);
        (start..end)
            // Synopsis pre-fault prune: drop a routed tile the directory proves
            // can't match a bound secondary component, **without fetching it**
            // (the remote win — a negative/sparse lookup costs zero tile reads).
            // A bound leading id routes to a single tile, so this is where it
            // bites; a fully-unbound leading scan leaves `pb`/`pc` unbound, so
            // `syn_admits` keeps every tile, unchanged.
            .filter(move |&ti| self.sections[si][ti].syn_admits(pb, pc))
            .flat_map(move |ti| {
                crate::read_path_metrics::record_tile(si, ti);
                // Fault in (if remote), parse (untrusted bytes ⇒ `None` on
                // malformed), and zone-prune per tile, then stream the
                // matching groups.
                if self.sections[si][ti].data.get().is_none() {
                    let mut w = window.get();
                    if window_offered.get() > 0 && window_consumed.get() >= window_offered.get() {
                        w = w.saturating_mul(2).min(window_cap);
                    }
                    let offered = self.prefetch_span(si, ti, (ti + w).min(end), intent);
                    feedback.offer_tiles(offered);
                    window.set(w);
                    window_offered.set(offered);
                    window_consumed.set(0);
                }
                feedback.consume_tile();
                window_consumed.set(window_consumed.get().saturating_add(1));
                let tile = &self.sections[si][ti];
                TripleBlock::parse(self.tile_data(si, ti))
                    .ok()
                    .filter(|b| b.zone().may_contain(pa, pb, pc))
                    .map(|b| {
                        #[cfg(feature = "unsafe-decode-bench")]
                        {
                            if self
                                .unchecked_decode
                                .load(std::sync::atomic::Ordering::Relaxed)
                            {
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
    }

    /// Stream the stored third-component IDs for one exact `(a, b)` prefix.
    /// This retains ordinary tile routing, lazy faulting, corruption handling,
    /// and split-group chaining while avoiding canonical triple materialization.
    pub(crate) fn scan_prefix2(
        &self,
        permutation: IndexPermutation,
        a: u32,
        b: u32,
    ) -> impl Iterator<Item = u32> + '_ {
        self.scan_permuted_with(
            permutation,
            Some(a),
            Some(b),
            None,
            ReadIntent::SelectiveProbe,
        )
        .map(|(_, _, c)| c)
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
    use crate::adaptive::{AdaptiveReadController, ReadObservation};
    use std::sync::{Arc, Mutex};

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

    #[test]
    fn paired_families_expand_to_every_existing_permutation_view() {
        let (_, data) = graph();
        let paired = GraphIndexBuilder::from_triples(data.clone())
            .with_tile_budget(64)
            .build_families()
            .unwrap();
        for permutation in ALL_PERMS {
            let mut expected: Vec<_> = data
                .iter()
                .copied()
                .map(|triple| permutation.forward(triple))
                .collect();
            expected.sort_unstable();
            expected.dedup();
            let section = paired.tile_sections()[permutation.section_index()];
            let actual: Vec<_> = section
                .iter()
                .flat_map(|tile| TripleBlock::parse(tile.bytes()).unwrap().triples())
                .collect();
            assert_eq!(actual, expected, "{}", permutation.name());
            let (slot, second) = permutation.family_slot();
            let family = [
                IndexFamily::Subject,
                IndexFamily::Predicate,
                IndexFamily::Object,
            ][slot];
            let view = paired.family_view(family);
            let selected = if second { view.second } else { view.first };
            assert_eq!(
                selected.iter().map(Tile::bytes).collect::<Vec<_>>(),
                section.iter().map(Tile::bytes).collect::<Vec<_>>(),
                "{} family slot",
                permutation.name()
            );
            assert_eq!(view.family, family);
        }
    }

    #[test]
    fn adaptive_bounded_scan_reports_unused_prefetch_on_drop() {
        let mut blocks = Vec::new();
        let mut spo = Vec::new();
        for id in 1..=12u32 {
            let mut builder = TripleBlockBuilder::new();
            builder.push((id, 1, id + 100));
            blocks.push(builder.build());
            spo.push((id, id, None));
        }
        let images = Arc::new(blocks);
        let single = images.clone();
        let loader: TileLoader = Box::new(move |_section, tile| single.get(tile).cloned());
        let batches = Arc::new(Mutex::new(Vec::new()));
        let recorded = batches.clone();
        let bulk_images = images.clone();
        let bulk: TileBulkLoader = Box::new(move |_section, tiles, intent| {
            recorded.lock().unwrap().push((tiles.to_vec(), intent));
            tiles
                .iter()
                .map(|&tile| bulk_images.get(tile).cloned())
                .collect()
        });
        let mut dirs = std::array::from_fn(|_| Vec::new());
        dirs[0] = spo;
        let controller = Arc::new(AdaptiveReadController::new());
        for _ in 0..2 {
            controller.observe(ReadObservation {
                requested_bytes: 1024 * 1024,
                returned_bytes: 1024 * 1024,
                physical_ranges: 1,
                elapsed_micros: Some(120_000),
                success: true,
            });
        }
        let mut index =
            GraphIndex::from_remote_directories(dirs, PermSet::ALL, loader).with_bulk_loader(bulk);
        index.set_adaptive_controller(Some(controller.clone()));

        let first = index
            .scan_iter_with_intent((None, None, None), ReadIntent::BoundedScan)
            .next();
        assert_eq!(first, Some((1, 1, 101)));

        let batches = batches.lock().unwrap();
        assert_eq!(batches.len(), 1, "LIMIT-like demand began a second window");
        assert_eq!(batches[0].0.len(), 8);
        assert_eq!(batches[0].1, ReadIntent::BoundedScan);
        drop(batches);
        let next = controller.plan(ReadIntent::BoundedScan, 1024 * 1024, 4096, 8);
        assert_eq!(next.prefetch_start, 2, "unused window was not reported");
    }

    #[test]
    fn adaptive_full_scan_uses_full_intent_and_consumes_every_tile() {
        let mut blocks = Vec::new();
        let mut spo = Vec::new();
        for id in 1..=40u32 {
            let mut builder = TripleBlockBuilder::new();
            builder.push((id, 1, id + 100));
            blocks.push(builder.build());
            spo.push((id, id, None));
        }
        let images = Arc::new(blocks);
        let single = images.clone();
        let loader: TileLoader = Box::new(move |_section, tile| single.get(tile).cloned());
        let batches = Arc::new(Mutex::new(Vec::new()));
        let recorded = batches.clone();
        let bulk_images = images.clone();
        let bulk: TileBulkLoader = Box::new(move |_section, tiles, intent| {
            recorded.lock().unwrap().push((tiles.to_vec(), intent));
            tiles
                .iter()
                .map(|&tile| bulk_images.get(tile).cloned())
                .collect()
        });
        let mut dirs = std::array::from_fn(|_| Vec::new());
        dirs[0] = spo;
        let controller = Arc::new(AdaptiveReadController::new());
        for _ in 0..2 {
            controller.observe(ReadObservation {
                requested_bytes: 1024 * 1024,
                returned_bytes: 1024 * 1024,
                physical_ranges: 1,
                elapsed_micros: Some(120_000),
                success: true,
            });
        }
        let mut index =
            GraphIndex::from_remote_directories(dirs, PermSet::ALL, loader).with_bulk_loader(bulk);
        index.set_adaptive_controller(Some(controller));

        assert_eq!(
            index
                .scan_iter_with_intent((None, None, None), ReadIntent::FullScan)
                .count(),
            40
        );
        let batches = batches.lock().unwrap();
        assert_eq!(
            batches
                .iter()
                .map(|(tiles, _)| tiles.len())
                .collect::<Vec<_>>(),
            vec![8, 16, 16]
        );
        assert_eq!(
            batches.iter().map(|(tiles, _)| tiles.len()).sum::<usize>(),
            40
        );
        assert!(batches
            .iter()
            .all(|(_, intent)| *intent == ReadIntent::FullScan));
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
    fn prefix2_neighbor_scan_matches_each_permutation() {
        let (index, data) = graph();
        for permutation in ALL_PERMS {
            let stored: Vec<_> = data
                .iter()
                .copied()
                .map(|triple| permutation.forward(triple))
                .collect();
            let mut prefixes: Vec<_> = stored.iter().map(|&(a, b, _)| (a, b)).collect();
            prefixes.push((u32::MAX, u32::MAX));
            prefixes.sort_unstable();
            prefixes.dedup();
            for (a, b) in prefixes {
                let mut want: Vec<_> = stored
                    .iter()
                    .filter(|&&(x, y, _)| x == a && y == b)
                    .map(|&(_, _, c)| c)
                    .collect();
                want.sort_unstable();
                assert_eq!(
                    index.scan_prefix2(permutation, a, b).collect::<Vec<_>>(),
                    want,
                    "{} ({a}, {b})",
                    permutation.name()
                );
            }
        }
    }

    #[test]
    fn prefix2_neighbor_scan_chains_a_split_group() {
        let mut builder = GraphIndexBuilder::new().with_tile_budget(128);
        let want: Vec<u32> = (10_000..50_000).collect();
        for &object in &want {
            builder.push((7, 11, object));
        }
        let index = builder.build();

        assert!(
            index
                .tile_span(IndexPermutation::Spo.section_index(), Some(7))
                .1
                > 1
        );
        assert_eq!(
            index
                .scan_prefix2(IndexPermutation::Spo, 7, 11)
                .collect::<Vec<_>>(),
            want
        );
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
