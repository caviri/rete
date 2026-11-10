//! Class clustering of the subject-leading permutations (`0x07`,
//! `--cluster-subjects-by-class`, SPEC.md §6.6).
//!
//! The default graph's SPO (and SOP, when the file stores it) normally sorts
//! let route = |rows by subject id. Ids are lexical, so the subjects of one class are spread
//! over the whole id space and a scan of "every Software record" touches a tile
//! of every few thousand records. A clustered permutation sorts by
//! `(cluster(s), s, p, o)` instead: `cluster(s)` is the rank of the subject's
//! primary class among the most frequent classes (run 0: the rest and the
//! untyped), so one class is one contiguous run of tiles. Ids are not touched;
//! only the tile order is.
//!
//! [`derive`] is the one implementation, shared by every writer like
//! `neworders`: the in-RAM build, the memory-bounded build and
//! `convert` hand the assembler an unclustered SPO, and this reads it, partitions
//! its rows by cluster (a stable partition: each run stays in `(s, p, o)` order,
//! so nothing is sorted), cuts and encodes each run with the ordinary tiler and
//! lays the tiles out past the end of the input as extra address space. The same
//! pass undoes a clustering: it merges the runs back into subject order.
//!
//! The cluster of a subject is one byte (`Vec<u8>`, one entry per subject id:
//! 951 MB for the 951 M subjects of OpenAIRE), filled from the `rdf:type` rows
//! of POS in a first pass.

use std::collections::{BinaryHeap, HashMap};

use super::neworders::{Extras, Sink};
use super::parts::{Parts, PermPart};
use super::{ClusterOrder, TileEnt};
use crate::file::FileError;
use crate::index::{GroupSizer, IndexPermutation};
use crate::reader::RangeReader;
use crate::triples::{TripleBlock, TripleBlockBuilder};
use crate::write_opts::ClusterOpts;

/// Tiles read per batch from one run.
const READ_BATCH: usize = 64;
/// Tiles encoded and compressed per parallel batch.
const ENCODE_BATCH: usize = 256;
/// Up to this many stored bytes of tiles the new runs stay in memory.
const MEM_LIMIT: u64 = 256 << 20;

type Row = (u32, u32, u32);
/// The listed classes and the cluster of every subject id.
type Classified = (Vec<u32>, Vec<u8>);
/// A rewritten permutation: its tiles (with offsets) and the tiles of each run.
type Rewritten = (Vec<(TileEnt, u64)>, Vec<usize>);

fn subject_leading(si: usize) -> bool {
    si == IndexPermutation::Spo.section_index() || si == IndexPermutation::Sop.section_index()
}

/// Does the assembler have clustering work for `parts` under `want`? A file
/// is clustered when `want` is set and none of its permutations is yet; it is
/// un-clustered when `want` is `None` and one is. A clustered input kept under
/// `want` keeps its clusters as they are.
pub(crate) fn needed(parts: &Parts, want: Option<ClusterOpts>) -> bool {
    match want {
        Some(_) => {
            parts.cluster.is_empty()
                && parts
                    .index
                    .iter()
                    .any(|p| subject_leading(p.si) && !p.tiles.is_empty())
        }
        None => !parts.cluster.is_empty(),
    }
}

fn bad(w: &'static str) -> FileError {
    FileError::Container(w)
}

#[cfg(feature = "parallel")]
fn par_map<T: Sync, R: Send>(v: &[T], f: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    use rayon::prelude::*;
    v.par_iter().map(f).collect()
}
#[cfg(not(feature = "parallel"))]
fn par_map<T: Sync, R: Send>(v: &[T], f: impl Fn(&T) -> R + Sync + Send) -> Vec<R> {
    v.iter().map(f).collect()
}

fn decode(codec: u8, stored: &[u8]) -> Result<Vec<Row>, FileError> {
    let body = crate::file::decompress(codec, stored)?;
    let block = TripleBlock::parse(&body).map_err(|_| bad("a permutation tile does not decode"))?;
    let rows = block.triples();
    if rows.len() as u64 != block.zone().count as u64 {
        return Err(bad("a permutation tile does not decode"));
    }
    Ok(rows)
}

/// Read and decode `tiles` (in order) in batches, calling `f` with each tile's
/// rows.
fn for_each_tile(
    codec: u8,
    input: &dyn RangeReader,
    tiles: &[(TileEnt, u64)],
    f: &mut dyn FnMut(Vec<Row>) -> Result<(), FileError>,
) -> Result<(), FileError> {
    for chunk in tiles.chunks(READ_BATCH) {
        let ranges: Vec<(u64, u64)> = chunk.iter().map(|(t, off)| (*off, t.clen)).collect();
        let blobs = input.read_many(&ranges)?;
        for rows in par_map(&blobs, |b| decode(codec, b)) {
            f(rows?)?;
        }
    }
    Ok(())
}

/// The id `rdf:type` has in the predicate section, by a scan of its chunks.
fn type_predicate(parts: &Parts, input: &dyn RangeReader) -> Result<Option<u32>, FileError> {
    let want = crate::file::RDF_TYPE.as_bytes();
    let mut id = 0u32;
    let mut buf: Vec<u8> = Vec::new();
    for (c, off) in &parts.dict[3].chunks {
        let stored = input.read_at(*off, c.clen)?;
        let body = crate::file::decompress(parts.dict_codec, &stored)?;
        let mut pos = 0usize;
        while pos < body.len() {
            let (shared, n1) = crate::varint::read_uvarint(&body[pos..])
                .ok_or(bad("malformed predicate chunk"))?;
            let (suf, n2) = crate::varint::read_uvarint(&body[pos + n1..])
                .ok_or(bad("malformed predicate chunk"))?;
            let start = pos + n1 + n2;
            let end = start
                .checked_add(suf as usize)
                .filter(|&e| e <= body.len())
                .ok_or(bad("malformed predicate chunk"))?;
            if shared as usize > buf.len() {
                return Err(bad("malformed predicate chunk"));
            }
            buf.truncate(shared as usize);
            buf.extend_from_slice(&body[start..end]);
            id += 1;
            if buf == want {
                return Ok(Some(id));
            }
            pos = end;
        }
    }
    Ok(None)
}

/// Call `f(class, subject)` for every `rdf:type` row, from POS.
fn type_rows(
    parts: &Parts,
    input: &dyn RangeReader,
    pid: u32,
    f: &mut dyn FnMut(u32, u32),
) -> Result<(), FileError> {
    let Some(pos) = parts
        .index
        .iter()
        .find(|p| p.si == IndexPermutation::Pos.section_index())
    else {
        return Ok(());
    };
    let tiles: Vec<(TileEnt, u64)> = pos
        .tiles
        .iter()
        .filter(|(t, _)| t.min_a <= pid && pid <= t.max_a)
        .copied()
        .collect();
    for_each_tile(parts.block_codec, input, &tiles, &mut |rows| {
        for (p, o, s) in rows {
            if p == pid {
                f(o, s);
            }
        }
        Ok(())
    })
}

/// The classes that get a run (most subjects first) and the cluster of every
/// subject id (`arr[s]`, 0 = run 0). `None` when no class qualifies.
fn classify(
    parts: &Parts,
    input: &dyn RangeReader,
    opts: ClusterOpts,
) -> Result<Option<Classified>, FileError> {
    let Some(pid) = type_predicate(parts, input)? else {
        return Ok(None);
    };
    let mut counts: HashMap<u32, u64> = HashMap::new();
    type_rows(parts, input, pid, &mut |class, _| {
        *counts.entry(class).or_insert(0) += 1;
    })?;
    let mut ranked: Vec<(u32, u64)> = counts
        .into_iter()
        .filter(|&(_, n)| n >= opts.min_subjects.max(1))
        .collect();
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(opts.max_classes.min(super::MAX_CLUSTERS));
    if ranked.is_empty() {
        return Ok(None);
    }
    let classes: Vec<u32> = ranked.iter().map(|&(c, _)| c).collect();
    let arr = cluster_array(parts, input, pid, &classes)?;
    Ok(Some((classes, arr)))
}

/// The cluster of every subject id (`arr[s]`, 0 = run 0) for the listed
/// `classes` (run `i + 1` is `classes[i]`): one byte a subject. The primary
/// class of a multi-typed subject is its rarest listed class, the highest run
/// number.
fn cluster_array(
    parts: &Parts,
    input: &dyn RangeReader,
    pid: u32,
    classes: &[u32],
) -> Result<Vec<u8>, FileError> {
    let cluster_of: HashMap<u32, u8> = classes
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, (i + 1) as u8))
        .collect();
    let n = parts.dict[0].term_count as usize + parts.dict[1].term_count as usize + 1;
    let mut arr = vec![0u8; n];
    let mut out_of_range = false;
    type_rows(parts, input, pid, &mut |class, s| {
        if let Some(&c) = cluster_of.get(&class) {
            match arr.get_mut(s as usize) {
                Some(x) => *x = (*x).max(c),
                None => out_of_range = true,
            }
        }
    })?;
    if out_of_range {
        return Err(bad("an rdf:type subject id is past the subject sections"));
    }
    Ok(arr)
}

/// One run of the input, read tile by tile.
struct RunCursor<'a> {
    codec: u8,
    input: &'a dyn RangeReader,
    tiles: &'a [(TileEnt, u64)],
    next: usize,
    batch: usize,
    buf: std::vec::IntoIter<Row>,
}

impl<'a> RunCursor<'a> {
    fn pull(&mut self) -> Result<Option<Row>, FileError> {
        loop {
            if let Some(r) = self.buf.next() {
                return Ok(Some(r));
            }
            if self.next >= self.tiles.len() {
                return Ok(None);
            }
            let end = (self.next + self.batch).min(self.tiles.len());
            let chunk = &self.tiles[self.next..end];
            self.next = end;
            let ranges: Vec<(u64, u64)> = chunk.iter().map(|(t, off)| (*off, t.clen)).collect();
            let blobs = self.input.read_many(&ranges)?;
            let mut rows = Vec::new();
            for r in par_map(&blobs, |b| decode(self.codec, b)) {
                rows.extend(r?);
            }
            self.buf = rows.into_iter();
        }
    }
}

/// A tiler cutting like the ordinary writers (`StreamingTiler`, `build_tiles`):
/// whole leading-id groups to the budget, an oversize group cut mid-group.
struct RunTiler {
    budget: usize,
    tile: Vec<Row>,
    tile_size: usize,
    group: Vec<Row>,
    sizer: GroupSizer,
    prev_a: u32,
}

impl RunTiler {
    fn new(budget: usize) -> Self {
        RunTiler {
            budget,
            tile: Vec::new(),
            tile_size: 0,
            group: Vec::new(),
            sizer: GroupSizer::start(0, 0),
            prev_a: 0,
        }
    }

    fn push(&mut self, t: Row, emit: &mut dyn FnMut(Vec<Row>)) {
        if let Some(&(ga, _, _)) = self.group.first() {
            if t.0 != ga {
                self.close_group(emit);
            }
        }
        if self.group.is_empty() {
            self.sizer = GroupSizer::start(t.0, self.prev_a);
        }
        self.group.push(t);
        let gtotal = self.sizer.push(t.1, t.2);
        if gtotal > self.budget {
            self.tile.append(&mut self.group);
            emit(std::mem::take(&mut self.tile));
            self.tile_size = 0;
            self.prev_a = t.0;
        }
    }

    fn close_group(&mut self, emit: &mut dyn FnMut(Vec<Row>)) {
        if self.group.is_empty() {
            return;
        }
        let a = self.group[0].0;
        let gsize = self.sizer.total();
        if !self.tile.is_empty() && self.tile_size + gsize > self.budget {
            emit(std::mem::take(&mut self.tile));
            self.tile_size = 0;
        }
        self.tile_size += gsize;
        self.prev_a = a;
        self.tile.append(&mut self.group);
    }

    fn finish(mut self, emit: &mut dyn FnMut(Vec<Row>)) {
        self.close_group(emit);
        if !self.tile.is_empty() {
            emit(std::mem::take(&mut self.tile));
        }
    }
}

/// One run being written.
struct RunOut {
    sink: Sink,
    ents: Vec<TileEnt>,
    at: u64,
}

fn encode_tile(codec: u8, rows: &Vec<Row>) -> (TileEnt, Vec<u8>) {
    let mut b = TripleBlockBuilder::new();
    for &t in rows {
        b.push(t);
    }
    let bytes = b.build();
    let syn = match TripleBlock::parse(&bytes) {
        Ok(blk) => {
            let z = blk.zone();
            (z.min_b, z.max_b, z.min_c, z.max_c)
        }
        Err(_) => (0, u32::MAX, 0, u32::MAX),
    };
    let comp = crate::file::compress(codec, &bytes);
    (
        TileEnt {
            min_a: rows[0].0,
            max_a: rows[rows.len() - 1].0,
            clen: comp.len() as u64,
            syn,
        },
        comp,
    )
}

fn drain(
    codec: u8,
    pending: &mut Vec<(usize, Vec<Row>)>,
    runs: &mut [RunOut],
) -> Result<(), FileError> {
    let encoded = par_map(pending, |(_, rows)| encode_tile(codec, rows));
    for ((run, _), (ent, comp)) in pending.iter().zip(encoded) {
        let r = &mut runs[*run];
        r.sink.write(&comp)?;
        r.ents.push(ent);
        r.at += comp.len() as u64;
    }
    pending.clear();
    Ok(())
}

/// Rewrite one permutation: its rows in subject order (merged from the runs of
/// its current clustering, if any), partitioned by `arr` into `k + 1` runs.
fn rewrite(
    parts: &Parts,
    input: &dyn RangeReader,
    extras: &mut Extras,
    ps: &PermPart,
    current: Option<&ClusterOrder>,
    arr: Option<&[u8]>,
    k: usize,
) -> Result<Rewritten, FileError> {
    let codec = parts.block_codec;
    // The current runs.
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    match current {
        Some(c) => {
            let mut at = 0usize;
            for &n in &c.run_tiles {
                bounds.push((at, at + n));
                at += n;
            }
        }
        None => bounds.push((0, ps.tiles.len())),
    }
    let multi = bounds.len() > 1;
    let mut cursors: Vec<RunCursor> = bounds
        .iter()
        .map(|&(a, b)| RunCursor {
            codec,
            input,
            tiles: &ps.tiles[a..b],
            next: 0,
            batch: if multi { 16 } else { READ_BATCH * 4 },
            buf: Vec::new().into_iter(),
        })
        .collect();
    let stored: u64 = ps.tiles.iter().map(|(t, _)| t.clen).sum();
    let in_memory = stored <= MEM_LIMIT;
    let tag = format!(
        "cl{}-{:x}-{}",
        ps.si,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let mut runs: Vec<RunOut> = Vec::with_capacity(k + 1);
    for r in 0..=k {
        runs.push(RunOut {
            sink: Sink::new(in_memory, &format!("{tag}-{r}.tiles"))?,
            ents: Vec::new(),
            at: 0,
        });
    }
    let budget = crate::write_opts::tile_budget();
    let mut tilers: Vec<Option<RunTiler>> = (0..=k).map(|_| Some(RunTiler::new(budget))).collect();
    let mut pending: Vec<(usize, Vec<Row>)> = Vec::new();
    let cluster_of = |a: u32| -> usize {
        match arr {
            Some(x) => x.get(a as usize).copied().unwrap_or(0) as usize,
            None => 0,
        }
    };
    let route = |row: Row,
                 tilers: &mut Vec<Option<RunTiler>>,
                 pending: &mut Vec<(usize, Vec<Row>)>,
                 runs: &mut Vec<RunOut>|
     -> Result<(), FileError> {
        let c = cluster_of(row.0);
        if let Some(t) = tilers[c].as_mut() {
            t.push(row, &mut |x| pending.push((c, x)));
        }
        if pending.len() >= ENCODE_BATCH {
            drain(codec, pending, runs)?;
        }
        Ok(())
    };
    if !multi {
        while let Some(row) = cursors[0].pull()? {
            route(row, &mut tilers, &mut pending, &mut runs)?;
        }
    } else {
        // K-way merge of the runs' rows, each ascending.
        let mut heap: BinaryHeap<std::cmp::Reverse<(Row, usize)>> = BinaryHeap::new();
        for (i, c) in cursors.iter_mut().enumerate() {
            if let Some(r) = c.pull()? {
                heap.push(std::cmp::Reverse((r, i)));
            }
        }
        while let Some(std::cmp::Reverse((row, i))) = heap.pop() {
            route(row, &mut tilers, &mut pending, &mut runs)?;
            if let Some(r) = cursors[i].pull()? {
                heap.push(std::cmp::Reverse((r, i)));
            }
        }
    }
    for (c, t) in tilers.iter_mut().enumerate() {
        if let Some(t) = t.take() {
            t.finish(&mut |x| pending.push((c, x)));
        }
    }
    drain(codec, &mut pending, &mut runs)?;
    // Lay the runs out past the end of the input.
    let mut tiles = Vec::new();
    let mut run_tiles = Vec::with_capacity(k + 1);
    for mut r in runs {
        r.sink.flush()?;
        run_tiles.push(r.ents.len());
        let base = extras.push(r.at, r.sink);
        let mut off = base;
        for e in r.ents {
            tiles.push((e, off));
            off += e.clen;
        }
    }
    Ok((tiles, run_tiles))
}

/// Cluster (`want`) or un-cluster (`None`) the subject-leading permutations of
/// `parts`. `Ok(None)` when there is nothing to do (no class qualifies). The
/// returned parts' tile offsets point into the input, then into the
/// [`Extras`].
pub(crate) fn derive(
    parts: &Parts,
    input: &dyn RangeReader,
    want: Option<ClusterOpts>,
) -> Result<Option<(Parts, Extras)>, FileError> {
    let mut extras = Extras::new(input.len());
    let mut out = parts.clone();
    match want {
        Some(opts) => {
            let Some((classes, arr)) = classify(parts, input, opts)? else {
                return Ok(None);
            };
            let mut orders = Vec::new();
            for (i, ps) in parts.index.iter().enumerate() {
                if !subject_leading(ps.si) || ps.tiles.is_empty() {
                    continue;
                }
                let (tiles, run_tiles) = rewrite(
                    parts,
                    input,
                    &mut extras,
                    ps,
                    None,
                    Some(&arr),
                    classes.len(),
                )?;
                out.index[i].tiles = tiles;
                orders.push(ClusterOrder {
                    si: ps.si,
                    classes: classes.clone(),
                    run_tiles,
                });
            }
            out.cluster = orders;
        }
        None => {
            for c in &parts.cluster {
                let Some(i) = parts.index.iter().position(|p| p.si == c.si) else {
                    continue;
                };
                let (tiles, _) =
                    rewrite(parts, input, &mut extras, &parts.index[i], Some(c), None, 0)?;
                out.index[i].tiles = tiles;
            }
            out.cluster = Vec::new();
        }
    }
    Ok(Some((out, extras)))
}

/// Check the clustered permutations of a `0x07` file tile by tile: every tile
/// decodes, ascends, agrees with its directory entry, every row of run `i` is of
/// a subject whose primary class is run `i`'s (from the file's own `rdf:type`
/// rows), and the clustered permutations hold the same number of rows. Returns
/// what is wrong (empty when sound) and how many permutations it checked.
pub fn check_clusters(input: &dyn RangeReader) -> Result<(usize, Vec<String>), FileError> {
    let parts = super::parts::from_v06(input)?;
    if parts.cluster.is_empty() {
        return Ok((0, Vec::new()));
    }
    let mut bad_ = Vec::new();
    let pid = type_predicate(&parts, input)?;
    let mut counts: Vec<(usize, u64)> = Vec::new();
    for c in &parts.cluster {
        let name = ["SPO", "POS", "OSP", "SOP", "PSO", "OPS"][c.si.min(5)];
        let Some(ps) = parts.index.iter().find(|p| p.si == c.si) else {
            bad_.push(format!("{name}: no tiles"));
            continue;
        };
        let arr = match pid {
            Some(pid) => cluster_array(&parts, input, pid, &c.classes)?,
            None => vec![0u8; 1],
        };
        let mut at = 0usize;
        let mut rows_total = 0u64;
        for (run, &n) in c.run_tiles.iter().enumerate() {
            let tiles = &ps.tiles[at..at + n];
            let mut prev: Option<Row> = None;
            for (k, chunk) in tiles.chunks(READ_BATCH).enumerate() {
                let ranges: Vec<(u64, u64)> = chunk.iter().map(|(t, off)| (*off, t.clen)).collect();
                let blobs = input.read_many(&ranges)?;
                for (j, ((ent, _), blob)) in chunk.iter().zip(&blobs).enumerate() {
                    let here = at + k * READ_BATCH + j;
                    let Ok(rows) = decode(parts.block_codec, blob) else {
                        bad_.push(format!("{name} tile {here}: does not decode"));
                        continue;
                    };
                    if rows.is_empty()
                        || rows[0].0 != ent.min_a
                        || rows[rows.len() - 1].0 != ent.max_a
                    {
                        bad_.push(format!(
                            "{name} tile {here}: key range differs from the directory"
                        ));
                    }
                    if rows.windows(2).any(|w| w[0] >= w[1]) || prev.is_some_and(|p| p >= rows[0]) {
                        bad_.push(format!(
                            "{name} tile {here}: rows are not ascending within run {run}"
                        ));
                    }
                    prev = rows.last().copied();
                    let wrong = rows
                        .iter()
                        .find(|r| arr.get(r.0 as usize).copied().unwrap_or(0) as usize != run);
                    if let Some(r) = wrong {
                        bad_.push(format!(
                            "{name} tile {here}: subject {} is in run {run}, its class says run {}",
                            r.0,
                            arr.get(r.0 as usize).copied().unwrap_or(0)
                        ));
                    }
                    rows_total += rows.len() as u64;
                }
            }
            at += n;
        }
        counts.push((c.si, rows_total));
    }
    if let Some(&(_, first)) = counts.first() {
        if counts.iter().any(|&(_, n)| n != first) {
            bad_.push("the clustered permutations hold different numbers of rows".to_string());
        }
    }
    Ok((parts.cluster.len(), bad_))
}
