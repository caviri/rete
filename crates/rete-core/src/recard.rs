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
//!   for byte**; only its offset in the header moves;
//! * the content hash is recomputed, because the card is inside it
//!   ([`crate::format::content_hash_parts`]); it is folded while the bytes go
//!   past, so the whole job is one read and one write of the file.
//!
//! [`plan_card_rewrite`] is the arithmetic, kept apart (like
//! [`crate::format::plan_build_info`]) so it is testable without a file;
//! [`rewrite_card`] is the streaming copy.

use std::io::{Seek, SeekFrom, Write};

use crate::file::{content_hash_parts, FileError};
use crate::header::{Header, HEADER_LEN};
use crate::reader::RangeReader;

/// What a card rewrite does to a file's layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardRewritePlan {
    /// The new header: every section offset fixed, metadata and build info at
    /// their new lengths. The content hash is still the **old** one —
    /// [`rewrite_card`] writes the new one once it has folded it.
    pub header: Header,
    /// The span of the old file copied verbatim **before** the new card,
    /// `[HEADER_LEN, card_start)` — empty on every file the writers produce;
    /// non-empty when a section was laid out ahead of the card.
    pub before: (u64, u64),
    /// The span of the old file copied verbatim **after** the new card,
    /// `[card_end, file_len)`.
    pub after: (u64, u64),
    /// Where the new metadata section starts (build info follows it).
    pub card_offset: u64,
    /// The rewritten file's length.
    pub new_len: u64,
}

/// Plan replacing the metadata and build-info sections of a file whose first
/// [`HEADER_LEN`] bytes are `head` and whose length is `file_len`, with
/// sections of `metadata_len` and `build_info_len` bytes (`0` = none).
///
/// The card occupies one span — the metadata section, with the build-info
/// section immediately after it (the layout every writer produces, so a card
/// read is one coalesced range). A file with neither gets them right after the
/// header. Sections after the span shift by the length delta; sections before
/// it stay where they are; a section overlapping it, or a build-info section
/// that is not adjacent to the metadata, is a layout this function will not
/// reshape and is refused rather than guessed at.
pub fn plan_card_rewrite(
    head: &[u8],
    file_len: u64,
    metadata_len: u64,
    build_info_len: u64,
) -> Result<CardRewritePlan, FileError> {
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
    let new_span = metadata_len.saturating_add(build_info_len);
    let shift = |off: u64| -> u64 { off - (end - start) + new_span };

    // Every other section must lie wholly before or wholly after the span.
    let check = |off: &mut u64, len: u64| -> Result<(), FileError> {
        if len == 0 {
            return Ok(());
        }
        let sec_end = off.saturating_add(len);
        if sec_end > file_len {
            return Err(FileError::Container("a section overruns the file"));
        }
        if sec_end <= start {
            Ok(())
        } else if *off >= end {
            *off = shift(*off);
            Ok(())
        } else {
            Err(FileError::Container(
                "a section overlaps the card; this layout cannot be re-carded in place",
            ))
        }
    };
    check(&mut header.dictionary_offset, header.dictionary_len)?;
    check(&mut header.root_dir_offset, header.root_dir_len)?;
    check(&mut header.pyramid_meta_offset, header.pyramid_meta_len)?;
    check(&mut header.named_graphs_offset, header.named_graphs_len)?;
    check(&mut header.text_index_offset, header.text_index_len)?;
    for s in &mut header.extra_sections {
        check(&mut s.offset, s.length)?;
    }

    header.metadata_offset = if metadata_len > 0 { start } else { 0 };
    header.metadata_len = metadata_len;
    header.build_info_offset = if build_info_len > 0 {
        start + metadata_len
    } else {
        0
    };
    header.build_info_len = build_info_len;
    Ok(CardRewritePlan {
        header,
        before: (HEADER_LEN as u64, start),
        after: (end, file_len),
        card_offset: start,
        new_len: file_len - (end - start) + new_span,
    })
}

/// How the rewriter moves bytes: large enough to keep a disk streaming, small
/// enough that the peak memory of a 26 GB rewrite is this and nothing else.
pub const REWRITE_CHUNK: u64 = 8 << 20;

/// Rewrite `src` into `out` with `metadata` as its card and `build_info` as its
/// build record (either may be empty, which removes the section), copying every
/// other byte verbatim and folding the new content hash on the way. Returns
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
    let streamable = !force_second_pass
        && parts.windows(2).all(|w| w[0].0 + w[0].1 <= w[1].0)
        && parts
            .iter()
            .all(|&(off, len)| off >= plan.card_offset || off + len <= plan.card_offset);
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
    for (from, to, then) in [
        (plan.before.0, plan.before.1, Some([metadata, build_info])),
        (plan.after.0, plan.after.1, None),
    ] {
        let mut at = from;
        while at < to {
            let n = (to - at).min(REWRITE_CHUNK);
            let buf = src.read_at(at, n)?;
            if buf.len() as u64 != n {
                return Err(FileError::Container("short read while copying"));
            }
            sink.write(&buf)?;
            at += n;
            copied += n;
            progress(copied);
        }
        for section in then.into_iter().flatten() {
            sink.write(section)?;
        }
    }
    let (written, folded) = (sink.written, sink.feed.next);
    if written != plan.new_len {
        return Err(FileError::Container(
            "the rewrite came out the wrong length",
        ));
    }

    if !streamable || folded < parts.len() {
        // Second pass: each part from where it lives in the old file (or, for
        // the metadata part, the new card itself).
        hasher = blake3::Hasher::new();
        // Sections after the card moved by `new_len - file_len`; undo that.
        // (Added before subtracting: the difference can be negative.)
        let delta = |off: u64| -> u64 {
            if off >= plan.card_offset + metadata.len() as u64 + build_info.len() as u64 {
                off + file_len - plan.new_len
            } else {
                off
            }
        };
        for &(off, len) in &parts {
            if metadata.len() as u64 == len && off == plan.card_offset && !metadata.is_empty() {
                hasher.update(metadata);
                continue;
            }
            let mut at = delta(off);
            let end = at + len;
            while at < end {
                let n = (end - at).min(REWRITE_CHUNK);
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
