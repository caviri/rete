//! `rete-graph` — the Rust library for [Rete](https://github.com/caviri/rete)
//! graph files, under the name it already has on PyPI and npm.
//!
//! This crate is a **facade**: every item is re-exported from
//! [`rete_core`](https://docs.rs/rete-core), which is where the engine actually
//! lives and where the implementation documentation belongs. Depending on either
//! gets you the same code; this one exists so that
//!
//! ```text
//! pip install rete-graph      # Python
//! npm  install rete-graph     # JavaScript
//! cargo add    rete-graph     # Rust
//! ```
//!
//! all name the same thing. Before it existed, a Rust user following any of the
//! project's other install instructions had to know that the crate was called
//! something else.
//!
//! ## What this is NOT
//!
//! It does not pull in the whole workspace, because the three published crates
//! are not interchangeable:
//!
//! - **`rete-core`** is the library — what a Rust program depends on, and what
//!   this crate re-exports.
//! - **`rete-cli`** is a *binary*. You install it (`cargo install rete-cli`);
//!   depending on it from a library would drag an executable into your build for
//!   nothing.
//! - **`rete-wasm`** is the browser binding, meaningful only on `wasm32`
//!   targets.
//!
//! PyPI's and npm's `rete-graph` are likewise the *library* for their language,
//! not the CLI, so re-exporting the library is the faithful mapping.
//!
//! ## Features
//!
//! `compression` (default), `parallel` and `wasm-js` are forwarded verbatim to
//! `rete-core`, so this crate can be configured exactly like it.
//!
//! ## Example
//!
//! ```no_run
//! use rete_graph::Rete;
//!
//! let bytes = std::fs::read("graph.rete")?;
//! let graph = Rete::open(&bytes)?;
//! println!("{} quads", graph.dump(None).len());
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
#![doc(html_root_url = "https://docs.rs/rete-graph")]

pub use rete_core::*;

/// The `rete-core` version this facade re-exports.
///
/// Identical to [`rete_core::VERSION`] by construction — the dependency is
/// pinned with `=`, so the two can never disagree — and re-stated here because a
/// caller who only knows `rete_graph` should not have to reach for another crate
/// to answer "which engine am I on?".
pub const ENGINE_VERSION: &str = rete_core::VERSION;

#[cfg(test)]
mod tests {
    /// The facade must expose the engine, not an empty shell: build a tiny graph
    /// through the re-exported API and read it back. If `pub use` ever stopped
    /// covering the types a user needs, this would not compile.
    #[test]
    fn the_facade_round_trips_a_graph() {
        use crate::{DictionaryBuilder, GraphIndexBuilder, Rete};

        let triples = [(
            "<http://ex/alice>".to_string(),
            "<http://ex/knows>".to_string(),
            "<http://ex/bob>".to_string(),
        )];
        let mut db = DictionaryBuilder::new();
        for (s, p, o) in &triples {
            db.observe(s, p, o);
        }
        let dict = db.build();
        let mut ib = GraphIndexBuilder::new();
        for (s, p, o) in &triples {
            ib.push(dict.encode(s, p, o).unwrap());
        }
        let bytes = crate::file::write_file(&dict, &ib.build(), false, &[], 0);

        let graph = Rete::open(&bytes).unwrap();
        assert_eq!(graph.dump(None).len(), 1);
        assert_eq!(crate::ENGINE_VERSION, rete_core::VERSION);
    }
}
