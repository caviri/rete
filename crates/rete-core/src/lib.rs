//! Core on-disk types for the Rete graph file format.
//!
//! See `docs/SPEC.md` for the full design. This crate owns the byte-level
//! layout (header, directory, blocks) and is deliberately free of any I/O or
//! query logic so it compiles cleanly to both native and `wasm32`.

pub mod bgp;
pub mod block_cache;
pub mod dict;
pub mod dictionary;
pub mod file;
pub mod geo;
pub mod header;
pub mod index;
pub mod ingest;
pub mod meta;
#[cfg(feature = "parallel")]
pub mod parallel;
pub mod pyramid;
pub mod reach;
pub mod reader;
pub mod reason;
pub mod results;
mod row;
pub mod schema_pyramid;
pub mod shacl;
pub mod sparql;
pub mod terms;
pub mod text_index;
pub mod tiling;
pub mod triples;
pub mod varint;

pub use bgp::{eval_bgp, Binding, PatternTerm, TriplePattern};
pub use block_cache::{auto_block, BlockCacheReader, DEFAULT_BLOCK, DEFAULT_CACHE_CAP};
pub use dict::{DictSection, DictSectionBuilder};
pub use dictionary::{Dictionary, DictionaryBuilder};
pub use file::{
    build_pyramid_meta, build_pyramid_meta_algo, build_pyramid_meta_with, read_metadata_ranged,
    read_schema_coherence_ranged, read_schema_summary_ranged, schema_classes, schema_coherence,
    schema_summary, verify, write_dataset, write_dataset_with_metadata, write_file, ByteRange,
    LayoutSegment, Rete, SummaryView, TermTriple, TripleProvenance, CODEC_NONE, CODEC_ZSTD,
    DEFAULT_TILE_BUDGET, RDF_TYPE,
};
pub use header::{Header, HeaderError, Section, SectionKind, HEADER_LEN, MAGIC, VERSION};
pub use index::{GraphIndex, GraphIndexBuilder, IndexPermutation, Pattern};
pub use meta::{
    CharSet, ClassNode, ClassRelation, CommunityDescriptor, LabelEntry, LevelLinks, LevelRollup,
    PredStat, PyramidMeta,
};
pub use pyramid::{
    build_dendrogram, louvain_one_level, project_graph, Dendrogram, Graph, Partition, PyramidAlgo,
};
pub use reach::{batch_reach_serial, build_adjacency, reach_one};
pub use reader::{CountingReader, RangeReader, SliceReader};
pub use reason::{reason, Inconsistency, Reasoning, REASON_RULESET};
pub use results::{push_json_string, results_envelope_json};
pub use schema_pyramid::build_schema_pyramid;
pub use shacl::{
    validate_shacl, DataGraph, GraphView, ReteGraph, Severity, ShaclError, ShaclShapes,
    ValidationReport, ValidationResult,
};
pub use sparql::{
    eval_query, eval_select_communities, eval_sparql, parse_select, query_predicates,
    routed_triple_pattern, summary_query_shape, Agg, CommunityPartial, CommunitySelect, FExpr,
    GraphTarget, GroupSpec, Op, PathAst, Plan, QueryOutput, Rep, RoutedTriplePattern, Select,
    SparqlError, SummaryQueryShape,
};
pub use terms::{NodeId, ObjectId, PredicateId, SubjectId, TermToken};
pub use text_index::{tokenize, TextIndex, TextIndexBuilder};
pub use tiling::{choose_round_for_budget, summarize, tile_by_community, SuperEdge, Tile};
pub use triples::{GroupDirectory, Triple, TripleBlock, TripleBlockBuilder, ZoneMap};
