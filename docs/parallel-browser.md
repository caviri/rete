# Parallelism in the browser (experimental)

The default playground runs **single-threaded** — WASM threads need
`SharedArrayBuffer`, which only exists on a *cross-origin-isolated* page, and the
offline single-file playground can't satisfy that. This page explains the options
and documents the **experimental threaded build** that actually runs the engine
on multiple browser threads.

## The three options

| Approach | What it is | For rete | Catch |
|---|---|---|---|
| **`wasm-bindgen-rayon`** (WASM threads) | `rayon` on a pool of Web Workers over `SharedArrayBuffer` + atomics | Runs the existing `batch_reach_parallel` multicore — minimal code change | Needs **nightly + `-Z build-std`** to build, and the page must be served **cross-origin isolated** (COOP/COEP). No `file://`, no plain static host without header config. |
| **Web Workers (message passing)** | Spawn N workers, each a full wasm instance + a copy of the data; partition work, merge results | Fits batch reachability (embarrassingly parallel) | Copies the graph per worker; also effectively served-only |
| **WebGPU** | GPU compute shaders (WGSL) | Could do batch reach as sparse-matrix frontier expansion | Specialized kernels only; the dictionary/triple model isn't GPU-native; uneven browser support. A research-grade backend. |

For rete the high-leverage path is `wasm-bindgen-rayon`, because the parallel
workload (`rete_core::parallel::batch_reach_parallel`) already exists — the
browser build just needs a thread pool. **WebGPU** is a future, workload-specific
backend, not a general "make it parallel" switch.

## Why cross-origin isolation?

`SharedArrayBuffer` (which the worker pool needs to share wasm memory) is only
exposed when the page sends:

```
Cross-Origin-Opener-Policy: same-origin
Cross-Origin-Embedder-Policy: require-corp
```

A `file://` page can't be isolated, and GitHub Pages doesn't send these by
default — which is exactly why the **stable** playground stays single-threaded and
self-contained. The threaded build is a separate, served artifact.

## Running the experimental threaded build

It's gated behind the `rete-wasm` `threads` feature (off by default; never in the
normal `web/pkg` build or wasm). Everything runs in Docker.

```sh
# 1. Build the threaded wasm into web/pkg-threads (nightly + build-std + atomics):
bash scripts/build_playground_threads.sh

# 2. Stage a big graph to make threads matter, then serve cross-origin isolated:
cp data/opencitations/enriched-all.rete web/        # ~590k triples
python3 scripts/serve_coi.py 8080 web

# 3. Open the experimental page (NOT file://):
#    http://localhost:8080/playground-threads.html
```

The page calls `await initThreadPool(navigator.hardwareConcurrency)`, then
benchmarks **`reach` (serial) vs `reach_parallel` (threaded)** over many seeds,
shows the wall-time of each, the speedup, the worker count, and asserts the two
results are byte-identical before trusting the timing. It reports
`self.crossOriginIsolated` so you can confirm isolation is active.

## What was added

- **`rete-wasm` `threads` feature** → `wasm-bindgen-rayon` + `rete-core/parallel`;
  exports `init_thread_pool` and `reach_parallel` (the threaded twin of `reach`).
  Off by default — the normal build is unchanged and stays `file://`-able.
- **`scripts/build_playground_threads.sh`** — the nightly/`build-std`/atomics build.
- **`scripts/serve_coi.py`** — a Range-capable static server that adds the
  COOP/COEP headers.
- **`web/playground-threads.html`** — the served benchmark page.

This is **experimental**: it requires a specific build + serving setup and exists
to explore real in-browser multicore. The stable path remains the native
`parallel` feature (`rete reach --parallel`, the [Benchmarks](BENCHMARK.md)) and
the offline single-threaded [playground](playground.html).
