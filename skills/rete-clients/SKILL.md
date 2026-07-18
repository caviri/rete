---
name: rete-clients
description: Set up a rete client in a NEW project — Python, Pyodide/JupyterLite, JavaScript/Node, browser script-tag, your own wasm build, or Rust. Use when wiring .rete querying into an application, notebook, or script; covers install, a verified first query, and the per-runtime gotchas.
---

# Configure a rete client in a new project

One engine, one file format, many runtimes. Pick by project type:

| Project | Client | Install |
|---|---|---|
| Python script / Jupyter / pipeline | `rete-graph` (PyPI) | `pip install rete-graph` |
| Browser Python (JupyterLite, marimo WASM) | same package, Pyodide wheels | `piplite`/`micropip` |
| Node app / bundled web app | `rete-graph` (npm) | `npm install rete-graph` |
| Plain HTML page | script-tag single file | one CDN `<script>` |
| Custom wasm host | `crates/rete-wasm` | `wasm-pack build` |
| Rust service / CLI tool | `rete-core` | git dependency (crates.io pending) |
| R analysis | `rete` package | see docs/r.md |

All clients share the same contract: opens are **lazy** (local positional
reads or HTTP `Range`), remote hosts must answer `206` with CORS, results
come back as parsed terms, and `stats()` shows the bytes actually fetched.

## Python

```sh
pip install rete-graph        # CPython >= 3.9, Linux/macOS/Windows wheels
```

```python
import rete_graph as rete

g = rete.open("https://data.graphplaza.com/boe/boe.rete")   # or a local path / bytes
for row in g.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 3"):
    print(row["s"].value)          # Term: .kind/.value/.datatype/.lang/.to_python()

g.query_df("SELECT …")             # pandas (pip install rete-graph[pandas])
g.card(); g.examples(); g.schema() # the file explains itself
g.shacl(shapes_ttl)                # >= 0.2.2, lazy over shape targets
rete.build(nt_text)                # or the Builder for card/examples/pyramid
```

Custom storage (authed S3/GCS): any object with `read_at(offset, length)`
and `len()` via `rete.open(reader=obj)` — fsspec plugs in directly.

## Pyodide (JupyterLite, marimo WASM)

Same package — PyPI ships `pyemscripten` wheels from 0.2.0:

```python
import piplite               # JupyterLite (micropip in raw Pyodide)
await piplite.install("rete-graph")
import rete_graph as rete    # then identical code to native Python
```

Gotchas: remote opens use synchronous XHR, allowed only in **web workers**
— JupyterLite/marimo kernels run there, so it just works; a stale cached
notebook can pin an old wheel (delete it in the file browser). No SERVICE
federation in-browser; fetches are sequential. Live demo + build tutorial:
docs → JupyterLite.

## JavaScript / Node

```sh
npm install rete-graph       # Node >= 18; TypeScript types included
```

```js
import { open, build } from "rete-graph";

const g = await open("https://data.graphplaza.com/boe/boe.rete");
for (const row of g.query("SELECT ?s ?label WHERE { ?s rdfs:label ?label } LIMIT 5")) {
  console.log(row.s.value, row.label.toJS());
}
g.stats();                   // { fileLength, bytes, requests } — verify laziness
```

Remote opens work out of the box in Node (built-in sync-fetch bridge) and
in **browser web workers**. On a browser main thread, pass bytes
(`await open(new Uint8Array(buf))`) or move the graph into a worker — the
playground's own pattern.

### Plain HTML page (p5.js-style)

```html
<script src="https://cdn.jsdelivr.net/npm/rete-graph/dist/rete-graph.min.js"></script>
<script>
  (async () => {
    const g = await rete.open(await rete.build("<urn:a> <urn:knows> <urn:b> ."));
    console.log(g.query("ASK { <urn:a> ?p ?o }")); // true
  })();
</script>
```

One self-contained file, engine embedded, global `rete`. A live worked
example (editor + D3): docs → JS lab.

## Your own wasm build

When embedding the engine in a custom wasm host, build fresh from the
crates — never copy `web/pkg` (playground artifacts follow their own
pipeline and can lag the engine):

```sh
wasm-pack build crates/rete-wasm --target web --out-dir pkg
```

`clients/js/build-wasm.sh` is the reference invocation; the exported
surface (open/query/card/schema/shacl/…) is documented in docs/browser.md.

## Rust

```toml
[dependencies]                # until the crates.io release:
rete-core = { git = "https://github.com/caviri/rete" }
```

```rust
use rete_core::format::{parse_statements, assemble_dataset, Rete};
use rete_core::query::{eval_query, QueryOutput};

let quads = parse_statements("<urn:a> <urn:knows> <urn:b> .", "nt")?;
let (bytes, _stats) = assemble_dataset(quads, br#"{"source":"example"}"#);
let graph = Rete::open(&bytes)?;
match eval_query(&graph, "SELECT ?o WHERE { <urn:a> <urn:knows> ?o }")? {
    QueryOutput::Select(vars, rows) => println!("{vars:?} {} rows", rows.len()),
    _ => unreachable!(),
}
```

Remote/lazy: implement `RangeReader` (or reuse the CLI's HTTP reader) and
`Rete::open_ranged_lazy`. Keep a wildcard arm on non-exhaustive enums.
For shell workflows the `rete` CLI covers build/query/export/serve without
writing Rust.

## No client at all

Every published dataset — and any `.rete` URL — is a standard SPARQL 1.1
endpoint via the gateway (`/sparql/<key-or-url>`), so SPARQLWrapper, rdflib,
Jena, or a `SERVICE` clause need zero rete-specific code. See the
rete-catalog skill for discovery and docs/interop.md for triple-store
round-trips.
