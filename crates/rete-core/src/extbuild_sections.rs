//! The memory-bounded build's LABEL_INDEX and SCHEMA sections (kinds 8, 9).
//!
//! Kept out of `extbuild.rs` on purpose: that file's phases call a handful of
//! hooks here and nothing else, so the label and schema passes stay one
//! delimited unit.
//!
//! - **Labels** need only strings: each chunk, while its raw quads are
//!   resident, emits `(subject, slot, value)` records for the label and name
//!   predicates ([`ExtLabels::observe_chunk`]). After the input, one external
//!   sort groups them by subject, the L1/L2 rules run per subject, a second
//!   external sort orders the entries by `(key, subject)`, and the section's
//!   data blocks stream to a spill file ([`ExtLabels::finish`]).
//! - **Schema** follows the deduplicated permutation streams: the remap pass
//!   tees the type statements into a `(subject, class)` sort
//!   ([`ExtSchema::tee`]); the SPO and POS merges feed a
//!   [`SchemaAccumulator`] ([`ExtSchema::observe_spo`], `observe_pos`), with
//!   `class(s)` from a cursor over the sorted types in lockstep with SPO, and
//!   the links whose object is a shared term deferred through a sort on the
//!   object and joined after the pass ([`ExtSchema::finish_links`]). The few
//!   ids the section names are resolved through the finished file's own
//!   dictionary ([`ExtSchema::finish`]).
//!
//! RAM: every sort holds at most one run, sized from the build's budget; the
//! accumulator holds per-predicate counters, the tracked entity shapes and the
//! class-level link counts. Both sections come out byte-identical to the
//! in-memory build's (`ingest::finish_assembly`).

#![cfg(not(target_arch = "wasm32"))]

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use crate::extsort::{ExtSorter, Sorted};
use crate::label_index::{
    candidate_value, slot_of_predicate, LabelIndexWriter, Slot, SubjectCandidates,
};
use crate::schema_section::{
    ClassCursor, SchemaAccumulator, SchemaIds, Space, OWL_DISJOINT_WITH, OWL_EQUIVALENT_CLASS,
    RDFS_SUBCLASS_OF, UNTYPED,
};

/// A local file as a [`crate::RangeReader`], for reading back the output.
struct FileReader {
    f: std::sync::Mutex<File>,
    len: u64,
}

impl crate::RangeReader for FileReader {
    fn len(&self) -> u64 {
        self.len
    }
    fn read_at(&self, offset: u64, len: u64) -> std::io::Result<Vec<u8>> {
        use std::io::{Seek, SeekFrom};
        let mut f = self
            .f
            .lock()
            .map_err(|_| std::io::Error::other("poisoned"))?;
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }
}

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// The label pass of the memory-bounded build.
pub(crate) struct ExtLabels {
    dir: PathBuf,
    run_bytes: usize,
    candidates: ExtSorter<(String, String, String)>,
}

/// A finished LABEL_INDEX, ready to append: `digest ++ head ++ body file`.
pub(crate) struct ExtLabelSection {
    pub digest_and_head: Vec<u8>,
    pub body: PathBuf,
    pub entries: u64,
}

impl ExtLabels {
    /// `run_bytes` bounds each sort run.
    pub(crate) fn new(dir: &Path, run_bytes: usize) -> Self {
        ExtLabels {
            dir: dir.to_path_buf(),
            run_bytes,
            candidates: ExtSorter::new(dir, "lab-cand", run_bytes),
        }
    }

    /// Record the label candidates of one chunk's default-graph quads.
    pub(crate) fn observe_chunk(
        &mut self,
        quads: &[(String, String, String)],
    ) -> std::io::Result<()> {
        for (s, p, o) in quads {
            let Some(slot) = slot_of_predicate(p) else {
                continue;
            };
            if let Some(v) = candidate_value(o) {
                self.candidates
                    .push((s.clone(), (slot as u8).to_string(), v))?;
            }
        }
        Ok(())
    }

    /// Run the rules and write the section's blocks to a spill file. `None`
    /// when the graph has no label.
    pub(crate) fn finish(self, codec: u8) -> std::io::Result<Option<ExtLabelSection>> {
        if self.candidates.pushed() == 0 {
            return Ok(None);
        }
        // 1. grouped by subject → the rules → entries in a second sort
        let mut entries: ExtSorter<(String, String, String)> =
            ExtSorter::new(&self.dir, "lab-entry", self.run_bytes);
        {
            let mut sorted = self.candidates.finish()?;
            let mut cur: Option<String> = None;
            let mut cands = SubjectCandidates::default();
            let flush = |subject: &str,
                         cands: &mut SubjectCandidates,
                         entries: &mut ExtSorter<(String, String, String)>|
             -> std::io::Result<()> {
                for (key, label) in cands.labels() {
                    entries.push((key, subject.to_string(), label))?;
                }
                *cands = SubjectCandidates::default();
                Ok(())
            };
            while let Some((s, slot, v)) = sorted.next_record()? {
                if cur.as_deref() != Some(s.as_str()) {
                    if let Some(prev) = cur.take() {
                        flush(&prev, &mut cands, &mut entries)?;
                    }
                    cur = Some(s);
                }
                let slot = slot
                    .parse::<u8>()
                    .ok()
                    .and_then(Slot::from_u8)
                    .ok_or_else(|| std::io::Error::other("bad label slot in spill"))?;
                cands.push(slot, v);
            }
            if let Some(prev) = cur {
                flush(&prev, &mut cands, &mut entries)?;
            }
        }
        // 2. entries in (key, subject) order → the section writer
        let body_path = self.dir.join("labels.body");
        let body = BufWriter::with_capacity(1 << 20, File::create(&body_path)?);
        let mut w = LabelIndexWriter::new(codec, body);
        let mut sorted = entries.finish()?;
        while let Some((key, subject, label)) = sorted.next_record()? {
            w.push(&key, &subject, &label)?;
        }
        let parts = w.finish()?;
        let mut body = parts.body;
        body.flush()?;
        drop(body);
        if parts.entry_count == 0 {
            let _ = std::fs::remove_file(&body_path);
            return Ok(None);
        }
        // digest = blake3(head ++ body)
        let mut h = blake3::Hasher::new();
        h.update(&parts.head);
        let mut rd = BufReader::with_capacity(1 << 20, File::open(&body_path)?);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = rd.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        let mut digest_and_head = h.finalize().as_bytes()[..16].to_vec();
        digest_and_head.extend_from_slice(&parts.head);
        Ok(Some(ExtLabelSection {
            digest_and_head,
            body: body_path,
            entries: parts.entry_count,
        }))
    }
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// The predicate terms the schema pass needs ids for, in this order: the type
/// predicate, `rdfs:subClassOf`, `owl:disjointWith`, `owl:equivalentClass`.
pub(crate) fn schema_predicates(type_override: Option<&str>) -> Vec<String> {
    vec![
        SchemaIds::type_term_for(type_override),
        RDFS_SUBCLASS_OF.to_string(),
        OWL_DISJOINT_WITH.to_string(),
        OWL_EQUIVALENT_CLASS.to_string(),
    ]
}

/// What the dictionary merge learned for the schema pass.
#[derive(Debug, Clone, Default)]
pub(crate) struct MergeFacts {
    /// Global predicate ids of [`schema_predicates`], in order.
    pub pred_ids: Vec<Option<u32>>,
    pub n_shared: u32,
    pub shared_literals: u32,
    pub objonly_literals: u32,
}

/// A sorted `(subject, class)` file read back as a [`ClassCursor`] source.
struct RepReader {
    rd: BufReader<File>,
}

impl Iterator for RepReader {
    type Item = (u32, u32);
    fn next(&mut self) -> Option<(u32, u32)> {
        let mut b = [0u8; 8];
        self.rd.read_exact(&mut b).ok()?;
        Some((
            u32::from_le_bytes(b[0..4].try_into().unwrap()),
            u32::from_le_bytes(b[4..8].try_into().unwrap()),
        ))
    }
}

/// A cursor over the representative-class file.
fn rep_cursor(path: &Path) -> std::io::Result<ClassCursor<RepReader>> {
    Ok(ClassCursor::new(RepReader {
        rd: BufReader::with_capacity(1 << 20, File::open(path)?),
    }))
}

/// The schema pass of the memory-bounded build.
pub(crate) struct ExtSchema {
    type_term: String,
    acc: Option<SchemaAccumulator>,
    types: Option<ExtSorter<(u32, u32)>>,
    reps: PathBuf,
    cursor: Option<ClassCursor<RepReader>>,
    deferred: Option<ExtSorter<(u32, u32, u32)>>,
}

impl ExtSchema {
    pub(crate) fn new(dir: &Path, run_bytes: usize, type_override: Option<&str>) -> Self {
        ExtSchema {
            type_term: SchemaIds::type_term_for(type_override),
            acc: None,
            types: Some(ExtSorter::new(dir, "sch-types", run_bytes)),
            reps: dir.join("sch-reps"),
            cursor: None,
            deferred: Some(ExtSorter::new_keep_duplicates(dir, "sch-defer", run_bytes)),
        }
    }

    /// The ids the dictionary merge found.
    pub(crate) fn set_merge_facts(&mut self, f: &MergeFacts) {
        let id = |i: usize| f.pred_ids.get(i).copied().flatten();
        let ids = SchemaIds {
            type_pid: id(0),
            type_term: id(0).map(|_| self.type_term.clone()),
            subclass_pid: id(1),
            disjoint_pid: id(2),
            equivalent_pid: id(3),
            n_shared: f.n_shared,
            shared_literals: f.shared_literals,
            objonly_literals: f.objonly_literals,
        };
        self.acc = Some(SchemaAccumulator::new(ids));
    }

    /// The remap pass: one default-graph statement in global ids.
    pub(crate) fn tee(&mut self, s: u32, p: u32, o: u32) -> std::io::Result<()> {
        let tp = self.acc.as_ref().and_then(|a| a.ids().type_pid);
        if Some(p) == tp {
            if let Some(t) = self.types.as_mut() {
                t.push((s, o))?;
            }
        }
        Ok(())
    }

    /// Before the SPO pass: the representative class (smallest class id) of
    /// every typed subject, to a file the cursors read.
    pub(crate) fn begin_spo(&mut self) -> std::io::Result<()> {
        let Some(types) = self.types.take() else {
            return Ok(());
        };
        let mut sorted = types.finish()?;
        let mut w = BufWriter::with_capacity(1 << 20, File::create(&self.reps)?);
        let mut last: Option<u32> = None;
        while let Some((s, c)) = sorted.next_record()? {
            if last != Some(s) {
                w.write_all(&s.to_le_bytes())?;
                w.write_all(&c.to_le_bytes())?;
                last = Some(s);
            }
        }
        w.flush()?;
        self.cursor = Some(rep_cursor(&self.reps)?);
        Ok(())
    }

    /// One deduplicated statement of the SPO merge, as `(s, p, o)`.
    pub(crate) fn observe_spo(&mut self, t: (u32, u32, u32)) -> std::io::Result<()> {
        let (s, p, o) = t;
        let cs = self.cursor.as_mut().and_then(|c| c.get(s));
        let Some(acc) = self.acc.as_mut() else {
            return Ok(());
        };
        if let Some((obj, cls_s, pred)) = acc.observe_spo(s, p, o, cs) {
            if let Some(d) = self.deferred.as_mut() {
                d.push((obj, cls_s, pred))?;
            }
        }
        Ok(())
    }

    /// One deduplicated statement of the POS merge, as `(p, o, s)`.
    pub(crate) fn observe_pos(&mut self, t: (u32, u32, u32)) {
        if let Some(acc) = self.acc.as_mut() {
            acc.observe_pos(t.0, t.1);
        }
    }

    /// After the permutations: close both passes and join the deferred links
    /// with the object's class.
    pub(crate) fn finish_links(&mut self) -> std::io::Result<()> {
        let Some(acc) = self.acc.as_mut() else {
            return Ok(());
        };
        acc.finish_spo();
        acc.finish_pos();
        self.cursor = None;
        let Some(deferred) = self.deferred.take() else {
            return Ok(());
        };
        let mut sorted: Sorted<(u32, u32, u32)> = deferred.finish()?;
        let mut cursor = if self.reps.exists() {
            Some(rep_cursor(&self.reps)?)
        } else {
            None
        };
        while let Some((o, cls_s, p)) = sorted.next_record()? {
            let co = cursor.as_mut().and_then(|c| c.get(o)).unwrap_or(UNTYPED);
            acc.add_link(cls_s, p, co);
        }
        let _ = std::fs::remove_file(&self.reps);
        Ok(())
    }

    /// Resolve the ids through the finished file's dictionary and encode the
    /// section. Empty when there is nothing to say.
    pub(crate) fn finish(mut self, output: &Path) -> Result<Vec<u8>, crate::file::FileError> {
        let Some(acc) = self.acc.take() else {
            return Ok(Vec::new());
        };
        let f = File::open(output)?;
        let len = f.metadata()?.len();
        let dict = crate::file::open_dictionary_lazy(FileReader {
            f: std::sync::Mutex::new(f),
            len,
        })?;
        let meta = acc.finalize(&|space, id| match space {
            Space::Subject => dict.subject_term(id),
            Space::Object => dict.object_term(id),
            Space::Predicate => dict.predicate_term(id),
        });
        Ok(meta.encode())
    }
}

#[cfg(test)]
mod tests {
    use crate::extbuild::{build_external, ExternalBuildOptions};
    use crate::header::Header;
    use crate::ingest::{self, AssembleOptions, RawQuad};

    const T: &str = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";
    const SUB: &str = "<http://www.w3.org/2000/01/rdf-schema#subClassOf>";
    const TITLE: &str = "<http://purl.org/dc/terms/title>";
    const NAME: &str = "<http://xmlns.com/foaf/0.1/name>";
    const FAM: &str = "<http://xmlns.com/foaf/0.1/familyName>";
    const GIV: &str = "<http://xmlns.com/foaf/0.1/givenName>";

    /// Works with titles, people with only given/family names (and a few with
    /// a foaf:name, which wins), a small class hierarchy, a disjointness,
    /// literals that need normalizing, duplicates, blank nodes, and labels in
    /// a named graph (which are not indexed).
    fn section_quads(n: usize) -> Vec<RawQuad> {
        let q = |s: String, p: &str, o: String| (s, p.to_string(), o, None);
        let mut v: Vec<RawQuad> = vec![
            q("<http://ex/Article>".into(), SUB, "<http://ex/Work>".into()),
            q("<http://ex/Book>".into(), SUB, "<http://ex/Work>".into()),
            q("<http://ex/Person>".into(), SUB, "<http://ex/Agent>".into()),
            q(
                "<http://ex/Work>".into(),
                "<http://www.w3.org/2002/07/owl#disjointWith>",
                "<http://ex/Agent>".into(),
            ),
        ];
        for i in 0..n {
            let w = format!("<http://ex/w{i}>");
            let class = if i % 7 == 0 {
                "<http://ex/Book>"
            } else {
                "<http://ex/Article>"
            };
            v.push(q(w.clone(), T, class.into()));
            // `\n` as the two characters N-Triples writes: terms never hold a raw newline
            v.push(q(
                w.clone(),
                TITLE,
                format!("\"Title {i}  of\\n a work\"@en"),
            ));
            if i % 11 == 0 {
                // same key, other case: one entry per (subject, key)
                v.push(q(w.clone(), TITLE, format!("\"TITLE {i} OF A WORK\"")));
            }
            let p = format!("<http://ex/p{}>", i % (n / 3 + 1));
            v.push(q(w.clone(), "<http://ex/author>", p.clone()));
            v.push(q(
                w.clone(),
                "<http://ex/cites>",
                format!("<http://ex/w{}>", (i * 7) % n),
            ));
            v.push(q(w.clone(), "<http://ex/page>", format!("\"{}\"", i % 300)));
            v.push(q(w, "<http://ex/note>", format!("_:b{}", i % 50)));
            v.push(q(p.clone(), T, "<http://ex/Person>".into()));
            v.push(q(p.clone(), FAM, format!("\"Fam{}\"", i % (n / 3 + 1))));
            v.push(q(p.clone(), GIV, format!("\"Giv{}\"", i % 13)));
            if i % 50 == 0 {
                v.push(q(p, NAME, format!("\"Org {i}\"")));
            }
        }
        // duplicates, and a label in a named graph
        for i in 0..(n / 10) {
            let d = v[(i * 13) % v.len()].clone();
            v.push(d);
        }
        v.push((
            "<http://ex/g-only>".into(),
            TITLE.into(),
            "\"In a named graph\"".into(),
            Some("<http://ex/graph>".into()),
        ));
        v
    }

    fn reference(quads: &[RawQuad], label_index: bool, schema: bool) -> Vec<u8> {
        let opts = AssembleOptions {
            with_pyramid: false,
            label_index,
            schema,
            ..AssembleOptions::sectioned()
        };
        ingest::assemble_dataset_streaming_opts(
            |visit: &mut dyn FnMut(RawQuad)| {
                for q in quads.iter().cloned() {
                    visit(q);
                }
                Ok(())
            },
            &opts,
            |_, _, _| Vec::new(),
        )
        .unwrap()
        .0
    }

    fn external(quads: &[RawQuad], label_index: bool, schema: bool, tag: &str) -> Vec<u8> {
        let dir = std::env::temp_dir().join(format!(
            "rete-extsec-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("out.rete");
        build_external(
            |visit| {
                for q in quads.iter().cloned() {
                    visit(q)?;
                }
                Ok(())
            },
            &out,
            ExternalBuildOptions {
                memory_budget: 0,
                tmp_dir: Some(dir.clone()),
                label_index,
                schema,
                ..Default::default()
            },
        )
        .unwrap();
        let bytes = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        bytes
    }

    #[test]
    fn external_sections_are_byte_identical_to_the_in_memory_build() {
        let quads = section_quads(2_000);
        // Tiny sort runs: every external sort spills and k-way merges.
        crate::extsort::TEST_RUN_BYTES.with(|c| c.set(Some(4 * 1024)));
        let ext = external(&quads, true, true, "both");
        crate::extsort::TEST_RUN_BYTES.with(|c| c.set(None));
        let mem = reference(&quads, true, true);
        let h = Header::from_bytes(&ext).unwrap();
        assert!(h.label_index_section().is_some() && h.schema_section().is_some());
        assert_eq!(ext.len(), mem.len(), "same length");
        assert!(ext == mem, "the external build writes the in-memory bytes");

        // And the sections stay outside the content hash.
        let plain = external(&quads, false, false, "plain");
        assert_eq!(
            Header::from_bytes(&plain).unwrap().content_hash,
            h.content_hash
        );
        assert!(crate::verify(&ext).unwrap());
    }

    #[test]
    fn the_sections_answer_what_they_promise() {
        let quads = section_quads(600);
        let bytes = external(&quads, true, true, "read");
        let rete = crate::Rete::open(&bytes).unwrap();
        // People with only given/family names are labelled "family, given".
        let hits = rete.prefix_search("fam1, ", 5);
        assert!(!hits.is_empty(), "composed person labels");
        assert!(hits.iter().all(|(l, _)| l.starts_with("Fam1, Giv")));
        // Whitespace is collapsed; one entry per (subject, key).
        let t = rete.prefix_search("title 11 of a work", 10);
        assert_eq!(t.len(), 1, "{t:?}");
        assert_eq!(
            t[0].0, "TITLE 11 OF A WORK",
            "the smallest label of the key"
        );
        // foaf:name wins over the composed name.
        assert!(!rete.prefix_search("org 0", 3).is_empty());
        // Named-graph labels are not indexed.
        assert!(rete.prefix_search("in a named graph", 3).is_empty());

        let schema = rete.schema().expect("schema section");
        assert!(schema
            .class_hierarchy
            .iter()
            .any(|c| c.class == "<http://ex/Article>" && c.parents == ["<http://ex/Work>"]));
        assert_eq!(
            schema.disjoint_pairs,
            vec![(
                "<http://ex/Agent>".to_string(),
                "<http://ex/Work>".to_string()
            )]
        );
        let dict = rete.dictionary();
        let title = dict.predicate_id(TITLE).unwrap();
        let st = rete
            .predicate_stats()
            .iter()
            .find(|s| s.predicate == title)
            .unwrap();
        // 600 titles + the upper-case variants of i % 11 == 0, deduplicated.
        assert_eq!(st.count, 600 + 55);
        assert_eq!(st.distinct_subjects, 600);
        assert!(!rete.char_sets().is_empty());
        // No pyramid, yet the planner has its statistics.
        assert!(rete.pyramid().is_none());
        assert!(!rete.planner_stats(false).is_empty());
    }
}
