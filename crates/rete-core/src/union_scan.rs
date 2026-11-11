//! The union default graph over a quad index, streamed.
//!
//! A query with the union default graph on sees the file's own default graph
//! and every named graph as one set of triples. Building that set as an index
//! reads every graph's tiles, which on a file of millions of graphs is the
//! whole quad section. The graph-last orders of a `0x07` quad index make it a
//! scan instead: SPOG, POSG and OSPG sort a quad's `(s, p, o)` before its graph,
//! so dropping the graph column leaves the triples in the order's own sequence
//! with the copies of one triple next to each other.
//!
//! A [`UnionOverlay`] sits inside a [`GraphIndex`] built by
//! [`GraphIndex::union_of`]. Every scan the index answers goes to
//! [`UnionOverlay::scan`], which streams the quad index's matches in one order,
//! drops the repeats, and merges them with the file's default-graph index read
//! in the same order. Nothing is materialized, and a scan reads the tiles its
//! pattern routes to.

use std::collections::HashSet;
use std::iter::Peekable;
use std::sync::Arc;

use crate::index::{GraphIndex, IndexPermutation, Pattern, PermSet, ScanPlan};
use crate::quad::{QuadScan, QuadSrc};
use crate::triples::Triple;

/// The slice of a scan's tile range a shard keeps: `(start, end)` of the range
/// in, the part to keep out.
pub(crate) type Cut<'c> = &'c dyn Fn(usize, usize) -> (usize, usize);

/// The default-graph index and the quad index, read as one graph.
pub(crate) struct UnionOverlay {
    base: Arc<GraphIndex>,
    quad: QuadSrc,
    /// The default graph holds no triple: nothing to merge.
    base_empty: bool,
}

type Boxed<'a> = Box<dyn Iterator<Item = Triple> + 'a>;

impl GraphIndex {
    /// An index over the union of `base` (the file's default graph) and every
    /// graph of `quad`: see the module documentation.
    pub(crate) fn union_of(base: Arc<GraphIndex>, quad: QuadSrc) -> GraphIndex {
        let mut gi = GraphIndex::empty_with(PermSet::CORE);
        let base_empty = base
            .perms()
            .iter()
            .all(|p| base.tile_sections()[p.section_index()].is_empty());
        gi.union = Some(Box::new(UnionOverlay {
            base,
            quad,
            base_empty,
        }));
        gi
    }

    /// The union overlay, when this index is one.
    pub(crate) fn union(&self) -> Option<&UnionOverlay> {
        self.union.as_deref()
    }
}

/// `t`'s columns in the order `perm` sorts them.
fn key(perm: IndexPermutation, t: Triple) -> Triple {
    perm.forward(t)
}

impl UnionOverlay {
    /// The matches of `pattern` in `perm`'s order (`(a, b, c)` ascending),
    /// each triple once, the default graph's included. `cut` slices the work
    /// for a shard of a query run on several threads.
    ///
    /// `ordered` is false for the one layout whose order the sort-merge cannot
    /// rely on (a clustered order with no bound leading column); the output is
    /// then still each triple once, but not in `perm`'s order.
    pub(crate) fn scan<'a>(
        &'a self,
        pattern: Pattern,
        perm: IndexPermutation,
        cut: Option<Cut<'_>>,
    ) -> (Boxed<'a>, bool) {
        let base_ordered = self.base_ordered(perm, pattern);
        let base_scan = |cut: Option<Cut<'_>>| -> Boxed<'a> {
            if self.base_empty {
                return Box::new(std::iter::empty());
            }
            Box::new(self.base.scan_iter_in(pattern, perm, cut))
        };
        let Some(mut qs) = self.quad.scan(pattern, perm) else {
            // An unreadable section: the failure is recorded, the quads add
            // nothing.
            return (base_scan(cut), base_ordered);
        };
        let (start, end) = qs.tile_range();
        if start >= end {
            // Nothing in the quad index routes here: the default graph alone,
            // sliced by its own tiles.
            return (base_scan(cut), base_ordered);
        }
        let ordered = qs.globally_ordered() && self.base_ordered(perm, pattern);
        let base_perm = |c: Option<Cut<'_>>| -> Boxed<'a> {
            if self.base_empty {
                Box::new(std::iter::empty())
            } else {
                Box::new(self.base.scan_iter_in(pattern, perm, c))
            }
        };
        let Some(cut) = cut else {
            let quads = Distinct::new(qs, None, None);
            return if self.base_empty {
                (Box::new(quads), ordered)
            } else if ordered {
                (Box::new(Merge::new(perm, base_perm(None), quads)), true)
            } else {
                (Box::new(unordered_union(base_perm(None), quads)), false)
            };
        };
        // A default graph and an unordered quad scan cannot be sliced: the one
        // shard that `cut` gives the whole of a one-tile range runs everything.
        if !ordered && !self.base_empty {
            let (a, b) = cut(0, 1);
            if (a, b) == (0, 1) {
                let quads = Distinct::new(qs, None, None);
                return (Box::new(unordered_union(base_perm(None), quads)), false);
            }
            return (Box::new(std::iter::empty()), false);
        }
        // A shard of a query on several threads: the slice `cut` keeps of the
        // quad scan's tiles.
        let (lo, hi) = cut(start, end);
        let lo = lo.clamp(start, end);
        let hi = hi.clamp(lo, end);
        if lo >= hi {
            return (Box::new(std::iter::empty()), ordered);
        }
        if ordered {
            // The shard owns the triples whose key lies in `[first key of tile
            // lo, first key of tile hi)`: a triple whose quads straddle a tile
            // boundary is the one shard's, and so is each default-graph triple.
            let from = (lo > start).then(|| qs.first_key(lo)).flatten();
            let stop = (hi < end).then(|| qs.first_key(hi)).flatten();
            let split = (lo > start && from.is_none()) || (hi < end && stop.is_none());
            if split {
                // A tile that cannot be read: the failure is recorded.
                return (Box::new(std::iter::empty()), ordered);
            }
            qs.restrict(lo, hi);
            let quads = Distinct::new(qs, None, stop);
            if self.base_empty {
                return (Box::new(quads), true);
            }
            let in_range = move |t: &Triple| {
                let k = key(perm, *t);
                from.is_none_or(|f| k >= f) && stop.is_none_or(|s| k < s)
            };
            let base = base_perm(None).filter(in_range);
            return (Box::new(Merge::new(perm, Box::new(base), quads)), true);
        }
        // Not globally ordered. With no default graph, a shard skips the rows
        // that repeat the last row of the tile before its first.
        if self.base_empty {
            let prev = if lo > start {
                qs.last_key(lo - 1).map(|k| perm.back(k))
            } else {
                None
            };
            qs.restrict(lo, hi);
            return (Box::new(Distinct::new(qs, prev, None)), false);
        }
        (Box::new(std::iter::empty()), false)
    }

    /// Does the default graph's scan of `pattern` in `perm` come in order?
    fn base_ordered(&self, perm: IndexPermutation, pattern: Pattern) -> bool {
        self.base_empty || self.base.scan_is_ordered(perm, pattern)
    }

    /// Will [`scan`](Self::scan) give `pattern` in `perm`'s order?
    pub(crate) fn scan_is_ordered(&self, pattern: Pattern, perm: IndexPermutation) -> bool {
        self.quad
            .scan(pattern, perm)
            .is_some_and(|qs| qs.globally_ordered())
            && self.base_ordered(perm, pattern)
    }

    /// The batched pull form of a scan: matches from the `cursor`th on.
    pub(crate) fn scan_batch(
        &self,
        pattern: Pattern,
        cursor: u64,
        max_rows: usize,
        perm: IndexPermutation,
    ) -> (Vec<Triple>, u64, bool) {
        let (it, _) = self.scan(pattern, perm, None);
        let skip = cursor as usize;
        let mut out: Vec<Triple> = it.skip(skip).take(max_rows + 1).collect();
        let done = out.len() <= max_rows;
        out.truncate(max_rows);
        let next = (skip + out.len()) as u64;
        out.sort_unstable();
        (out, next, done)
    }

    /// The tile directories' view of a scan of `pattern`.
    pub(crate) fn plan(&self, pattern: Pattern, perm: IndexPermutation) -> ScanPlan {
        let mut plan = if self.base_empty {
            ScanPlan {
                permutation: perm,
                tiles_total: 0,
                tiles_routed: 0,
                tiles_admitted: 0,
                tile_bytes: 0,
                section_bytes: 0,
            }
        } else {
            self.base.scan_plan(pattern)
        };
        plan.permutation = perm;
        if let Some(qs) = self.quad.scan(pattern, perm) {
            let q = qs.plan();
            plan.tiles_total += q.order_tiles;
            plan.tiles_routed += q.routed;
            plan.tiles_admitted += q.admitted;
            plan.tile_bytes += q.admitted_bytes;
            plan.section_bytes += q.routed_bytes;
        }
        plan
    }

    /// The stored bytes a scan of `pattern` routes to, from the directories.
    pub(crate) fn routed_bytes(&self, pattern: Pattern) -> u64 {
        let perm = GraphIndex::best_permutation_in(PermSet::CORE, pattern);
        let base = if self.base_empty {
            0
        } else {
            crate::bgp::routed_scan_bytes(&self.base, pattern)
        };
        let quad = self
            .quad
            .scan(pattern, perm)
            .map_or(0, |qs| qs.plan().routed_bytes);
        base.saturating_add(quad)
    }

    /// A planner estimate of the matches of `pattern`, as
    /// [`GraphIndex::estimate_matches`]: `total` bounds the figure.
    pub(crate) fn estimate(&self, pattern: Pattern, total: u64) -> f64 {
        let all = total as f64;
        if pattern == (None, None, None) {
            return all;
        }
        let base = if self.base_empty {
            0.0
        } else {
            self.base.estimate_matches(pattern, total)
        };
        let perm = GraphIndex::best_permutation_in(PermSet::CORE, pattern);
        let quads = match self.quad.scan(pattern, perm) {
            None => 0.0,
            Some(qs) => {
                let q = qs.plan();
                if q.admitted == 0 {
                    0.0
                } else {
                    let per_tile = q.quads as f64 / q.order_tiles as f64;
                    (q.admitted as f64 - 0.5).max(0.5) * per_tile
                }
            }
        };
        (base + quads).min(all)
    }

    pub(crate) fn is_remote(&self) -> bool {
        self.quad.is_remote() || self.base.is_remote()
    }

    pub(crate) fn read_concurrency(&self) -> usize {
        self.quad.read_concurrency()
    }
}

/// A quad scan with the graph dropped and the repeats of a triple removed. The
/// repeats are adjacent, because the graph is the order's last column.
///
/// `prev`: the triple the scan is taken to have just passed (a shard starting
/// mid-scan, after the last row of the tile before it). `stop`: the first key
/// (in the scan's order) that is not this scan's, a shard's upper bound.
struct Distinct {
    scan: QuadScan,
    prev: Option<Triple>,
    stop: Option<Triple>,
    perm: IndexPermutation,
    done: bool,
}

impl Distinct {
    fn new(scan: QuadScan, prev: Option<Triple>, stop: Option<Triple>) -> Self {
        let perm = scan.perm();
        Distinct {
            scan,
            prev,
            stop,
            perm,
            done: false,
        }
    }
}

impl Iterator for Distinct {
    type Item = Triple;

    fn next(&mut self) -> Option<Triple> {
        if self.done {
            return None;
        }
        for (_, t) in self.scan.by_ref() {
            if let Some(stop) = self.stop {
                if key(self.perm, t) >= stop {
                    self.done = true;
                    return None;
                }
            }
            if self.prev == Some(t) {
                continue;
            }
            self.prev = Some(t);
            return Some(t);
        }
        self.done = true;
        None
    }
}

/// Two streams sorted in `perm`'s order, merged, a triple in both once.
struct Merge<'a> {
    perm: IndexPermutation,
    a: Peekable<Boxed<'a>>,
    b: Peekable<Distinct>,
}

impl<'a> Merge<'a> {
    fn new(perm: IndexPermutation, a: Boxed<'a>, b: Distinct) -> Self {
        Merge {
            perm,
            a: a.peekable(),
            b: b.peekable(),
        }
    }
}

impl Iterator for Merge<'_> {
    type Item = Triple;

    fn next(&mut self) -> Option<Triple> {
        match (self.a.peek().copied(), self.b.peek().copied()) {
            (None, None) => None,
            (Some(_), None) => self.a.next(),
            (None, Some(_)) => self.b.next(),
            (Some(x), Some(y)) => match key(self.perm, x).cmp(&key(self.perm, y)) {
                std::cmp::Ordering::Less => self.a.next(),
                std::cmp::Ordering::Greater => self.b.next(),
                std::cmp::Ordering::Equal => {
                    self.b.next();
                    self.a.next()
                }
            },
        }
    }
}

/// The two streams as a set union when neither can be relied on to be in
/// order: the default graph's triples are held, the quads that repeat one are
/// skipped, then the default graph's are yielded.
fn unordered_union<'a>(base: Boxed<'a>, quads: Distinct) -> Boxed<'a> {
    let held: Vec<Triple> = base.collect();
    let set: HashSet<Triple> = held.iter().copied().collect();
    Box::new(quads.filter(move |t| !set.contains(t)).chain(held))
}
