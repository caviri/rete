# rete-wasm

`rete-wasm` exposes the Rete RDF query engine to browsers through
`wasm-bindgen`. The default build is single-threaded, works from `file://`, and
can query either an in-memory `.rete` image or a remote range-readable file.

## Build

```sh
docker compose run --rm wasm
```

From a repository checkout this is the shared producer for the browser
packages and tracked pages. See
[Contributing](https://github.com/caviri/rete/blob/main/CONTRIBUTING.md)
for worktree source-revision settings. Application authors can usually use
the [JavaScript package](https://caviri.github.io/rete/javascript.html)
without building WASM themselves.

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
