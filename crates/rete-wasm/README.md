# rete-wasm

`rete-wasm` exposes the Rete RDF query engine to browsers through
`wasm-bindgen`. The default build is single-threaded, works from `file://`, and
can query either an in-memory `.rete` image or a remote range-readable file.

## Build

```sh
wasm-pack build crates/rete-wasm --target web --out-dir ../../web/pkg --no-opt
```

The repository's pinned Binaryen v108 corrupts current wasm-bindgen externref
table exports, so regular browser builds must skip its post-pass. From the
repository root, `docker compose run --rm wasm` is the canonical full build.

## In-memory graph

```js
import init, { Graph } from "./pkg/rete_wasm.js";

await init();
const bytes = new Uint8Array(
  await (await fetch("./graph.rete")).arrayBuffer(),
);
const graph = new Graph(bytes);
const result = JSON.parse(
  graph.query("SELECT * WHERE { ?s ?p ?o } LIMIT 20", "json"),
);
```

## Remote graph

`RemoteGraph` opens the header and lazily fetches only the ranges touched by a
query. The URL must support HTTP ranges and browser CORS without a redirect.

```js
import init, { RemoteGraph } from "./pkg/rete_wasm.js";

await init();
const graph = new RemoteGraph("https://data.example.net/graph.rete");
const result = JSON.parse(
  graph.query("SELECT * WHERE { ?s ?p ?o } LIMIT 20", "json"),
);
```

The `threads` feature is experimental. It requires a nightly `build-std` WASM
build, `wasm-bindgen-rayon`, and a cross-origin-isolated COOP/COEP server. It is
not enabled in the default package or playground build. The `asyncify` feature
is also experimental and produces a separate artifact.

See the [browser API guide](https://caviri.github.io/rete/browser.html), the
[hosting requirements](https://caviri.github.io/rete/hosting.html), and
[docs.rs](https://docs.rs/rete-wasm).

## License

Apache-2.0. See `LICENSE`.
