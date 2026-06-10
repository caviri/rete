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

use crate::triples::{GroupDirectory, Triple, TripleBlock, TripleBlockBuilder};

/// A triple pattern: `None` is an unbound variable, `Some(id)` a bound term.
pub type Pattern = (Option<u32>, Option<u32>, Option<u32>);

/// Default per-tile budget in (uncompressed) encoded bytes. Tiles are the
/// independently-compressed, independently-fetchable units of a permutation
/// section: ~64 KiB matches both zstd's sweet spot and one HTTP range read.
pub const INDEX_TILE_BUDGET: usize = 64 * 1024;

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
    data: OnceLock<Vec<u8>>,
    dir: OnceLock<GroupDirectory>,
}

impl Tile {
    fn local(min_a: u32, max_a: u32, bytes: Vec<u8>) -> Self {
        let data = OnceLock::new();
        let _ = data.set(bytes);
        Tile {
            min_a,
            max_a,
            data,
            dir: OnceLock::new(),
        }
    }

    fn remote(min_a: u32, max_a: u32) -> Self {
        Tile {
            min_a,
            max_a,
            data: OnceLock::new(),
            dir: OnceLock::new(),
        }
    }

    /// Leading-component (permuted `a`) range this tile covers, inclusive.
    pub fn leading_range(&self) -> (u32, u32) {
        (self.min_a, self.max_a)
    }

    /// The tile's serialized (uncompressed) [`TripleBlock`] image, if present
    /// locally (always, for built/opened indexes; empty for an unfaulted
    /// remote tile — the writer never sees those).
    pub fn bytes(&self) -> &[u8] {
        self.data.get().map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Which stored permutation to scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexPermutation {
    Spo,
    Pos,
    Osp,
}

impl IndexPermutation {
    /// Stable display name used in CLI diagnostics and provenance records.
    pub fn name(self) -> &'static str {
        match self {
            IndexPermutation::Spo => "SPO",
            IndexPermutation::Pos => "POS",
            IndexPermutation::Osp => "OSP",
        }
    }

    /// Section number inside the stored permutation container.
    pub fn section_index(self) -> usize {
        match self {
            IndexPermutation::Spo => 0,
            IndexPermutation::Pos => 1,
            IndexPermutation::Osp => 2,
        }
    }

    /// Map a canonical `(s, p, o)` triple into this permutation's `(a, b, c)`.
    pub(crate) fn forward(self, (s, p, o): Triple) -> Triple {
        match self {
            IndexPermutation::Spo => (s, p, o),
            IndexPermutation::Pos => (p, o, s),
            IndexPermutation::Osp => (o, s, p),
        }
    }

    /// Map a stored `(a, b, c)` back to canonical `(s, p, o)`.
    fn back(self, (a, b, c): Triple) -> Triple {
        match self {
            IndexPermutation::Spo => (a, b, c),
            IndexPermutation::Pos => (c, a, b), // a=p, b=o, c=s
            IndexPermutation::Osp => (b, c, a), // a=o, b=s, c=p
        }
    }

    /// The pattern's bound components in this permutation's component order.
    pub(crate) fn order_pattern(self, (s, p, o): Pattern) -> [Option<u32>; 3] {
        match self {
            IndexPermutation::Spo => [s, p, o],
            IndexPermutation::Pos => [p, o, s],
            IndexPermutation::Osp => [o, s, p],
        }
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

    /// Override the per-tile byte budget (tests use tiny budgets to force
    /// multi-tile sections on small data).
    pub fn with_tile_budget(mut self, bytes: usize) -> Self {
        self.tile_budget = bytes.max(1);
        self
    }

    pub fn push(&mut self, t: Triple) {
        self.triples.push(t);
    }

    pub fn build(self) -> GraphIndex {
        let sections = [
            IndexPermutation::Spo,
            IndexPermutation::Pos,
            IndexPermutation::Osp,
        ]
        .map(|perm| {
            let permuted: Vec<Triple> = self.triples.iter().map(|&t| perm.forward(t)).collect();
            build_tiles(permuted, self.tile_budget)
        });
        GraphIndex::from_sections(sections)
    }
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

/// Split sorted, deduped permuted triples into size-targeted tiles on whole
/// a-group boundaries: append groups until the (estimated, near-exact) encoded
/// size would exceed `budget`, then flush. A single group larger than the
/// budget becomes one oversized tile — groups are never split, so a bound
/// leading id always routes to exactly one tile.
fn build_tiles(mut triples: Vec<Triple>, budget: usize) -> Vec<Tile> {
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
    let mut tile_size = 0usize;
    let mut prev_a = 0u32;
    let mut i = 0usize;
    while i < triples.len() {
        // Measure the a-group starting at `i` (its encoded size given the
        // running delta chain — near-exact; the budget is a soft target).
        let a = triples[i].0;
        let mut gsize = varint_len((a - prev_a) as u64);
        let gstart = i;
        let mut num_b = 0u64;
        while i < triples.len() && triples[i].0 == a {
            let b = triples[i].1;
            num_b += 1;
            let prev_b = if num_b == 1 { 0 } else { triples[i - 1].1 };
            gsize += varint_len((b - prev_b) as u64);
            let mut num_c = 0u64;
            let mut prev_c = 0u32;
            while i < triples.len() && triples[i].0 == a && triples[i].1 == b {
                gsize += varint_len((triples[i].2 - prev_c) as u64);
                prev_c = triples[i].2;
                num_c += 1;
                i += 1;
            }
            gsize += varint_len(num_c);
        }
        gsize += varint_len(num_b);

        if gstart > tile_start && tile_size + gsize > budget {
            tiles.push(make_tile(&triples[tile_start..gstart]));
            tile_start = gstart;
            tile_size = 0;
        }
        tile_size += gsize;
        prev_a = a;
    }
    tiles.push(make_tile(&triples[tile_start..]));
    tiles
}

/// The three tiled permutation sections, queryable by triple pattern.
pub struct GraphIndex {
    /// Tiles per permutation (SPO, POS, OSP), ascending and disjoint in their
    /// leading-id ranges.
    sections: [Vec<Tile>; 3],
    /// Faults in remote tiles on first scan (`None` for local indexes).
    loader: Option<TileLoader>,
    /// Optional batched fetch for multi-tile scans (`None` falls back to
    /// one [`TileLoader`] call per tile).
    bulk: Option<TileBulkLoader>,
    /// Set when the loader failed for some tile: results may be incomplete and
    /// the caller must surface an error rather than the partial answer.
    load_failed: std::sync::atomic::AtomicBool,
}

impl GraphIndex {
    fn from_sections(sections: [Vec<Tile>; 3]) -> Self {
        GraphIndex {
            sections,
            loader: None,
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Rebuild from three single serialized permutation blocks (SPO, POS,
    /// OSP) — the v0.1 layout, and the natural form for tests. Each block
    /// becomes a one-tile section (its leading range read off the zone map).
    pub fn from_blocks(blocks: [Vec<u8>; 3]) -> Self {
        let sections = blocks.map(|bytes| {
            match TripleBlock::parse(&bytes) {
                Ok(b) if b.zone().count > 0 => {
                    vec![Tile::local(b.zone().min_a, b.zone().max_a, bytes)]
                }
                // Empty or unparsable: an empty section (scans yield nothing
                // either way; this just skips the dead tile).
                _ => Vec::new(),
            }
        });
        Self::from_sections(sections)
    }

    /// Rebuild from tiled sections: per permutation, `(min_a, max_a, block
    /// bytes)` per tile in ascending leading-id order — the v0.2 layout.
    pub fn from_tiles(sections: [Vec<(u32, u32, Vec<u8>)>; 3]) -> Self {
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
    pub fn from_remote_directories(directories: [Vec<(u32, u32)>; 3], loader: TileLoader) -> Self {
        let sections = directories.map(|dir| {
            dir.into_iter()
                .map(|(min_a, max_a)| Tile::remote(min_a, max_a))
                .collect()
        });
        GraphIndex {
            sections,
            loader: Some(loader),
            bulk: None,
            load_failed: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Attach a batched tile fetcher (see [`TileBulkLoader`]): multi-tile
    /// scans prefetch their span through it instead of faulting tile by tile.
    pub fn with_bulk_loader(mut self, bulk: TileBulkLoader) -> Self {
        self.bulk = Some(bulk);
        self
    }

    /// Did any tile fetch fail since this index was opened? (Sticky.)
    pub fn load_incomplete(&self) -> bool {
        self.load_failed.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Batch-fault the unloaded tiles in `[start, end)` of `section` through
    /// the bulk loader, if one is attached and at least two tiles are missing
    /// (a single missing tile costs the same either way). A failed batch is
    /// not an error here: the tiles stay unloaded and the per-tile loader
    /// retries each one (recording failures) when the scan reaches it.
    fn prefetch_span(&self, section: usize, start: usize, end: usize) {
        let Some(bulk) = &self.bulk else { return };
        let missing: Vec<usize> = (start..end)
            .filter(|&ti| self.sections[section][ti].data.get().is_none())
            .collect();
        if missing.len() < 2 {
            return;
        }
        if let Some(images) = bulk(section, &missing) {
            if images.len() == missing.len() {
                for (&ti, img) in missing.iter().zip(images) {
                    let _ = self.sections[section][ti].data.set(img);
                }
            }
        }
    }

    /// The tile's block image, faulting it in through the loader if remote.
    fn tile_data(&self, section: usize, tile: usize) -> &[u8] {
        self.sections[section][tile].data.get_or_init(|| {
            match &self.loader {
                Some(load) => load(section, tile).unwrap_or_else(|| {
                    self.load_failed
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    Vec::new()
                }),
                // A local tile with no data was constructed empty on purpose.
                None => Vec::new(),
            }
        })
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

    /// The tiles of each permutation section (SPO, POS, OSP), for the file
    /// writer.
    pub fn tile_sections(&self) -> [&[Tile]; 3] {
        [&self.sections[0], &self.sections[1], &self.sections[2]]
    }

    /// The permutation selected for a pattern: the one with the longest bound
    /// prefix. Ties keep the canonical SPO order (then POS, then OSP), which
    /// makes provenance stable for unbound or equally selective shapes and routes
    /// a fully unbound pattern to the SPO block rather than fetching all three.
    pub fn best_permutation(pattern: Pattern) -> IndexPermutation {
        let mut best = IndexPermutation::Spo;
        let mut best_score = best.leading_bound(pattern);
        for perm in [IndexPermutation::Pos, IndexPermutation::Osp] {
            let score = perm.leading_bound(pattern);
            if score > best_score {
                best = perm;
                best_score = score;
            }
        }
        best
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
    /// than panicking.
    pub fn scan_iter(&self, pattern: Pattern) -> impl Iterator<Item = Triple> + '_ {
        // Pick the permutation with the longest leading-bound prefix, then
        // route: a bound leading id binary-searches the tile directory to
        // exactly one tile (groups are never split across tiles); an unbound
        // one chains every tile's cursor. Within a tile, a bound leading scan
        // jumps to its a-group via the tile's lazily-built group directory —
        // built on first use, costing one walk of that (budget-sized) tile.
        let perm = Self::best_permutation(pattern);
        let [pa, pb, pc] = perm.order_pattern(pattern);
        let si = perm.section_index();
        let (start, end) = self.tile_span(si, pa);
        // A multi-tile span (unbound leading component) batch-faults its
        // missing tiles in coalesced range reads instead of one per tile.
        self.prefetch_span(si, start, end);
        (start..end)
            .flat_map(move |ti| {
                // Fault in (if remote), parse (untrusted bytes ⇒ `None` on
                // malformed), and zone-prune per tile, then stream the
                // matching groups.
                let tile = &self.sections[si][ti];
                TripleBlock::parse(self.tile_data(si, ti))
                    .ok()
                    .filter(|b| b.zone().may_contain(pa, pb, pc))
                    .map(|b| match pa {
                        Some(a) => {
                            let dir = tile.dir.get_or_init(|| b.group_directory());
                            b.scan_from(dir, a, pb, pc)
                        }
                        None => b.scan(pa, pb, pc),
                    })
                    .into_iter()
                    .flatten()
            })
            .map(move |abc| perm.back(abc))
    }

    /// The tile index span a scan must visit: every tile when the leading
    /// component is unbound, else the single tile whose leading-id range
    /// covers it (tile ranges are ascending and disjoint).
    fn tile_span(&self, section: usize, pa: Option<u32>) -> (usize, usize) {
        let tiles = &self.sections[section];
        match pa {
            None => (0, tiles.len()),
            Some(a) => {
                let i = tiles.partition_point(|t| t.max_a < a);
                if i < tiles.len() && tiles[i].min_a <= a {
                    (i, i + 1)
                } else {
                    (0, 0)
                }
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

    /// A tiny tile budget must split sections into many tiles, and every
    /// pattern shape must still match the brute-force reference — bound
    /// leading ids route to exactly one tile, unbound scans chain all tiles.
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
            // Tile ranges must be ascending and disjoint.
            for w in idx.tile_sections()[0].windows(2) {
                assert!(w[0].leading_range().1 < w[1].leading_range().0);
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
}
