//! Subcommand handlers for the `rete` CLI, one module per command group. The
//! crate root (`main.rs`) keeps the Clap definitions + dispatch and the shared
//! helpers; each module here owns one command's logic.

pub(crate) mod build;
pub(crate) mod communities;
pub(crate) mod export;
pub(crate) mod federate;
pub(crate) mod inspect;
pub(crate) mod reach;
pub(crate) mod reason;
