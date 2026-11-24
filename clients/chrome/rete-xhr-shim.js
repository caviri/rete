// XMLHttpRequest shim for the engine worker. Loaded BEFORE the rete-graph bundle (importScripts).
//
// What the wasm engine does with XHR (read from vendor/rete-graph.min.js 0.7.8): `new XMLHttpRequest()` per read,
// `open("GET", url, false)` (sync, worker only), `setRequestHeader("Range", "bytes=a-b")`, `responseType = "arraybuffer"`,
// `send()`, then `status`, `response` and `getResponseHeader(name)` (Content-Range for the total). No multi-range requests,
// no body. `globalThis.reteReadMany` is a Node-only hook and stays undefined here.
//
// For a file with a chunk policy (configure()), reads of exactly that URL are answered from large aligned chunks:
// RAM LRU first, then one real sync XHR per contiguous run of missing chunks. Everything else passes straight through
// to the real XHR. A 429 is waited out (Retry-After) and retried up to 3 times, for chunked and passed-through reads alike.
(function () {
  const Real = self.XMLHttpRequest;
  if (!Real || self.__reteShim) return;
  const MiB = 1048576;
  const io = { requests: 0, bytes: 0, engineRanges: 0, ramHits: 0, idbChunks: 0, retries: 0, waitedMs: 0, passed: 0, chunkReqs: 0 };
  const asked = new Set(); // response header names the engine asked for (debug)
  const ranges = new Set(); // distinct Range header shapes (debug)
  const S = (self.__reteShim = { io, asked, ranges, cfg: null, ram: new Map(), ramBytes: 0, pending: [], touched: new Set() });

  const notice = (text) => { try { self.postMessage({ id: -2, notice: text }); } catch (_) {} };

  // ---- waiting (Atomics.wait on a SharedArrayBuffer when the worker has one, else a bounded busy loop)
  let atomicsOk = null;
  function sleep(ms) {
    ms = Math.min(Math.max(ms, 0), 90000);
    const end = Date.now() + ms;
    if (atomicsOk === null) { try { atomicsOk = typeof SharedArrayBuffer === "function" && !!new Int32Array(new SharedArrayBuffer(4)); } catch (_) { atomicsOk = false; } }
    S.waitMode = atomicsOk ? "atomics" : "busy";
    if (atomicsOk) { const ia = new Int32Array(new SharedArrayBuffer(4)); while (Date.now() < end) Atomics.wait(ia, 0, 0, Math.min(500, end - Date.now())); }
    else { while (Date.now() < end) { /* bounded busy wait */ } }
    io.waitedMs += ms;
  }
  function retryAfterMs(x) {
    const v = x.getResponseHeader("retry-after");
    let s = v == null ? NaN : Number(v);
    if (!Number.isFinite(s) && v) { const t = Date.parse(v); s = Number.isFinite(t) ? (t - Date.now()) / 1000 : NaN; }
    return (Number.isFinite(s) && s > 0 ? s : 5) * 1000;
  }
  // Wait out a 429, telling the status line each second. true = retry.
  function backoff(x, attempt, host) {
    if (attempt >= 3) return false;
    const ms = retryAfterMs(x);
    io.retries++;
    for (let left = Math.ceil(ms / 1000); left > 0; left--) { notice(`${host} rate limit, retrying in ${left} s`); sleep(Math.min(1000, ms)); }
    notice("");
    return true;
  }
  const hostOf = (u) => { try { return new URL(u).host; } catch (_) { return "server"; } };

  // ---- one real ranged read (sync), with 429 backoff
  function fetchRange(url, a, b) {
    for (let attempt = 0; ; attempt++) {
      const x = new Real();
      x.open("GET", url, false);
      x.setRequestHeader("Range", `bytes=${a}-${b}`);
      x.responseType = "arraybuffer";
      x.send();
      io.requests++; io.chunkReqs++;
      if (x.status === 429 && backoff(x, attempt, hostOf(url))) continue;
      const body = x.status === 206 || x.status === 200 ? x.response : null;
      if (body) io.bytes += body.byteLength;
      return { status: x.status, body, x };
    }
  }

  // ---- RAM chunks (LRU by insertion order of a Map)
  const key = (i) => S.cfg.fid + "#" + i;
  function ramGet(i) { const k = key(i), v = S.ram.get(k); if (v) { S.ram.delete(k); S.ram.set(k, v); } return v; }
  function ramPut(i, data) {
    const k = key(i);
    if (S.ram.has(k)) { S.ramBytes -= S.ram.get(k).byteLength; S.ram.delete(k); }
    S.ram.set(k, data); S.ramBytes += data.byteLength;
    while (S.ramBytes > S.cfg.ram && S.ram.size > 1) { const [ok, ov] = S.ram.entries().next().value; S.ram.delete(ok); S.ramBytes -= ov.byteLength; }
  }

  // chunks i0..i1 present in RAM (fetching the missing contiguous runs, one XHR each); returns false on failure
  function ensure(i0, i1) {
    const c = S.cfg, C = c.chunk;
    let i = i0;
    while (i <= i1) {
      if (ramGet(i)) { io.ramHits++; i++; continue; }
      let j = i; while (j + 1 <= i1 && !S.ram.has(key(j + 1))) j++;
      const a = i * C, b = Math.min((j + 1) * C, c.total) - 1;
      const r = fetchRange(c.url, a, b);
      if (r.status === 200) c.on = false; // the server ignores Range: stop chunking
      if (!r.body || r.status !== 206 || r.body.byteLength !== b - a + 1) { S.lastFail = { status: r.status, a, b, got: r.body && r.body.byteLength }; return false; }
      const buf = new Uint8Array(r.body);
      for (let k = i; k <= j; k++) {
        const part = buf.slice((k - i) * C, Math.min((k - i + 1) * C, buf.length));
        ramPut(k, part);
        S.pending.push({ idx: k, data: part.buffer.slice(0) });
      }
      i = j + 1;
    }
    return true;
  }

  class ReteXHR {
    constructor() { this.real = new Real(); this.mode = "pass"; this._h = {}; this._rt = ""; this._r = null; this._open = null; this._over = null; }
    set responseType(v) { this._rt = v; }
    get responseType() { return this._rt; }
    open(method, url, async, ...rest) {
      const c = S.cfg;
      this.mode = c && c.on && method === "GET" && async === false && url === c.url ? "chunk" : "pass";
      this._open = [method, url, async, ...rest]; this._h = {}; this._r = null;
      if (this.mode === "pass") this.real.open(method, url, async, ...rest);
    }
    setRequestHeader(k, v) { this._h[String(k).toLowerCase()] = v; if (this.mode === "pass") this.real.setRequestHeader(k, v); }
    overrideMimeType(t) { if (this.mode === "pass") this.real.overrideMimeType(t); }
    send(body) {
      if (this.mode === "chunk") { if (this._chunk()) return; this.mode = "pass"; this.real.open(...this._open); for (const [k, v] of Object.entries(this._h)) this.real.setRequestHeader(k, v); }
      io.passed++;
      const host = hostOf(this._open[1]);
      for (let attempt = 0; ; attempt++) {
        if (this._rt) this.real.responseType = this._rt;
        this.real.send(body);
        io.requests++;
        if (this.real.status === 429 && backoff(this.real, attempt, host)) { this.real.open(...this._open); for (const [k, v] of Object.entries(this._h)) this.real.setRequestHeader(k, v); continue; }
        break;
      }
      try { if (this.real.status < 300 && this._rt === "arraybuffer" && this.real.response) io.bytes += this.real.response.byteLength; } catch (_) {}
    }
    // answer a Range read from chunks; false = let it pass through (not a shape we handle)
    _chunk() {
      const c = S.cfg, hdr = this._h.range || "";
      ranges.add(hdr.replace(/\d+/g, "N"));
      const m = /^bytes=(\d*)-(\d*)$/.exec(hdr);
      if (!m || (m[1] === "" && m[2] === "")) return false;
      let a, b;
      if (m[1] === "") { a = Math.max(0, c.total - parseInt(m[2], 10)); b = c.total - 1; }
      else { a = parseInt(m[1], 10); b = m[2] === "" ? c.total - 1 : Math.min(parseInt(m[2], 10), c.total - 1); }
      if (a >= c.total) { this._r = { status: 416, body: new ArrayBuffer(0), headers: { "content-range": `bytes */${c.total}` } }; return true; }
      if (b - a + 1 > 128 * MiB) return false; // absurd span: let it pass through uncached
      io.engineRanges++;
      const i0 = Math.floor(a / c.chunk), i1 = Math.floor(b / c.chunk);
      if (!ensure(i0, i1)) return false;
      const out = new Uint8Array(b - a + 1);
      for (let i = i0; i <= i1; i++) {
        const ch = ramGet(i); if (!ch) return false; // evicted between ensure and use (tiny RAM cap): pass through
        S.touched.add(i);
        const base = i * c.chunk, from = Math.max(a, base), to = Math.min(b, base + ch.length - 1);
        out.set(ch.subarray(from - base, to - base + 1), from - a);
      }
      this._r = { status: 206, body: out.buffer, headers: { "content-range": `bytes ${a}-${b}/${c.total}`, "content-length": String(out.length), "accept-ranges": "bytes", "content-type": "application/octet-stream", ...(c.etag ? { etag: c.etag } : {}) } };
      return true;
    }
    get status() { return this._r ? this._r.status : this.real.status; }
    get statusText() { return this._r ? "" : this.real.statusText; }
    get readyState() { return this._r ? 4 : this.real.readyState; }
    get response() { return this._r ? (this._rt === "arraybuffer" ? this._r.body : "") : this.real.response; }
    get responseText() { return this._r ? "" : this.real.responseText; }
    getResponseHeader(k) { asked.add(String(k).toLowerCase()); return this._r ? (this._r.headers[String(k).toLowerCase()] ?? null) : this.real.getResponseHeader(k); }
    getAllResponseHeaders() { return this._r ? Object.entries(this._r.headers).map(([k, v]) => `${k}: ${v}`).join("\r\n") : this.real.getAllResponseHeaders(); }
    abort() { if (!this._r) this.real.abort(); }
  }
  self.XMLHttpRequest = ReteXHR;

  // ---- control surface for rete-worker.js
  // cfg: {url, total, etag, head(Uint8Array), chunk(bytes), ram(bytes)}; chunk 0 / no cfg = shim off for this file
  S.configure = async function (cfg) {
    S.cfg = null;
    if (!cfg || !cfg.chunk || !cfg.total) return false;
    const fid = cfg.url.split(/[?#]/)[0] + "|" + cfg.total + "|" + (cfg.etag || "") + "|" + cfg.chunk;
    S.cfg = { ...cfg, fid, on: true };
    S.ram.clear(); S.ramBytes = 0; S.pending = []; S.touched = new Set();
    Object.assign(io, { requests: 1, bytes: cfg.head ? cfg.head.length : 0, engineRanges: 0, ramHits: 0, idbChunks: 0, retries: 0, waitedMs: 0, passed: 0, chunkReqs: 0 }); // the probe counts as one request
    if (self.ReteCache) {
      try {
        const rows = await ReteCache.load(fid, Math.min(cfg.ram, 128 * MiB));
        // validate against the first response: chunk 0, when cached, must start with the bytes the probe just returned
        const first = rows.find((r) => r.idx === 0);
        const n = cfg.head ? Math.min(cfg.head.length, 65536) : 0;
        let ok = true;
        if (first && n) { const a = new Uint8Array(first.data, 0, Math.min(n, first.data.byteLength)); for (let i = 0; i < a.length; i++) if (a[i] !== cfg.head[i]) { ok = false; break; } }
        if (!ok) { await ReteCache.purge(fid); S.cacheNote = "stale cache entries dropped"; }
        else for (const r of rows.reverse()) { ramPut(r.idx, new Uint8Array(r.data)); io.idbChunks++; } // oldest first, so the LRU order survives
      } catch (e) { S.cacheNote = "cache unavailable: " + e; }
    }
    return true;
  };
  // persist what this op read (called after the reply is posted)
  S.flush = async function () {
    const c = S.cfg;
    if (!c || !self.ReteCache) return;
    const entries = S.pending.splice(0), touched = [...S.touched]; S.touched.clear();
    try { await ReteCache.save(c.fid, entries, touched.filter((i) => !entries.some((e) => e.idx === i))); } catch (e) { S.cacheNote = "cache write failed: " + e; }
  };
  // lazy file, chunking off (Auto): reads pass through but are still counted, so the status line can show real requests
  S.off = function (headLen) {
    S.cfg = null; S.lazy = true;
    Object.assign(io, { requests: 1, bytes: headLen || 0, engineRanges: 0, ramHits: 0, idbChunks: 0, retries: 0, waitedMs: 0, passed: 0, chunkReqs: 0 });
  };
  // whole-file (small) and local files: no remote reads to count
  S.disable = function () { S.cfg = null; S.lazy = false; };
  S.summary = () => (S.cfg && S.cfg.on ? { ...io, chunk: S.cfg.chunk } : S.lazy ? { ...io, chunk: 0 } : null);
})();
