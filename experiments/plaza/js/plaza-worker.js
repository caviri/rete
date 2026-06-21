// plaza-worker.js — owns one WASM graph so the live explore panel can query and
// autocomplete a .rete file. (Classic worker, not a module: it uses
// importScripts to load the playground's current `pkg-nomodules` build, which
// exposes a global `wasm_bindgen` with `.Graph` / `.RemoteGraph` / prefix_search.
// The ESM `web/pkg` build is reduced and lacks those classes.)
//
// Two engine handles cover both delivery modes:
//   • wasm_bindgen.Graph(bytes)     — a fully-loaded local/bundled file.
//   • wasm_bindgen.RemoteGraph(url) — a remote file read lazily over HTTP range.
// The remote handle uses synchronous XHR (the engine can't block on fetch),
// which browsers allow only inside a worker — which is why this code lives here.

importScripts("../../../web/pkg-nomodules/rete_wasm.js");

// The engine calls this after each physical range fetch when present; a no-op
// keeps it happy without wiring up progress UI for this sketch.
self.reteProgress = function () {};

const ready = wasm_bindgen({ module_or_path: "../../../web/pkg-nomodules/rete_wasm_bg.wasm" });
let graph = null;

self.onmessage = async (e) => {
  const m = e.data || {};
  try {
    await ready;

    if (m.type === "open") {
      graph =
        m.mode === "remote"
          ? new wasm_bindgen.RemoteGraph(m.url)
          : new wasm_bindgen.Graph(new Uint8Array(m.bytes));
      let info = null;
      try { info = graph.info ? JSON.parse(graph.info()) : null; } catch (_) {}
      self.postMessage({ type: "opened", ok: true, info });
      return;
    }

    if (!graph) throw new Error("no dataset open");

    if (m.type === "prefix") {
      const results = JSON.parse(graph.prefix_search(m.prefix, m.limit || 25));
      self.postMessage({ type: "prefix", reqId: m.reqId, ok: true, results });
      return;
    }

    if (m.type === "query") {
      const json = graph.query(m.sparql, "table");
      self.postMessage({ type: "query", reqId: m.reqId, ok: true, json });
      return;
    }
  } catch (err) {
    self.postMessage({ type: m.type, reqId: m.reqId, ok: false, error: String(err) });
  }
};
