# Benchmarks

This page details the performance characteristics of Rete, covering file sizes, build times, query latency, and comparisons with other engines. 

All synthetic benchmarks use a social graph of **139,093 triples** (20k people in ~200 communities, with ~5 `knows` edges each, plus `age` and `name` literals). This data was generated via `scripts/gen_graph.py` and measured using `scripts/bench.sh` within the release-build dev container (Rust 1.92). Query timings exclude Cargo overhead.

## File Size

How does a `.rete` file compare to raw text and standard compression?

| Artifact | Bytes | vs Raw |
|---|--:|--:|
| Raw N-Triples | 8,384,101 | 1.0× |
| `gzip -9` (N-Triples) | 564,590 | 14.8× |
| **`.rete`** (zstd + pyramid summary) | **1,302,792** | **6.4×** |

**Takeaway:** A `.rete` file is about 2.3× larger than a gzipped file, but with a massive advantage: **it is queryable in place over HTTP ranges.** (Gzip requires a full download and scan). 

Stable format generation 1 stores all **six permutations** (SPO, POS, OSP, SOP, PSO, OPS), meaning each triple is indexed six ways to optimize *any* query pattern and enable sort-merge joins.

## Build Performance

### Small Graphs (139k triples)

| Step | Time |
|---|--:|
| `rete build` (parse → dict → 6 indexes → Louvain pyramid → zstd) | **320 ms** |

*(Result: 3 pyramid levels, 137,512 quads, 40,063 terms).*

### Large Graphs (3M triples / 189 MB N-Triples)

Building large graphs can be memory-intensive. By stream-parsing and dropping string statements immediately after encoding them into IDs, we've significantly reduced peak memory:

| Metric | Old Engine | Optimized Engine |
|---|--:|--:|
| **Peak RAM (VmHWM)** | 1,371 MiB | **836 MiB** (−39%) |
| **Louvain Pyramid Build** | 118.2 s | **44.1 s** (2.7× faster) |
| **Total Build Time** | ~141 s | **67 s** (~2.1× faster) |

**Pro Tip:** Set `RETE_BUILD_TIMING=1` when running `rete build` to see a phase-by-phase time breakdown.

## Query Latency (End-to-End)

These times cover the full pipeline: open, decompress, and evaluate (5-run average, including process startup, using the six-permutation format).

| Query Type | Time |
|---|--:|
| Triple pattern (`?s knows p100`) | **33 ms** |
| 2-hop BGP join (`p0 knows ?y . ?y knows ?z`) | **39 ms** |
| Property path (`p0 knows+ ?y`, reaches whole graph) | **59 ms** |
| `GROUP BY COUNT` (degree of every node) | **107 ms** |
| Per-predicate totals (**summary only, index bypassed**) | **24 ms** |

## HTTP Range Reads

A remote point query behaves like PMTiles: it is bounded.
*   **Full Queries:** Fetch the header, dictionary, and exactly the permutation blocks needed.
*   **Progressive/Overview Queries:** (`summary-url`) Fetch *only* the header, dictionary, and summary graph. For example, grabbing a summary of a 15.6 KB file takes just **2.2 KB in 3 requests**, completely bypassing the index.

## Scaling Up

At **347,884 triples** (2.5× the base benchmark), performance scales roughly linearly without pathological blowups:
*   Build: 890 ms
*   Triple query: 54 ms
*   `GROUP BY`: 261 ms
*   Predicate totals: 26 ms (remains cheap regardless of graph size)

### Billion-Triple Scale (DataCite)

The largest `.rete` built to date is the **52 GB DataCite graph** (9.83 billion triples). It remains queryable even on low-memory machines using the lazy range reader.

*(Measured with hard Docker memory caps, swap disabled)*

| Query | Memory Cap | Result |
|---|---|---|
| `SELECT ?s ... LIMIT 1` | 2 GiB | 6 s |
| `SELECT (COUNT(*) AS ?n) ...` | 2 GiB | **779,399** in 4 s |
| `SELECT ?t (COUNT(*)) ... GROUP BY ?t` | 4 GiB | **30 groups** in 131 s |
| `rete info` (Header + Card tier) | 1 GiB | ~1 s |

## The Pyramid: Cost vs. Benefit

Is the community summary pyramid worth it? It depends on your use case. Here is a breakdown using a typed ontology (8.5k to 137k triples):

*   **The Schema Pyramid is Cheap:** The schema-level zoom (which tracks ontology, not graph size) stays extremely small (~tens of KB) and fast (~20ms).
*   **The Community Pyramid Scales with Data:** Generating Louvain super-edges for large graphs adds significant build time and byte overhead.
*   **Selective Queries Gain Nothing:** If you only run highly selective SPARQL queries (e.g., finding a specific node), the community pyramid adds overhead without speeding up the query.

**Rule of Thumb:**
*   Use the default (with pyramid) for **exploration, overviews, and semantic zooming**.
*   Use `--no-pyramid` if you are exclusively serving **selective queries at scale** (it yields a smaller file and builds ~4× faster).

## Coherence Checking

How expensive is it to validate an ontology remotely? Rete splits this into tiers. Testing a synthetic medical ontology with a planted unsatisfiable class:

| Instances | File Size | Tier-0 (Index-Free Schema) | Tier-1 (Selective) | Tier-2 (Full Graph) |
|----------:|-----:|---------------------------:|-----------------------:|------------------:|
| 1,000   | 33 KB  | **986 B** · 2 req · 0.02 ms | 26 KB · 7 ms   | 33 KB · 10 ms |
| 100,000 | 9.4 MB | **8.1 KB** · 2 req · 0.09 ms | 346 KB · 1.1 s | 1.8 MB · 2.9 s |
| 500,000 | 48.8 MB| **8.1 KB** · 2 req · 0.10 ms | 1.4 MB · 7.5 s | 8.5 MB · 15.9 s |

**Takeaway:** Tier-0 checks the schema coherence using only **~8 KB and 2 range requests**, regardless of whether the graph is 10 MB or 10 GB. 

<!-- benchmark:opencitations:start -->
## Comparison vs. Oxigraph

This pits Rete (a queryable file) against [Oxigraph](https://github.com/oxigraph/oxigraph) (a full in-memory triplestore). 
*Dataset: OpenCitations (539,246 triples).*

### Load / Open Time

| Engine | Step | Time | Peak RAM |
|---|---|--:|--:|
| **Rete** | `Rete::open` (indexes pre-built in file) | **19.9 ms** | 13.35 MiB |
| Oxigraph | Bulk-load N-Triples + build indexes | 2437 ms | 144.98 MiB |

**Takeaway:** Rete opens instantly because the work was done at build time. 

### SPARQL Operator Coverage

Comparing **Rete (Eager)**, **Rete (Lazy HTTP-style)**, and **Oxigraph (In-Memory)**:

| Operator | Rete (Eager) | Rete (Lazy) | Oxigraph | Winner |
|---|--:|--:|--:|---|
| SELECT count | **3.78 ms** | 4.65 ms | 9.14 ms | **Rete** |
| VALUES | **5.22 ms** | 6.63 ms | 11.0 ms | **Rete** |
| UNION | **7.03 ms** | 7.97 ms | 12.9 ms | **Rete** |
| FILTER REGEX | 6.99 ms | 8.24 ms | **0.90 ms** | **Oxigraph** |
| Path transitive | **6.94 ms** | 8.74 ms | 14.0 ms | **Rete** |
| GROUP BY + ORDER | **3.81 ms** | 3.53 ms | 10.7 ms | **Rete** |
| ORDER + LIMIT + OFFSET | **4.05 ms** | 5.02 ms | 22.7 ms | **Rete** |

*(Note: 24/24 queries return identical row counts across both engines).*

Rete excels at aggregates, paths, and sorted pagination. Oxigraph keeps an edge on REGEX scans. Notably, Rete's *lazy remote reads* add only a ~1ms penalty over eager local reads.

### Batch Transitive Reachability

"From each of 300 seed authors, who is reachable through co-authorship?"

| Engine | Time | vs Oxigraph |
|---|--:|--:|
| Rete (Serial, 1 core) | 641 ms | ~4.7× faster |
| **Rete (Parallel, 32 cores)** | **39.0 ms** | **~77× faster** |
| Oxigraph (`coauthor+` path) | 3026 ms | Base |

Rete provides a dedicated primitive (`rete reach`) for multi-source reachability, which decimates standard property-path evaluation.

### Reproduce

```sh
# In the dev container (Docker). The OpenCitations + synthetic-enrichment data
# comes from scripts/fetch_opencitations.py + scripts/enrich.py (-> enriched-all.nt).
# Sanitize malformed compound-DOI IRIs so both engines load identical data:
grep -vE "<[^>]* [^>]*>" data/opencitations/enriched-all.nt \
  > data/opencitations/enriched-clean.nt
./target/release/rete build data/opencitations/enriched-clean.nt \
  -o data/opencitations/enriched-clean.rete

cargo build --release -p rete-bench
./target/release/rete-bench --json data/opencitations/enriched-clean.rete \
  data/opencitations/enriched-clean.nt 300 > /tmp/bench.json
# preserve the curated dataset note + run date from the existing JSON:
uv run python scripts/merge_bench_metadata.py /tmp/bench.json \
  docs/benchmark-opencitations.json --date $(date +%F)
uv run python scripts/render_benchmark_doc.py docs/benchmark-opencitations.json \
  --input docs/BENCHMARK.md --output docs/BENCHMARK.md
cargo run -p docgen
```

<!-- benchmark:opencitations:end -->

<!-- benchmark:lubm:start -->
## LUBM Benchmark

Testing the 14 standard LUBM queries on 118,680 pre-materialized triples.

| Metric | Rete | Oxigraph |
|---|--:|--:|
| **Load Time** | **6.2 ms** | 180 ms |
| Q1 (Grad students) | **0.30 ms** | 0.83 ms |
| Q4 (Professors) | **0.24 ms** | 0.49 ms |
| Q6 (All students) | 2.47 ms | **1.71 ms** |
| Q7 (Students of a professor) | 0.94 ms | **0.08 ms** |
| Q12 (Chairs) | **0.01 ms** | 0.02 ms |

*(14/14 queries return identical rows. See the raw JSON for full metrics).*

Reproduce: `cargo run --release -p rete-bench -- --json --lubm 1 > docs/benchmark-lubm.json` then re-render this doc.
<!-- benchmark:lubm:end -->

## Parallelism

Rete includes a prototype data-parallel query evaluator (opt-in via the `parallel` feature for native builds; not available in WASM). 

Testing on a **32-core machine** with 343,844 triples:

| Workload | Serial | Parallel | Speedup |
|---|--:|--:|--:|
| Predicate count | 7.08 ms | 1.44 ms | 4.92× |
| Per-subject out-degree | 7.94 ms | 4.87 ms | 1.63× |
| **Batch reachability (512 seeds)** | 9,057.8 ms | **584.6 ms** | **15.50×** |

**Takeaway:** Parallelism shines for heavy, independent tasks like batch reachability (`rete reach --parallel`). For lightweight scans, the thread fork/join overhead eats the benefits, so they are best left serial. 
