//! Dictionary assembly: classify RDF terms by role and assign the HDT-style
//! global IDs that triple blocks reference (SPEC.md §5, §5.1).
//!
//! Four sections are built from the §5.1 front-coded primitive:
//! `shared` (used as both subject and object), `subjects` (subject-only),
//! `objects` (object-only), and `predicates`. Subject IDs run `1..=S` (shared)
//! then `S+1..` (subject-only); object IDs reuse `1..=S` for the same shared
//! terms then `S+1..` for object-only. Predicates have their own space.

use std::collections::{HashMap, HashSet};

use crate::chunk_cache::ChunkCache;
use crate::dict::{ChunkedSection, DictSectionBuilder, MultiChunkBulkLoader};
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
    /// Frequent-object promotion ([`Self::with_frequent_objects`]): how many of
    /// the most frequent object-only terms move into the shared section.
    promote: usize,
    /// Object occurrence counts, kept **instead of** `objects` while promotion
    /// is on (so a default build pays nothing for it).
    object_counts: HashMap<String, u64>,
    /// Count objects without promoting any ([`Self::with_object_counts`]).
    count_objects: bool,
    /// Entries in the three term sets — a term once per set it is in — and
    /// their string bytes ([`Self::footprint`]).
    entries: u64,
    entry_bytes: u64,
}

impl DictionaryBuilder {
    /// A builder with the process-wide writer settings
    /// ([`crate::write_opts::frequent_objects`]).
    pub fn new() -> Self {
        Self::default().with_frequent_objects(crate::write_opts::frequent_objects())
    }

    /// Promote the `n` most frequent **object-only** terms (by the number of
    /// statements that carry them as object; ties by term order) into the
    /// shared section (`0` = off, the default).
    ///
    /// The shared section is numbered first and in term order, so a promoted
    /// vocabulary term (a class, a scheme individual, a status literal) gets a
    /// small object id wherever it sorts ahead of the shared entities — the
    /// effect declaring the vocabulary in a TBox has, without the TBox. A
    /// promoted term keeps its object id role; the subject id it also gets is
    /// simply never used by any statement. Readers need nothing new: the shared
    /// section has always been looked up first for both roles.
    ///
    /// Set it before the first [`Self::observe`]: promotion counts objects as
    /// they are observed.
    pub fn with_frequent_objects(mut self, n: usize) -> Self {
        debug_assert!(
            self.objects.is_empty() && self.object_counts.is_empty(),
            "with_frequent_objects after observe"
        );
        self.promote = n;
        self
    }

    /// Count every object's occurrences, as promotion does, but promote
    /// nothing: [`Self::build_counted`] hands the counts of the object-only
    /// terms back instead. The memory-bounded build's chunks use it, because
    /// a term's frequency is only known once every chunk's counts are summed
    /// (`crate::extbuild`).
    // Only the memory-bounded build (`crate::extbuild`, not on wasm32) uses it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn with_object_counts(mut self) -> Self {
        debug_assert!(
            self.objects.is_empty() && self.object_counts.is_empty(),
            "with_object_counts after observe"
        );
        self.count_objects = true;
        self
    }

    /// Whether objects are kept with their counts rather than as a set.
    fn counting(&self) -> bool {
        self.promote > 0 || self.count_objects
    }

    /// `(entries, bytes)`: the entries of the three term sets observed so far
    /// — a term once for every set it is in (subject, object, predicate) — and
    /// the bytes of their strings. What the memory-bounded build charges a
    /// chunk's dictionary against its budget, since [`Self::build`] costs a
    /// multiple of each entry.
    // Only the memory-bounded build (`crate::extbuild`, not on wasm32) uses it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn footprint(&self) -> (u64, u64) {
        (self.entries, self.entry_bytes)
    }

    fn note_entry(&mut self, term: &str) {
        self.entries += 1;
        self.entry_bytes += term.len() as u64;
    }

    /// Record one triple's terms (IDs are assigned later, in [`Self::build`]).
    /// We probe with the borrowed `&str` first and only allocate a `String` for
    /// a genuinely new term — so a graph of N triples does far fewer than 3N
    /// allocations (each subject/predicate recurs across many triples).
    pub fn observe(&mut self, subject: &str, predicate: &str, object: &str) {
        if !self.subjects.contains(subject) {
            self.subjects.insert(subject.to_string());
            self.note_entry(subject);
        }
        if self.counting() {
            if let Some(n) = self.object_counts.get_mut(object) {
                *n += 1;
            } else {
                self.object_counts.insert(object.to_string(), 1);
                self.note_entry(object);
            }
        } else if !self.objects.contains(object) {
            self.objects.insert(object.to_string());
            self.note_entry(object);
        }
        if !self.predicates.contains(predicate) {
            self.predicates.insert(predicate.to_string());
            self.note_entry(predicate);
        }
    }

    /// Record a **graph name** as a term in the subject role — how the quad
    /// index (SPEC.md §6.5) makes graph names dictionary ids. It lands in the
    /// shared section when the name is also an object, else in the
    /// subject-only section.
    pub fn observe_graph(&mut self, graph: &str) {
        if !self.subjects.contains(graph) {
            self.subjects.insert(graph.to_string());
            self.note_entry(graph);
        }
    }

    pub fn build(self) -> Dictionary {
        self.build_inner().0
    }

    /// [`Self::build`] for a builder made [`Self::with_object_counts`]: also
    /// returns, for every object-only term in the order of the object-only
    /// section (local ids `1..`), the number of statements that carry it.
    /// Empty when objects were not counted.
    // Only the memory-bounded build (`crate::extbuild`, not on wasm32) uses it.
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    pub(crate) fn build_counted(self) -> (Dictionary, Vec<u64>) {
        self.build_inner()
    }

    fn build_inner(self) -> (Dictionary, Vec<u64>) {
        // Sort each role's terms once (ascending) — identical output order to
        // the old BTreeSet iteration, but without the per-insert tree cost.
        let mut subjects: Vec<String> = self.subjects.into_iter().collect();
        let mut predicates: Vec<String> = self.predicates.into_iter().collect();
        sort_terms(&mut subjects);
        sort_terms(&mut predicates);

        // A term is "shared" when it appears as both a subject and an object.
        let subject_set: HashSet<&str> = subjects.iter().map(String::as_str).collect();

        // The object terms: a plain set, or (promotion on) the count map, used
        // as-is rather than converted, so no second table over every object.
        let count_objects = self.count_objects;
        let object_set = if self.promote > 0 || count_objects {
            ObjectTerms::Counted(self.object_counts)
        } else {
            ObjectTerms::Plain(self.objects)
        };
        // Frequent-object promotion: the top `promote` object-only terms by
        // statement count (ties by term order) join the shared section.
        let mut promoted: Vec<String> = Vec::new();
        if let (true, ObjectTerms::Counted(counts)) = (self.promote > 0, &object_set) {
            let mut top: Vec<(&String, u64)> = counts
                .iter()
                .filter(|(t, _)| !subject_set.contains(t.as_str()))
                .map(|(t, &n)| (t, n))
                .collect();
            let k = self.promote.min(top.len());
            if k > 0 && k < top.len() {
                top.select_nth_unstable_by(k - 1, |a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            }
            top.truncate(k);
            promoted = top.into_iter().map(|(t, _)| t.clone()).collect();
            sort_terms(&mut promoted);
        }
        let promoted_set: HashSet<&str> = promoted.iter().map(String::as_str).collect();

        let mut shared_b = DictSectionBuilder::new();
        let mut subj_b = DictSectionBuilder::new();
        let mut obj_b = DictSectionBuilder::new();
        let mut pred_b = DictSectionBuilder::new();

        // Subjects ascending: shared (also an object) vs subject-only. Promoted
        // objects are merged into the shared run in term order.
        let mut promoted_iter = promoted.iter().peekable();
        for s in &subjects {
            if object_set.contains(s) {
                while let Some(p) = promoted_iter.next_if(|p| p.as_str() < s.as_str()) {
                    shared_b.push(p.clone());
                }
                shared_b.push(s.clone());
            } else {
                subj_b.push(s.clone());
            }
        }
        for p in promoted_iter {
            shared_b.push(p.clone());
        }
        // Objects ascending: object-only (shared terms were emitted above),
        // with their counts when the caller asked for them.
        // RDF-star: a quoted triple only ever appears in subject/object position,
        // so a single prefix scan of those term sets detects the whole file.
        let mut has_quoted_triples = subjects.iter().any(|s| s.starts_with("<<"));
        let mut object_only_counts: Vec<u64> = Vec::new();
        let mut take_object = |o: &String, n: u64| {
            has_quoted_triples |= o.starts_with("<<");
            if !subject_set.contains(o.as_str()) && !promoted_set.contains(o.as_str()) {
                obj_b.push(o.clone());
                if count_objects {
                    object_only_counts.push(n);
                }
            }
        };
        match object_set {
            ObjectTerms::Counted(counts) if count_objects => {
                let mut objects: Vec<(String, u64)> = counts.into_iter().collect();
                sort_counted_terms(&mut objects);
                for (o, n) in &objects {
                    take_object(o, *n);
                }
            }
            object_set => {
                let mut objects: Vec<String> = object_set.into_terms();
                sort_terms(&mut objects);
                for o in &objects {
                    take_object(o, 0);
                }
            }
        }
        for p in &predicates {
            pred_b.push(p.clone());
        }

        let shared = shared_b.build();
        let mut dict =
            Dictionary::from_sections([shared, subj_b.build(), obj_b.build(), pred_b.build()]);
        dict.has_quoted_triples = has_quoted_triples;
        (dict, object_only_counts)
    }
}

/// Sort `(term, count)` pairs by term — the terms are distinct, so the
/// count never decides. In parallel when the `parallel` feature is on.
fn sort_counted_terms(v: &mut [(String, u64)]) {
    #[cfg(feature = "parallel")]
    {
        use rayon::slice::ParallelSliceMut;
        v.par_sort_unstable_by(|a, b| a.0.cmp(&b.0));
    }
    #[cfg(not(feature = "parallel"))]
    v.sort_unstable_by(|a, b| a.0.cmp(&b.0));
}

/// The object terms a [`DictionaryBuilder`] observed: a set, or the
/// occurrence counts frequent-object promotion needs.
enum ObjectTerms {
    Plain(HashSet<String>),
    Counted(HashMap<String, u64>),
}

impl ObjectTerms {
    fn contains(&self, term: &str) -> bool {
        match self {
            ObjectTerms::Plain(s) => s.contains(term),
            ObjectTerms::Counted(m) => m.contains_key(term),
        }
    }

    fn into_terms(self) -> Vec<String> {
        match self {
            ObjectTerms::Plain(s) => s.into_iter().collect(),
            ObjectTerms::Counted(m) => m.into_keys().collect(),
        }
    }
}

/// Reusable per-window scratch for [`Dictionary::resolve_window`]: one job list
/// per section (`(local_id, out_slot)` pairs) and the resolved-term output.
/// Held across windows by the export loop so a window allocates only when a
/// buffer must grow.
#[derive(Default)]
pub struct WindowResolver {
    /// Jobs per dictionary section (shared, subject-only, object-only,
    /// predicates): each `(section-local id, output slot)`.
    jobs: [Vec<(u32, usize)>; 4],
    /// Resolved terms, indexed by output slot `row * 3 + {0:s, 1:p, 2:o}`.
    out: Vec<Option<String>>,
}

/// The most chunks one id-list prefetch ([`Dictionary::prefetch_subject_terms`],
/// [`Dictionary::prefetch_node_terms`]) faults in a single batch. A batch holds
/// every body it fetched until all are cached, so this bounds that transient
/// (512 chunks of ≤64 KiB: ≤32 MiB) however many ids a caller passes; a longer
/// list goes out in several batches.
const PREFETCH_BATCH_CHUNKS: usize = 512;

/// A read-only dictionary mapping terms ↔ role-specific IDs.
///
/// Holds the four sections as [`ChunkedSection`]s — local files keep each
/// section as one resident chunk; a lazily-opened remote file faults
/// individual chunks in on first touch. Lookups never re-parse a header.
pub struct Dictionary {
    /// shared, subjects, objects, predicates.
    sections: [ChunkedSection; 4],
    shared_len: u32,
    /// Whether any term is an RDF-star quoted triple (`<< … >>`). Set at build
    /// time (from the observed terms); `false` on the read path, where the
    /// header's `FLAG_HAS_QUOTED_TRIPLES` is the source of truth instead.
    has_quoted_triples: bool,
    /// Faults the chunks of several sections in one batch (a ranged open);
    /// `None` = section by section (local, or no batch reader).
    multi_bulk: Option<MultiChunkBulkLoader>,
    /// The literal object ids ([`Self::literal_objects`]), once found.
    literal_objects: std::sync::OnceLock<LiteralObjects>,
}

/// Which object ids are literals. Literal tokens (`"…`) sort before IRIs
/// (`<…`), blank nodes (`_:…`) and quoted triples (`<<…`) in every dictionary
/// section, so the literals of a section are one run of consecutive ids: the
/// shared section's are normally none (a literal is never a subject), the
/// object-only section's come first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LiteralObjects {
    /// Shared object ids `shared.0+1..=shared.1` are literals.
    pub shared: (u32, u32),
    /// `S`, the shared-term count: object ids above it are object-only.
    pub n_shared: u32,
    /// Object-only object ids `S+only.0+1..=S+only.1` are literals.
    pub only: (u32, u32),
}

impl LiteralObjects {
    /// Is every object id in `lo..=hi` a literal?
    pub fn covers(&self, lo: u32, hi: u32) -> bool {
        let within = |(a, b): (u64, u64)| lo as u64 > a && hi as u64 <= b;
        lo <= hi
            && (within((self.shared.0 as u64, self.shared.1 as u64))
                || within((
                    self.n_shared as u64 + self.only.0 as u64,
                    self.n_shared as u64 + self.only.1 as u64,
                )))
    }

    /// Is object id `o` a literal?
    pub fn contains(&self, o: u32) -> bool {
        self.covers(o, o)
    }
}

impl Dictionary {
    /// Rebuild from four serialized sections (shared, subjects, objects,
    /// predicates), e.g. when reading a `.rete` file. The four sections share one
    /// [`ChunkCache`] (unlimited cap here, so every section body stays resident
    /// exactly as before).
    pub fn from_sections(sections: [Vec<u8>; 4]) -> Self {
        let cache = ChunkCache::unlimited_arc();
        let mut section_index = 0u8;
        let chunked = sections.map(|bytes| {
            let s = ChunkedSection::local(bytes, cache.clone(), section_index);
            section_index += 1;
            s
        });
        Self::from_chunked_sections(chunked)
    }

    /// Rebuild from four already-chunked sections (the remote lazy-open path).
    pub fn from_chunked_sections(sections: [ChunkedSection; 4]) -> Self {
        let shared_len = sections[0].term_count();
        Dictionary {
            sections,
            shared_len,
            has_quoted_triples: false,
            multi_bulk: None,
            literal_objects: std::sync::OnceLock::new(),
        }
    }

    /// The object ids that are literals (see [`LiteralObjects`]): two binary
    /// searches per section for where the `"` tokens start and end, each in the
    /// one chunk the boundary routes to — two chunk faults at most on a lazy
    /// open, done once. `None` when a chunk cannot be read, which a caller
    /// treats as "not proven".
    pub fn literal_objects(&self) -> Option<LiteralObjects> {
        if let Some(found) = self.literal_objects.get() {
            return Some(*found);
        }
        // `"` is 0x22 and `#` the next byte: a literal token sorts in
        // [`"`, `#`), and nothing a well-formed file holds sorts before it.
        // A failed read is not cached, so the next query tries again.
        let run = |si: usize| -> Option<(u32, u32)> {
            Some((self.sections[si].rank("\"")?, self.sections[si].rank("#")?))
        };
        let found = LiteralObjects {
            shared: run(0)?,
            n_shared: self.shared_len,
            only: run(2)?,
        };
        Some(*self.literal_objects.get_or_init(|| found))
    }

    /// Attach a cross-section chunk fetcher (see [`MultiChunkBulkLoader`]):
    /// the term prefetchers then fault every section's missing chunks in one
    /// batch instead of one batch per section.
    pub(crate) fn with_multi_bulk_loader(mut self, loader: MultiChunkBulkLoader) -> Self {
        self.multi_bulk = Some(loader);
        self
    }

    /// Fault the chunks `want[si]` of each section, at most `batch` chunks
    /// per read. With a cross-section fetcher every missing chunk goes out in
    /// one batch (round trips: one, not one per section); without one, each
    /// section batches its own. A failed batch leaves the chunks for the
    /// per-chunk loader, which retries them lookup by lookup. Purely a cache
    /// warmer: results never depend on it.
    fn prefetch_sections(&self, want: &[Vec<usize>; 4], batch: usize) {
        let batch = batch.max(2);
        if let Some(multi) = &self.multi_bulk {
            let pairs: Vec<(u8, usize)> = want
                .iter()
                .enumerate()
                .flat_map(|(si, cis)| {
                    self.sections[si]
                        .missing_chunks(cis)
                        .into_iter()
                        .map(move |ci| (si as u8, ci))
                })
                .collect();
            // A single missing chunk costs the same either way: leave it to
            // the per-chunk loader, as a section's own batch does.
            if pairs.len() < 2 {
                return;
            }
            for part in pairs.chunks(batch) {
                if let Some(bodies) = multi(part) {
                    if bodies.len() == part.len() {
                        for (&(si, ci), body) in part.iter().zip(bodies) {
                            self.sections[si as usize].seed_chunk(ci, body);
                        }
                    }
                }
            }
            return;
        }
        for (si, cis) in want.iter().enumerate() {
            for part in cis.chunks(batch) {
                self.sections[si].prefetch_chunks(part);
            }
        }
    }

    /// Total number of distinct terms across all four sections.
    pub fn term_count(&self) -> u32 {
        self.sections.iter().map(|s| s.term_count()).sum()
    }

    /// Set the byte cap of the shared chunk cache (bounded-export phase 2).
    /// `u64::MAX` (the default from every open) means unlimited — no eviction,
    /// residency identical to the pre-budget design. A finite cap evicts
    /// least-recently-used chunk bodies down to it on every fault, so a full
    /// dump's peak dictionary residency is bounded rather than the whole
    /// decompressed dictionary. All four sections share one cache; setting it on
    /// each is idempotent (they are the same `Arc`).
    pub fn set_cache_cap(&self, cap: u64) {
        for s in &self.sections {
            s.cache().set_cap(cap);
        }
    }

    /// Heap bytes the four sections' chunk directories hold for as long as
    /// the dictionary is open (see [`ChunkedSection::directory_bytes`]).
    ///
    /// [`ChunkedSection::directory_bytes`]: crate::dict::ChunkedSection::directory_bytes
    pub fn directory_bytes(&self) -> u64 {
        self.sections.iter().map(|s| s.directory_bytes()).sum()
    }

    /// The current chunk-cache byte cap (`u64::MAX` = unlimited).
    pub fn cache_cap(&self) -> u64 {
        self.sections[0].cache().cap()
    }

    /// A snapshot of the shared chunk cache's observability counters — for
    /// `RETE_OPEN_DEBUG` accounting and the eviction tests.
    pub fn cache_stats(&self) -> crate::chunk_cache::CacheStats {
        self.sections[0].cache().stats()
    }

    /// Does this dataset contain RDF-star quoted triples? Meaningful only for a
    /// freshly built dictionary (a read-back one carries the truth in the file
    /// header's `FLAG_HAS_QUOTED_TRIPLES`, not here).
    pub fn has_quoted_triples(&self) -> bool {
        self.has_quoted_triples
    }

    /// Did any lazy chunk fetch fail since this dictionary was opened — or
    /// since the last [`reset_load_failure`](Self::reset_load_failure)?
    pub fn load_incomplete(&self) -> bool {
        self.sections.iter().any(|s| s.load_incomplete())
    }

    /// Forget recorded fetch failures across every section (see
    /// `GraphIndex::reset_load_failure` — the per-query reset for resident
    /// sessions). Failed chunks were never cached, so they simply retry.
    pub fn reset_load_failure(&self) {
        self.sections.iter().for_each(|s| s.reset_load_failure());
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
        self.prefetch_terms_batched(node_ids, predicate_ids, usize::MAX);
    }

    /// [`prefetch_terms`](Self::prefetch_terms), faulting at most `batch` chunks
    /// per bulk read. A bulk read holds every chunk it fetches — the coalesced
    /// span, the per-chunk slices and the decoded bodies — until all of them
    /// are cached, so an unbatched prefetch of a scattered window materialized
    /// up to thousands of chunks at once, outside every cache cap. A bounded
    /// export batches it so that transient stays a few MB.
    pub fn prefetch_terms_batched(&self, node_ids: &[u32], predicate_ids: &[u32], batch: usize) {
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
        let want: [Vec<usize>; 4] = want.map(|set| set.into_iter().collect());
        self.prefetch_sections(&want, batch);
    }

    /// Does any section fault chunk bodies in on demand (a lazy or ranged
    /// open)? `false` for a resident dictionary, where every prefetch is a no-op.
    pub fn faults_chunks(&self) -> bool {
        self.sections.iter().any(|s| s.faults_chunks())
    }

    /// Batch-fault every chunk that looking these terms up will read, so the
    /// lookups that follow are cache hits: **one** batched read (one round trip
    /// remotely) instead of one dependent fetch per term and section.
    ///
    /// `node_terms` are looked up as nodes ([`node_of_term`](Self::node_of_term),
    /// [`subject_id`](Self::subject_id), [`object_id`](Self::object_id)), which
    /// probe the shared section and then the subject-only or object-only one —
    /// so all three routed chunks are fetched, plus those of the other spelling
    /// of a string literal (`"x"` ↔ `"x"^^xsd:string`). `pred_terms` are looked
    /// up in the predicate section. The routing is the lookup's own
    /// ([`ChunkedSection::chunk_for_term`]), read off the resident chunk
    /// directories. Purely a cache warmer: no-op on a resident dictionary, and
    /// a failed batch leaves every chunk to the per-lookup loader.
    ///
    /// [`ChunkedSection::chunk_for_term`]: crate::dict::ChunkedSection::chunk_for_term
    pub fn prefetch_lookups<'a>(
        &self,
        node_terms: impl IntoIterator<Item = &'a str>,
        pred_terms: impl IntoIterator<Item = &'a str>,
    ) {
        if !self.faults_chunks() {
            return;
        }
        let mut want: [std::collections::BTreeSet<usize>; 4] = Default::default();
        for term in node_terms {
            let alias = crate::terms::string_literal_alias(term);
            for t in std::iter::once(term).chain(alias.as_deref()) {
                for (si, set) in want.iter_mut().enumerate().take(3) {
                    if let Some(ci) = self.sections[si].chunk_for_term(t) {
                        set.insert(ci);
                    }
                }
            }
        }
        for term in pred_terms {
            if let Some(ci) = self.sections[3].chunk_for_term(term) {
                want[3].insert(ci);
            }
        }
        let want: [Vec<usize>; 4] = want.map(|set| set.into_iter().collect());
        self.prefetch_sections(&want, PREFETCH_BATCH_CHUNKS);
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

    /// Number of predicate terms.
    pub fn predicate_count(&self) -> u32 {
        self.sections[3].term_count()
    }

    /// [`subject_term`](Self::subject_term) for many ids at once: one
    /// chunk-ordered, single-pass walk per section
    /// ([`ChunkedSection::resolve_into`]) instead of a fault and a run walk per
    /// id — the difference between a whole-dictionary pass taking minutes and
    /// taking hours. `out[i]` is exactly what `subject_term(ids[i])` returns.
    ///
    /// [`ChunkedSection::resolve_into`]: crate::dict::ChunkedSection::resolve_into
    pub fn subject_terms(&self, ids: &[SubjectId]) -> Vec<Option<String>> {
        self.role_terms(ids, 1)
    }

    /// [`object_term`](Self::object_term) for many ids at once — see
    /// [`subject_terms`](Self::subject_terms).
    pub fn object_terms(&self, ids: &[ObjectId]) -> Vec<Option<String>> {
        self.role_terms(ids, 2)
    }

    /// Shared ids route to section 0, the rest to `own` (1 subjects, 2 objects)
    /// with the shared block subtracted — the routing of `subject_term` /
    /// `object_term`.
    fn role_terms(&self, ids: &[u32], own: usize) -> Vec<Option<String>> {
        let mut out = vec![None; ids.len()];
        let mut shared = Vec::new();
        let mut rest = Vec::new();
        for (slot, &id) in ids.iter().enumerate() {
            if id <= self.shared_len {
                shared.push((id, slot));
            } else {
                rest.push((id - self.shared_len, slot));
            }
        }
        self.sections[0].resolve_into(&mut shared, &mut out);
        self.sections[own].resolve_into(&mut rest, &mut out);
        out
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

    /// Batch-fault the dictionary chunks that decoding these subject-role ids
    /// will touch — one coalesced read per section instead of a blocking round
    /// trip per id. Purely a cache warmer (resident sections no-op).
    pub fn prefetch_subject_terms(&self, ids: &[SubjectId]) {
        let mut per_section: [Vec<usize>; 2] = Default::default();
        for &id in ids {
            let (si, local) = if id <= self.shared_len {
                (0usize, id)
            } else {
                (1, id - self.shared_len)
            };
            if let Some(ci) = self.sections[si].chunk_of_term(local) {
                per_section[si].push(ci);
            }
        }
        let [shared, subjects] = per_section;
        self.prefetch_sorted([shared, subjects, Vec::new(), Vec::new()]);
    }

    /// Sort, dedup and fault each section's chunk list (one batch across
    /// sections when the dictionary has a cross-section fetcher), at most
    /// [`PREFETCH_BATCH_CHUNKS`] chunks per read.
    fn prefetch_sorted(&self, mut want: [Vec<usize>; 4]) {
        for cis in &mut want {
            cis.sort_unstable();
            cis.dedup();
        }
        self.prefetch_sections(&want, PREFETCH_BATCH_CHUNKS);
    }

    /// [`prefetch_subject_terms`](Self::prefetch_subject_terms) for unified
    /// node ids — the id space solution rows carry. Warms every chunk a batch
    /// of row decodes (a FILTER's string ops, the projection) is about to
    /// fault, using [`node_term`](Self::node_term)'s exact routing.
    pub fn prefetch_node_terms(&self, nodes: &[NodeId]) {
        let su = self.subject_only_count();
        let mut per_section: [Vec<usize>; 3] = Default::default();
        for &node in nodes {
            let (si, local) = if node < self.shared_len + su {
                let sid = node + 1;
                if sid <= self.shared_len {
                    (0usize, sid)
                } else {
                    (1, sid - self.shared_len)
                }
            } else {
                let oid = node + 1 - su;
                if oid <= self.shared_len {
                    (0, oid)
                } else {
                    (2, oid - self.shared_len)
                }
            };
            if let Some(ci) = self.sections[si].chunk_of_term(local) {
                per_section[si].push(ci);
            }
        }
        let [shared, subjects, objects] = per_section;
        self.prefetch_sorted([shared, subjects, objects, Vec::new()]);
    }

    /// Unified node ID for a term, resolving via subject role then object role.
    /// A string literal also matches its other spelling (`"x"` ↔
    /// `"x"^^xsd:string`, one RDF 1.1 term; see [`Self::subject_id`]).
    pub fn node_of_term(&self, term: &str) -> Option<NodeId> {
        let exact = |t: &str| {
            self.subject_id_exact(t)
                .map(|sid| self.subject_node(sid))
                .or_else(|| self.object_id_exact(t).map(|oid| self.object_node(oid)))
        };
        exact(term).or_else(|| exact(&crate::terms::string_literal_alias(term)?))
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
    ///
    /// A string literal that is not found under its own spelling is looked up
    /// under the other (`"x"` ↔ `"x"^^xsd:string`): RDF 1.1 makes them one
    /// term, ingest keeps only the plain one, and a file built before it did
    /// may hold the typed one. The second lookup runs only on a miss, and the
    /// two spellings sort side by side, so it reads no further chunk.
    pub fn subject_id(&self, term: &str) -> Option<SubjectId> {
        self.subject_id_exact(term)
            .or_else(|| self.subject_id_exact(&crate::terms::string_literal_alias(term)?))
    }

    /// Object-role ID for `term`, with the same string-literal fallback as
    /// [`Self::subject_id`].
    pub fn object_id(&self, term: &str) -> Option<ObjectId> {
        self.object_id_exact(term)
            .or_else(|| self.object_id_exact(&crate::terms::string_literal_alias(term)?))
    }

    fn subject_id_exact(&self, term: &str) -> Option<SubjectId> {
        if let Some(id) = self.sections[0].id(term) {
            return Some(id);
        }
        self.sections[1].id(term).map(|id| self.shared_len + id)
    }

    /// Object-role ID for `term`: shared `1..=S`, else object-only `S+1..`.
    fn object_id_exact(&self, term: &str) -> Option<ObjectId> {
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

    /// Resolve a window of `(subject_id, predicate_id, object_id)` triples in
    /// **chunk order** and emit each fully-resolved row through `emit` in the
    /// window's **original order**.
    ///
    /// This is the batch twin of resolving each row with
    /// [`subject_term`](Self::subject_term) /
    /// [`predicate_term`](Self::predicate_term) /
    /// [`object_term`](Self::object_term) and emitting on `(Some, Some, Some)`
    /// — the exact thing it replaces inside `Rete::dump_filtered_each`. A row is
    /// emitted only when all three of its terms resolve, and the bytes of each
    /// term are identical to the per-row path, so the dump stays
    /// byte-for-byte the same.
    ///
    /// What changes is *how* the terms are decoded. Each row contributes three
    /// jobs to the section that owns its id (shared/subject-only for the
    /// subject, predicates for the predicate, shared/object-only for the
    /// object); each section then resolves its jobs in one chunk-ordered,
    /// single-pass walk ([`ChunkedSection::resolve_into`]). A window whose
    /// object ids scatter across the object-only section therefore touches each
    /// chunk once — one faulted body, one forward walk — instead of the
    /// scattered re-faults and per-row re-walks the row-order path performed.
    ///
    /// `w` carries the per-window scratch buffers so back-to-back windows cost
    /// no allocation beyond growth.
    ///
    /// [`ChunkedSection::resolve_into`]: crate::dict::ChunkedSection::resolve_into
    pub fn resolve_window(
        &self,
        ids: &[(SubjectId, PredicateId, ObjectId)],
        w: &mut WindowResolver,
        mut emit: impl FnMut(&str, &str, &str),
    ) {
        for j in &mut w.jobs {
            j.clear();
        }
        w.out.clear();
        w.out.resize(ids.len() * 3, None);
        for (i, &(s, p, o)) in ids.iter().enumerate() {
            // Subject role: shared (section 0) if `id <= S`, else subject-only
            // (section 1) with the shared block subtracted — the exact routing
            // of `subject_term`.
            if s <= self.shared_len {
                w.jobs[0].push((s, i * 3));
            } else {
                w.jobs[1].push((s - self.shared_len, i * 3));
            }
            // Predicate role: its own section (3).
            w.jobs[3].push((p, i * 3 + 1));
            // Object role: shared (section 0) if `id <= S`, else object-only
            // (section 2) — the exact routing of `object_term`.
            if o <= self.shared_len {
                w.jobs[0].push((o, i * 3 + 2));
            } else {
                w.jobs[2].push((o - self.shared_len, i * 3 + 2));
            }
        }
        for si in 0..4 {
            self.sections[si].resolve_into(&mut w.jobs[si], &mut w.out);
        }
        for i in 0..ids.len() {
            if let (Some(s), Some(p), Some(o)) =
                (&w.out[i * 3], &w.out[i * 3 + 1], &w.out[i * 3 + 2])
            {
                emit(s, p, o);
            }
        }
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
