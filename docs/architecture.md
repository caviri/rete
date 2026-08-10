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

## 7. A Day in the Life of a Query

What actually happens when you query a multi-gigabyte `.rete` file hosted on an HTTP server from your browser? Because `rete` is serverless, the client drives the entire process using HTTP Range requests. 

Here is the exact step-by-step execution for a query like `SELECT * WHERE { <User:123> foaf:knows ?who }`:

> [!NOTE]
> **The Goal:** Answer the query using the absolute minimum network bandwidth and memory.

### 🥾 Step 1: The Client Connects (The Header)

The client knows the URL of the `.rete` file. It needs the map of the file layout.

1. **Request:** `GET` with `Range: bytes=0-1023`
2. **Response:** The server returns exactly the first **1KB** of the file.
3. **Parse:** The client reads the magic bytes, validates the version, and parses the **Section Directory**. The client now knows the exact byte offsets and lengths of the Dictionary, the Indexes, and the Summary.

### 📖 Step 2: Term Translation (The Dictionary)

`rete` works entirely with compact Integer IDs, not text strings. The client needs to translate `<User:123>` and `foaf:knows` into numbers.

1. The client knows exactly where the Dictionary lives in the file.
2. It performs a **binary search over HTTP**. It calculates the byte range of the middle of the dictionary and fetches a small chunk.
3. It compares the string, updates its bounds, and fetches another small chunk.
4. **Result:** In a few tiny network requests, it learns that `<User:123>` = `ID 45` and `foaf:knows` = `ID 12`. 

### 🗺️ Step 3: Tile Navigation (The Indexes)

The query pattern is now `45 12 ?who` (Subject=45, Predicate=12, Object=Unknown). The client needs to find the `SPO` (Subject-Predicate-Object) index to scan for this prefix.

1. From the header, the client looks up the **SPO Index**.
2. The index isn't just raw data; it's split into compressed **Tiles**, each with a min/max Subject and Predicate boundary listed in the section directory.
3. The client does the math: *Which tiles contain `Subject >= 45` and `Subject <= 45`?* 
4. **Pruning:** It immediately proves that 99% of the file's tiles *cannot* contain the answer. 

### 🚚 Step 4: The Fetch & Decompression

1. **Request:** The client groups the matching adjacent tiles and fires off a combined HTTP `Range` request for those specific byte ranges.
2. **Response:** The server sends back only the compressed blocks.
3. **Process:** The client decompresses the tiles locally, scans the rows matching `45 12`, and extracts the `?who` Object IDs (e.g., `ID 88`, `ID 91`).

### ✨ Step 5: Materialization (Late Binding)

The query engine has the answers (`88`, `91`), but the user wants real text.

1. The client goes back to the Dictionary.
2. It looks up `ID 88` and `ID 91` (again using binary search or index jumps).
3. **Final Output:** The client returns `"<Alice>"` and `"<Bob>"`.

> [!TIP]
> **Total Cost:** Out of a 5 GB file, the client downloaded perhaps 15 KB of data in a few parallel requests, keeping memory usage flat and rendering results in milliseconds—all without a database server.

## 8. Reasoning & Validation

- **Reasoning:** `rete build --materialize` runs RDFS and OWL-RL inferences at build time, permanently baking the inferred triples directly into the snapshot.
- **Validation:** `rete shacl` reads the snapshot against a standard SHACL graph to ensure data quality and schema compliance. Both features treat the graph as a fixed, publishable state.

## 9. Browser & WebAssembly (WASM)

`rete` is built to run flawlessly in the browser. 
- The WASM build drops native dependencies (like Rayon multithreading) to remain compatible with standard browser environments.
- Results crossing the WASM boundary are serialized directly into tight JSON strings, avoiding massive DOM/Memory object allocations. This keeps the browser memory footprint low and prevents Out-Of-Memory (OOM) crashes on large queries.
