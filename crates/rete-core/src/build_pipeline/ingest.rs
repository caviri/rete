use std::collections::{BTreeMap, HashMap};

use crate::ingest::{BuildStats, RawQuad};
use crate::{Dictionary, Triple};

#[cfg(not(target_arch = "wasm32"))]
use super::spool::{BuildTemp, TripleSpool};
use super::BuildPipelineError;

pub(crate) const DEFAULT_GRAPH_ID: u32 = u32::MAX;

const SUBJECT_ROLE: u8 = 1;
const OBJECT_ROLE: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ProvisionalQuad {
    pub subject: u32,
    pub predicate: u32,
    pub object: u32,
    pub graph: u32,
}

pub(crate) struct CanonicalMemory {
    pub dictionary: Dictionary,
    pub default_triples: Vec<Triple>,
    pub named: BTreeMap<String, Vec<Triple>>,
    pub metadata: Vec<u8>,
    pub stats: BuildStats,
}

impl CanonicalMemory {
    pub(crate) fn unique_node_terms(&self) -> u32 {
        self.dictionary.node_count()
    }

    pub(crate) fn unique_predicate_terms(&self) -> u32 {
        self.dictionary
            .term_count()
            .saturating_sub(self.dictionary.node_count())
    }
}

pub(crate) struct MemoryIngest {
    nodes: HashMap<String, u32>,
    node_roles: Vec<u8>,
    predicates: HashMap<String, u32>,
    graphs: HashMap<String, u32>,
    records: Vec<ProvisionalQuad>,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct SpilledDictionary {
    pub(crate) section_paths: [std::path::PathBuf; 4],
    pub(crate) term_count: u64,
    pub(crate) has_quoted_triples: bool,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct CanonicalSpilled {
    pub(crate) dictionary: SpilledDictionary,
    pub(crate) triples: TripleSpool,
    pub(crate) metadata: Vec<u8>,
    pub(crate) stats: BuildStats,
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct ChunkedIngest<'a> {
    temp: &'a BuildTemp,
    chunker: crate::extbuild::Chunker<'a>,
}

#[cfg(not(target_arch = "wasm32"))]
impl<'a> ChunkedIngest<'a> {
    pub(crate) fn new(temp: &'a BuildTemp, memory_budget: u64) -> Self {
        let budget = memory_budget.max(64 << 20);
        let chunk_budget = (budget as f64 * crate::extbuild::CHUNK_BUDGET_FRACTION) as u64;
        Self {
            temp,
            chunker: crate::extbuild::Chunker::new(temp, chunk_budget),
        }
    }

    pub(crate) fn push(&mut self, quad: RawQuad) -> Result<(), BuildPipelineError> {
        self.chunker
            .push(quad)
            .map_err(crate::extbuild::ExtBuildError::into_pipeline)
    }

    pub(crate) fn finish(
        self,
        metadata: impl FnOnce(&BuildStats) -> Vec<u8>,
    ) -> Result<CanonicalSpilled, BuildPipelineError> {
        let chunks = self
            .chunker
            .finish()
            .map_err(crate::extbuild::ExtBuildError::into_pipeline)?;
        let statements = chunks.iter().try_fold(0u64, |total, chunk| {
            total
                .checked_add(chunk.triple_count)
                .ok_or(BuildPipelineError::Overflow("statement count"))
        })?;
        let mut merged = crate::extbuild::merge_dictionaries(self.temp, &chunks)
            .map_err(crate::extbuild::ExtBuildError::into_pipeline)?;
        let stats = BuildStats {
            statements: usize::try_from(statements)
                .map_err(|_| BuildPipelineError::Overflow("statement count"))?,
            default_triples: usize::try_from(statements)
                .map_err(|_| BuildPipelineError::Overflow("statement count"))?,
            named_graphs: 0,
            terms: usize::try_from(merged.term_count)
                .map_err(|_| BuildPipelineError::TooManyTerms)?,
            pyramid_levels: 0,
        };
        let dictionary = SpilledDictionary {
            section_paths: merged.section_paths(),
            term_count: merged.term_count,
            has_quoted_triples: merged.has_quoted,
        };
        let triples = crate::extbuild::remap_chunks_to_spool(self.temp, &chunks, &mut merged)
            .map_err(crate::extbuild::ExtBuildError::into_pipeline)?;
        Ok(CanonicalSpilled {
            dictionary,
            triples,
            metadata: metadata(&stats),
            stats,
        })
    }
}

struct CanonicalNodes {
    shared: Vec<String>,
    subjects: Vec<String>,
    objects: Vec<String>,
    subject_remap: Vec<u32>,
    object_remap: Vec<u32>,
    has_quoted_triples: bool,
}

impl MemoryIngest {
    pub(crate) fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            node_roles: Vec::new(),
            predicates: HashMap::new(),
            graphs: HashMap::new(),
            records: Vec::new(),
        }
    }

    pub(crate) fn push(
        &mut self,
        (subject, predicate, object, graph): RawQuad,
    ) -> Result<(), BuildPipelineError> {
        let subject = self.intern_node(subject, SUBJECT_ROLE)?;
        let predicate = intern_term(&mut self.predicates, predicate)?;
        let object = self.intern_node(object, OBJECT_ROLE)?;
        let graph = match graph {
            Some(graph) => intern_term(&mut self.graphs, graph)?,
            None => DEFAULT_GRAPH_ID,
        };
        self.records.push(ProvisionalQuad {
            subject,
            predicate,
            object,
            graph,
        });
        Ok(())
    }

    pub(crate) fn finish(
        self,
        metadata: impl FnOnce(&BuildStats) -> Vec<u8>,
    ) -> Result<CanonicalMemory, BuildPipelineError> {
        let Self {
            nodes,
            node_roles,
            predicates,
            graphs,
            records,
        } = self;
        let nodes = canonicalize_nodes(nodes, node_roles)?;
        let (predicates, predicate_remap) = canonicalize_predicates(predicates)?;
        let dictionary = Dictionary::from_role_terms(
            nodes.shared,
            nodes.subjects,
            nodes.objects,
            predicates,
            nodes.has_quoted_triples,
        );

        let mut default_triples = Vec::new();
        let mut named_by_id: HashMap<u32, Vec<Triple>> = HashMap::new();
        for record in records {
            let triple = (
                remap(&nodes.subject_remap, record.subject)?,
                remap(&predicate_remap, record.predicate)?,
                remap(&nodes.object_remap, record.object)?,
            );
            if record.graph == DEFAULT_GRAPH_ID {
                default_triples.push(triple);
            } else {
                named_by_id.entry(record.graph).or_default().push(triple);
            }
        }
        let named_statements = named_by_id.values().try_fold(0usize, |count, triples| {
            count
                .checked_add(triples.len())
                .ok_or(BuildPipelineError::Overflow("statement count"))
        })?;
        let statements = default_triples
            .len()
            .checked_add(named_statements)
            .ok_or(BuildPipelineError::Overflow("statement count"))?;
        let named_graphs = named_by_id.len();
        let named = graphs
            .into_iter()
            .map(|(graph, id)| (graph, named_by_id.remove(&id).unwrap_or_default()))
            .collect();
        let stats = BuildStats {
            statements,
            default_triples: default_triples.len(),
            named_graphs,
            terms: dictionary.term_count() as usize,
            pyramid_levels: 0,
        };
        let metadata = metadata(&stats);
        Ok(CanonicalMemory {
            dictionary,
            default_triples,
            named,
            metadata,
            stats,
        })
    }

    fn intern_node(&mut self, term: String, role: u8) -> Result<u32, BuildPipelineError> {
        if let Some(&id) = self.nodes.get(term.as_str()) {
            let index = usize::try_from(id).map_err(|_| BuildPipelineError::TooManyTerms)?;
            let bits = self
                .node_roles
                .get_mut(index)
                .ok_or(BuildPipelineError::InvalidSpool("node role missing"))?;
            *bits |= role;
            return Ok(id);
        }
        let id = next_provisional_id(self.nodes.len())?;
        self.nodes.insert(term, id);
        self.node_roles.push(role);
        Ok(id)
    }
}

fn intern_term(terms: &mut HashMap<String, u32>, term: String) -> Result<u32, BuildPipelineError> {
    if let Some(&id) = terms.get(term.as_str()) {
        return Ok(id);
    }
    let id = next_provisional_id(terms.len())?;
    terms.insert(term, id);
    Ok(id)
}

fn next_provisional_id(term_count: usize) -> Result<u32, BuildPipelineError> {
    let id = u32::try_from(term_count).map_err(|_| BuildPipelineError::TooManyTerms)?;
    if id == u32::MAX {
        return Err(BuildPipelineError::TooManyTerms);
    }
    Ok(id)
}

fn canonicalize_nodes(
    nodes: HashMap<String, u32>,
    roles: Vec<u8>,
) -> Result<CanonicalNodes, BuildPipelineError> {
    let mut nodes: Vec<(String, u32)> = nodes.into_iter().collect();
    nodes.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut shared = Vec::new();
    let mut subjects = Vec::new();
    let mut objects = Vec::new();
    let mut subject_remap = vec![0; roles.len()];
    let mut object_remap = vec![0; roles.len()];
    let mut has_quoted_triples = false;

    for (term, id) in nodes {
        let index = usize::try_from(id).map_err(|_| BuildPipelineError::TooManyTerms)?;
        let role = *roles
            .get(index)
            .ok_or(BuildPipelineError::InvalidSpool("node role missing"))?;
        has_quoted_triples |= term.starts_with("<<");
        match role {
            role if role == SUBJECT_ROLE | OBJECT_ROLE => shared.push((term, id)),
            SUBJECT_ROLE => subjects.push((term, id)),
            OBJECT_ROLE => objects.push((term, id)),
            _ => return Err(BuildPipelineError::InvalidSpool("invalid node role")),
        }
    }

    for (index, (_, id)) in shared.iter().enumerate() {
        let canonical = u32::try_from(index + 1).map_err(|_| BuildPipelineError::TooManyTerms)?;
        let provisional = usize::try_from(*id).map_err(|_| BuildPipelineError::TooManyTerms)?;
        subject_remap[provisional] = canonical;
        object_remap[provisional] = canonical;
    }
    let shared_len = shared.len();
    for (index, (_, id)) in subjects.iter().enumerate() {
        let canonical =
            u32::try_from(shared_len + index + 1).map_err(|_| BuildPipelineError::TooManyTerms)?;
        let provisional = usize::try_from(*id).map_err(|_| BuildPipelineError::TooManyTerms)?;
        subject_remap[provisional] = canonical;
    }
    for (index, (_, id)) in objects.iter().enumerate() {
        let canonical =
            u32::try_from(shared_len + index + 1).map_err(|_| BuildPipelineError::TooManyTerms)?;
        let provisional = usize::try_from(*id).map_err(|_| BuildPipelineError::TooManyTerms)?;
        object_remap[provisional] = canonical;
    }

    Ok(CanonicalNodes {
        shared: shared.into_iter().map(|(term, _)| term).collect(),
        subjects: subjects.into_iter().map(|(term, _)| term).collect(),
        objects: objects.into_iter().map(|(term, _)| term).collect(),
        subject_remap,
        object_remap,
        has_quoted_triples,
    })
}

fn canonicalize_predicates(
    predicates: HashMap<String, u32>,
) -> Result<(Vec<String>, Vec<u32>), BuildPipelineError> {
    let mut predicates: Vec<(String, u32)> = predicates.into_iter().collect();
    predicates.sort_unstable_by(|left, right| left.0.cmp(&right.0));
    let mut remap = vec![0; predicates.len()];
    let mut terms = Vec::with_capacity(predicates.len());
    for (index, (term, id)) in predicates.into_iter().enumerate() {
        let canonical = u32::try_from(index + 1).map_err(|_| BuildPipelineError::TooManyTerms)?;
        let provisional = usize::try_from(id).map_err(|_| BuildPipelineError::TooManyTerms)?;
        remap[provisional] = canonical;
        terms.push(term);
    }
    Ok((terms, remap))
}

fn remap(ids: &[u32], provisional: u32) -> Result<u32, BuildPipelineError> {
    let index = usize::try_from(provisional).map_err(|_| BuildPipelineError::TooManyTerms)?;
    ids.get(index)
        .copied()
        .filter(|&id| id != 0)
        .ok_or(BuildPipelineError::InvalidSpool("missing canonical id"))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::dictionary::DictionaryBuilder;
    use crate::ingest::RawQuad;

    use super::{
        next_provisional_id, BuildPipelineError, ChunkedIngest, MemoryIngest, DEFAULT_GRAPH_ID,
    };
    use crate::build_pipeline::spool::BuildTemp;

    type CanonicalContent = (
        [Vec<u8>; 4],
        Vec<(u32, u32, u32)>,
        BTreeMap<String, Vec<(u32, u32, u32)>>,
    );

    fn q(subject: &str, predicate: &str, object: &str, graph: Option<&str>) -> RawQuad {
        (
            subject.to_owned(),
            predicate.to_owned(),
            object.to_owned(),
            graph.map(str::to_owned),
        )
    }

    #[test]
    fn repeated_terms_allocate_once_and_roles_remap_exactly() {
        let mut ingest = MemoryIngest::new();
        ingest.push(q("<a>", "<p>", "<b>", None)).unwrap();
        ingest.push(q("<b>", "<p>", "<a>", None)).unwrap();

        let built = ingest.finish(|_| Vec::new()).unwrap();

        assert_eq!(built.unique_node_terms(), 2);
        assert_eq!(built.unique_predicate_terms(), 1);
        assert_eq!(built.default_triples, vec![(1, 1, 2), (2, 1, 1)]);
        assert_eq!(built.dictionary.subject_term(1).as_deref(), Some("<a>"));
        assert_eq!(built.dictionary.object_term(2).as_deref(), Some("<b>"));
        assert_eq!(built.stats.statements, 2);
        assert_eq!(built.stats.terms, 3);
    }

    #[test]
    fn provisional_ids_reserve_u32_max_for_the_default_graph_sentinel() {
        assert_eq!(DEFAULT_GRAPH_ID, u32::MAX);
        let last_usable = usize::try_from(DEFAULT_GRAPH_ID - 1).unwrap();
        let reserved = usize::try_from(DEFAULT_GRAPH_ID).unwrap();

        assert_eq!(
            next_provisional_id(last_usable).unwrap(),
            DEFAULT_GRAPH_ID - 1
        );
        assert!(matches!(
            next_provisional_id(reserved),
            Err(BuildPipelineError::TooManyTerms)
        ));
    }

    #[test]
    fn metadata_and_stats_cover_default_and_lexically_ordered_named_graphs() {
        let mut ingest = MemoryIngest::new();
        ingest
            .push(q("<default-s>", "<p>", "<default-o>", None))
            .unwrap();
        ingest
            .push(q("<z-s>", "<p>", "<z-o>", Some("<z-graph>")))
            .unwrap();
        ingest
            .push(q("<a-s>", "<p>", "<a-o>", Some("<a-graph>")))
            .unwrap();
        ingest
            .push(q("<a-s>", "<q>", "<a-second-o>", Some("<a-graph>")))
            .unwrap();

        let built = ingest
            .finish(|stats| {
                format!(
                    "statements={};default={};named={}",
                    stats.statements, stats.default_triples, stats.named_graphs
                )
                .into_bytes()
            })
            .unwrap();

        assert_eq!(built.stats.statements, 4);
        assert_eq!(built.stats.default_triples, 1);
        assert_eq!(built.stats.named_graphs, 2);
        assert_eq!(built.stats.terms, 9);
        assert_eq!(built.stats.pyramid_levels, 0);
        assert_eq!(built.metadata, b"statements=4;default=1;named=2");
        assert_eq!(built.default_triples.len(), 1);
        assert_eq!(built.named["<a-graph>"].len(), 2);
        assert_eq!(built.named["<z-graph>"].len(), 1);
        assert_eq!(
            built.named.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["<a-graph>", "<z-graph>"]
        );
    }

    #[test]
    fn input_order_and_duplicates_do_not_change_sorted_canonical_content() {
        let forward = vec![
            q("<a>", "<p>", "<b>", None),
            q("<b>", "<q>", "<c>", Some("<g>")),
        ];
        let reversed_with_duplicates = vec![
            q("<b>", "<q>", "<c>", Some("<g>")),
            q("<a>", "<p>", "<b>", None),
            q("<b>", "<q>", "<c>", Some("<g>")),
            q("<a>", "<p>", "<b>", None),
        ];

        assert_eq!(
            sorted_dictionary_and_deduped_triples(forward),
            sorted_dictionary_and_deduped_triples(reversed_with_duplicates),
        );
    }

    #[test]
    fn canonicalization_matches_dictionary_builder_for_every_term_shape() {
        let quads = vec![
            q(
                "<https://example.test/iri>",
                "<https://example.test/p>",
                "_:blank",
                None,
            ),
            q(
                "_:blank",
                "<https://example.test/p>",
                "\"plain literal\"",
                None,
            ),
            q(
                "<https://example.test/lang>",
                "<https://example.test/p>",
                "\"bonjour\"@fr",
                None,
            ),
            q(
                "<https://example.test/datatype>",
                "<https://example.test/p>",
                "\"42\"^^<http://www.w3.org/2001/XMLSchema#integer>",
                None,
            ),
            q(
                "<<<https://example.test/s> <https://example.test/p> \"nested\"@en>>",
                "<https://example.test/quoted>",
                "<https://example.test/iri>",
                Some("<https://example.test/graph>"),
            ),
            q(
                "<https://example.test/iri>",
                "<https://example.test/p>",
                "_:blank",
                None,
            ),
        ];

        let mut ingest = MemoryIngest::new();
        for quad in quads.iter().cloned() {
            ingest.push(quad).unwrap();
        }
        let built = ingest.finish(|_| Vec::new()).unwrap();

        let mut reference_builder = DictionaryBuilder::new();
        for (subject, predicate, object, _) in &quads {
            reference_builder.observe(subject, predicate, object);
        }
        let reference = reference_builder.build();
        let mut reference_default = Vec::new();
        let mut reference_named = BTreeMap::new();
        for (subject, predicate, object, graph) in &quads {
            let triple = reference.encode(subject, predicate, object).unwrap();
            match graph {
                None => reference_default.push(triple),
                Some(graph) => reference_named
                    .entry(graph.clone())
                    .or_insert_with(Vec::new)
                    .push(triple),
            }
        }

        assert_eq!(built.dictionary.sections(), reference.sections());
        assert!(built.dictionary.has_quoted_triples());
        assert_eq!(built.default_triples, reference_default);
        assert_eq!(built.named, reference_named);
        assert_eq!(built.stats.statements, quads.len());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn chunked_ingest_matches_memory_canonical_artifacts_across_chunk_boundaries() {
        let quads: Vec<_> = (0..25_000)
            .flat_map(|i| {
                let pad = "x".repeat(700);
                let subject = format!("<https://example.test/s/{i}/{pad}>");
                let object = format!("<https://example.test/o/{}/{}>", i % 97, pad);
                let predicate = format!("<https://example.test/p/{}>", i % 7);
                [
                    (subject.clone(), predicate.clone(), object.clone(), None),
                    (subject, predicate, object, None),
                ]
            })
            .collect();
        let mut memory = MemoryIngest::new();
        for quad in quads.iter().cloned() {
            memory.push(quad).unwrap();
        }
        let expected = memory.finish(|_| b"metadata".to_vec()).unwrap();

        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let parent = std::env::temp_dir().join(format!(
            "rete-chunked-ingest-test-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir(&parent).unwrap();
        let mut observed = None;
        for budget in [64 << 20, 256 << 20] {
            let temp = BuildTemp::new(&parent).unwrap();
            let mut chunked = ChunkedIngest::new(&temp, budget);
            for quad in quads.iter().cloned() {
                chunked.push(quad).unwrap();
            }
            let spilled = chunked.finish(|_| b"metadata".to_vec()).unwrap();
            let sections = spilled
                .dictionary
                .section_paths
                .each_ref()
                .map(|path| std::fs::read(path).unwrap());
            let mut dictionary_bytes = Vec::new();
            crate::varint::write_uvarint(&mut dictionary_bytes, 4);
            for section in &sections {
                crate::varint::write_uvarint(&mut dictionary_bytes, section.len() as u64);
                dictionary_bytes.extend_from_slice(section);
            }
            assert_eq!(
                dictionary_bytes,
                crate::file::encode_dict_container(
                    &expected.dictionary,
                    crate::file::writer_codec(),
                )
            );
            let mut triples = Vec::new();
            spilled
                .triples
                .for_each_block(127, &mut |block| {
                    triples.extend_from_slice(block);
                    Ok(())
                })
                .unwrap();
            triples.sort_unstable();
            let mut expected_triples = expected.default_triples.clone();
            expected_triples.sort_unstable();
            assert_eq!(triples, expected_triples);
            assert_eq!(spilled.metadata, b"metadata");
            assert_eq!(spilled.stats.statements, expected.stats.statements);
            assert_eq!(
                spilled.stats.default_triples,
                expected.stats.default_triples
            );
            assert_eq!(spilled.stats.named_graphs, expected.stats.named_graphs);
            assert_eq!(spilled.stats.terms, expected.stats.terms);
            assert_eq!(spilled.stats.pyramid_levels, expected.stats.pyramid_levels);
            assert_eq!(
                observed.get_or_insert(dictionary_bytes.clone()),
                &dictionary_bytes
            );
            drop(temp);
        }
        std::fs::remove_dir(&parent).unwrap();
    }

    fn sorted_dictionary_and_deduped_triples(quads: Vec<RawQuad>) -> CanonicalContent {
        let mut ingest = MemoryIngest::new();
        for quad in quads {
            ingest.push(quad).unwrap();
        }
        let built = ingest.finish(|_| Vec::new()).unwrap();
        let mut default_triples = built.default_triples;
        default_triples.sort_unstable();
        default_triples.dedup();
        let named: BTreeMap<String, Vec<(u32, u32, u32)>> = built
            .named
            .into_iter()
            .map(|(graph, mut triples)| {
                triples.sort_unstable();
                triples.dedup();
                (graph, triples)
            })
            .collect();
        (built.dictionary.sections(), default_triples, named)
    }
}
