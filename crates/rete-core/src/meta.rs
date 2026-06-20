//! Pyramid-meta section encoding (SPEC.md §7.3): the summary (quotient) graph,
//! the per-community tile directory, and — as of **pyramid-meta v2** — the
//! **schema pyramid** (the non-exclusive `subClassOf` DAG, per-level type rollups,
//! per-level lateral class relations, and optional per-community descriptors).
//! Stored in the `.rete` file at the header's `pyramid_meta_offset`.
//!
//! Layout:
//! ```text
//! varint round                      # dendrogram round used for this pyramid
//! varint num_superedges
//!   per edge: varint s_comm, predicate, o_comm, count
//! varint num_tiles
//!   per tile: varint community, varint block_len, block_bytes (SPO triple block)
//! --- v2 (optional, appended; absent in v1 files; ignored by v1 readers) ---
//! u8 schema_version (= 2)
//! varint num_strings;        per: len-prefixed UTF-8 IRI/sentinel  # a local table (classes + predicates)
//! varint num_hierarchy;      per: varint class_idx, num_parents, parent_idx…, depth   # non-exclusive DAG
//! varint num_rollups;        per: varint round, depth, num_entries, (class_idx,count)…
//! varint num_level_links;    per: varint round, depth, num_links, (s_idx, pred_idx, o_idx, count)…
//! varint num_descriptors;    per: varint community, dominant_idx+1 (0=none),
//!                                  num_class_counts, (class_idx,count)…,
//!                                  u8 has_bbox [+ 4×f64 le],
//!                                  u8 has_time [+ from(str) + to(str)]
//! ```
//! The v2 block is written **only** when there is schema content, so a typeless
//! graph stays byte-identical to a v1 file. A v1 reader stops after the tiles
//! loop and never sees the appended bytes.

use std::collections::BTreeMap;

use crate::tiling::{SuperEdge, Tile};
use crate::varint::{read_uvarint, write_uvarint};

/// The schema-pyramid section version tag (first byte of the v2 block).
const SCHEMA_V2: u8 = 2;
/// The query-stats block tag. Distinct from [`SCHEMA_V2`]; the block is
/// length-prefixed and written *before* the schema block (which stays trailing,
/// so `schema_meta_len` is unaffected). Index-free per-predicate cardinality the
/// cost-based planner uses; old readers skip it via the length prefix.
const QSTATS_V1: u8 = 3;
/// The characteristic-sets block tag. Length-prefixed, written after the
/// query-stats block and before the (trailing) schema block; old readers skip it.
const CHARSET_V1: u8 = 4;
/// The label-index block tag. Length-prefixed, written after the characteristic-
/// sets block and before the (trailing) schema block; old readers skip it. A
/// bounded, label-sorted `(label, subject)` table for prefix search without a
/// literal scan; see [`LabelEntry`].
const LABELIDX_V1: u8 = 5;

/// A **characteristic set** — one entity "shape": the sorted set of predicates a
/// subject carries, plus how many subjects share exactly that shape. Computed at
/// build (top-N shapes by subject count), range-fetched with the pyramid. Useful
/// for data profiling (the distinct entity shapes) and for estimating the
/// selectivity of a subject star (subjects matching ALL of several predicates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CharSet {
    /// Predicate ids, ascending.
    pub predicates: Vec<u32>,
    /// Subjects whose predicate set is exactly this.
    pub subjects: u64,
}

/// One entry of the **label index**: the display label of a subject (its
/// `rdfs:label` / `skos:prefLabel` / … literal) paired with that subject's id.
/// Computed at build (the most-connected labeled subjects, bounded), the table
/// is sorted by the label's lowercased form so a prefix query is a binary search
/// — autocomplete and CONTAINS/prefix lookup **without scanning the literals**.
/// Range-fetched with the pyramid; the label string is stored inline so the
/// search is self-contained on the remote-lazy path (no dictionary fault).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelEntry {
    /// The label literal's lexical value, original case (sorted case-insensitively).
    pub label: String,
    /// The subject id (subject id space) this label belongs to.
    pub subject: u32,
}

/// Per-predicate cardinality statistics for the cost-based query planner —
/// computed at build, range-fetched with the pyramid (index-free). `count` is the
/// triple total; the distinct-subject/object counts and max multiplicities let
/// the planner estimate a bound subject's/object's selectivity (`functional`
/// iff `max_objects_per_subject == 1`, inverse-functional iff
/// `max_subjects_per_object == 1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredStat {
    pub predicate: u32,
    pub count: u64,
    pub distinct_subjects: u64,
    pub distinct_objects: u64,
    pub max_objects_per_subject: u32,
    pub max_subjects_per_object: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum MetaError {
    #[error("malformed pyramid meta: {0}")]
    Malformed(&'static str),
}

/// One node of the shipped `subClassOf` DAG: a class, **all** its direct parents,
/// and its `depth` (0 = root). The hierarchy is **non-exclusive** — a class may
/// have several parents (multiple inheritance), so this is a directed acyclic
/// *graph*, not a tree. The first parent (parents are sorted) is the *canonical*
/// one used for the depth/rollup spanning tree; the rest preserve the cross-links.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassNode {
    pub class: String,
    pub parents: Vec<String>,
    pub depth: u16,
}

impl ClassNode {
    /// The canonical parent (smallest) used for the rollup spanning tree, if any.
    pub fn canonical_parent(&self) -> Option<&String> {
        self.parents.first()
    }
}

/// A type histogram **rolled up to `depth`** — the global class distribution at
/// one semantic-zoom level. Coarse levels (small depth) hold abstract ancestor
/// classes; fine levels (large depth) resolve to leaves. `round` is the
/// dendrogram round this level is aligned with (informational).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelRollup {
    pub round: u32,
    pub depth: u16,
    pub classes: Vec<(String, u64)>,
}

/// A class-to-class relation (the **lateral**, non-`is-a` connection): subjects of
/// `s_class` related by `predicate` to objects of `o_class`, with the instance
/// `count`. `(literal)` / `(untyped)` are the object-class sentinels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassRelation {
    pub s_class: String,
    pub predicate: String,
    pub o_class: String,
    pub count: u64,
}

/// The class-relation graph **rolled up to `depth`** — the lateral connections at
/// one semantic-zoom level. Coarse levels show relations between abstract classes
/// (`Agent → Agent`); finer levels resolve them (`Person → Organisation`). This is
/// what makes the pyramid a leveled *graph*, not just a leveled histogram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelLinks {
    pub round: u32,
    pub depth: u16,
    pub links: Vec<ClassRelation>,
}

/// A per-community refinement descriptor (Phase 4): what a client sees when it
/// zooms into one community, without fetching that community's triples. Carries
/// the dominant class, the local type histogram, and optional spatial/temporal
/// extents. (The physical per-community triple tiles are still future work; this
/// descriptor index ships in the index-free pyramid-meta and is ready to attach
/// to those tiles when they exist.)
#[derive(Debug, Clone, PartialEq)]
pub struct CommunityDescriptor {
    pub community: u32,
    pub dominant_class: Option<String>,
    pub class_counts: Vec<(String, u64)>,
    /// `[minLon, minLat, maxLon, maxLat]` over wgs84 lat/long (CRS84).
    pub bbox: Option<[f64; 4]>,
    /// `(min, max)` lexical extent over a date/year-typed predicate.
    pub time_range: Option<(String, String)>,
}

/// Decoded pyramid metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PyramidMeta {
    pub round: u32,
    pub summary: Vec<SuperEdge>,
    /// `(community, encoded SPO triple block)` per tile.
    pub tiles: Vec<(u32, Vec<u8>)>,
    // --- v2 schema pyramid (empty on v1 files) ---
    pub class_hierarchy: Vec<ClassNode>,
    pub level_rollups: Vec<LevelRollup>,
    /// Per-level lateral class-relation graph (the non-`is-a` connections).
    pub level_links: Vec<LevelLinks>,
    pub descriptors: Vec<CommunityDescriptor>,
    // --- v2.1 coherence axioms (empty on v1 and on v2.0 files) ---
    /// `subClassOf` cycles — each a sorted class set (SCC > 1, or a self-loop).
    pub subclass_cycles: Vec<Vec<String>>,
    /// `owl:disjointWith` class pairs (canonical `(min, max)`).
    pub disjoint_pairs: Vec<(String, String)>,
    /// `owl:equivalentClass` class pairs (canonical `(min, max)`).
    pub equivalent_pairs: Vec<(String, String)>,
    // --- query-stats block (empty when not computed; independent of the schema) ---
    /// Per-predicate cardinality for the planner; see [`PredStat`].
    pub predicate_stats: Vec<PredStat>,
    // --- characteristic-sets block (entity shapes; empty when not computed) ---
    /// The distinct entity shapes (top-N by subject count); see [`CharSet`].
    pub char_sets: Vec<CharSet>,
    // --- label-index block (prefix search; empty when not computed) ---
    /// Label-sorted `(label, subject)` table for prefix search; see [`LabelEntry`].
    pub label_index: Vec<LabelEntry>,
}

impl PyramidMeta {
    /// Build from a chosen round, the summary superedges, and the tiles — with an
    /// empty schema pyramid (v1 shape). Use [`with_schema`](Self::with_schema) to
    /// attach the v2 schema pyramid.
    pub fn new(round: u32, summary: Vec<SuperEdge>, tiles: &[Tile]) -> Self {
        let tiles = tiles
            .iter()
            .map(|t| (t.community as u32, t.encoded.clone()))
            .collect();
        PyramidMeta {
            round,
            summary,
            tiles,
            class_hierarchy: Vec::new(),
            level_rollups: Vec::new(),
            level_links: Vec::new(),
            descriptors: Vec::new(),
            subclass_cycles: Vec::new(),
            disjoint_pairs: Vec::new(),
            equivalent_pairs: Vec::new(),
            predicate_stats: Vec::new(),
            char_sets: Vec::new(),
            label_index: Vec::new(),
        }
    }

    /// Attach the v2 schema pyramid (consuming builder).
    #[allow(clippy::too_many_arguments)]
    pub fn with_schema(
        mut self,
        class_hierarchy: Vec<ClassNode>,
        level_rollups: Vec<LevelRollup>,
        level_links: Vec<LevelLinks>,
        descriptors: Vec<CommunityDescriptor>,
        subclass_cycles: Vec<Vec<String>>,
        disjoint_pairs: Vec<(String, String)>,
        equivalent_pairs: Vec<(String, String)>,
    ) -> Self {
        self.class_hierarchy = class_hierarchy;
        self.level_rollups = level_rollups;
        self.level_links = level_links;
        self.descriptors = descriptors;
        self.subclass_cycles = subclass_cycles;
        self.disjoint_pairs = disjoint_pairs;
        self.equivalent_pairs = equivalent_pairs;
        self
    }

    /// Attach per-predicate query stats (consuming builder). Independent of the
    /// schema pyramid — a typeless graph can still carry stats.
    pub fn with_predicate_stats(mut self, predicate_stats: Vec<PredStat>) -> Self {
        self.predicate_stats = predicate_stats;
        self
    }

    /// Attach the characteristic sets / entity shapes (consuming builder).
    pub fn with_char_sets(mut self, char_sets: Vec<CharSet>) -> Self {
        self.char_sets = char_sets;
        self
    }

    /// Attach the label index (consuming builder). Independent of the schema —
    /// any graph with label literals can carry it. The entries should already be
    /// sorted by the label's lowercased form (see [`Self::prefix_search`]).
    pub fn with_label_index(mut self, label_index: Vec<LabelEntry>) -> Self {
        self.label_index = label_index;
        self
    }

    /// All entries whose label, compared case-insensitively, starts with
    /// `prefix`, capped at `limit`. The index is stored sorted by the lowercased
    /// label, so this is a binary search for the lower bound followed by a linear
    /// walk while the prefix still matches — no literal scan. An empty `prefix`
    /// returns the first `limit` entries.
    pub fn prefix_search(&self, prefix: &str, limit: usize) -> Vec<&LabelEntry> {
        let needle = prefix.to_lowercase();
        // Lower bound: first entry whose lowercased label is >= the needle.
        let start = self
            .label_index
            .partition_point(|e| e.label.to_lowercase().as_str() < needle.as_str());
        let mut out = Vec::new();
        for e in &self.label_index[start..] {
            if out.len() >= limit {
                break;
            }
            let lower = e.label.to_lowercase();
            if needle.is_empty() || lower.starts_with(&needle) {
                out.push(e);
            } else if lower.as_str() > needle.as_str() {
                // Past the prefix run (entries are sorted) — nothing more matches.
                break;
            }
        }
        out
    }

    /// True when no schema pyramid is present (v1 shape).
    fn schema_is_empty(&self) -> bool {
        self.class_hierarchy.is_empty()
            && self.level_rollups.is_empty()
            && self.level_links.is_empty()
            && self.descriptors.is_empty()
            && self.subclass_cycles.is_empty()
            && self.disjoint_pairs.is_empty()
            && self.equivalent_pairs.is_empty()
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        write_uvarint(&mut out, self.round as u64);
        write_uvarint(&mut out, self.summary.len() as u64);
        for e in &self.summary {
            write_uvarint(&mut out, e.s_comm as u64);
            write_uvarint(&mut out, e.predicate as u64);
            write_uvarint(&mut out, e.o_comm as u64);
            write_uvarint(&mut out, e.count as u64);
        }
        write_uvarint(&mut out, self.tiles.len() as u64);
        for (community, block) in &self.tiles {
            write_uvarint(&mut out, *community as u64);
            write_uvarint(&mut out, block.len() as u64);
            out.extend_from_slice(block);
        }
        // query-stats block (length-prefixed, tag QSTATS_V1) — written before the
        // schema block so the schema stays the trailing section (schema_meta_len).
        // Present only when computed; a v1/v2 reader sees the tag, reads the length,
        // and skips it.
        if !self.predicate_stats.is_empty() {
            let mut payload = Vec::new();
            write_uvarint(&mut payload, self.predicate_stats.len() as u64);
            for p in &self.predicate_stats {
                write_uvarint(&mut payload, p.predicate as u64);
                write_uvarint(&mut payload, p.count);
                write_uvarint(&mut payload, p.distinct_subjects);
                write_uvarint(&mut payload, p.distinct_objects);
                write_uvarint(&mut payload, p.max_objects_per_subject as u64);
                write_uvarint(&mut payload, p.max_subjects_per_object as u64);
            }
            out.push(QSTATS_V1);
            write_uvarint(&mut out, payload.len() as u64);
            out.extend_from_slice(&payload);
        }
        // characteristic-sets block (length-prefixed, tag CHARSET_V1) — after the
        // query-stats block, still before the trailing schema block.
        if !self.char_sets.is_empty() {
            let mut payload = Vec::new();
            write_uvarint(&mut payload, self.char_sets.len() as u64);
            for c in &self.char_sets {
                write_uvarint(&mut payload, c.predicates.len() as u64);
                for &p in &c.predicates {
                    write_uvarint(&mut payload, p as u64);
                }
                write_uvarint(&mut payload, c.subjects);
            }
            out.push(CHARSET_V1);
            write_uvarint(&mut out, payload.len() as u64);
            out.extend_from_slice(&payload);
        }
        // label-index block (length-prefixed, tag LABELIDX_V1) — after the
        // characteristic-sets block, still before the trailing schema block.
        if !self.label_index.is_empty() {
            let mut payload = Vec::new();
            write_uvarint(&mut payload, self.label_index.len() as u64);
            for e in &self.label_index {
                write_str(&mut payload, &e.label);
                write_uvarint(&mut payload, e.subject as u64);
            }
            out.push(LABELIDX_V1);
            write_uvarint(&mut out, payload.len() as u64);
            out.extend_from_slice(&payload);
        }
        // v2 schema pyramid — appended only when present (a typeless graph stays
        // byte-identical to v1, and v1 readers stop after the tiles above).
        if !self.schema_is_empty() {
            self.encode_schema(&mut out);
        }
        out
    }

    /// Encode the v2 block: a deduped class-string table, then the hierarchy,
    /// rollups, and descriptors as indices into it.
    fn encode_schema(&self, out: &mut Vec<u8>) {
        // Collect every class string into a deterministic, deduped table.
        // Interning order (first appearance) fixes the table order → reproducible.
        let mut table: Vec<String> = Vec::new();
        let mut index: BTreeMap<String, u32> = BTreeMap::new();
        fn intern(table: &mut Vec<String>, index: &mut BTreeMap<String, u32>, s: &str) {
            if !index.contains_key(s) {
                index.insert(s.to_string(), table.len() as u32);
                table.push(s.to_string());
            }
        }
        for n in &self.class_hierarchy {
            intern(&mut table, &mut index, &n.class);
            for p in &n.parents {
                intern(&mut table, &mut index, p);
            }
        }
        for r in &self.level_rollups {
            for (c, _) in &r.classes {
                intern(&mut table, &mut index, c);
            }
        }
        for l in &self.level_links {
            for r in &l.links {
                intern(&mut table, &mut index, &r.s_class);
                intern(&mut table, &mut index, &r.predicate);
                intern(&mut table, &mut index, &r.o_class);
            }
        }
        for d in &self.descriptors {
            if let Some(c) = &d.dominant_class {
                intern(&mut table, &mut index, c);
            }
            for (c, _) in &d.class_counts {
                intern(&mut table, &mut index, c);
            }
        }
        // v2.1 coherence axioms must join the SAME first interning pass, or `idx`
        // below panics on a class string that was never interned.
        for cyc in &self.subclass_cycles {
            for c in cyc {
                intern(&mut table, &mut index, c);
            }
        }
        for (a, b) in self.disjoint_pairs.iter().chain(&self.equivalent_pairs) {
            intern(&mut table, &mut index, a);
            intern(&mut table, &mut index, b);
        }
        let idx = |s: &str| *index.get(s).expect("interned");

        out.push(SCHEMA_V2);
        write_uvarint(out, table.len() as u64);
        for s in &table {
            write_str(out, s);
        }
        write_uvarint(out, self.class_hierarchy.len() as u64);
        for n in &self.class_hierarchy {
            write_uvarint(out, idx(&n.class) as u64);
            write_uvarint(out, n.parents.len() as u64);
            for p in &n.parents {
                write_uvarint(out, idx(p) as u64);
            }
            write_uvarint(out, n.depth as u64);
        }
        write_uvarint(out, self.level_rollups.len() as u64);
        for r in &self.level_rollups {
            write_uvarint(out, r.round as u64);
            write_uvarint(out, r.depth as u64);
            write_uvarint(out, r.classes.len() as u64);
            for (c, count) in &r.classes {
                write_uvarint(out, idx(c) as u64);
                write_uvarint(out, *count);
            }
        }
        write_uvarint(out, self.level_links.len() as u64);
        for l in &self.level_links {
            write_uvarint(out, l.round as u64);
            write_uvarint(out, l.depth as u64);
            write_uvarint(out, l.links.len() as u64);
            for r in &l.links {
                write_uvarint(out, idx(&r.s_class) as u64);
                write_uvarint(out, idx(&r.predicate) as u64);
                write_uvarint(out, idx(&r.o_class) as u64);
                write_uvarint(out, r.count);
            }
        }
        write_uvarint(out, self.descriptors.len() as u64);
        for d in &self.descriptors {
            write_uvarint(out, d.community as u64);
            match &d.dominant_class {
                Some(c) => write_uvarint(out, idx(c) as u64 + 1),
                None => write_uvarint(out, 0),
            }
            write_uvarint(out, d.class_counts.len() as u64);
            for (c, count) in &d.class_counts {
                write_uvarint(out, idx(c) as u64);
                write_uvarint(out, *count);
            }
            match d.bbox {
                Some(b) => {
                    out.push(1);
                    for v in b {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
                None => out.push(0),
            }
            match &d.time_range {
                Some((from, to)) => {
                    out.push(1);
                    write_str(out, from);
                    write_str(out, to);
                }
                None => out.push(0),
            }
        }
        // --- v2.1 additive extension: subClassOf cycles + disjoint/equivalent ---
        // Written as ONE block, only when any of the three is present, so existing
        // v2.0 files (schema but no coherence axioms) stay byte-identical and a
        // v2.0 reader stops cleanly after the descriptors above.
        if !self.subclass_cycles.is_empty()
            || !self.disjoint_pairs.is_empty()
            || !self.equivalent_pairs.is_empty()
        {
            write_uvarint(out, self.subclass_cycles.len() as u64);
            for cyc in &self.subclass_cycles {
                write_uvarint(out, cyc.len() as u64);
                for c in cyc {
                    write_uvarint(out, idx(c) as u64);
                }
            }
            write_uvarint(out, self.disjoint_pairs.len() as u64);
            for (a, b) in &self.disjoint_pairs {
                write_uvarint(out, idx(a) as u64);
                write_uvarint(out, idx(b) as u64);
            }
            write_uvarint(out, self.equivalent_pairs.len() as u64);
            for (a, b) in &self.equivalent_pairs {
                write_uvarint(out, idx(a) as u64);
                write_uvarint(out, idx(b) as u64);
            }
        }
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MetaError> {
        let mut pos = 0;
        let g = |pos: &mut usize| -> Result<u64, MetaError> {
            let (v, n) = read_uvarint(&bytes[*pos..]).ok_or(MetaError::Malformed("truncated"))?;
            *pos += n;
            Ok(v)
        };
        let round = g(&mut pos)? as u32;
        // Counts are untrusted; each entry consumes several bytes, so cap the
        // pre-allocation at the buffer length to avoid an OOM on a bogus count.
        let n_edges = g(&mut pos)? as usize;
        let mut summary = Vec::with_capacity(n_edges.min(bytes.len()));
        for _ in 0..n_edges {
            summary.push(SuperEdge {
                s_comm: g(&mut pos)? as usize,
                predicate: g(&mut pos)? as u32,
                o_comm: g(&mut pos)? as usize,
                count: g(&mut pos)? as u32,
            });
        }
        let n_tiles = g(&mut pos)? as usize;
        let mut tiles = Vec::with_capacity(n_tiles.min(bytes.len()));
        for _ in 0..n_tiles {
            let community = g(&mut pos)? as u32;
            let len = g(&mut pos)? as usize;
            // checked_add: a corrupt `len` must not overflow-panic before the bound check.
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or(MetaError::Malformed("tile block overruns buffer"))?;
            tiles.push((community, bytes[pos..end].to_vec()));
            pos = end;
        }

        let mut meta = PyramidMeta {
            round,
            summary,
            tiles,
            class_hierarchy: Vec::new(),
            level_rollups: Vec::new(),
            level_links: Vec::new(),
            descriptors: Vec::new(),
            subclass_cycles: Vec::new(),
            disjoint_pairs: Vec::new(),
            equivalent_pairs: Vec::new(),
            predicate_stats: Vec::new(),
            char_sets: Vec::new(),
            label_index: Vec::new(),
        };
        // query-stats block (before the schema block) — tag, uvarint length, then
        // the payload. The length lets a reader that doesn't know it skip past;
        // we read it and jump to the block end regardless.
        if pos < bytes.len() && bytes[pos] == QSTATS_V1 {
            pos += 1;
            let len = g(&mut pos)? as usize;
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or(MetaError::Malformed("query-stats block overruns buffer"))?;
            let mut p = pos;
            let np = g(&mut p)? as usize;
            let mut stats = Vec::with_capacity(np.min(bytes.len()));
            for _ in 0..np {
                stats.push(PredStat {
                    predicate: g(&mut p)? as u32,
                    count: g(&mut p)?,
                    distinct_subjects: g(&mut p)?,
                    distinct_objects: g(&mut p)?,
                    max_objects_per_subject: g(&mut p)? as u32,
                    max_subjects_per_object: g(&mut p)? as u32,
                });
            }
            meta.predicate_stats = stats;
            pos = end;
        }
        // characteristic-sets block (after query-stats, before schema).
        if pos < bytes.len() && bytes[pos] == CHARSET_V1 {
            pos += 1;
            let len = g(&mut pos)? as usize;
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or(MetaError::Malformed("char-sets block overruns buffer"))?;
            let mut p = pos;
            let nc = g(&mut p)? as usize;
            let mut sets = Vec::with_capacity(nc.min(bytes.len()));
            for _ in 0..nc {
                let np = g(&mut p)? as usize;
                let mut predicates = Vec::with_capacity(np.min(bytes.len()));
                for _ in 0..np {
                    predicates.push(g(&mut p)? as u32);
                }
                let subjects = g(&mut p)?;
                sets.push(CharSet {
                    predicates,
                    subjects,
                });
            }
            meta.char_sets = sets;
            pos = end;
        }
        // label-index block (after characteristic-sets, before schema).
        if pos < bytes.len() && bytes[pos] == LABELIDX_V1 {
            pos += 1;
            let len = g(&mut pos)? as usize;
            let end = pos
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or(MetaError::Malformed("label-index block overruns buffer"))?;
            let mut p = pos;
            let nl = g(&mut p)? as usize;
            let mut entries = Vec::with_capacity(nl.min(bytes.len()));
            for _ in 0..nl {
                let label = read_str(bytes, &mut p)?;
                let subject = g(&mut p)? as u32;
                entries.push(LabelEntry { label, subject });
            }
            meta.label_index = entries;
            pos = end;
        }
        // v2 schema pyramid — present iff there are trailing bytes tagged
        // SCHEMA_V2. Best-effort: the v1 fields (round/summary/tiles) are already
        // populated, and `decode_schema` only assigns the schema fields on full
        // success, so a malformed/ambiguous trailing region leaves the schema
        // empty rather than failing the whole decode. (A genuine v1 file has no
        // trailing bytes at all, so this branch never fires for it.)
        if pos < bytes.len() && bytes[pos] == SCHEMA_V2 {
            let mut p = pos + 1;
            let _ = decode_schema(bytes, &mut p, &mut meta);
        }
        Ok(meta)
    }
}

/// Decode the v2 block into `meta` (called only when the tag byte matched).
fn decode_schema(bytes: &[u8], pos: &mut usize, meta: &mut PyramidMeta) -> Result<(), MetaError> {
    let g = |pos: &mut usize| -> Result<u64, MetaError> {
        let (v, n) = read_uvarint(&bytes[*pos..]).ok_or(MetaError::Malformed("truncated v2"))?;
        *pos += n;
        Ok(v)
    };
    let n_table = g(pos)? as usize;
    let mut table = Vec::with_capacity(n_table.min(bytes.len()));
    for _ in 0..n_table {
        table.push(read_str(bytes, pos)?);
    }
    let lookup = |idx: u64| -> Result<String, MetaError> {
        table
            .get(idx as usize)
            .cloned()
            .ok_or(MetaError::Malformed("class index out of range"))
    };

    let n_hier = g(pos)? as usize;
    let mut class_hierarchy = Vec::with_capacity(n_hier.min(bytes.len()));
    for _ in 0..n_hier {
        let class = lookup(g(pos)?)?;
        let n_parents = g(pos)? as usize;
        let mut parents = Vec::with_capacity(n_parents.min(bytes.len()));
        for _ in 0..n_parents {
            parents.push(lookup(g(pos)?)?);
        }
        let depth = g(pos)? as u16;
        class_hierarchy.push(ClassNode {
            class,
            parents,
            depth,
        });
    }

    let n_roll = g(pos)? as usize;
    let mut level_rollups = Vec::with_capacity(n_roll.min(bytes.len()));
    for _ in 0..n_roll {
        let round = g(pos)? as u32;
        let depth = g(pos)? as u16;
        let n = g(pos)? as usize;
        let mut classes = Vec::with_capacity(n.min(bytes.len()));
        for _ in 0..n {
            let c = lookup(g(pos)?)?;
            let count = g(pos)?;
            classes.push((c, count));
        }
        level_rollups.push(LevelRollup {
            round,
            depth,
            classes,
        });
    }

    let n_links = g(pos)? as usize;
    let mut level_links = Vec::with_capacity(n_links.min(bytes.len()));
    for _ in 0..n_links {
        let round = g(pos)? as u32;
        let depth = g(pos)? as u16;
        let n = g(pos)? as usize;
        let mut links = Vec::with_capacity(n.min(bytes.len()));
        for _ in 0..n {
            let s_class = lookup(g(pos)?)?;
            let predicate = lookup(g(pos)?)?;
            let o_class = lookup(g(pos)?)?;
            let count = g(pos)?;
            links.push(ClassRelation {
                s_class,
                predicate,
                o_class,
                count,
            });
        }
        level_links.push(LevelLinks {
            round,
            depth,
            links,
        });
    }

    let n_desc = g(pos)? as usize;
    let mut descriptors = Vec::with_capacity(n_desc.min(bytes.len()));
    for _ in 0..n_desc {
        let community = g(pos)? as u32;
        let dom_plus1 = g(pos)?;
        let dominant_class = if dom_plus1 == 0 {
            None
        } else {
            Some(lookup(dom_plus1 - 1)?)
        };
        let n = g(pos)? as usize;
        let mut class_counts = Vec::with_capacity(n.min(bytes.len()));
        for _ in 0..n {
            let c = lookup(g(pos)?)?;
            let count = g(pos)?;
            class_counts.push((c, count));
        }
        let bbox = if read_u8(bytes, pos)? == 1 {
            let mut b = [0f64; 4];
            for v in b.iter_mut() {
                *v = read_f64(bytes, pos)?;
            }
            Some(b)
        } else {
            None
        };
        let time_range = if read_u8(bytes, pos)? == 1 {
            let from = read_str(bytes, pos)?;
            let to = read_str(bytes, pos)?;
            Some((from, to))
        } else {
            None
        };
        descriptors.push(CommunityDescriptor {
            community,
            dominant_class,
            class_counts,
            bbox,
            time_range,
        });
    }

    // Assign the v2.0 fields BEFORE the best-effort v2.1 extension, so a truncated
    // or v2.0-only file keeps its fully-decoded hierarchy/rollups/links/descriptors
    // (the extension reads below must never discard what already decoded cleanly).
    meta.class_hierarchy = class_hierarchy;
    meta.level_rollups = level_rollups;
    meta.level_links = level_links;
    meta.descriptors = descriptors;

    // --- v2.1 additive extension: cycles + disjoint/equivalent pairs ---
    // A v2.0 file ends exactly here, so stop cleanly at EOF before any new read.
    if *pos >= bytes.len() {
        return Ok(());
    }
    let n_cycles = g(pos)? as usize;
    let mut subclass_cycles = Vec::with_capacity(n_cycles.min(bytes.len()));
    for _ in 0..n_cycles {
        let k = g(pos)? as usize;
        let mut members = Vec::with_capacity(k.min(bytes.len()));
        for _ in 0..k {
            members.push(lookup(g(pos)?)?);
        }
        subclass_cycles.push(members);
    }
    let n_disjoint = g(pos)? as usize;
    let mut disjoint_pairs = Vec::with_capacity(n_disjoint.min(bytes.len()));
    for _ in 0..n_disjoint {
        let a = lookup(g(pos)?)?;
        let b = lookup(g(pos)?)?;
        disjoint_pairs.push((a, b));
    }
    let n_equiv = g(pos)? as usize;
    let mut equivalent_pairs = Vec::with_capacity(n_equiv.min(bytes.len()));
    for _ in 0..n_equiv {
        let a = lookup(g(pos)?)?;
        let b = lookup(g(pos)?)?;
        equivalent_pairs.push((a, b));
    }
    meta.subclass_cycles = subclass_cycles;
    meta.disjoint_pairs = disjoint_pairs;
    meta.equivalent_pairs = equivalent_pairs;
    Ok(())
}

/// Byte length of the trailing schema-pyramid block within an encoded pyramid-meta
/// (0 when there is none). The writer records this in the header so a reader can
/// fetch *only* the schema block (`schema_meta_len` bytes at the end of pyramid-meta)
/// for a summary-free, dictionary-free Tier-0 read. Re-walks round + summary + tiles
/// to find where the schema block begins; any malformed prefix yields 0 (the reader
/// then safely falls back to decoding the whole section).
pub fn schema_block_len(bytes: &[u8]) -> u32 {
    fn walk(bytes: &[u8]) -> Option<usize> {
        let mut pos = 0usize;
        macro_rules! uv {
            () => {{
                let (v, n) = read_uvarint(bytes.get(pos..)?)?;
                pos += n;
                v
            }};
        }
        let _round = uv!();
        let n_edges = uv!() as usize;
        for _ in 0..n_edges {
            uv!();
            uv!();
            uv!();
            uv!();
        }
        let n_tiles = uv!() as usize;
        for _ in 0..n_tiles {
            let _comm = uv!();
            let len = uv!() as usize;
            pos = pos.checked_add(len)?;
            if pos > bytes.len() {
                return None;
            }
        }
        // Skip the optional query-stats, characteristic-sets and label-index
        // blocks (each tag + uvarint length + payload) so the schema stays trailing.
        for tag in [QSTATS_V1, CHARSET_V1, LABELIDX_V1] {
            if pos < bytes.len() && bytes[pos] == tag {
                pos += 1;
                let len = uv!() as usize;
                pos = pos.checked_add(len)?;
                if pos > bytes.len() {
                    return None;
                }
            }
        }
        Some(pos)
    }
    match walk(bytes) {
        Some(start) if start < bytes.len() && bytes[start] == SCHEMA_V2 => {
            (bytes.len() - start) as u32
        }
        _ => 0,
    }
}

/// Decode a STANDALONE schema-pyramid block — the `schema_meta_len` bytes the header
/// points at, beginning with the `SCHEMA_V2` tag — into just the schema fields,
/// without the surrounding round/summary/tiles. Powers the index/dictionary/summary
/// -free Tier-0 coherence read.
#[allow(clippy::type_complexity)]
pub fn decode_schema_block(
    block: &[u8],
) -> Result<
    (
        Vec<ClassNode>,
        Vec<Vec<String>>,
        Vec<(String, String)>,
        Vec<(String, String)>,
    ),
    MetaError,
> {
    if block.first() != Some(&SCHEMA_V2) {
        return Err(MetaError::Malformed("not a schema block"));
    }
    let mut meta = PyramidMeta {
        round: 0,
        summary: Vec::new(),
        tiles: Vec::new(),
        class_hierarchy: Vec::new(),
        level_rollups: Vec::new(),
        level_links: Vec::new(),
        descriptors: Vec::new(),
        subclass_cycles: Vec::new(),
        disjoint_pairs: Vec::new(),
        equivalent_pairs: Vec::new(),
        predicate_stats: Vec::new(),
        char_sets: Vec::new(),
        label_index: Vec::new(),
    };
    let mut pos = 1; // skip the SCHEMA_V2 tag
    decode_schema(block, &mut pos, &mut meta)?;
    Ok((
        meta.class_hierarchy,
        meta.subclass_cycles,
        meta.disjoint_pairs,
        meta.equivalent_pairs,
    ))
}

/// Decode the same standalone schema block as [`decode_schema_block`] but return
/// the **schema summary** — the per-class instance histogram and the class-to-class
/// relations at the finest (leaf) semantic-zoom level. This is the index-free,
/// range-readable source for a Schema view of a remote graph: `(classes, relations)`
/// where `classes = [(class_iri, count)]` and `relations = [(s_class, predicate,
/// o_class, count)]`.
#[allow(clippy::type_complexity)]
pub fn decode_schema_block_summary(
    block: &[u8],
) -> Result<(Vec<(String, u64)>, Vec<(String, String, String, u64)>), MetaError> {
    if block.first() != Some(&SCHEMA_V2) {
        return Err(MetaError::Malformed("not a schema block"));
    }
    let mut meta = PyramidMeta {
        round: 0,
        summary: Vec::new(),
        tiles: Vec::new(),
        class_hierarchy: Vec::new(),
        level_rollups: Vec::new(),
        level_links: Vec::new(),
        descriptors: Vec::new(),
        subclass_cycles: Vec::new(),
        disjoint_pairs: Vec::new(),
        equivalent_pairs: Vec::new(),
        predicate_stats: Vec::new(),
        char_sets: Vec::new(),
        label_index: Vec::new(),
    };
    let mut pos = 1;
    decode_schema(block, &mut pos, &mut meta)?;
    // Finest level = largest depth (leaves); resolves abstract rollups to real classes.
    let classes = meta
        .level_rollups
        .iter()
        .max_by_key(|r| r.depth)
        .map(|r| r.classes.clone())
        .unwrap_or_default();
    let relations = meta
        .level_links
        .iter()
        .max_by_key(|l| l.depth)
        .map(|l| {
            l.links
                .iter()
                .map(|c| {
                    (
                        c.s_class.clone(),
                        c.predicate.clone(),
                        c.o_class.clone(),
                        c.count,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Ok((classes, relations))
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    write_uvarint(out, s.len() as u64);
    out.extend_from_slice(s.as_bytes());
}

fn read_str(bytes: &[u8], pos: &mut usize) -> Result<String, MetaError> {
    let (len, n) = read_uvarint(&bytes[*pos..]).ok_or(MetaError::Malformed("truncated str len"))?;
    *pos += n;
    let len = len as usize;
    let end = pos
        .checked_add(len)
        .filter(|&e| e <= bytes.len())
        .ok_or(MetaError::Malformed("string overruns buffer"))?;
    let s = std::str::from_utf8(&bytes[*pos..end])
        .map_err(|_| MetaError::Malformed("invalid utf8"))?
        .to_string();
    *pos = end;
    Ok(s)
}

fn read_u8(bytes: &[u8], pos: &mut usize) -> Result<u8, MetaError> {
    let b = *bytes
        .get(*pos)
        .ok_or(MetaError::Malformed("truncated u8"))?;
    *pos += 1;
    Ok(b)
}

fn read_f64(bytes: &[u8], pos: &mut usize) -> Result<f64, MetaError> {
    let end = pos
        .checked_add(8)
        .filter(|&e| e <= bytes.len())
        .ok_or(MetaError::Malformed("truncated f64"))?;
    let arr: [u8; 8] = bytes[*pos..end].try_into().unwrap();
    *pos = end;
    Ok(f64::from_le_bytes(arr))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_v1() -> PyramidMeta {
        let summary = vec![
            SuperEdge {
                s_comm: 0,
                predicate: 5,
                o_comm: 0,
                count: 4,
            },
            SuperEdge {
                s_comm: 0,
                predicate: 5,
                o_comm: 1,
                count: 1,
            },
        ];
        let tiles = vec![
            Tile {
                community: 0,
                triples: vec![],
                encoded: vec![1, 2, 3],
            },
            Tile {
                community: 1,
                triples: vec![],
                encoded: vec![9, 8],
            },
        ];
        PyramidMeta::new(2, summary, &tiles)
    }

    #[test]
    fn meta_round_trips() {
        let meta = sample_v1();
        let bytes = meta.encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(meta, back);
        assert_eq!(back.round, 2);
        assert_eq!(back.tiles[0], (0, vec![1, 2, 3]));
        assert!(back.class_hierarchy.is_empty(), "v1 shape has no schema");
    }

    #[test]
    fn empty_meta() {
        let meta = PyramidMeta::new(0, vec![], &[]);
        assert_eq!(PyramidMeta::decode(&meta.encode()).unwrap(), meta);
    }

    #[test]
    fn v1_encoding_is_unchanged_without_schema() {
        // A schema-less meta must encode exactly as before — no trailing v2 byte —
        // so typeless files stay byte-identical across the v1→v2 upgrade.
        let meta = sample_v1();
        let bytes = meta.encode();
        // The last byte is the final tile block byte (8), not a schema tag.
        assert_eq!(*bytes.last().unwrap(), 8);
        assert_ne!(*bytes.last().unwrap(), SCHEMA_V2);
    }

    #[test]
    fn schema_pyramid_round_trips() {
        let mut meta = sample_v1();
        meta = meta.with_schema(
            vec![
                ClassNode {
                    class: "<http://ex/Agent>".into(),
                    parents: vec![],
                    depth: 0,
                },
                ClassNode {
                    // A non-exclusive node: two parents (multiple inheritance).
                    class: "<http://ex/Astronaut>".into(),
                    parents: vec!["<http://ex/Explorer>".into(), "<http://ex/Person>".into()],
                    depth: 2,
                },
            ],
            vec![
                LevelRollup {
                    round: 1,
                    depth: 0,
                    classes: vec![("<http://ex/Agent>".into(), 12)],
                },
                LevelRollup {
                    round: 0,
                    depth: 1,
                    classes: vec![
                        ("<http://ex/Person>".into(), 9),
                        ("<http://ex/Agent>".into(), 3),
                    ],
                },
            ],
            vec![LevelLinks {
                round: 1,
                depth: 0,
                links: vec![ClassRelation {
                    s_class: "<http://ex/Agent>".into(),
                    predicate: "<http://ex/memberOf>".into(),
                    o_class: "<http://ex/Agent>".into(),
                    count: 6,
                }],
            }],
            vec![CommunityDescriptor {
                community: 7,
                dominant_class: Some("<http://ex/Person>".into()),
                class_counts: vec![("<http://ex/Person>".into(), 5)],
                bbox: Some([-10.0, 40.0, 12.5, 51.2]),
                time_range: Some(("1700".into(), "1900".into())),
            }],
            vec![],
            vec![],
            vec![],
        );
        let bytes = meta.encode();
        // The v2 block is appended → last byte is NOT the v1 tile byte.
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(meta, back);
        assert_eq!(back.level_rollups.len(), 2);
        // Non-exclusive hierarchy: both parents survive the round-trip.
        assert_eq!(back.class_hierarchy[1].parents.len(), 2);
        // Lateral connections round-trip.
        assert_eq!(
            back.level_links[0].links[0].predicate,
            "<http://ex/memberOf>"
        );
        assert_eq!(back.descriptors[0].bbox, Some([-10.0, 40.0, 12.5, 51.2]));
        assert_eq!(
            back.descriptors[0].time_range,
            Some(("1700".into(), "1900".into()))
        );
    }

    #[test]
    fn coherence_axioms_round_trip_and_stay_additive() {
        // Hierarchy names all the classes the axioms reference, so the interned
        // string table (written at the start of the schema block) is identical with
        // or without the axioms — the extension is then a pure byte append.
        let hier = || {
            vec![
                ClassNode {
                    class: "<http://ex/C>".into(),
                    parents: vec![],
                    depth: 0,
                },
                ClassNode {
                    class: "<http://ex/D>".into(),
                    parents: vec![],
                    depth: 0,
                },
                ClassNode {
                    class: "<http://ex/E>".into(),
                    parents: vec![],
                    depth: 0,
                },
            ]
        };
        // v2.0: hierarchy only, no coherence axioms.
        let v20 = sample_v1()
            .with_schema(hier(), vec![], vec![], vec![], vec![], vec![], vec![])
            .encode();
        // v2.1: same hierarchy + axioms over hierarchy classes → table unchanged.
        let full = sample_v1().with_schema(
            hier(),
            vec![],
            vec![],
            vec![],
            vec![],
            vec![("<http://ex/D>".into(), "<http://ex/E>".into())],
            vec![],
        );
        let v21 = full.encode();
        assert!(
            v21.starts_with(&v20),
            "the extension is appended; the v2.0 prefix is byte-identical"
        );
        assert!(v21.len() > v20.len());

        let back = PyramidMeta::decode(&v21).unwrap();
        assert_eq!(back, full, "the coherence axioms round-trip");
        assert_eq!(
            back.disjoint_pairs,
            vec![("<http://ex/D>".to_string(), "<http://ex/E>".to_string())]
        );

        // The v2.0 bytes (no extension) decode with empty axioms AND an intact
        // hierarchy — the hoist fix: a missing extension must not discard v2.0.
        let back20 = PyramidMeta::decode(&v20).unwrap();
        assert!(back20.disjoint_pairs.is_empty());
        assert!(back20.subclass_cycles.is_empty());
        assert_eq!(back20.class_hierarchy.len(), 3, "v2.0 hierarchy intact");
    }

    #[test]
    fn v2_is_readable_as_v1_prefix() {
        // A v2 file's leading bytes are exactly the v1 encoding, so an old reader
        // that stops after the tiles loop still recovers round/summary/tiles.
        let mut meta = sample_v1();
        let v1_bytes = meta.encode();
        meta = meta.with_schema(
            vec![ClassNode {
                class: "<http://ex/C>".into(),
                parents: vec![],
                depth: 0,
            }],
            vec![LevelRollup {
                round: 0,
                depth: 0,
                classes: vec![("<http://ex/C>".into(), 1)],
            }],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let v2_bytes = meta.encode();
        assert!(
            v2_bytes.starts_with(&v1_bytes),
            "v2 extends v1 byte-for-byte"
        );
        assert_eq!(v2_bytes[v1_bytes.len()], SCHEMA_V2, "v2 tag follows");
    }

    #[test]
    fn query_stats_round_trip_and_stay_additive() {
        let v1 = sample_v1().encode();
        let stats = vec![
            PredStat {
                predicate: 5,
                count: 100,
                distinct_subjects: 40,
                distinct_objects: 25,
                max_objects_per_subject: 6,
                max_subjects_per_object: 9,
            },
            PredStat {
                predicate: 9,
                count: 3,
                distinct_subjects: 3,
                distinct_objects: 1,
                max_objects_per_subject: 1,
                max_subjects_per_object: 3,
            },
        ];
        let bytes = sample_v1().with_predicate_stats(stats.clone()).encode();
        // The qstats block is appended after the v1 section → v1 prefix unchanged.
        assert!(bytes.starts_with(&v1), "query-stats is appended after v1");
        assert_eq!(bytes[v1.len()], QSTATS_V1, "the qstats tag follows v1");
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(back.predicate_stats, stats);
        assert_eq!(back.round, 2, "v1 fields intact");
        assert_eq!(back.tiles[0], (0, vec![1, 2, 3]));
    }

    #[test]
    fn query_stats_before_schema_keeps_schema_trailing() {
        // With BOTH stats and a schema block, schema_block_len must skip the
        // qstats block and still find the trailing schema block.
        let stats = vec![PredStat {
            predicate: 1,
            count: 9,
            distinct_subjects: 3,
            distinct_objects: 3,
            max_objects_per_subject: 3,
            max_subjects_per_object: 3,
        }];
        let bytes = sample_v1()
            .with_schema(
                vec![ClassNode {
                    class: "<http://ex/C>".into(),
                    parents: vec![],
                    depth: 0,
                }],
                vec![LevelRollup {
                    round: 0,
                    depth: 0,
                    classes: vec![("<http://ex/C>".into(), 1)],
                }],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .with_predicate_stats(stats.clone())
            .encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(back.predicate_stats, stats);
        assert_eq!(back.class_hierarchy.len(), 1, "schema decoded past qstats");
        let len = schema_block_len(&bytes) as usize;
        assert!(len > 0);
        assert_eq!(
            bytes[bytes.len() - len],
            SCHEMA_V2,
            "schema is still the trailing block"
        );
    }

    #[test]
    fn char_sets_coexist_with_query_stats_and_schema() {
        let sets = vec![
            CharSet {
                predicates: vec![1, 5, 9],
                subjects: 100,
            },
            CharSet {
                predicates: vec![1, 5],
                subjects: 40,
            },
        ];
        let stats = vec![PredStat {
            predicate: 1,
            count: 9,
            distinct_subjects: 3,
            distinct_objects: 3,
            max_objects_per_subject: 3,
            max_subjects_per_object: 3,
        }];
        let bytes = sample_v1()
            .with_predicate_stats(stats.clone())
            .with_char_sets(sets.clone())
            .with_schema(
                vec![ClassNode {
                    class: "<http://ex/C>".into(),
                    parents: vec![],
                    depth: 0,
                }],
                vec![LevelRollup {
                    round: 0,
                    depth: 0,
                    classes: vec![("<http://ex/C>".into(), 1)],
                }],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(back.predicate_stats, stats);
        assert_eq!(back.char_sets, sets);
        assert_eq!(
            back.class_hierarchy.len(),
            1,
            "schema decoded past both blocks"
        );
        let len = schema_block_len(&bytes) as usize;
        assert_eq!(bytes[bytes.len() - len], SCHEMA_V2, "schema still trailing");
    }

    #[test]
    fn label_index_round_trips_and_prefix_searches() {
        // Sorted by lowercased label (case-insensitive), as the builder emits.
        let labels = vec![
            LabelEntry {
                label: "Alanine".into(),
                subject: 10,
            },
            LabelEntry {
                label: "alpha-D-glucose".into(),
                subject: 11,
            },
            LabelEntry {
                label: "Benzene".into(),
                subject: 12,
            },
        ];
        let meta = sample_v1().with_label_index(labels.clone());
        let bytes = meta.encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(back.label_index, labels, "label index survives round trip");

        // Case-insensitive prefix: "al" matches Alanine + alpha-D-glucose, in order.
        let hits = back.prefix_search("al", 10);
        assert_eq!(
            hits.iter().map(|e| e.subject).collect::<Vec<_>>(),
            vec![10, 11]
        );
        // A more specific prefix narrows to one; the walk stops at the run's end.
        let hits = back.prefix_search("BEN", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, 12);
        // No match → empty.
        assert!(back.prefix_search("zzz", 10).is_empty());
        // The limit caps the result.
        assert_eq!(back.prefix_search("", 2).len(), 2);
    }

    #[test]
    fn label_index_stays_additive_before_schema() {
        let labels = vec![LabelEntry {
            label: "x".into(),
            subject: 1,
        }];
        let stats = vec![PredStat {
            predicate: 1,
            count: 1,
            distinct_subjects: 1,
            distinct_objects: 1,
            max_objects_per_subject: 1,
            max_subjects_per_object: 1,
        }];
        let sets = vec![CharSet {
            predicates: vec![1],
            subjects: 1,
        }];
        let bytes = sample_v1()
            .with_predicate_stats(stats.clone())
            .with_char_sets(sets.clone())
            .with_label_index(labels.clone())
            .with_schema(
                vec![ClassNode {
                    class: "<http://ex/C>".into(),
                    parents: vec![],
                    depth: 0,
                }],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
            )
            .encode();
        let back = PyramidMeta::decode(&bytes).unwrap();
        assert_eq!(back.predicate_stats, stats);
        assert_eq!(back.char_sets, sets);
        assert_eq!(back.label_index, labels);
        assert_eq!(
            back.class_hierarchy.len(),
            1,
            "schema decoded past 3 blocks"
        );
        let len = schema_block_len(&bytes) as usize;
        assert_eq!(
            bytes[bytes.len() - len],
            SCHEMA_V2,
            "schema still trailing past the label index"
        );
    }
}
