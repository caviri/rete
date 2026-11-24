// IndexedDB chunk cache + the small settings the engine worker needs (read chunk size per host).
// Used by rete-worker.js (importScripts) and popup.js (script tag). Extension origin only.
//
//   chunks   k = "<fid>#<idx>" -> { data: ArrayBuffer }
//   meta     k -> { k, fid, idx, size, t }      (small records: LRU order and totals without loading data)
//   settings key -> value
// fid identifies one immutable file: URL (no query) + total size + ETag.
(function () {
  const DB = "retelens-cache";
  const CAP = 1024 * 1024 * 1024; // 1 GiB
  let dbp = null;
  const req = (r) => new Promise((res, rej) => { r.onsuccess = () => res(r.result); r.onerror = () => rej(r.error); });
  const done = (tx) => new Promise((res, rej) => { tx.oncomplete = () => res(); tx.onerror = () => rej(tx.error); tx.onabort = () => rej(tx.error); });

  function db() {
    if (dbp) return dbp;
    dbp = new Promise((res, rej) => {
      const o = indexedDB.open(DB, 1);
      o.onupgradeneeded = () => {
        const d = o.result;
        d.createObjectStore("chunks");
        const m = d.createObjectStore("meta", { keyPath: "k" });
        m.createIndex("t", "t"); m.createIndex("fid", "fid");
        d.createObjectStore("settings");
      };
      o.onsuccess = () => res(o.result);
      o.onerror = () => rej(o.error);
    }).catch((e) => { dbp = null; throw e; });
    return dbp;
  }

  const Cache = {
    CAP,
    async getSetting(key) { const d = await db(); return req(d.transaction("settings").objectStore("settings").get(key)); },
    async setSetting(key, value) { const d = await db(); const tx = d.transaction("settings", "readwrite"); tx.objectStore("settings").put(value, key); return done(tx); },

    // the chunks of one file, most recently used first, up to `budget` bytes: [{idx, data: ArrayBuffer}]
    async load(fid, budget) {
      const d = await db();
      const metas = await req(d.transaction("meta").objectStore("meta").index("fid").getAll(fid));
      metas.sort((a, b) => b.t - a.t);
      const out = [];
      let used = 0;
      const tx = d.transaction("chunks");
      const store = tx.objectStore("chunks");
      const picks = [];
      for (const m of metas) { if (used + m.size > budget) continue; used += m.size; picks.push(m); }
      const rows = await Promise.all(picks.map((m) => req(store.get(m.k))));
      picks.forEach((m, i) => { if (rows[i] && rows[i].data) out.push({ idx: m.idx, data: rows[i].data }); });
      return out;
    },
    // entries: [{idx, data: ArrayBuffer(not detached)}]; touched: [idx]
    async save(fid, entries, touched) {
      if (!entries.length && !(touched && touched.length)) return;
      const d = await db();
      const tx = d.transaction(["chunks", "meta"], "readwrite");
      const chunks = tx.objectStore("chunks"), meta = tx.objectStore("meta");
      const now = Date.now();
      for (const e of entries) {
        const k = fid + "#" + e.idx;
        chunks.put({ data: e.data }, k);
        meta.put({ k, fid, idx: e.idx, size: e.data.byteLength, t: now });
      }
      for (const idx of touched || []) {
        const k = fid + "#" + idx;
        const m = await req(meta.get(k));
        if (m) { m.t = now; meta.put(m); }
      }
      await done(tx);
      await Cache.trim();
    },
    async purge(fid) {
      const d = await db();
      const metas = await req(d.transaction("meta").objectStore("meta").index("fid").getAllKeys(fid));
      const tx = d.transaction(["chunks", "meta"], "readwrite");
      for (const k of metas) { tx.objectStore("chunks").delete(k); tx.objectStore("meta").delete(k); }
      return done(tx);
    },
    // least recently used chunks go first until the total fits under the cap
    async trim(cap) {
      cap = cap || CAP;
      const d = await db();
      const all = await req(d.transaction("meta").objectStore("meta").getAll());
      let total = all.reduce((s, m) => s + m.size, 0);
      if (total <= cap) return 0;
      all.sort((a, b) => a.t - b.t);
      const tx = d.transaction(["chunks", "meta"], "readwrite");
      let n = 0;
      for (const m of all) {
        if (total <= cap) break;
        tx.objectStore("chunks").delete(m.k); tx.objectStore("meta").delete(m.k);
        total -= m.size; n++;
      }
      await done(tx);
      return n;
    },
    async usage() {
      const d = await db();
      const all = await req(d.transaction("meta").objectStore("meta").getAll());
      return { bytes: all.reduce((s, m) => s + m.size, 0), chunks: all.length, files: new Set(all.map((m) => m.fid)).size };
    },
    async clear() {
      const d = await db();
      const tx = d.transaction(["chunks", "meta"], "readwrite");
      tx.objectStore("chunks").clear(); tx.objectStore("meta").clear();
      return done(tx);
    },
  };
  (typeof self !== "undefined" ? self : window).ReteCache = Cache;
})();
