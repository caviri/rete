# Rete: The Cloud-Native RDF Graph Format

**Status:** Stable format generation **1**, implemented (header byte `0x05`; Rete 1.x compatibility baseline)
**File extension:** `.rete` 
**Header magic:** `RETE`

> **The Pitch:** Put one file on S3, GitHub, or any HTTP server that honors `Range`. Give a client the URL. Run SPARQL. **No database server required.**
>
> It's like **Parquet** for tables and **PMTiles** for maps—but for **RDF graphs**, featuring a progressive "zoomable" summary pyramid.

---

## 1. Goals & Non-Goals

### What Rete IS (Goals)
*   **Single, Immutable File:** Queryable in-place over HTTP `Range` requests.
*   **Bounded Request Counts:** A client reads a tiny header, then fetches *only* the specific byte ranges required by the query (usually ≤4 requests).
*   **SPARQL Native:** Evaluates BGPs, joins, filters, paths, aggregates, and named graphs directly against the file.
*   **Progressive Access:** Loads a coarse summary graph first, allowing clients to drill down into regions of interest without downloading the whole file.
*   **WASM-Friendly:** The exact same query engine runs in a CLI and directly in the browser.
*   **RDF-Faithful:** Supports IRIs, blank nodes, literals (with datatypes/languages), quads, and RDF-star quoted triples.

### What Rete IS NOT (Non-Goals)
*   **Not a Mutable Database:** The file is "build once, read many," enabling aggressive compression and CDN caching. (Updates are handled via a sidecar journal in `rete serve`).
*   **Not a Real-Time Reasoner:** Inference and materialization happen at build time, not query time.

---

## 2. Core Concepts

How do we make an RDF graph range-queryable? By combining three transformations:

1.  **Dictionary Encoding:** Every IRI, literal, and blank node is mapped to a dense integer ID. The graph becomes a list of integer triples, allowing massive compression.
2.  **Permutation Indexes:** Triples are sorted and stored in all **six possible orders** (SPO, POS, OSP, SOP, PSO, OPS). This guarantees that *any* triple pattern resolves to a contiguous, pre-sorted scan.
3.  **Community Pyramid:** Nodes are clustered into a hierarchy. Level 0 is a coarse summary of super-nodes and super-edges. Deeper levels expand into the full graph. 

---

## 3. File Layout

A `.rete` file is designed so that a single 1 KB header read reveals where everything else lives.

```text
┌──────────────────────────────────────────────────────────────┐
│ HEADER            (1 KB) Section directory + Content Hash    │
├──────────────────────────────────────────────────────────────┤
│ METADATA (Card)   (Optional) Dataset Card JSON               │
├──────────────────────────────────────────────────────────────┤
│ BUILD INFO        (Optional) Provenance, timestamps, stats   │
├──────────────────────────────────────────────────────────────┤
│ DICTIONARY        Front-coded strings (Shared, Subj, Obj, Pr)│
├──────────────────────────────────────────────────────────────┤
│ INDEX             Default graph: SPO, POS, OSP, SOP, PSO, OPS│
├──────────────────────────────────────────────────────────────┤
│ PYRAMID META      Summary superedges + Schema pyramid        │
├──────────────────────────────────────────────────────────────┤
│ NAMED GRAPHS      (Optional) Quads + per-graph permutations  │
├──────────────────────────────────────────────────────────────┤
│ TEXT INDEX        (Optional) Full-text search postings       │
├──────────────────────────────────────────────────────────────┤
│ FOOTER            (4 bytes) 'RETE' magic sentinel            │
└──────────────────────────────────────────────────────────────┘
```

### The Header (1024 bytes)
The header contains a 64-byte core (Magic bytes, format version, flags, quad/term counts, content hash) followed by a **typed section directory**. 
*   Because the directory is at the front, clients don't have to chase footers. 
*   It is zero-padded, leaving room to add new sections in the future without breaking the format.

---

## 4. The Dictionary

The dictionary translates text to integer IDs and is split into four front-coded sections:
1.  **Shared:** Terms used as both subjects and objects.
2.  **Subjects-only**
3.  **Objects-only** (includes literals)
4.  **Predicates**

**Why this matters:** Because IDs are assigned in this specific order (Shared first, then specific), the query engine can look at an integer ID and instantly know its role in the graph. 

### Chunking for HTTP
Dictionary sections are grouped into ~64 KB compressed chunks. If a query needs to resolve an IRI, the client fetches *only* the specific chunk containing that IRI, not the whole dictionary.

---

## 5. Triples & Permutations

Triples are stored in **all six permutations** (SPO, POS, OSP, SOP, PSO, OPS). 

*   **Zone Maps:** Just like Parquet, triples are split into compressed tiles (~64 KB). Each tile has a header (zone map) stating its minimum and maximum IDs. If a query is looking for a specific subject, the engine reads the zone maps and simply skips the tiles that don't contain it.
*   **Delta Encoding:** Inside a tile, triples are delta-encoded. Because they are pre-sorted, the difference between consecutive IDs is very small, allowing for highly efficient variable-integer (varint) compression.

---

## 6. The Pyramid & Semantic Zoom

The pyramid allows clients to understand the graph without downloading the index. 

### Topological Zoom (v1)
*   Built using Louvain community detection.
*   **Level 0** creates super-nodes representing entire communities. A client can fetch Level 0 to visualize the shape of the data instantly.

### Semantic Zoom (v2)
*   If the data contains `rdf:type` and `rdfs:subClassOf`, Rete builds a **Schema Pyramid**.
*   It provides a zoomable type histogram. At a high zoom, you see `Agent: 12k`. Zoom in, and it resolves into `Person: 9k, Organization: 3k`. 
*   This schema data is entirely **index-free**. A remote client can populate a faceted search UI by reading just a few kilobytes.

---

## 7. SPARQL Evaluation

Rete implements a staged SPARQL evaluator (`rete-core::sparql`) designed specifically to minimize byte-fetches over HTTP.

*   **Stage 1 (BGPs):** Basic Graph Patterns hit the permutation indexes. Using zone maps, it rapidly isolates matching triple blocks.
*   **Stage 2 (Filters & Modifiers):** Evaluates lazily. A `LIMIT 10` stops the network fetches the moment 10 rows are found.
*   **Stage 3 (Advanced):** Supports `UNION`, `OPTIONAL`, property paths (`p+`, `p*`), and named graphs (`GRAPH ?g`).
*   **Stage 4 (Aggregates):** Supports `COUNT`, `SUM`, `AVG`, `MIN`, `MAX`. Notably, basic per-predicate totals are answered directly from the summary pyramid without ever touching the index.
*   **Federation:** Supports SPARQL 1.1 `SERVICE`, allowing a query to seamlessly join data across multiple `.rete` files or external endpoints.

---

## 8. Client Access Flow

When you query a `.rete` file over HTTP, here is exactly what happens:

1.  **Read Header:** `GET bytes=0-1023`. The client learns where every section lives.
2.  **Determine Path:**
    *   *Overview query?* Fetch the Dictionary and Pyramid ranges. Return the summary. (3 requests total, Index bypassed).
    *   *Single-pattern query?* Resolve the constant in the dictionary, pick the best of the six permutations, and fetch *only* the specific triple tiles required.
    *   *Complex query?* Fetch the required Dictionary and Index tiles, evaluate the BGP, and stream the results. 
