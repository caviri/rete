// yasgui-wasm worker — owns the rete WASM engine and every open graph handle.
// Classic worker (not a module): the build script prepends the wasm-bindgen
// no-modules glue, which defines the global `wasm_bindgen`. It must live in a
// worker because RemoteGraph reads lazily over *synchronous* XHR range
// requests, which browsers allow only here — never on the main thread.
//
// Protocol (request → reply carries the same `type` + `reqId`):
//   {type:"init",  wasm:ArrayBuffer}                      → {ok}
//   {type:"open",  reqId, key, mode:"remote"|"local", url?, bytes?}
//                                                         → {ok, remote, info?, stats?, openMs}
//   {type:"query", reqId, key, sparql, reason:bool}       → {ok, json, ms, traffic?, remote}
//   {type:"close", reqId, key}                            → {ok}
// Plus unsolicited {type:"progress", bytes} ticks — cumulative physical bytes
// fetched by the engine since this worker booted (the engine calls the global
// `reteProgress` hook after each completed range request).

"use strict";

let fetchedTotal = 0;
let lastTick = 0;
self.reteProgress = function (n) {
  fetchedTotal += n >>> 0;
  const now = Date.now();
  if (now - lastTick > 120) {
    lastTick = now;
    postMessage({ type: "progress", bytes: fetchedTotal });
  }
};

let ready = null;
const graphs = new Map(); // key → { handle, remote, openMs }

function readStats(g) {
  if (!g.remote) return null;
  try { return JSON.parse(g.handle.stats()); } catch (_) { return null; }
}

self.onmessage = async (e) => {
  const m = e.data || {};
  const reply = (extra) => postMessage(Object.assign({ type: m.type, reqId: m.reqId }, extra));
  try {
    if (m.type === "init") {
      ready = wasm_bindgen({ module_or_path: m.wasm });
      await ready;
      reply({ ok: true });
      return;
    }
    await ready;

    if (m.type === "open") {
      let g = graphs.get(m.key);
      if (!g) {
        const t0 = performance.now();
        g = m.mode === "remote"
          ? { handle: new wasm_bindgen.RemoteGraph(m.url), remote: true }
          : { handle: new wasm_bindgen.Graph(new Uint8Array(m.bytes)), remote: false };
        g.openMs = Math.round(performance.now() - t0);
        graphs.set(m.key, g);
      }
      let info = null;
      if (!g.remote) { try { info = JSON.parse(g.handle.info()); } catch (_) {} }
      reply({ ok: true, remote: g.remote, info, stats: readStats(g), openMs: g.openMs });
      return;
    }

    if (m.type === "query") {
      const g = graphs.get(m.key);
      if (!g) throw new Error("no dataset open for this tab — set a .rete endpoint first");
      const before = readStats(g);
      const t0 = performance.now();
      const json = m.reason
        ? g.handle.query_reasoned(m.sparql, "table")
        : g.handle.query(m.sparql, "table");
      const ms = performance.now() - t0;
      const after = readStats(g);
      const traffic = before && after
        ? { bytes: after.bytes - before.bytes, requests: after.requests - before.requests, fileLength: after.fileLength }
        : null;
      // flush any progress not yet ticked out
      postMessage({ type: "progress", bytes: fetchedTotal });
      reply({ ok: true, json, ms, traffic, remote: g.remote });
      return;
    }

    if (m.type === "close") {
      graphs.delete(m.key);
      reply({ ok: true });
      return;
    }

    throw new Error("unknown message type: " + m.type);
  } catch (err) {
    reply({ ok: false, error: String((err && err.message) || err) });
  }
};
