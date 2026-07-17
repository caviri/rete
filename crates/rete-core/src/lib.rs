//! Core on-disk types for the Rete graph file format.
//!
//! See `docs/SPEC.md` for the full design. This crate owns the byte-level
//! layout (header, directory, blocks) and is deliberately free of any I/O or
//! query logic so it compiles cleanly to both native and `wasm32`.

#![deny(rustdoc::broken_intra_doc_links)]
#![warn(missing_docs)]

#[doc(hidden)]
pub mod bgp;
#[doc(hidden)]
pub mod block_cache;
#[doc(hidden)]
pub mod dict;
#[doc(hidden)]
pub mod dictionary;
#[doc(hidden)]
pub mod file;
#[doc(hidden)]
pub mod geo;
#[doc(hidden)]
pub mod header;
#[doc(hidden)]
pub mod index;
#[doc(hidden)]
pub mod extbuild;

pub mod ingest;
#[doc(hidden)]
pub mod meta;
#[cfg(feature = "parallel")]
#[doc(hidden)]
pub mod parallel;
#[doc(hidden)]
pub mod pyramid;
#[doc(hidden)]
pub mod reach;
#[doc(hidden)]
pub mod reader;
#[doc(hidden)]
pub mod reason;
#[doc(hidden)]
pub mod results;
mod row;
#[doc(hidden)]
pub mod schema_pyramid;
#[doc(hidden)]
pub mod service;
#[doc(hidden)]
pub mod shacl;
#[doc(hidden)]
pub mod sparql;
#[doc(hidden)]
pub mod terms;
#[doc(hidden)]
pub mod text_index;
#[doc(hidden)]
pub mod tiling;
#[doc(hidden)]
pub mod triples;
#[doc(hidden)]
pub mod varint;

/// Stable file-format, reader, and in-memory build API.
pub mod format {
    pub use crate::file::{
        verify, ByteRange, FileError, LayoutSegment, Rete, TermTriple, TripleProvenance,
        CODEC_NONE, CODEC_ZSTD, RDF_TYPE,
    };
    pub use crate::header::{
        Header, HeaderError, Section, SectionKind, CURRENT_FORMAT_VERSION, HEADER_LEN, MAGIC,
        MIN_STABLE_READ_VERSION,
    };
    pub use crate::ingest::{
        assemble_dataset, assemble_dataset_with, assemble_dataset_with_opts,
        assemble_dataset_with_opts_algo, parse, parse_quads, parse_rdfxml, parse_statements,
        parse_turtle, BuildStats, IngestError, RawQuad, RawTriple,
    };
}

/// Stable SPARQL, graph-pattern, federation, and result API.
pub mod query {
    pub use crate::bgp::{Binding, PatternTerm, TriplePattern};
    pub use crate::file::TripleProvenance;
    pub use crate::results::{push_json_string, results_envelope_json};
    pub use crate::service::{
        parse_sparql_json_results, sparql_json_ask, sparql_json_results, ServiceClient,
    };
    pub use crate::sparql::{
        eval_query, eval_query_reasoned, eval_select_communities, eval_sparql,
        eval_sparql_reasoned, query_predicates, routed_triple_pattern, summary_query_shape,
        CommunityPartial, CommunitySelect, QueryOutput, RoutedTriplePattern, SparqlError,
        SummaryQueryShape,
    };
}

/// Stable byte-range and lazy-open API for local or remote `.rete` files.
pub mod range {
    pub use crate::block_cache::{auto_block, BlockCacheReader, DEFAULT_BLOCK, DEFAULT_CACHE_CAP};
    pub use crate::file::{
        read_metadata_ranged, read_schema_coherence_ranged, read_schema_summary_ranged, ByteRange,
        LayoutSegment, SummaryView,
    };
    pub use crate::reader::{CountingReader, RangeReader, SliceReader};
}

/// Stable integrity and SHACL validation API.
pub mod validation {
    pub use crate::file::verify;
    pub use crate::shacl::{
        validate_shacl, DataGraph, GraphView, ReteGraph, Severity, ShaclError, ShaclShapes,
        ValidationReport, ValidationResult,
    };
}

/// Stable RDFS/OWL reasoning and schema-coherence API.
pub mod reasoning {
    pub use crate::file::{
        read_schema_coherence_ranged, schema_coherence, schema_summary, TermTriple,
    };
    pub use crate::reason::{reason, Inconsistency, Reasoning, REASON_RULESET};
    pub use crate::sparql::{eval_query_reasoned, eval_sparql_reasoned};
}

#[doc(hidden)]
pub use bgp::{eval_bgp, Binding, PatternTerm, TriplePattern};
#[doc(hidden)]
pub use block_cache::{auto_block, BlockCacheReader, DEFAULT_BLOCK, DEFAULT_CACHE_CAP};
#[doc(hidden)]
pub use dict::{DictSection, DictSectionBuilder};
#[doc(hidden)]
pub use dictionary::{Dictionary, DictionaryBuilder};
#[doc(hidden)]
pub use file::{
    build_pyramid_meta, build_pyramid_meta_algo, build_pyramid_meta_with, read_metadata_ranged,
    read_schema_coherence_ranged, read_schema_summary_ranged, schema_classes, schema_coherence,
    schema_summary, verify, write_dataset, write_dataset_with_metadata, write_file, ByteRange,
    LayoutSegment, Rete, SummaryView, TermTriple, TripleProvenance, CODEC_NONE, CODEC_ZSTD,
    DEFAULT_TILE_BUDGET, RDF_TYPE,
};
#[doc(hidden)]
pub use header::{
    Header, HeaderError, Section, SectionKind, CURRENT_FORMAT_VERSION, HEADER_LEN, MAGIC,
    MIN_STABLE_READ_VERSION,
};
#[doc(hidden)]
pub use index::{GraphIndex, GraphIndexBuilder, IndexPermutation, Pattern};
#[doc(hidden)]
pub use meta::{
    CharSet, ClassNode, ClassRelation, CommunityDescriptor, LabelEntry, LevelLinks, LevelRollup,
    PredStat, PyramidMeta,
};
#[doc(hidden)]
pub use pyramid::{
    build_dendrogram, louvain_one_level, project_graph, Dendrogram, Graph, Partition, PyramidAlgo,
};
#[doc(hidden)]
pub use reach::{batch_reach_serial, build_adjacency, reach_one};
#[doc(hidden)]
pub use reader::{CountingReader, RangeReader, SliceReader};
#[doc(hidden)]
pub use reason::{reason, Inconsistency, Reasoning, REASON_RULESET};
#[doc(hidden)]
pub use results::{push_json_string, results_envelope_json};
#[doc(hidden)]
pub use schema_pyramid::build_schema_pyramid;
#[doc(hidden)]
pub use service::{parse_sparql_json_results, sparql_json_ask, sparql_json_results, ServiceClient};
#[doc(hidden)]
pub use shacl::{
    validate_shacl, DataGraph, GraphView, ReteGraph, Severity, ShaclError, ShaclShapes,
    ValidationReport, ValidationResult,
};
#[doc(hidden)]
pub use sparql::{
    eval_query, eval_query_reasoned, eval_select_communities, eval_sparql, eval_sparql_reasoned,
    parse_select, query_predicates, routed_triple_pattern, summary_query_shape, Agg,
    CommunityPartial, CommunitySelect, FExpr, GraphTarget, GroupSpec, Op, PathAst, Plan,
    QueryOutput, Rep, RoutedTriplePattern, Select, SparqlError, SummaryQueryShape,
};
#[doc(hidden)]
pub use terms::{NodeId, ObjectId, PredicateId, SubjectId, TermToken};
#[doc(hidden)]
pub use text_index::{tokenize, TextIndex, TextIndexBuilder};
#[doc(hidden)]
pub use tiling::{choose_round_for_budget, summarize, tile_by_community, SuperEdge, Tile};
#[doc(hidden)]
pub use triples::{GroupDirectory, Triple, TripleBlock, TripleBlockBuilder, ZoneMap};
