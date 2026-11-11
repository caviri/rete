//! Derived IRI families, writer side (SPEC.md section 5.3): the pass that finds
//! the subject-only dictionary chunks whose every term derives from the index,
//! and `rete verify`'s check of the ones already dropped.
//!
//! [`derive`] runs inside the assembler, over [`Parts`] and the input they
//! point into, like `cluster::derive`. For each family it walks the subject-only
//! chunks in order, rebuilds each term from the inputs' rows in PSO (or an
//! unclustered SPO) and the object dictionary, and compares the result with the
//! stored text byte for byte. A chunk where every term matches (and splits back
//! into its inputs, which is what the lookup does) loses its body: its stored
//! length becomes 0 and the family's run list records it. Any other chunk is
//! kept whole.
//!
//! Terms are rebuilt in batches of chunks: the objects they name are sorted and
//! read in object-dictionary order, so a batch decodes each object chunk once
//! instead of once per term.

use std::collections::VecDeque;

use super::parts::Parts;
use super::{ChunkEnt, TileEnt};
use crate::derived::{Family, SECTION};
use crate::file::FileError;
use crate::index::IndexPermutation;
use crate::reader::RangeReader;
use crate::triples::TripleBlock;

/// A chunk index and its rebuilt terms, `None` when they do not rebuild.
pub(crate) type Rebuilt = (usize, Option<Vec<Vec<u8>>>);
/// A dictionary chunk and the `(position in the id list, index in the chunk)` of
/// the terms wanted from it: `(section, chunk, wanted)`.
type Group = (usize, usize, Vec<(usize, usize)>);

/// Chunks rebuilt per batch.
const BATCH_CHUNKS: usize = 256;
/// Tiles read per batch from one column.
const READ_TILES: usize = 16;

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

/// The terms of one decoded chunk body, in order.
fn decode_terms(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    walk(body, &mut |_, t| out.push(t.to_vec()))?;
    Some(out)
}

/// Walk the front-coded entries of a chunk body: `f(index, term)`.
fn walk(body: &[u8], f: &mut dyn FnMut(usize, &[u8])) -> Option<()> {
    let mut pos = 0usize;
    let mut buf: Vec<u8> = Vec::new();
    let mut i = 0usize;
    while pos < body.len() {
        let (shared, n1) = crate::varint::read_uvarint(body.get(pos..)?)?;
        let p = pos + n1;
        let (suf, n2) = crate::varint::read_uvarint(body.get(p..)?)?;
        let start = p + n2;
        let end = start.checked_add(suf as usize)?;
        let s = body.get(start..end)?;
        if shared as usize > buf.len() {
            return None;
        }
        buf.truncate(shared as usize);
        buf.extend_from_slice(s);
        f(i, &buf);
        i += 1;
        pos = end;
    }
    Some(())
}

/// How many terms chunk `ci` of a section holds.
pub(crate) fn chunk_terms(chunks: &[(ChunkEnt, u64)], ci: usize, ri: usize, tc: u32) -> usize {
    let first = chunks[ci].0.first_run.saturating_mul(ri);
    let end = match chunks.get(ci + 1) {
        Some(n) => n.0.first_run.saturating_mul(ri).min(tc as usize),
        None => tc as usize,
    };
    end.saturating_sub(first)
}

/// Where the rows of one input predicate are read from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    /// `(p, s, o)`.
    Pso,
    /// `(s, p, o)`, unclustered.
    Spo,
}

/// A forward-only cursor over the `(subject, object)` rows of one predicate, in
/// subject order.
struct Column {
    order: Order,
    pred: u32,
    /// Indices of the permutation's tiles that can hold the predicate.
    tiles: Vec<usize>,
    next: usize,
    queue: VecDeque<(u32, u32)>,
}

impl Column {
    fn new(order: Order, pred: u32, tiles: &[(TileEnt, u64)]) -> Self {
        let tiles = tiles
            .iter()
            .enumerate()
            .filter(|(_, (t, _))| match order {
                Order::Pso => t.min_a <= pred && pred <= t.max_a,
                Order::Spo => true,
            })
            .map(|(i, _)| i)
            .collect();
        Column {
            order,
            pred,
            tiles,
            next: 0,
            queue: VecDeque::new(),
        }
    }

    /// Can the tile be skipped because every subject in it is below `lo`?
    fn below(&self, t: &TileEnt, lo: u32) -> bool {
        match self.order {
            Order::Pso => t.syn.1 != 0 && t.syn.1 < lo,
            Order::Spo => t.max_a < lo,
        }
    }

    /// The rows with a subject in `lo..=hi`, ascending; the cursor moves on to
    /// `lo`, so calls must ask for ascending ranges.
    fn rows(
        &mut self,
        input: &dyn RangeReader,
        codec: u8,
        tiles: &[(TileEnt, u64)],
        lo: u32,
        hi: u32,
    ) -> Result<Vec<(u32, u32)>, FileError> {
        while self.queue.front().is_some_and(|r| r.0 < lo) {
            self.queue.pop_front();
        }
        while self.queue.back().is_none_or(|r| r.0 <= hi) && self.next < self.tiles.len() {
            let mut take = Vec::new();
            while take.len() < READ_TILES && self.next < self.tiles.len() {
                let ti = self.tiles[self.next];
                self.next += 1;
                if !self.below(&tiles[ti].0, lo) {
                    take.push(ti);
                }
            }
            if take.is_empty() {
                continue;
            }
            let ranges: Vec<(u64, u64)> = take
                .iter()
                .map(|&i| (tiles[i].1, tiles[i].0.clen))
                .collect();
            let blobs = input.read_many(&ranges)?;
            let decoded = par_map(&blobs, |b| decode_tile(codec, b));
            for rows in decoded {
                for (a, b, c) in rows? {
                    let (p, s, o) = match self.order {
                        Order::Pso => (a, b, c),
                        Order::Spo => (b, a, c),
                    };
                    if p == self.pred && s >= lo {
                        self.queue.push_back((s, o));
                    }
                }
            }
        }
        let mut out = Vec::new();
        while self.queue.front().is_some_and(|r| r.0 <= hi) {
            out.extend(self.queue.pop_front());
        }
        Ok(out)
    }
}

fn decode_tile(codec: u8, stored: &[u8]) -> Result<Vec<(u32, u32, u32)>, FileError> {
    let body = crate::file::decompress(codec, stored)?;
    let block = TripleBlock::parse(&body).map_err(|_| bad("a permutation tile does not decode"))?;
    let rows = block.triples();
    if rows.len() as u64 != block.zone().count as u64 {
        return Err(bad("a permutation tile does not decode"));
    }
    Ok(rows)
}

/// The predicate terms of the file, in id order.
fn predicate_terms(parts: &Parts, input: &dyn RangeReader) -> Result<Vec<Vec<u8>>, FileError> {
    let mut out = Vec::new();
    for (c, off) in &parts.dict[3].chunks {
        let stored = input.read_at(*off, c.clen)?;
        let body = crate::file::decompress(parts.dict_codec, &stored)?;
        out.extend(decode_terms(&body).ok_or(bad("malformed predicate chunk"))?);
    }
    Ok(out)
}

/// Rebuilds the terms of the subject-only chunks of one family.
pub(crate) struct Deriver<'a> {
    parts: &'a Parts,
    input: &'a dyn RangeReader,
    fam: &'a Family,
    cols: Vec<Column>,
    tiles: &'a [(TileEnt, u64)],
    shared: u32,
}

impl<'a> Deriver<'a> {
    /// A deriver for `fam`, or the reason it cannot be (a predicate the file
    /// lacks, no permutation to read the rows from).
    pub(crate) fn new(
        parts: &'a Parts,
        input: &'a dyn RangeReader,
        fam: &'a Family,
    ) -> Result<Result<Self, String>, FileError> {
        let terms = predicate_terms(parts, input)?;
        let mut preds = Vec::new();
        for i in &fam.inputs {
            let want = format!("<{}>", i.predicate);
            match terms.iter().position(|t| t == want.as_bytes()) {
                Some(p) => preds.push(p as u32 + 1),
                None => return Ok(Err(format!("the file has no predicate <{}>", i.predicate))),
            }
        }
        let pso = IndexPermutation::Pso.section_index();
        let spo = IndexPermutation::Spo.section_index();
        let find = |si: usize| parts.index.iter().find(|p| p.si == si);
        let (order, perm) = match (find(pso), find(spo)) {
            (Some(p), _) => (Order::Pso, p),
            (None, Some(p)) if !parts.cluster.iter().any(|c| c.si == spo) => (Order::Spo, p),
            _ => {
                return Ok(Err(
                    "the file has neither PSO nor an unclustered SPO to read the inputs from"
                        .to_string(),
                ))
            }
        };
        let cols = preds
            .iter()
            .map(|&p| Column::new(order, p, &perm.tiles))
            .collect();
        Ok(Ok(Deriver {
            parts,
            input,
            fam,
            cols,
            tiles: &perm.tiles,
            shared: parts.dict[0].term_count,
        }))
    }

    /// The text of the objects `ids` (sorted, distinct), read in dictionary
    /// order; `None` where an id is out of range or a chunk is unreadable.
    fn objects(&self, ids: &[u32]) -> Result<Vec<Option<Vec<u8>>>, FileError> {
        let d = self.parts;
        // (section, chunk) -> the wanted `(position in ids, index in chunk)`.
        let mut groups: Vec<Group> = Vec::new();
        for (pos, &id) in ids.iter().enumerate() {
            let (si, local) = if id <= self.shared {
                (0usize, id)
            } else {
                (2usize, id - self.shared)
            };
            let sec = &d.dict[si];
            if local == 0 || local > sec.term_count || sec.restart_interval == 0 {
                continue;
            }
            let ri = sec.restart_interval as usize;
            let ci = sec
                .chunks
                .partition_point(|c| c.0.first_run * ri < local as usize)
                .saturating_sub(1);
            let Some(c) = sec.chunks.get(ci) else {
                continue;
            };
            let idx = (local as usize - 1).saturating_sub(c.0.first_run * ri);
            match groups.last_mut() {
                Some(g) if g.0 == si && g.1 == ci => g.2.push((pos, idx)),
                _ => groups.push((si, ci, vec![(pos, idx)])),
            }
        }
        let ranges: Vec<(u64, u64)> = groups
            .iter()
            .map(|g| {
                let (c, off) = &d.dict[g.0].chunks[g.1];
                (*off, c.clen)
            })
            .collect();
        let blobs = self.input.read_many(&ranges)?;
        let picked = par_map(&blobs.iter().zip(&groups).collect::<Vec<_>>(), |(b, g)| {
            let body = crate::file::decompress(d.dict_codec, b).ok()?;
            let mut want = g.2.iter().peekable();
            let mut got: Vec<(usize, Vec<u8>)> = Vec::with_capacity(g.2.len());
            walk(&body, &mut |i, t| {
                while let Some(&&(pos, idx)) = want.peek() {
                    if idx == i {
                        got.push((pos, t.to_vec()));
                        want.next();
                    } else if idx < i {
                        want.next();
                    } else {
                        break;
                    }
                }
            })?;
            Some(got)
        });
        let mut out: Vec<Option<Vec<u8>>> = vec![None; ids.len()];
        for got in picked.into_iter().flatten() {
            for (pos, t) in got {
                out[pos] = Some(t);
            }
        }
        Ok(out)
    }

    /// Rebuild the terms of subject-only chunks `cis` (ascending): per chunk the
    /// terms in id order, or `None` when some term of it has no single object
    /// for an input, does not sit under its prefix, or does not split back.
    pub(crate) fn terms(&mut self, cis: &[usize]) -> Result<Vec<Option<Vec<Vec<u8>>>>, FileError> {
        let sec = &self.parts.dict[SECTION];
        let ri = sec.restart_interval as usize;
        let n = self.fam.inputs.len();
        // The subject id range of every chunk, and the span of the batch.
        let spans: Vec<(u32, usize)> = cis
            .iter()
            .map(|&ci| {
                let first = sec.chunks[ci].0.first_run.saturating_mul(ri);
                (
                    self.shared.saturating_add(first as u32 + 1),
                    chunk_terms(&sec.chunks, ci, ri, sec.term_count),
                )
            })
            .collect();
        let (Some(&(lo, _)), Some(&(last, cnt))) = (spans.first(), spans.last()) else {
            return Ok(Vec::new());
        };
        let hi = last.saturating_add(cnt.saturating_sub(1) as u32);
        let codec = self.parts.block_codec;
        let mut rows = Vec::with_capacity(n);
        for c in self.cols.iter_mut() {
            rows.push(c.rows(self.input, codec, self.tiles, lo, hi)?);
        }
        // Per chunk, the object ids of each term (or the chunk fails).
        let mut ids: Vec<Option<Vec<u32>>> = Vec::with_capacity(cis.len());
        let mut at = vec![0usize; n];
        for &(first, cnt) in &spans {
            let mut chunk: Option<Vec<u32>> = Some(Vec::with_capacity(cnt * n));
            for i in 0..cnt {
                let s = first + i as u32;
                for k in 0..n {
                    while at[k] < rows[k].len() && rows[k][at[k]].0 < s {
                        at[k] += 1;
                    }
                    let r = rows[k].get(at[k]);
                    let dup = rows[k].get(at[k] + 1).is_some_and(|x| x.0 == s);
                    match (r, dup, chunk.as_mut()) {
                        (Some(&(rs, o)), false, Some(v)) if rs == s => v.push(o),
                        _ => chunk = None,
                    }
                }
                if chunk.is_none() {
                    break;
                }
            }
            ids.push(chunk);
        }
        let mut wanted: Vec<u32> = ids.iter().flatten().flatten().copied().collect();
        wanted.sort_unstable();
        wanted.dedup();
        let texts = self.objects(&wanted)?;
        let lookup = |o: u32| -> Option<&[u8]> {
            let i = wanted.binary_search(&o).ok()?;
            texts[i].as_deref()
        };
        let mut out = Vec::with_capacity(cis.len());
        for chunk in &ids {
            let built = chunk.as_ref().and_then(|v| {
                let mut terms = Vec::with_capacity(v.len() / n);
                for t in v.chunks(n) {
                    let objs: Vec<&str> = t
                        .iter()
                        .map(|&o| std::str::from_utf8(lookup(o)?).ok())
                        .collect::<Option<_>>()?;
                    let mut values = Vec::with_capacity(n);
                    for (k, o) in objs.iter().enumerate() {
                        values.push(self.fam.value_of(k, o)?);
                    }
                    let term = self.fam.build(&values);
                    if self.fam.split(&term).as_deref() != Some(values.as_slice()) {
                        return None;
                    }
                    terms.push(term.into_bytes());
                }
                Some(terms)
            });
            out.push(built);
        }
        Ok(out)
    }
}

/// Does the assembler have derivation work for `parts` under `want`?
pub(crate) fn needed(parts: &Parts, want: &[Family]) -> bool {
    !want.is_empty()
        && parts.derived.is_empty()
        && parts.dict[SECTION].chunks.iter().any(|(c, _)| c.clen > 0)
}

/// `parts` with the derivable chunks of the subject-only section dropped and the
/// families recorded, or `None` when nothing derives. Notes about families that
/// cannot be used go to stderr.
pub(crate) fn derive(
    parts: &Parts,
    input: &dyn RangeReader,
    want: &[Family],
) -> Result<Option<Parts>, FileError> {
    let mut out = parts.clone();
    let mut claimed = vec![false; parts.dict[SECTION].chunks.len()];
    let mut fams: Vec<Family> = Vec::new();
    for spec in want {
        let mut fam = Family {
            runs: Vec::new(),
            ..spec.clone()
        };
        fam.check()
            .map_err(|_| bad("derive-iris: malformed family"))?;
        let mut d = match Deriver::new(parts, input, &fam)? {
            Ok(d) => d,
            Err(why) => {
                eprintln!("derive-iris: {}: {why}", spec.spec());
                continue;
            }
        };
        let sec = &parts.dict[SECTION];
        let ri = sec.restart_interval as usize;
        let todo: Vec<usize> = (0..sec.chunks.len()).filter(|&ci| !claimed[ci]).collect();
        let mut won: Vec<usize> = Vec::new();
        let mut seen = 0usize;
        for batch in todo.chunks(BATCH_CHUNKS) {
            let derived = d.terms(batch)?;
            let stored_ranges: Vec<(u64, u64)> = batch
                .iter()
                .map(|&ci| (sec.chunks[ci].1, sec.chunks[ci].0.clen))
                .collect();
            let blobs = input.read_many(&stored_ranges)?;
            let stored = par_map(&blobs, |b| {
                crate::file::decompress(parts.dict_codec, b)
                    .ok()
                    .and_then(|body| decode_terms(&body))
            });
            for ((&ci, derived), stored) in batch.iter().zip(derived).zip(stored) {
                seen += 1;
                let n = chunk_terms(&sec.chunks, ci, ri, sec.term_count);
                if let (Some(a), Some(b)) = (derived, stored) {
                    if a.len() == n && a == b {
                        won.push(ci);
                    }
                }
            }
        }
        eprintln!(
            "derive-iris: {}: {} of {} subject-only chunks derive",
            spec.spec(),
            won.len(),
            seen
        );
        for &ci in &won {
            claimed[ci] = true;
            out.dict[SECTION].chunks[ci].0.clen = 0;
        }
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for &ci in &won {
            match runs.last_mut() {
                Some(r) if r.0 + r.1 == ci => r.1 += 1,
                _ => runs.push((ci, 1)),
            }
        }
        if !runs.is_empty() {
            fam.runs = runs;
            fams.push(fam);
        }
    }
    if fams.is_empty() {
        return Ok(None);
    }
    out.derived = fams;
    Ok(Some(out))
}

/// `rete verify`'s side of a derived file: rebuilds the terms of every dropped
/// chunk, in ascending chunk order, a batch at a time.
pub(crate) struct Walk<'a> {
    derivers: Vec<Option<Deriver<'a>>>,
    /// `(chunk, family)`, ascending.
    todo: Vec<(usize, usize)>,
    at: usize,
    /// What is wrong with the families themselves.
    pub(crate) problems: Vec<String>,
}

impl<'a> Walk<'a> {
    pub(crate) fn new(parts: &'a Parts, input: &'a dyn RangeReader) -> Result<Self, FileError> {
        let mut problems = Vec::new();
        let mut derivers = Vec::new();
        let mut todo: Vec<(usize, usize)> = Vec::new();
        for (fi, fam) in parts.derived.iter().enumerate() {
            match Deriver::new(parts, input, fam)? {
                Ok(d) => derivers.push(Some(d)),
                Err(why) => {
                    problems.push(format!("derived family {fi}: {why}"));
                    derivers.push(None);
                }
            }
            for &(first, n) in &fam.runs {
                todo.extend((first..first + n).map(|ci| (ci, fi)));
            }
        }
        todo.sort_unstable();
        Ok(Walk {
            derivers,
            todo,
            at: 0,
            problems,
        })
    }

    /// The next chunks' terms (`None` for a chunk that does not rebuild), or an
    /// empty batch when done.
    pub(crate) fn batch(&mut self) -> Result<Vec<Rebuilt>, FileError> {
        let end = (self.at + BATCH_CHUNKS).min(self.todo.len());
        let slice = self.todo[self.at..end].to_vec();
        self.at = end;
        let mut out: Vec<Rebuilt> = Vec::with_capacity(slice.len());
        let mut fi = 0;
        while fi < self.derivers.len() {
            let cis: Vec<usize> = slice.iter().filter(|s| s.1 == fi).map(|s| s.0).collect();
            if !cis.is_empty() {
                match self.derivers[fi].as_mut() {
                    Some(d) => out.extend(cis.iter().copied().zip(d.terms(&cis)?)),
                    None => out.extend(cis.iter().map(|&c| (c, None))),
                }
            }
            fi += 1;
        }
        out.sort_by_key(|o| o.0);
        Ok(out)
    }
}
