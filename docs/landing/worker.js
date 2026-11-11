// Landing-page query worker.
//
// The engine is synchronous and reads remote files with synchronous XHR
// range requests, which browsers only allow inside a worker. This worker owns
// one wasm instance and keeps one resident RemoteGraph per URL, so a second
// query on the same file reuses the header, directories and block cache the
// first one fetched.
//
// The engine calls `self.reteProgress(bytes, spans, n)` after every physical
// fetch. We forward those spans to the page, which draws them on the file
// strip while the query is still running.
import init, {
  RemoteGraph,
  register_local_file,
  build,
  query as queryBytes,
  card as cardBytes,
} from "../engine/rete_wasm.js";

const ready = init();
const sessions = new Map(); // url -> RemoteGraph
const memory = new Map();   // key -> Uint8Array (graphs built in the browser)
let live = null;            // the query in flight: { id, bytes, requests }

self.reteProgress = (bytes, spans, n) => {
  if (!live) return;
  live.requests += 1;
  live.bytes += bytes || 0;
  const out = [];
  if (spans && typeof spans.length === "number") {
    for (let i = 0; i < spans.length; i++) {
      const [a, z] = String(spans[i]).split("-").map(Number);
      if (Number.isFinite(a) && Number.isFinite(z)) out.push([a, z]);
    }
  }
  self.postMessage({
    type: "progress", id: live.id,
    bytes: live.bytes, requests: live.requests, spans: out, more: (n || 0) > out.length,
  });
};

function session(url) {
  let g = sessions.get(url);
  if (!g) {
    g = new RemoteGraph(url);
    sessions.set(url, g);
  }
  return g;
}

function parseStats(g) {
  try { return JSON.parse(g.stats()); } catch { return {}; }
}

function cardOf(g) {
  try {
    const c = g.card();
    return c ? JSON.parse(c) : null;
  } catch { return null; }
}

const handlers = {
  // Open a remote or local `.rete` and return what the header already holds.
  // The ranges read while opening are reported like any query's.
  async open({ id, url }) {
    live = { id, bytes: 0, requests: 0 };
    try {
      const g = session(url);
      const card = cardOf(g);
      const s = parseStats(g);
      return { fileLength: s.fileLength || 0, card, openBytes: s.bytes || 0, openRequests: s.requests || 0 };
    } finally {
      live = null;
    }
  },

  // A picked file: the Blob crosses by reference; nothing is read until the
  // engine asks for a range, exactly like a remote file.
  async local({ id, url, file }) {
    register_local_file(url, file);
    return handlers.open({ id, url });
  },

  // Turtle / N-Triples pasted by the visitor, built into a .rete in memory.
  async build({ key, text, format }) {
    const bytes = build(text, format);
    memory.set(key, bytes);
    let card = null;
    try { card = JSON.parse(cardBytes(bytes)); } catch { /* no card */ }
    return { fileLength: bytes.length, card, bytes: bytes.slice() };
  },

  async query({ id, url, key, query }) {
    const t0 = performance.now();
    if (key) {
      const bytes = memory.get(key);
      if (!bytes) throw new Error("that graph is gone; build it again");
      const json = queryBytes(bytes, query, "json");
      return {
        result: JSON.parse(json), ms: performance.now() - t0,
        bytes: bytes.length, requests: 0, fileLength: bytes.length, inMemory: true,
      };
    }
    const g = session(url);
    const before = parseStats(g);
    live = { id, bytes: 0, requests: 0 };
    try {
      const json = g.query(query, "json");
      const after = parseStats(g);
      return {
        result: JSON.parse(json),
        ms: performance.now() - t0,
        bytes: (after.bytes || 0) - (before.bytes || 0),
        requests: (after.requests || 0) - (before.requests || 0),
        totalBytes: after.bytes || 0,
        fileLength: after.fileLength || 0,
        base: after.base || 0,
      };
    } finally {
      live = null;
    }
  },
};

self.onmessage = async (e) => {
  const m = e.data;
  try {
    await ready;
    const value = await handlers[m.type](m);
    const transfer = value && value.bytes instanceof Uint8Array ? [value.bytes.buffer] : [];
    self.postMessage({ type: "done", id: m.id, ok: true, value }, transfer);
  } catch (err) {
    self.postMessage({ type: "done", id: m.id, ok: false, error: String(err && err.message || err) });
  }
};

self.postMessage({ type: "boot" });
