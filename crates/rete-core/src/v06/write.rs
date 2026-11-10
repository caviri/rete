//! The `0x06` assembler: [`super::parts::Parts`] in, a `0x06` file out. Every
//! writer goes through here — the in-RAM build (via `0x05` section images),
//! `rete convert`, and the memory-bounded build and `merge` (which hand it their
//! spill as an address space, see `extbuild_v06`). `recard` only re-lays the
//! front.
//!
//! The front (root, card region, stats, leaf directories) is built in memory;
//! the data region is streamed from the input reader, hashed on the way. The
//! data is written first at its final offset, then the front at offset 0, so the
//! output needs `Write + Seek` and the input is read once.

use std::io::{Seek, SeekFrom, Write};

use super::parts::Parts;
use super::*;
use crate::file::{compress, FileError};
use crate::header::CURRENT_FORMAT_VERSION;
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

/// How big the writer makes directory leaves (SPEC.md §9): small, because a
/// leaf is what a cold lookup transfers; the root carries the leaf lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeafPolicy {
    /// Uncompressed size a leaf is cut at to begin with.
    pub start: usize,
    /// The largest a leaf grows to in order to bring the root under
    /// `root_budget`. Growing happens only if one leaf per directory would
    /// fit the budget.
    pub max: usize,
    /// Stored root size the writer tries to stay under.
    pub root_budget: usize,
    /// The most the leaf lists may add to the root of one leaf per directory;
    /// past that every directory's leaves grow, beyond `max`.
    pub list_cap: usize,
}

impl Default for LeafPolicy {
    fn default() -> Self {
        LeafPolicy {
            start: LEAF_START,
            max: LEAF_MAX,
            root_budget: ROOT_BUDGET,
            list_cap: LIST_CAP,
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
    /// Directory leaf sizes.
    pub leaves: LeafPolicy,
    /// The generation to write: [`VERSION`] (always, for every real writer) or
    /// [`VERSION_06`], which the tests use to make a `0x06` input for `convert`
    /// (label root inside the root, no annex).
    pub version: u8,
    /// Cluster the subject-leading permutations by class (`0x07` only); `None`
    /// writes them in subject order, and un-clusters a clustered input.
    pub cluster: Option<crate::write_opts::ClusterOpts>,
}

impl WriteOpts {
    /// These options with class clustering on (`Some`) or off.
    pub fn with_cluster(mut self, c: Option<crate::write_opts::ClusterOpts>) -> Self {
        self.cluster = c;
        self
    }
}

impl Default for WriteOpts {
    fn default() -> Self {
        WriteOpts {
            internal_codec: crate::file::writer_codec(),
            quad: QuadClustering::from_env(),
            leaves: LeafPolicy::default(),
            version: VERSION,
            cluster: crate::write_opts::cluster(),
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
    /// The block is stored uncompressed (a raw quad tile).
    raw: bool,
}

/// A data section: its kind and the input segments, in output order.
struct DataSec {
    kind: u8,
    segs: Vec<Seg>,
    /// Each segment is one compressed block of this codec, whose decompressed
    /// size the writer records ([`Sec06::max_block`]); `None` = not measured.
    measure: Option<u8>,
    /// A largest block known without measuring (the dictionary's, from the
    /// rekey), or 0.
    known_max: u64,
    /// Bytes written from memory rather than copied from the input (the card
    /// annex); `segs` is empty then.
    inline: Option<Vec<u8>>,
}

impl DataSec {
    fn len(&self) -> u64 {
        if let Some(b) = &self.inline {
            return b.len() as u64;
        }
        self.segs.iter().map(|s| s.len).sum()
    }
}

/// Leaf groups whose leaf size the root budget may grow.
#[derive(Clone)]
struct Targets {
    dict: [usize; 4],
    index: [usize; 6],
    quad: [usize; crate::quad7::NUM_SLOTS],
    text: usize,
}

/// The front built for a set of leaf targets.
struct Front {
    units: Vec<(u8, Vec<u8>)>,
    dict_leaves: Vec<u8>,
    index_leaves: Vec<u8>,
    quad_leaves: Vec<u8>,
    text_leaves: Vec<u8>,
    /// The largest leaf of each leaves section, decompressed: dict, index,
    /// quad, text.
    leaf_max: [u64; 4],
    /// Leaves per group, to pick which group to grow: `(group id, count)`.
    counts: Vec<(usize, usize)>,
}

/// Leaf-group id of the text index (dictionary 0..4, index 4..10, quad 10..19).
const TEXT_GROUP: usize = 19;

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
                raw: false,
            });
            at += c.clen;
        }
    }
    if at > 0 {
        secs.push(DataSec {
            kind: kind::DICT_DATA,
            segs,
            measure: None,
            known_max: parts.dict.iter().map(|d| d.max_raw).max().unwrap_or(0),
            inline: None,
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
                raw: false,
            });
            at += t.clen;
        }
    }
    if at > 0 {
        secs.push(DataSec {
            kind: kind::INDEX_DATA,
            segs,
            measure: Some(parts.block_codec),
            known_max: 0,
            inline: None,
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
                            raw: t.raw,
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
                        raw: t.raw,
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
                        raw: t.raw,
                    });
                    at += t.clen;
                }
            }
        }
        if at > 0 {
            secs.push(DataSec {
                kind: kind::QUAD_DATA,
                segs,
                measure: Some(parts.block_codec),
                known_max: 0,
                inline: None,
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
                    raw: false,
                }],
                measure: None,
                known_max: 0,
                inline: None,
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
                    raw: false,
                }],
                measure: None,
                known_max: 0,
                inline: None,
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
                    raw: false,
                }],
                measure: None,
                known_max: 0,
                inline: None,
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
    max_raw: &mut u64,
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
        *max_raw = (*max_raw).max(raw.len() as u64);
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

#[allow(clippy::too_many_arguments)]
fn build_front(
    parts: &Parts,
    t: &Targets,
    codec: u8,
    dict_off: &[u64; 4],
    perm_off: &[u64],
    quad_off: &[Vec<u64>],
    quad_gdir: &[u8],
    version: u8,
) -> Front {
    let mut f = Front {
        units: Vec::new(),
        dict_leaves: Vec::new(),
        index_leaves: Vec::new(),
        quad_leaves: Vec::new(),
        text_leaves: Vec::new(),
        leaf_max: [0; 4],
        counts: Vec::new(),
    };
    // Dictionary.
    let mut droots: [DictSecRoot; 4] = Default::default();
    for &s in &DICT_ORDER {
        let d = &parts.dict[s];
        let ents: Vec<ChunkEnt> = d.chunks.iter().map(|(c, _)| c.clone()).collect();
        let leaves = leaves_of(
            &ents,
            t.dict[s],
            codec,
            &mut f.dict_leaves,
            &mut f.leaf_max[0],
            |e, _| encode_chunk_leaf(e),
        );
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
        // A clustered permutation cuts its leaves at the runs: no leaf spans two.
        let runs: Vec<(usize, usize)> = match parts.cluster.iter().find(|c| c.si == p.si) {
            Some(c) => {
                let mut at = 0;
                c.run_tiles
                    .iter()
                    .map(|&n| {
                        at += n;
                        (at - n, at)
                    })
                    .collect()
            }
            None => vec![(0, ents.len())],
        };
        let mut leaves = Vec::new();
        for (a, b) in runs {
            for mut l in leaves_of(
                &ents[a..b],
                t.index[p.si],
                codec,
                &mut f.index_leaves,
                &mut f.leaf_max[1],
                |e, _| encode_tile_leaf(e),
            ) {
                l.first += a;
                leaves.push(l);
            }
        }
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
    if !parts.cluster.is_empty() {
        f.units
            .push((tag::CLUSTER, encode_cluster_root(&parts.cluster)));
    }
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
            let leaves = leaves_of(
                &ents,
                t.quad[*si],
                codec,
                &mut f.quad_leaves,
                &mut f.leaf_max[2],
                |e, i| {
                    let _ = i;
                    encode_qtile_leaf(e, e[0].off, version == VERSION)
                },
            );
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
            // 0x07: the unit root keeps the 32-byte preamble only; the root
            // node follows in its own section (kind LABEL_ROOT).
            encode_label_root(&LabelRoot {
                data_len: l.data_len,
                head: if version == VERSION {
                    l.head[..l.head.len().min(crate::label_index::PREAMBLE_LEN)].to_vec()
                } else {
                    l.head.clone()
                },
            }),
        ));
    }
    // Text.
    if let Some(tp) = &parts.text {
        let leaves = leaves_of(
            &tp.tokens,
            t.text,
            codec,
            &mut f.text_leaves,
            &mut f.leaf_max[3],
            |e, _| encode_token_leaf(e),
        );
        f.counts.push((TEXT_GROUP, leaves.len()));
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
#[allow(clippy::too_many_arguments)]
fn copy_segs(
    input: &dyn RangeReader,
    segs: &[Seg],
    measure: Option<u8>,
    out: &mut dyn Write,
    h: &mut blake3::Hasher,
    progress: &mut dyn FnMut(u64),
    consumed: &mut dyn FnMut(u64, u64),
    settle: &mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
) -> Result<u64, FileError> {
    const SPAN: u64 = 8 << 20;
    // The ceiling for measuring a block whose frame does not state its size:
    // generous, since this only bounds the writer's own check.
    const MEASURE_CEILING: u64 = 1 << 30;
    let mut max_raw = 0u64;
    let mut done = 0u64;
    let mut i = 0;
    while i < segs.len() {
        let start = segs[i].input;
        let mut end = start + segs[i].len;
        let mut j = i + 1;
        while j < segs.len() && segs[j].input == end && end - start < SPAN {
            end += segs[j].len;
            j += 1;
        }
        if let Some(codec) = measure {
            // Blocks are measured whole, so the run is read in one piece.
            let b = input.read_at(start, end - start)?;
            for sg in &segs[i..j] {
                let lo = (sg.input - start) as usize;
                let blk = &b[lo..lo + sg.len as usize];
                let n = if sg.raw {
                    sg.len
                } else {
                    crate::file::block_raw_len(codec, blk, MEASURE_CEILING)?
                };
                max_raw = max_raw.max(n);
            }
            h.update(&b);
            out.write_all(&b)?;
            done += b.len() as u64;
            progress(done);
        } else {
            let mut at = start;
            while at < end {
                let n = (end - at).min(SPAN);
                let b = input.read_at(at, n)?;
                h.update(&b);
                out.write_all(&b)?;
                at += n;
                done += n;
                progress(done);
            }
        }
        for sg in &segs[i..j] {
            consumed(sg.input, sg.len);
        }
        settle(out)?;
        i = j;
    }
    Ok(max_raw)
}

/// The largest graph-directory block of a quad index, decompressed.
fn gdir_max(p0: &[u8], gdir: &[u8], codec: u8) -> Result<u64, FileError> {
    let mut c = Cur::new(p0);
    let n = c.count(u32::MAX as u64)?;
    let (mut at, mut max) = (0usize, 0u64);
    for _ in 0..n {
        let _ = c.u32v()?;
        let packed = c.uv()?;
        let len = (packed >> 1) as usize;
        let blk = gdir
            .get(at..at + len)
            .ok_or(FileError::Container("quad graph directory overruns"))?;
        let raw = if packed & 1 == 1 {
            len as u64
        } else {
            crate::file::block_raw_len(codec, blk, 1 << 30)?
        };
        max = max.max(raw);
        at += len;
    }
    Ok(max)
}

/// Assemble a `0x06` file from `parts`, whose payload ranges point into `input`.
pub fn assemble<W: Write + Seek>(
    parts: &Parts,
    input: &dyn RangeReader,
    out: &mut W,
    opts: WriteOpts,
) -> Result<Written, FileError> {
    assemble_with(
        parts,
        input,
        out,
        opts,
        &mut Hooks {
            copied: &mut |_| {},
            progress: &mut |_, _| {},
            needed: &mut |_, _| {},
            consumed: &mut |_, _| {},
            settle: &mut |_| Ok(()),
        },
    )
}

/// What [`assemble_with`] reports while it writes.
pub struct Hooks<'a> {
    /// Called once each data section (a [`kind`] of `DICT_DATA`, `INDEX_DATA`,
    /// `QUAD_DATA`, `LABEL_DATA`, `TEXT_DATA` or `PYRAMID`) is in the output:
    /// the input bytes it came from are not read again.
    pub copied: &'a mut dyn FnMut(u8),
    /// `(data bytes written so far, data bytes in all)`, after each write.
    pub progress: &'a mut dyn FnMut(u64, u64),
    /// `(input offset, length)` of every payload range the copy will read,
    /// announced once before the first byte is written.
    pub needed: &'a mut dyn FnMut(u64, u64),
    /// `(input offset, length)` of a payload range now in the output, whatever
    /// order the ranges come in. Once every announced range of a spill file
    /// has been consumed, nothing reads that file again.
    pub consumed: &'a mut dyn FnMut(u64, u64),
    /// Called with the output after every copied run and every finished data
    /// section, once `consumed` / `copied` may have made spill files ready to
    /// go. The implementation flushes `out`, makes the bytes durable, and only
    /// then deletes: a file must never be removed while its bytes are still in
    /// a buffer or an unsynced page.
    pub settle: &'a mut dyn FnMut(&mut dyn Write) -> std::io::Result<()>,
}

/// [`assemble`], reporting through `hooks`. The memory-bounded build deletes
/// each spill file as soon as every range it feeds is in the output, so the
/// output grows as the spill shrinks, and drives `--progress` from the byte
/// count.
pub fn assemble_with<W: Write + Seek>(
    parts: &Parts,
    input: &dyn RangeReader,
    out: &mut W,
    opts: WriteOpts,
    hooks: &mut Hooks<'_>,
) -> Result<Written, FileError> {
    // `0x07` writes SPOG, POSG and OSPG next to GSPO: derive them from the GSPO
    // tiles first (the other graph-leading orders of the input are dropped).
    if opts.version == VERSION && super::neworders::needed(parts) {
        let (derived, extras) = super::neworders::derive(parts, input)?;
        let layered = super::neworders::Layered::new(input, &extras);
        return assemble_with(&derived, &layered, out, opts, hooks);
    }
    // `--cluster-subjects-by-class`: the subject-leading permutations by class
    // (or back to subject order). Another pass over the input, same shape.
    if opts.version == VERSION && super::cluster::needed(parts, opts.cluster) {
        if let Some((derived, extras)) = super::cluster::derive(parts, input, opts.cluster)? {
            let layered = super::neworders::Layered::new(input, &extras);
            return assemble_with(&derived, &layered, out, opts, hooks);
        }
    }
    let codec = opts.internal_codec;
    if parts.pyramid_levels > u8::MAX as u16 {
        return Err(FileError::Container(
            "0x06 stores at most 255 pyramid levels",
        ));
    }
    // Ids are dense and 1-based, so no position holds an id above the term
    // count (graphs: the graph count). Checked, never truncated.
    let id_widths = super::id_widths_for([
        parts.term_count,
        parts.term_count,
        parts.term_count,
        parts.quad.as_ref().map_or(0, |q| q.graph_count),
    ])?;
    // `0x07` is order-major, always: its graph-leading order is one, and the
    // graph-last orders have no graph neighbourhood to cluster on.
    // Quad clustering (`0x06`): graph-major when the mean graph is under a tile.
    let graph_major = match (&parts.quad, opts.quad) {
        (None, _) => false,
        (_, _) if opts.version == VERSION => false,
        (Some(_), QuadClustering::GraphMajor) => true,
        (Some(_), QuadClustering::OrderMajor) => false,
        (Some(q), QuadClustering::Auto) => {
            let bytes: u64 = q.orders.iter().flat_map(|(_, t)| t).map(|t| t.clen).sum();
            q.graph_count > 0 && bytes / q.graph_count.max(1) < 64 * 1024
        }
    };
    let (data, dict_off, perm_off, quad_off) = layout_data(parts, graph_major);
    for d in &data {
        for sg in &d.segs {
            (hooks.needed)(sg.input, sg.len);
        }
    }
    let quad_gdir = match &parts.quad {
        Some(q) if q.gdir.len > 0 => input.read_at(q.gdir.offset, q.gdir.len)?,
        _ => Vec::new(),
    };
    let label_nodes = match &parts.labels {
        Some(l) if l.nodes.is_some_and(|n| n.len > 0) => {
            let n = l.nodes.unwrap_or_default();
            input.read_at(n.offset, n.len)?
        }
        Some(l) if l.nodes.is_none() && l.body.len > l.data_len => {
            input.read_at(l.body.offset + l.data_len, l.body.len - l.data_len)?
        }
        _ => Vec::new(),
    };

    // Leaf policy (`opts.leaves`). A leaf is a cold lookup's unit of transfer,
    // so leaves stay small (`start`) and the root carries the leaf lists.
    //  1. They grow, up to `max`, only to bring the stored root under
    //     `root_budget`, and only when that can work: if the root is over
    //     budget even with one leaf per directory (the graph directory's block
    //     index alone can do that), bigger leaves would cost every lookup
    //     bytes and buy nothing.
    //  2. The leaf lists add at most `list_cap` to the root of one leaf per
    //     directory; past that every directory's leaves double, beyond `max`.
    let pol = opts.leaves;
    let root_len_of = |front: &Front| {
        Root06 {
            sections: vec![
                Sec06 {
                    kind: 0,
                    max_block: 0,
                    offset: 0,
                    len: 0,
                    digest: [0; 16]
                };
                kind::ORDER.len()
            ],
            units: front.units.clone(),
        }
        .encode(codec)
        .len()
    };
    let size_of = |t: &Targets, g: usize| match g {
        0..=3 => t.dict[g],
        4..=9 => t.index[g - 4],
        10..=18 => t.quad[g - 10],
        _ => t.text,
    };
    let grow = |t: &mut Targets, g: usize| match g {
        0..=3 => t.dict[g] = t.dict[g].saturating_mul(2),
        4..=9 => t.index[g - 4] = t.index[g - 4].saturating_mul(2),
        10..=18 => t.quad[g - 10] = t.quad[g - 10].saturating_mul(2),
        _ => t.text = t.text.saturating_mul(2),
    };
    let build = |t: &Targets| {
        build_front(
            parts,
            t,
            codec,
            &dict_off,
            &perm_off,
            &quad_off,
            &quad_gdir,
            opts.version,
        )
    };
    let mut t = Targets {
        dict: [pol.start; 4],
        index: [pol.start; 6],
        quad: [pol.start; crate::quad7::NUM_SLOTS],
        text: pol.start,
    };
    let mut front = build(&t);
    let mut root_len = root_len_of(&front);
    if root_len > pol.root_budget && front.counts.iter().any(|(_, n)| *n > 1) {
        // The root with one leaf per directory: what no leaf size can go under.
        let one = usize::MAX / 4;
        let floor = root_len_of(&build(&Targets {
            dict: [one; 4],
            index: [one; 6],
            quad: [one; crate::quad7::NUM_SLOTS],
            text: one,
        }));
        if floor <= pol.root_budget {
            while root_len > pol.root_budget {
                let pick = front
                    .counts
                    .iter()
                    .filter(|(g, n)| *n > 1 && size_of(&t, *g) < pol.max)
                    .max_by_key(|(_, n)| *n);
                let Some(&(g, _)) = pick else {
                    break;
                };
                grow(&mut t, g);
                front = build(&t);
                root_len = root_len_of(&front);
            }
        }
        while root_len > floor + pol.list_cap && front.counts.iter().any(|(_, n)| *n > 1) {
            for (g, n) in front.counts.clone() {
                if n > 1 {
                    grow(&mut t, g);
                }
            }
            front = build(&t);
            root_len = root_len_of(&front);
        }
    }

    // Front sections, in file order, with their largest block.
    let schema = parts
        .schema
        .as_ref()
        .map(|s| (compress(codec, s), s.len() as u64));
    let quad_gdir_max = match &parts.quad {
        Some(q) if !quad_gdir.is_empty() => gdir_max(&q.p0, &quad_gdir, parts.block_codec)?,
        _ => 0,
    };
    let mut fronts: Vec<(u8, Vec<u8>, u64)> = Vec::new();
    if let Some((s, raw)) = schema {
        fronts.push((kind::SCHEMA, s, raw));
    }
    if !front.dict_leaves.is_empty() {
        fronts.push((
            kind::DICT_LEAVES,
            std::mem::take(&mut front.dict_leaves),
            front.leaf_max[0],
        ));
    }
    if !front.index_leaves.is_empty() {
        fronts.push((
            kind::INDEX_LEAVES,
            std::mem::take(&mut front.index_leaves),
            front.leaf_max[1],
        ));
    }
    if !front.quad_leaves.is_empty() {
        fronts.push((
            kind::QUAD_LEAVES,
            std::mem::take(&mut front.quad_leaves),
            front.leaf_max[2].max(quad_gdir_max),
        ));
    }
    if !label_nodes.is_empty() {
        fronts.push((kind::LABEL_LEAVES, label_nodes, 0));
    }
    if !front.text_leaves.is_empty() {
        fronts.push((
            kind::TEXT_LEAVES,
            std::mem::take(&mut front.text_leaves),
            front.leaf_max[3],
        ));
    }
    // The label index's root node: cold, read only by a label search.
    if let (Some(l), true) = (&parts.labels, opts.version == VERSION) {
        let pre = crate::label_index::PREAMBLE_LEN;
        if l.head.len() > pre {
            fronts.push((kind::LABEL_ROOT, l.head[pre..].to_vec(), 0));
        }
    }
    let clamp = |n: u64| n.min(u32::MAX as u64) as u32;

    // Card region: card | record | slack.
    // A card with a field over the hot cap is split: the field goes to the
    // annex, the last data section. A card handed over with its annex (a
    // `0x07` source) keeps both as they are.
    let (hot_card, annex_raw) = match (&parts.card, &parts.annex) {
        (Some(c), None) if !c.is_empty() => {
            let s = super::annex::split(c);
            (Some(s.hot), s.annex)
        }
        (c, a) => (c.clone(), a.clone()),
    };
    let card = hot_card
        .as_ref()
        .filter(|c| !c.is_empty())
        .map(|c| compress(codec, &stamp_card_version(c)));
    let mut data = data;
    if let Some(raw) = annex_raw.as_ref().filter(|a| !a.is_empty()) {
        if opts.version != VERSION {
            return Err(FileError::Container("0x06 has no card annex"));
        }
        data.push(DataSec {
            kind: kind::CARD_ANNEX,
            segs: Vec::new(),
            measure: None,
            known_max: raw.len() as u64,
            inline: Some(compress(codec, raw)),
        });
    }
    let build = parts.build.clone().filter(|b| !b.is_empty());
    let card_len = card.as_ref().map_or(0, |c| c.len()) as u32;
    let build_len = build.as_ref().map_or(0, |b| b.len()) as u32;
    let mut region_len = card_region_len(card_len, build_len);

    // Section table (digests filled once the data is hashed).
    let mut sections = Vec::new();
    let mut at = 0u64;
    for (k, b, raw) in &fronts {
        sections.push(Sec06 {
            kind: *k,
            max_block: clamp(*raw),
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
            max_block: clamp(d.known_max),
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
    let data_offset = HEADER_LEN as u64 + root_len as u64 + region_len as u64 + front_sections_len;

    // Data first, at its final offset, hashed per section.
    out.seek(SeekFrom::Start(data_offset))?;
    let n_front = fronts.len();
    let mut written_before = 0u64;
    for (i, d) in data.iter().enumerate() {
        let mut h = blake3::Hasher::new();
        let progress = &mut hooks.progress;
        let consumed = &mut hooks.consumed;
        let settle = &mut hooks.settle;
        let measured = if let Some(b) = &d.inline {
            h.update(b);
            out.write_all(b)?;
            0
        } else {
            copy_segs(
                input,
                &d.segs,
                d.measure,
                out,
                &mut h,
                &mut |n| progress(written_before + n, data_len),
                &mut **consumed,
                &mut **settle,
            )?
        };
        written_before += d.len();
        (hooks.copied)(d.kind);
        (hooks.settle)(out)?;
        let sec = &mut root.sections[n_front + i];
        sec.digest.copy_from_slice(&h.finalize().as_bytes()[..16]);
        if d.measure.is_some() {
            sec.max_block = clamp(measured);
        }
    }
    out.write_all(&footer(opts.version))?;
    let root_bytes = root.encode(codec);
    debug_assert_eq!(root_bytes.len() as u32, root_len);

    let mut header = Header06 {
        version: opts.version,
        flags: (if parts.named { F_NAMED } else { 0 })
            | (if parts.quoted { F_QUOTED } else { 0 })
            | (if parts.text.as_ref().is_some_and(|t| t.named) {
                F_TEXT_NAMED
            } else {
                0
            })
            | (if graph_major { F_QUAD_GRAPH_MAJOR } else { 0 })
            | (if parts.cluster.is_empty() {
                0
            } else {
                F_CLUSTERED
            }),
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
        id_widths,
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
    for (_, b, _) in &fronts {
        out.write_all(b)?;
    }
    out.seek(SeekFrom::Start(header.file_len))?;
    out.flush()?;
    Ok(Written { header, root })
}

/// The smallest `0x06` file: 1 KiB, what every reader reads first anyway.
pub const MIN_FILE_LEN: u64 = 1024;

/// The card region's length for a card and record of these stored lengths.
///
/// With a card: both, twice over, plus [`CARD_SLACK_MIN`] — so a re-card can
/// write the new pair beside the old one and switch with one header write
/// (SPEC.md §4.4). With **no card** the region is the record alone: that space
/// is in the 64 KiB the open reads first, so slack there would only push the
/// leaf directories out of it, and a file nobody carded yet is re-carded by a
/// copy. No card and no record: no region.
pub fn card_region_len(card_len: u32, build_len: u32) -> u32 {
    match (card_len, build_len) {
        (0, 0) => 0,
        (0, b) => b,
        (c, b) => 2 * (c + b) + CARD_SLACK_MIN,
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

/// A card written into a `0x07` file says `"format_version":7`. The cards the
/// core builders derive stamp the `0x05` layout they were produced for
/// (`LEGACY_FORMAT_VERSION`); every card entering a `0x06` file passes here, so
/// a build, a converted build and a re-card agree on the stamp. Only the first
/// compact `"format_version":N` with N below 7 is rewritten; anything else is
/// returned unchanged.
pub fn stamp_card_version(card: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    const KEY: &[u8] = b"\"format_version\":";
    let Some(at) = card.windows(KEY.len()).position(|w| w == KEY) else {
        return std::borrow::Cow::Borrowed(card);
    };
    let s = at + KEY.len();
    let digits = card[s..].iter().take_while(|b| b.is_ascii_digit()).count();
    let old = std::str::from_utf8(&card[s..s + digits]).unwrap_or("");
    match old.parse::<u8>() {
        Ok(v) if v < CURRENT_FORMAT_VERSION => {
            let mut out = card[..s].to_vec();
            out.extend_from_slice(CURRENT_FORMAT_VERSION.to_string().as_bytes());
            out.extend_from_slice(&card[s + digits..]);
            std::borrow::Cow::Owned(out)
        }
        _ => std::borrow::Cow::Borrowed(card),
    }
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
    convert_with_labels(input, None, out, opts)
}

/// [`convert`], giving the file the LABEL_INDEX section `labels` (its `0x05`
/// encoding, from [`label_section`]) when it has none.
pub fn convert_with_labels<W: Write + Seek>(
    input: &dyn RangeReader,
    labels: Option<&[u8]>,
    out: &mut W,
    opts: WriteOpts,
) -> Result<Written, FileError> {
    convert_with_record(input, labels, None, out, opts)
}

/// The source file's build record as stored (raw JSON, possibly padded), or
/// `None` when it has none — what [`convert_with_record`] is given to extend.
pub fn source_build_record(input: &dyn RangeReader) -> Result<Option<Vec<u8>>, FileError> {
    let len = input.len().min(crate::header::HEADER_LEN as u64);
    let h = crate::header::Header::from_bytes(&input.read_at(0, len)?)?;
    if h.build_info_len == 0 {
        return Ok(None);
    }
    Ok(Some(input.read_at(h.build_info_offset, h.build_info_len)?))
}

/// [`convert_with_labels`], writing `record` as the build record instead of
/// the input's own (`None` keeps the input's, byte for byte). The record is
/// outside the content hash.
pub fn convert_with_record<W: Write + Seek>(
    input: &dyn RangeReader,
    labels: Option<&[u8]>,
    record: Option<Vec<u8>>,
    out: &mut W,
    opts: WriteOpts,
) -> Result<Written, FileError> {
    let mut parts = parts::from_v05(input)?;
    if record.is_some() {
        parts.build = record;
    }
    let Some(sec) = labels.filter(|s| !s.is_empty() && parts.labels.is_none()) else {
        return assemble(&parts, input, out, opts);
    };
    // The new section is read through an overlay that places it right after
    // the input, so the assembler copies it like any other.
    let overlay = Overlay {
        base: input,
        extra: sec,
    };
    parts.labels = Some(parts::label_part(
        &overlay,
        input.len(),
        sec.len() as u64,
        parts.block_codec,
    )?);
    assemble(&parts, &overlay, out, opts)
}

/// `base`, then `extra` past its end.
struct Overlay<'a> {
    base: &'a dyn RangeReader,
    extra: &'a [u8],
}

impl RangeReader for Overlay<'_> {
    fn len(&self) -> u64 {
        self.base.len() + self.extra.len() as u64
    }

    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        let b = self.base.len();
        if offset + len <= b {
            return self.base.read_at(offset, len);
        }
        if offset >= b {
            let s = (offset - b) as usize;
            return self
                .extra
                .get(s..s + len as usize)
                .map(<[u8]>::to_vec)
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "oob"));
        }
        let mut v = self.base.read_at(offset, b - offset)?;
        v.extend_from_slice(self.read_at(b, offset + len - b)?.as_slice());
        Ok(v)
    }
}

/// The LABEL_INDEX section (`0x05` encoding) of the default graph of `rete`,
/// computed as `rete build --label-index` computes it — from the triples of
/// the label predicates only, read through the index. Empty when the graph
/// has no label predicate.
pub fn label_section(rete: &crate::file::Rete) -> Vec<u8> {
    use crate::label_index::{
        DIRECT_LABEL_PREDICATES, FAMILY_NAME_PREDICATES, GIVEN_NAME_PREDICATES,
    };
    let dict = rete.dictionary();
    let index = rete.default_index();
    let mut triples: Vec<(u32, u32, u32)> = Vec::new();
    for list in [
        DIRECT_LABEL_PREDICATES,
        FAMILY_NAME_PREDICATES,
        GIVEN_NAME_PREDICATES,
    ] {
        for p in list {
            if let Some(pid) = dict.predicate_id(p) {
                triples.extend(index.scan_iter((None, Some(pid), None)));
            }
        }
    }
    triples.sort_unstable();
    triples.dedup();
    let entries = crate::label_index::entries_from_sorted_triples(dict, &triples);
    if entries.is_empty() {
        return Vec::new();
    }
    crate::label_index::encode_section(&entries, crate::file::writer_codec())
}

/// The card a re-card writes.
#[derive(Debug, Clone, Copy)]
pub enum NewCard<'a> {
    /// The file's own card, byte for byte (only the build record changes, so
    /// the content hash does not).
    Keep,
    /// This card (JSON), or none. A field over the hot cap moves to the annex
    /// ([`super::annex::split`]); the full card goes in, the hot one is stored.
    Set(Option<&'a [u8]>),
}

/// A card change, resolved: what is stored in the card region and what happens
/// to the annex (the last data section).
struct CardPlan {
    /// The hot card, stored (compressed); `None` = no card.
    stored: Option<Vec<u8>>,
    /// The root block to write: the old one unless the annex changes (its
    /// section-table entry then does, and with it the root digest).
    root_bytes: Vec<u8>,
    /// The annex changes: `(offset where the old tail starts, new stored annex
    /// or empty for none)`. The data region is copied only up to the offset;
    /// the new annex and the footer follow.
    tail: Option<(u64, Vec<u8>)>,
}

/// Resolve `card` against the file `h` with root block `root_bytes`.
/// `old_card` is the file's stored card (for [`NewCard::Keep`]).
fn plan_card(
    h: &Header06,
    root_bytes: &[u8],
    card: NewCard<'_>,
    old_card: Option<Vec<u8>>,
) -> Result<CardPlan, FileError> {
    let codec = h.internal_codec;
    let keep = |stored: Option<Vec<u8>>| CardPlan {
        stored,
        root_bytes: root_bytes.to_vec(),
        tail: None,
    };
    let c = match card {
        NewCard::Keep => return Ok(keep(old_card)),
        NewCard::Set(c) => c.filter(|c| !c.is_empty()),
    };
    let (hot, annex_raw) = match c {
        Some(c) => {
            let s = annex::split(c);
            (Some(s.hot), s.annex)
        }
        None => (None, None),
    };
    let stored = hot.map(|c| compress(codec, &stamp_card_version(&c)));
    let root = Root06::decode(root_bytes, codec)?;
    let old = root.section(kind::CARD_ANNEX).copied();
    let new_stored = annex_raw.as_ref().map(|a| compress(codec, a));
    let same = match (&old, &new_stored) {
        (None, None) => true,
        (Some(o), Some(n)) => o.len == n.len() as u64 && o.digest == digest16(n),
        _ => false,
    };
    if same {
        return Ok(keep(stored));
    }
    if h.version == VERSION_06 && new_stored.is_some() {
        return Err(FileError::Container(
            "this card has fields over the hot-card cap, which need a card annex; \
             a 0x06 file has none: run `rete convert` to 0x07 first, or trim the card",
        ));
    }
    let end = h.file_len - FOOTER.len() as u64;
    let cut = match &old {
        Some(o) => {
            let a = o.abs(h)?;
            if a.offset + a.len != end {
                return Err(FileError::Container(
                    "0x07 card annex is not the last section",
                ));
            }
            a.offset
        }
        None => end,
    };
    let mut nr = root.clone();
    nr.sections.retain(|s| s.kind != kind::CARD_ANNEX);
    if let (Some(n), Some(raw)) = (&new_stored, &annex_raw) {
        nr.sections.push(Sec06 {
            kind: kind::CARD_ANNEX,
            max_block: raw.len().min(u32::MAX as usize) as u32,
            offset: cut - h.data_offset,
            len: n.len() as u64,
            digest: digest16(n),
        });
    }
    Ok(CardPlan {
        stored,
        root_bytes: nr.encode(codec),
        tail: Some((cut, new_stored.unwrap_or_default())),
    })
}

/// The bytes after the copied part of the data region when the annex changes:
/// the new annex, then the footer.
fn tail_bytes(h: &Header06, annex: &[u8]) -> Vec<u8> {
    let mut v = annex.to_vec();
    v.extend_from_slice(&footer(h.version));
    v
}

/// Replace the card (and the build record) of a `0x06`/`0x07` image held in
/// memory: the front is re-laid out with the new card region; every section is
/// copied byte for byte (a changed card annex replaces the last section). The
/// content hash follows the card.
pub fn replace_card(
    image: &[u8],
    card: Option<&[u8]>,
    build: Option<&[u8]>,
) -> Result<Vec<u8>, FileError> {
    replace_card_with(image, NewCard::Set(card), build)
}

/// [`replace_card`] with the card given as a [`NewCard`]: `Keep` leaves the
/// file's own card (and its annex) as they are, so only the record changes.
pub fn replace_card_with(
    image: &[u8],
    card: NewCard<'_>,
    build: Option<&[u8]>,
) -> Result<Vec<u8>, FileError> {
    let h = Header06::from_bytes(image)?;
    let front = image
        .get(..h.data_offset as usize)
        .ok_or(FileError::Container("0x06 front overruns"))?;
    let root_bytes = front
        .get(HEADER_LEN..HEADER_LEN + h.root_len as usize)
        .ok_or(FileError::Container("0x06 root overruns"))?;
    let r = h.card_range();
    let old = (r.len > 0)
        .then(|| front.get(r.offset as usize..(r.offset + r.len) as usize))
        .flatten()
        .map(<[u8]>::to_vec);
    let plan = plan_card(&h, root_bytes, card, old)?;
    let end = plan.tail.as_ref().map_or(h.file_len, |t| t.0);
    let mut rest = image
        .get(h.data_offset as usize..end as usize)
        .ok_or(FileError::Container("0x06 data overruns"))?
        .to_vec();
    if let Some((_, a)) = &plan.tail {
        rest.extend_from_slice(&tail_bytes(&h, a));
    }
    let (_, mut out) = recard_front(
        &h,
        front,
        &plan.root_bytes,
        plan.stored.as_deref(),
        build,
        rest.len() as u64,
    )?;
    out.extend_from_slice(&rest);
    Ok(out)
}

/// The header and front (everything before the data region) of `h`'s file
/// with a new card and build record: the card region re-laid out (no slack
/// reuse: a fresh region sized for the new pair), the root block as given
/// (`root_bytes`), the front sections copied, the data region (`new_data_len`
/// bytes) shifted by the front's change.
fn recard_front(
    h: &Header06,
    front: &[u8],
    root_bytes: &[u8],
    card_stored: Option<&[u8]>,
    build: Option<&[u8]>,
    new_data_len: u64,
) -> Result<(Header06, Vec<u8>), FileError> {
    let sections = front
        .get(h.front_base() as usize..h.data_offset as usize)
        .ok_or(FileError::Container("0x06 front overruns"))?;
    let card_len = card_stored.map_or(0, |c| c.len()) as u32;
    let build_len = build.map_or(0, |b| b.len()) as u32;
    let mut region_len = card_region_len(card_len, build_len);
    let total = HEADER_LEN as u64
        + root_bytes.len() as u64
        + region_len as u64
        + sections.len() as u64
        + new_data_len;
    if total < MIN_FILE_LEN {
        region_len += (MIN_FILE_LEN - total) as u32;
    }
    let mut nh = h.clone();
    nh.root_len = root_bytes.len() as u32;
    nh.root_digest = digest16(root_bytes);
    nh.card_region_len = region_len;
    nh.card_off = 0;
    nh.card_len = card_len;
    nh.card_digest = card_stored.map_or([0; 16], digest16);
    nh.build_off = card_len;
    nh.build_len = build_len;
    nh.data_offset =
        HEADER_LEN as u64 + root_bytes.len() as u64 + region_len as u64 + sections.len() as u64;
    nh.file_len = nh.data_offset + new_data_len;
    nh.content_hash = content_hash(&nh);
    let mut out = Vec::with_capacity(nh.data_offset as usize);
    out.extend_from_slice(&nh.to_bytes());
    out.extend_from_slice(root_bytes);
    let mut region = Vec::with_capacity(region_len as usize);
    if let Some(c) = card_stored {
        region.extend_from_slice(c);
    }
    if let Some(b) = build {
        region.extend_from_slice(b);
    }
    region.resize(region_len as usize, 0);
    out.extend_from_slice(&region);
    out.extend_from_slice(sections);
    Ok((nh, out))
}

/// Rewrite a `0x06`/`0x07` file read through `src` into `out` with a new card
/// and build record: [`replace_card`] without holding the file. The front is
/// re-laid out around the new card region and the data region copied byte for
/// byte (its digests unchanged); a changed card annex replaces the file's last
/// section, and the root with it. `progress` receives the data bytes copied so
/// far.
pub fn rewrite_card_stream<W: Write>(
    src: &dyn RangeReader,
    out: &mut W,
    card: NewCard<'_>,
    build: Option<&[u8]>,
    progress: &mut dyn FnMut(u64),
) -> Result<Header06, FileError> {
    const SPAN: u64 = 8 << 20;
    let head = src.read_at(0, HEADER_LEN as u64)?;
    let h = Header06::from_bytes(&head)?;
    if h.file_len != src.len() {
        return Err(FileError::Container(
            "0x06 file length does not match its header",
        ));
    }
    // Everything before the data region is small: the header, the root, the
    // card region and the front sections.
    let front = src.read_at(0, h.data_offset)?;
    let root_bytes = front
        .get(HEADER_LEN..HEADER_LEN + h.root_len as usize)
        .ok_or(FileError::Container("0x06 root overruns"))?;
    let r = h.card_range();
    let old = (r.len > 0).then(|| front[r.offset as usize..(r.offset + r.len) as usize].to_vec());
    let plan = plan_card(&h, root_bytes, card, old)?;
    let end = plan.tail.as_ref().map_or(h.file_len, |t| t.0);
    let tail = plan.tail.as_ref().map(|(_, a)| tail_bytes(&h, a));
    let new_data_len = end - h.data_offset + tail.as_ref().map_or(0, |t| t.len() as u64);
    let (nh, new_front) = recard_front(
        &h,
        &front,
        &plan.root_bytes,
        plan.stored.as_deref(),
        build,
        new_data_len,
    )?;
    out.write_all(&new_front)?;
    let mut at = h.data_offset;
    while at < end {
        let n = (end - at).min(SPAN);
        out.write_all(&src.read_at(at, n)?)?;
        at += n;
        progress(at - h.data_offset);
    }
    if let Some(t) = tail {
        out.write_all(&t)?;
    }
    out.flush()?;
    Ok(nh)
}

/// Re-card a `0x06` file **in place**, when the new card and record fit in a
/// free gap of its card region (beside the old pair, which stays readable until
/// the switch): they are written there and the new header is returned, *not*
/// written. The caller syncs the file, then [`commit_header`] — the 128-byte
/// header is the one commit point, so a crash before it leaves the old file
/// whole. A card equal to the current one stays where it is (only the record
/// moves; the content hash does not change). `Ok(None)` when the pair does not
/// fit: the caller copies instead ([`rewrite_card_stream`]).
pub fn recard_in_place<F: std::io::Read + Write + Seek>(
    file: &mut F,
    card: NewCard<'_>,
    build: Option<&[u8]>,
) -> Result<Option<Header06>, FileError> {
    let mut head = vec![0u8; HEADER_LEN];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut head)?;
    let h = Header06::from_bytes(&head)?;
    // A changed annex is the file's last section and its root entry: that is a
    // copy, not an in-place write (the header is the one commit point).
    let mut root_bytes = vec![0u8; h.root_len as usize];
    file.read_exact(&mut root_bytes)?;
    let plan = plan_card(&h, &root_bytes, card, None)?;
    if plan.tail.is_some() {
        return Ok(None);
    }
    let card_stored = match card {
        NewCard::Keep => None,
        NewCard::Set(_) => plan.stored,
    };
    let keep_card = match (card, &card_stored) {
        (NewCard::Keep, _) => true,
        (_, Some(c)) => c.len() as u32 == h.card_len && digest16(c) == h.card_digest,
        (_, None) => h.card_len == 0,
    };
    let card_need = if keep_card {
        0
    } else {
        card_stored.as_ref().map_or(0, |c| c.len()) as u64
    };
    let build_need = build.map_or(0, |b| b.len()) as u64;
    let need = card_need + build_need;
    // The free gaps of the region around the pair in use.
    let mut used: Vec<(u64, u64)> = [
        (h.card_off as u64, h.card_len as u64),
        (h.build_off as u64, h.build_len as u64),
    ]
    .into_iter()
    .filter(|&(_, l)| l > 0)
    .collect();
    used.sort_unstable();
    let region = h.card_region_len as u64;
    let mut gaps = Vec::new();
    let mut at = 0u64;
    for (o, l) in used {
        if o > at {
            gaps.push((at, o - at));
        }
        at = at.max(o + l);
    }
    if region > at {
        gaps.push((at, region - at));
    }
    let start = if need == 0 {
        0
    } else {
        match gaps.iter().find(|&&(_, l)| l >= need) {
            Some(&(o, _)) => o,
            None => return Ok(None),
        }
    };
    file.seek(SeekFrom::Start(h.card_region_offset() + start))?;
    if !keep_card {
        if let Some(c) = &card_stored {
            file.write_all(c)?;
        }
    }
    if let Some(b) = build {
        file.write_all(b)?;
    }
    file.flush()?;
    let mut nh = h.clone();
    if !keep_card {
        nh.card_off = if card_need > 0 { start as u32 } else { 0 };
        nh.card_len = card_need as u32;
        nh.card_digest = card_stored.as_ref().map_or([0; 16], |c| digest16(c));
    }
    nh.build_off = if build_need > 0 {
        (start + card_need) as u32
    } else {
        0
    };
    nh.build_len = build_need as u32;
    nh.content_hash = content_hash(&nh);
    Ok(Some(nh))
}

/// Write `h` over the first 128 bytes (the commit of [`recard_in_place`]).
pub fn commit_header<F: Write + Seek>(file: &mut F, h: &Header06) -> Result<(), FileError> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&h.to_bytes())?;
    file.flush()?;
    Ok(())
}

/// Write the `0x06` file `input` again under the writer's current leaf policy:
/// the payload is copied verbatim and only the directory leaves, the root and
/// the section table change. `opts.quad` keeps the input's own clustering when
/// it is `Auto`.
pub fn rewrite<W: Write + Seek>(
    input: &dyn RangeReader,
    out: &mut W,
    mut opts: WriteOpts,
) -> Result<Written, FileError> {
    let parts = parts::from_v06(input)?;
    if opts.quad == QuadClustering::Auto {
        let head = input.read_at(0, HEADER_LEN as u64)?;
        let h = Header06::from_bytes(&head)?;
        opts.quad = if h.flags & F_QUAD_GRAPH_MAJOR != 0 {
            QuadClustering::GraphMajor
        } else {
            QuadClustering::OrderMajor
        };
    }
    assemble(&parts, input, out, opts)
}

#[cfg(test)]
mod stamp_tests {
    use super::stamp_card_version;

    #[test]
    fn stamps_older_versions_only() {
        let s = |b: &[u8]| stamp_card_version(b).into_owned();
        assert_eq!(
            s(br#"{"a":1,"format_version":5}"#),
            br#"{"a":1,"format_version":7}"#
        );
        assert_eq!(
            s(br#"{"format_version":5,"z":2}"#),
            br#"{"format_version":7,"z":2}"#
        );
        assert_eq!(s(br#"{"format_version":6}"#), br#"{"format_version":7}"#);
        assert_eq!(s(br#"{"format_version":7}"#), br#"{"format_version":7}"#);
        assert_eq!(s(br#"{"format_version":8}"#), br#"{"format_version":8}"#);
        assert_eq!(s(br#"{"title":"x"}"#), br#"{"title":"x"}"#);
    }
}
