use std::collections::{BTreeMap, HashMap};

use crate::ingest::{BuildStats, RawQuad};
use crate::{Dictionary, Triple};

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

    use super::MemoryIngest;

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
