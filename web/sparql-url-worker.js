// Remote lazy-SPARQL worker.
//
// The engine is synchronous and wasm cannot block on `fetch`, so the remote
// path uses **synchronous XHR** for its byte-range reads — which browsers
// permit only inside Web Workers. This worker owns one wasm instance and runs
// `sparql_url(url, query)` per message: the engine fetches the header, the
// dictionary chunk directories and tile directories, then faults in just the
// chunks/tiles the query touches (full scans coalesce adjacent tiles into
// batched range reads). The URL resolves relative to this script.
import init, { sparql_url } from "./pkg/rete_wasm.js";

const ready = init();

self.onmessage = async (e) => {
  try {
    await ready;
    const { url, query } = e.data;
    self.postMessage({ ok: true, json: sparql_url(url, query, "table") });
  } catch (err) {
    self.postMessage({ ok: false, error: String(err) });
  }
};
