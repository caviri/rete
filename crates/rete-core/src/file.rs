//! `.rete` file assembly and reading (SPEC.md §4, §9).
//!
//! v0 layout (single tile, no pyramid yet):
//!
//! ```text
//! [0..128)   header
//! [dict]     dictionary container: 4 front-coded sections
//! [index]    permutation container: 3 triple blocks (SPO, POS, OSP)
//! [footer]   trailing magic
//! ```
//!
//! The header points at the dictionary container (`dictionary_offset/len`) and,
//! for v0, at the permutation container via `root_dir_offset/len` (the pyramid
//! directory replaces this once tiling lands).

use crate::dictionary::Dictionary;
use crate::header::{Header, FLAG_HAS_QUADS, HEADER_LEN, MAGIC};
use crate::index::{GraphIndex, Pattern};
use crate::meta::PyramidMeta;
use crate::pyramid::{build_dendrogram, project_graph};
use crate::reader::RangeReader;
use crate::tiling::{choose_round_for_budget, summarize, SuperEdge};
use crate::varint::{read_uvarint, write_uvarint};

/// Default per-tile byte budget `T` (SPEC.md §7.1).
pub const DEFAULT_TILE_BUDGET: usize = 64 * 1024;

/// Build the encoded pyramid-meta section for a graph: cluster, pick a round
/// sized to `budget`, then emit the **summary** (quotient) graph. Returns
/// `(encoded_meta, pyramid_levels)`.
///
/// Per-community tiles are *not* stored: they would duplicate every triple
/// (the index already answers all queries), and nothing reads them yet. They
/// return when tile-routed range queries are implemented (SPEC §7.2).
pub fn build_pyramid_meta(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    budget: usize,
) -> (Vec<u8>, u16) {
    let g = project_graph(dict, triples);
    let dend = build_dendrogram(&g);
    let round = choose_round_for_budget(dict, triples, &dend, budget);
    let summary = summarize(dict, triples, &dend, round);
    let meta = PyramidMeta::new(round as u32, summary, &[]);
    (meta.encode(), dend.rounds() as u16)
}

/// No compression.
pub const CODEC_NONE: u8 = 0;
/// zstd compression (per section).
pub const CODEC_ZSTD: u8 = 1;
/// zstd compression level used by the writer.
#[cfg(feature = "compression")]
const ZSTD_LEVEL: i32 = 9;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("header: {0}")]
    Header(#[from] crate::header::HeaderError),
    #[error("malformed container: {0}")]
    Container(&'static str),
    #[error("unknown codec: {0}")]
    UnknownCodec(u8),
    #[error("decompression failed: {0}")]
    Decompress(std::io::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// The codec the writer uses: zstd when the `compression` feature is on, else
/// none. Reading honors whatever codec the header records (when supported).
fn writer_codec() -> u8 {
    if cfg!(feature = "compression") {
        CODEC_ZSTD
    } else {
        CODEC_NONE
    }
}

fn compress(codec: u8, bytes: &[u8]) -> Vec<u8> {
    match codec {
        #[cfg(feature = "compression")]
        CODEC_ZSTD => {
            zstd::encode_all(bytes, ZSTD_LEVEL).expect("zstd encode is infallible in-memory")
        }
        _ => bytes.to_vec(),
    }
}

fn decompress(codec: u8, bytes: &[u8]) -> Result<Vec<u8>, FileError> {
    match codec {
        CODEC_NONE => Ok(bytes.to_vec()),
        // Pure-Rust decode so any target (including wasm) can read compressed
        // files, regardless of whether the C encoder was compiled in.
        CODEC_ZSTD => {
            use std::io::Read;
            let mut dec = ruzstd::StreamingDecoder::new(bytes)
                .map_err(|e| FileError::Decompress(std::io::Error::other(e.to_string())))?;
            let mut out = Vec::new();
            dec.read_to_end(&mut out).map_err(FileError::Decompress)?;
            Ok(out)
        }
        other => Err(FileError::UnknownCodec(other)),
    }
}

/// Content hash (first 16 bytes of blake3) over the file payload sections.
/// Identifies the immutable content independent of the header.
fn content_hash(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = blake3::Hasher::new();
    for p in parts {
        h.update(p);
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&h.finalize().as_bytes()[..16]);
    out
}

/// A resolved triple as terms.
pub type TermTriple = (String, String, String);

/// Encode a length-prefixed container of byte sections, each compressed with
/// `codec` independently (so a range-reading client decompresses only the
/// sections it fetches). Stored length is the *compressed* length.
fn encode_container(sections: &[&[u8]], codec: u8) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, sections.len() as u64);
    for s in sections {
        let payload = compress(codec, s);
        write_uvarint(&mut out, payload.len() as u64);
        out.extend_from_slice(&payload);
    }
    out
}

/// Decode a container into owned, decompressed sections.
fn decode_container(bytes: &[u8], codec: u8) -> Result<Vec<Vec<u8>>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated count"))?;
    // `n` is untrusted; each section needs ≥1 byte, so cap the pre-allocation at
    // the buffer length rather than trusting the count (avoids an OOM on a bogus
    // header pointing at a small region).
    let mut out = Vec::with_capacity((n as usize).min(bytes.len()));
    for _ in 0..n {
        let (len, used) =
            read_uvarint(&bytes[pos..]).ok_or(FileError::Container("truncated length"))?;
        pos += used;
        let end = pos + len as usize;
        if end > bytes.len() {
            return Err(FileError::Container("section overruns buffer"));
        }
        out.push(decompress(codec, &bytes[pos..end])?);
        pos = end;
    }
    Ok(out)
}

/// Serialize a complete `.rete` file image from a dictionary, index, and an
/// (optionally empty) encoded pyramid-meta section. `pyramid_levels` records the
/// number of dendrogram rounds the pyramid spans (0 if no pyramid).
pub fn write_file(
    dict: &Dictionary,
    index: &GraphIndex,
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
) -> Vec<u8> {
    write_dataset(dict, index, &[], has_quads, pyramid_meta, pyramid_levels)
}

/// Encode the named-graphs section: each graph as `(iri, permutation container)`.
fn encode_named_graphs(named: &[(String, GraphIndex)], codec: u8) -> Vec<u8> {
    let mut out = Vec::new();
    write_uvarint(&mut out, named.len() as u64);
    for (iri, index) in named {
        write_uvarint(&mut out, iri.len() as u64);
        out.extend_from_slice(iri.as_bytes());
        let container = encode_container(&index.blocks(), codec);
        write_uvarint(&mut out, container.len() as u64);
        out.extend_from_slice(&container);
    }
    out
}

fn decode_named_graphs(bytes: &[u8], codec: u8) -> Result<Vec<(String, GraphIndex)>, FileError> {
    let (n, mut pos) = read_uvarint(bytes).ok_or(FileError::Container("truncated graph count"))?;
    // Bounds-checked slice within this (already bounded) section. Lengths read
    // below are untrusted, so every range is validated before indexing.
    let bound = |start: usize, len: u64| -> Result<usize, FileError> {
        start
            .checked_add(len as usize)
            .filter(|&e| e <= bytes.len())
            .ok_or(FileError::Container("named-graph field overruns buffer"))
    };
    let mut out = Vec::with_capacity((n as usize).min(bytes.len()));
    for _ in 0..n {
        let (ilen, u1) = read_uvarint(bytes.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated iri len"))?;
        pos += u1;
        let iend = bound(pos, ilen)?;
        let iri = String::from_utf8_lossy(&bytes[pos..iend]).into_owned();
        pos = iend;
        let (clen, u2) = read_uvarint(bytes.get(pos..).unwrap_or(&[]))
            .ok_or(FileError::Container("truncated container len"))?;
        pos += u2;
        let cend = bound(pos, clen)?;
        let mut blocks = decode_container(&bytes[pos..cend], codec)?;
        if blocks.len() != 3 {
            return Err(FileError::Container("expected 3 graph permutation blocks"));
        }
        let index = GraphIndex::from_blocks([
            std::mem::take(&mut blocks[0]),
            std::mem::take(&mut blocks[1]),
            std::mem::take(&mut blocks[2]),
        ]);
        out.push((iri, index));
        pos = cend;
    }
    Ok(out)
}

/// Serialize a full RDF *dataset*: the default-graph index plus zero or more
/// named graphs `(iri, index)`, all sharing one dictionary.
pub fn write_dataset(
    dict: &Dictionary,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
) -> Vec<u8> {
    write_dataset_with_metadata(
        dict,
        default_index,
        named,
        has_quads,
        pyramid_meta,
        pyramid_levels,
        &[],
    )
}

/// Serialize a dataset with an opaque **metadata** payload occupying the file's
/// metadata section (the application layer defines its meaning — the CLI stores a
/// JSON Dataset Card there). The section sits immediately after the 128-byte
/// header and before the dictionary, so `metadata_offset` stays at `HEADER_LEN`
/// and every downstream section shifts by `metadata.len()`. The payload is folded
/// into the `content_hash`, so `verify` covers it and it is tamper-evident.
///
/// Passing an empty `metadata` is byte-identical to [`write_dataset`]: the section
/// is omitted (`metadata_len = 0`, `dictionary_offset = HEADER_LEN`) and the hash
/// is computed over exactly the same parts (a zero-length hash update is a no-op).
pub fn write_dataset_with_metadata(
    dict: &Dictionary,
    default_index: &GraphIndex,
    named: &[(String, GraphIndex)],
    has_quads: bool,
    pyramid_meta: &[u8],
    pyramid_levels: u16,
    metadata: &[u8],
) -> Vec<u8> {
    let codec = writer_codec();
    let dict_container = encode_container(&dict.sections(), codec);
    let index_container = encode_container(&default_index.blocks(), codec);
    let named_section = encode_named_graphs(named, codec);

    // The metadata section (if any) sits between the header and the dictionary,
    // so the dictionary — and everything after it — shifts forward by its length.
    let meta_section_len = metadata.len() as u64;
    let dict_offset = HEADER_LEN as u64 + meta_section_len;
    let dict_len = dict_container.len() as u64;
    let index_offset = dict_offset + dict_len;
    let index_len = index_container.len() as u64;
    let pyr_offset = index_offset + index_len;
    let pyr_len = pyramid_meta.len() as u64;
    let named_offset = pyr_offset + pyr_len;
    let named_len = if named.is_empty() {
        0
    } else {
        named_section.len() as u64
    };

    // Hash parts in physical order, with the metadata payload prepended when
    // present. Omitting it entirely (rather than hashing an empty slice) keeps the
    // no-metadata output's hash byte-identical to the pre-metadata writer.
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    if meta_section_len > 0 {
        parts.push(metadata);
    }
    parts.push(&dict_container);
    parts.push(&index_container);
    parts.push(pyramid_meta);
    if named_len > 0 {
        parts.push(&named_section);
    }

    let header = Header {
        flags: if has_quads { FLAG_HAS_QUADS } else { 0 },
        metadata_offset: HEADER_LEN as u64,
        metadata_len: meta_section_len,
        dictionary_offset: dict_offset,
        dictionary_len: dict_len,
        root_dir_offset: index_offset,
        root_dir_len: index_len,
        pyramid_meta_offset: if pyr_len > 0 { pyr_offset } else { 0 },
        pyramid_meta_len: pyr_len,
        dict_codec: codec,
        block_codec: codec,
        pyramid_levels,
        quad_count: default_index.triple_count() as u64,
        term_count: dict.term_count() as u64,
        content_hash: content_hash(&parts),
        named_graphs_offset: if named_len > 0 { named_offset } else { 0 },
        named_graphs_len: named_len,
    };

    let mut out = Vec::with_capacity(
        HEADER_LEN
            + metadata.len()
            + dict_container.len()
            + index_container.len()
            + pyramid_meta.len()
            + named_section.len()
            + MAGIC.len(),
    );
    out.extend_from_slice(&header.to_bytes());
    if meta_section_len > 0 {
        out.extend_from_slice(metadata);
    }
    out.extend_from_slice(&dict_container);
    out.extend_from_slice(&index_container);
    out.extend_from_slice(pyramid_meta);
    if named_len > 0 {
        out.extend_from_slice(&named_section);
    }
    out.extend_from_slice(&MAGIC); // footer marker
    out
}

/// `rdf:type` — the predicate that assigns a class to a resource.
pub const RDF_TYPE: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

/// An **ontology-aware** coarse graph: instead of structural communities, group
/// entities by their `rdf:type` class and aggregate relations between classes.
/// Returns `(subject_class, predicate, object_class, count)` over the default
/// graph. Entities with no type are `(untyped)`; literals are `(literal)`.
/// `rdf:type` triples themselves define the classes and are not counted as
/// relations. This is the dataset's effective schema with instance volumes.
pub fn schema_summary(rete: &Rete) -> Vec<(String, String, String, u32)> {
    use std::collections::{BTreeMap, HashMap};
    let triples = rete.dump(None);

    let mut class_of: HashMap<&str, &str> = HashMap::new();
    for (s, p, o) in &triples {
        if p == RDF_TYPE {
            class_of.insert(s.as_str(), o.as_str());
        }
    }
    let classify = |t: &str| -> String {
        if let Some(c) = class_of.get(t) {
            (*c).to_string()
        } else if t.starts_with('"') {
            "(literal)".to_string()
        } else {
            "(untyped)".to_string()
        }
    };

    let mut counts: BTreeMap<(String, String, String), u32> = BTreeMap::new();
    for (s, p, o) in &triples {
        if p == RDF_TYPE {
            continue; // type assertions define classes, not data relations
        }
        *counts
            .entry((classify(s), p.clone(), classify(o)))
            .or_default() += 1;
    }
    counts
        .into_iter()
        .map(|((a, p, b), c)| (a, p, b, c))
        .collect()
}

/// Class populations: the number of resources of each `rdf:type` class in the
/// default graph, descending by count. The instance-count companion to
/// [`schema_summary`].
pub fn schema_classes(rete: &Rete) -> Vec<(String, u32)> {
    use std::collections::BTreeMap;
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for (_s, p, o) in rete.dump(None) {
        if p == RDF_TYPE {
            *counts.entry(o).or_default() += 1;
        }
    }
    let mut out: Vec<(String, u32)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Recompute the content hash from a file image and check it against the header
/// — detects corruption or truncation of the payload sections.
pub fn verify(bytes: &[u8]) -> Result<bool, FileError> {
    let header = Header::from_bytes(bytes)?;
    let slice = |off: u64, len: u64| -> Result<&[u8], FileError> {
        bytes
            .get(off as usize..(off + len) as usize)
            .ok_or(FileError::Container("section overruns buffer"))
    };
    let d = slice(header.dictionary_offset, header.dictionary_len)?;
    let i = slice(header.root_dir_offset, header.root_dir_len)?;
    let m = if header.pyramid_meta_len > 0 {
        slice(header.pyramid_meta_offset, header.pyramid_meta_len)?
    } else {
        &[]
    };
    // Match the writer's ordering exactly: the metadata payload is prepended when
    // present, then dict, index, pyramid-meta, and (if any) the named graphs.
    let mut parts: Vec<&[u8]> = Vec::with_capacity(5);
    if header.metadata_len > 0 {
        parts.push(slice(header.metadata_offset, header.metadata_len)?);
    }
    parts.push(d);
    parts.push(i);
    parts.push(m);
    if header.named_graphs_len > 0 {
        parts.push(slice(header.named_graphs_offset, header.named_graphs_len)?);
    }
    Ok(content_hash(&parts) == header.content_hash)
}

/// A read-only, in-memory view over a `.rete` file image.
pub struct Rete {
    header: Header,
    dict: Dictionary,
    index: GraphIndex,
    pyramid: Option<PyramidMeta>,
    named_graphs: Vec<(String, GraphIndex)>,
    /// Raw bytes of the metadata section (empty if the file has none). The
    /// application layer decodes this (the CLI stores a JSON Dataset Card here).
    /// Only [`Rete::open`] populates it; [`Rete::open_ranged`] leaves it empty to
    /// preserve its minimal-fetch budget.
    metadata: Vec<u8>,
}

impl Rete {
    /// Parse a full file image (v0 loads everything; a range-reading client
    /// will fetch only the sections it needs — same container format).
    pub fn open(bytes: &[u8]) -> Result<Self, FileError> {
        let header = Header::from_bytes(bytes)?;

        // Header offsets/lengths are untrusted (a `.rete` may be fetched truncated
        // or corrupt from an arbitrary URL). Slice through a checked helper so a
        // bad region yields an error instead of panicking on an OOB index.
        let region = |off: u64, len: u64| -> Result<&[u8], FileError> {
            let start = off as usize;
            let end = start
                .checked_add(len as usize)
                .filter(|&e| e <= bytes.len())
                .ok_or(FileError::Container("section range out of bounds"))?;
            Ok(&bytes[start..end])
        };

        let mut dsecs = decode_container(
            region(header.dictionary_offset, header.dictionary_len)?,
            header.dict_codec,
        )?;
        if dsecs.len() != 4 {
            return Err(FileError::Container("expected 4 dictionary sections"));
        }
        let dict = Dictionary::from_sections([
            std::mem::take(&mut dsecs[0]),
            std::mem::take(&mut dsecs[1]),
            std::mem::take(&mut dsecs[2]),
            std::mem::take(&mut dsecs[3]),
        ]);

        let mut isecs = decode_container(
            region(header.root_dir_offset, header.root_dir_len)?,
            header.block_codec,
        )?;
        if isecs.len() != 3 {
            return Err(FileError::Container("expected 3 permutation blocks"));
        }
        let index = GraphIndex::from_blocks([
            std::mem::take(&mut isecs[0]),
            std::mem::take(&mut isecs[1]),
            std::mem::take(&mut isecs[2]),
        ]);

        let pyramid = if header.pyramid_meta_len > 0 {
            Some(
                PyramidMeta::decode(region(header.pyramid_meta_offset, header.pyramid_meta_len)?)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        };

        let named_graphs = if header.named_graphs_len > 0 {
            decode_named_graphs(
                region(header.named_graphs_offset, header.named_graphs_len)?,
                header.block_codec,
            )?
        } else {
            Vec::new()
        };

        let metadata = if header.metadata_len > 0 {
            region(header.metadata_offset, header.metadata_len)?.to_vec()
        } else {
            Vec::new()
        };

        Ok(Self {
            header,
            dict,
            index,
            pyramid,
            named_graphs,
            metadata,
        })
    }

    pub fn header(&self) -> &Header {
        &self.header
    }

    /// Raw bytes of the file's metadata section, or `None` if it has none. The
    /// CLI stores a JSON Dataset Card here; `rete-core` treats it as opaque.
    /// Populated by [`Rete::open`] only — an [`Rete::open_ranged`] view returns
    /// `None` here (the card is not fetched on the minimal query path).
    pub fn metadata(&self) -> Option<&[u8]> {
        if self.metadata.is_empty() {
            None
        } else {
            Some(&self.metadata)
        }
    }

    pub fn dictionary(&self) -> &Dictionary {
        &self.dict
    }

    /// The pyramid metadata (summary graph + tiles), if the file has a pyramid.
    pub fn pyramid(&self) -> Option<&PyramidMeta> {
        self.pyramid.as_ref()
    }

    /// The default-graph permutation index.
    pub fn default_index(&self) -> &GraphIndex {
        &self.index
    }

    /// Resolve every triple of a graph (`None` = default graph) back to terms.
    pub fn dump(&self, graph: Option<&str>) -> Vec<TermTriple> {
        let index = match graph {
            None => &self.index,
            Some(g) => match self.graph_index(g) {
                Some(i) => i,
                None => return Vec::new(),
            },
        };
        index
            .match_pattern((None, None, None))
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
            .collect()
    }

    /// All named graphs as `(iri, index)`.
    pub fn named_graphs(&self) -> &[(String, GraphIndex)] {
        &self.named_graphs
    }

    /// IRIs of the named graphs in this dataset (the default graph is unnamed).
    pub fn graph_names(&self) -> Vec<&str> {
        self.named_graphs
            .iter()
            .map(|(iri, _)| iri.as_str())
            .collect()
    }

    /// The permutation index of a named graph, or `None` if absent.
    pub fn graph_index(&self, iri: &str) -> Option<&GraphIndex> {
        self.named_graphs
            .iter()
            .find(|(name, _)| name == iri)
            .map(|(_, idx)| idx)
    }

    /// Match a triple pattern in dictionary-ID space (subject/predicate/object
    /// IDs), returning integer triples — the fast path used by the BGP engine.
    pub fn match_ids(
        &self,
        pattern: (Option<u32>, Option<u32>, Option<u32>),
    ) -> Vec<(u32, u32, u32)> {
        self.index.match_pattern(pattern)
    }

    /// All `(subject_node, object_node)` pairs for a predicate, as unified node
    /// IDs — no term resolution. The fast path for graph traversal.
    pub fn predicate_pairs(&self, predicate: &str) -> Vec<(u32, u32)> {
        let pid = match self.dict.predicate_id(predicate) {
            Some(p) => p,
            None => return Vec::new(),
        };
        self.index
            .match_pattern((None, Some(pid), None))
            .into_iter()
            .map(|(s, _p, o)| (self.dict.subject_node(s), self.dict.object_node(o)))
            .collect()
    }

    /// Open via a [`RangeReader`], fetching only the header and the named
    /// section ranges — never a linear scan of the whole resource. A full query
    /// open touches at most 4 ranges (header, dictionary, index, pyramid-meta).
    pub fn open_ranged<R: RangeReader>(reader: &R) -> Result<Self, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;

        let dict_bytes = reader.read_at(header.dictionary_offset, header.dictionary_len)?;
        let mut dsecs = decode_container(&dict_bytes, header.dict_codec)?;
        if dsecs.len() != 4 {
            return Err(FileError::Container("expected 4 dictionary sections"));
        }
        let dict = Dictionary::from_sections([
            std::mem::take(&mut dsecs[0]),
            std::mem::take(&mut dsecs[1]),
            std::mem::take(&mut dsecs[2]),
            std::mem::take(&mut dsecs[3]),
        ]);

        let index_bytes = reader.read_at(header.root_dir_offset, header.root_dir_len)?;
        let mut isecs = decode_container(&index_bytes, header.block_codec)?;
        if isecs.len() != 3 {
            return Err(FileError::Container("expected 3 permutation blocks"));
        }
        let index = GraphIndex::from_blocks([
            std::mem::take(&mut isecs[0]),
            std::mem::take(&mut isecs[1]),
            std::mem::take(&mut isecs[2]),
        ]);

        let pyramid = if header.pyramid_meta_len > 0 {
            let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
            Some(
                PyramidMeta::decode(&mb)
                    .map_err(|_| FileError::Container("malformed pyramid meta"))?,
            )
        } else {
            None
        };

        let named_graphs = if header.named_graphs_len > 0 {
            let nb = reader.read_at(header.named_graphs_offset, header.named_graphs_len)?;
            decode_named_graphs(&nb, header.block_codec)?
        } else {
            Vec::new()
        };

        // The metadata section (Dataset Card) is deliberately NOT fetched here:
        // a ranged query open keeps to its small range budget. Use `Rete::open`
        // (or a dedicated card fetch) when the card is actually needed.
        Ok(Self {
            header,
            dict,
            index,
            pyramid,
            named_graphs,
            metadata: Vec::new(),
        })
    }

    /// Evaluate a triple pattern given as optional term strings, returning
    /// matching triples resolved back to terms. A bound term that is unknown to
    /// the dictionary yields no matches.
    pub fn query(&self, s: Option<&str>, p: Option<&str>, o: Option<&str>) -> Vec<TermTriple> {
        // Resolve bound terms to IDs; a bound-but-unknown term => empty result.
        let sid = match s {
            Some(t) => match self.dict.subject_id(t) {
                Some(id) => Some(id),
                None => return Vec::new(),
            },
            None => None,
        };
        let pid = match p {
            Some(t) => match self.dict.predicate_id(t) {
                Some(id) => Some(id),
                None => return Vec::new(),
            },
            None => None,
        };
        let oid = match o {
            Some(t) => match self.dict.object_id(t) {
                Some(id) => Some(id),
                None => return Vec::new(),
            },
            None => None,
        };

        let pattern: Pattern = (sid, pid, oid);
        self.index
            .match_pattern(pattern)
            .into_iter()
            .filter_map(|(s, p, o)| {
                Some((
                    self.dict.subject_term(s)?,
                    self.dict.predicate_term(p)?,
                    self.dict.object_term(o)?,
                ))
            })
            .collect()
    }
}

/// A lightweight, overview-only view of a file: the pyramid summary graph plus
/// just enough dictionary to label predicates. Fetched via ranges *without*
/// touching the (large) triple index — the "load the coarse graph first" path
/// from SPEC.md §7.2.
pub struct SummaryView {
    pub round: u32,
    pub summary: Vec<SuperEdge>,
    dict: Dictionary,
}

impl SummaryView {
    /// Read header → dictionary → pyramid-meta only (skips the index container).
    pub fn open_ranged<R: RangeReader>(reader: &R) -> Result<Option<Self>, FileError> {
        let head = reader.read_at(0, HEADER_LEN as u64)?;
        let header = Header::from_bytes(&head)?;
        if header.pyramid_meta_len == 0 {
            return Ok(None);
        }

        let dict_bytes = reader.read_at(header.dictionary_offset, header.dictionary_len)?;
        let mut dsecs = decode_container(&dict_bytes, header.dict_codec)?;
        if dsecs.len() != 4 {
            return Err(FileError::Container("expected 4 dictionary sections"));
        }
        let dict = Dictionary::from_sections([
            std::mem::take(&mut dsecs[0]),
            std::mem::take(&mut dsecs[1]),
            std::mem::take(&mut dsecs[2]),
            std::mem::take(&mut dsecs[3]),
        ]);

        let mb = reader.read_at(header.pyramid_meta_offset, header.pyramid_meta_len)?;
        let meta =
            PyramidMeta::decode(&mb).map_err(|_| FileError::Container("malformed pyramid meta"))?;

        Ok(Some(SummaryView {
            round: meta.round,
            summary: meta.summary,
            dict,
        }))
    }

    /// Resolve a predicate ID in the summary to its term.
    pub fn predicate_term(&self, id: u32) -> Option<String> {
        self.dict.predicate_term(id)
    }

    /// Exact number of triples using `predicate`, summed from the summary's
    /// superedge counts — answered without ever reading the triple index.
    pub fn predicate_total(&self, predicate: &str) -> u32 {
        match self.dict.predicate_id(predicate) {
            Some(pid) => self
                .summary
                .iter()
                .filter(|e| e.predicate == pid)
                .map(|e| e.count)
                .sum(),
            None => 0,
        }
    }

    /// All predicates with their exact triple totals, descending by count.
    pub fn predicate_totals(&self) -> Vec<(String, u32)> {
        let mut by_pred: std::collections::BTreeMap<u32, u32> = std::collections::BTreeMap::new();
        for e in &self.summary {
            *by_pred.entry(e.predicate).or_default() += e.count;
        }
        let mut out: Vec<(String, u32)> = by_pred
            .into_iter()
            .filter_map(|(pid, c)| self.dict.predicate_term(pid).map(|t| (t, c)))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    /// Number of communities the summary spans (distinct supernode endpoints).
    pub fn community_count(&self) -> usize {
        let mut comms = std::collections::BTreeSet::new();
        for e in &self.summary {
            comms.insert(e.s_comm);
            comms.insert(e.o_comm);
        }
        comms.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::index::GraphIndexBuilder;

    fn build_image() -> Vec<u8> {
        let triples = [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();

        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let index = ib.build();

        let (meta, levels) = build_pyramid_meta(&dict, &triples_ids(&dict), DEFAULT_TILE_BUDGET);
        write_file(&dict, &index, false, &meta, levels)
    }

    fn triples_ids(dict: &Dictionary) -> Vec<(u32, u32, u32)> {
        [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ]
        .iter()
        .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
        .collect()
    }

    #[test]
    fn file_round_trips_header_and_counts() {
        let bytes = build_image();
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.header().quad_count, 3);
        assert!(rete.header().term_count >= 5);
        let expected_codec = writer_codec();
        assert_eq!(rete.header().dict_codec, expected_codec);
        assert_eq!(rete.header().block_codec, expected_codec);
        assert_eq!(&bytes[bytes.len() - 4..], &MAGIC); // footer marker
    }

    #[test]
    #[cfg(feature = "compression")]
    fn compression_shrinks_repetitive_data() {
        // Many triples sharing IRI prefixes — exactly what front-coding + zstd
        // should crush. The compressed file must be far smaller than the raw
        // term bytes, and still query correctly.
        let mut db = DictionaryBuilder::new();
        let triples: Vec<(String, String, String)> = (0..500)
            .map(|i| {
                (
                    format!("<http://example.org/entity/{i}>"),
                    "<http://example.org/p/relatedTo>".to_string(),
                    format!("<http://example.org/entity/{}>", (i + 1) % 500),
                )
            })
            .collect();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = write_file(&dict, &ib.build(), false, &[], 0);

        let raw: usize = triples
            .iter()
            .map(|(s, p, o)| s.len() + p.len() + o.len())
            .sum();
        assert!(
            bytes.len() < raw / 2,
            "expected strong compression: file {} vs raw terms {raw}",
            bytes.len()
        );

        // Still queryable after compression.
        let rete = Rete::open(&bytes).unwrap();
        let r = rete.query(Some("<http://example.org/entity/0>"), None, None);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].2, "<http://example.org/entity/1>");
    }

    fn big_file_with_pyramid() -> Vec<u8> {
        // A ring of 300 entities -> index dwarfs dict+meta.
        let triples: Vec<(String, String, String)> = (0..300)
            .map(|i| {
                (
                    format!("<http://ex/e{i}>"),
                    "<http://ex/next>".to_string(),
                    format!("<http://ex/e{}>", (i + 1) % 300),
                )
            })
            .collect();
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let (meta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        write_file(&dict, &ib.build(), false, &meta, levels)
    }

    #[test]
    fn ranged_open_is_minimal_and_correct() {
        use crate::reader::{CountingReader, SliceReader};
        let bytes = big_file_with_pyramid();

        let full = CountingReader::new(SliceReader::new(&bytes));
        let rete = Rete::open_ranged(&full).unwrap();
        // Full open touches at most 4 ranges (header, dict, index, meta).
        assert!(full.requests() <= 4, "requests = {}", full.requests());
        assert_eq!(
            rete.query(Some("<http://ex/e0>"), None, None)[0].2,
            "<http://ex/e1>"
        );

        // Summary-only open skips the index → strictly fewer bytes than the file.
        let summ_reader = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&summ_reader).unwrap().unwrap();
        assert!(!view.summary.is_empty());
        assert!(
            summ_reader.bytes_read() < bytes.len() as u64,
            "summary read {} of {} bytes",
            summ_reader.bytes_read(),
            bytes.len()
        );
        // And fewer than a full open, since it never fetched the index.
        assert!(summ_reader.bytes_read() < full.bytes_read());
    }

    #[test]
    fn content_hash_is_set_and_verifies() {
        let bytes = build_image();
        let rete = Rete::open(&bytes).unwrap();
        assert_ne!(
            rete.header().content_hash,
            [0u8; 16],
            "hash must be populated"
        );
        assert!(verify(&bytes).unwrap(), "freshly built file verifies");

        // Same data builds an identical hash (deterministic).
        assert_eq!(
            Rete::open(&build_image()).unwrap().header().content_hash,
            rete.header().content_hash
        );

        // Corrupting a payload byte breaks verification.
        let mut tampered = bytes.clone();
        let last = tampered.len() - 5; // inside payload, before footer magic
        tampered[last] ^= 0xff;
        assert!(!verify(&tampered).unwrap());
    }

    /// Build the standard 3-triple image with an opaque metadata payload.
    fn build_with_metadata(meta: &[u8]) -> Vec<u8> {
        let triples = [
            ("Alice", "knows", "Bob"),
            ("Bob", "knows", "Carol"),
            ("Alice", "age", "30"),
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let mut ib = GraphIndexBuilder::new();
        for &t in &ids {
            ib.push(t);
        }
        let (pmeta, levels) = build_pyramid_meta(&dict, &ids, DEFAULT_TILE_BUDGET);
        write_dataset_with_metadata(&dict, &ib.build(), &[], false, &pmeta, levels, meta)
    }

    #[test]
    fn metadata_round_trips_and_shifts_offsets() {
        let card = br#"{"title":"My Dataset"}"#;
        let bytes = build_with_metadata(card);
        let rete = Rete::open(&bytes).unwrap();

        // The opaque payload reads back verbatim.
        assert_eq!(rete.metadata(), Some(card.as_slice()));
        let h = rete.header();
        assert_eq!(h.metadata_offset, HEADER_LEN as u64);
        assert_eq!(h.metadata_len, card.len() as u64);
        // The dictionary (and everything after it) shifted forward by the card.
        assert_eq!(h.dictionary_offset, HEADER_LEN as u64 + card.len() as u64);

        // The index still decodes correctly at its shifted offset.
        assert_eq!(
            rete.query(Some("Bob"), Some("knows"), Some("Carol")).len(),
            1
        );
        // The card is inside the content hash, so the file still verifies.
        assert!(verify(&bytes).unwrap());
    }

    #[test]
    fn empty_metadata_is_byte_identical_to_plain_writer() {
        // The `&[]` path must produce exactly the bytes of the metadata-free
        // writer for identical inputs — old files and outputs are unchanged.
        assert_eq!(
            build_with_metadata(&[]),
            build_image(),
            "empty-metadata output must equal the plain writer byte-for-byte"
        );
    }

    #[test]
    fn metadata_is_tamper_evident() {
        let card = br#"{"title":"x"}"#;
        let mut bytes = build_with_metadata(card);
        assert!(verify(&bytes).unwrap());
        // The card occupies [HEADER_LEN .. HEADER_LEN+card_len); flip a byte in it.
        bytes[HEADER_LEN + 2] ^= 0xff;
        assert!(
            !verify(&bytes).unwrap(),
            "tampering with the card must break verify()"
        );
    }

    #[test]
    fn ranged_opens_do_not_fetch_metadata() {
        use crate::reader::{CountingReader, SliceReader};
        let card = vec![0xABu8; 512]; // distinctive and sizable
        let bytes = build_with_metadata(&card);
        let total = bytes.len() as u64;

        // A full ranged open never loads the card and never reads its byte range.
        let r = CountingReader::new(SliceReader::new(&bytes));
        let rete = Rete::open_ranged(&r).unwrap();
        assert!(
            rete.metadata().is_none(),
            "open_ranged must not load the card"
        );
        assert!(r.requests() <= 4, "requests = {}", r.requests());
        assert!(
            r.bytes_read() <= total - card.len() as u64,
            "read {} of {} bytes; the {}-byte card must be skipped",
            r.bytes_read(),
            total,
            card.len()
        );

        // Summary-only open likewise ignores the card and still summarizes.
        let rs = CountingReader::new(SliceReader::new(&bytes));
        let view = SummaryView::open_ranged(&rs).unwrap().unwrap();
        assert!(!view.summary.is_empty());
        assert!(rs.bytes_read() <= total - card.len() as u64);
    }

    #[test]
    fn schema_summary_groups_by_type() {
        let rt = RDF_TYPE;
        let bytes = build_from(&[
            ("Alice", rt, "Person"),
            ("Bob", rt, "Person"),
            ("NYC", rt, "City"),
            ("Alice", "knows", "Bob"),
            ("Alice", "livesIn", "NYC"),
            ("Alice", "name", "\"Alice\""),
        ]);
        let rete = Rete::open(&bytes).unwrap();
        let summary = schema_summary(&rete);
        // Expect class-level relations, rdf:type excluded.
        assert!(summary.contains(&("Person".into(), "knows".into(), "Person".into(), 1)));
        assert!(summary.contains(&("Person".into(), "livesIn".into(), "City".into(), 1)));
        assert!(summary.contains(&("Person".into(), "name".into(), "(literal)".into(), 1)));
        // No rdf:type relations in the summary.
        assert!(!summary.iter().any(|(_, p, _, _)| p == RDF_TYPE));

        // Class populations: 2 People, 1 City, sorted by count desc.
        let classes = schema_classes(&rete);
        assert_eq!(
            classes,
            vec![("Person".into(), 2u32), ("City".into(), 1u32)]
        );
    }

    fn build_from(triples: &[(&str, &str, &str)]) -> Vec<u8> {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        write_file(&dict, &ib.build(), false, &[], 0)
    }

    #[test]
    fn named_graphs_round_trip() {
        // One shared dictionary; default graph + a named graph "g1".
        let all = [
            ("Alice", "knows", "Bob"), // default
            ("Bob", "age", "30"),      // named g1
        ];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in all {
            db.observe(s, p, o);
        }
        let dict = db.build();

        let mut def = GraphIndexBuilder::new();
        def.push(dict.encode("Alice", "knows", "Bob").unwrap());
        let mut g1 = GraphIndexBuilder::new();
        g1.push(dict.encode("Bob", "age", "30").unwrap());

        let named = vec![("http://ex/g1".to_string(), g1.build())];
        let bytes = write_dataset(&dict, &def.build(), &named, true, &[], 0);

        assert!(verify(&bytes).unwrap());
        let rete = Rete::open(&bytes).unwrap();
        assert_eq!(rete.graph_names(), vec!["http://ex/g1"]);
        // The named graph contains Bob age 30, not the default-graph triple.
        let gi = rete.graph_index("http://ex/g1").unwrap();
        assert_eq!(gi.triple_count(), 1);
        assert!(rete.graph_index("http://ex/missing").is_none());

        // Default-graph query path is unchanged.
        assert_eq!(rete.query(Some("Alice"), None, None).len(), 1);

        // dump() round-trips each graph back to terms.
        assert_eq!(
            rete.dump(None),
            vec![("Alice".into(), "knows".into(), "Bob".into())]
        );
        assert_eq!(
            rete.dump(Some("http://ex/g1")),
            vec![("Bob".into(), "age".into(), "30".into())]
        );
    }

    #[test]
    fn pyramid_meta_round_trips_in_file() {
        let rete = Rete::open(&build_image()).unwrap();
        let pyr = rete.pyramid().expect("file has a pyramid");
        // Summary covers all 3 triples by count; tiles are not stored in v0.
        let total: u32 = pyr.summary.iter().map(|e| e.count).sum();
        assert_eq!(total, 3);
        assert!(!pyr.summary.is_empty());
        assert!(pyr.tiles.is_empty());
    }

    #[test]
    fn predicate_totals_from_summary_only() {
        use crate::reader::SliceReader;
        // build_image: 2 `knows` triples + 1 `age` triple.
        let bytes = build_image();
        let reader = SliceReader::new(&bytes);
        let view = SummaryView::open_ranged(&reader).unwrap().unwrap();
        assert_eq!(view.predicate_total("knows"), 2);
        assert_eq!(view.predicate_total("age"), 1);
        assert_eq!(view.predicate_total("missing"), 0);
        let totals = view.predicate_totals();
        assert_eq!(totals[0], ("knows".to_string(), 2)); // sorted by count desc
    }

    #[test]
    fn query_patterns_resolve_to_terms() {
        let rete = Rete::open(&build_image()).unwrap();

        // All triples.
        assert_eq!(rete.query(None, None, None).len(), 3);

        // Subject bound.
        let mut alice = rete.query(Some("Alice"), None, None);
        alice.sort();
        assert_eq!(
            alice,
            vec![
                ("Alice".into(), "age".into(), "30".into()),
                ("Alice".into(), "knows".into(), "Bob".into()),
            ]
        );

        // Predicate bound.
        assert_eq!(rete.query(None, Some("knows"), None).len(), 2);

        // Full triple, present and absent.
        assert_eq!(
            rete.query(Some("Bob"), Some("knows"), Some("Carol")),
            vec![("Bob".into(), "knows".into(), "Carol".into())]
        );
        assert!(rete.query(Some("Nobody"), None, None).is_empty());
        assert!(rete.query(None, Some("likes"), None).is_empty());
    }
}
