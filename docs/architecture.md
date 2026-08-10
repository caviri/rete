# Architecture

`rete` is a "publish-and-query" RDF stack built entirely around a single, immutable, range-readable file format. 

**The core design philosophy:** Do all the expensive graph preparation at build time, ship a single highly-cacheable artifact, and allow clients to answer complex questions using bounded byte-range reads—all without needing a database server.

## 1. Workspace Map

The repository is organized by clear responsibilities:

- **`crates/rete-core`**: The engine. Handles the file format, dictionaries, triple indexes, pyramid summaries, SPARQL evaluation, reasoning, SHACL, and HTTP range readers.
- **`crates/rete-cli`**: The command-line interface. Routes commands, renders text/JSON, and handles URLs.
- **`crates/rete-wasm`**: The browser API. Compiled via `wasm-bindgen` with no native-only dependencies.
- **`crates/docgen`**: The static documentation renderer.
- **`crates/bench`**: The dev-only benchmarking suite (compares against Oxigraph).
- **`scripts/`**: Utilities for testing, synthetic data generation, and CI automation.

## 2. The Build Pipeline

The `rete build` command converts raw RDF text into a compressed `.rete` snapshot. The pipeline is designed to be highly memory-efficient and parallelized.

### Steps:
1. **Parse & Stream:** Reads RDF input line-by-line (avoiding full-memory buffering).
2. **Intern Terms:** Replaces large text strings with compact Integer IDs using a dictionary.
3. **Build Indexes:** Generates 6 permutation indexes (`SPO`, `POS`, `OSP`, `SOP`, `PSO`, `OPS`) concurrently. This allows any join key to be pre-sorted.
4. **Compute Summaries:** Builds the pyramid summary, class schemas, and community hierarchies.
5. **Write File:** Packages the header, dictionary, indexes, summary, metadata, and content hash into a single `.rete` file.

**Memory Efficiency:** Because string statements are dropped immediately after being encoded into integer IDs, memory spikes are avoided. On a 3M triple build, peak memory is kept under 850 MiB.

## 3. File Layout

A `.rete` file is composed of several independent sections, making it perfect for partial reads over the network.

| Section | Purpose |
|---|---|
| **Header (1KB)** | Magic bytes, version, content hash, and a "section directory" mapping offsets. |
| **Dictionary** | Maps integer IDs to their actual term strings. |
| **Indexes** | The compressed triple blocks stored in all 6 permutation orders. |
| **Summary** | Index-free metadata: community graphs, predicate totals, class structures, and label indexes. |
| **Text Index (Opt-in)** | Maps specific words to subject IDs for full-text searching (`--contains`). |
| **Metadata (Opt-in)** | Dataset Card information (license, title, examples). |

## 4. The Query Pipeline

SPARQL queries are evaluated through a strict, lazy pull-pipeline:

1. **Parse:** Uses `spargebra` to parse the incoming query.
2. **Compile:** Translates the query into an internal physical plan and variable slots.
3. **Evaluate (Lazy):** Runs as a streaming pipeline of integer slot rows. Operations like `LIMIT` or `ASK` stop the underlying index scans early.
4. **Materialize (Late):** Only the final, surviving rows are converted back from Integer IDs to real text strings.

**Cost-Based Joins:** `rete` uses the pyramid summary to know the exact cardinality (count) of predicates *before* executing the query. It automatically orders joins so that the most selective, rarest patterns execute first, preventing huge intermediate memory bloats.

## 5. Result Provenance

Want to know exactly where a result came from? `rete why` exposes the physical file provenance. 

It reports exactly which permutation order (e.g., `POS`) was chosen and specifies the physical, compressed byte ranges (tiles) that were fetched to answer the query.

**Tile Pruning:** Because each compressed tile includes min/max boundary data in the file header, remote queries can mathematically prove that a tile doesn't contain matching data *before* downloading it. A sparse query might fetch **zero** index tiles.

## 6. Progressive Queries & Range Reads

Not every query needs to scan millions of rows. `rete` implements progressive query paths:

- **Summary-Only Paths:** Queries like `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }` never touch the index. They return exact answers by reading the tiny summary metadata.
- **Range Reads:** When querying a remote HTTP server, the engine fetches the 1KB header, reads the dictionary, and uses HTTP `Range` requests to pull down only the exact index tiles needed. 
- *Crucially, if a server doesn't support Range requests, `rete` explicitly fails rather than silently downloading the entire multi-gigabyte file.*

## 7. Reasoning & Validation

- **Reasoning:** `rete build --materialize` runs RDFS and OWL-RL inferences at build time, permanently baking the inferred triples directly into the snapshot.
- **Validation:** `rete shacl` reads the snapshot against a standard SHACL graph to ensure data quality and schema compliance. Both features treat the graph as a fixed, publishable state.

## 8. Browser & WebAssembly (WASM)

`rete` is built to run flawlessly in the browser. 
- The WASM build drops native dependencies (like Rayon multithreading) to remain compatible with standard browser environments.
- Results crossing the WASM boundary are serialized directly into tight JSON strings, avoiding massive DOM/Memory object allocations. This keeps the browser memory footprint low and prevents Out-Of-Memory (OOM) crashes on large queries.
