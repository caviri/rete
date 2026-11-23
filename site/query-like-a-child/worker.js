// Query worker for the sketch page.
//
// The engine reads a remote file with synchronous range requests, which a
// browser allows only inside a worker. This one keeps one RemoteGraph per
// file, so the header, directories and blocks a query fetched stay for the
// next. Parts read on an earlier visit are kept in this browser by the
// landing page's range cache and answered from there.
import * as rangeCache from "../landing/rangecache.js";
import init, { RemoteGraph, register_local_file } from "../docs/engine/rete_wasm.js";

rangeCache.install();
const ready = init();
const sessions = new Map(); // url -> RemoteGraph

self.reteProgress = () => {};

function session(url) {
  let g = sessions.get(url);
  if (!g) {
    g = new RemoteGraph(url);
    sessions.set(url, g);
  }
  return g;
}
const json = (f, d) => { try { const s = f(); return s ? JSON.parse(s) : d; } catch { return d; } };

async function armed(url, g) {
  let hash = "";
  try { hash = g.content_hash(); } catch { /* an engine without it: no cache */ }
  await rangeCache.arm(url, hash);
}

const handlers = {
  async open({ url }) {
    const g = session(url);
    const card = json(() => g.card(), null);
    const info = json(() => g.info(), {});
    const s = json(() => g.stats(), {});
    await armed(url, g);
    return { card, info, fileLength: s.fileLength || 0 };
  },

  // A picked file: the Blob crosses by reference and is read range by range.
  async local({ url, file }) {
    register_local_file(url, file);
    return handlers.open({ url });
  },

  async query({ url, query }) {
    const g = session(url);
    await armed(url, g);
    const before = json(() => g.stats(), {});
    const hit0 = rangeCache.hits.bytes;
    const t0 = performance.now();
    try {
      const result = JSON.parse(g.query(query, "json"));
      const after = json(() => g.stats(), {});
      return {
        result, ms: performance.now() - t0,
        bytes: (after.bytes || 0) - (before.bytes || 0),
        requests: (after.requests || 0) - (before.requests || 0),
        savedBytes: rangeCache.hits.bytes - hit0,
      };
    } finally {
      rangeCache.flush();
    }
  },

  // Things whose label starts with `prefix`, from the file's label index.
  async search({ url, prefix, limit }) {
    const g = session(url);
    await armed(url, g);
    try {
      return { hits: JSON.parse(g.prefix_search(prefix, limit || 30)) };
    } finally {
      rangeCache.flush();
    }
  },
};

self.onmessage = async (e) => {
  const m = e.data;
  try {
    await ready;
    const value = await handlers[m.type](m);
    self.postMessage({ id: m.id, ok: true, value });
  } catch (err) {
    self.postMessage({ id: m.id, ok: false, error: String((err && err.message) || err) });
  }
};
