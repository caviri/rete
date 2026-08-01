//! Subcommand handlers for the `rete` CLI, one module per command group. The
//! crate root (`main.rs`) keeps only the Clap definitions + dispatch; each module
//! here owns one command's logic, and `render` holds the result-formatting
//! helpers shared across the query/export/communities commands.

pub(crate) mod build;
pub(crate) mod card;
pub(crate) mod communities;
pub(crate) mod cost;
pub(crate) mod estimate;
pub(crate) mod export;
pub(crate) mod federate;
pub(crate) mod inspect;
pub(crate) mod manifest;
pub(crate) mod progressive;
pub(crate) mod queries;
pub(crate) mod query;
pub(crate) mod range_source;
pub(crate) mod reach;
pub(crate) mod reason;
pub(crate) mod render;
pub(crate) mod serve;
pub(crate) mod service_http;
pub(crate) mod shacl;
pub(crate) mod url;
