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
use crate::derived::{DeriveOpts, Family, Input, SECTION};
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

/// The text of the objects `ids` (sorted, distinct), read in dictionary order;
/// `None` where an id is out of range or a chunk is unreadable.
fn object_texts(
    d: &Parts,
    input: &dyn RangeReader,
    ids: &[u32],
) -> Result<Vec<Option<Vec<u8>>>, FileError> {
    // (section, chunk) -> the wanted `(position in ids, index in chunk)`.
    let mut groups: Vec<Group> = Vec::new();
    for (pos, &id) in ids.iter().enumerate() {
        let (si, local) = if id <= d.dict[0].term_count {
            (0usize, id)
        } else {
            (2usize, id - d.dict[0].term_count)
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
    let blobs = input.read_many(&ranges)?;
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

    /// The text of the objects `ids` (sorted, distinct).
    fn objects(&self, ids: &[u32]) -> Result<Vec<Option<Vec<u8>>>, FileError> {
        object_texts(self.parts, self.input, ids)
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

/// Does the assembler have derivation work for `parts`? Only a file whose
/// subject-only dictionary is stored, and not derived yet.
pub(crate) fn needed(parts: &Parts) -> bool {
    parts.derived.is_empty() && parts.dict[SECTION].chunks.iter().any(|(c, _)| c.clen > 0)
}

// ---------------------------------------------------------------------------
// Detection. The writer finds the families itself, from the data:
//
// 1. A deterministic sample of `WINDOWS` runs of `WINDOW_TERMS` consecutive
//    subject-only ids, spread over the section (a golden-ratio sequence, so any
//    prefix of the windows is spread too).
// 2. For each sampled subject: its rows (from PSO, or an unclustered SPO), the
//    single-valued predicates among them, and the text of those objects that
//    are IRIs. The longest suffix of such an object's IRI that occurs inside the
//    subject's own IRI is that predicate's hole; the hole is located and the
//    rest of the subject's IRI is the literal pieces. So every subject has a
//    signature: the literals, and per hole the predicate and the constant
//    prefix its object IRI has before the hole.
// 3. A window fits a signature when all its subjects have it. A signature
//    whose share of windows, times the section's stored bytes, is worth the
//    threshold becomes a candidate.
// 4. The candidate is verified over the whole section ([`Deriver`]): every term
//    is rebuilt and compared byte for byte, and a chunk with one term that does
//    not fit stays stored. The family is kept only if the chunks that derive
//    are worth the threshold too.
// ---------------------------------------------------------------------------

/// Windows sampled.
const WINDOWS: usize = 64;
/// Consecutive subject-only ids per window.
const WINDOW_TERMS: usize = 16;
/// Most index tiles the sample reads.
const SAMPLE_TILES: usize = 1536;
/// Shortest hole, in bytes.
const MIN_HOLE: usize = 2;
/// Candidate families verified at most.
const MAX_CANDIDATES: usize = 4;
/// Longest object IRI looked at.
const MAX_OBJECT: usize = 512;
/// Entries and text kept in a report.
const REPORT_ENTRIES: usize = 4;
const REPORT_TEXT: usize = 400;

/// One subject's shape: the literal pieces of its IRI (`<` and `>` included) and,
/// per hole, the predicate id and the constant prefix of the object's IRI.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Sig {
    literals: Vec<String>,
    holes: Vec<(u32, String)>,
}

/// A candidate family and how much of the sample fits it.
struct Candidate {
    fam: Family,
    fit: usize,
    windows: usize,
}

/// What detection did and why, for the build record and stderr.
#[derive(Default)]
pub(crate) struct Notes {
    taken: Vec<serde_json::Value>,
    rejected: Vec<serde_json::Value>,
    windows: usize,
    subjects: usize,
    sampled: bool,
}

fn clip(s: &str) -> String {
    if s.len() <= REPORT_TEXT {
        return s.to_string();
    }
    let mut end = REPORT_TEXT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &s[..end])
}

impl Notes {
    fn reject(&mut self, template: Option<&str>, why: String) {
        eprintln!(
            "derive-iris: not taken{}: {why}",
            template.map(|t| format!(" ({t})")).unwrap_or_default()
        );
        if self.rejected.len() < REPORT_ENTRIES {
            let mut o = serde_json::Map::new();
            if let Some(t) = template {
                o.insert("template".into(), clip(t).into());
            }
            o.insert("why".into(), clip(&why).into());
            self.rejected.push(o.into());
        }
    }

    /// The record's `derive_iris` value, or `None` when nothing was examined.
    fn report(&self, opts: &DeriveOpts, file: u64, section: u64) -> Option<serde_json::Value> {
        if !self.sampled && self.taken.is_empty() {
            return None;
        }
        let mut o = serde_json::Map::new();
        o.insert(
            "rule".into(),
            format!(
                "a family is taken when it removes at least {}.{} % of the file and {} bytes",
                opts.min_permille / 10,
                opts.min_permille % 10,
                opts.min_bytes
            )
            .into(),
        );
        o.insert("file_bytes".into(), file.into());
        o.insert("subject_only_dictionary_bytes".into(), section.into());
        o.insert("sampled_windows".into(), self.windows.into());
        o.insert("sampled_subjects".into(), self.subjects.into());
        o.insert("taken".into(), self.taken.clone().into());
        o.insert("rejected".into(), self.rejected.clone().into());
        Some(o.into())
    }
}

/// `n` terms of dictionary section `si`, from local id `first` (1-based).
fn section_terms(
    parts: &Parts,
    input: &dyn RangeReader,
    si: usize,
    first: usize,
    n: usize,
) -> Result<Option<Vec<Vec<u8>>>, FileError> {
    let sec = &parts.dict[si];
    let ri = sec.restart_interval as usize;
    if first == 0 || n == 0 || ri == 0 || first + n - 1 > sec.term_count as usize {
        return Ok(None);
    }
    let chunk_of = |local: usize| {
        sec.chunks
            .partition_point(|c| c.0.first_run * ri < local)
            .saturating_sub(1)
    };
    let (c0, c1) = (chunk_of(first), chunk_of(first + n - 1));
    if sec.chunks[c0..=c1].iter().any(|c| c.0.clen == 0) {
        return Ok(None);
    }
    let ranges: Vec<(u64, u64)> = sec.chunks[c0..=c1]
        .iter()
        .map(|(c, off)| (*off, c.clen))
        .collect();
    let blobs = input.read_many(&ranges)?;
    let mut all: Vec<Vec<u8>> = Vec::new();
    for b in &blobs {
        let body = crate::file::decompress(parts.dict_codec, b)?;
        match decode_terms(&body) {
            Some(t) => all.extend(t),
            None => return Ok(None),
        }
    }
    let base = sec.chunks[c0].0.first_run * ri;
    let at = first - 1 - base;
    Ok(all.get(at..at + n).map(<[Vec<u8>]>::to_vec))
}

/// The `(subject, predicate, object)` rows of the subjects in each window
/// `(lo, hi)`, from the permutation's tiles. Windows are taken in order until
/// the sample would read more than [`SAMPLE_TILES`] tiles; the rows are those
/// of the windows used.
fn sample_rows(
    input: &dyn RangeReader,
    codec: u8,
    order: Order,
    tiles: &[(TileEnt, u64)],
    wins: &[(u32, u32)],
) -> Result<Vec<Vec<Row>>, FileError> {
    use std::collections::BTreeMap;
    let mut want: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    let mut used = 0usize;
    for (w, &(lo, hi)) in wins.iter().enumerate() {
        let hit: Vec<usize> = tiles
            .iter()
            .enumerate()
            .filter(|(_, (t, _))| match order {
                Order::Spo => t.min_a <= hi && t.max_a >= lo,
                Order::Pso => t.syn.0 <= hi && t.syn.1 >= lo,
            })
            .map(|(i, _)| i)
            .collect();
        let new = hit.iter().filter(|i| !want.contains_key(i)).count();
        if used > 0 && want.len() + new > SAMPLE_TILES {
            break;
        }
        for i in hit {
            want.entry(i).or_default().push(w);
        }
        used = w + 1;
    }
    let mut out: Vec<Vec<Row>> = vec![Vec::new(); used];
    let ids: Vec<usize> = want.keys().copied().collect();
    for batch in ids.chunks(64) {
        let ranges: Vec<(u64, u64)> = batch
            .iter()
            .map(|&i| (tiles[i].1, tiles[i].0.clen))
            .collect();
        let blobs = input.read_many(&ranges)?;
        let decoded = par_map(&blobs, |b| decode_tile(codec, b));
        for (&ti, rows) in batch.iter().zip(decoded) {
            let rows = rows?;
            for &w in &want[&ti] {
                let (lo, hi) = wins[w];
                for &(a, b, c) in &rows {
                    let (s, p, o) = match order {
                        Order::Spo => (a, b, c),
                        Order::Pso => (b, a, c),
                    };
                    if (lo..=hi).contains(&s) {
                        out[w].push((s, p, o));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// A stopwatch for the log lines (a browser build has no clock to read).
struct Clock(#[cfg(not(target_arch = "wasm32"))] std::time::Instant);

impl Clock {
    fn start() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        return Clock(std::time::Instant::now());
        #[cfg(target_arch = "wasm32")]
        Clock()
    }

    fn elapsed(&self) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        return format!("{:.2} s", self.0.elapsed().as_secs_f64());
        #[cfg(target_arch = "wasm32")]
        String::from("n/a")
    }
}

/// A `(subject, predicate, object)` row.
type Row = (u32, u32, u32);

/// One located hole: offset and length in the subject's text, offset in the
/// object's text where the hole starts, and the predicate.
type Found = (usize, usize, usize, u32);

/// The shape of subject IRI `inner` (no `<>`) given the single-valued objects
/// `cands` it has, `(predicate id, object IRI without <>)`; `None` when no
/// object's text occurs in it.
fn signature(inner: &str, cands: &[(u32, &str)]) -> Option<Sig> {
    // For each candidate, the longest suffix of the object's IRI found in the
    // subject's.
    let mut found: Vec<Found> = Vec::new();
    for &(p, u) in cands {
        if u.len() > MAX_OBJECT {
            continue;
        }
        for (i, _) in u.char_indices() {
            let suffix = &u[i..];
            if suffix.len() < MIN_HOLE {
                break;
            }
            if let Some(at) = inner.find(suffix) {
                found.push((at, suffix.len(), i, p));
                break;
            }
        }
    }
    // Longest first, then lowest predicate; keep the ones that do not overlap.
    found.sort_by(|a, b| b.1.cmp(&a.1).then(a.3.cmp(&b.3)));
    let mut chosen: Vec<Found> = Vec::new();
    for f in found {
        let (s, l, _, _) = f;
        if chosen.iter().all(|c| s + l <= c.0 || c.0 + c.1 <= s) {
            chosen.push(f);
        }
    }
    if chosen.is_empty() {
        return None;
    }
    chosen.sort_unstable();
    // A hole starts at its object's first character that is not punctuation:
    // the separator in front of an id belongs to the literal and to the prefix.
    let objects: std::collections::HashMap<u32, &str> = cands.iter().copied().collect();
    let mut holes: Vec<(usize, usize, u32, String)> = Vec::new();
    for (mut s, mut l, mut i, p) in chosen {
        let u = objects[&p];
        while l > 1 && u.as_bytes()[i].is_ascii_punctuation() {
            s += 1;
            l -= 1;
            i += 1;
        }
        if !u.is_char_boundary(i) || !inner.is_char_boundary(s) {
            return None;
        }
        holes.push((s, l, p, u[..i].to_string()));
    }
    let mut literals = Vec::with_capacity(holes.len() + 1);
    let mut at = 0usize;
    for (s, l, _, _) in &holes {
        if *s < at {
            return None;
        }
        literals.push(inner[at..*s].to_string());
        at = s + l;
    }
    literals.push(inner[at..].to_string());
    literals[0].insert(0, '<');
    let last = literals.len() - 1;
    literals[last].push('>');
    let n = holes.len();
    if literals[0].len() < 2 || literals[1..n].iter().any(String::is_empty) {
        return None;
    }
    Some(Sig {
        literals,
        holes: holes.into_iter().map(|(_, _, p, pre)| (p, pre)).collect(),
    })
}

/// The families the sample suggests, best first; what is not taken is noted.
fn discover(
    parts: &Parts,
    input: &dyn RangeReader,
    opts: &DeriveOpts,
    file: u64,
    notes: &mut Notes,
) -> Result<Vec<Candidate>, FileError> {
    let sec = &parts.dict[SECTION];
    let section: u64 = sec.chunks.iter().map(|c| c.0.clen).sum();
    let need = opts.threshold(file);
    if section < need {
        eprintln!(
            "derive-iris: the subject-only dictionary is {section} B, under the {need} B a family \
             must remove: not examined"
        );
        return Ok(Vec::new());
    }
    let pso = IndexPermutation::Pso.section_index();
    let spo = IndexPermutation::Spo.section_index();
    let find = |si: usize| parts.index.iter().find(|p| p.si == si);
    let (order, perm) = match (find(pso), find(spo)) {
        (Some(p), _) => (Order::Pso, p),
        (None, Some(p)) if !parts.cluster.iter().any(|c| c.si == spo) => (Order::Spo, p),
        _ => {
            notes.sampled = true;
            notes.reject(
                None,
                "the file has neither PSO nor an unclustered SPO to read the rows from".into(),
            );
            return Ok(Vec::new());
        }
    };
    let n = sec.term_count as usize;
    let shared = parts.dict[0].term_count;
    // The windows: a golden-ratio sequence over the section's ids.
    let k = WINDOW_TERMS.min(n);
    let mut starts: Vec<usize> = Vec::new();
    for w in 0..WINDOWS.min(n / k.max(1)) {
        let frac = (w as f64 * 0.618_033_988_749_895).fract();
        starts.push((frac * (n - k + 1) as f64) as usize);
    }
    let wins: Vec<(u32, u32)> = starts
        .iter()
        .map(|&a| {
            let lo = shared + a as u32 + 1;
            (lo, lo + k as u32 - 1)
        })
        .collect();
    let rows = sample_rows(input, parts.block_codec, order, &perm.tiles, &wins)?;
    let used = rows.len();
    notes.sampled = true;
    notes.windows = used;
    // Per window: its subjects' texts and single-valued objects.
    let pred_terms = predicate_terms(parts, input)?;
    let mut win_subjects: Vec<Vec<(u32, String)>> = Vec::with_capacity(used);
    for a in &starts[..used] {
        let terms = section_terms(parts, input, SECTION, a + 1, k)?.unwrap_or_default();
        win_subjects.push(
            terms
                .into_iter()
                .enumerate()
                .filter_map(|(j, t)| {
                    Some((shared + (a + 1 + j) as u32, String::from_utf8(t).ok()?))
                })
                .collect(),
        );
    }
    let mut objs: Vec<u32> = Vec::new();
    for w in &rows {
        objs.extend(w.iter().map(|r| r.2));
    }
    objs.sort_unstable();
    objs.dedup();
    let texts = object_texts(parts, input, &objs)?;
    let text_of = |o: u32| -> Option<&str> {
        let i = objs.binary_search(&o).ok()?;
        std::str::from_utf8(texts[i].as_deref()?).ok()
    };
    let mut tally: std::collections::BTreeMap<Sig, usize> = std::collections::BTreeMap::new();
    for (w, subjects) in win_subjects.iter().enumerate() {
        let mut sigs: Vec<Option<Sig>> = Vec::new();
        for (s, t) in subjects {
            notes.subjects += 1;
            let own: Vec<&(u32, u32, u32)> = rows[w].iter().filter(|r| r.0 == *s).collect();
            let mut cands: Vec<(u32, &str)> = Vec::new();
            for r in &own {
                // Single-valued: the subject has exactly one row of this predicate.
                if own.iter().filter(|x| x.1 == r.1).count() != 1 {
                    continue;
                }
                if let Some(u) = text_of(r.2)
                    .and_then(|u| u.strip_prefix('<'))
                    .and_then(|u| u.strip_suffix('>'))
                {
                    cands.push((r.1, u));
                }
            }
            let sig = t
                .strip_prefix('<')
                .and_then(|t| t.strip_suffix('>'))
                .and_then(|inner| signature(inner, &cands));
            sigs.push(sig);
        }
        if let Some(Some(first)) = sigs.first() {
            if sigs.len() == subjects.len() && sigs.iter().all(|s| s.as_ref() == Some(first)) {
                *tally.entry(first.clone()).or_default() += 1;
            }
        }
    }
    let mut ranked: Vec<(Sig, usize)> = tally.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out = Vec::new();
    for (sig, fit) in ranked {
        let est = section.saturating_mul(fit as u64) / used.max(1) as u64;
        let inputs: Option<Vec<Input>> = sig
            .holes
            .iter()
            .map(|(p, prefix)| {
                let t = pred_terms.get(*p as usize - 1)?;
                let t = std::str::from_utf8(t).ok()?;
                Some(Input {
                    predicate: t.strip_prefix('<')?.strip_suffix('>')?.to_string(),
                    prefix: prefix.clone(),
                })
            })
            .collect();
        let fam = inputs.and_then(|i| Family::new(sig.literals.clone(), i).ok());
        let Some(fam) = fam else { continue };
        if est < need {
            notes.reject(
                Some(&fam.spec()),
                format!(
                    "fits {fit} of {used} sampled windows, about {est} B of the {section} B \
                     subject-only dictionary, under the {need} B a family must remove"
                ),
            );
            continue;
        }
        if out.len() < MAX_CANDIDATES {
            out.push(Candidate {
                fam,
                fit,
                windows: used,
            });
        }
    }
    if out.is_empty() && notes.rejected.is_empty() {
        notes.reject(
            None,
            format!(
                "no template of the subject IRIs is spelled by their own objects in the {} \
                 sampled subjects",
                notes.subjects
            ),
        );
    }
    Ok(out)
}

/// Detection and verification for `parts`: the families found, the chunks that
/// derive dropped, the build record extended with what was found and why.
/// `None` when there is nothing to change (not examined, nothing taken).
pub(crate) fn detect_and_derive(
    parts: &Parts,
    input: &dyn RangeReader,
    opts: &DeriveOpts,
) -> Result<Option<Parts>, FileError> {
    let file = input.len();
    let need = opts.threshold(file);
    let mut notes = Notes::default();
    let t0 = Clock::start();
    let cands = discover(parts, input, opts, file, &mut notes)?;
    if notes.sampled {
        eprintln!(
            "derive-iris: sampled {} subjects in {} windows in {}, {} candidate(s)",
            notes.subjects,
            notes.windows,
            t0.elapsed(),
            cands.len()
        );
    }
    let t1 = Clock::start();
    let section: u64 = parts.dict[SECTION].chunks.iter().map(|c| c.0.clen).sum();
    let mut out = parts.clone();
    let mut claimed = vec![false; parts.dict[SECTION].chunks.len()];
    let mut fams: Vec<Family> = Vec::new();
    for c in &cands {
        let mut fam = Family {
            runs: Vec::new(),
            ..c.fam.clone()
        };
        let template = fam.spec();
        let mut d = match Deriver::new(parts, input, &fam)? {
            Ok(d) => d,
            Err(why) => {
                notes.reject(Some(&template), why);
                continue;
            }
        };
        let sec = &parts.dict[SECTION];
        let ri = sec.restart_interval as usize;
        let todo: Vec<usize> = (0..sec.chunks.len()).filter(|&ci| !claimed[ci]).collect();
        let mut won: Vec<usize> = Vec::new();
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
                let n = chunk_terms(&sec.chunks, ci, ri, sec.term_count);
                if let (Some(a), Some(b)) = (derived, stored) {
                    if a.len() == n && a == b {
                        won.push(ci);
                    }
                }
            }
        }
        let saved: u64 = won.iter().map(|&ci| sec.chunks[ci].0.clen).sum();
        if saved < need {
            notes.reject(
                Some(&template),
                format!(
                    "verified {} of {} subject-only chunks, {saved} B removed, under the {need} B \
                     a family must remove (a chunk with one term that does not fit stays stored)",
                    won.len(),
                    todo.len()
                ),
            );
            continue;
        }
        let pct = saved as f64 * 100.0 / file.max(1) as f64;
        eprintln!(
            "derive-iris: taken {template}: {} of {} subject-only chunks derive, {saved} B \
             removed ({pct:.1} % of the file); verified in {}",
            won.len(),
            todo.len(),
            t1.elapsed()
        );
        notes.taken.push(serde_json::json!({
            "template": clip(&template),
            "chunks": won.len(),
            "of_chunks": todo.len(),
            "bytes_removed": saved,
            "percent_of_file": (pct * 10.0).round() / 10.0,
            "sampled_windows_fitting": format!("{} of {}", c.fit, c.windows),
        }));
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
        fam.runs = runs;
        fams.push(fam);
    }
    let mut changed = !fams.is_empty();
    out.derived = fams;
    // The report goes into the build record (a file that has none gets one with
    // just the report: the front end replaces it with its own, carrying the
    // report over).
    if let Some(report) = notes.report(opts, file, section) {
        let base = out.build.clone().unwrap_or_else(|| b"{}".to_vec());
        if let Some(r) = crate::build_record::with_derive_report(&base, &report) {
            out.build = Some(r);
            changed = true;
        }
    }
    Ok(changed.then_some(out))
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
