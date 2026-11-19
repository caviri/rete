// Parts of a file this page has read, kept in the visitor's browser
// (IndexedDB), so a later visit only fetches what it has not read before.
//
// The engine reads with synchronous XHR, and IndexedDB only answers
// asynchronously, so the cache cannot be asked in the middle of a query.
// Instead, once a file is open, its saved parts are loaded into memory
// (`arm`), and a thin XMLHttpRequest subclass answers a range request from
// them. A request it cannot answer goes to the network as before, and the
// answer is kept for next time.
//
// Ranges are kept exactly as the engine asked for them. Rounding them to
// fixed blocks would make a first visit read more. They are keyed by the
// file's content hash, which the engine reads from the file's header on
// every visit. A file republished at the same URL gets a new hash and starts
// empty. Any error in here falls back to a real request.

const DB_NAME = "rete-landing-ranges";
const PARTS = "parts"; // `${hash} ${offset} ${length}` -> ArrayBuffer
const FILES = "files"; // hash -> { url, used, ranges: [[offset, length, used], ...] }
const FILE_CAP = 64 * 1024 * 1024;   // loaded into memory per file, newest first
const TOTAL_CAP = 192 * 1024 * 1024; // kept on disk for all files

const files = new Map(); // url -> { hash, parts: [{ off, len, buf, used, fresh }] }
export const hits = { bytes: 0, requests: 0 };
let dbp = null;

function db() {
  if (!dbp) {
    dbp = new Promise((res, rej) => {
      const r = indexedDB.open(DB_NAME, 1);
      r.onupgradeneeded = () => {
        r.result.createObjectStore(PARTS);
        r.result.createObjectStore(FILES);
      };
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
      r.onblocked = () => rej(new Error("range cache blocked"));
    });
    dbp.catch(() => {});
  }
  return dbp;
}
const req = (r) => new Promise((res, rej) => { r.onsuccess = () => res(r.result); r.onerror = () => rej(r.error); });
const done = (tx) => new Promise((res, rej) => { tx.oncomplete = res; tx.onerror = tx.onabort = () => rej(tx.error); });
const partKey = (hash, off, len) => `${hash} ${off} ${len}`;

/** Load the saved parts of an open file. Call it after the engine has read
 *  the header (which gives the content hash) and before the first query. */
export async function arm(url, hash) {
  if (!hash || typeof indexedDB === "undefined" || !/^https?:/i.test(url)) return;
  const known = files.get(url);
  if (known && known.hash === hash) return;
  const entry = { hash, parts: [] };
  files.set(url, entry);
  try {
    const d = await db();
    const tx = d.transaction([FILES, PARTS]);
    const meta = await req(tx.objectStore(FILES).get(hash));
    if (!meta) return;
    let budget = FILE_CAP;
    const want = [...meta.ranges].sort((a, b) => b[2] - a[2]).filter(([, len]) => (budget -= len) >= 0);
    const store = tx.objectStore(PARTS);
    const bufs = await Promise.all(want.map(([off, len]) => req(store.get(partKey(hash, off, len)))));
    want.forEach(([off, len, used], i) => {
      if (bufs[i] && bufs[i].byteLength === len) entry.parts.push({ off, len, buf: bufs[i], used, fresh: false });
    });
  } catch {
    // No IndexedDB (a private window, blocked storage): every read is fetched.
  }
}

function lookup(url, start, end) {
  const entry = files.get(url);
  if (!entry) return null;
  for (const p of entry.parts) {
    if (p.off <= start && p.off + p.len > end) {
      p.used = Date.now();
      return p.buf.slice(start - p.off, end + 1 - p.off);
    }
  }
  return null;
}

function keep(url, start, end, response) {
  const entry = files.get(url);
  const len = end + 1 - start;
  if (!entry || !(response instanceof ArrayBuffer) || response.byteLength < len) return;
  entry.parts.push({ off: start, len, buf: response.slice(0, len), used: Date.now(), fresh: true });
}

/** Write what this session fetched, and trim the store to its caps. One
 *  flush runs at a time. */
let flushing = Promise.resolve();
export function flush() {
  flushing = flushing.then(write, write);
  return flushing;
}
async function write() {
  let d;
  try { d = await db(); } catch { return; }
  for (const [url, entry] of files) {
    const fresh = entry.parts.filter((p) => p.fresh);
    try {
      const tx = d.transaction([FILES, PARTS], "readwrite");
      const fs = tx.objectStore(FILES), ps = tx.objectStore(PARTS);
      const meta = (await req(fs.get(entry.hash))) || { url, used: 0, ranges: [] };
      const byKey = new Map(meta.ranges.map((r) => [`${r[0]} ${r[1]}`, r]));
      for (const p of fresh) ps.put(p.buf, partKey(entry.hash, p.off, p.len));
      for (const p of entry.parts) {
        const k = `${p.off} ${p.len}`, r = byKey.get(k);
        if (r) r[2] = Math.max(r[2], p.used);
        else if (p.fresh) byKey.set(k, [p.off, p.len, p.used]);
      }
      // Oldest first out when this file alone is over its cap.
      const ranges = [...byKey.values()].sort((a, b) => b[2] - a[2]);
      const gone = new Set();
      let sum = 0;
      meta.ranges = ranges.filter(([off, len]) => {
        sum += len;
        if (sum <= FILE_CAP) return true;
        ps.delete(partKey(entry.hash, off, len));
        gone.add(`${off} ${len}`);
        return false;
      });
      meta.url = url;
      meta.used = Math.max(meta.used, ...entry.parts.map((p) => p.used), 0);
      fs.put(meta, entry.hash);
      await done(tx);
      for (const p of fresh) p.fresh = false;
      if (gone.size) entry.parts = entry.parts.filter((p) => !gone.has(`${p.off} ${p.len}`));
    } catch { /* keep them fresh; the next flush tries again */ }
  }
  // Whole files out, least recently used first, when all of them are over the total.
  try {
    const tx = d.transaction([FILES, PARTS], "readwrite");
    const fs = tx.objectStore(FILES), ps = tx.objectStore(PARTS);
    const [hashes, metas] = await Promise.all([req(fs.getAllKeys()), req(fs.getAll())]);
    const all = hashes.map((h, i) => ({ h, m: metas[i], bytes: metas[i].ranges.reduce((s, r) => s + r[1], 0) }))
      .sort((a, b) => b.m.used - a.m.used);
    let sum = 0;
    for (const f of all) {
      sum += f.bytes;
      if (sum <= TOTAL_CAP) continue;
      for (const [off, len] of f.m.ranges) ps.delete(partKey(f.h, off, len));
      fs.delete(f.h);
    }
    await done(tx);
  } catch { /* trimmed next time */ }
}

function parseRange(v) {
  const m = /^bytes=(\d+)-(\d+)$/.exec(String(v).trim());
  if (!m) return null;
  const s = Number(m[1]), e = Number(m[2]);
  return e >= s ? [s, e] : null;
}

/** Put the caching XMLHttpRequest in place of the worker's own. */
export function install() {
  if (typeof XMLHttpRequest === "undefined" || typeof indexedDB === "undefined") return;
  const Real = XMLHttpRequest;
  class CachedXHR extends Real {
    open(method, url, async) {
      this._get = String(method).toUpperCase() === "GET" && async === false;
      this._url = String(url);
      this._range = null;
      this._hit = null;
      return super.open(method, url, async);
    }
    setRequestHeader(name, value) {
      if (String(name).toLowerCase() === "range") this._range = parseRange(value);
      return super.setRequestHeader(name, value);
    }
    send(body) {
      const r = this._get ? this._range : null;
      if (r) {
        try {
          const b = lookup(this._url, r[0], r[1]);
          if (b) {
            this._hit = b;
            hits.bytes += b.byteLength;
            hits.requests += 1;
            return;
          }
        } catch { /* fetch it */ }
      }
      super.send(body);
      if (r) {
        try { if (super.status === 206) keep(this._url, r[0], r[1], super.response); } catch { /* not kept */ }
      }
    }
    get status() { return this._hit ? 206 : super.status; }
    get statusText() { return this._hit ? "Partial Content" : super.statusText; }
    get readyState() { return this._hit ? 4 : super.readyState; }
    get response() { return this._hit ? this._hit : super.response; }
    getResponseHeader(name) {
      if (!this._hit) return super.getResponseHeader(name);
      const n = String(name).toLowerCase();
      if (n === "content-range") return `bytes ${this._range[0]}-${this._range[1]}/*`;
      if (n === "content-length") return String(this._hit.byteLength);
      return null;
    }
    getAllResponseHeaders() {
      return this._hit ? `content-range: bytes ${this._range[0]}-${this._range[1]}/*\r\n` : super.getAllResponseHeaders();
    }
  }
  self.XMLHttpRequest = CachedXHR;
}
