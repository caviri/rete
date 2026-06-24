//! Build the **schema pyramid** — the v2 `PyramidMeta` payload (SPEC.md §7.3/7.4):
//! the non-exclusive `subClassOf` DAG with per-class depth, the per-level type
//! rollups (semantic zoom: abstract classes at coarse levels, leaves at fine),
//! the per-level lateral class-relation graph (the non-`is-a` connections), and
//! the optional per-community descriptors (Phase 4 progressive refinement).
//!
//! The rollup function *is* the ontology: each instance's class is folded up its
//! `subClassOf` chain to the depth matched to a level. With no hierarchy in the
//! data, every class is a depth-0 root and the pyramid degrades to one flat
//! histogram (= the card's class list).

use std::collections::BTreeMap;

use crate::dictionary::Dictionary;
use crate::meta::{ClassNode, ClassRelation, CommunityDescriptor, LevelLinks, LevelRollup};
use crate::pyramid::{Dendrogram, Partition};
use crate::RDF_TYPE;

const RDFS_SUBCLASS_OF: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
const OWL_DISJOINT_WITH: &str = "<http://www.w3.org/2002/07/owl#disjointWith>";
const OWL_EQUIVALENT_CLASS: &str = "<http://www.w3.org/2002/07/owl#equivalentClass>";
const WGS_LAT: &str = "<http://www.w3.org/2003/01/geo/wgs84_pos#lat>";
const WGS_LONG: &str = "<http://www.w3.org/2003/01/geo/wgs84_pos#long>";

/// Max semantic-zoom levels shipped (Q2 in the design: a small fixed N).
const MAX_LEVELS: usize = 6;
/// Cap on classes per rollup and on shipped hierarchy nodes / descriptors.
const TOP_CLASSES: usize = 64;
/// Cap on class-relation edges per level (the lateral connections).
const TOP_LINKS: usize = 128;
const MAX_HIERARCHY: usize = 512;
const TOP_DESCRIPTORS: usize = 256;
/// Cap on the parent-chain walk — a backstop against a cyclic `subClassOf`.
const MAX_CHAIN: u16 = 64;

/// The computed schema pyramid: the (non-exclusive) `subClassOf` DAG, the
/// per-level type rollups, the per-level lateral class-relation graph, and the
/// per-community descriptors. All empty when the graph has no `rdf:type`.
#[derive(Debug, Clone, Default)]
pub struct SchemaPyramid {
    pub class_hierarchy: Vec<ClassNode>,
    pub level_rollups: Vec<LevelRollup>,
    pub level_links: Vec<LevelLinks>,
    pub descriptors: Vec<CommunityDescriptor>,
    // --- v2.1 coherence axioms (the T-Box a remote reader checks index-free) ---
    /// `subClassOf` cycles: every strongly-connected component of size > 1 in the
    /// child→parent graph, plus every self-loop (`C ⊑ C`). Each a sorted class set.
    pub subclass_cycles: Vec<Vec<String>>,
    /// `owl:disjointWith` class pairs, canonicalized `(min, max)`, deduped + sorted.
    pub disjoint_pairs: Vec<(String, String)>,
    /// `owl:equivalentClass` class pairs, canonicalized `(min, max)`, deduped + sorted.
    pub equivalent_pairs: Vec<(String, String)>,
}

/// The predicate that types subjects with classes:
/// 1. an explicit `type_override` when given (e.g. Wikidata's `wdt:P31`, where
///    `rdf:type` is only the structural `schema:Dataset`/`wikibase:Item`);
/// 2. else `rdf:type` whenever it is actually used — the canonical typing;
/// 3. else the predicate that behaves like "instance of" — IRI-class objects each
///    reused by many subjects, covering most of the graph (not a label/located-in)
///    — so a graph typed purely with a custom predicate still gets a pyramid.
fn pick_type_predicate(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    type_override: Option<&str>,
) -> Option<u32> {
    if let Some(tp) = type_override {
        return dict.predicate_id(tp);
    }
    if let Some(pid) = dict.predicate_id(RDF_TYPE) {
        if triples.iter().any(|&(_, p, _)| p == pid) {
            return Some(pid);
        }
    }
    use std::collections::{HashMap, HashSet};
    let total_subjects: HashSet<u32> = triples.iter().map(|&(s, _, _)| s).collect();
    if total_subjects.is_empty() {
        return None;
    }
    let mut subs: HashMap<u32, HashSet<u32>> = HashMap::new();
    let mut objs: HashMap<u32, HashSet<u32>> = HashMap::new();
    for &(s, p, o) in triples {
        // The object must be an IRI (a class), not a literal (`"…"`).
        if dict
            .object_term(o)
            .map(|t| !t.starts_with('"'))
            .unwrap_or(false)
        {
            subs.entry(p).or_default().insert(s);
            objs.entry(p).or_default().insert(o);
        }
    }
    let mut best: Option<(u32, usize)> = None;
    for (&p, ss) in &subs {
        let no = objs.get(&p).map(|o| o.len()).unwrap_or(0);
        if no == 0 {
            continue;
        }
        // Strong "instance of" signal: classes reused a lot AND most subjects typed.
        if ss.len() / no >= 8
            && ss.len() * 2 >= total_subjects.len()
            && best.is_none_or(|(_, c)| ss.len() > c)
        {
            best = Some((p, ss.len()));
        }
    }
    best.map(|(p, _)| p)
}

/// Build a flat, **type-based** dendrogram for `--pyramid-algo types`: each node's
/// community is the class of the subject it represents (first `rdf:type` wins —
/// the same representative class the descriptors use). Untyped or object-only
/// nodes (including literals) fall into one «untyped» bucket. A single level, so
/// round 0 is the leaf-class partition; the existing summarizer then yields the
/// class→class quotient graph, and the schema pyramid's `subClassOf` rollups
/// supply the coarser semantic levels.
///
/// This is the **parallelizable** pyramid: deterministic and a single linear pass
/// (no Louvain), so a graph far too large for the single-threaded modularity build
/// still gets a pyramid + the cost-based planner's `query_stats`. Communities are
/// self-naming — a community *is* a class.
///
/// Returns `None` when the graph has no usable type predicate or no subject is
/// typed, so the caller falls back to [`crate::pyramid::PyramidAlgo::Louvain`].
pub fn build_type_dendrogram(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    type_override: Option<&str>,
) -> Option<Dendrogram> {
    use std::collections::{BTreeMap, BTreeSet, HashMap};

    let type_pid = pick_type_predicate(dict, triples, type_override)?;

    // Representative class (object id) per subject id — first `rdf:type` wins, the
    // same rule `build_schema_pyramid_with` uses for `subject_class`, so a
    // community's class equals its descriptor's `dominant_class`.
    let mut class_of_subject: HashMap<u32, u32> = HashMap::new();
    let mut classes: BTreeSet<u32> = BTreeSet::new();
    for &(s, p, o) in triples {
        if p == type_pid {
            class_of_subject.entry(s).or_insert(o);
            classes.insert(o);
        }
    }
    if classes.is_empty() {
        return None;
    }

    // Deterministic class → raw community id (ascending class object-id); the
    // «untyped» bucket — every node that is not a typed subject — gets the next id.
    let class_comm: HashMap<u32, usize> =
        classes.iter().enumerate().map(|(i, &c)| (c, i)).collect();
    let untyped = classes.len();

    let n = dict.node_count() as usize;
    let mut comm = vec![untyped; n];
    for (&sid, &cid) in &class_of_subject {
        comm[dict.subject_node(sid) as usize] = class_comm[&cid];
    }

    // Densify to `0..count` (drops the «untyped» bucket when unused) per the
    // `Partition` contract. The remap walks `comm` in node order, so the numbering
    // is independent of the HashMap iteration order above — fully reproducible.
    let mut remap: BTreeMap<usize, usize> = BTreeMap::new();
    for c in &mut comm {
        let next = remap.len();
        *c = *remap.entry(*c).or_insert(next);
    }
    Some(Dendrogram {
        levels: vec![Partition {
            comm,
            count: remap.len(),
        }],
    })
}

/// Compute the schema pyramid for a graph at the materialized `round`, auto-picking
/// the type predicate. Empty when the graph has no usable typing.
pub fn build_schema_pyramid(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    round: usize,
) -> SchemaPyramid {
    build_schema_pyramid_with(dict, triples, dend, round, None)
}

/// Like [`build_schema_pyramid`], but `type_override` forces the type predicate
/// (e.g. `wdt:P31`) instead of the auto-detection.
pub fn build_schema_pyramid_with(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    round: usize,
    type_override: Option<&str>,
) -> SchemaPyramid {
    let type_pid = match pick_type_predicate(dict, triples, type_override) {
        Some(p) => p,
        None => return SchemaPyramid::default(),
    };
    let subclass_pid = dict.predicate_id(RDFS_SUBCLASS_OF);
    let lat_pid = dict.predicate_id(WGS_LAT);
    let long_pid = dict.predicate_id(WGS_LONG);

    // --- instance counts + per-subject class (descriptors) + IRI→class (relations) ---
    let mut instance_counts: BTreeMap<String, u64> = BTreeMap::new();
    let mut subject_class: BTreeMap<u32, String> = BTreeMap::new();
    let mut class_of_iri: BTreeMap<String, String> = BTreeMap::new();
    for &(s, p, o) in triples {
        if p == type_pid {
            if let Some(class) = dict.object_term(o) {
                *instance_counts.entry(class.clone()).or_default() += 1;
                // First type wins as the subject's representative class.
                subject_class.entry(s).or_insert_with(|| class.clone());
                if let Some(iri) = dict.subject_term(s) {
                    class_of_iri.entry(iri).or_insert(class);
                }
            }
        }
    }
    if instance_counts.is_empty() {
        return SchemaPyramid::default();
    }

    // --- subClassOf edges: keep ALL parents (a non-exclusive DAG) ---
    let mut parents: BTreeMap<String, std::collections::BTreeSet<String>> = BTreeMap::new();
    let mut self_loops: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Some(sc_pid) = subclass_pid {
        for &(s, p, o) in triples {
            if p == sc_pid {
                if let (Some(child), Some(parent)) = (dict.subject_term(s), dict.object_term(o)) {
                    if child != parent {
                        parents.entry(child).or_default().insert(parent);
                    } else {
                        // `C ⊑ C` is dropped from the DAG (depth/rollup) but is a
                        // degenerate cycle worth reporting on its own.
                        self_loops.insert(child);
                    }
                }
            }
        }
    }

    // --- coherence axioms (v2.1): disjointWith / equivalentClass pairs + cycles ---
    // Canonicalized (min, max) pairs, deduped — the index-free T-Box that lets a
    // remote reader flag unsatisfiable classes without touching the triple index.
    let pair_set = |pid: Option<u32>| -> Vec<(String, String)> {
        let mut set: std::collections::BTreeSet<(String, String)> =
            std::collections::BTreeSet::new();
        if let Some(pid) = pid {
            for &(s, p, o) in triples {
                if p == pid {
                    if let (Some(a), Some(b)) = (dict.subject_term(s), dict.object_term(o)) {
                        if a != b {
                            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
                            set.insert((lo, hi));
                        }
                    }
                }
            }
        }
        set.into_iter().collect()
    };
    let disjoint_pairs = pair_set(dict.predicate_id(OWL_DISJOINT_WITH));
    let equivalent_pairs = pair_set(dict.predicate_id(OWL_EQUIVALENT_CLASS));
    let subclass_cycles = subclass_cycles_of(&parents, &self_loops);
    // Canonical single parent (smallest) for the deterministic depth/rollup tree.
    let canonical: BTreeMap<String, String> = parents
        .iter()
        .filter_map(|(c, ps)| ps.iter().next().map(|p| (c.clone(), p.clone())))
        .collect();

    // All classes: instantiated ∪ every class named in subClassOf.
    let mut classes: std::collections::BTreeSet<String> = instance_counts.keys().cloned().collect();
    for (c, ps) in &parents {
        classes.insert(c.clone());
        for p in ps {
            classes.insert(p.clone());
        }
    }

    let depth: BTreeMap<String, u16> = classes
        .iter()
        .map(|c| (c.clone(), depth_of(c, &canonical)))
        .collect();
    let max_depth = depth.values().copied().max().unwrap_or(0);

    // --- class hierarchy (non-exclusive: all parents kept; sorted, capped) ---
    let mut class_hierarchy: Vec<ClassNode> = classes
        .iter()
        .map(|c| ClassNode {
            class: c.clone(),
            parents: parents
                .get(c)
                .map(|ps| ps.iter().cloned().collect())
                .unwrap_or_default(),
            depth: depth.get(c).copied().unwrap_or(0),
        })
        .collect();
    class_hierarchy.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.class.cmp(&b.class)));
    class_hierarchy.truncate(MAX_HIERARCHY);

    // --- leaf class-relation graph (the lateral, non-is-a connections) ---
    // (s_class, predicate, o_class) over the non-type triples, the same quotient
    // as `schema_summary`, with the `(literal)`/`(untyped)` object sentinels.
    let classify_obj = |term: &str| -> String {
        if term.starts_with('"') {
            "(literal)".to_string()
        } else {
            class_of_iri
                .get(term)
                .cloned()
                .unwrap_or_else(|| "(untyped)".to_string())
        }
    };
    let mut leaf_links: BTreeMap<(String, String, String), u64> = BTreeMap::new();
    for &(s, p, o) in triples {
        // Skip the schema-defining predicates — `rdf:type` and `rdfs:subClassOf`
        // are the hierarchy itself, not lateral data relations.
        if p == type_pid || Some(p) == subclass_pid {
            continue;
        }
        let (s_iri, o_term, pred) = match (
            dict.subject_term(s),
            dict.object_term(o),
            dict.predicate_term(p),
        ) {
            (Some(a), Some(b), Some(c)) => (a, b, c),
            _ => continue,
        };
        let sc = class_of_iri
            .get(&s_iri)
            .cloned()
            .unwrap_or_else(|| "(untyped)".to_string());
        let oc = classify_obj(&o_term);
        *leaf_links.entry((sc, pred, oc)).or_default() += 1;
    }

    // --- per-level rollups: type histogram + lateral relation graph at each depth ---
    let n_levels = (max_depth as usize + 1).clamp(1, MAX_LEVELS);
    let dend_rounds = dend.rounds();
    let mut level_rollups = Vec::with_capacity(n_levels);
    let mut level_links = Vec::with_capacity(n_levels);
    for i in 0..n_levels {
        // Level 0 = the most abstract (depth 0); the last level = leaves.
        let target_depth = if n_levels == 1 {
            0
        } else {
            ((i * max_depth as usize) / (n_levels - 1)) as u16
        };
        // Align level i with a dendrogram round. Convention (pyramid.rs): round 0
        // is the FINEST grouping, the last round the COARSEST — so the abstract
        // level i=0 maps to the highest (coarsest) round. Informational only.
        let round_align = if dend_rounds > 1 && n_levels > 1 {
            (((n_levels - 1 - i) * (dend_rounds - 1)) / (n_levels - 1)) as u32
        } else {
            round as u32
        };

        // Type histogram rolled up to this depth.
        let mut roll: BTreeMap<String, u64> = BTreeMap::new();
        for (class, &count) in &instance_counts {
            *roll
                .entry(ancestor_at(class, target_depth, &depth, &canonical))
                .or_default() += count;
        }
        let mut hist: Vec<(String, u64)> = roll.into_iter().collect();
        hist.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        hist.truncate(TOP_CLASSES);
        level_rollups.push(LevelRollup {
            round: round_align,
            depth: target_depth,
            classes: hist,
        });

        // Lateral relations rolled up to this depth (sentinels pass through).
        let mut rel: BTreeMap<(String, String, String), u64> = BTreeMap::new();
        for ((sc, pred, oc), &count) in &leaf_links {
            let sa = ancestor_at(sc, target_depth, &depth, &canonical);
            let oa = ancestor_at(oc, target_depth, &depth, &canonical);
            *rel.entry((sa, pred.clone(), oa)).or_default() += count;
        }
        let mut links: Vec<ClassRelation> = rel
            .into_iter()
            .map(|((s, p, o), count)| ClassRelation {
                s_class: s,
                predicate: p,
                o_class: o,
                count,
            })
            .collect();
        links.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.s_class.cmp(&b.s_class))
                .then_with(|| a.predicate.cmp(&b.predicate))
                .then_with(|| a.o_class.cmp(&b.o_class))
        });
        links.truncate(TOP_LINKS);
        level_links.push(LevelLinks {
            round: round_align,
            depth: target_depth,
            links,
        });
    }

    let descriptors = build_descriptors(
        dict,
        triples,
        dend,
        round,
        &subject_class,
        lat_pid,
        long_pid,
        type_pid,
    );

    SchemaPyramid {
        class_hierarchy,
        level_rollups,
        level_links,
        descriptors,
        subclass_cycles,
        disjoint_pairs,
        equivalent_pairs,
    }
}

/// Find `subClassOf` cycles: every strongly-connected component of size > 1 in
/// the child→parent graph (iterative Tarjan), plus every self-loop. Output is
/// deterministic — members sorted within a component, components sorted, deduped,
/// capped at `TOP_CLASSES`.
fn subclass_cycles_of(
    parents: &BTreeMap<String, std::collections::BTreeSet<String>>,
    self_loops: &std::collections::BTreeSet<String>,
) -> Vec<Vec<String>> {
    // Node universe = every class that appears as a child or a parent.
    let mut node_set: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for (c, ps) in parents {
        node_set.insert(c.as_str());
        for p in ps {
            node_set.insert(p.as_str());
        }
    }
    let nodes: Vec<&str> = node_set.into_iter().collect();
    let idx_of: BTreeMap<&str, usize> = nodes.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let adj: Vec<Vec<usize>> = nodes
        .iter()
        .map(|&n| {
            let mut v: Vec<usize> = parents
                .get(n)
                .map(|ps| ps.iter().map(|p| idx_of[p.as_str()]).collect())
                .unwrap_or_default();
            v.sort_unstable();
            v
        })
        .collect();

    let n = nodes.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut next = 0usize;
    let mut comps: Vec<Vec<String>> = Vec::new();

    for start in 0..n {
        if index[start] != usize::MAX {
            continue;
        }
        // Iterative DFS: a call stack of (node, next-child-position).
        index[start] = next;
        low[start] = next;
        next += 1;
        stack.push(start);
        on_stack[start] = true;
        let mut call: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(v, ci)) = call.last() {
            if ci < adj[v].len() {
                call.last_mut().unwrap().1 += 1;
                let w = adj[v][ci];
                if index[w] == usize::MAX {
                    index[w] = next;
                    low[w] = next;
                    next += 1;
                    stack.push(w);
                    on_stack[w] = true;
                    call.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                if low[v] == index[v] {
                    let mut comp: Vec<String> = Vec::new();
                    loop {
                        let w = stack.pop().unwrap();
                        on_stack[w] = false;
                        comp.push(nodes[w].to_string());
                        if w == v {
                            break;
                        }
                    }
                    if comp.len() > 1 {
                        comp.sort();
                        comps.push(comp);
                    }
                }
                call.pop();
                if let Some(&(parent, _)) = call.last() {
                    low[parent] = low[parent].min(low[v]);
                }
            }
        }
    }

    for s in self_loops {
        comps.push(vec![s.clone()]);
    }
    comps.sort();
    comps.dedup();
    comps.truncate(TOP_CLASSES);
    comps
}

/// Depth of a class = steps up its canonical-parent chain to a root, with a
/// cycle/length backstop.
fn depth_of(class: &str, parent: &BTreeMap<String, String>) -> u16 {
    let mut d = 0u16;
    let mut cur = class.to_string();
    let mut seen = std::collections::BTreeSet::new();
    loop {
        if !seen.insert(cur.clone()) || d >= MAX_CHAIN {
            break;
        }
        match parent.get(&cur) {
            Some(p) if p != &cur => {
                d = d.saturating_add(1);
                cur = p.clone();
            }
            _ => break,
        }
    }
    d
}

/// The ancestor of `class` at `target` depth (walk up until depth ≤ target).
fn ancestor_at(
    class: &str,
    target: u16,
    depth: &BTreeMap<String, u16>,
    parent: &BTreeMap<String, String>,
) -> String {
    let mut cur = class.to_string();
    let mut steps = 0u16;
    while depth.get(&cur).copied().unwrap_or(0) > target && steps < MAX_CHAIN {
        match parent.get(&cur) {
            Some(p) if p != &cur => cur = p.clone(),
            _ => break,
        }
        steps += 1;
    }
    cur
}

#[allow(clippy::too_many_arguments)]
fn build_descriptors(
    dict: &Dictionary,
    triples: &[(u32, u32, u32)],
    dend: &Dendrogram,
    round: usize,
    subject_class: &BTreeMap<u32, String>,
    lat_pid: Option<u32>,
    long_pid: Option<u32>,
    type_pid: u32,
) -> Vec<CommunityDescriptor> {
    let comm_of = |sid: u32| -> usize {
        if dend.rounds() == 0 {
            0
        } else {
            dend.base_community(dict.subject_node(sid) as usize, round)
        }
    };

    #[derive(Default)]
    struct Acc {
        class_counts: BTreeMap<String, u64>,
        members: u64,
        lat: Option<(f64, f64)>,
        lon: Option<(f64, f64)>,
        time: Option<(String, String)>,
    }
    let mut acc: BTreeMap<usize, Acc> = BTreeMap::new();

    // Type triples → class counts per community.
    for (&sid, class) in subject_class {
        let a = acc.entry(comm_of(sid)).or_default();
        *a.class_counts.entry(class.clone()).or_default() += 1;
        a.members += 1;
    }
    // wgs84 + temporal extents per community.
    for &(s, p, o) in triples {
        if p == type_pid {
            continue;
        }
        let term = match dict.object_term(o) {
            Some(t) => t,
            None => continue,
        };
        if Some(p) == lat_pid {
            if let Some(v) = literal_f64(&term) {
                let a = acc.entry(comm_of(s)).or_default();
                a.lat = Some(merge_min_max(a.lat, v));
            }
        } else if Some(p) == long_pid {
            if let Some(v) = literal_f64(&term) {
                let a = acc.entry(comm_of(s)).or_default();
                a.lon = Some(merge_min_max(a.lon, v));
            }
        } else if let Some(val) = literal_temporal(&term) {
            let a = acc.entry(comm_of(s)).or_default();
            a.time = Some(match a.time.take() {
                Some((lo, hi)) => (
                    if val < lo.as_str() {
                        val.to_string()
                    } else {
                        lo
                    },
                    if val > hi.as_str() {
                        val.to_string()
                    } else {
                        hi
                    },
                ),
                None => (val.to_string(), val.to_string()),
            });
        }
    }

    let mut out: Vec<CommunityDescriptor> = acc
        .into_iter()
        .filter(|(_, a)| a.members > 0)
        .map(|(community, a)| {
            let dominant_class = a
                .class_counts
                .iter()
                .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
                .map(|(c, _)| c.clone());
            // Keep the full per-community histogram (no truncation) so a
            // descriptor's class_counts sum exactly equals its typed-member count
            // — distinct classes within one community are naturally few, and the
            // descriptor *count* is already capped (TOP_DESCRIPTORS).
            let mut class_counts: Vec<(String, u64)> = a.class_counts.into_iter().collect();
            class_counts.sort_by(|x, y| y.1.cmp(&x.1).then_with(|| x.0.cmp(&y.0)));
            let bbox = match (a.lat, a.lon) {
                (Some((min_lat, max_lat)), Some((min_lon, max_lon))) => {
                    Some([min_lon, min_lat, max_lon, max_lat])
                }
                _ => None,
            };
            CommunityDescriptor {
                community: community as u32,
                dominant_class,
                class_counts,
                bbox,
                time_range: a.time,
            }
        })
        .collect();

    // Keep the largest communities (most worth zooming into), then order by id.
    out.sort_by(|a, b| {
        let am: u64 = a.class_counts.iter().map(|(_, c)| c).sum();
        let bm: u64 = b.class_counts.iter().map(|(_, c)| c).sum();
        bm.cmp(&am).then_with(|| a.community.cmp(&b.community))
    });
    out.truncate(TOP_DESCRIPTORS);
    out.sort_by_key(|d| d.community);
    out
}

fn merge_min_max(cur: Option<(f64, f64)>, v: f64) -> (f64, f64) {
    match cur {
        Some((lo, hi)) => (lo.min(v), hi.max(v)),
        None => (v, v),
    }
}

/// Numeric value of an RDF literal term (`"41.9"` / `"41.9"^^<…double>`).
fn literal_f64(term: &str) -> Option<f64> {
    literal_value(term)?.parse::<f64>().ok()
}

/// The lexical value of a date/year-typed (or year-shaped) literal, for a
/// community's temporal extent. `None` for non-temporal terms.
fn literal_temporal(term: &str) -> Option<&str> {
    let val = literal_value(term)?;
    let is_date_dt = term.contains("XMLSchema#date")
        || term.contains("XMLSchema#dateTime")
        || term.contains("XMLSchema#gYear");
    let is_year_shaped = {
        let head = val.strip_prefix('-').unwrap_or(val);
        head.len() >= 4 && head.as_bytes()[..4].iter().all(u8::is_ascii_digit)
    };
    if is_date_dt || is_year_shaped {
        Some(val)
    } else {
        None
    }
}

/// The value between the opening quote and the first **unescaped** closing quote
/// of a literal term, or `None` if the term is not a literal. Escape-aware so a
/// `\"` inside the value is not mistaken for the terminator (the returned slice
/// keeps the lexical escapes — adequate for numeric parsing / year detection).
fn literal_value(term: &str) -> Option<&str> {
    let bytes = term.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut i = 1;
    let mut esc = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => esc = !esc,
            b'"' if !esc => return Some(&term[1..i]),
            _ => esc = false,
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::DictionaryBuilder;
    use crate::pyramid::{build_dendrogram, project_graph};

    const TYPE: &str = RDF_TYPE;
    const SUB: &str = RDFS_SUBCLASS_OF;

    fn build(triples: &[(&str, &str, &str)]) -> (Dictionary, Vec<(u32, u32, u32)>, Dendrogram) {
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let ids: Vec<_> = triples
            .iter()
            .map(|(s, p, o)| dict.encode(s, p, o).unwrap())
            .collect();
        let g = project_graph(&dict, &ids);
        let dend = build_dendrogram(&g);
        (dict, ids, dend)
    }

    #[test]
    fn type_dendrogram_partitions_by_class() {
        // Two same-class subjects + one of a different class, each rdf:type'd.
        let triples = vec![
            ("<a>", TYPE, "<Concept>"),
            ("<b>", TYPE, "<Concept>"),
            ("<r>", TYPE, "<Relation>"),
            ("<r>", "<cause>", "<a>"),
            ("<r>", "<effect>", "<b>"),
            ("<a>", "<label>", "\"a\""), // a literal object → «untyped» node, must not panic
        ];
        let (dict, ids, _louvain) = build(&triples);
        let dend = build_type_dendrogram(&dict, &ids, None).expect("typed graph yields a dendrogram");
        assert_eq!(dend.rounds(), 1, "flat single-level type partition");
        let comm = |t: &str| {
            let sid = dict.subject_id(t).unwrap();
            dend.base_community(dict.subject_node(sid) as usize, 0)
        };
        // Both Concepts share a community; the Relation is a different one.
        assert_eq!(comm("<a>"), comm("<b>"), "same class → same community");
        assert_ne!(comm("<a>"), comm("<r>"), "different class → different community");

        // The descriptor's dominant_class equals the community's class — the
        // self-naming property the type pyramid is built for.
        let descriptors = build_schema_pyramid(&dict, &ids, &dend, 0).descriptors;
        let dom = |c: u32| {
            descriptors
                .iter()
                .find(|d| d.community == c)
                .and_then(|d| d.dominant_class.clone())
        };
        assert_eq!(dom(comm("<a>") as u32).as_deref(), Some("<Concept>"));
        assert_eq!(dom(comm("<r>") as u32).as_deref(), Some("<Relation>"));
    }

    #[test]
    fn type_dendrogram_none_without_types() {
        // No rdf:type and no "instance of"-like predicate → None, so the caller
        // (build_pyramid_meta_algo) falls back to Louvain.
        let triples = vec![("<a>", "<knows>", "<b>"), ("<b>", "<knows>", "<c>")];
        let (dict, ids, _l) = build(&triples);
        assert!(build_type_dendrogram(&dict, &ids, None).is_none());
    }

    #[test]
    fn detects_custom_type_predicate_when_no_rdf_type() {
        // No rdf:type: subjects are typed via a custom "instance of" predicate the
        // way Wikidata uses wdt:P31 — each class reused by many subjects.
        let mut owned: Vec<(String, String, String)> = Vec::new();
        for i in 0..20 {
            owned.push((format!("<e{i}>"), "<P31>".into(), "<Human>".into()));
        }
        for i in 20..32 {
            owned.push((format!("<e{i}>"), "<P31>".into(), "<City>".into()));
        }
        // A label predicate (literal objects, 1:1) must NOT be mistaken for typing.
        for i in 0..32 {
            owned.push((format!("<e{i}>"), "<label>".into(), format!("\"name{i}\"")));
        }
        let refs: Vec<(&str, &str, &str)> = owned
            .iter()
            .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
            .collect();
        let (dict, ids, dend) = build(&refs);
        let sp = build_schema_pyramid(&dict, &ids, &dend, 0);
        let fine = sp
            .level_rollups
            .last()
            .expect("custom type predicate should yield a schema pyramid");
        let cls: std::collections::HashMap<&str, u64> =
            fine.classes.iter().map(|(c, n)| (c.as_str(), *n)).collect();
        assert_eq!(cls.get("<Human>"), Some(&20));
        assert_eq!(cls.get("<City>"), Some(&12));
        // The label predicate is not a class set.
        assert!(!cls.contains_key("<label>"));
    }

    #[test]
    fn coarse_levels_are_ancestors_of_fine_levels() {
        // Astronomer ⊑ Scientist ⊑ Person ⊑ Agent, with instances at the leaf.
        let triples = vec![
            ("<Scientist>", SUB, "<Person>"),
            ("<Person>", SUB, "<Agent>"),
            ("<Astronomer>", SUB, "<Scientist>"),
            ("<a>", TYPE, "<Astronomer>"),
            ("<b>", TYPE, "<Astronomer>"),
            ("<c>", TYPE, "<Person>"),
            ("<a>", "<knows>", "<b>"),
        ];
        let (dict, ids, dend) = build(&triples);
        let sp = build_schema_pyramid(&dict, &ids, &dend, 0);
        let hierarchy = sp.class_hierarchy;
        let rollups = sp.level_rollups;

        // Depths: Agent 0, Person 1, Scientist 2, Astronomer 3.
        let depth = |c: &str| hierarchy.iter().find(|n| n.class == c).map(|n| n.depth);
        assert_eq!(depth("<Agent>"), Some(0));
        assert_eq!(depth("<Person>"), Some(1));
        assert_eq!(depth("<Astronomer>"), Some(3));

        assert!(rollups.len() >= 2, "multi-level pyramid");
        // Coarsest level (index 0, depth 0) rolls everything up to Agent.
        let coarse = &rollups[0];
        assert_eq!(coarse.depth, 0);
        assert!(coarse
            .classes
            .iter()
            .any(|(c, n)| c == "<Agent>" && *n == 3));
        // The finest level resolves the leaf Astronomer.
        let fine = rollups.last().unwrap();
        assert!(fine.depth >= coarse.depth);
        assert!(fine.classes.iter().any(|(c, _)| c == "<Astronomer>"));
        // Every coarse class is an ancestor (or equal) of some fine class.
        for (cc, _) in &coarse.classes {
            assert!(
                fine.classes
                    .iter()
                    .any(|(fc, _)| is_ancestor_or_equal(cc, fc, &hierarchy)),
                "coarse class {cc} should be an ancestor of a fine class"
            );
        }
        // Round alignment: the abstract level maps to a coarser (>=) dendrogram
        // round than the leaf level (round 0 is finest; the last round coarsest).
        assert!(
            rollups[0].round >= rollups.last().unwrap().round,
            "abstract level should align with a coarser (>=) round"
        );
    }

    #[test]
    fn non_exclusive_hierarchy_keeps_all_parents() {
        // Astronaut is BOTH a Scientist and an Explorer (multiple inheritance).
        let triples = vec![
            ("<Scientist>", SUB, "<Person>"),
            ("<Explorer>", SUB, "<Person>"),
            ("<Astronaut>", SUB, "<Scientist>"),
            ("<Astronaut>", SUB, "<Explorer>"),
            ("<x>", TYPE, "<Astronaut>"),
            ("<y>", TYPE, "<Astronaut>"),
            ("<x>", "<knows>", "<y>"),
        ];
        let (dict, ids, dend) = build(&triples);
        let sp = build_schema_pyramid(&dict, &ids, &dend, 0);
        let astro = sp
            .class_hierarchy
            .iter()
            .find(|n| n.class == "<Astronaut>")
            .unwrap();
        // BOTH parents survive — the hierarchy is a non-exclusive DAG, not a tree.
        assert_eq!(
            astro.parents,
            vec!["<Explorer>".to_string(), "<Scientist>".to_string()]
        );
        // The canonical (smallest) parent drives the deterministic rollup tree.
        assert_eq!(
            astro.canonical_parent().map(String::as_str),
            Some("<Explorer>")
        );
    }

    #[test]
    fn level_links_roll_relations_up_the_hierarchy() {
        // Person --memberOf--> Organisation, both under Agent.
        let triples = vec![
            ("<Person>", SUB, "<Agent>"),
            ("<Organisation>", SUB, "<Agent>"),
            ("<ada>", TYPE, "<Person>"),
            ("<bob>", TYPE, "<Person>"),
            ("<nasa>", TYPE, "<Organisation>"),
            ("<ada>", "<memberOf>", "<nasa>"),
            ("<bob>", "<memberOf>", "<nasa>"),
        ];
        let (dict, ids, dend) = build(&triples);
        let sp = build_schema_pyramid(&dict, &ids, &dend, 0);
        assert!(!sp.level_links.is_empty(), "lateral relations present");

        // Leaf level: the concrete relation Person --memberOf--> Organisation ×2.
        let leaf = sp.level_links.last().unwrap();
        assert!(leaf.links.iter().any(|r| r.s_class == "<Person>"
            && r.predicate == "<memberOf>"
            && r.o_class == "<Organisation>"
            && r.count == 2));
        // Coarsest level: the SAME relation rolled up to Agent --memberOf--> Agent ×2.
        let coarse = &sp.level_links[0];
        assert_eq!(coarse.depth, 0);
        assert!(coarse.links.iter().any(|r| r.s_class == "<Agent>"
            && r.predicate == "<memberOf>"
            && r.o_class == "<Agent>"
            && r.count == 2));
    }

    #[test]
    fn per_descriptor_counts_sum_to_community_members() {
        // Two typed triangles, each subject a distinct class, bridged — so the
        // graph forms multiple communities with several classes each. Each
        // descriptor's class_counts must sum exactly to that community's typed
        // members (the no-truncation invariant), checked against an independent
        // recomputation of community membership.
        let names = ["A", "B", "C", "D", "E", "F"];
        let edges = [
            ("A", "B"),
            ("B", "C"),
            ("A", "C"),
            ("D", "E"),
            ("E", "F"),
            ("D", "F"),
            ("C", "D"),
        ];
        let mut triples: Vec<(String, String, String)> = names
            .iter()
            .map(|n| (format!("<{n}>"), TYPE.to_string(), format!("<T{n}>")))
            .collect();
        for (s, o) in edges {
            triples.push((format!("<{s}>"), "<knows>".into(), format!("<{o}>")));
        }
        let refs: Vec<(&str, &str, &str)> = triples
            .iter()
            .map(|(s, p, o)| (s.as_str(), p.as_str(), o.as_str()))
            .collect();
        let (dict, ids, dend) = build(&refs);
        let descriptors = build_schema_pyramid(&dict, &ids, &dend, 0).descriptors;

        // Independently recompute typed members per community.
        let type_pid = dict.predicate_id(RDF_TYPE).unwrap();
        let comm_of = |sid: u32| -> u32 {
            if dend.rounds() == 0 {
                0
            } else {
                dend.base_community(dict.subject_node(sid) as usize, 0) as u32
            }
        };
        let mut expected: BTreeMap<u32, u64> = BTreeMap::new();
        let mut seen = std::collections::BTreeSet::new();
        for &(s, p, _o) in &ids {
            if p == type_pid && seen.insert(s) {
                *expected.entry(comm_of(s)).or_default() += 1;
            }
        }
        assert!(!descriptors.is_empty());
        for d in &descriptors {
            let sum: u64 = d.class_counts.iter().map(|(_, c)| c).sum();
            assert_eq!(
                sum, expected[&d.community],
                "descriptor {} counts must sum to its members",
                d.community
            );
        }
        // All 6 typed subjects accounted for across descriptors.
        let total: u64 = descriptors
            .iter()
            .flat_map(|d| d.class_counts.iter().map(|(_, c)| *c))
            .sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn literal_value_handles_escaped_quotes() {
        // The closing quote is the first UNescaped one.
        assert_eq!(literal_value(r#""1850""#), Some("1850"));
        assert_eq!(literal_value(r#""a\"b""#), Some(r#"a\"b"#));
        assert_eq!(
            literal_value(r#""1850"^^<http://www.w3.org/2001/XMLSchema#gYear>"#),
            Some("1850")
        );
        assert_eq!(literal_value("<http://ex/x>"), None);
        // A year-shaped temporal value is detected after correct boundary parsing.
        assert_eq!(literal_temporal(r#""1850-01-02""#), Some("1850-01-02"));
        assert_eq!(literal_temporal(r#""hello""#), None);
    }

    fn is_ancestor_or_equal(anc: &str, c: &str, h: &[ClassNode]) -> bool {
        let mut cur = c.to_string();
        for _ in 0..64 {
            if cur == anc {
                return true;
            }
            match h
                .iter()
                .find(|n| n.class == cur)
                .and_then(|n| n.canonical_parent().cloned())
            {
                Some(p) => cur = p,
                None => return false,
            }
        }
        false
    }

    #[test]
    fn no_hierarchy_degrades_to_flat_histogram() {
        let triples = vec![
            ("<a>", TYPE, "<Person>"),
            ("<b>", TYPE, "<Person>"),
            ("<c>", TYPE, "<City>"),
            ("<a>", "<knows>", "<b>"),
        ];
        let (dict, ids, dend) = build(&triples);
        let sp = build_schema_pyramid(&dict, &ids, &dend, 0);
        let hierarchy = sp.class_hierarchy;
        let rollups = sp.level_rollups;
        // All classes are depth-0 roots; exactly one (flat) level.
        assert!(hierarchy
            .iter()
            .all(|n| n.depth == 0 && n.parents.is_empty()));
        assert_eq!(rollups.len(), 1, "flat = one depth-0 level");
        let flat = &rollups[0];
        assert_eq!(flat.depth, 0);
        assert!(flat.classes.contains(&("<Person>".to_string(), 2)));
        assert!(flat.classes.contains(&("<City>".to_string(), 1)));
    }

    #[test]
    fn descriptor_class_counts_sum_to_typed_subjects() {
        let triples = vec![
            ("<a>", TYPE, "<Person>"),
            ("<b>", TYPE, "<Person>"),
            (
                "<a>",
                WGS_LAT,
                "\"41.9\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            (
                "<a>",
                WGS_LONG,
                "\"12.5\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            (
                "<b>",
                WGS_LAT,
                "\"45.0\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            (
                "<b>",
                WGS_LONG,
                "\"9.0\"^^<http://www.w3.org/2001/XMLSchema#double>",
            ),
            (
                "<a>",
                "<born>",
                "\"1850\"^^<http://www.w3.org/2001/XMLSchema#gYear>",
            ),
            (
                "<b>",
                "<born>",
                "\"1875\"^^<http://www.w3.org/2001/XMLSchema#gYear>",
            ),
            ("<a>", "<knows>", "<b>"),
        ];
        let (dict, ids, dend) = build(&triples);
        let descriptors = build_schema_pyramid(&dict, &ids, &dend, 0).descriptors;
        assert!(!descriptors.is_empty());
        let total: u64 = descriptors
            .iter()
            .flat_map(|d| d.class_counts.iter().map(|(_, c)| *c))
            .sum();
        assert_eq!(total, 2, "every typed subject counted once");

        // Per-community bboxes (a and b may land in different communities); each
        // box must be valid, and their union must cover both points (lon/lat).
        let boxes: Vec<[f64; 4]> = descriptors.iter().filter_map(|d| d.bbox).collect();
        assert!(!boxes.is_empty(), "geometry descriptors present");
        for b in &boxes {
            assert!(b[0] <= b[2] && b[1] <= b[3], "valid box {b:?}");
        }
        let gmin_lon = boxes.iter().map(|b| b[0]).fold(f64::INFINITY, f64::min);
        let gmax_lon = boxes.iter().map(|b| b[2]).fold(f64::NEG_INFINITY, f64::max);
        let gmin_lat = boxes.iter().map(|b| b[1]).fold(f64::INFINITY, f64::min);
        let gmax_lat = boxes.iter().map(|b| b[3]).fold(f64::NEG_INFINITY, f64::max);
        assert!(
            gmin_lon <= 9.0 && gmax_lon >= 12.5,
            "lon union covers points"
        );
        assert!(
            gmin_lat <= 41.9 && gmax_lat >= 45.0,
            "lat union covers points"
        );

        // Temporal extents: each valid, union spanning the actual years.
        let times: Vec<(String, String)> = descriptors
            .iter()
            .filter_map(|d| d.time_range.clone())
            .collect();
        assert!(!times.is_empty(), "temporal descriptors present");
        let gfrom = times.iter().map(|(f, _)| f.as_str()).min().unwrap();
        let gto = times.iter().map(|(_, t)| t.as_str()).max().unwrap();
        assert!(gfrom <= "1850" && gto >= "1875", "time union covers years");
    }
}
