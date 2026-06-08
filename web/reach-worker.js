// Reach worker (message-passing parallelism — no SharedArrayBuffer / wasm-bindgen-rayon).
//
// Each worker is its own WASM instance. The main thread partitions the seeds and
// sends one slice per worker; the worker runs the *serial* `reach` over its slice
// and posts the results back. N workers computing concurrently => real parallel
// speedup, with none of the shared-memory / cross-origin-isolation requirements
// (works on any plain static server). The cost is one wasm init + a copy of the
// file bytes per worker, which amortizes for heavy multi-seed workloads.
import init, { reach } from "./pkg/rete_wasm.js";

const ready = init(); // instantiate this worker's own wasm module once

self.onmessage = async (e) => {
  try {
    await ready;
    const { bytes, predicate, seeds, reverse, id } = e.data;
    const json = reach(bytes, predicate, JSON.stringify(seeds), reverse);
    self.postMessage({ id, ok: true, json });
  } catch (err) {
    self.postMessage({ id: e.data && e.data.id, ok: false, error: String(err) });
  }
};
