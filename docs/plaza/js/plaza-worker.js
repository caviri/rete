// plaza-worker.js — owns one WASM graph so the live explore panel can query and
// autocomplete a .rete file. (Classic worker, not a module: it uses
// importScripts to load the `pkg-nomodules` build — see ../pkg-nomodules/rete-build.json
// for its provenance — which exposes a global `wasm_bindgen` with `.Graph` /
// `.RemoteGraph`. The ESM `web/pkg` build is reduced and lacks those classes.)
//
// ── Message protocol with js/dataset.js (unchanged since the July engine) ─────
//   page -> worker
//     { type: "open", mode: "remote", url }          lazy HTTP-range handle
//     { type: "open", mode: "local",  bytes }        whole file (ArrayBuffer, transferred)
//     { type: "prefix", reqId, prefix, limit? }      label autocomplete
//     { type: "query",  reqId, sparql }              SELECT/ASK/CONSTRUCT
//   worker -> page
//     { type: "opened", ok: true,  info }            info = { quads, terms, ... } or null
//     { type: "opened", ok: false, error }           open failed (unsupported format,
//                                                    network, corrupt file): ALWAYS
//                                                    answered, never silent
//     { type: "prefix", reqId, ok: true,  results }  results = [{label, subject}, ...]
//     { type: "query",  reqId, ok: true,  json, stats }  json = engine "table" result, a string;
//                                                    stats = { bytes, requests } fetched by this
//                                                    query (remote files), or null (local)
//     { type: <request type>, reqId, ok: false, error }
//
// Engine: 0.7.x. It reads formats 0x05 .. 0x08 (0x05 is the oldest it accepts;
// 0x03/0x04 files are refused up front here with a readable message instead of
// an engine error). Result handles (query_rows) and the other 0.7 exports are
// not needed by this page: `query(sparql, "table")` is the same call as before.
//
// The remote handle uses synchronous XHR (the engine can't block on fetch),
// which browsers allow only inside a worker — which is why this code lives here.

importScripts("../pkg-nomodules/rete_wasm.js");

const MIN_FORMAT = 5; // oldest format the engine opens
const MAX_FORMAT = 8; // newest this build knows

// The engine calls this after each physical range fetch when present; a no-op
// keeps it happy without wiring up progress UI.
self.reteProgress = function () {};

const ready = wasm_bindgen({ module_or_path: "../pkg-nomodules/rete_wasm_bg.wasm" });
let graph = null;

const hex = (v) => "0x" + v.toString(16).padStart(2, "0");

/** Throw a readable error unless `head` starts like a .rete this engine can open. */
function checkHeader(head, where) {
  const b = new Uint8Array(head);
  if (b.length < 8 || String.fromCharCode(b[0], b[1], b[2], b[3]) !== "RETE") {
    throw new Error(`${where} is not a .rete file (bad magic)`);
  }
  const v = b[4];
  if (v < MIN_FORMAT) {
    throw new Error(
      `this file is format ${hex(v)}; the in-browser engine reads ${hex(MIN_FORMAT)} and newer. ` +
        `Re-convert it with a current rete CLI (rete convert) to explore it live.`
    );
  }
  if (v > MAX_FORMAT) {
    throw new Error(`this file is format ${hex(v)}, newer than the in-browser engine (${hex(MAX_FORMAT)}); reload the page for a newer build`);
  }
}

async function open(m) {
  if (m.mode === "remote") {
    // Cheap pre-flight so an unsupported format is named, not a wasm trap.
    const res = await fetch(m.url, { headers: { Range: "bytes=0-15" } });
    if (!res.ok && res.status !== 206) throw new Error(`HTTP ${res.status} fetching ${m.url}`);
    checkHeader(await res.arrayBuffer(), "the remote file");
    return new wasm_bindgen.RemoteGraph(m.url);
  }
  const bytes = new Uint8Array(m.bytes);
  checkHeader(bytes.subarray(0, 16), "the file");
  return new wasm_bindgen.Graph(bytes);
}

function readStats() {
  try { return graph && typeof graph.stats === "function" ? JSON.parse(graph.stats()) : null; } catch (_) { return null; }
}

self.onmessage = async (e) => {
  const m = e.data || {};
  try {
    await ready;

    if (m.type === "open") {
      try {
        graph = await open(m);
      } catch (err) {
        graph = null;
        // dataset.js waits for type "opened" — answer it, whatever went wrong.
        self.postMessage({ type: "opened", ok: false, error: String((err && err.message) || err) });
        return;
      }
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
      // RemoteGraph.stats() is cumulative; the difference around one query is
      // what that query physically fetched. A local Graph has no stats().
      const before = readStats();
      const json = graph.query(m.sparql, "table");
      const after = readStats();
      const stats = before && after
        ? { bytes: after.bytes - before.bytes, requests: after.requests - before.requests }
        : null;
      self.postMessage({ type: "query", reqId: m.reqId, ok: true, json, stats });
      return;
    }
  } catch (err) {
    // `ready` failing (wasm could not load) lands here too; an open message gets
    // the "opened" reply the page is waiting for.
    const type = m.type === "open" ? "opened" : m.type;
    self.postMessage({ type, reqId: m.reqId, ok: false, error: String((err && err.message) || err) });
  }
};
