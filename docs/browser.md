# WASM & JavaScript API

Welcome to the browser architecture of `rete`! Using the `crates/rete-wasm` package, we compile the **exact same Rust engine** (dictionary, indexes, SPARQL, zstd) to WebAssembly. This allows web pages to query `.rete` files entirely client-side—no backend server required. 

Our fully functional serverless explorer lives in `web/index.html`.

## 1. Building the WASM Artifacts

Use the provided Docker tools to build the WebAssembly packages and regenerate the playground:

```sh
docker compose build wasm
docker compose run --rm wasm
```

> **Windows Users:** If you are using a Git worktree mounted into Linux, pass the revision explicitly:
> `docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm`

This process outputs `web/pkg` and `web/pkg-nomodules`, writes build metadata to `wasm-build.json`, and ensures the pure-Rust `ruzstd` decoder is ready for browser use.

## 2. The Stable JavaScript API

The core 1.0 stable API is built around `Graph`, `RemoteGraph`, and standalone functions for querying and building. 

**Data Types & Error Handling:**
- Functions take file bytes as `Uint8Array` and return JSON strings.
- Standard JSON envelopes include `"schemaVersion": 1`.
- Binding failures throw native JavaScript `Error` objects.

### Core Functions at a Glance

| Function | What it does |
|---|---|
| `info(bytes)` | Returns basic metadata: quads, terms, pyramid levels. |
| `query_sparql(bytes, query)` | Runs standard SPARQL and returns array of solution objects. |
| `prefix_search(bytes, prefix)` | Autocomplete for labels, incredibly fast (no literal scans). |
| `text_search(bytes, words)` | Full-text search over literals (if text-indexed). |
| `schema(bytes)` | Returns the class and relation schema profile. |
| `shacl(bytes, shapes)` | Validates the graph against SHACL Turtle shapes. |
| `progressive_query(bytes, query)` | Fast counts and aggregation by querying only the summary! |
| `communities(bytes)` | Computes Louvain community clusters. |
| `build(text, format)` | Builds a complete `.rete` file array from RDF text directly in the browser! |

### Working with Remote URLs (Workers Only)

Because WebAssembly cannot block the main thread with asynchronous `fetch` calls, we use **synchronous XHR** to achieve lazy HTTP Range reads. **This means URL-based methods must be run inside a Web Worker.**

| Remote Function | What it does |
|---|---|
| `sparql_url(url, query)` | Runs full SPARQL lazily against a remote URL. Fetches only the needed bytes! |
| `shacl_url(url, shapes)` | Validates SHACL lazily; only downloads the nodes targeted by the shapes. |
| `schema_url(url)` | Fetches the high-level schema instantly over HTTP. |

*(Host requirements: Your server must answer `Range` requests with `206 Partial Content` and send CORS headers. Silently failing ranges will trigger loud errors).*

### `Graph` vs `RemoteGraph`

Instead of calling free functions every time (which decodes dictionaries from scratch), use our stateful classes:
- **`new Graph(bytes)`**: Keeps the decoded index in memory. Subsequent queries on this object are incredibly fast.
- **`new RemoteGraph(url)`**: Keeps the block cache and faulted tiles resident. Subsequent queries on the same remote graph will heavily reuse the cache!

## 3. Caching Remote Reads

Efficiency is key. `rete-wasm` implements two layers of caching:

1. **Within a query (Block Cache):** The lazy reader fetches data in 64 KiB aligned blocks. If a query requests multiple tiles that fall in the same block, they are served from memory.
2. **Across queries (Resident Session):** Using `RemoteGraph`, all decoded dictionary chunks and index tiles survive between queries. If you run a query, refine it, and run it again, **0 new bytes** cross the wire!

**Persistent Range Cache:** 
In the playground, you can enable "Persist fetched ranges across reloads." This saves 1 MiB data blocks into IndexedDB. If you reload the page tomorrow, it will instantly serve those cached blocks instead of hitting the network.

## 4. Progressive Loading: Overview First

The "Progressive Fetch" architecture is a game-changer for UI responsiveness. 

Using `header_ranges` and `summary_overview`, the browser fetches the 1 KB header, the dictionary, and the pyramid summary (usually ~25% of the file) in exactly three tiny range requests. 
**The massive triple index is never downloaded.**

You can use `progressive_query` to instantly answer shapes like:
- `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`
- `ASK { ?s <predicate> ?o }`

If the query requires a full index scan, the progressive engine will cleanly reject it, prompting you to run a standard SPARQL query.

## 5. The Browser Playground

Our static explorer (`docs/playground.html`) requires no bundler. It opens remote files lazily, supports federation across multiple sources, and renders rich results!

### Rich Result Cells
The table views in the playground go beyond plain text:
- **Media Previews:** Renders images, audio, video, IIIF manifests, PDFs, and 3D models inline. 
- **Page Previews:** Lazily embeds a sandboxed thumbnail of a webpage when hovered.
- **Markdown:** Renders RDF literals as beautiful markdown (headings, lists, code blocks).

### Find a Term (Search)
Don't memorize IRIs. The **🔎 Find a term** button opens a fast autocomplete picker for classes, predicates, and entities. It runs synchronously for local graphs, and seamlessly utilizes HTTP-range reads for remote datasets. 

Clicking the **"values ›"** drill down on a predicate instantly shows a faceted list of all objects assigned to that predicate!
