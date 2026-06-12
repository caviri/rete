# lance-rag — a cloud-friendly vector sidecar for `.rete` (side experiment)

> **Experimental, read-only.** Reads a `.rete` via the `rete` CLI; never modifies
> the format. Pairs the graph with a [Lance](https://lancedb.github.io/lance/)
> vector dataset for RAG-style retrieval.

## The idea

A `.rete` file answers **symbolic** questions exactly and cheaply — *what is this
node 1 hop from?* — by HTTP range reads over its permutation indexes. It says
nothing about **meaning**. Lance is the complement: a **cloud-native columnar
vector format** (range-read from S3/HF, no server) with a built-in disk-based
**ANN index** (IVF-PQ) that reads only the partitions a query touches — the same
publish-and-range-read story as `.rete` and PMTiles.

Put one **embedding per entity** in a `.lance` dataset beside the `.rete`, keyed
by the entity's IRI (the same key the dictionary and the Parquet entity tables
use). Then:

```
rete = symbolic adjacency  (exact 1-hop neighbourhood)
lance = semantic similarity (per-entity vectors + ANN)
```

**The retrieval loop** (`rag_demo.py`):
1. rete → the node's **1-hop neighbours**,
2. Lance → those neighbours' **vectors**,
3. **cluster** the neighbourhood (k-means) → a tight, de-duplicated, on-topic
   context window for an LLM instead of a hub's noisy neighbour dump,
4. *(optional)* Lance **ANN** → semantically similar entities the graph doesn't
   link (associative recall beyond the edges), flagged structural vs semantic.

This is the continuous successor to the [topic-modeling](../../docs/topic-modeling.md)
step: communities are *structural*, topics are *latent themes in text*, and
vectors make that theme axis continuous and queryable.

## Why Lance (vs the alternatives)

| | range-read S3/HF | built-in ANN | keyed lookup | browser/WASM |
|---|---|---|---|---|
| **Lance** | ✅ | ✅ (IVF-PQ/HNSW) | ✅ | ❌ (server-side only) |
| Zarr | ✅ | ❌ | ❌ (positional) | partial |
| Parquet (we ship entity tables) | ✅ | ❌ (brute-force only) | ✅ | partial |
| rete-native vector section | ✅ | would build from scratch | ✅ | ✅ |

Lance is the only off-the-shelf format that is **both** range-readable **and**
carries a real partition-selective ANN index. Its one weakness vs `.rete`/PMTiles
is **no browser/WASM** — so retrieval runs **server-side** (a script / Lambda /
worker), and a browser demo calls a thin endpoint rather than reading Lance
directly.

## Run it

```sh
docker build -t rete-lancerag -f experiments/lance-rag/Dockerfile experiments/lance-rag

# 0. build a tiny graph (the topic-modeling demo: 9 papers, 3 citation clusters)
docker run --rm -v "${PWD}:/work" -w /work rete-dev \
  ./target/release/rete build examples/papers.nt -o experiments/lance-rag/out/papers.rete

# 1. embed every entity's literal text -> a Lance vector dataset
docker run --rm -v "${PWD}:/work" -w /work rete-lancerag \
  python experiments/lance-rag/build_vectors.py experiments/lance-rag/out/papers.rete \
  -o experiments/lance-rag/out/vectors.lance

# 2. node + 1-hop retrieval, clustered, with optional ANN expansion
docker run --rm -v "${PWD}:/work" -w /work rete-lancerag \
  python experiments/lance-rag/rag_demo.py experiments/lance-rag/out/papers.rete \
  '<http://ex/p1>' --lance experiments/lance-rag/out/vectors.lance --clusters 3 --expand 5
```

For a real run, swap in `data/wikidata-100MB/wikidata.rete`, pass `--index` to
`build_vectors.py` (builds the IVF-PQ index), and publish `vectors.lance/` beside
`wikidata.rete` on HF/S3 — query it from a small script/Lambda with the `lance`
runtime (the documented serverless-on-S3 pattern).

## Pieces

| file | what |
|------|------|
| `build_vectors.py` | `rete export` → per-entity literal text → fastembed (ONNX, no torch) → Lance `(entity, label, vector)` [+ optional IVF-PQ] |
| `rag_demo.py` | rete 1-hop → Lance vectors → k-means cluster → optional ANN expand |
| `ask.py` | **GraphRAG agent query**: a question → Lance ANN ranked nodes → rete BFS **paths (triples)** connecting them |
| `Dockerfile` | lancedb + fastembed + scikit-learn (calls the mounted `rete` binary) |

## Asking questions (the agent loop)

```sh
docker run --rm -v "${PWD}:/work" -w /work rete-lancerag \
  python experiments/lance-rag/ask.py experiments/lance-rag/out/papers.rete \
  "deep learning for images" --lance experiments/lance-rag/out/vectors.lance --topk 5
```

A question → embed → **Lance ANN ranks the entity nodes** → rete BFS returns the
**path of triples** between the top node and the others. So the answer is grounded
*structure* — ranked nodes **and** how the graph connects them — which is exactly
what an LLM agent wants as context (wrap `ask.py`'s output with the Claude API to
phrase the final answer; that LLM step is deliberately out of scope here).

## Browser / playground — can this run in WASM?

**Lance has no WASM build** (lancedb/lance#680, closed *not planned*), so you
can't read a `.lance` dataset directly in the browser the way the playground
reads a `.rete`. But a **fully in-browser GraphRAG is still possible** — just with
a browser-native vector backend instead of Lance:

| step | server (this experiment) | browser / playground |
|------|--------------------------|----------------------|
| graph 1-hop / paths | `rete` CLI | **rete WASM** (already in the playground) |
| embed the question | fastembed (ONNX) | **transformers.js** (same ONNX model, in WASM) |
| vector store + ANN | **Lance** (IVF-PQ, range-read S3/HF) | a vectors **Parquet via DuckDB-WASM** (already loaded for Tables), **or** a flat `Float32Array` blob with brute-force cosine, **or** a WASM ANN lib (voy / hnswlib-wasm) |

So the split is: **Lance is the server-side publish-and-range-read format** (great
for big datasets + a Lambda/HF-Space endpoint); for the **static playground**,
export the same vectors to a DuckDB-WASM-friendly Parquet (or a small binary) and
do embedding + search entirely client-side, composed with rete's existing WASM
graph engine. The graph half is already WASM; only the vector half needs the
swap. A playground "ask a question → ranked nodes + path" panel would call:
`transformers.js(question) → DuckDB-WASM/voy ANN → rete-WASM paths` — no server.

## Caveats

- **Server-side only** (no Lance WASM) — fine for batch/RAG, not a static
  in-browser viewer like graph-map.
- **IVF-PQ build is slow/RAM-heavy** — skip it for small sets (brute-force is
  fine); build it once for large ones.
- **Two-artifact coupling** — rebuild `vectors.lance` when the `.rete` changes;
  keying by IRI (not internal dict id) keeps it portable across rebuilds.
- Vectors are only as good as the entity text — thin labels → weak embeddings.
