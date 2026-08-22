// fs-worker.js — owns one WASM engine handle for the filesystem browser.
//
// A classic worker, not a module: it uses importScripts to load the
// `pkg-nomodules` build, which exposes a global `wasm_bindgen` carrying the
// `Graph` / `RemoteGraph` classes (the reduced ESM `web/pkg` build has neither).
//
// Two handles cover every way of opening an archive:
//   • wasm_bindgen.Graph(bytes)     — a small file dropped onto the page.
//   • wasm_bindgen.RemoteGraph(url) — a file on R2, read lazily over HTTP range,
//     OR a big LOCAL file registered under a `rete-local:` URL and read lazily
//     with Blob.slice() + FileReaderSync (issue #102). One reader, two
//     transports: the only difference is where the bytes come from.
// Both lazy paths read synchronously (sync XHR / FileReaderSync), which browsers
// permit only inside a worker — the reason this file exists at all.
//
// It also reports traffic. `RemoteGraph.stats()` is cumulative, so every reply
// carries the running byte/request totals: the page can honestly say "you
// browsed 17 GB and downloaded 400 KB", which is the whole argument.

importScripts("../../../web/pkg-nomodules/rete_wasm.js");

// The engine calls this with the byte count of each completed range fetch.
// Forwarding it gives the UI a live counter that ticks while a listing loads,
// instead of a spinner that says nothing.
let liveBytes = 0;
let liveRequests = 0;
self.reteProgress = function (bytes) {
  liveBytes += Number(bytes) || 0;
  liveRequests += 1;
  self.postMessage({ type: "progress", bytes: liveBytes, requests: liveRequests });
};

const ready = wasm_bindgen({ module_or_path: "../../../web/pkg-nomodules/rete_wasm_bg.wasm" });

let graph = null;
let remote = false;

function stats() {
  if (!graph || !remote || typeof graph.stats !== "function") return null;
  try {
    return JSON.parse(graph.stats());
  } catch (_) {
    return null;
  }
}

self.onmessage = async (e) => {
  const m = e.data || {};
  const reply = (payload) => self.postMessage({ ...payload, type: m.type, reqId: m.reqId, stats: stats() });

  try {
    await ready;

    if (m.type === "open") {
      if (m.mode === "local-lazy") {
        if (typeof wasm_bindgen.register_local_file !== "function") {
          throw new Error("this engine build cannot read a local file lazily (no register_local_file export)");
        }
        // Registered per open: bootWorker() replaces the worker (and its wasm
        // instance) on every archive.
        wasm_bindgen.register_local_file(m.url, m.file);
      }
      remote = m.mode === "remote" || m.mode === "local-lazy";
      graph = remote
        ? new wasm_bindgen.RemoteGraph(m.url)
        : new wasm_bindgen.Graph(new Uint8Array(m.bytes));

      // The baked schema is what the folder tree is built from, and reading it
      // is index-free — a few KB even on a multi-gigabyte file. A file built
      // without a pyramid has none; that is a fact about the file, not an error,
      // so report it and let the views degrade.
      let schema = null;
      let schemaError = null;
      try {
        const raw = remote
          ? wasm_bindgen.schema_url(m.url)
          : wasm_bindgen.schema_packed(new Uint8Array(m.bytes));
        schema = JSON.parse(raw);
      } catch (err) {
        schemaError = String(err && err.message ? err.message : err);
      }

      let contentHash = null;
      try {
        contentHash = typeof graph.content_hash === "function" ? graph.content_hash() : null;
      } catch (_) { /* informational only */ }

      reply({ ok: true, schema, schemaError, contentHash });
      return;
    }

    if (!graph) throw new Error("no archive open");

    if (m.type === "query") {
      reply({ ok: true, json: graph.query(m.sparql, m.format || "table") });
      return;
    }

    if (m.type === "prefix") {
      reply({ ok: true, results: JSON.parse(graph.prefix_search(m.prefix, m.limit || 40)) });
      return;
    }

    if (m.type === "text") {
      reply({ ok: true, results: JSON.parse(graph.text_search(m.words, m.containsPrefix ?? undefined, m.limit || 40)) });
      return;
    }

    if (m.type === "stats") {
      reply({ ok: true });
      return;
    }

    throw new Error(`unknown command: ${m.type}`);
  } catch (err) {
    self.postMessage({
      type: m.type,
      reqId: m.reqId,
      ok: false,
      error: String(err && err.message ? err.message : err),
      stats: stats(),
    });
  }
};
