//! The **SCHEMA** section (kind 9, SPEC §7.5): the class hierarchy, the
//! per-level type rollups and lateral class links, the coherence axioms, the
//! planner's per-predicate `query_stats` and the characteristic sets —
//! computed **without the community dendrogram**, so any build writes it,
//! `--no-pyramid` and the memory-bounded build included.
//!
//! These used to be blocks of the pyramid-meta (kind 4), built only after the
//! dendrogram; a file without a pyramid had none of them, and the planner had
//! no statistics on exactly the large files where it matters most.
//!
//! ## Layout
//!
//! ```text
//! 0   16  digest: blake3(section[16..])[..16]  (the section is outside the header hash)
//! 16  1   version (= 1)
//! 17  …   varint len + bytes: the type predicate the rollups used (empty = no typing)
//!     …   a pyramid-meta encoding (SPEC §7.3) with round 0, no superedges, no tiles,
//!         no label block and no descriptors: [QSTATS_V1] [CHARSET_V1] [SCHEMA_V2]
//! ```
//!
//! ## Rules (order-independent, so every builder writes the same bytes)
//!
//! Everything is computed over the **deduplicated** default graph:
//! - the type predicate is `--type-predicate` when given, else `rdf:type`;
//! - a subject's representative class is its type with the **smallest object
//!   id**; instance counts count every type statement;
//! - lateral links `(class(s), p, class(o))` skip the type and `subClassOf`
//!   statements; `(literal)` / `(untyped)` are the object sentinels;
//! - levels carry `round = 0` (there is no dendrogram to align with);
//! - characteristic sets: the 128 commonest exact predicate sets.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::meta::{CharSet, ClassNode, LevelLinks, LevelRollup, PredStat, PyramidMeta};
use crate::varint::{read_uvarint, write_uvarint};

/// Section format version written by this crate.
const VERSION: u8 = 1;
/// Length of the leading digest.
const DIGEST_LEN: usize = 16;
/// Characteristic sets kept (the same bound as the pyramid's block).
const MAX_CHAR_SETS: usize = 128;
/// Distinct predicate sets tracked before singletons are pruned.
const MAX_TRACKED_SHAPES: usize = 1 << 20;

/// `rdfs:subClassOf`.
pub(crate) const RDFS_SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
/// `owl:disjointWith`.
pub(crate) const OWL_DISJOINT_WITH: &str = "<http://www.w3.org/2002/07/owl#disjointWith>";
/// `owl:equivalentClass`.
pub(crate) const OWL_EQUIVALENT_CLASS: &str = "<http://www.w3.org/2002/07/owl#equivalentClass>";

/// Class code of an object with no class.
pub(crate) const UNTYPED: u32 = u32::MAX;
/// Class code of a literal object.
pub(crate) const LITERAL: u32 = u32::MAX - 1;

/// A decoded SCHEMA section — or, for a file built before it existed, the same
/// fields read from the pyramid-meta (see [`SchemaMeta::from_pyramid`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SchemaMeta {
    /// The type predicate the class rollups used; `None` when the graph has no
    /// typing (or the fields came from a pyramid-meta, which does not say).
    pub type_predicate: Option<String>,
    /// Per-predicate cardinality for the planner; see [`PredStat`].
    pub predicate_stats: Vec<PredStat>,
    /// The commonest entity shapes; see [`CharSet`].
    pub char_sets: Vec<CharSet>,
    /// The `subClassOf` DAG with per-class depth.
    pub class_hierarchy: Vec<ClassNode>,
    /// The per-level type histograms.
    pub level_rollups: Vec<LevelRollup>,
    /// The per-level lateral class-relation graphs.
    pub level_links: Vec<LevelLinks>,
    /// `subClassOf` cycles.
    pub subclass_cycles: Vec<Vec<String>>,
    /// `owl:disjointWith` class pairs.
    pub disjoint_pairs: Vec<(String, String)>,
    /// `owl:equivalentClass` class pairs.
    pub equivalent_pairs: Vec<(String, String)>,
}

impl SchemaMeta {
    /// True when the section would carry nothing.
    pub fn is_empty(&self) -> bool {
        self.predicate_stats.is_empty()
            && self.char_sets.is_empty()
            && self.class_hierarchy.is_empty()
            && self.level_rollups.is_empty()
            && self.level_links.is_empty()
            && self.subclass_cycles.is_empty()
            && self.disjoint_pairs.is_empty()
            && self.equivalent_pairs.is_empty()
    }

    /// The schema fields of a pyramid-meta (files built before kind 9).
    pub fn from_pyramid(p: &PyramidMeta) -> Self {
        SchemaMeta {
            type_predicate: None,
            predicate_stats: p.predicate_stats.clone(),
            char_sets: p.char_sets.clone(),
            class_hierarchy: p.class_hierarchy.clone(),
            level_rollups: p.level_rollups.clone(),
            level_links: p.level_links.clone(),
            subclass_cycles: p.subclass_cycles.clone(),
            disjoint_pairs: p.disjoint_pairs.clone(),
            equivalent_pairs: p.equivalent_pairs.clone(),
        }
    }

    /// Encode the whole section, digest first. Empty for an empty schema.
    pub fn encode(&self) -> Vec<u8> {
        if self.is_empty() {
            return Vec::new();
        }
        let pm = PyramidMeta::new(0, Vec::new(), &[])
            .with_schema(
                self.class_hierarchy.clone(),
                self.level_rollups.clone(),
                self.level_links.clone(),
                Vec::new(),
                self.subclass_cycles.clone(),
                self.disjoint_pairs.clone(),
                self.equivalent_pairs.clone(),
            )
            .with_predicate_stats(self.predicate_stats.clone())
            .with_char_sets(self.char_sets.clone());
        let mut body = vec![VERSION];
        let tp = self.type_predicate.as_deref().unwrap_or("");
        write_uvarint(&mut body, tp.len() as u64);
        body.extend_from_slice(tp.as_bytes());
        body.extend_from_slice(&pm.encode());
        let mut out = Vec::with_capacity(DIGEST_LEN + body.len());
        out.extend_from_slice(&blake3::hash(&body).as_bytes()[..DIGEST_LEN]);
        out.extend_from_slice(&body);
        out
    }

    /// Decode a section, checking its digest and version.
    pub fn decode(section: &[u8]) -> Result<Self, crate::file::FileError> {
        use crate::file::FileError;
        if !crate::label_index::digest_ok(section) {
            return Err(FileError::Container("schema section: digest mismatch"));
        }
        let body = &section[DIGEST_LEN..];
        if body.first() != Some(&VERSION) {
            return Err(FileError::Container("schema section: unknown version"));
        }
        let (len, n) =
            read_uvarint(&body[1..]).ok_or(FileError::Container("schema section: truncated"))?;
        let start = 1 + n;
        let end = start
            .checked_add(len as usize)
            .filter(|&e| e <= body.len())
            .ok_or(FileError::Container("schema section: truncated"))?;
        let tp = std::str::from_utf8(&body[start..end])
            .map_err(|_| FileError::Container("schema section: bad utf8"))?;
        let pm = PyramidMeta::decode(&body[end..])
            .map_err(|_| FileError::Container("schema section: malformed blocks"))?;
        Ok(SchemaMeta {
            type_predicate: (!tp.is_empty()).then(|| tp.to_string()),
            predicate_stats: pm.predicate_stats,
            char_sets: pm.char_sets,
            class_hierarchy: pm.class_hierarchy,
            level_rollups: pm.level_rollups,
            level_links: pm.level_links,
            subclass_cycles: pm.subclass_cycles,
            disjoint_pairs: pm.disjoint_pairs,
            equivalent_pairs: pm.equivalent_pairs,
        })
    }
}

/// Which id space an id lives in, for resolving it to a term.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Space {
    Subject,
    Object,
    Predicate,
}

/// The ids a schema pass needs, resolved against one file's dictionary.
#[derive(Debug, Clone, Default)]
pub(crate) struct SchemaIds {
    /// The type predicate's id and term.
    pub type_pid: Option<u32>,
    pub type_term: Option<String>,
    pub subclass_pid: Option<u32>,
    pub disjoint_pid: Option<u32>,
    pub equivalent_pid: Option<u32>,
    /// `S`, the shared-term count: object ids `1..=S` are also subjects.
    pub n_shared: u32,
    /// Shared ids `1..=shared_literals` are literals (normally none).
    pub shared_literals: u32,
    /// Object ids `S+1..=S+objonly_literals` are literals: literals (`"…`)
    /// sort before IRIs (`<`) and blank nodes (`_`) in every section.
    pub objonly_literals: u32,
}

impl SchemaIds {
    fn is_literal(&self, o: u32) -> bool {
        o <= self.shared_literals
            || (o > self.n_shared && o - self.n_shared <= self.objonly_literals)
    }

    /// The type predicate term a build uses: the override, else `rdf:type`.
    pub(crate) fn type_term_for(type_override: Option<&str>) -> String {
        type_override.unwrap_or(crate::RDF_TYPE).to_string()
    }

    /// Resolve against an in-memory dictionary.
    pub(crate) fn from_dictionary(dict: &crate::Dictionary, type_override: Option<&str>) -> Self {
        let type_term = Self::type_term_for(type_override);
        let type_pid = dict.predicate_id(&type_term);
        let n_shared = dict.shared_count();
        let is_lit = |t: Option<String>| t.is_some_and(|t| t.starts_with('"'));
        // Literals are a prefix of each sorted section: binary-search its end.
        let prefix_len = |n: u32, term: &dyn Fn(u32) -> Option<String>| -> u32 {
            let (mut lo, mut hi) = (0u32, n);
            while lo < hi {
                let mid = lo + (hi - lo) / 2;
                if is_lit(term(mid + 1)) {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let shared_literals = prefix_len(n_shared, &|i| dict.object_term(i));
        let objonly_literals = prefix_len(dict.object_only_count(), &|i| {
            dict.object_term(n_shared + i)
        });
        SchemaIds {
            type_pid,
            type_term: type_pid.map(|_| type_term),
            subclass_pid: dict.predicate_id(RDFS_SUBCLASS_OF),
            disjoint_pid: dict.predicate_id(OWL_DISJOINT_WITH),
            equivalent_pid: dict.predicate_id(OWL_EQUIVALENT_CLASS),
            n_shared,
            shared_literals,
            objonly_literals,
        }
    }
}

/// A link whose object class is not known yet: `(object id, class(s), p)`.
pub(crate) type DeferredLink = (u32, u32, u32);

/// Accumulates every schema statistic from two sorted, deduplicated passes over
/// the default graph — SPO order, then POS order. Both builders feed it the
/// same statements in the same order, which is what makes their sections
/// byte-identical.
pub(crate) struct SchemaAccumulator {
    ids: SchemaIds,
    /// predicate → [count, distinct subjects, distinct objects,
    /// max objects per subject, max subjects per object]
    pstats: HashMap<u32, [u64; 5]>,
    cur_s: Option<u32>,
    cur_p: Option<u32>,
    run_sp: u64,
    shape: Vec<u32>,
    shapes: HashMap<Vec<u32>, u64>,
    shapes_frozen: bool,
    cur_po: Option<(u32, u32)>,
    run_po: u64,
    instance: HashMap<u32, u64>,
    subclass: Vec<(u32, u32)>,
    disjoint: Vec<(u32, u32)>,
    equivalent: Vec<(u32, u32)>,
    links: HashMap<(u32, u32, u32), u64>,
}

impl SchemaAccumulator {
    pub(crate) fn new(ids: SchemaIds) -> Self {
        SchemaAccumulator {
            ids,
            pstats: HashMap::new(),
            cur_s: None,
            cur_p: None,
            run_sp: 0,
            shape: Vec::new(),
            shapes: HashMap::new(),
            shapes_frozen: false,
            cur_po: None,
            run_po: 0,
            instance: HashMap::new(),
            subclass: Vec::new(),
            disjoint: Vec::new(),
            equivalent: Vec::new(),
            links: HashMap::new(),
        }
    }

    pub(crate) fn ids(&self) -> &SchemaIds {
        &self.ids
    }

    fn end_sp(&mut self) {
        if let Some(p) = self.cur_p.take() {
            let e = self.pstats.entry(p).or_default();
            e[3] = e[3].max(self.run_sp);
        }
        self.run_sp = 0;
    }

    fn end_subject(&mut self) {
        self.end_sp();
        if self.cur_s.take().is_none() {
            return;
        }
        let shape = std::mem::take(&mut self.shape);
        if let Some(c) = self.shapes.get_mut(&shape) {
            *c += 1;
        } else if !self.shapes_frozen {
            self.shapes.insert(shape, 1);
            if self.shapes.len() > MAX_TRACKED_SHAPES {
                // Deterministic bound: drop the shapes seen once so far; if
                // that does not halve the map, stop admitting new shapes.
                self.shapes.retain(|_, c| *c > 1);
                if self.shapes.len() > MAX_TRACKED_SHAPES / 2 {
                    self.shapes_frozen = true;
                }
            }
        }
    }

    /// One statement in SPO order, with its subject's representative class.
    /// Returns a [`DeferredLink`] when the object's class has to be looked up
    /// by the caller (the object is a shared term); the caller resolves it
    /// and calls [`add_link`](Self::add_link).
    pub(crate) fn observe_spo(
        &mut self,
        s: u32,
        p: u32,
        o: u32,
        class_s: Option<u32>,
    ) -> Option<DeferredLink> {
        if self.cur_s != Some(s) {
            self.end_subject();
            self.cur_s = Some(s);
        }
        if self.cur_p != Some(p) {
            self.end_sp();
            self.cur_p = Some(p);
            self.shape.push(p);
            self.pstats.entry(p).or_default()[1] += 1;
        }
        self.run_sp += 1;
        self.pstats.entry(p).or_default()[0] += 1;

        let ids = &self.ids;
        if Some(p) == ids.type_pid {
            *self.instance.entry(o).or_default() += 1;
            return None;
        }
        if Some(p) == ids.subclass_pid {
            self.subclass.push((s, o));
            return None;
        }
        if Some(p) == ids.disjoint_pid {
            self.disjoint.push((s, o));
        }
        if Some(p) == ids.equivalent_pid {
            self.equivalent.push((s, o));
        }
        ids.type_pid?;
        let cs = class_s.unwrap_or(UNTYPED);
        if ids.is_literal(o) {
            self.add_link(cs, p, LITERAL);
            None
        } else if o > ids.n_shared {
            // An object-only term is never a subject, so it has no class.
            self.add_link(cs, p, UNTYPED);
            None
        } else {
            Some((o, cs, p))
        }
    }

    /// Count one lateral link.
    pub(crate) fn add_link(&mut self, class_s: u32, p: u32, class_o: u32) {
        *self.links.entry((class_s, p, class_o)).or_default() += 1;
    }

    /// End of the SPO pass.
    pub(crate) fn finish_spo(&mut self) {
        self.end_subject();
    }

    fn end_po(&mut self) {
        if let Some((p, _)) = self.cur_po.take() {
            let e = self.pstats.entry(p).or_default();
            e[4] = e[4].max(self.run_po);
        }
        self.run_po = 0;
    }

    /// One statement in POS order.
    pub(crate) fn observe_pos(&mut self, p: u32, o: u32) {
        if self.cur_po != Some((p, o)) {
            self.end_po();
            self.cur_po = Some((p, o));
            self.pstats.entry(p).or_default()[2] += 1;
        }
        self.run_po += 1;
    }

    /// End of the POS pass.
    pub(crate) fn finish_pos(&mut self) {
        self.end_po();
    }

    /// Resolve ids to terms and fold everything into the section.
    pub(crate) fn finalize(self, resolve: &dyn Fn(Space, u32) -> Option<String>) -> SchemaMeta {
        let mut predicate_stats: Vec<PredStat> = self
            .pstats
            .iter()
            .map(|(&predicate, e)| PredStat {
                predicate,
                count: e[0],
                distinct_subjects: e[1],
                distinct_objects: e[2],
                max_objects_per_subject: e[3].min(u32::MAX as u64) as u32,
                max_subjects_per_object: e[4].min(u32::MAX as u64) as u32,
            })
            .collect();
        predicate_stats.sort_by_key(|p| p.predicate);

        let mut char_sets: Vec<CharSet> = self
            .shapes
            .into_iter()
            .map(|(predicates, subjects)| CharSet {
                predicates,
                subjects,
            })
            .collect();
        char_sets.sort_by(|a, b| {
            b.subjects
                .cmp(&a.subjects)
                .then_with(|| a.predicates.cmp(&b.predicates))
        });
        char_sets.truncate(MAX_CHAR_SETS);

        let mut out = SchemaMeta {
            type_predicate: None,
            predicate_stats,
            char_sets,
            ..SchemaMeta::default()
        };
        if self.instance.is_empty() {
            return out;
        }
        out.type_predicate = self.ids.type_term.clone();
        let class = |c: u32| -> Option<String> {
            match c {
                UNTYPED => Some("(untyped)".to_string()),
                LITERAL => Some("(literal)".to_string()),
                _ => resolve(Space::Object, c),
            }
        };
        let mut instance_counts: BTreeMap<String, u64> = BTreeMap::new();
        for (&c, &n) in &self.instance {
            if let Some(t) = resolve(Space::Object, c) {
                *instance_counts.entry(t).or_default() += n;
            }
        }
        let mut parents: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut self_loops: BTreeSet<String> = BTreeSet::new();
        for &(s, o) in &self.subclass {
            if let (Some(child), Some(parent)) =
                (resolve(Space::Subject, s), resolve(Space::Object, o))
            {
                if child != parent {
                    parents.entry(child).or_default().insert(parent);
                } else {
                    self_loops.insert(child);
                }
            }
        }
        let pairs = |v: &[(u32, u32)]| -> Vec<(String, String)> {
            let mut set: BTreeSet<(String, String)> = BTreeSet::new();
            for &(s, o) in v {
                if let (Some(a), Some(b)) = (resolve(Space::Subject, s), resolve(Space::Object, o))
                {
                    if a != b {
                        set.insert(if a < b { (a, b) } else { (b, a) });
                    }
                }
            }
            set.into_iter().collect()
        };
        let disjoint_pairs = pairs(&self.disjoint);
        let equivalent_pairs = pairs(&self.equivalent);
        let mut leaf_links: BTreeMap<(String, String, String), u64> = BTreeMap::new();
        for (&(cs, p, co), &n) in &self.links {
            if let (Some(a), Some(pr), Some(b)) =
                (class(cs), resolve(Space::Predicate, p), class(co))
            {
                *leaf_links.entry((a, pr, b)).or_default() += n;
            }
        }
        let core = crate::schema_pyramid::assemble_schema(
            &instance_counts,
            &parents,
            &self_loops,
            &leaf_links,
            &|_, _| 0,
        );
        out.class_hierarchy = core.class_hierarchy;
        out.level_rollups = core.level_rollups;
        out.level_links = core.level_links;
        out.subclass_cycles = core.subclass_cycles;
        out.disjoint_pairs = disjoint_pairs;
        out.equivalent_pairs = equivalent_pairs;
        out
    }
}

/// Walks a `(subject, class)` list sorted by subject, answering "the class of
/// subject `s`" for non-decreasing `s`.
pub(crate) struct ClassCursor<I: Iterator<Item = (u32, u32)>> {
    it: I,
    cur: Option<(u32, u32)>,
}

impl<I: Iterator<Item = (u32, u32)>> ClassCursor<I> {
    pub(crate) fn new(mut it: I) -> Self {
        let cur = it.next();
        ClassCursor { it, cur }
    }

    /// The class of `s`; calls must not go backwards in `s`.
    pub(crate) fn get(&mut self, s: u32) -> Option<u32> {
        while let Some((cs, _)) = self.cur {
            if cs >= s {
                break;
            }
            self.cur = self.it.next();
        }
        match self.cur {
            Some((cs, c)) if cs == s => Some(c),
            _ => None,
        }
    }
}

/// Compute the SCHEMA section of an in-memory graph. `triples` must hold the
/// **deduplicated** default graph **sorted in SPO order**; it is left sorted
/// in POS order (every index build re-sorts anyway).
pub(crate) fn compute_in_memory(
    dict: &crate::Dictionary,
    triples: &mut [(u32, u32, u32)],
    type_override: Option<&str>,
) -> SchemaMeta {
    let ids = SchemaIds::from_dictionary(dict, type_override);
    // Representative class per subject: the smallest class id, which in SPO
    // order is the first type statement of each subject.
    let mut reps: Vec<(u32, u32)> = Vec::new();
    if let Some(tp) = ids.type_pid {
        for &(s, p, o) in triples.iter() {
            if p == tp && reps.last().map(|r| r.0) != Some(s) {
                reps.push((s, o));
            }
        }
    }
    let mut acc = SchemaAccumulator::new(ids);
    let mut cursor = ClassCursor::new(reps.iter().copied());
    for &(s, p, o) in triples.iter() {
        let cs = cursor.get(s);
        if let Some((obj, cls_s, pred)) = acc.observe_spo(s, p, o, cs) {
            let co = reps
                .binary_search_by_key(&obj, |r| r.0)
                .map(|i| reps[i].1)
                .unwrap_or(UNTYPED);
            acc.add_link(cls_s, pred, co);
        }
    }
    acc.finish_spo();
    sort_pos(triples);
    for &(_s, p, o) in triples.iter() {
        acc.observe_pos(p, o);
    }
    acc.finish_pos();
    acc.finalize(&|space, id| match space {
        Space::Subject => dict.subject_term(id),
        Space::Object => dict.object_term(id),
        Space::Predicate => dict.predicate_term(id),
    })
}

#[cfg(feature = "parallel")]
fn sort_pos(v: &mut [(u32, u32, u32)]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable_by_key(|&(s, p, o)| (p, o, s));
}
#[cfg(not(feature = "parallel"))]
fn sort_pos(v: &mut [(u32, u32, u32)]) {
    v.sort_unstable_by_key(|&(s, p, o)| (p, o, s));
}

/// Sort and deduplicate id-triples in SPO order, in place.
pub(crate) fn sort_dedup_spo(v: &mut Vec<(u32, u32, u32)>) {
    #[cfg(feature = "parallel")]
    {
        use rayon::slice::ParallelSliceMut;
        v.par_sort_unstable();
    }
    #[cfg(not(feature = "parallel"))]
    v.sort_unstable();
    v.dedup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DictionaryBuilder;

    fn build(triples: &[(&str, &str, &str)]) -> (crate::Dictionary, Vec<(u32, u32, u32)>) {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        sort_dedup_spo(&mut ids);
        (dict, ids)
    }

    const T: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

    #[test]
    fn computes_stats_shapes_and_schema_without_a_dendrogram() {
        let (dict, mut ids) = build(&[
            ("<a>", T, "<Person>"),
            ("<a>", "<knows>", "<b>"),
            ("<a>", "<name>", "\"A\""),
            ("<b>", T, "<Person>"),
            ("<b>", "<name>", "\"B\""),
            ("<b>", "<name>", "\"B2\""),
            ("<c>", T, "<Org>"),
            ("<a>", "<worksFor>", "<c>"),
            ("<Person>", RDFS_SUBCLASS_OF, "<Agent>"),
            ("<Org>", RDFS_SUBCLASS_OF, "<Agent>"),
            ("<a>", "<knows>", "<b>"), // duplicate: must not count twice
        ]);
        let m = compute_in_memory(&dict, &mut ids, None);
        let name = dict.predicate_id("<name>").unwrap();
        let st = m
            .predicate_stats
            .iter()
            .find(|s| s.predicate == name)
            .unwrap();
        assert_eq!(
            (st.count, st.distinct_subjects, st.distinct_objects),
            (3, 2, 3)
        );
        assert_eq!(st.max_objects_per_subject, 2);
        let knows = dict.predicate_id("<knows>").unwrap();
        assert_eq!(
            m.predicate_stats
                .iter()
                .find(|s| s.predicate == knows)
                .unwrap()
                .count,
            1,
            "deduplicated"
        );
        assert!(!m.char_sets.is_empty());
        assert_eq!(m.type_predicate.as_deref(), Some(T));
        assert!(m.class_hierarchy.iter().any(|n| n.class == "<Person>"
            && n.parents == vec!["<Agent>".to_string()]
            && n.depth == 1));
        // the abstract level rolls Person + Org up to Agent
        let top = &m.level_rollups[0];
        assert_eq!(top.round, 0);
        assert_eq!(top.classes, vec![("<Agent>".to_string(), 3)]);
        // the leaf links: Person -knows-> Person, Person -worksFor-> Org, Person -name-> literal
        let leaf = m.level_links.last().unwrap();
        let has = |s: &str, p: &str, o: &str, n: u64| {
            leaf.links
                .iter()
                .any(|l| l.s_class == s && l.predicate == p && l.o_class == o && l.count == n)
        };
        assert!(has("<Person>", "<knows>", "<Person>", 1));
        assert!(has("<Person>", "<worksFor>", "<Org>", 1));
        assert!(has("<Person>", "<name>", "(literal)", 3));
        // round trip through the section bytes
        let bytes = m.encode();
        assert_eq!(SchemaMeta::decode(&bytes).unwrap(), m);
        let mut bad = bytes.clone();
        bad[20] ^= 0xff;
        assert!(SchemaMeta::decode(&bad).is_err(), "digest catches a flip");
    }

    #[test]
    fn untyped_graph_still_gets_stats() {
        let (dict, mut ids) = build(&[("<a>", "<p>", "<b>"), ("<b>", "<p>", "\"x\"")]);
        let m = compute_in_memory(&dict, &mut ids, None);
        assert_eq!(m.predicate_stats.len(), 1);
        assert!(m.class_hierarchy.is_empty() && m.level_rollups.is_empty());
        assert_eq!(m.type_predicate, None);
        assert!(!m.encode().is_empty());
    }

    #[test]
    fn literal_ranges_are_found_by_binary_search() {
        let (dict, _) = build(&[
            ("<a>", "<p>", "\"1\""),
            ("<a>", "<p>", "\"2\""),
            ("<a>", "<p>", "<x>"),
            ("<a>", "<p>", "_:b"),
            ("<x>", "<p>", "<a>"),
        ]);
        let ids = SchemaIds::from_dictionary(&dict, None);
        let lits: Vec<u32> = (1..=dict.shared_count() + dict.object_only_count())
            .filter(|&o| ids.is_literal(o))
            .collect();
        let expect: Vec<u32> = (1..=dict.shared_count() + dict.object_only_count())
            .filter(|&o| dict.object_term(o).unwrap().starts_with('"'))
            .collect();
        assert_eq!(lits, expect);
        assert_eq!(lits.len(), 2);
    }
}
