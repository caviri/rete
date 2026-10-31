//! Reading a `0x06` file: the header and the root come with the first read;
//! everything else is built lazily from them — the dictionary and the index as
//! two-level directories whose leaves fault on demand and stay for the session,
//! the text index likewise, the label index and the quad index through their
//! own lazy machinery.

use std::sync::Arc;

use super::*;
use crate::dict::{ChunkLeaf, ChunkedSection, LeafedChunks, SectionMeta};
use crate::dictionary::Dictionary;
use crate::file::{decompress, ByteRange, FileError};
use crate::index::{GraphIndex, LeafFetch, LeafedTiles, TileLeaf, NUM_PERMS};
use crate::reader::RangeReader;

/// Coalescing gaps for the batched faults (as for `0x05`).
const TILE_GAP: u64 = 4096;
const DICT_GAP: u64 = 64 * 1024;

/// An open `0x06` file: its header and root, and the reader.
pub struct V06File {
    /// The 128-byte header.
    pub h: Header06,
    /// The decoded root block.
    pub root: Root06,
    /// Where everything else is read from.
    pub reader: Arc<dyn RangeReader + Send + Sync>,
}

/// Read `(offset, len)` ranges as one coalesced batch.
fn read_coalesced(
    r: &(dyn RangeReader + Send + Sync),
    ranges: &[(u64, u64)],
    gap: u64,
) -> Option<Vec<Vec<u8>>> {
    let rs: Vec<ByteRange> = ranges
        .iter()
        .map(|&(offset, len)| ByteRange { offset, len })
        .collect();
    crate::file::read_coalesced(r, &rs, gap)
}

impl V06File {
    /// Open from `first`, the file's leading bytes (at least the header; the
    /// root is read from it when it fits, else fetched).
    pub fn open(
        reader: Arc<dyn RangeReader + Send + Sync>,
        first: &[u8],
    ) -> Result<Self, FileError> {
        let h = Header06::from_bytes(first)?;
        if h.file_len != reader.len() {
            return Err(FileError::Container(
                "0x06 file length does not match the header (truncated?)",
            ));
        }
        let rs = HEADER_LEN;
        let re = rs + h.root_len as usize;
        let root_bytes = if re <= first.len() {
            first[rs..re].to_vec()
        } else {
            reader.read_at(rs as u64, h.root_len as u64)?
        };
        if digest16(&root_bytes) != h.root_digest {
            return Err(FileError::Container("0x06 root digest mismatch"));
        }
        let root = Root06::decode(&root_bytes, h.internal_codec)?;
        for s in &root.sections {
            let a = s.abs(&h);
            let end = a
                .offset
                .checked_add(a.len)
                .ok_or(FileError::Container("0x06 section overflows"))?;
            let ok = if kind::is_front(s.kind) {
                a.offset >= h.front_base() && end <= h.data_offset
            } else {
                a.offset >= h.data_offset && end <= h.file_len - FOOTER.len() as u64
            };
            if !ok {
                return Err(FileError::Container("0x06 section out of its region"));
            }
        }
        Ok(V06File { h, root, reader })
    }

    /// The absolute range of section `k`, if present.
    pub fn section(&self, k: u8) -> Option<ByteRange> {
        self.root.section(k).map(|s| s.abs(&self.h))
    }

    /// A batched leaf fetcher over this file's reader (leaves are stored with
    /// the internal codec; decoding happens in the directory types).
    pub fn fetch(&self) -> LeafFetch {
        let r = self.reader.clone();
        Arc::new(move |ranges: &[(u64, u64)]| read_coalesced(&*r, ranges, 0))
    }

    /// The Dataset Card (decompressed), if the file has one.
    pub fn card(&self) -> Result<Option<Vec<u8>>, FileError> {
        let r = self.h.card_range();
        if r.len == 0 {
            return Ok(None);
        }
        let stored = self.reader.read_at(r.offset, r.len)?;
        if digest16(&stored) != self.h.card_digest {
            return Err(FileError::Container("0x06 card digest mismatch"));
        }
        Ok(Some(decompress(self.h.internal_codec, &stored)?))
    }

    /// The build record (raw), if the file has one.
    pub fn build_record(&self) -> Result<Option<Vec<u8>>, FileError> {
        let r = self.h.build_range();
        if r.len == 0 {
            return Ok(None);
        }
        Ok(Some(self.reader.read_at(r.offset, r.len)?))
    }

    /// The stats + schema section, as the `0x05` SCHEMA section bytes
    /// (digest-prefixed) its decoder takes.
    pub fn schema_section_bytes(&self) -> Option<Vec<u8>> {
        let r = self.section(kind::SCHEMA)?;
        let stored = self.reader.read_at(r.offset, r.len).ok()?;
        let body = decompress(self.h.internal_codec, &stored).ok()?;
        let mut out = digest16(&body).to_vec();
        out.extend_from_slice(&body);
        Some(out)
    }

    /// The decoded stats + schema section.
    pub fn schema(&self) -> Option<crate::schema_section::SchemaMeta> {
        crate::schema_section::SchemaMeta::decode(&self.schema_section_bytes()?).ok()
    }

    /// The community pyramid's bytes.
    pub fn pyramid_bytes(&self) -> Option<Vec<u8>> {
        let r = self.section(kind::PYRAMID)?;
        self.reader.read_at(r.offset, r.len).ok()
    }

    /// The four dictionary sections, two-level, chunks faulted on demand.
    /// `want[si] == false` leaves that section empty (a search-only open).
    pub fn dictionary(&self, want: [bool; 4]) -> Result<Dictionary, FileError> {
        let unit = self
            .root
            .unit(tag::DICT)
            .ok_or(FileError::Container("0x06 file has no dictionary"))?;
        let roots = decode_dict_root(unit)?;
        let leaves_base = self.section(kind::DICT_LEAVES).map_or(0, |r| r.offset);
        let data_base = self.section(kind::DICT_DATA).map_or(0, |r| r.offset);
        let cache = crate::chunk_cache::ChunkCache::unlimited_arc();
        let codec = self.h.dict_codec;
        let mut dirs: Vec<Option<Arc<LeafedChunks>>> = Vec::with_capacity(4);
        let mut sections = Vec::with_capacity(4);
        for (si, s) in roots.iter().enumerate() {
            let meta = SectionMeta {
                term_count: if want[si] { s.term_count } else { 0 },
                restart_interval: s.restart_interval.max(1),
                restart_offsets: Vec::new(),
            };
            if !want[si] {
                sections.push(ChunkedSection::from_parts(
                    meta,
                    Vec::new(),
                    None,
                    cache.clone(),
                    si as u8,
                ));
                dirs.push(None);
                continue;
            }
            let leaves: Vec<ChunkLeaf> = s
                .leaves
                .iter()
                .map(|(l, run, coff, key)| ChunkLeaf {
                    first: l.first,
                    n: l.n,
                    first_run: *run,
                    key: key.clone(),
                    leaf: (leaves_base + l.off, l.len),
                    chunk_base: data_base + s.data_off + coff,
                })
                .collect();
            let dir = Arc::new(LeafedChunks::new(
                leaves,
                s.n_chunks,
                self.fetch(),
                self.h.internal_codec,
            ));
            let (r, d) = (self.reader.clone(), dir.clone());
            let loader: crate::dict::ChunkLoader = Box::new(move |ci| {
                let (off, len) = d.range(ci)?;
                let b = r.read_at(off, len).ok()?;
                decompress(codec, &b).ok()
            });
            let (r, d) = (self.reader.clone(), dir.clone());
            let bulk: crate::dict::ChunkBulkLoader = Box::new(move |cis| {
                d.ensure_leaves(&d.leaves_of(cis));
                let ranges: Option<Vec<(u64, u64)>> = cis.iter().map(|&ci| d.range(ci)).collect();
                let blobs = read_coalesced(&*r, &ranges?, DICT_GAP)?;
                blobs.iter().map(|b| decompress(codec, b).ok()).collect()
            });
            sections.push(
                ChunkedSection::leafed(meta, dir.clone(), loader, cache.clone(), si as u8)
                    .with_bulk_loader(bulk),
            );
            dirs.push(Some(dir));
        }
        let arr: [ChunkedSection; 4] = sections
            .try_into()
            .map_err(|_| FileError::Container("expected 4 dictionary sections"))?;
        let r = self.reader.clone();
        let multi: crate::dict::MultiChunkBulkLoader = Box::new(move |pairs| {
            // Fault every section's leaves first (one batch), then the chunks.
            let mut per: [Vec<usize>; 4] = Default::default();
            for &(si, ci) in pairs {
                if let Some(v) = per.get_mut(si as usize) {
                    v.push(ci);
                }
            }
            let mut leaf_ranges: Vec<(u64, u64)> = Vec::new();
            let _ = &mut leaf_ranges;
            for (si, cis) in per.iter().enumerate() {
                if let Some(Some(d)) = dirs.get(si) {
                    d.ensure_leaves(&d.leaves_of(cis));
                }
            }
            let ranges: Option<Vec<(u64, u64)>> = pairs
                .iter()
                .map(|&(si, ci)| dirs.get(si as usize)?.as_ref()?.range(ci))
                .collect();
            let blobs = read_coalesced(&*r, &ranges?, DICT_GAP)?;
            blobs.iter().map(|b| decompress(codec, b).ok()).collect()
        });
        let mut dict = Dictionary::from_chunked_sections(arr).with_multi_bulk_loader(multi);
        dict.set_has_quoted_triples(self.h.flags & F_QUOTED != 0);
        Ok(dict)
    }

    /// The default graph's index, two-level, tiles faulted on demand.
    pub fn index(&self, read_concurrency: usize) -> Result<GraphIndex, FileError> {
        let unit = self
            .root
            .unit(tag::INDEX)
            .ok_or(FileError::Container("0x06 file has no index"))?;
        let perms_root = decode_index_root(unit)?;
        let leaves_base = self.section(kind::INDEX_LEAVES).map_or(0, |r| r.offset);
        let data_base = self.section(kind::INDEX_DATA).map_or(0, |r| r.offset);
        let perms = self.h.perms();
        let mut lists: [Option<Arc<LeafedTiles>>; NUM_PERMS] = Default::default();
        for p in &perms_root {
            if p.si >= NUM_PERMS {
                return Err(FileError::Container("0x06 index: bad permutation"));
            }
            let leaves: Vec<TileLeaf> = p
                .leaves
                .iter()
                .map(|(l, mn, mx, toff)| TileLeaf {
                    first: l.first,
                    n: l.n,
                    min_a: *mn,
                    max_a: *mx,
                    leaf: (leaves_base + l.off, l.len),
                    tile_base: data_base + p.data_off + toff,
                })
                .collect();
            lists[p.si] = Some(Arc::new(LeafedTiles::new(
                leaves,
                p.n_tiles,
                p.data_len,
                self.fetch(),
                self.h.internal_codec,
            )));
        }
        for perm in perms.iter() {
            if lists[perm.section_index()].is_none() {
                return Err(FileError::Container(
                    "0x06 index lacks a stored permutation",
                ));
            }
        }
        let codec = self.h.block_codec;
        let (r, l) = (self.reader.clone(), lists.clone());
        let loader: crate::index::TileLoader = Box::new(move |si, ti| {
            let (off, len) = l.get(si)?.as_ref()?.range(ti)?;
            let b = r.read_at(off, len).ok()?;
            decompress(codec, &b).ok()
        });
        let (r, l) = (self.reader.clone(), lists.clone());
        let bulk: crate::index::TileBulkLoader = Box::new(move |si, tis| {
            let list = l.get(si)?.as_ref()?;
            if let (Some(&a), Some(&b)) = (tis.first(), tis.last()) {
                list.ensure_span(a, b + 1);
            }
            let ranges: Option<Vec<(u64, u64)>> = tis.iter().map(|&ti| list.range(ti)).collect();
            let blobs = read_coalesced(&*r, &ranges?, TILE_GAP)?;
            blobs.iter().map(|b| decompress(codec, b).ok()).collect()
        });
        let mut index = GraphIndex::from_leafed(lists, perms, loader).with_bulk_loader(bulk);
        index.set_read_concurrency(read_concurrency);
        Ok(index)
    }

    /// The text index (leafed token table), if the file has one.
    pub fn text_index(&self) -> Option<Result<crate::text_index::TextIndex, FileError>> {
        let unit = self.root.unit(tag::TEXT)?;
        Some((|| {
            let t = decode_text_root(unit)?;
            let leaves_base = self.section(kind::TEXT_LEAVES).map_or(0, |r| r.offset);
            let data = self
                .section(kind::TEXT_DATA)
                .unwrap_or(ByteRange { offset: 0, len: 0 });
            let leaves = t
                .leaves
                .iter()
                .map(|(l, tok)| crate::text_index::TokenLeaf {
                    first: l.first,
                    n: l.n,
                    first_token: String::from_utf8_lossy(tok).into_owned(),
                    leaf: (leaves_base + l.off, l.len),
                })
                .collect();
            let tokens = Arc::new(crate::text_index::LeafedTokens::new(
                leaves,
                t.n_tokens,
                self.fetch(),
                self.h.internal_codec,
            ));
            let r = self.reader.clone();
            let loader = Box::new(move |off: u64, len: u64| {
                if off.checked_add(len)? > data.len {
                    return None;
                }
                r.read_at(data.offset + off, len).ok()
            });
            crate::text_index::TextIndex::leafed(tokens, &t.scope, loader)
        })())
    }

    /// Stored bytes of the token table (all leaves) — what a substring search
    /// would read; a word lookup reads one leaf.
    pub fn token_table_bytes(&self) -> Option<u64> {
        self.root.unit(tag::TEXT)?;
        Some(self.section(kind::TEXT_LEAVES).map_or(0, |r| r.len))
    }

    /// The label index, if the file has one.
    pub fn label_index(&self) -> Option<Result<crate::label_index::LabelIndex, FileError>> {
        let unit = self.root.unit(tag::LABELS)?;
        Some((|| {
            let l = decode_label_root(unit)?;
            let data = self
                .section(kind::LABEL_DATA)
                .unwrap_or(ByteRange { offset: 0, len: 0 });
            let nodes = self
                .section(kind::LABEL_LEAVES)
                .unwrap_or(ByteRange { offset: 0, len: 0 });
            if data.len != l.data_len {
                return Err(FileError::Container("0x06 label data length mismatch"));
            }
            let head_len = l.head.len() as u64;
            let section_len = head_len + data.len + nodes.len;
            let r = self.reader.clone();
            let head = l.head.clone();
            let loader = Box::new(move |off: u64, len: u64| -> Option<Vec<u8>> {
                let end = off.checked_add(len)?;
                if end <= head_len {
                    return head.get(off as usize..end as usize).map(<[u8]>::to_vec);
                }
                let bo = off.checked_sub(head_len)?;
                if bo + len <= data.len {
                    r.read_at(data.offset + bo, len).ok()
                } else if bo >= data.len && bo - data.len + len <= nodes.len {
                    r.read_at(nodes.offset + (bo - data.len), len).ok()
                } else {
                    None
                }
            });
            crate::label_index::LabelIndex::from_head(
                &l.head,
                section_len,
                self.h.block_codec,
                loader,
            )
        })())
    }

    /// The label index's entry count, from the root alone.
    pub fn label_entry_count(&self) -> Option<u64> {
        let l = decode_label_root(self.root.unit(tag::LABELS)?).ok()?;
        crate::label_index::LabelIndexPreamble::parse(&l.head)
            .ok()
            .map(|p| p.entry_count)
    }

    /// The quad index's root, if the file has named graphs.
    pub fn quad_root(&self) -> Option<Result<QuadRoot, FileError>> {
        self.root.unit(tag::QUAD).map(decode_quad_root)
    }

    /// The named graphs (quad index), if any.
    pub(crate) fn quad(
        &self,
        read_concurrency: usize,
    ) -> Option<Result<crate::quad::QuadNamed, FileError>> {
        let root = match self.quad_root()? {
            Ok(r) => r,
            Err(e) => return Some(Err(e)),
        };
        let leaves = self.section(kind::QUAD_LEAVES).map_or(0, |r| r.offset);
        let data = self.section(kind::QUAD_DATA).map_or(0, |r| r.offset);
        Some(
            crate::quad::V06Quad::new(&root, leaves, data, self.h.internal_codec).map(|v| {
                crate::quad::QuadNamed::new_v06(
                    self.reader.clone(),
                    v,
                    self.h.block_codec,
                    self.h.perms(),
                    read_concurrency,
                )
            }),
        )
    }

    /// Verify every digest: the root, the card, each section (streamed), and
    /// the content hash. `only` restricts the section checks to one kind.
    /// `Ok(false)` names nothing; [`verify_report`](Self::verify_report) does.
    pub fn verify(&self, only: Option<u8>) -> Result<bool, FileError> {
        Ok(self.verify_report(only)?.is_empty())
    }

    /// Like [`verify`](Self::verify), returning what failed (empty = all good).
    pub fn verify_report(&self, only: Option<u8>) -> Result<Vec<String>, FileError> {
        const SLICE: u64 = 8 << 20;
        let mut bad = Vec::new();
        if content_hash(&self.h) != self.h.content_hash {
            bad.push("content hash".to_string());
        }
        let card = self.h.card_range();
        if only.is_none() && card.len > 0 {
            let stored = self.reader.read_at(card.offset, card.len)?;
            if digest16(&stored) != self.h.card_digest {
                bad.push("card".to_string());
            }
        }
        for s in &self.root.sections {
            if only.is_some_and(|k| k != s.kind) {
                continue;
            }
            let a = s.abs(&self.h);
            let mut h = blake3::Hasher::new();
            let mut at = a.offset;
            while at < a.offset + a.len {
                let n = (a.offset + a.len - at).min(SLICE);
                h.update(&self.reader.read_at(at, n)?);
                at += n;
            }
            if h.finalize().as_bytes()[..16] != s.digest {
                bad.push(kind::name(s.kind).to_string());
            }
        }
        if only.is_none() {
            let foot = self
                .reader
                .read_at(self.h.file_len - FOOTER.len() as u64, FOOTER.len() as u64)?;
            if foot != FOOTER {
                bad.push("footer".to_string());
            }
        }
        Ok(bad)
    }
}

/// Is this file (by its first bytes) a `0x06` file?
pub fn is_v06(first: &[u8]) -> bool {
    first.len() > 4 && first[0..4] == crate::header::MAGIC && first[4] == VERSION
}

/// Read a `0x06` file's header and root through any reader (two reads at most;
/// over a seeded or prefix-holding reader, none).
pub fn read_front<R: RangeReader + ?Sized>(r: &R) -> Result<(Header06, Root06), FileError> {
    let head = r.read_at(0, HEADER_LEN as u64)?;
    let h = Header06::from_bytes(&head)?;
    let root_bytes = r.read_at(HEADER_LEN as u64, h.root_len as u64)?;
    if digest16(&root_bytes) != h.root_digest {
        return Err(FileError::Container("0x06 root digest mismatch"));
    }
    let root = Root06::decode(&root_bytes, h.internal_codec)?;
    Ok((h, root))
}

/// The stored bytes of section `k`, if present.
pub fn read_section<R: RangeReader + ?Sized>(
    r: &R,
    h: &Header06,
    root: &Root06,
    k: u8,
) -> Result<Option<Vec<u8>>, FileError> {
    match root.section(k) {
        Some(s) => {
            let a = s.abs(h);
            Ok(Some(r.read_at(a.offset, a.len)?))
        }
        None => Ok(None),
    }
}

/// The card (decompressed) and the build record (raw): one read of the card
/// region, which sits right after the root.
#[allow(clippy::type_complexity)]
pub fn read_card_build<R: RangeReader + ?Sized>(
    r: &R,
    h: &Header06,
) -> Result<(Option<Vec<u8>>, Option<Vec<u8>>), FileError> {
    let c = h.card_range();
    let b = h.build_range();
    if c.len == 0 && b.len == 0 {
        return Ok((None, None));
    }
    let lo = if c.len == 0 {
        b.offset
    } else if b.len == 0 {
        c.offset
    } else {
        c.offset.min(b.offset)
    };
    let hi = (c.offset + c.len).max(b.offset + b.len);
    let span = r.read_at(lo, hi - lo)?;
    let card = if c.len > 0 {
        let stored = &span[(c.offset - lo) as usize..(c.offset - lo + c.len) as usize];
        if digest16(stored) != h.card_digest {
            return Err(FileError::Container("0x06 card digest mismatch"));
        }
        Some(decompress(h.internal_codec, stored)?)
    } else {
        None
    };
    let build = (b.len > 0)
        .then(|| span[(b.offset - lo) as usize..(b.offset - lo + b.len) as usize].to_vec());
    Ok((card, build))
}

/// The stats + schema section as the digest-prefixed bytes its decoder takes.
pub fn read_schema_bytes<R: RangeReader + ?Sized>(r: &R) -> Result<Option<Vec<u8>>, FileError> {
    let (h, root) = read_front(r)?;
    let Some(stored) = read_section(r, &h, &root, kind::SCHEMA)? else {
        return Ok(None);
    };
    let body = decompress(h.internal_codec, &stored)?;
    let mut out = digest16(&body).to_vec();
    out.extend_from_slice(&body);
    Ok(Some(out))
}

/// The community pyramid's bytes, if any.
pub fn read_pyramid<R: RangeReader + ?Sized>(r: &R) -> Result<Option<Vec<u8>>, FileError> {
    let (h, root) = read_front(r)?;
    read_section(r, &h, &root, kind::PYRAMID)
}

/// Stored bytes of the text index's token table (all leaves).
pub fn read_token_table_bytes<R: RangeReader + ?Sized>(r: &R) -> Option<u64> {
    let (_, root) = read_front(r).ok()?;
    root.unit(tag::TEXT)?;
    Some(root.section(kind::TEXT_LEAVES).map_or(0, |s| s.len))
}

/// A reader holding only some ranges of a file in memory (the borrowed-reader
/// opens of a `0x06` file read what they need into one of these).
pub struct SparseReader {
    len: u64,
    parts: Vec<(u64, Vec<u8>)>,
}

impl SparseReader {
    /// Fetch `ranges` from `r` (plus the first bytes `first`) into memory —
    /// the ones `first` does not already hold, in one batch.
    pub fn fetch<R: RangeReader + ?Sized>(
        r: &R,
        first: Vec<u8>,
        ranges: &[ByteRange],
    ) -> Result<Self, FileError> {
        let held = first.len() as u64;
        let want: Vec<(u64, u64)> = ranges
            .iter()
            .filter(|g| g.len > 0 && g.offset + g.len > held)
            .map(|g| (g.offset, g.len))
            .collect();
        let mut parts = vec![(0u64, first)];
        if !want.is_empty() {
            for ((o, _), b) in want.iter().zip(r.read_many(&want)?) {
                parts.push((*o, b));
            }
        }
        Ok(SparseReader {
            len: r.len(),
            parts,
        })
    }
}

/// A borrowed reader whose first bytes are already in memory: reads inside
/// them cost nothing, the rest go to `r`.
pub struct Prefixed<'a, R: RangeReader + ?Sized> {
    /// The file's first bytes.
    pub first: &'a [u8],
    /// The reader behind them.
    pub r: &'a R,
}

impl<R: RangeReader + ?Sized> RangeReader for Prefixed<'_, R> {
    fn len(&self) -> u64 {
        self.r.len()
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        match offset.checked_add(len) {
            Some(end) if end <= self.first.len() as u64 => {
                Ok(self.first[offset as usize..end as usize].to_vec())
            }
            _ => self.r.read_at(offset, len),
        }
    }
}

impl RangeReader for SparseReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        for (o, b) in &self.parts {
            if offset >= *o && offset + len <= o + b.len() as u64 {
                let s = (offset - o) as usize;
                return Ok(b[s..s + len as usize].to_vec());
            }
        }
        Err(std::io::Error::other("0x06: range not held by this reader"))
    }
}

/// What the card's measured signals need from a `0x06` file's root: the text
/// index's `(bytes, token-table bytes)` and the label index's `(bytes, entries)`.
#[allow(clippy::type_complexity)]
pub fn read_signals<R: RangeReader + ?Sized>(
    r: &R,
) -> Result<(Option<(u64, u64)>, Option<(u64, Option<u64>)>), FileError> {
    let (_, root) = read_front(r)?;
    let len = |k: u8| root.section(k).map_or(0, |s| s.len);
    let text = root.unit(tag::TEXT).map(|_| {
        (
            len(kind::TEXT_LEAVES) + len(kind::TEXT_DATA),
            len(kind::TEXT_LEAVES),
        )
    });
    let labels = match root.unit(tag::LABELS) {
        None => None,
        Some(u) => {
            let l = decode_label_root(u)?;
            let entries = crate::label_index::LabelIndexPreamble::parse(&l.head)
                .ok()
                .map(|p| p.entry_count);
            Some((
                l.head.len() as u64 + len(kind::LABEL_DATA) + len(kind::LABEL_LEAVES),
                entries,
            ))
        }
    };
    Ok((text, labels))
}
