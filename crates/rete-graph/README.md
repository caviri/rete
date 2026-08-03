# rete-graph

The Rust library for [Rete](https://github.com/caviri/rete) graph files, under
the name it already carries on PyPI and npm.

```sh
pip install rete-graph      # Python
npm  install rete-graph     # JavaScript
cargo add    rete-graph     # Rust
```

All three name the same thing. This crate exists so that stays true: before it,
a Rust user following any of the project's other install instructions had to
know the crate went by a different name.

## What it is

A **facade** over [`rete-core`](https://crates.io/crates/rete-core) — every item
is re-exported, so depending on either gets you the same code. The engine, and
its documentation, live in `rete-core`.

```rust
use rete_graph::Rete;

let bytes = std::fs::read("graph.rete")?;
let graph = Rete::open(&bytes)?;
println!("{} quads", graph.dump(None).len());
```

Features (`compression` — default, `parallel`, `wasm-js`) are forwarded verbatim,
so this crate can be configured exactly like `rete-core`.

## What it is not

It does not pull in the whole workspace, because the published crates are not
interchangeable:

| crate | what it is | how you use it |
|---|---|---|
| [`rete-core`](https://crates.io/crates/rete-core) | the library | a dependency — what this facade re-exports |
| [`rete-cli`](https://crates.io/crates/rete-cli) | a **binary** | `cargo install rete-cli` |
| [`rete-wasm`](https://crates.io/crates/rete-wasm) | browser bindings | only meaningful on `wasm32` targets |

Depending on the CLI would drag an executable into your build for nothing, and
the wasm bindings are inert off `wasm32`. PyPI's and npm's `rete-graph` are
likewise the *library* for their language, not the CLI — so re-exporting the
library is the faithful mapping.

## Licence

Apache-2.0. See [LICENSE](LICENSE).
