//! The `0x06` assembler: [`super::parts::Parts`] in, a `0x06` file out. Every
//! writer goes through here — the in-RAM build (via `0x05` section images),
//! `rete convert`, and in phase 2 the memory-bounded build, `merge` and
//! `recard`.
//!
//! The front (root, card region, stats, leaf directories) is built in memory;
//! the data region is streamed from the input reader, hashed on the way. The
//! data is written first at its final offset, then the front at offset 0, so the
//! output needs `Write + Seek` and the input is read once.

use std::io::{Seek, SeekFrom, Write};

use super::parts::Parts;
use super::*;
use crate::file::{compress, FileError};
use crate::reader::RangeReader;

/// How quad tiles are clustered in the data region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuadClustering {
    /// Graph-major when the mean graph is smaller than a tile, else order-major.
    Auto,
    /// The tiles of every order for one block of graphs side by side.
    GraphMajor,
    /// Each order's tiles contiguous.
    OrderMajor,
}

impl QuadClustering {
    /// `RETE_QUAD_CLUSTERING=graph|order|auto` (default auto).
    pub fn from_env() -> Self {
        match std::env::var("RETE_QUAD_CLUSTERING").ok().as_deref() {
            Some("graph") => QuadClustering::GraphMajor,
            Some("order") => QuadClustering::OrderMajor,
            _ => QuadClustering::Auto,
        }
    }
}

/// Assembly options.
#[derive(Debug, Clone, Copy)]
pub struct WriteOpts {
    /// Codec of the root, leaves, card and stats (`writer_codec()`).
    pub internal_codec: u8,
    /// Quad tile clustering.
    pub quad: QuadClustering,
}

impl Default for WriteOpts {
    fn default() -> Self {
        WriteOpts {
            internal_codec: crate::file::writer_codec(),
            quad: QuadClustering::from_env(),
        }
    }
}

/// What the assembler wrote.
#[derive(Debug, Clone)]
pub struct Written {
    /// The header as written.
    pub header: Header06,
    /// The root as written.
    pub root: Root06,
}

/// One segment of the data region: input bytes to copy.
#[derive(Debug, Clone, Copy)]
struct Seg {
    input: u64,
    len: u64,
}

/// A data section: its kind and the input segments, in output order.
struct DataSec {
    kind: u8,
    segs: Vec<Seg>,
}

impl DataSec {
    fn len(&self) -> u64 {
        self.segs.iter().map(|s| s.len).sum()
    }
}

/// Leaf groups whose leaf size the root budget may grow.
#[derive(Clone)]
struct Targets {
    dict: [usize; 4],
    index: [usize; 6],
    quad: [usize; 6],
    text: usize,
}

/// The front built for a set of leaf targets.
struct Front {
    units: Vec<(u8, Vec<u8>)>,
    dict_leaves: Vec<u8>,
    index_leaves: Vec<u8>,
    quad_leaves: Vec<u8>,
    text_leaves: Vec<u8>,
    /// Leaves per group, to pick which group to grow: `(group id, count)`.
    counts: Vec<(usize, usize)>,
}

/// Order in which dictionary sections are laid out (predicates first).
const DICT_ORDER: [usize; 4] = [3, 0, 1, 2];

/// Lay out the data region: the input segments of every data section and, for
/// the quad index, each tile's new offset within QUAD_DATA.
#[allow(clippy::type_complexity)]
fn layout_data(
    parts: &Parts,
    graph_major: bool,
) -> (Vec<DataSec>, [u64; 4], Vec<u64>, Vec<Vec<u64>>) {
    let mut secs = Vec::new();
    // Dictionary: sections in DICT_ORDER, each a run of chunks.
    let mut dict_off = [0u64; 4];
    let mut segs = Vec::new();
    let mut at = 0u64;
    for &s in &DICT_ORDER {
        dict_off[s] = at;
        for (c, inp) in &parts.dict[s].chunks {
            segs.push(Seg {
                input: *inp,
                len: c.clen,
            });
            at += c.clen;
        }
    }
    if at > 0 {
        secs.push(DataSec {
            kind: kind::DICT_DATA,
            segs,
        });
    }
    // Index: permutations in section-index order.
    let mut perm_off = Vec::new();
    let mut segs = Vec::new();
    let mut at = 0u64;
    for p in &parts.index {
        perm_off.push(at);
        for (t, inp) in &p.tiles {
            segs.push(Seg {
                input: *inp,
                len: t.clen,
            });
            at += t.clen;
        }
    }
    if at > 0 {
        secs.push(DataSec {
            kind: kind::INDEX_DATA,
            segs,
        });
    }
    // Quad: order-major, or graph-major by directory block.
    let mut quad_off: Vec<Vec<u64>> = Vec::new();
    if let Some(q) = &parts.quad {
        let mut segs = Vec::new();
        quad_off = q.orders.iter().map(|(_, t)| vec![0u64; t.len()]).collect();
        let mut at = 0u64;
        if graph_major {
            let firsts = block_first_graphs(&q.p0);
            // The block a tile belongs to: the last block whose first graph is
            // <= the tile's first graph.
            let block_of = |g: u32| firsts.partition_point(|&f| f <= g).saturating_sub(1);
            let mut cursor = vec![0usize; q.orders.len()];
            for b in 0..firsts.len().max(1) {
                for (k, (_, tiles)) in q.orders.iter().enumerate() {
                    while cursor[k] < tiles.len()
                        && (firsts.is_empty() || block_of(tiles[cursor[k]].first.0) <= b)
                    {
                        let t = &tiles[cursor[k]];
                        quad_off[k][cursor[k]] = at;
                        segs.push(Seg {
                            input: t.off,
                            len: t.clen,
                        });
                        at += t.clen;
                        cursor[k] += 1;
                    }
                }
            }
            // Anything left (a tile past the last block's first graph is already
            // covered above; this only guards a malformed P0).
            for (k, (_, tiles)) in q.orders.iter().enumerate() {
                while cursor[k] < tiles.len() {
                    let t = &tiles[cursor[k]];
                    quad_off[k][cursor[k]] = at;
                    segs.push(Seg {
                        input: t.off,
                        len: t.clen,
                    });
                    at += t.clen;
                    cursor[k] += 1;
                }
            }
        } else {
            for (k, (_, tiles)) in q.orders.iter().enumerate() {
                for (i, t) in tiles.iter().enumerate() {
                    quad_off[k][i] = at;
                    segs.push(Seg {
                        input: t.off,
                        len: t.clen,
                    });
                    at += t.clen;
                }
            }
        }
        if at > 0 {
            secs.push(DataSec {
                kind: kind::QUAD_DATA,
                segs,
            });
        }
    }
    if let Some(l) = &parts.labels {
        if l.data_len > 0 {
            secs.push(DataSec {
                kind: kind::LABEL_DATA,
                segs: vec![Seg {
                    input: l.body.offset,
                    len: l.data_len,
                }],
            });
        }
    }
    if let Some(t) = &parts.text {
        if t.postings.len > 0 {
            secs.push(DataSec {
                kind: kind::TEXT_DATA,
                segs: vec![Seg {
                    input: t.postings.offset,
                    len: t.postings.len,
                }],
            });
        }
    }
    if let Some(py) = parts.pyramid {
        if py.len > 0 {
            secs.push(DataSec {
                kind: kind::PYRAMID,
                segs: vec![Seg {
                    input: py.offset,
                    len: py.len,
                }],
            });
        }
    }
    (secs, dict_off, perm_off, quad_off)
}

/// First graph id of each graph-directory block, from a `P0` block index.
fn block_first_graphs(p0: &[u8]) -> Vec<u32> {
    let mut c = Cur::new(p0);
    let mut out = Vec::new();
    let Ok(n) = c.count(u32::MAX as u64) else {
        return out;
    };
    let mut g = 0u32;
    for _ in 0..n {
        let (Ok(d), Ok(_)) = (c.u32v(), c.uv()) else {
            break;
        };
        g = g.saturating_add(d);
        out.push(g);
    }
    out
}

/// Compress each leaf of `entries` (cut at `target` encoded bytes) and append
/// it to `section`. Returns `(leaf ref, first entry index)` per leaf.
fn leaves_of<E>(
    entries: &[E],
    target: usize,
    codec: u8,
    section: &mut Vec<u8>,
    encode: impl Fn(&[E], usize) -> Vec<u8>,
) -> Vec<LeafRef> {
    let sizes: Vec<usize> = entries
        .chunks(1)
        .enumerate()
        .map(|(i, e)| encode(e, i).len())
        .collect();
    let mut out = Vec::new();
    for (a, b) in cut(&sizes, target) {
        let raw = encode(&entries[a..b], a);
        let stored = compress(codec, &raw);
        out.push(LeafRef {
            first: a,
            n: b - a,
            off: section.len() as u64,
            len: stored.len() as u64,
        });
        section.extend_from_slice(&stored);
    }
    out
}

fn build_front(
    parts: &Parts,
    t: &Targets,
    codec: u8,
    dict_off: &[u64; 4],
    perm_off: &[u64],
    quad_off: &[Vec<u64>],
    quad_gdir: &[u8],
) -> Front {
    let mut f = Front {
        units: Vec::new(),
        dict_leaves: Vec::new(),
        index_leaves: Vec::new(),
        quad_leaves: Vec::new(),
        text_leaves: Vec::new(),
        counts: Vec::new(),
    };
    // Dictionary.
    let mut droots: [DictSecRoot; 4] = Default::default();
    for &s in &DICT_ORDER {
        let d = &parts.dict[s];
        let ents: Vec<ChunkEnt> = d.chunks.iter().map(|(c, _)| c.clone()).collect();
        let leaves = leaves_of(&ents, t.dict[s], codec, &mut f.dict_leaves, |e, _| {
            encode_chunk_leaf(e)
        });
        // Offset of each chunk within the section's data.
        let mut coff = Vec::with_capacity(ents.len());
        let mut at = 0u64;
        for e in &ents {
            coff.push(at);
            at += e.clen;
        }
        f.counts.push((s, leaves.len()));
        droots[s] = DictSecRoot {
            term_count: d.term_count,
            restart_interval: d.restart_interval,
            n_chunks: ents.len(),
            data_off: dict_off[s],
            data_len: at,
            leaves: leaves
                .into_iter()
                .map(|l| {
                    let e = &ents[l.first];
                    let run = e.first_run;
                    let key = e.key.clone();
                    let co = coff[l.first];
                    (l, run, co, key)
                })
                .collect(),
        };
    }
    f.units.push((tag::DICT, encode_dict_root(&droots)));
    // Index.
    let mut proots = Vec::new();
    for (pi, p) in parts.index.iter().enumerate() {
        let ents: Vec<TileEnt> = p.tiles.iter().map(|(t, _)| *t).collect();
        let leaves = leaves_of(&ents, t.index[p.si], codec, &mut f.index_leaves, |e, _| {
            encode_tile_leaf(e)
        });
        let mut toff = Vec::with_capacity(ents.len());
        let mut at = 0u64;
        for e in &ents {
            toff.push(at);
            at += e.clen;
        }
        f.counts.push((4 + p.si, leaves.len()));
        proots.push(PermRoot {
            si: p.si,
            n_tiles: ents.len(),
            data_off: perm_off[pi],
            data_len: at,
            leaves: leaves
                .into_iter()
                .map(|l| {
                    let mn = ents[l.first].min_a;
                    let mx = ents[l.first + l.n - 1].max_a;
                    let o = toff[l.first];
                    (l, mn, mx, o)
                })
                .collect(),
        });
    }
    f.units.push((tag::INDEX, encode_index_root(&proots)));
    // Quad.
    if let Some(q) = &parts.quad {
        f.quad_leaves.extend_from_slice(quad_gdir);
        let mut orders = Vec::new();
        for (k, (si, tiles)) in q.orders.iter().enumerate() {
            let ents: Vec<QTileEnt> = tiles
                .iter()
                .enumerate()
                .map(|(i, e)| QTileEnt {
                    off: quad_off[k][i],
                    ..*e
                })
                .collect();
            let leaves = leaves_of(&ents, t.quad[*si], codec, &mut f.quad_leaves, |e, i| {
                let _ = i;
                encode_qtile_leaf(e, e[0].off)
            });
            f.counts.push((10 + si, leaves.len()));
            orders.push(QuadOrderRoot {
                si: *si,
                n_tiles: ents.len(),
                leaves: leaves
                    .into_iter()
                    .map(|l| {
                        let a = &ents[l.first];
                        let b = &ents[l.first + l.n - 1];
                        let o = a.off;
                        (l, a.first, b.last, o)
                    })
                    .collect(),
            });
        }
        f.units.push((
            tag::QUAD,
            encode_quad_root(&QuadRoot {
                graph_count: q.graph_count,
                quad_count: q.quad_count,
                block_graphs: q.block_graphs,
                p0: q.p0.clone(),
                gdir_len: quad_gdir.len() as u64,
                orders,
            }),
        ));
    }
    // Labels.
    if let Some(l) = &parts.labels {
        f.units.push((
            tag::LABELS,
            encode_label_root(&LabelRoot {
                data_len: l.data_len,
                head: l.head.clone(),
            }),
        ));
    }
    // Text.
    if let Some(tp) = &parts.text {
        let leaves = leaves_of(&tp.tokens, t.text, codec, &mut f.text_leaves, |e, _| {
            encode_token_leaf(e)
        });
        f.counts.push((16, leaves.len()));
        f.units.push((
            tag::TEXT,
            encode_text_root(&TextRoot {
                scope: tp.scope.clone(),
                n_tokens: tp.tokens.len(),
                leaves: leaves
                    .into_iter()
                    .map(|l| {
                        let tok = tp.tokens[l.first].token.clone();
                        (l, tok)
                    })
                    .collect(),
            }),
        ));
    }
    f
}

/// Copy `segs` from `input` to `out`, coalescing input-adjacent segments into
/// reads of at most 8 MiB, hashing everything written into `h`.
fn copy_segs(
    input: &dyn RangeReader,
    segs: &[Seg],
    out: &mut dyn Write,
    h: &mut blake3::Hasher,
) -> Result<(), FileError> {
    const SPAN: u64 = 8 << 20;
    let mut i = 0;
    while i < segs.len() {
        let start = segs[i].input;
        let mut end = start + segs[i].len;
        let mut j = i + 1;
        while j < segs.len() && segs[j].input == end && end - start < SPAN {
            end += segs[j].len;
            j += 1;
        }
        let mut at = start;
        while at < end {
            let n = (end - at).min(SPAN);
            let b = input.read_at(at, n)?;
            h.update(&b);
            out.write_all(&b)?;
            at += n;
        }
        i = j;
    }
    Ok(())
}

/// Assemble a `0x06` file from `parts`, whose payload ranges point into `input`.
pub fn assemble<W: Write + Seek>(
    parts: &Parts,
    input: &dyn RangeReader,
    out: &mut W,
    opts: WriteOpts,
) -> Result<Written, FileError> {
    let codec = opts.internal_codec;
    // Quad clustering: graph-major when the mean graph is under a tile.
    let graph_major = match (&parts.quad, opts.quad) {
        (None, _) => false,
        (Some(_), QuadClustering::GraphMajor) => true,
        (Some(_), QuadClustering::OrderMajor) => false,
        (Some(q), QuadClustering::Auto) => {
            let bytes: u64 = q.orders.iter().flat_map(|(_, t)| t).map(|t| t.clen).sum();
            q.graph_count > 0 && bytes / q.graph_count.max(1) < 64 * 1024
        }
    };
    let (data, dict_off, perm_off, quad_off) = layout_data(parts, graph_major);
    let quad_gdir = match &parts.quad {
        Some(q) if q.gdir.len > 0 => input.read_at(q.gdir.offset, q.gdir.len)?,
        _ => Vec::new(),
    };
    let label_nodes = match &parts.labels {
        Some(l) if l.body.len > l.data_len => input.read_at(
            l.body.offset + l.data_len,
            l.body.len - l.data_len,
        )?,
        _ => Vec::new(),
    };

    // Grow leaves until the stored root fits its budget.
    let mut t = Targets {
        dict: [LEAF_START; 4],
        index: [LEAF_START; 6],
        quad: [LEAF_START; 6],
        text: LEAF_START,
    };
    let mut front;
    loop {
        front = build_front(
            parts, &t, codec, &dict_off, &perm_off, &quad_off, &quad_gdir,
        );
        let probe = Root06 {
            sections: vec![
                Sec06 {
                    kind: 0,
                    offset: 0,
                    len: 0,
                    digest: [0; 16]
                };
                kind::ORDER.len()
            ],
            units: front.units.clone(),
        };
        if probe.encode(codec).len() <= ROOT_BUDGET {
            break;
        }
        let Some(&(g, n)) = front.counts.iter().max_by_key(|(_, n)| *n) else {
            break;
        };
        if n <= 1 {
            break;
        }
        match g {
            0..=3 => t.dict[g] *= 2,
            4..=9 => t.index[g - 4] *= 2,
            10..=15 => t.quad[g - 10] *= 2,
            _ => t.text *= 2,
        }
    }

    // Front sections, in file order.
    let schema = parts.schema.as_ref().map(|s| compress(codec, s));
    let mut fronts: Vec<(u8, Vec<u8>)> = Vec::new();
    if let Some(s) = schema {
        fronts.push((kind::SCHEMA, s));
    }
    if !front.dict_leaves.is_empty() {
        fronts.push((kind::DICT_LEAVES, std::mem::take(&mut front.dict_leaves)));
    }
    if !front.index_leaves.is_empty() {
        fronts.push((kind::INDEX_LEAVES, std::mem::take(&mut front.index_leaves)));
    }
    if !front.quad_leaves.is_empty() {
        fronts.push((kind::QUAD_LEAVES, std::mem::take(&mut front.quad_leaves)));
    }
    if !label_nodes.is_empty() {
        fronts.push((kind::LABEL_LEAVES, label_nodes));
    }
    if !front.text_leaves.is_empty() {
        fronts.push((kind::TEXT_LEAVES, std::mem::take(&mut front.text_leaves)));
    }

    // Card region: card | record | slack.
    let card = parts
        .card
        .as_ref()
        .filter(|c| !c.is_empty())
        .map(|c| compress(codec, c));
    let build = parts.build.clone().filter(|b| !b.is_empty());
    let card_len = card.as_ref().map_or(0, |c| c.len()) as u32;
    let build_len = build.as_ref().map_or(0, |b| b.len()) as u32;
    let mut region_len = card_region_len(card_len, build_len);

    // Section table (digests filled once the data is hashed).
    let mut sections = Vec::new();
    let mut at = 0u64;
    for (k, b) in &fronts {
        sections.push(Sec06 {
            kind: *k,
            offset: at,
            len: b.len() as u64,
            digest: digest16(b),
        });
        at += b.len() as u64;
    }
    let front_sections_len = at;
    let mut at = 0u64;
    for d in &data {
        sections.push(Sec06 {
            kind: d.kind,
            offset: at,
            len: d.len(),
            digest: [0; 16],
        });
        at += d.len();
    }
    let data_len = at;
    let mut root = Root06 {
        sections,
        units: front.units,
    };
    let root_len = root.encode(codec).len() as u32;
    // A file is never shorter than the 1 KiB a 0x05-era reader fetches first:
    // a tiny one gets the difference as card-region slack.
    let total = HEADER_LEN as u64
        + root_len as u64
        + region_len as u64
        + front_sections_len
        + data_len
        + FOOTER.len() as u64;
    if total < MIN_FILE_LEN {
        region_len += (MIN_FILE_LEN - total) as u32;
    }
    let data_offset =
        HEADER_LEN as u64 + root_len as u64 + region_len as u64 + front_sections_len;

    // Data first, at its final offset, hashed per section.
    out.seek(SeekFrom::Start(data_offset))?;
    let n_front = fronts.len();
    for (i, d) in data.iter().enumerate() {
        let mut h = blake3::Hasher::new();
        copy_segs(input, &d.segs, out, &mut h)?;
        root.sections[n_front + i]
            .digest
            .copy_from_slice(&h.finalize().as_bytes()[..16]);
    }
    out.write_all(&FOOTER)?;
    let root_bytes = root.encode(codec);
    debug_assert_eq!(root_bytes.len() as u32, root_len);

    let mut header = Header06 {
        flags: (if parts.named { F_NAMED } else { 0 })
            | (if parts.quoted { F_QUOTED } else { 0 })
            | (if parts.text.as_ref().is_some_and(|t| t.named) {
                F_TEXT_NAMED
            } else {
                0
            })
            | (if graph_major { F_QUAD_GRAPH_MAJOR } else { 0 }),
        content_hash: [0; 16],
        file_len: data_offset + data_len + FOOTER.len() as u64,
        root_len,
        card_region_len: region_len,
        data_offset,
        quad_count: parts.quad_count,
        term_count: parts.term_count,
        graph_count: parts.quad.as_ref().map_or(0, |q| q.graph_count),
        perm_mask: parts.perm_mask,
        dict_codec: parts.dict_codec,
        block_codec: parts.block_codec,
        internal_codec: codec,
        pyramid_levels: parts.pyramid_levels,
        presence: presence_of(parts),
        card_off: 0,
        card_len,
        card_digest: card.as_ref().map_or([0; 16], |c| digest16(c)),
        build_off: card_len,
        build_len,
        root_digest: digest16(&root_bytes),
    };
    header.content_hash = content_hash(&header);

    out.seek(SeekFrom::Start(0))?;
    out.write_all(&header.to_bytes())?;
    out.write_all(&root_bytes)?;
    let mut region = Vec::with_capacity(region_len as usize);
    if let Some(c) = &card {
        region.extend_from_slice(c);
    }
    if let Some(b) = &build {
        region.extend_from_slice(b);
    }
    region.resize(region_len as usize, 0);
    out.write_all(&region)?;
    for (_, b) in &fronts {
        out.write_all(b)?;
    }
    out.seek(SeekFrom::Start(header.file_len))?;
    out.flush()?;
    Ok(Written { header, root })
}

/// The smallest `0x06` file: 1 KiB, what every reader reads first anyway.
pub const MIN_FILE_LEN: u64 = 1024;

/// The card region's length for a card and record of these stored lengths:
/// both, twice over, plus [`CARD_SLACK_MIN`] — so a re-card can write the new
/// pair beside the old one and switch with one header write (SPEC.md §6).
/// No card and no record: no region.
pub fn card_region_len(card_len: u32, build_len: u32) -> u32 {
    if card_len == 0 && build_len == 0 {
        0
    } else {
        2 * (card_len + build_len) + CARD_SLACK_MIN
    }
}

fn presence_of(p: &Parts) -> u16 {
    let mut x = 0u16;
    if let Some(t) = &p.text {
        x |= P_TEXT;
        if t.partial {
            x |= P_TEXT_PARTIAL;
        }
    }
    if p.labels.is_some() {
        x |= P_LABELS;
    }
    if p.schema.is_some() {
        x |= P_SCHEMA;
    }
    if p.pyramid.is_some_and(|r| r.len > 0) {
        x |= P_PYRAMID;
    }
    if p.quad.as_ref().is_some_and(|q| q.graph_count > 0) {
        x |= P_QUAD;
    }
    x
}

/// Convert a whole `0x05` image held in memory to a `0x06` image.
pub fn convert_image(image: &[u8], opts: WriteOpts) -> Result<Vec<u8>, FileError> {
    let r = crate::reader::SliceReader::new(image);
    let parts = parts::from_v05(&r)?;
    let mut out = std::io::Cursor::new(Vec::with_capacity(image.len()));
    assemble(&parts, &r, &mut out, opts)?;
    Ok(out.into_inner())
}

/// Convert a `0x05` file read through `input` into `out`.
pub fn convert<W: Write + Seek>(
    input: &dyn RangeReader,
    out: &mut W,
    opts: WriteOpts,
) -> Result<Written, FileError> {
    let parts = parts::from_v05(input)?;
    assemble(&parts, input, out, opts)
}

/// Replace the card (and the build record) of a `0x06` image held in memory:
/// the front is re-laid out with the new card region; every section is copied
/// byte for byte. The content hash follows the card.
pub fn replace_card(
    image: &[u8],
    card: Option<&[u8]>,
    build: Option<&[u8]>,
) -> Result<Vec<u8>, FileError> {
    let h = Header06::from_bytes(image)?;
    let codec = h.internal_codec;
    let root_bytes = image
        .get(HEADER_LEN..HEADER_LEN + h.root_len as usize)
        .ok_or(FileError::Container("0x06 root overruns"))?;
    let card_stored = card.filter(|c| !c.is_empty()).map(|c| compress(codec, c));
    let card_len = card_stored.as_ref().map_or(0, |c| c.len()) as u32;
    let build_len = build.map_or(0, |b| b.len()) as u32;
    let mut region_len = card_region_len(card_len, build_len);
    let rest_len = h.file_len - h.front_base();
    let total = HEADER_LEN as u64 + h.root_len as u64 + region_len as u64 + rest_len;
    if total < MIN_FILE_LEN {
        region_len += (MIN_FILE_LEN - total) as u32;
    }
    let old_front = h.front_base() as usize;
    let rest = image
        .get(old_front..)
        .ok_or(FileError::Container("0x06 front overruns"))?;
    let mut nh = h.clone();
    nh.card_region_len = region_len;
    nh.card_off = 0;
    nh.card_len = card_len;
    nh.card_digest = card_stored.as_ref().map_or([0; 16], |c| digest16(c));
    nh.build_off = card_len;
    nh.build_len = build_len;
    let shift = region_len as i64 - h.card_region_len as i64;
    nh.data_offset = (h.data_offset as i64 + shift) as u64;
    nh.file_len = (h.file_len as i64 + shift) as u64;
    nh.content_hash = content_hash(&nh);
    let mut out = Vec::with_capacity(nh.file_len as usize);
    out.extend_from_slice(&nh.to_bytes());
    out.extend_from_slice(root_bytes);
    let mut region = Vec::with_capacity(region_len as usize);
    if let Some(c) = &card_stored {
        region.extend_from_slice(c);
    }
    if let Some(b) = build {
        region.extend_from_slice(b);
    }
    region.resize(region_len as usize, 0);
    out.extend_from_slice(&region);
    out.extend_from_slice(rest);
    Ok(out)
}
