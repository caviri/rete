//! Subject lookups in SPOG, the quad index's subject-first order, clustered by
//! class or not (`0x07`, SPEC.md section 6.6).
//!
//! A clustered SPOG is `k + 1` consecutive runs, each sorted by
//! `(s, p, o, g)`; a subject's rows are all in the run of its primary class. The
//! directory is the same two-level one as every order (leaves cut at the runs),
//! so a reader finds a subject's tiles by binary search inside a run, and does
//! not know the run until it looked:
//!
//! - a **point lookup** ([`QuadCore::resolve_run`]) tries the run that held the
//!   last subject, then the others in one batch, and remembers where it found
//!   it;
//! - a **window** of subjects ([`QuadCore::subject_rows`]), what a class-scoped
//!   `GRAPH ?g { ?s a <C> . ?s ?p ?o }` probes, asks the run of the class first
//!   for all of them in one batch of leaves and one of tiles, and only the
//!   subjects that run does not hold go to the others.
//!
//! The same code serves an unclustered SPOG as a single run.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::{QTile, QuadCore, QuadNamed, V06Quad};
use crate::quad7::{self, Row4};

/// Tiles read and decoded together while a window's rows are extracted.
const PROBE_CHUNK: usize = 128;

/// Bytes the estimate charges the graph walk for each graph it visits.
const GRAPH_VISIT_BYTES: f64 = 512.0;

/// Subjects an estimate samples to learn which runs they live in.
const SAMPLE_SUBJECTS: usize = 24;

/// A sorted run of leaf indices `[first, end)`.
type Leaves = (usize, usize);
/// The candidate tiles of one subject, per run: `(run, [(tile, entry)])`.
type RunTiles = Vec<(usize, Vec<(usize, QTile)>)>;
/// The rows of each subject and the run each was found in.
type FoundRows = (Vec<Vec<Row4>>, Vec<Option<usize>>);

impl QuadCore {
    /// Is order slot `si` clustered into class runs?
    pub(super) fn is_clustered(&self, si: usize) -> bool {
        self.v06
            .as_ref()
            .is_some_and(|v| v.clusters.get(si).is_some_and(|c| c.is_some()))
    }

    /// The leaf ranges of each run of order `si` (one run when unclustered).
    fn layout(&self, v: &V06Quad, si: usize) -> Vec<Leaves> {
        match v.clusters.get(si).and_then(|c| c.as_ref()) {
            Some(c) => c.run_leaves.clone(),
            None => vec![(0, v.orders[si].len())],
        }
    }

    /// The leaves of `run` (leaf range `leaves`) that can hold subject `a`,
    /// from the root alone.
    fn leaves_in(v: &V06Quad, si: usize, leaves: Leaves, a: u32) -> std::ops::Range<usize> {
        let ls = &v.orders[si][leaves.0..leaves.1];
        let lo = ls.partition_point(|l| l.last_g < a);
        let hi = ls.partition_point(|l| l.first_g <= a).max(lo);
        leaves.0 + lo..leaves.0 + hi
    }

    /// The tiles of the (resident) leaves `lis` whose key range can hold subject
    /// `a`, as `(index in the order, entry)`.
    fn candidates(
        v: &V06Quad,
        si: usize,
        lis: std::ops::Range<usize>,
        a: u32,
    ) -> Vec<(usize, QTile)> {
        let mut out = Vec::new();
        for li in lis {
            let Some(cell) = v.leaf_cells[si][li].get() else {
                continue;
            };
            let lo = cell.partition_point(|t| t.last_g < a);
            let hi = cell.partition_point(|t| t.first_g <= a).max(lo);
            for (k, t) in cell[lo..hi].iter().enumerate() {
                out.push((v.orders[si][li].first_tile + lo + k, *t));
            }
        }
        out
    }

    /// The directory of the tiles of `run` that can hold subject `a`: `(tiles,
    /// index of the first in the whole order)`. Faults the leaves it needs.
    pub(super) fn dir_in_run(
        &self,
        si: usize,
        run: usize,
        a: u32,
    ) -> Option<(Arc<[QTile]>, usize)> {
        let v = self.v06.as_ref()?;
        let layout = self.layout(v, si);
        let lis = Self::leaves_in(v, si, *layout.get(run)?, a);
        if lis.is_empty() {
            return Some((Arc::from(Vec::new()), 0));
        }
        let want: Vec<usize> = lis.clone().collect();
        self.fault_leaves(v, si, &want)?;
        let base = v.orders[si][lis.start].first_tile;
        if want.len() == 1 {
            return Some((v.leaf_cells[si][lis.start].get()?.clone(), base));
        }
        let mut out = Vec::new();
        for li in lis {
            out.extend_from_slice(v.leaf_cells[si][li].get()?);
        }
        Some((Arc::from(out), base))
    }

    /// Does the tile (index `ti`, entry `t`) of order `si` hold rows of subject
    /// `a`? Reads it through the cache.
    fn tile_holds(&self, si: usize, ti: usize, t: &QTile, a: u32) -> bool {
        let Some(entry) = self.tile(si, std::slice::from_ref(t), ti, ti) else {
            return false;
        };
        let mut rows: Vec<Row4> = Vec::new();
        let scol = quad7::subject_col(quad7::perm_of(si));
        if quad7::decode_tile(entry.body(), scol, &mut rows).is_none() {
            self.fail();
            return false;
        }
        let lo = rows.partition_point(|r| r.0 < a);
        rows.get(lo).is_some_and(|r| r.0 == a)
    }

    /// For each of `subjects`, the one of `runs` that holds it: the leaves of
    /// every run in one batch, then every candidate tile in one batch, then the
    /// tiles are checked. `None` on a failed read.
    fn find_in_runs(
        &self,
        v: &V06Quad,
        si: usize,
        layout: &[Leaves],
        subjects: &[u32],
        runs: &[usize],
    ) -> Option<Vec<Option<usize>>> {
        let mut lis: BTreeSet<usize> = BTreeSet::new();
        for &a in subjects {
            for &r in runs {
                lis.extend(Self::leaves_in(v, si, layout[r], a));
            }
        }
        let lis: Vec<usize> = lis.into_iter().collect();
        self.fault_leaves(v, si, &lis)?;
        let cands: Vec<RunTiles> = subjects
            .iter()
            .map(|&a| {
                runs.iter()
                    .map(|&r| {
                        (
                            r,
                            Self::candidates(v, si, Self::leaves_in(v, si, layout[r], a), a),
                        )
                    })
                    .filter(|(_, c)| !c.is_empty())
                    .collect()
            })
            .collect();
        let mut missing: BTreeMap<usize, QTile> = BTreeMap::new();
        for (ti, t) in cands.iter().flatten().flat_map(|(_, c)| c.iter()) {
            if !self.tiles.contains((si as u8, *ti as u32)) {
                missing.insert(*ti, *t);
            }
        }
        if !missing.is_empty() {
            let tiles: Vec<(usize, QTile)> = missing.into_iter().collect();
            self.fault_tiles_with(si, &tiles, true)?;
        }
        Some(
            subjects
                .iter()
                .zip(&cands)
                .map(|(&a, per_run)| {
                    per_run
                        .iter()
                        .find(|(_, c)| c.iter().any(|(ti, t)| self.tile_holds(si, *ti, t, a)))
                        .map(|&(r, _)| r)
                })
                .collect(),
        )
    }

    /// The run of clustered order `si` that holds subject `a` (`Some(None)`:
    /// none does; `None`: a read failed). The run the previous subject was found
    /// in is tried alone first, a subject it does not hold is looked for in all
    /// the others at once.
    pub(super) fn resolve_run(&self, si: usize, a: u32) -> Option<Option<usize>> {
        let v = self.v06.as_ref()?;
        let cl = v.clusters.get(si)?.as_ref()?;
        if let Some(known) = cl.runs.recall(a) {
            return Some(known);
        }
        let layout = self.layout(v, si);
        let n = layout.len();
        let hint = cl.runs.hint.load(Ordering::Relaxed).min(n - 1);
        let mut found = self.find_in_runs(v, si, &layout, &[a], &[hint])?[0];
        if found.is_none() && n > 1 {
            let rest: Vec<usize> = (0..n).filter(|&r| r != hint).collect();
            found = self.find_in_runs(v, si, &layout, &[a], &rest)?[0];
        }
        cl.runs.remember(a, found);
        Some(found)
    }

    /// The rows of each of `subjects` (ascending, distinct) in order `si`, as
    /// `(s, p, o, g)`, and the run each was found in. Tiles are read in bounded
    /// batches and dropped once their rows are taken (a window of subjects of a
    /// big class does not fill the tile cache). `hint` is the run asked first.
    fn subject_rows_in(&self, si: usize, subjects: &[u32], hint: usize) -> Option<FoundRows> {
        let v = self.v06.as_ref()?;
        let layout = self.layout(v, si);
        let n = layout.len();
        let hint = hint.min(n.saturating_sub(1));
        let mut out: Vec<Vec<Row4>> = vec![Vec::new(); subjects.len()];
        let mut found: Vec<Option<usize>> = vec![None; subjects.len()];
        let rounds: Vec<Vec<usize>> = if n <= 1 {
            vec![vec![0]]
        } else {
            vec![vec![hint], (0..n).filter(|&r| r != hint).collect()]
        };
        let scol = quad7::subject_col(quad7::perm_of(si));
        let mut pending: Vec<usize> = (0..subjects.len()).collect();
        for runs in rounds {
            if pending.is_empty() {
                break;
            }
            let mut lis: BTreeSet<usize> = BTreeSet::new();
            for &i in &pending {
                for &r in &runs {
                    lis.extend(Self::leaves_in(v, si, layout[r], subjects[i]));
                }
            }
            let lis: Vec<usize> = lis.into_iter().collect();
            self.fault_leaves(v, si, &lis)?;
            // tile -> (entry, [(subject index, run)])
            type Want = (QTile, Vec<(usize, usize)>);
            let mut by_tile: BTreeMap<usize, Want> = BTreeMap::new();
            for &i in &pending {
                for &r in &runs {
                    let ls = Self::leaves_in(v, si, layout[r], subjects[i]);
                    for (ti, t) in Self::candidates(v, si, ls, subjects[i]) {
                        by_tile.entry(ti).or_insert((t, Vec::new())).1.push((i, r));
                    }
                }
            }
            let tiles: Vec<(usize, Want)> = by_tile.into_iter().collect();
            for chunk in tiles.chunks(PROBE_CHUNK) {
                let bodies = self.read_bodies(si, chunk.iter().map(|(ti, (t, _))| (*ti, *t)))?;
                let decoded = decode_bodies(&bodies, scol);
                for ((_, (_, wanted)), rows) in chunk.iter().zip(decoded) {
                    let Some(rows) = rows else {
                        self.fail();
                        return None;
                    };
                    for &(i, r) in wanted {
                        let a = subjects[i];
                        let lo = rows.partition_point(|x| x.0 < a);
                        let hi = lo + rows[lo..].partition_point(|x| x.0 == a);
                        if hi > lo {
                            out[i].extend_from_slice(&rows[lo..hi]);
                            found[i] = Some(r);
                        }
                    }
                }
            }
            pending.retain(|&i| found[i].is_none());
        }
        if let Some(cl) = v.clusters.get(si).and_then(|c| c.as_ref()) {
            let mut counts = vec![0usize; n];
            for r in found.iter().flatten() {
                counts[*r] += 1;
            }
            if let Some((best, &c)) = counts.iter().enumerate().max_by_key(|(_, c)| **c) {
                if c > 0 {
                    cl.runs.hint.store(best, Ordering::Relaxed);
                }
            }
        }
        Some((out, found))
    }

    /// The decompressed bodies of `tiles` (ascending), from the cache or read
    /// in one coalesced batch; not added to the cache.
    fn read_bodies(
        &self,
        si: usize,
        tiles: impl Iterator<Item = (usize, QTile)>,
    ) -> Option<Vec<Arc<[u8]>>> {
        let tiles: Vec<(usize, QTile)> = tiles.collect();
        let mut bodies: Vec<Option<Arc<[u8]>>> = vec![None; tiles.len()];
        let mut spans: Vec<(u64, u64)> = Vec::new();
        // The tiles of each span, `(position in `tiles`)`.
        let mut members: Vec<Vec<usize>> = Vec::new();
        for (k, (ti, t)) in tiles.iter().enumerate() {
            if let Some(e) = self.tiles.get((si as u8, *ti as u32)) {
                bodies[k] = Some(Arc::from(e.body()));
                continue;
            }
            match spans.last_mut() {
                Some(last) if last.0 + last.1 == t.range.offset => {
                    last.1 += t.range.len;
                    members.last_mut()?.push(k);
                }
                _ => {
                    spans.push((t.range.offset, t.range.len));
                    members.push(vec![k]);
                }
            }
        }
        if !spans.is_empty() {
            let blobs = match self.reader.read_many(&spans) {
                Ok(b) if b.len() == spans.len() => b,
                _ => {
                    self.fail();
                    return None;
                }
            };
            for ((start, _), (blob, ks)) in spans.iter().zip(blobs.iter().zip(&members)) {
                for &k in ks {
                    let t = &tiles[k].1;
                    let lo = (t.range.offset - start) as usize;
                    let stored = blob.get(lo..lo + t.range.len as usize)?;
                    match self.unpack(t, stored) {
                        Some(b) => bodies[k] = Some(b),
                        None => {
                            self.fail();
                            return None;
                        }
                    }
                }
            }
        }
        bodies.into_iter().collect()
    }

    /// The stored bytes a lookup of `wanted` (ascending subjects) reads in the
    /// leaves `leaves` of order `si`, assuming each wanted subject in a leaf's
    /// range is a tile of its own, scaled by `share` (the fraction of the subjects
    /// that live in these leaves).
    fn leaf_estimate(v: &V06Quad, si: usize, leaves: Leaves, wanted: &[u32], share: f64) -> f64 {
        let mut bytes = 0f64;
        for l in &v.orders[si][leaves.0..leaves.1] {
            let k = wanted.partition_point(|&x| x <= l.last_g)
                - wanted.partition_point(|&x| x < l.first_g);
            let touched = (k as f64 * share).min(l.n as f64);
            bytes += touched * v.tile_bytes[si] as f64;
        }
        bytes
    }
}

/// Decode `bodies` (each a SPOG-style tile) in parallel when the `parallel`
/// feature is on; `None` for a tile that does not decode.
fn decode_bodies(bodies: &[Arc<[u8]>], scol: usize) -> Vec<Option<Vec<Row4>>> {
    let one = |b: &Arc<[u8]>| -> Option<Vec<Row4>> {
        let mut rows = Vec::new();
        quad7::decode_tile(b, scol, &mut rows).map(|_| rows)
    };
    #[cfg(feature = "parallel")]
    {
        use rayon::prelude::*;
        bodies.par_iter().map(one).collect()
    }
    #[cfg(not(feature = "parallel"))]
    {
        bodies.iter().map(one).collect()
    }
}

impl QuadNamed {
    /// Can a class-scoped graph-variable query probe SPOG by subject? The file
    /// has the graph-last orders SPOG and POSG (`0x07`) and a `0x06`-style
    /// directory (always, from a file).
    pub(crate) fn has_subject_probe(&self) -> bool {
        let mask = self.core.mask();
        let (spog, posg) = (
            quad7::last_slot(crate::index::IndexPermutation::Spo),
            quad7::last_slot(crate::index::IndexPermutation::Pos),
        );
        self.core.v06.is_some() && mask & (1 << spog) != 0 && mask & (1 << posg) != 0
    }

    /// The rows `(s, p, o, g)` of each of `subjects` (ascending, distinct) in
    /// SPOG, read in one batch of leaves and one of tiles (plus one more for the
    /// subjects the guessed run does not hold). `class` is the class the
    /// subjects were found by: its run is asked first. `None` on a failed read.
    pub(crate) fn subject_rows(
        &self,
        subjects: &[u32],
        class: Option<u32>,
    ) -> Option<Vec<Vec<Row4>>> {
        let si = quad7::last_slot(crate::index::IndexPermutation::Spo);
        let v = self.core.v06.as_ref()?;
        let hint = match (class, v.clusters[si].as_ref()) {
            (Some(c), Some(cl)) => cl
                .runs
                .classes
                .iter()
                .position(|&x| x == c)
                .map_or_else(|| cl.runs.hint.load(Ordering::Relaxed), |i| i + 1),
            _ => 0,
        };
        self.core
            .subject_rows_in(si, subjects, hint)
            .map(|(rows, _)| rows)
    }

    /// Estimated stored bytes of a probe of SPOG for `subjects` (ascending,
    /// distinct) and of the GSPO walk over `graphs` (ascending, distinct), from
    /// the directory the root lists, plus a sample of the subjects when SPOG is
    /// clustered (which run each lives in). `None` when it cannot say.
    pub(crate) fn probe_vs_walk_bytes(
        &self,
        subjects: &[u32],
        graphs: &[u32],
        class: Option<u32>,
    ) -> Option<(u64, u64)> {
        let v = self.core.v06.as_ref()?;
        let spog = quad7::last_slot(crate::index::IndexPermutation::Spo);
        let layout = self.core.layout(v, spog);
        let probe = if v.clusters[spog].is_some() {
            // Which runs do the subjects live in? Look up an evenly spread sample.
            let step = (subjects.len() / SAMPLE_SUBJECTS).max(1);
            let sample: Vec<u32> = subjects.iter().step_by(step).copied().collect();
            let hint = match (class, v.clusters[spog].as_ref()) {
                (Some(c), Some(cl)) => cl
                    .runs
                    .classes
                    .iter()
                    .position(|&x| x == c)
                    .map_or(0, |i| i + 1),
                _ => 0,
            };
            let (_, found) = self.core.subject_rows_in(spog, &sample, hint)?;
            let mut share = vec![0f64; layout.len()];
            for r in found.iter().flatten() {
                share[*r] += 1.0 / sample.len() as f64;
            }
            layout
                .iter()
                .zip(&share)
                .map(|(l, &f)| QuadCore::leaf_estimate(v, spog, *l, subjects, f))
                .sum::<f64>()
        } else {
            layout
                .iter()
                .map(|l| QuadCore::leaf_estimate(v, spog, *l, subjects, 1.0))
                .sum::<f64>()
        };
        let gspo = self.core.layout(v, 0);
        let walk: f64 = gspo
            .iter()
            .map(|l| QuadCore::leaf_estimate(v, 0, *l, graphs, 1.0))
            .sum();
        // The walk also resolves the name of every graph it visits (a dictionary
        // chunk, a request on a lazy file) and opens a view of each: a fixed cost a
        // graph that the byte count of its tiles leaves out.
        let walk = walk + graphs.len() as f64 * GRAPH_VISIT_BYTES;
        Some((probe as u64, walk as u64))
    }
}
