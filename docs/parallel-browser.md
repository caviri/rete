# Parallelism in the browser (experimental)

The default offline playground runs **single-threaded** — it is a single
self-contained `file://`-able page. This page explains the options and documents
the **experimental threaded playground** that actually runs the engine on
multiple browser threads, plus the shared-memory path that is scaffolded but
version-fragile.

## The three options

| Approach | What it is | For rete | Catch |
|---|---|---|---|
| **Web Workers (message passing)** ✅ | Spawn N workers, each a full wasm instance + a copy of the data; partition the seeds, run serial `reach` per slice, merge results | Fits batch reachability (embarrassingly parallel); **works on any plain static server — no special headers** | Copies the graph per worker |
| **`wasm-bindgen-rayon`** (WASM threads) | `rayon` on a pool of Web Workers over `SharedArrayBuffer` + atomics | Would run the existing `batch_reach_parallel` multicore with minimal code change | Needs **nightly + `-Z build-std`** to build, and the page must be served **cross-origin isolated** (COOP/COEP); also version-fragile (see below) |
| **WebGPU** | GPU compute shaders (WGSL) | Could do batch reach as sparse-matrix frontier expansion | Specialized kernels only; the dictionary/triple model isn't GPU-native; uneven browser support. A research-grade backend. |

The **working** in-browser-parallel demo is the **Web Worker pool**: it needs no
special serving setup, so it is the recommended approach. **WebGPU** is a future,
workload-specific backend, not a general "make it parallel" switch.

## The working demo: a Web Worker pool (message passing)

The experimental threaded playground uses a **Web Worker pool with message
passing** — no `SharedArrayBuffer`, no cross-origin isolation:

- **`web/playground-threads.html`** — the page; it spawns
  `navigator.hardwareConcurrency` workers, splits the seed list across them,
  and merges the results.
- **`web/reach-worker.js`** — each worker is its own WASM instance running
  serial `reach` over its slice of seeds.

Because it is plain message passing, **it runs on any static server** — no
`Cross-Origin-Opener-Policy` / `Cross-Origin-Embedder-Policy` headers required.

```sh
# 1. Stage a big graph to make parallelism matter:
cp data/opencitations/enriched-all.rete web/        # ~590k triples

# 2. Serve web/ with ANY static server (the bundled one works, but COOP/COEP
#    is NOT required — any plain static host is fine):
python3 scripts/serve_coi.py 8080 web

# 3. Open the experimental page (NOT file://, because workers need http(s)):
#    http://localhost:8080/playground-threads.html
```

The page benchmarks **serial `reach` vs the Web-Worker pool** over many seeds,
shows the wall-time of each, the speedup, and the worker count, and asserts the
two result sets are identical before trusting the timing.

## The scaffolded (but fragile) shared-memory path

The `wasm-bindgen-rayon` / `SharedArrayBuffer` shared-memory path is also
scaffolded, but it is **version-fragile**: it hit a
`DataCloneError: # could not be cloned` from a wasm-bindgen ↔ wasm-bindgen-rayon
version mismatch. The Web-Worker approach above is the recommended one. The
shared-memory scaffolding:

- **`rete-wasm` `threads` feature** → `wasm-bindgen-rayon` + `rete-core/parallel`;
  exports `init_thread_pool` and `reach_parallel` (the threaded twin of `reach`).
  Off by default — the normal build is unchanged and stays `file://`-able.
- **`scripts/build_playground_threads.sh`** — the nightly/`build-std`/atomics build.
- **`scripts/serve_coi.py`** — a Range-capable static server that *can* add the
  COOP/COEP headers (needed only for this shared-memory path; the Web-Worker
  demo doesn't require them).

### Why cross-origin isolation (for the shared-memory path only)

`SharedArrayBuffer` (which a shared-memory worker pool needs) is only exposed
when the page sends:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

A `file://` page can't be isolated, and GitHub Pages doesn't send these by
default — which is part of why the **stable** playground stays single-threaded and
self-contained, and why the working parallel demo uses message-passing Web
Workers instead.

## What was added

- **`web/playground-threads.html`** + **`web/reach-worker.js`** — the working
  Web-Worker-pool benchmark page (no special headers; recommended).
- **`rete-wasm` `threads` feature**, **`scripts/build_playground_threads.sh`**,
  **`scripts/serve_coi.py`** — the scaffolded `wasm-bindgen-rayon` shared-memory
  path (version-fragile; see above).

This is **experimental** and exists to explore real in-browser multicore. The
stable path remains the native `parallel` feature (`rete reach --parallel`, the
[Benchmarks](BENCHMARK.md)) and the offline single-threaded
[playground](playground.html).
