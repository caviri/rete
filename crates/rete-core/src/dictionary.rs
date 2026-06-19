//! Dictionary assembly: classify RDF terms by role and assign the HDT-style
//! global IDs that triple blocks reference (SPEC.md §5, §5.1).
//!
//! Four sections are built from the §5.1 front-coded primitive:
//! `shared` (used as both subject and object), `subjects` (subject-only),
//! `objects` (object-only), and `predicates`. Subject IDs run `1..=S` (shared)
//! then `S+1..` (subject-only); object IDs reuse `1..=S` for the same shared
//! terms then `S+1..` for object-only. Predicates have their own space.

use std::collections::HashSet;

use crate::dict::{ChunkedSection, DictSectionBuilder};
use crate::terms::{NodeId, ObjectId, PredicateId, SubjectId};

/// Sort a term slice ascending — in parallel when the `parallel` feature is on.
/// The front-coded dictionary sections need ascending order; doing it once here
/// (rather than per-insert in a `BTreeSet`) is the fast path for large graphs.
#[cfg(feature = "parallel")]
fn sort_terms(v: &mut [String]) {
    use rayon::slice::ParallelSliceMut;
    v.par_sort_unstable();
}
#[cfg(not(feature = "parallel"))]
fn sort_terms(v: &mut [String]) {
    v.sort_unstable();
}

/// Builds a [`Dictionary`] from observed `(subject, predicate, object)` terms.
///
/// Terms are deduped with `HashSet` during ingest (O(1) average insert, no tree
/// rebalancing) and sorted **once** in [`Self::build`]. This replaces a trio of
/// `BTreeSet<String>` whose per-insert `O(log n)` cost + a `String` allocation
/// for *every* observed term (most of them duplicates) dominated the build on
/// large graphs.
#[derive(Default)]
pub struct DictionaryBuilder {
    subjects: HashSet<String>,
    objects: HashSet<String>,
    predicates: HashSet<String>,
}

impl DictionaryBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one triple's terms (IDs are assigned later, in [`Self::build`]).
    /// We probe with the borrowed `&str` first and only allocate a `String` for
    /// a genuinely new term — so a graph of N triples does far fewer than 3N
    /// allocations (each subject/predicate recurs across many triples).
    pub fn observe(&mut self, subject: &str, predicate: &str, object: &str) {
        if !self.subjects.contains(subject) {
            self.subjects.insert(subject.to_string());
        }
        if !self.objects.contains(object) {
            self.objects.insert(object.to_string());
        }
        if !self.predicates.contains(predicate) {
            self.predicates.insert(predicate.to_string());
        }
    }

    pub fn build(self) -> Dictionary {
        // Sort each role's terms once (ascending) — identical output order to
        // the old BTreeSet iteration, but without the per-insert tree cost.
        let mut subjects: Vec<String> = self.subjects.into_iter().collect();
        let mut predicates: Vec<String> = self.predicates.into_iter().collect();
        let object_set = self.objects; // kept for the shared (subject ∩ object) test
        sort_terms(&mut subjects);
        sort_terms(&mut predicates);

        // A term is "shared" when it appears as both a subject and an object.
        let subject_set: HashSet<&str> = subjects.iter().map(String::as_str).collect();

        let mut shared_b = DictSectionBuilder::new();
        let mut subj_b = DictSectionBuilder::new();
        let mut obj_b = DictSectionBuilder::new();
        let mut pred_b = DictSectionBuilder::new();

        // Subjects ascending: shared (also an object) vs subject-only.
        for s in &subjects {
            if object_set.contains(s) {
                shared_b.push(s.clone());
            } else {
                subj_b.push(s.clone());
            }
        }
        // Objects ascending: object-only (shared terms were emitted above).
        let mut objects: Vec<String> = object_set.into_iter().collect();
        sort_terms(&mut objects);
        for o in &objects {
            if !subject_set.contains(o.as_str()) {
                obj_b.push(o.clone());
            }
        }
        for p in &predicates {
            pred_b.push(p.clone());
        }

        let shared = shared_b.build();
        Dictionary::from_sections([shared, subj_b.build(), obj_b.build(), pred_b.build()])
    }
}

/// A read-only dictionary mapping terms ↔ role-specific IDs.
///
/// Holds the four sections as [`ChunkedSection`]s — local files keep each
/// section as one resident chunk; a lazily-opened remote file faults
/// individual chunks in on first touch. Lookups never re-parse a header.
pub struct Dictionary {
    /// shared, subjects, objects, predicates.
    sections: [ChunkedSection; 4],
    shared_len: u32,
}

impl Dictionary {
    /// Rebuild from four serialized sections (shared, subjects, objects,
    /// predicates), e.g. when reading a `.rete` file.
    pub fn from_sections(sections: [Vec<u8>; 4]) -> Self {
        Self::from_chunked_sections(sections.map(ChunkedSection::local))
    }

    /// Rebuild from four already-chunked sections (the remote lazy-open path).
    pub fn from_chunked_sections(sections: [ChunkedSection; 4]) -> Self {
        let shared_len = sections[0].term_count();
        Dictionary {
            sections,
            shared_len,
        }
    }

    /// Total number of distinct terms across all four sections.
    pub fn term_count(&self) -> u32 {
        self.sections.iter().map(|s| s.term_count()).sum()
    }

    /// Did any lazy chunk fetch fail since this dictionary was opened?
    pub fn load_incomplete(&self) -> bool {
        self.sections.iter().any(|s| s.load_incomplete())
    }

    /// Batch-fault every unloaded chunk of every section (no-op for local
    /// dictionaries). Callers about to resolve *every* term — export, dump —
    /// call this once so the sweep coalesces into a few range reads instead
    /// of one fetch per chunk.
    pub fn prefetch_all(&self) {
        for s in &self.sections {
            s.prefetch_all();
        }
    }

    /// Batch-fault just the chunks needed to resolve a bounded result set:
    /// `node_ids` are unified-node ids (the `Val::Id(id)` with `id >= 0`),
    /// `predicate_ids` are predicate-space ids. Groups them by `(section,
    /// chunk)` and coalesces each section's faults into a few range reads —
    /// turning "one request per distinct output term" into "a handful per
    /// query". No-op for a local dictionary (every chunk is already resident).
    pub fn prefetch_terms(&self, node_ids: &[u32], predicate_ids: &[u32]) {
        let mut want: [std::collections::BTreeSet<usize>; 4] = Default::default();
        for &n in node_ids {
            if let Some((si, ci)) = self.node_chunk(n) {
                want[si].insert(ci);
            }
        }
        for &p in predicate_ids {
            if let Some(ci) = self.sections[3].chunk_of_id(p) {
                want[3].insert(ci);
            }
        }
        for (si, set) in want.iter().enumerate() {
            if set.len() >= 2 {
                let cis: Vec<usize> = set.iter().copied().collect();
                self.sections[si].prefetch_chunks(&cis);
            }
        }
    }

    /// The `(section, chunk)` holding a unified node id — mirrors the section
    /// routing in [`node_term`](Self::node_term).
    fn node_chunk(&self, node: u32) -> Option<(usize, usize)> {
        let su = self.subject_only_count();
        let (si, local) = if node < self.shared_len + su {
            let id = node + 1; // subject_term(id)
            if id <= self.shared_len {
                (0, id)
            } else {
                (1, id - self.shared_len)
            }
        } else {
            let id = node + 1 - su; // object_term(id)
            if id <= self.shared_len {
                (0, id)
            } else {
                (2, id - self.shared_len)
            }
        };
        Some((si, self.sections[si].chunk_of_id(local)?))
    }

    /// Number of shared terms `S`.
    pub fn shared_count(&self) -> u32 {
        self.shared_len
    }

    /// Number of subject-only terms `Su`.
    pub fn subject_only_count(&self) -> u32 {
        self.sections[1].term_count()
    }

    /// Number of object-only terms `Oo`.
    pub fn object_only_count(&self) -> u32 {
        self.sections[2].term_count()
    }

    // --- unified node space (for graph algorithms / the pyramid) ----------
    //
    // Subjects and objects use *separate* ID spaces that overlap on shared
    // terms; the pyramid needs one node per distinct term. We lay them out as:
    //   [0 .. S)            shared
    //   [S .. S+Su)         subject-only
    //   [S+Su .. S+Su+Oo)   object-only

    /// Total distinct graph nodes `N = S + Su + Oo`.
    pub fn node_count(&self) -> u32 {
        self.shared_len + self.subject_only_count() + self.object_only_count()
    }

    /// Unified node ID for a subject-role ID. Shared and subject-only IDs are
    /// already contiguous (`1..=S+Su`), so this is just `sid - 1`.
    pub fn subject_node(&self, sid: SubjectId) -> NodeId {
        // saturating_sub: a corrupt block may carry id 0; valid ids are ≥1 so this
        // is identical for well-formed files but never underflow-panics.
        sid.saturating_sub(1)
    }

    /// Unified node ID for an object-role ID: shared stay at `oid-1`, object-only
    /// IDs are shifted past the subject-only block.
    pub fn object_node(&self, oid: ObjectId) -> NodeId {
        if oid <= self.shared_len {
            oid.saturating_sub(1)
        } else {
            oid.saturating_sub(1) + self.subject_only_count()
        }
    }

    /// Resolve a unified node ID back to its term.
    pub fn node_term(&self, node: NodeId) -> Option<String> {
        let su = self.subject_only_count();
        if node < self.shared_len + su {
            self.subject_term(node + 1)
        } else {
            self.object_term(node + 1 - su)
        }
    }

    /// Unified node ID for a term, resolving via subject role then object role.
    pub fn node_of_term(&self, term: &str) -> Option<NodeId> {
        if let Some(sid) = self.subject_id(term) {
            return Some(self.subject_node(sid));
        }
        self.object_id(term).map(|oid| self.object_node(oid))
    }

    /// Subject-role ID of a node, or `None` if the node never appears as a
    /// subject (an object-only term).
    pub fn node_as_subject_id(&self, node: NodeId) -> Option<SubjectId> {
        if node < self.shared_len + self.subject_only_count() {
            Some(node + 1)
        } else {
            None
        }
    }

    /// Object-role ID of a node, or `None` if the node never appears as an
    /// object (a subject-only term).
    pub fn node_as_object_id(&self, node: NodeId) -> Option<ObjectId> {
        let (s, su) = (self.shared_len, self.subject_only_count());
        if node < s {
            Some(node + 1) // shared
        } else if node >= s + su {
            Some(node + 1 - su) // object-only
        } else {
            None // subject-only
        }
    }

    // --- term -> id -------------------------------------------------------

    /// Subject-role ID for `term`: shared `1..=S`, else subject-only `S+1..`.
    pub fn subject_id(&self, term: &str) -> Option<SubjectId> {
        if let Some(id) = self.sections[0].id(term) {
            return Some(id);
        }
        self.sections[1].id(term).map(|id| self.shared_len + id)
    }

    /// Object-role ID for `term`: shared `1..=S`, else object-only `S+1..`.
    pub fn object_id(&self, term: &str) -> Option<ObjectId> {
        if let Some(id) = self.sections[0].id(term) {
            return Some(id);
        }
        self.sections[2].id(term).map(|id| self.shared_len + id)
    }

    /// Predicate ID for `term` (independent space).
    pub fn predicate_id(&self, term: &str) -> Option<PredicateId> {
        self.sections[3].id(term)
    }

    // --- id -> term -------------------------------------------------------

    pub fn subject_term(&self, id: SubjectId) -> Option<String> {
        if id <= self.shared_len {
            self.sections[0].term(id)
        } else {
            self.sections[1].term(id - self.shared_len)
        }
    }

    pub fn object_term(&self, id: ObjectId) -> Option<String> {
        if id <= self.shared_len {
            self.sections[0].term(id)
        } else {
            self.sections[2].term(id - self.shared_len)
        }
    }

    pub fn predicate_term(&self, id: PredicateId) -> Option<String> {
        self.sections[3].term(id)
    }

    /// Encode a `(s, p, o)` term triple to its `(subject_id, predicate_id,
    /// object_id)`. `None` if any term is unknown.
    pub fn encode(&self, s: &str, p: &str, o: &str) -> Option<(SubjectId, PredicateId, ObjectId)> {
        Some((
            self.subject_id(s)?,
            self.predicate_id(p)?,
            self.object_id(o)?,
        ))
    }

    /// The four serialized sections (header + body each), for the file writer.
    pub fn sections(&self) -> [Vec<u8>; 4] {
        [
            self.sections[0].raw_section_bytes(),
            self.sections[1].raw_section_bytes(),
            self.sections[2].raw_section_bytes(),
            self.sections[3].raw_section_bytes(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict() -> Dictionary {
        let mut b = DictionaryBuilder::new();
        // Alice knows Bob; Bob knows Carol. So Bob is both subject and object
        // (shared); Alice is subject-only; Carol is object-only.
        b.observe("Alice", "knows", "Bob");
        b.observe("Bob", "knows", "Carol");
        b.build()
    }

    #[test]
    fn shared_term_has_same_id_in_both_roles() {
        let d = dict();
        assert_eq!(d.shared_count(), 1); // just "Bob"
        let sid = d.subject_id("Bob").unwrap();
        let oid = d.object_id("Bob").unwrap();
        assert_eq!(sid, oid, "shared term must share its ID across roles");
        assert_eq!(sid, 1, "shared terms get the lowest IDs");
    }

    #[test]
    fn role_specific_terms_round_trip() {
        let d = dict();
        // subject-only Alice
        let a = d.subject_id("Alice").unwrap();
        assert!(a > d.shared_count());
        assert_eq!(d.subject_term(a).as_deref(), Some("Alice"));
        // object-only Carol
        let c = d.object_id("Carol").unwrap();
        assert!(c > d.shared_count());
        assert_eq!(d.object_term(c).as_deref(), Some("Carol"));
        // predicate
        let k = d.predicate_id("knows").unwrap();
        assert_eq!(d.predicate_term(k).as_deref(), Some("knows"));
        // Alice never appears as object, Carol never as subject.
        assert_eq!(d.object_id("Alice"), None);
        assert_eq!(d.subject_id("Carol"), None);
    }

    #[test]
    fn encode_full_triples() {
        let d = dict();
        let t1 = d.encode("Alice", "knows", "Bob").unwrap();
        let t2 = d.encode("Bob", "knows", "Carol").unwrap();
        // Bob is object in t1 and subject in t2 — same ID both times.
        assert_eq!(t1.2, t2.0);
        assert!(d.encode("Nobody", "knows", "Bob").is_none());
    }
}
