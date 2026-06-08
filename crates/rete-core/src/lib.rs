//! Core on-disk types for the Rete graph file format.
//!
//! See `docs/SPEC.md` for the full design. This crate owns the byte-level
//! layout (header, directory, blocks) and is deliberately free of any I/O or
//! query logic so it compiles cleanly to both native and `wasm32`.

pub mod bgp;
pub mod dict;
pub mod dictionary;
pub mod file;
pub mod header;
pub mod index;
pub mod meta;
#[cfg(feature = "parallel")]
pub mod parallel;
pub mod pyramid;
pub mod reach;
pub mod reader;
pub mod reason;
pub mod sparql;
pub mod tiling;
pub mod triples;
pub mod varint;

pub use bgp::{eval_bgp, Binding, PatternTerm, TriplePattern};
pub use dict::{DictSection, DictSectionBuilder};
pub use dictionary::{Dictionary, DictionaryBuilder};
pub use file::{
    build_pyramid_meta, schema_classes, schema_summary, verify, write_dataset,
    write_dataset_with_metadata, write_file, Rete, SummaryView, TermTriple, CODEC_NONE, CODEC_ZSTD,
    DEFAULT_TILE_BUDGET, RDF_TYPE,
};
pub use header::{Header, HeaderError, MAGIC, VERSION};
pub use index::{GraphIndex, GraphIndexBuilder, Pattern};
pub use meta::PyramidMeta;
pub use pyramid::{
    build_dendrogram, louvain_one_level, project_graph, Dendrogram, Graph, Partition,
};
pub use reach::{batch_reach_serial, build_adjacency, reach_one};
pub use reader::{CountingReader, RangeReader, SliceReader};
pub use reason::{reason, Inconsistency, Reasoning};
pub use sparql::{
    eval_query, eval_sparql, parse_select, query_predicates, Agg, FExpr, GraphTarget, GroupSpec,
    Op, PathAst, Plan, QueryOutput, Rep, Select, SparqlError,
};
pub use tiling::{choose_round_for_budget, summarize, tile_by_community, SuperEdge, Tile};
pub use triples::{Triple, TripleBlock, TripleBlockBuilder, ZoneMap};
