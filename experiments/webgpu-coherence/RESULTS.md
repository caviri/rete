# WebGPU fuzzy-coherence benchmark — first run

Run 2026-07-20 via `index.html` (serve the directory over localhost, WebGPU needs a secure context):

```
python -m http.server 8787 --directory experiments/webgpu-coherence --bind 127.0.0.1
```

Baseline is a single-threaded JS TypedArray loop, standing in for the wasm engine
(which is also single-threaded in the browser — rayon never ships in the wasm build).
Median of 5 reps. Every GPU result matched the CPU reference exactly
(kernel A: identical counts; kernel B: `maxDiff == 0`).

## Environment

| | |
|---|---|
| GPU | NVIDIA Blackwell |
| Browser | Chrome 150, Windows |
| CPU threads | 32 |
| adapter `maxBufferSize` | 2.00 GiB |
| adapter `maxStorageBufferBindingSize` | 2.00 GiB |
| wasm32 heap cap, for comparison | 4.00 GiB |

Note the adapter reports **2 GiB**, not the 4 GiB seen in some Chrome docs — so a
*single* WebGPU buffer here is smaller than the wasm32 heap. The memory win is
*many* buffers, not one big one.

## Kernel A — cross-graph fuzzy disjointness (low arithmetic intensity)

Elementwise `min(μ_G1, μ_G2)` over N aligned entities + count above τ=0.75.

| size | CPU ms | GPU total ms | upload | compute | readback | speedup (total) | speedup (compute) |
|---|---|---|---|---|---|---|---|
| 100k | 0.8 | 4.1 | 0.3 | 2.6 | 0.3 | **0.20×** | 0.31× |
| 1M | 8.3 | 4.8 | 1.2 | 3.2 | 0.3 | **1.73×** | 2.59× |
| 10M | 81.6 | 37.7 | 36.5 | 1.2 | 0.3 | **2.16×** | 68.0× |
| 30M | 257.7 | 117.4 | 110.3 | 6.5 | 0.5 | **2.20×** | 39.6× |

Total speedup plateaus at ~2.2× — and the breakdown says exactly why: at 30M,
**110 ms of the 117 ms is upload**. Compute is essentially free.

## Kernel B — fuzzy transitive-closure contradiction (high arithmetic intensity)

Max-min semiring SpMV in CSR, 8 iterations, ping-pong buffers, one upload and one readback.

| size | CPU ms | GPU total ms | upload | compute | readback | speedup (total) | speedup (compute) |
|---|---|---|---|---|---|---|---|
| 10k, deg 8 (80k edges) | 5.0 | 8.5 | 3.1 | 3.0 | 2.7 | **0.59×** | 1.67× |
| 100k, deg 8 (800k edges) | 61.8 | 5.9 | 1.3 | 3.0 | 0.9 | **10.5×** | 20.6× |
| 1M, deg 8 (8M edges) | 656.1 | 42.1 | 35.3 | 3.1 | 4.8 | **15.6×** | 211.6× |
| 2M, deg 16 (32M edges) | 2357.3 | 135.4 | 113.6 | 16.4 | 3.8 | **17.4×** | 143.7× |

256M edge-visits in **16.4 ms** of GPU compute versus 2.36 s single-threaded.

## What this says

1. **The predicted contrast is real.** Low-intensity elementwise work caps at ~2×;
   iterated closure hits 15–17× end-to-end. Which coherence operation you pick
   matters more than the fact you're using a GPU.
2. **Crossover is low.** Kernel A crosses over near 1M elements; kernel B somewhere
   between 10k and 100k nodes. Real coherence checks are well past both.
3. **Upload bandwidth is the wall, not compute.** Effective `writeBuffer` throughput
   is ~2.2–2.4 GB/s. Compute-only speedups of 144–212× mean the design rule is
   **stage once, run many**: upload the aligned edge slab a single time, then run
   disjointness + asymmetry + k-hop closure + threshold sweeps against the resident
   copy. A one-shot check wastes most of the GPU.
4. **Readback was not a problem** (0.3–4.8 ms), notably better than the 5–15 ms
   figure in the literature — modern Chrome on a fast discrete GPU.

## Caveats — do not over-read these numbers

- Baseline is **JS, not wasm**. Wasm would likely be ~1.5–2× faster, so discount
  accordingly; kernel B still lands around 8–9× end-to-end.
- The graph is **uniform random**. Real causal/citation graphs are power-law, and
  thread-per-row will suffer load imbalance. Needs a merge-path or warp-per-row
  variant before trusting the number on real data.
- **Blackwell is a high-end discrete GPU.** Integrated GPUs will be far less dramatic.
- **The `.rete` extraction cost is not measured here** — decoding tiles and
  materialising the columnar slab is plausibly larger than both the CPU *and* GPU
  numbers above. That is the next thing to measure, not more kernel tuning.

## Next

1. Measure extraction: federated SPARQL → flat `(src, dst, degree, graph)` arrays out
   of two real graphs (causenet × causalgraph, or the worldcup multi-source predictions).
2. Re-run kernel B on a power-law degree distribution; fix load balancing if it regresses.
3. Compare against the real wasm engine rather than JS.
4. Only then decide whether a `coherence-gpu` backend is worth building.
