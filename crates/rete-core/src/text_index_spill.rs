//! The **memory-bounded** builder of the TEXT_INDEX section — `--text-index`
//! for `rete build --memory-budget-mb` ([`crate::extbuild`]).
//!
//! The index is `token → sorted distinct subject ids` over the literals in the
//! scope. The in-RAM build holds that whole map; here it is a second external
//! sort, on a string key, beside the triple spill:
//!
//! ```text
//! seal   (per chunk)  in-scope literal objects → tokenize → c<i>.tok   (token, chunk-local subject)
//! remap  (phase 3)    c<i>.tok × the chunk's subject remap  → global.tok (token, global subject)
//! runs                budget-sized, sorted by (token bytes, subject), deduped → tok.run<k>
//! merge               k-way (dedup) → the shared SectionWriter → token entries + postings, on disk
//! table               varint(num_tokens) ‖ entries ‖ scope block → block codec → token table, on disk
//! ```
//!
//! The tokens come from the chunk's raw strings and its own dictionary while
//! the chunk is sealed, so the literals are never read twice; the subject ids
//! reuse the chunk-local → global remap the dictionary merge already produces.
//!
//! **Byte identity with the in-RAM build.** Both builds feed the same
//! [`SectionWriter`] the same `(token, subjects)` sequence: tokens ascending by
//! bytes (= `String` order), each token's distinct subjects ascending. The
//! token table is compressed by `compress_to`, the streaming twin of the
//! `compress` the in-RAM build calls, pinned to the same bytes by a test here.
//!
//! **Memory.** A sort run holds at most `run_bytes` of records (token bytes +
//! 12 B each), the merge one record per run, and a posting being assembled at
//! most [`OVERFLOW_BYTES`] of deltas before the rest goes to disk. The token
//! table is streamed through the encoder. Nothing is sized by the dataset.

#![cfg(not(target_arch = "wasm32"))]

use std::collections::BinaryHeap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::dictionary::Dictionary;
use crate::extbuild::ExtBuildError;
use crate::terms::{is_literal, literal_lexical};
use crate::text_index::{
    push_delta, tokenize, warn_unmatched, SectionWriter, TextIndexPredicates, TextIndexScope,
};
use crate::varint::write_uvarint;

/// A posting whose delta bytes outgrow this spills them to disk while it is
/// assembled, so one very common word (`the`, a year) cannot blow the budget.
const OVERFLOW_BYTES: usize = 8 << 20;
/// Read buffer per spill/run reader.
const READ_BUF: usize = 256 << 10;
/// Bytes of a sort run's arena that its `u32` offsets can address.
const MAX_RUN_ARENA: usize = u32::MAX as usize;

/// The finished section, spilled: `head` (the `token_table_len` varint), then
/// the compressed token table file, then the postings file — in that order.
pub(crate) struct TextSection {
    pub(crate) head: Vec<u8>,
    pub(crate) table: PathBuf,
    pub(crate) table_len: u64,
    pub(crate) postings: PathBuf,
    pub(crate) postings_len: u64,
    /// The scope leaves predicates out: the header lists it as kind 11, and
    /// the section starts with its 16-byte self-digest.
    pub(crate) partial: bool,
    /// The scope covers the named graphs: the header's
    /// `FLAG_TEXT_INDEX_NAMED_GRAPHS` bit.
    pub(crate) named: bool,
}

impl TextSection {
    /// Payload length: the token-table varint, the token table, the postings.
    pub(crate) fn payload_len(&self) -> u64 {
        self.head.len() as u64 + self.table_len + self.postings_len
    }

    /// Section length in the file: the payload, behind a kind-11 section's
    /// 16-byte digest.
    pub(crate) fn len(&self) -> u64 {
        let digest = if self.partial {
            crate::header::TEXT_INDEX_PARTIAL_DIGEST_LEN
        } else {
            0
        };
        digest + self.payload_len()
    }
}

/// The text-index side of one external build: collects every chunk's token
/// records, remaps them, then sorts and serializes them.
pub(crate) struct TextSpill {
    scope: TextIndexScope,
    /// Which listed include/exclude predicates some in-scope literal carried.
    seen: Vec<bool>,
    /// Records written by the chunks (before any dedup), for the log line.
    records: u64,
    global: Option<(PathBuf, BufWriter<File>)>,
}

impl TextSpill {
    pub(crate) fn new(scope: TextIndexScope) -> Self {
        let seen = vec![false; scope.listed_predicates().len()];
        TextSpill {
            scope,
            seen,
            records: 0,
            global: None,
        }
    }

    /// Is predicate `p` in scope? Marks a listed predicate as seen.
    fn covered(&mut self, p: &str) -> bool {
        let hit = self
            .scope
            .listed_predicates()
            .binary_search_by(|x| x.as_str().cmp(p))
            .ok();
        if let Some(i) = hit {
            self.seen[i] = true;
        }
        match self.scope.predicates {
            TextIndexPredicates::All => true,
            TextIndexPredicates::Only(_) => hit.is_some(),
            TextIndexPredicates::Except(_) => hit.is_none(),
        }
    }

    /// Phase 1, inside a chunk seal: write `(token, chunk-local subject)` for
    /// every word of every in-scope literal object to `path`. `dict` is the
    /// chunk's own dictionary; `named` carries chunk-local graph ids that only
    /// matter for whether named graphs are in scope at all.
    pub(crate) fn spill_chunk(
        &mut self,
        path: &Path,
        dict: &Dictionary,
        default: &[(String, String, String)],
        named: &[(u32, String, String, String)],
    ) -> Result<(), ExtBuildError> {
        let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
        let named_in_scope = if self.scope.covers_named_graphs() {
            named
        } else {
            &[]
        };
        let triples = default
            .iter()
            .map(|(s, p, o)| (s, p, o))
            .chain(named_in_scope.iter().map(|(_, s, p, o)| (s, p, o)));
        let mut words: Vec<String> = Vec::new();
        let mut rec = Vec::with_capacity(64);
        for (s, p, o) in triples {
            if !is_literal(o) || !self.covered(p) {
                continue;
            }
            let Some(text) = literal_lexical(o) else {
                continue;
            };
            words.clear();
            words.extend(tokenize(&text));
            if words.is_empty() {
                continue;
            }
            // A word repeated inside one literal is one posting entry anyway;
            // dropping it here just keeps it off the disk.
            words.sort_unstable();
            words.dedup();
            let subject = dict.subject_id(s).ok_or(ExtBuildError::Internal(
                "chunk subject missing from own dict",
            ))?;
            for word in &words {
                rec.clear();
                write_uvarint(&mut rec, word.len() as u64);
                rec.extend_from_slice(word.as_bytes());
                rec.extend_from_slice(&subject.to_le_bytes());
                w.write_all(&rec)?;
                self.records += 1;
            }
        }
        w.flush()?;
        Ok(())
    }

    /// Phase 3: open the global spill the chunks are remapped into.
    pub(crate) fn begin_remap(&mut self, path: PathBuf) -> Result<(), ExtBuildError> {
        let w = BufWriter::with_capacity(1 << 20, File::create(&path)?);
        self.global = Some((path, w));
        Ok(())
    }

    /// Phase 3: append chunk `path`'s records to the global spill with their
    /// subjects remapped through `subj` (chunk-local id − 1 → global id), then
    /// delete the chunk file. A chunk that never wrote one (the empty-input
    /// placeholder) contributes nothing.
    pub(crate) fn remap_chunk(&mut self, path: &Path, subj: &[u32]) -> Result<(), ExtBuildError> {
        let (_, out) = self.global.as_mut().ok_or(ExtBuildError::Internal(
            "text-index remap before begin_remap",
        ))?;
        if !path.exists() {
            return Ok(());
        }
        let mut rd = BufReader::with_capacity(READ_BUF, File::open(path)?);
        let mut token = Vec::new();
        let mut rec = Vec::with_capacity(64);
        while let Some(local) = read_record(&mut rd, &mut token)? {
            let global =
                *subj
                    .get((local as usize).wrapping_sub(1))
                    .ok_or(ExtBuildError::Internal(
                        "text-index subject outside the chunk remap",
                    ))?;
            rec.clear();
            write_uvarint(&mut rec, token.len() as u64);
            rec.extend_from_slice(&token);
            rec.extend_from_slice(&global.to_le_bytes());
            out.write_all(&rec)?;
        }
        let _ = std::fs::remove_file(path);
        Ok(())
    }

    /// Phase 3b: sort the global spill in runs of at most `run_bytes`, merge
    /// them into the section's token entries and postings, and compress the
    /// token table. `path_of` names spill files in the build's tmp dir. `None`
    /// when no literal was in scope — the file then has no text index, as the
    /// in-RAM build writes none.
    pub(crate) fn finish(
        mut self,
        path_of: &dyn Fn(&str) -> PathBuf,
        run_bytes: usize,
        codec: u8,
    ) -> Result<Option<TextSection>, ExtBuildError> {
        let (global, mut w) = self.global.take().ok_or(ExtBuildError::Internal(
            "text-index finish before begin_remap",
        ))?;
        w.flush()?;
        drop(w);
        warn_unmatched(&self.scope, &self.seen);

        let runs = sort_runs(&global, run_bytes.clamp(1 << 10, MAX_RUN_ARENA), path_of)?;
        let _ = std::fs::remove_file(&global);

        let entries_path = path_of("tok.entries");
        let postings_path = path_of("tok.postings");
        let overflow_path = path_of("tok.overflow");
        let mut writer = SectionWriter::new(
            BufWriter::with_capacity(1 << 20, File::create(&entries_path)?),
            BufWriter::with_capacity(1 << 20, File::create(&postings_path)?),
        );
        merge_runs(&runs, &mut writer, &overflow_path)?;
        for r in &runs {
            let _ = std::fs::remove_file(r);
        }
        let _ = std::fs::remove_file(&overflow_path);
        let (mut entries, mut postings, tokens, postings_len) = writer.finish();
        entries.flush()?;
        postings.flush()?;
        drop((entries, postings));

        if tokens == 0 {
            let _ = std::fs::remove_file(&entries_path);
            let _ = std::fs::remove_file(&postings_path);
            eprintln!("extbuild: text index — no literal in scope, no section written");
            return Ok(None);
        }

        // token table = varint(num_tokens) ‖ entries ‖ scope block, compressed.
        let mut count = Vec::new();
        write_uvarint(&mut count, tokens);
        let mut scope_block = Vec::new();
        self.scope.encode(&mut scope_block);
        let table_path = path_of("tok.table");
        {
            let entries_len = std::fs::metadata(&entries_path)?.len();
            let src_len = count.len() as u64 + entries_len + scope_block.len() as u64;
            let entries = BufReader::with_capacity(READ_BUF, File::open(&entries_path)?);
            let mut src = count
                .as_slice()
                .chain(entries)
                .chain(scope_block.as_slice());
            let mut dst = BufWriter::with_capacity(1 << 20, File::create(&table_path)?);
            crate::file::compress_to(codec, src_len, &mut src, &mut dst)?;
            dst.flush()?;
        }
        let _ = std::fs::remove_file(&entries_path);
        let table_len = std::fs::metadata(&table_path)?.len();
        let mut head = Vec::new();
        write_uvarint(&mut head, table_len);
        let section = TextSection {
            head,
            table: table_path,
            table_len,
            postings: postings_path,
            postings_len,
            partial: self.scope.restricts_predicates(),
            named: self.scope.covers_named_graphs(),
        };
        eprintln!(
            "extbuild: text index — {} token(s) from {} record(s), {} byte(s) ({}{})",
            tokens,
            self.records,
            section.len(),
            self.scope.describe(),
            if section.partial {
                "; section kind 11"
            } else {
                ""
            }
        );
        Ok(Some(section))
    }
}

/// Read a varint from a buffered stream: `None` at a clean end of stream.
fn read_varint(rd: &mut impl BufRead) -> std::io::Result<Option<u64>> {
    let mut value = 0u64;
    let mut shift = 0u32;
    loop {
        let mut b = [0u8; 1];
        if rd.read(&mut b)? == 0 {
            return if shift == 0 {
                Ok(None)
            } else {
                Err(std::io::ErrorKind::UnexpectedEof.into())
            };
        }
        value |= u64::from(b[0] & 0x7f) << shift;
        if b[0] & 0x80 == 0 {
            return Ok(Some(value));
        }
        shift += 7;
        if shift > 63 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "text-index spill varint overflows",
            ));
        }
    }
}

/// Read one `(varint len, token bytes, u32 subject)` record into `token`.
fn read_record(rd: &mut impl BufRead, token: &mut Vec<u8>) -> std::io::Result<Option<u32>> {
    let Some(len) = read_varint(rd)? else {
        return Ok(None);
    };
    token.resize(len as usize, 0);
    rd.read_exact(token)?;
    let mut s = [0u8; 4];
    rd.read_exact(&mut s)?;
    Ok(Some(u32::from_le_bytes(s)))
}

/// One sort run in memory: token bytes in an arena, records as
/// `(arena offset, token length, subject)`.
struct RunBuf {
    arena: Vec<u8>,
    recs: Vec<(u32, u32, u32)>,
}

impl RunBuf {
    fn bytes(&self) -> usize {
        self.arena.len() + self.recs.len() * 12
    }

    fn token(&self, r: &(u32, u32, u32)) -> &[u8] {
        &self.arena[r.0 as usize..(r.0 + r.1) as usize]
    }

    /// Sort by (token bytes, subject), drop duplicates, write to `path`.
    fn seal(&mut self, path: &Path) -> Result<(), ExtBuildError> {
        let arena = &self.arena;
        let tok = |r: &(u32, u32, u32)| &arena[r.0 as usize..(r.0 + r.1) as usize];
        let cmp = |a: &(u32, u32, u32), b: &(u32, u32, u32)| tok(a).cmp(tok(b)).then(a.2.cmp(&b.2));
        sort_records(&mut self.recs, &cmp);
        self.recs
            .dedup_by(|a, b| a.2 == b.2 && tok(&*a) == tok(&*b));
        let mut w = BufWriter::with_capacity(1 << 20, File::create(path)?);
        let mut rec = Vec::with_capacity(64);
        for r in &self.recs {
            rec.clear();
            let t = self.token(r);
            write_uvarint(&mut rec, t.len() as u64);
            rec.extend_from_slice(t);
            rec.extend_from_slice(&r.2.to_le_bytes());
            w.write_all(&rec)?;
        }
        w.flush()?;
        self.arena.clear();
        self.recs.clear();
        Ok(())
    }
}

#[cfg(feature = "parallel")]
fn sort_records<F>(v: &mut [(u32, u32, u32)], cmp: &F)
where
    F: Fn(&(u32, u32, u32), &(u32, u32, u32)) -> std::cmp::Ordering + Sync,
{
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable_by(cmp);
}

#[cfg(not(feature = "parallel"))]
fn sort_records<F>(v: &mut [(u32, u32, u32)], cmp: &F)
where
    F: Fn(&(u32, u32, u32), &(u32, u32, u32)) -> std::cmp::Ordering + Sync,
{
    v.sort_unstable_by(cmp);
}

/// Cut the global spill into sorted, deduped runs of at most `run_bytes`.
fn sort_runs(
    global: &Path,
    run_bytes: usize,
    path_of: &dyn Fn(&str) -> PathBuf,
) -> Result<Vec<PathBuf>, ExtBuildError> {
    let mut rd = BufReader::with_capacity(READ_BUF, File::open(global)?);
    let mut run = RunBuf {
        arena: Vec::new(),
        recs: Vec::new(),
    };
    let mut runs = Vec::new();
    let mut token = Vec::new();
    loop {
        let next = read_record(&mut rd, &mut token)?;
        let full = run.bytes() + token.len() + 12 > run_bytes
            || run.arena.len() + token.len() > MAX_RUN_ARENA;
        if !run.recs.is_empty() && (next.is_none() || full) {
            let path = path_of(&format!("tok.run{}", runs.len()));
            run.seal(&path)?;
            runs.push(path);
        }
        let Some(subject) = next else { break };
        let off = u32::try_from(run.arena.len())
            .map_err(|_| ExtBuildError::Internal("text-index run arena overflow"))?;
        run.arena.extend_from_slice(&token);
        run.recs.push((off, token.len() as u32, subject));
    }
    Ok(runs)
}

/// The posting being assembled for the current token: delta bytes in memory,
/// or — past [`OVERFLOW_BYTES`] — on disk.
struct PostingAcc<'p> {
    count: u64,
    prev: u32,
    deltas: Vec<u8>,
    overflow_path: &'p Path,
    overflow: Option<BufWriter<File>>,
    overflow_len: u64,
}

impl PostingAcc<'_> {
    fn push(&mut self, s: u32) -> std::io::Result<()> {
        if self.count > 0 && s == self.prev {
            return Ok(()); // the same (token, subject) from another run
        }
        push_delta(&mut self.deltas, &mut self.prev, s);
        self.count += 1;
        if self.deltas.len() >= OVERFLOW_BYTES {
            self.spill()?;
        }
        Ok(())
    }

    fn spill(&mut self) -> std::io::Result<()> {
        if self.overflow.is_none() {
            self.overflow = Some(BufWriter::with_capacity(
                1 << 20,
                File::create(self.overflow_path)?,
            ));
            self.overflow_len = 0;
        }
        let w = self.overflow.as_mut().expect("just opened");
        w.write_all(&self.deltas)?;
        self.overflow_len += self.deltas.len() as u64;
        self.deltas.clear();
        Ok(())
    }

    /// Hand the finished posting for `token` to the writer and reset.
    fn emit<E: Write, P: Write>(
        &mut self,
        token: &[u8],
        writer: &mut SectionWriter<E, P>,
    ) -> std::io::Result<()> {
        if self.count == 0 {
            return Ok(());
        }
        if self.overflow.is_some() {
            self.spill()?;
            let mut w = self.overflow.take().expect("overflow is open");
            w.flush()?;
            drop(w);
            let path = self.overflow_path;
            writer.push_with(token, self.count, self.overflow_len, |p| {
                let mut rd = BufReader::with_capacity(READ_BUF, File::open(path)?);
                std::io::copy(&mut rd, p).map(|_| ())
            })?;
        } else {
            writer.push(token, self.count, &self.deltas)?;
        }
        self.count = 0;
        self.prev = 0;
        self.deltas.clear();
        self.overflow_len = 0;
        Ok(())
    }
}

/// K-way merge of the sorted runs, deduplicating `(token, subject)`, feeding
/// each finished token to `writer` in ascending token order.
fn merge_runs<E: Write, P: Write>(
    runs: &[PathBuf],
    writer: &mut SectionWriter<E, P>,
    overflow_path: &Path,
) -> Result<(), ExtBuildError> {
    crate::build_trace::note_merge(runs.len());
    let mut readers: Vec<BufReader<File>> = runs
        .iter()
        .map(|p| File::open(p).map(|f| BufReader::with_capacity(READ_BUF, f)))
        .collect::<Result<_, _>>()?;
    // (token, subject, run) — the min-heap order is exactly the output order.
    let mut heap: BinaryHeap<std::cmp::Reverse<(Vec<u8>, u32, usize)>> = BinaryHeap::new();
    for (i, rd) in readers.iter_mut().enumerate() {
        let mut token = Vec::new();
        if let Some(s) = read_record(rd, &mut token)? {
            heap.push(std::cmp::Reverse((token, s, i)));
        }
    }
    let mut current: Vec<u8> = Vec::new();
    let mut acc = PostingAcc {
        count: 0,
        prev: 0,
        deltas: Vec::new(),
        overflow_path,
        overflow: None,
        overflow_len: 0,
    };
    while let Some(std::cmp::Reverse((mut token, s, i))) = heap.pop() {
        if acc.count == 0 || token != current {
            acc.emit(&current, writer)?;
            // The new token becomes `current`; the old buffer is recycled for
            // this run's next record.
            std::mem::swap(&mut current, &mut token);
        }
        acc.push(s)?;
        if let Some(n) = read_record(&mut readers[i], &mut token)? {
            heap.push(std::cmp::Reverse((token, n, i)));
        }
    }
    acc.emit(&current, writer)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text_index::{TextIndex, TextIndexBuilder, TextIndexGraphs};

    fn tmpdir(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("rete-tispill-{tag}-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Run `(token, subject)` pairs through the spill path (as if one chunk had
    /// produced them with identity ids) and return the section bytes.
    fn spill_section(pairs: &[(&str, u32)], scope: TextIndexScope, run_bytes: usize) -> Vec<u8> {
        let dir = tmpdir("sec");
        let path_of = |n: &str| dir.join(n);
        let mut spill = TextSpill::new(scope);
        spill.begin_remap(path_of("global.tok")).unwrap();
        {
            let (_, w) = spill.global.as_mut().unwrap();
            for (t, s) in pairs {
                let mut rec = Vec::new();
                write_uvarint(&mut rec, t.len() as u64);
                rec.extend_from_slice(t.as_bytes());
                rec.extend_from_slice(&s.to_le_bytes());
                w.write_all(&rec).unwrap();
            }
        }
        let codec = crate::file::writer_codec();
        let sec = spill.finish(&path_of, run_bytes, codec).unwrap();
        let out = match sec {
            None => Vec::new(),
            Some(sec) => {
                let mut out = sec.head.clone();
                out.extend(std::fs::read(&sec.table).unwrap());
                out.extend(std::fs::read(&sec.postings).unwrap());
                assert_eq!(out.len() as u64, sec.payload_len());
                out
            }
        };
        let _ = std::fs::remove_dir_all(&dir);
        out
    }

    fn ram_section(pairs: &[(&str, u32)], scope: TextIndexScope) -> Vec<u8> {
        let mut b = TextIndexBuilder::with_scope(scope);
        for (t, s) in pairs {
            b.add(t, *s);
        }
        if b.is_empty() {
            Vec::new()
        } else {
            b.build(crate::file::writer_codec())
        }
    }

    /// Pseudo-random pairs with heavy duplication, multi-byte tokens and long
    /// shared prefixes — every shape the front coding and the dedup must agree on.
    fn pairs(n: usize) -> Vec<(String, u32)> {
        let words = [
            "glucose",
            "glucoside",
            "glu",
            "ab",
            "abc",
            "été",
            "étude",
            "ünïcode",
            "日本",
            "日本語",
            "0001",
            "0002",
            "z9",
        ];
        let mut x = 0x2545_f491_4f6c_dd1du64;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let w = words[(x % words.len() as u64) as usize];
                let tok = if i % 3 == 0 {
                    format!("{w}{}", x % 97)
                } else {
                    w.to_string()
                };
                (tok, 1 + (x >> 20) as u32 % 5000)
            })
            .collect()
    }

    #[test]
    fn spilled_section_is_byte_identical_to_the_in_ram_section() {
        let owned = pairs(20_000);
        let pairs: Vec<(&str, u32)> = owned.iter().map(|(t, s)| (t.as_str(), *s)).collect();
        let reference = ram_section(&pairs, TextIndexScope::default());
        // One run, and many tiny runs (forcing the k-way merge and the
        // cross-run dedup).
        for run_bytes in [usize::MAX, 4096, 1 << 10] {
            assert_eq!(
                spill_section(&pairs, TextIndexScope::default(), run_bytes),
                reference,
                "run_bytes {run_bytes}"
            );
        }
        // A non-default scope writes the same block both ways.
        let scope = TextIndexScope::from_options(
            TextIndexGraphs::All,
            Vec::<String>::new(),
            ["http://ex/p"],
        )
        .unwrap();
        assert_eq!(
            spill_section(&pairs, scope.clone(), 4096),
            ram_section(&pairs, scope)
        );
        // Nothing in scope: no section either way.
        assert!(spill_section(&[], TextIndexScope::default(), 4096).is_empty());
        assert!(ram_section(&[], TextIndexScope::default()).is_empty());
    }

    /// A posting larger than the in-memory threshold is assembled on disk and
    /// still comes out as the same bytes.
    #[test]
    fn oversized_posting_spills_and_stays_identical() {
        let dir = tmpdir("ovf");
        let overflow = dir.join("ovf");
        let mut w = SectionWriter::new(Vec::new(), Vec::new());
        let mut acc = PostingAcc {
            count: 0,
            prev: 0,
            deltas: Vec::new(),
            overflow_path: &overflow,
            overflow: None,
            overflow_len: 0,
        };
        // Force several spills by pushing past the threshold with big gaps.
        let n = OVERFLOW_BYTES / 2 + 1000;
        for s in 0..n as u32 {
            acc.push(1 + s * 1000).unwrap();
        }
        assert!(acc.overflow.is_some(), "the posting must have spilled");
        acc.emit(b"word", &mut w).unwrap();
        let (entries, postings, tokens, _) = w.finish();

        let mut reference = SectionWriter::new(Vec::new(), Vec::new());
        let mut deltas = Vec::new();
        let mut prev = 0;
        for s in 0..n as u32 {
            push_delta(&mut deltas, &mut prev, 1 + s * 1000);
        }
        reference.push(b"word", n as u64, &deltas).unwrap();
        let (re, rp, rt, _) = reference.finish();
        assert_eq!((entries, postings, tokens), (re, rp, rt));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `compress_to` (streamed from a file, fed in arbitrary pieces) and
    /// `compress` (one slice) must produce the same bytes: the memory-bounded
    /// build relies on it for a token table it never holds whole.
    #[test]
    fn streaming_compression_matches_one_shot() {
        let codec = crate::file::writer_codec();
        // Multi-block (zstd blocks are 128 KiB), compressible but not trivial.
        let mut data = Vec::new();
        let mut x = 7u64;
        while data.len() < 3 << 20 {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let word = format!("tok{}-", (x >> 40) % 5000);
            data.extend_from_slice(word.as_bytes());
        }
        let one_shot = crate::file::compress(codec, &data);
        // Fed through a reader that hands out odd-sized pieces.
        struct Pieces<'a>(&'a [u8], usize);
        impl Read for Pieces<'_> {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                self.1 = (self.1 * 7 + 13) % 70_001 + 1;
                let n = self.1.min(buf.len()).min(self.0.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut out = Vec::new();
        crate::file::compress_to(codec, data.len() as u64, &mut Pieces(&data, 1), &mut out)
            .unwrap();
        assert_eq!(out, one_shot);
        // And through the chain the builder uses (count ‖ file ‖ scope).
        let mut out2 = Vec::new();
        let (a, b) = data.split_at(12_345);
        let (b, c) = b.split_at(1 << 20);
        crate::file::compress_to(
            codec,
            data.len() as u64,
            &mut a.chain(b).chain(c),
            &mut out2,
        )
        .unwrap();
        assert_eq!(out2, one_shot);
    }

    /// The chunk seal reads raw strings: only literal objects, only in-scope
    /// predicates and graphs, one record per distinct word per literal.
    #[test]
    fn chunk_spill_respects_scope() {
        let dir = tmpdir("chunk");
        let default = vec![
            (
                "<http://ex/a>".to_string(),
                "<http://ex/title>".to_string(),
                "\"Glucose glucose Benzene\"".to_string(),
            ),
            (
                "<http://ex/a>".to_string(),
                "<http://ex/desc>".to_string(),
                "\"has been modified\"".to_string(),
            ),
            (
                "<http://ex/b>".to_string(),
                "<http://ex/title>".to_string(),
                "<http://ex/notaliteral>".to_string(),
            ),
        ];
        let named = vec![(
            1u32,
            "<http://ex/c>".to_string(),
            "<http://ex/title>".to_string(),
            "\"Provenance words\"".to_string(),
        )];
        let mut db = crate::DictionaryBuilder::new();
        for (s, p, o) in &default {
            db.observe(s, p, o);
        }
        for (_, s, p, o) in &named {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let read_back = |scope: TextIndexScope| -> Vec<(String, u32)> {
            let mut spill = TextSpill::new(scope);
            let path = dir.join("c0.tok");
            spill.spill_chunk(&path, &dict, &default, &named).unwrap();
            let mut rd = BufReader::new(File::open(&path).unwrap());
            let mut tok = Vec::new();
            let mut out = Vec::new();
            while let Some(s) = read_record(&mut rd, &mut tok).unwrap() {
                out.push((String::from_utf8(tok.clone()).unwrap(), s));
            }
            out.sort();
            out
        };
        let a = dict.subject_id("<http://ex/a>").unwrap();
        let c = dict.subject_id("<http://ex/c>").unwrap();
        let words = |v: &[(String, u32)]| v.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>();
        let all = read_back(TextIndexScope::default());
        assert_eq!(
            words(&all),
            ["been", "benzene", "glucose", "has", "modified"]
        );
        assert!(all.iter().all(|(_, s)| *s == a));
        let excl = read_back(
            TextIndexScope::from_options(
                TextIndexGraphs::Default,
                Vec::<String>::new(),
                ["http://ex/desc"],
            )
            .unwrap(),
        );
        assert_eq!(words(&excl), ["benzene", "glucose"]);
        let named_too = read_back(
            TextIndexScope::from_options(
                TextIndexGraphs::All,
                ["http://ex/title"],
                Vec::<String>::new(),
            )
            .unwrap(),
        );
        assert_eq!(
            words(&named_too),
            ["benzene", "glucose", "provenance", "words"]
        );
        assert!(named_too.contains(&("provenance".to_string(), c)));
        // A section built from the excluded scope parses back with its scope.
        let mut b = TextIndexBuilder::with_scope(
            TextIndexScope::from_options(
                TextIndexGraphs::Default,
                Vec::<String>::new(),
                ["http://ex/desc"],
            )
            .unwrap(),
        );
        b.add("glucose", a);
        let codec = crate::file::writer_codec();
        let idx = TextIndex::from_section(&b.build(codec), codec).unwrap();
        assert!(idx.scope().restricts_predicates());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
