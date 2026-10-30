//! Re-carding a finished `.rete` — replacing its Dataset Card and build record
//! **without rebuilding it**.
//!
//! A card used to be fixed at build time: it sits inside the content hash, near
//! the front of the file, so the only way to a new one was a new build. On the
//! 2.9 B-statement OpenCitations parts that is three hours each, for a few KB of
//! JSON. Everything a card is derived from is already in the file — sorted, see
//! [`crate::card::derive_card_streaming`] — and every other byte of the file is
//! untouched by a new card, so a re-card is a *rewrite of two sections and a
//! copy of the rest*:
//!
//! * the metadata section (the card) and the build-info section are replaced,
//!   in place, adjacent as the writers lay them out;
//! * every other section — dictionary, index, pyramid, named graphs, text
//!   index, and any section kind this reader does not know — is copied **byte
//!   for byte**, in its place in the file order; only its offset in the header
//!   moves, and bytes no section points at (what an in-place re-card leaves,
//!   below) are not copied;
//! * the content hash is recomputed, because the card is inside it
//!   ([`crate::format::content_hash_parts`]); it is folded while the bytes go
//!   past, so the whole job is one read and one write of the file.
//!
//! [`plan_card_rewrite`] is the arithmetic, kept apart (like
//! [`crate::format::plan_build_info`]) so it is testable without a file;
//! [`rewrite_card`] is the streaming copy.
//!
//! ## The card after the data
//!
//! A copy is the price of keeping the card at the front. The header is the
//! directory, so the card can sit anywhere: [`plan_card_append`],
//! [`card_append_hash`] and [`commit_card_append`] place a new card and build
//! record **after every other section**, in the file itself, with no copy.
//! The memory-bounded build uses them to write its full card once the index
//! sections it is derived from are on disk, and `rete recard --in-place` to
//! replace the card of an existing file. The content hash still has to be
//! folded over every hashed section — the card is hashed first, wherever it
//! lies — so the job is one read of the file and a few KB of writes.
//!
//! In place, the order of the writes is what keeps a crash harmless: the new
//! card, build record and footer go **past the end the old header records**,
//! then they are synced, and only then is the 1 KiB header rewritten (and
//! synced). Until that one write lands, every reader still sees the old file
//! — its bytes are never touched, only appended to — and once it lands, the
//! new one. The old card and build record stay where they were, as bytes no
//! header points at.

use std::io::{Seek, SeekFrom, Write};

use crate::file::{content_hash_parts, FileError};
use crate::header::{Header, HEADER_LEN, MAGIC};
use crate::reader::RangeReader;

/// What a card rewrite does to a file's layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRewritePlan {
    /// The new header: every section offset fixed, metadata and build info at
    /// their new lengths. The content hash is still the **old** one —
    /// [`rewrite_card`] writes the new one once it has folded it.
    pub header: Header,
    /// The sections of the old file copied verbatim, as `(old offset,
    /// length)`, in file order — every section the header points at except
    /// the card and the build record. They are written back to back, so bytes
    /// no section points at (a card an in-place re-card left behind, say) are
    /// not copied.
    pub copies: Vec<(u64, u64)>,
    /// How many of [`copies`](Self::copies) come before the new card: the new
    /// card and build record take the old ones' place in the file order —
    /// first on every file the in-RAM writers produce, last on one whose card
    /// was placed after the data.
    pub copies_before_card: usize,
    /// Where the new metadata section starts (build info follows it).
    pub card_offset: u64,
    /// The rewritten file's length, footer included.
    pub new_len: u64,
}

/// Plan replacing the metadata and build-info sections of a file whose first
/// [`HEADER_LEN`] bytes are `head` and whose length is `file_len`, with
/// sections of `metadata_len` and `build_info_len` bytes (`0` = none).
///
/// The card occupies one span — the metadata section, with the build-info
/// section immediately after it (the layout every writer produces, so a card
/// read is one coalesced range). A file with neither gets them right after the
/// header. Every other section keeps its place in the file order and is laid
/// back to back after the header, the new card in the old one's place, then
/// the footer. A section overlapping another (or the card), or a build-info
/// section that is not adjacent to the metadata, is a layout this function
/// will not reshape and is refused rather than guessed at.
pub fn plan_card_rewrite(
    head: &[u8],
    file_len: u64,
    metadata_len: u64,
    build_info_len: u64,
) -> Result<CardRewritePlan, FileError> {
    /// Which header field a live section is.
    #[derive(Clone, Copy)]
    enum Slot {
        Card,
        Dictionary,
        Index,
        Pyramid,
        Named,
        Text,
        Extra(usize),
    }

    let mut header = Header::from_bytes(head)?;
    let (m_off, m_len) = (header.metadata_offset, header.metadata_len);
    let (b_off, b_len) = (header.build_info_offset, header.build_info_len);
    let (start, end) = match (m_len > 0, b_len > 0) {
        (true, true) => {
            if b_off != m_off.saturating_add(m_len) {
                return Err(FileError::Container(
                    "the build-info section is not adjacent to the metadata section",
                ));
            }
            (m_off, b_off.saturating_add(b_len))
        }
        (true, false) => (m_off, m_off.saturating_add(m_len)),
        (false, true) => (b_off, b_off.saturating_add(b_len)),
        (false, false) => (HEADER_LEN as u64, HEADER_LEN as u64),
    };
    if start < HEADER_LEN as u64 || end > file_len {
        return Err(FileError::Container(
            "the card section lies outside the file",
        ));
    }

    let mut live: Vec<(u64, u64, Slot)> = vec![(start, end - start, Slot::Card)];
    let named = [
        (
            header.dictionary_offset,
            header.dictionary_len,
            Slot::Dictionary,
        ),
        (header.root_dir_offset, header.root_dir_len, Slot::Index),
        (
            header.pyramid_meta_offset,
            header.pyramid_meta_len,
            Slot::Pyramid,
        ),
        (
            header.named_graphs_offset,
            header.named_graphs_len,
            Slot::Named,
        ),
        (header.text_index_offset, header.text_index_len, Slot::Text),
    ];
    let extras = header
        .extra_sections
        .iter()
        .enumerate()
        .map(|(i, s)| (s.offset, s.length, Slot::Extra(i)));
    for (off, len, slot) in named.into_iter().chain(extras) {
        if len == 0 {
            continue;
        }
        if off < HEADER_LEN as u64 || off.checked_add(len).is_none_or(|e| e > file_len) {
            return Err(FileError::Container("a section overruns the file"));
        }
        live.push((off, len, slot));
    }
    // File order; an empty card span sorts ahead of a section at its offset.
    live.sort_by_key(|&(off, _, slot)| (off, !matches!(slot, Slot::Card)));
    if live.windows(2).any(|w| w[0].0 + w[0].1 > w[1].0) {
        return Err(FileError::Container(
            "a section overlaps the card or another section; this layout cannot be re-carded",
        ));
    }

    let new_span = metadata_len.saturating_add(build_info_len);
    let mut at = HEADER_LEN as u64;
    let mut copies = Vec::with_capacity(live.len());
    let (mut card_offset, mut copies_before_card) = (at, 0);
    for (off, len, slot) in live {
        let field = match slot {
            Slot::Card => {
                card_offset = at;
                copies_before_card = copies.len();
                at += new_span;
                continue;
            }
            Slot::Dictionary => &mut header.dictionary_offset,
            Slot::Index => &mut header.root_dir_offset,
            Slot::Pyramid => &mut header.pyramid_meta_offset,
            Slot::Named => &mut header.named_graphs_offset,
            Slot::Text => &mut header.text_index_offset,
            Slot::Extra(i) => &mut header.extra_sections[i].offset,
        };
        *field = at;
        copies.push((off, len));
        at += len;
    }

    header.metadata_offset = if metadata_len > 0 { card_offset } else { 0 };
    header.metadata_len = metadata_len;
    header.build_info_offset = if build_info_len > 0 {
        card_offset + metadata_len
    } else {
        0
    };
    header.build_info_len = build_info_len;
    Ok(CardRewritePlan {
        header,
        copies,
        copies_before_card,
        card_offset,
        new_len: at + MAGIC.len() as u64,
    })
}

/// How the rewriter moves bytes: large enough to keep a disk streaming, small
/// enough that the peak memory of a 26 GB rewrite is this and nothing else.
pub const REWRITE_CHUNK: u64 = 8 << 20;

/// Rewrite `src` into `out` with `metadata` as its card and `build_info` as its
/// build record (either may be empty, which removes the section), copying every
/// other section verbatim and folding the new content hash on the way. Returns
/// the header written at offset 0.
///
/// `out` must start empty; it is written front to back, then the header is
/// rewritten once the hash is known (hence `Seek`). `progress` is called with
/// the bytes copied so far after each chunk.
///
/// When the hashed sections do not follow each other in the file (no writer
/// lays them out that way, but a reader cannot assume its writers), the hash
/// is folded in a second pass over `src` instead — the result is the same.
pub fn rewrite_card<R: RangeReader + ?Sized, W: Write + Seek>(
    src: &R,
    out: &mut W,
    metadata: &[u8],
    build_info: &[u8],
    progress: &mut dyn FnMut(u64),
) -> Result<Header, FileError> {
    rewrite_card_impl(src, out, metadata, build_info, progress, false)
}

/// [`rewrite_card`], optionally forcing the second-pass hash — the path no
/// writer's layout reaches, which the tests take on purpose.
fn rewrite_card_impl<R: RangeReader + ?Sized, W: Write + Seek>(
    src: &R,
    out: &mut W,
    metadata: &[u8],
    build_info: &[u8],
    progress: &mut dyn FnMut(u64),
    force_second_pass: bool,
) -> Result<Header, FileError> {
    let file_len = src.len();
    if file_len < HEADER_LEN as u64 {
        return Err(FileError::Container("file shorter than its header"));
    }
    let head = src.read_at(0, HEADER_LEN as u64)?;
    let plan = plan_card_rewrite(
        &head,
        file_len,
        metadata.len() as u64,
        build_info.len() as u64,
    )?;
    let mut header = plan.header.clone();

    // The hashed ranges in **output** coordinates, in hash order. The metadata
    // part is the new card; every other part is unchanged input bytes.
    let parts = content_hash_parts(&header);
    let streamable = !force_second_pass && parts.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0);
    let mut hasher = blake3::Hasher::new();
    let feed = HashFeed {
        parts: &parts,
        next: 0,
    };

    out.write_all(&header.to_bytes())?;
    let mut sink = Sink {
        out: &mut *out,
        written: HEADER_LEN as u64,
        streamable,
        feed,
        hasher: &mut hasher,
    };
    let mut copied = 0u64;
    for i in 0..=plan.copies.len() {
        if i == plan.copies_before_card {
            sink.write(metadata)?;
            sink.write(build_info)?;
        }
        let Some(&(from, len)) = plan.copies.get(i) else {
            break;
        };
        let mut at = from;
        while at < from + len {
            let n = (from + len - at).min(REWRITE_CHUNK);
            let buf = src.read_at(at, n)?;
            if buf.len() as u64 != n {
                return Err(FileError::Container("short read while copying"));
            }
            sink.write(&buf)?;
            at += n;
            copied += n;
            progress(copied);
        }
    }
    sink.write(&MAGIC)?;
    let (written, folded) = (sink.written, sink.feed.next);
    if written != plan.new_len {
        return Err(FileError::Container(
            "the rewrite came out the wrong length",
        ));
    }

    if !streamable || folded < parts.len() {
        // Second pass, in hash order: the new card, then every other hashed
        // section from where it lives in the old file.
        hasher = blake3::Hasher::new();
        hasher.update(metadata);
        let mut old = Header::from_bytes(&head)?;
        old.metadata_len = 0;
        for (off, len) in content_hash_parts(&old) {
            let mut at = off;
            while at < off + len {
                let n = (off + len - at).min(REWRITE_CHUNK);
                hasher.update(&src.read_at(at, n)?);
                at += n;
            }
        }
    }
    header
        .content_hash
        .copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    out.seek(SeekFrom::Start(0))?;
    out.write_all(&header.to_bytes())?;
    out.flush()?;
    Ok(header)
}

/// What placing a new card after a file's data does to its layout — see "The
/// card after the data" in the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardAppendPlan {
    /// The new header: the metadata section at [`card_offset`](Self::card_offset),
    /// no build-info section yet (its length is only known when the record is
    /// sealed, after the hash — [`commit_card_append`] fills it), every other
    /// section where it was, and still the **old** content hash.
    pub header: Header,
    /// Where the new card starts: the file's end as its header records it
    /// (in place — past the old footer, so the old file stays whole), or the
    /// old footer itself (a fresh file nobody has read yet, so no gap).
    pub card_offset: u64,
    /// The file's length as its own header records it
    /// ([`Header::expected_file_len`]).
    pub old_len: u64,
    /// The bytes no section points at once the plan is committed: the old card
    /// and build record, and — in place — the old 4-byte footer.
    pub dead_bytes: u64,
    /// The header bytes the plan was made from. [`commit_card_append`]
    /// refuses a file whose header no longer reads exactly this.
    pub old_head: Vec<u8>,
}

/// Plan placing a card of `metadata_len` bytes (`0` = none) after the data of
/// the file whose first [`HEADER_LEN`] bytes are `head` and whose length on
/// disk is `file_len`.
///
/// `in_place` is for a file readers may already hold: the card goes past the
/// old footer, which is never overwritten, so an interrupted run leaves the
/// old file readable by every reader, the remote ones that confirm a length by
/// its footer included. Without it — a file this process has just written —
/// the card replaces the footer and the layout has no gap.
///
/// Bytes on disk past the end the header records are not part of the file:
/// they are what an interrupted in-place run leaves, and they are overwritten.
/// A file **shorter** than its header records is refused.
pub fn plan_card_append(
    head: &[u8],
    file_len: u64,
    metadata_len: u64,
    in_place: bool,
) -> Result<CardAppendPlan, FileError> {
    let mut header = Header::from_bytes(head)?;
    let old_len = header.expected_file_len().ok_or(FileError::Container(
        "the header's section directory overflows",
    ))?;
    if old_len > file_len {
        return Err(FileError::Container(
            "the file is shorter than its header records (truncated?)",
        ));
    }
    let footer = MAGIC.len() as u64;
    let card_offset = if in_place { old_len } else { old_len - footer };
    let dead_bytes = header
        .metadata_len
        .saturating_add(header.build_info_len)
        .saturating_add(if in_place { footer } else { 0 });
    if metadata_len > 0 {
        header.metadata_offset = card_offset;
        header.metadata_len = metadata_len;
    } else {
        header.metadata_offset = 0;
        header.metadata_len = 0;
    }
    header.build_info_offset = 0;
    header.build_info_len = 0;
    Ok(CardAppendPlan {
        header,
        card_offset,
        old_len,
        dead_bytes,
        old_head: head[..HEADER_LEN].to_vec(),
    })
}

/// Fold the content hash the file will have once `plan` is committed with
/// `metadata` as its card: the card from memory, every other hashed section
/// read from `src` (the file as the plan found it) in [`REWRITE_CHUNK`]
/// slices. `progress` gets the bytes read so far. The build record does not
/// take part — it is outside the hash — which is what lets a caller seal it
/// *after* this, its longest step, and still commit it in the same write.
///
/// The file must still end in its footer where its header says it does; a
/// file that does not is not one this plan describes.
pub fn card_append_hash<R: RangeReader + ?Sized>(
    src: &R,
    plan: &CardAppendPlan,
    metadata: &[u8],
    progress: &mut dyn FnMut(u64),
) -> Result<[u8; 16], FileError> {
    if metadata.len() as u64 != plan.header.metadata_len {
        return Err(FileError::Container(
            "the card is not the length the plan was made for",
        ));
    }
    let footer = MAGIC.len() as u64;
    if src.len() < plan.old_len || src.read_at(plan.old_len - footer, footer)? != MAGIC {
        return Err(FileError::Container(
            "the file does not end in its footer where its header says",
        ));
    }
    let mut hasher = blake3::Hasher::new();
    let mut read = 0u64;
    for (i, (off, len)) in content_hash_parts(&plan.header).into_iter().enumerate() {
        if i == 0 && !metadata.is_empty() {
            // `content_hash_parts` lists the metadata section first.
            hasher.update(metadata);
            continue;
        }
        // Every other part is a section of the old file, which ends at its
        // footer; nothing hashed can reach the new card.
        if off
            .checked_add(len)
            .is_none_or(|end| end > plan.old_len - footer)
        {
            return Err(FileError::Container("a hashed section overruns the file"));
        }
        let mut at = off;
        while at < off + len {
            let n = (off + len - at).min(REWRITE_CHUNK);
            let buf = src.read_at(at, n)?;
            if buf.len() as u64 != n {
                return Err(FileError::Container("short read while hashing"));
            }
            hasher.update(&buf);
            at += n;
            read += n;
            progress(read);
        }
    }
    let mut hash = [0u8; 16];
    hash.copy_from_slice(&hasher.finalize().as_bytes()[..16]);
    Ok(hash)
}

/// Commit `plan`: write `metadata`, then `build_info` (outside the hash, so it
/// may be sealed after [`card_append_hash`]), then a footer at the plan's
/// card offset, cut the file there, **sync**, and only then rewrite the 1 KiB
/// header with `hash` and the new section entries, and sync again. Returns the
/// header written.
///
/// Refuses a file whose header is no longer the one the plan was made from
/// (another writer got there first). In place, nothing the old header points
/// at is written before the new header lands: a crash before that leaves the
/// old file, plus bytes past its end that the next run overwrites; a crash
/// after it, the new file. The header is a single 1 KiB write at offset 0,
/// inside the file's first page.
#[cfg(not(target_arch = "wasm32"))]
pub fn commit_card_append(
    file: &mut std::fs::File,
    plan: &CardAppendPlan,
    hash: [u8; 16],
    metadata: &[u8],
    build_info: &[u8],
) -> Result<Header, FileError> {
    commit_card_append_impl(file, plan, hash, metadata, build_info, false)
}

/// [`commit_card_append`], optionally failing just before the header is
/// written — the crash the tests simulate.
#[cfg(not(target_arch = "wasm32"))]
fn commit_card_append_impl(
    file: &mut std::fs::File,
    plan: &CardAppendPlan,
    hash: [u8; 16],
    metadata: &[u8],
    build_info: &[u8],
    fail_before_header: bool,
) -> Result<Header, FileError> {
    use std::io::Read;
    if metadata.len() as u64 != plan.header.metadata_len {
        return Err(FileError::Container(
            "the card is not the length the plan was made for",
        ));
    }
    let mut head = vec![0u8; HEADER_LEN];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    if head != plan.old_head || file.metadata()?.len() < plan.old_len {
        return Err(FileError::Container(
            "the file changed since the card was planned; nothing was written",
        ));
    }
    let mut header = plan.header.clone();
    header.content_hash = hash;
    let info_offset = plan.card_offset + metadata.len() as u64;
    if !build_info.is_empty() {
        header.build_info_offset = info_offset;
        header.build_info_len = build_info.len() as u64;
    }
    let end = info_offset + build_info.len() as u64 + MAGIC.len() as u64;
    debug_assert_eq!(header.expected_file_len(), Some(end));

    // 1. The new sections and the footer, past (in place) or over (fresh) the
    //    old footer — never over a byte the old header points at.
    file.seek(SeekFrom::Start(plan.card_offset))?;
    let mut tail = Vec::with_capacity((end - plan.card_offset) as usize);
    tail.extend_from_slice(metadata);
    tail.extend_from_slice(build_info);
    tail.extend_from_slice(&MAGIC);
    file.write_all(&tail)?;
    file.set_len(end)?;
    file.sync_all()?;
    if fail_before_header {
        return Err(FileError::Container("simulated crash before the header"));
    }
    // 2. The header, last.
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header.to_bytes())?;
    file.sync_all()?;
    Ok(header)
}

/// The output side of a rewrite: writes go through here so the hash folds
/// exactly the bytes that land in the file, at the offsets they land at.
struct Sink<'a, 'p, W> {
    out: &'a mut W,
    written: u64,
    streamable: bool,
    feed: HashFeed<'p>,
    hasher: &'a mut blake3::Hasher,
}

impl<W: Write> Sink<'_, '_, W> {
    fn write(&mut self, buf: &[u8]) -> Result<(), FileError> {
        self.out.write_all(buf)?;
        if self.streamable {
            self.feed.fold(self.written, buf, self.hasher);
        }
        self.written += buf.len() as u64;
        Ok(())
    }
}

/// Folds the bytes of a sequential write into the hash, for the parts of
/// `parts` (output coordinates, hash order, ascending) the write overlaps.
struct HashFeed<'a> {
    parts: &'a [(u64, u64)],
    next: usize,
}

impl HashFeed<'_> {
    fn fold(&mut self, at: u64, buf: &[u8], hasher: &mut blake3::Hasher) {
        let end = at + buf.len() as u64;
        while let Some(&(off, len)) = self.parts.get(self.next) {
            let part_end = off + len;
            if off >= end {
                return;
            }
            let lo = off.max(at);
            let hi = part_end.min(end);
            if lo < hi {
                hasher.update(&buf[(lo - at) as usize..(hi - at) as usize]);
            }
            if part_end <= end {
                self.next += 1;
            } else {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{derive_card, CardInput};
    use crate::reader::SliceReader;

    fn q(s: &str, p: &str, o: &str) -> (String, String, String, Option<String>) {
        (s.into(), p.into(), o.into(), None)
    }

    fn quads() -> Vec<(String, String, String, Option<String>)> {
        let ty = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
        let mut v = Vec::new();
        for i in 0..20 {
            let s = format!("<http://ex/item/{i}>");
            v.push(q(&s, ty, "<http://ex/Item>"));
            v.push(q(
                &s,
                "<http://ex/next>",
                &format!("<http://ex/item/{}>", (i + 1) % 20),
            ));
            v.push(q(&s, "<http://ex/name>", &format!("\"item {i}\"")));
        }
        v.push((
            "<http://ex/a>".into(),
            "<http://ex/p>".into(),
            "<http://ex/b>".into(),
            Some("<http://ex/g>".into()),
        ));
        v
    }

    /// A carded file, with a build record after the card like every writer
    /// lays them out.
    fn carded(text_index: bool) -> Vec<u8> {
        let quads = quads();
        let card = derive_card(&quads, 50, 1, CardInput::default()).to_json_bytes();
        let (bytes, _) = crate::ingest::assemble_dataset_with_opts(
            quads,
            true,
            text_index,
            None,
            move |_, _| card.clone(),
        );
        crate::format::attach_build_info(&bytes, br#"{"schema":1,"builder":"old"}"#).unwrap()
    }

    fn rewrite(src: &[u8], meta: &[u8], info: &[u8]) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        rewrite_card(&SliceReader::new(src), &mut out, meta, info, &mut |_| {}).unwrap();
        out.into_inner()
    }

    /// Every section other than the card and the build record comes through
    /// byte for byte, the hash is the one `verify` recomputes, and the result
    /// equals the in-memory splice the build itself uses.
    #[test]
    fn a_rewrite_copies_every_other_section_and_rehashes() {
        for text_index in [false, true] {
            let src = carded(text_index);
            let old = Header::from_bytes(&src).unwrap();
            let meta = br#"{"title":"new card","triple_count":60,"quad_count":61,"named_graph_count":1,"term_count":50,"format_version":5}"#;
            let info = br#"{"schema":1,"builder":"recard"}"#;
            let out = rewrite(&src, meta, info);
            let new = Header::from_bytes(&out).unwrap();
            assert!(
                crate::format::verify(&out).unwrap(),
                "the new hash verifies"
            );
            assert_ne!(
                new.content_hash, old.content_hash,
                "the card is inside the hash"
            );
            let sec = |b: &[u8], off: u64, len: u64| b[off as usize..(off + len) as usize].to_vec();
            assert_eq!(
                sec(&out, new.metadata_offset, new.metadata_len),
                meta.to_vec()
            );
            assert_eq!(
                sec(&out, new.build_info_offset, new.build_info_len),
                info.to_vec()
            );
            for (o, n) in [
                (
                    (old.dictionary_offset, old.dictionary_len),
                    (new.dictionary_offset, new.dictionary_len),
                ),
                (
                    (old.root_dir_offset, old.root_dir_len),
                    (new.root_dir_offset, new.root_dir_len),
                ),
                (
                    (old.named_graphs_offset, old.named_graphs_len),
                    (new.named_graphs_offset, new.named_graphs_len),
                ),
                (
                    (old.text_index_offset, old.text_index_len),
                    (new.text_index_offset, new.text_index_len),
                ),
            ] {
                assert_eq!(o.1, n.1);
                assert_eq!(sec(&src, o.0, o.1), sec(&out, n.0, n.1));
            }
            // The footer survives too.
            assert_eq!(&out[out.len() - 4..], &src[src.len() - 4..]);
            // Same bytes as the build's own splice: replace the metadata, then
            // the build info.
            let spliced = crate::format::attach_build_info(
                &crate::format::replace_metadata(&src, meta).unwrap(),
                info,
            )
            .unwrap();
            assert_eq!(out, spliced);
            // And the file still answers queries.
            let rete = crate::Rete::open(&out).unwrap();
            assert_eq!(rete.dump(None).len(), 60);
        }
    }

    /// The second-pass hash — reading each part back from the source in hash
    /// order — lands on exactly the bytes and the hash the streamed fold does,
    /// for a growing and for a shrinking card.
    #[test]
    fn the_second_pass_hash_agrees_with_the_streamed_one() {
        let src = carded(false);
        for meta in [&b"{}"[..], &[b'x'; 5000][..]] {
            let streamed = rewrite(&src, meta, b"{}");
            let mut out = std::io::Cursor::new(Vec::new());
            rewrite_card_impl(
                &SliceReader::new(&src),
                &mut out,
                meta,
                b"{}",
                &mut |_| {},
                true,
            )
            .unwrap();
            assert_eq!(out.into_inner(), streamed);
        }
    }

    /// A smaller card and no build record shrink the file; removing the card
    /// entirely leaves a valid cardless file.
    #[test]
    fn a_rewrite_can_shrink_and_remove() {
        let src = carded(false);
        let out = rewrite(&src, b"{}", b"");
        let h = Header::from_bytes(&out).unwrap();
        assert_eq!(h.build_info_len, 0);
        assert!(crate::format::verify(&out).unwrap());
        let out = rewrite(&src, b"", b"");
        let h = Header::from_bytes(&out).unwrap();
        assert_eq!((h.metadata_len, h.build_info_len), (0, 0));
        assert!(crate::format::verify(&out).unwrap());
        assert!(crate::Rete::open(&out).is_ok());
    }

    /// A cardless file gains its card right after the header.
    #[test]
    fn a_cardless_file_gains_a_card_after_the_header() {
        let (src, _) =
            crate::ingest::assemble_dataset_with_opts(quads(), true, false, None, |_, _| {
                Vec::new()
            });
        let out = rewrite(&src, br#"{"title":"t"}"#, br#"{"schema":1}"#);
        let h = Header::from_bytes(&out).unwrap();
        assert_eq!(h.metadata_offset, HEADER_LEN as u64);
        assert_eq!(h.build_info_offset, HEADER_LEN as u64 + h.metadata_len);
        assert!(crate::format::verify(&out).unwrap());
    }

    /// A build record that does not follow the card is a layout the plan
    /// refuses rather than reshapes.
    #[test]
    fn a_detached_build_record_is_refused() {
        let src = carded(false);
        let mut h = Header::from_bytes(&src).unwrap();
        h.build_info_offset += 1;
        let err = plan_card_rewrite(&h.to_bytes(), src.len() as u64, 10, 10).unwrap_err();
        assert!(err.to_string().contains("not adjacent"), "{err}");
    }

    /// A scratch file holding `bytes`, removed on drop.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new(tag: &str, bytes: &[u8]) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "rete-recard-{tag}-{}-{}.rete",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&path, bytes).unwrap();
            Scratch(path)
        }

        fn bytes(&self) -> Vec<u8> {
            std::fs::read(&self.0).unwrap()
        }

        fn open(&self) -> std::fs::File {
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.0)
                .unwrap()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Plan, hash and commit a card after the data of `file` — the whole
    /// in-place sequence, as the CLI runs it.
    fn append(file: &Scratch, meta: &[u8], info: &[u8], in_place: bool) -> Header {
        let bytes = file.bytes();
        let plan =
            plan_card_append(&bytes, bytes.len() as u64, meta.len() as u64, in_place).unwrap();
        let hash = card_append_hash(&SliceReader::new(&bytes), &plan, meta, &mut |_| {}).unwrap();
        commit_card_append(&mut file.open(), &plan, hash, meta, info).unwrap()
    }

    const META: &[u8] = br#"{"title":"after the data","triple_count":60,"quad_count":61,"named_graph_count":1,"term_count":50,"format_version":5}"#;
    const INFO: &[u8] = br#"{"schema":1,"builder":"in place"}"#;

    /// In place: every data section stays at its offset, byte for byte; the
    /// card and the build record land after the old footer, adjacent; the
    /// hash is the one a copying re-card computes for the same card (the hash
    /// does not depend on where the card lies), and `verify` agrees.
    #[test]
    fn a_card_appended_in_place_moves_nothing_and_verifies() {
        for text_index in [false, true] {
            let src = carded(text_index);
            let old = Header::from_bytes(&src).unwrap();
            let file = Scratch::new("inplace", &src);
            let new = append(&file, META, INFO, true);
            let out = file.bytes();

            assert_eq!(
                new.metadata_offset,
                src.len() as u64,
                "after the old footer"
            );
            assert_eq!(
                new.build_info_offset,
                new.metadata_offset + META.len() as u64
            );
            assert_eq!(new.expected_file_len(), Some(out.len() as u64));
            assert_eq!(&out[out.len() - 4..], &MAGIC);
            assert_eq!(
                &out[..],
                &{
                    let mut want = src[..].to_vec();
                    want[..HEADER_LEN].copy_from_slice(&new.to_bytes());
                    want.extend_from_slice(META);
                    want.extend_from_slice(INFO);
                    want.extend_from_slice(&MAGIC);
                    want
                }[..]
            );
            for (o, n) in [
                (old.dictionary_offset, new.dictionary_offset),
                (old.root_dir_offset, new.root_dir_offset),
                (old.named_graphs_offset, new.named_graphs_offset),
                (old.text_index_offset, new.text_index_offset),
            ] {
                assert_eq!(o, n, "a data section moved");
            }
            assert!(crate::format::verify(&out).unwrap());
            let copied = rewrite(&src, META, INFO);
            assert_eq!(
                new.content_hash,
                Header::from_bytes(&copied).unwrap().content_hash,
                "the hash does not depend on where the card lies"
            );
            let (m, b) =
                crate::file::read_card_and_build_info_ranged(&SliceReader::new(&out)).unwrap();
            assert_eq!((m.unwrap(), b.unwrap()), (META.to_vec(), INFO.to_vec()));
            let rete = crate::Rete::open(&out).unwrap();
            assert_eq!(rete.metadata(), Some(META));
            assert_eq!(rete.dump(None).len(), 60);
        }
    }

    /// A crash after the new sections are written but before the header is:
    /// the file is the old one — same header, same card, same hash, still
    /// verifying and answering — plus bytes past its recorded end, which the
    /// next run overwrites and cuts.
    #[test]
    fn a_crash_before_the_header_leaves_the_old_file() {
        let src = carded(false);
        let file = Scratch::new("crash", &src);
        let plan = plan_card_append(&src, src.len() as u64, META.len() as u64, true).unwrap();
        let hash = card_append_hash(&SliceReader::new(&src), &plan, META, &mut |_| {}).unwrap();
        let err =
            commit_card_append_impl(&mut file.open(), &plan, hash, META, INFO, true).unwrap_err();
        assert!(err.to_string().contains("simulated"), "{err}");

        let crashed = file.bytes();
        assert!(crashed.len() > src.len(), "the new sections were written");
        assert_eq!(&crashed[..src.len()], &src[..], "the old file is untouched");
        assert!(crate::format::verify(&crashed).unwrap());
        let rete = crate::Rete::open(&crashed).unwrap();
        assert_eq!(
            rete.metadata(),
            crate::Rete::open(&src).unwrap().metadata(),
            "the old card"
        );
        assert_eq!(rete.dump(None).len(), 60);

        // The rerun overwrites the leftover and lands on the file a clean run
        // writes.
        let rerun = append(&file, META, INFO, true);
        let clean = Scratch::new("clean", &src);
        append(&clean, META, INFO, true);
        assert_eq!(file.bytes(), clean.bytes());
        assert!(crate::format::verify(&file.bytes()).unwrap());
        assert_eq!(rerun.expected_file_len(), Some(file.bytes().len() as u64));
    }

    /// A fresh file — what the memory-bounded build appends to — gets its card
    /// over its old footer: no gap, no dead bytes, and the same hash a card at
    /// the front would give it.
    #[test]
    fn a_fresh_append_replaces_the_footer() {
        let (src, _) =
            crate::ingest::assemble_dataset_with_opts(quads(), true, false, None, |_, _| {
                Vec::new()
            });
        let file = Scratch::new("fresh", &src);
        let plan = plan_card_append(&src, src.len() as u64, META.len() as u64, false).unwrap();
        assert_eq!(plan.dead_bytes, 0);
        let h = append(&file, META, INFO, false);
        let out = file.bytes();
        assert_eq!(h.metadata_offset, src.len() as u64 - 4);
        assert_eq!(out.len(), src.len() + META.len() + INFO.len());
        assert!(crate::format::verify(&out).unwrap());
        // The card at the front, through the copying rewrite: same identity.
        assert_eq!(
            Header::from_bytes(&rewrite(&src, META, INFO))
                .unwrap()
                .content_hash,
            h.content_hash
        );
    }

    /// A card already after the data is replaced in place again (the old one
    /// becomes dead bytes too), re-carded by copy (it stays at the end), and
    /// given a new build record by the build-info splice — each keeping the
    /// file valid.
    #[test]
    fn a_card_after_the_data_can_be_replaced_again_every_way() {
        let src = carded(true);
        let file = Scratch::new("again", &src);
        append(&file, META, INFO, true);
        let once = file.bytes();
        let plan = plan_card_append(&once, once.len() as u64, 2, true).unwrap();
        assert_eq!(
            plan.dead_bytes,
            (META.len() + INFO.len() + 4) as u64,
            "the first in-place card, its record and the footer before it"
        );
        append(&file, b"{}", b"{\"schema\":1}", true);
        assert!(crate::format::verify(&file.bytes()).unwrap());

        // A copying re-card keeps the card where it is — after the data — and
        // drops the bytes the in-place one left behind.
        let meta2 = br#"{"title":"copied"}"#;
        let copied = rewrite(&once, meta2, INFO);
        let h = Header::from_bytes(&copied).unwrap();
        assert!(h.metadata_offset > h.dictionary_offset);
        let old = Header::from_bytes(&src).unwrap();
        assert_eq!(
            copied.len() as u64,
            src.len() as u64 - old.metadata_len - old.build_info_len
                + (meta2.len() + INFO.len()) as u64
        );
        assert_eq!(h.expected_file_len(), Some(copied.len() as u64));
        assert!(crate::format::verify(&copied).unwrap());
        assert_eq!(crate::Rete::open(&copied).unwrap().dump(None).len(), 60);
        // Its data sections are the original's, back to back from the header.
        assert_eq!(h.dictionary_offset, HEADER_LEN as u64);
        let sec = |b: &[u8], off: u64, len: u64| b[off as usize..(off + len) as usize].to_vec();
        assert_eq!(
            sec(&copied, h.root_dir_offset, h.root_dir_len),
            sec(&src, old.root_dir_offset, old.root_dir_len)
        );

        // A new build record goes right after the card, wherever it is.
        let swapped = crate::format::attach_build_info(&once, b"{\"schema\":1,\"x\":2}").unwrap();
        let h = Header::from_bytes(&swapped).unwrap();
        assert_eq!(h.build_info_offset, h.metadata_offset + h.metadata_len);
        assert_eq!(h.expected_file_len(), Some(swapped.len() as u64));
        assert!(crate::format::verify(&swapped).unwrap());
    }

    /// Refusals: a truncated file, a file whose footer is not where its header
    /// says, and a file that changed between the plan and the commit.
    #[test]
    fn an_append_refuses_what_it_cannot_vouch_for() {
        let src = carded(false);
        let err = plan_card_append(&src, src.len() as u64 - 1, 10, true).unwrap_err();
        assert!(err.to_string().contains("shorter"), "{err}");

        let mut broken = src.clone();
        let n = broken.len();
        broken[n - 1] = b'X';
        let plan = plan_card_append(&broken, n as u64, META.len() as u64, true).unwrap();
        let err =
            card_append_hash(&SliceReader::new(&broken), &plan, META, &mut |_| {}).unwrap_err();
        assert!(err.to_string().contains("footer"), "{err}");

        let file = Scratch::new("changed", &src);
        let plan = plan_card_append(&src, src.len() as u64, META.len() as u64, true).unwrap();
        let hash = card_append_hash(&SliceReader::new(&src), &plan, META, &mut |_| {}).unwrap();
        append(&file, b"{}", b"", true); // another writer got there first
        let before = file.bytes();
        let err = commit_card_append(&mut file.open(), &plan, hash, META, INFO).unwrap_err();
        assert!(err.to_string().contains("changed"), "{err}");
        assert_eq!(file.bytes(), before, "nothing was written");
    }

    /// The streaming hash fold is exercised with a tiny chunk boundary by the
    /// feed directly: parts split across arbitrary write boundaries.
    #[test]
    fn the_hash_feed_folds_parts_across_write_boundaries() {
        let data: Vec<u8> = (0..200u8).collect();
        let parts = [(10u64, 30u64), (40, 5), (100, 50)];
        let mut want = blake3::Hasher::new();
        for &(o, l) in &parts {
            want.update(&data[o as usize..(o + l) as usize]);
        }
        let mut got = blake3::Hasher::new();
        let mut feed = HashFeed {
            parts: &parts,
            next: 0,
        };
        for (i, chunk) in data.chunks(7).enumerate() {
            feed.fold(i as u64 * 7, chunk, &mut got);
        }
        assert_eq!(want.finalize(), got.finalize());
        assert_eq!(feed.next, parts.len());
    }
}
