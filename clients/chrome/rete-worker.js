// retelens engine worker. Owns the rete-graph Graph (lazy HTTP Range reads
// need synchronous XHR, which only exists in workers). Classic worker so the
// single-file bundle (wasm embedded) loads with importScripts.
//
// Protocol: main -> worker  {id, op, ...args}
//           worker -> main  {id, ok:true, result} | {id, ok:false, error:{message, code, detail}}
// ops: open, card, examples, schema, info, stats, run, page, free

importScripts("rete-xhr-shim.js");   // BEFORE the bundle: the wasm looks XMLHttpRequest up at call time
importScripts("rete-cache.js");      // IndexedDB chunk cache + settings
importScripts("vendor/rete-graph.min.js"); // defines global `rete`

let graph = null;
let headBytes = null;    // first bytes of the file (magic + format byte)
let bytesMode = false;   // small files are fetched whole and opened from bytes (no lazy range reads)
let fileStats = null;    // synthetic stats for bytes mode (graph.stats() is null there)
const SMALL = 2000000;   // "small" = under about 2 MB
const PROBE = 65536;
let srcUrl = null;       // what the user asked for
let resolvedUrl = null;  // after the redirect (signed CDN URL on HF)
let nextHandle = 1;
const handles = new Map(); // id -> {kind: "rows"|"triples", rows|data, vars, query}

function coded(message, code) {
  const e = new Error(message);
  e.code = code;
  return e;
}

// ---- URL resolution -------------------------------------------------------
// HF answers resolve/main/... with 302 to a signed CDN URL. Resolve once so
// every range read goes straight to the CDN.
// One ranged GET of the first 64 KiB resolves the redirect, tells the total size (Content-Range) and, for a
// file of 64 KiB or less, already IS the file. Up to ~2 MB one more request fetches the rest; bigger files are
// opened lazily and the probe is the only waste.
async function probe(url) {
  let r;
  try { r = await fetch(url, { headers: { Range: `bytes=0-${PROBE - 1}` }, redirect: "follow" }); }
  catch (e) {
    let origin = ""; try { origin = new URL(url).origin; } catch (_) {}
    throw Object.assign(coded(`Could not read ${origin || url}: it may not allow cross-site reads, or the network is down.`, "cors"), { origin });
  }
  if (!r.ok && r.status !== 206) { try { await r.body?.cancel(); } catch (_) {} throw coded(`HTTP ${r.status} fetching ${url}`, "http"); }
  const final = r.url || url;
  const etag = r.headers.get("etag") || r.headers.get("last-modified") || "";
  const cr = r.headers.get("content-range");
  let total = cr ? parseInt(cr.split("/")[1], 10) : NaN;
  if (r.status === 200) { // server ignored Range
    const len = parseInt(r.headers.get("content-length") || "", 10);
    if (Number.isFinite(len) && len > SMALL) { try { await r.body?.cancel(); } catch (_) {} return { url: final, total: len, bytes: null, requests: 1 }; }
    const buf = new Uint8Array(await r.arrayBuffer());
    return { url: final, total: buf.length, bytes: buf, head: buf, requests: 1 };
  }
  const head = new Uint8Array(await r.arrayBuffer());
  if (!Number.isFinite(total) || total > SMALL) return { url: final, total: Number.isFinite(total) ? total : 0, bytes: null, head, requests: 1, etag, ranged: true };
  if (head.length >= total) return { url: final, total, bytes: head, head, requests: 1 };
  const r2 = await fetch(final, { headers: { Range: `bytes=${head.length}-` } });
  if (!r2.ok && r2.status !== 206) throw coded(`HTTP ${r2.status} fetching ${final}`, "http");
  const rest = new Uint8Array(await r2.arrayBuffer());
  const all = new Uint8Array(head.length + rest.length);
  all.set(head, 0); all.set(rest, head.length);
  return { url: final, total: all.length, bytes: all, head, requests: 2 };
}

// 4-byte magic "RETE"; anything else is not a .rete file (whatever the URL says)
function checkMagic(head) {
  const magic = [0x52, 0x45, 0x54, 0x45];
  if (head && head.length >= 4 && magic.every((b, i) => head[i] === b)) return;
  const hex = [...(head || []).slice(0, 8)].map((b) => b.toString(16).padStart(2, "0")).join(" ");
  throw coded(`This is not a .rete file: it starts with ${hex || "no bytes"}, not 52 45 54 45 ("RETE").`, "magic");
}
// With the chunk shim on, "bytes"/"requests" are what really went over the network; the engine's own count is
// the number of ranges it asked for.
const statsNow = () => {
  const s = graph.stats() || fileStats;
  const io = self.__reteShim && self.__reteShim.summary();
  if (!s || !io) return s;
  return { ...s, bytes: io.bytes, requests: io.requests, engineRanges: s.requests, chunk: io.chunk };
};

// ---- read chunk policy: host -> bytes (0 = Auto, the engine's own block sizes). Stored by the popup in IndexedDB.
const DEFAULT_CHUNKS = { "zenodo.org": 1048576 }; // 1 MiB: measured on wikidata.rete (test/hosts-chunks-results.json), see README
async function chunkFor(url) {
  let host = ""; try { host = new URL(url).host; } catch (_) {}
  let user = null;
  try { user = await ReteCache.getSetting("chunkPolicy"); } catch (_) {}
  const pick = (m) => (m && Object.prototype.hasOwnProperty.call(m, host) ? m[host] : undefined);
  const v = pick(user) ?? pick(DEFAULT_CHUNKS) ?? (user && user["*"] !== undefined ? user["*"] : undefined) ?? 0;
  return Number(v) > 0 ? Number(v) : 0;
}

// ---- error humanising -----------------------------------------------------
function human(e) {
  const msg = String(e && e.message ? e.message : e);
  const m = msg.match(/unsupported \.rete format (0x[0-9a-fA-F]+); this Rete build reads (0x[0-9a-fA-F]+)\.\.=(0x[0-9a-fA-F]+)/);
  if (m) {
    return {
      code: "format",
      message: `This file uses rete format ${m[1]}; this viewer reads ${m[2]} to ${m[3]}. ` +
        `Older files must be rebuilt from their RDF source (rete build).`,
      detail: msg,
    };
  }
  if (e && (e.code === "magic" || e.code === "cors")) return { code: e.code, message: msg, detail: msg, origin: e.origin || null };
  if (e && e.code === "http") return { code: "http", message: msg, detail: msg };
  if (/header/i.test(msg)) return { code: "header", message: "This does not look like a readable .rete file: " + msg, detail: msg };
  if (/NetworkError|Failed to fetch|XMLHttpRequest|network/i.test(msg)) {
    return { code: "network", message: "Could not read the file over the network (blocked, offline, or the link expired).", detail: msg };
  }
  return { code: (e && e.code) || "error", message: msg, detail: (e && e.stack) || msg };
}

const isExpired = (e) => !bytesMode && /\b(401|403|410)\b|forbidden|expired/i.test(String(e && e.message));

async function openGraph(url) {
  await rete.init();
  localFile = null;
  const p = await probe(url);
  resolvedUrl = p.url;
  headBytes = p.head;
  checkMagic(p.head);
  if (p.bytes) {
    if (self.__reteShim) self.__reteShim.disable();
    bytesMode = true;
    fileStats = { schemaVersion: 1, fileLength: p.bytes.length, bytes: p.bytes.length, requests: p.requests, base: 0 };
    graph = await rete.open(p.bytes);
  } else {
    bytesMode = false; fileStats = null;
    const shim = self.__reteShim;
    if (shim) {
      const chunk = p.ranged ? await chunkFor(resolvedUrl) : 0;
      if (chunk) await shim.configure({ url: resolvedUrl, total: p.total, etag: p.etag, head: p.head, chunk, ram: 256 * 1048576 });
      else shim.off(p.head ? p.head.length : 0);
    }
    graph = await rete.open(resolvedUrl);
  }
}

function freeAll() {
  for (const h of handles.values()) { try { h.rows?.free?.(); } catch (_) {} }
  handles.clear();
}

async function reopen() {
  freeAll();
  await openGraph(srcUrl);
}

// Run fn(); if the signed URL expired (403) re-resolve once and retry.
async function guarded(fn) {
  try { return await fn(); }
  catch (e) {
    if (!isExpired(e)) throw e;
    await reopen();
    return await fn();
  }
}

// ---- serialisation --------------------------------------------------------
const ser = (t) => (t ? { kind: t.kind, value: t.value, datatype: t.datatype || null, lang: t.lang || null } : null);
const serRow = (row, vars) => {
  const o = {};
  for (const v of vars) if (row[v]) o[v] = ser(row[v]);
  return o;
};

// ---- query form detection -------------------------------------------------
function queryForm(q) {
  const s = q
    .replace(/<[^<>\s]*>/g, "<>")   // IRIs first: they may contain '#'
    .replace(/#[^\n]*/g, " ");      // then comments
  const m = s.match(/\b(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i);
  return m ? m[1].toUpperCase() : "SELECT";
}

// ---- local files ------------------------------------------------------------
// A File from the viewer is read in place with FileReaderSync. rete-graph reads remote files through
// `new XMLHttpRequest()` (sync, Range); we give it a stand-in for URLs under LOCAL_HOST that answers
// Range requests from the File, so a huge local file opens lazily without loading it into memory.
const LOCAL_HOST = "https://retelens.local/";
let localFile = null, localInstalled = false;
function installLocalXhr() {
  if (localInstalled) return;
  localInstalled = true;
  const Real = self.XMLHttpRequest;
  class LocalAwareXHR {
    constructor() { this.real = new Real(); this.local = false; this._rt = ""; this._hdr = {}; this._resp = null; }
    set responseType(v) { this._rt = v; if (!this.local) this.real.responseType = v; }
    get responseType() { return this._rt; }
    open(method, url, async, ...rest) {
      this.local = typeof url === "string" && url.startsWith(LOCAL_HOST);
      if (this.local) return;
      this.real.open(method, url, async, ...rest);
      if (this._rt) this.real.responseType = this._rt;
    }
    setRequestHeader(k, v) { if (this.local) this._hdr[k.toLowerCase()] = v; else this.real.setRequestHeader(k, v); }
    overrideMimeType(t) { if (!this.local) this.real.overrideMimeType(t); }
    send(body) {
      if (!this.local) return this.real.send(body);
      const f = localFile, size = f.size;
      const m = /^bytes=(\d*)-(\d*)$/.exec(this._hdr.range || "");
      let a = 0, b = size - 1, status = 200;
      if (m) { a = m[1] === "" ? Math.max(0, size - parseInt(m[2], 10)) : parseInt(m[1], 10); b = m[1] !== "" && m[2] !== "" ? Math.min(parseInt(m[2], 10), size - 1) : size - 1; status = 206; }
      if (a >= size) { this._resp = { status: 416, body: new ArrayBuffer(0), headers: { "content-range": `bytes */${size}` } }; return; }
      const buf = new FileReaderSync().readAsArrayBuffer(f.slice(a, b + 1));
      this._resp = { status, body: buf, headers: { "content-length": String(buf.byteLength), "accept-ranges": "bytes", ...(status === 206 ? { "content-range": `bytes ${a}-${b}/${size}` } : {}) } };
    }
    get status() { return this.local ? (this._resp ? this._resp.status : 0) : this.real.status; }
    get statusText() { return this.local ? "" : this.real.statusText; }
    get readyState() { return this.local ? (this._resp ? 4 : 1) : this.real.readyState; }
    get response() { return this.local ? (this._rt === "arraybuffer" ? this._resp && this._resp.body : "") : this.real.response; }
    get responseText() { return this.local ? "" : this.real.responseText; }
    getResponseHeader(k) { return this.local ? ((this._resp && this._resp.headers[k.toLowerCase()]) ?? null) : this.real.getResponseHeader(k); }
    getAllResponseHeaders() { return this.local ? Object.entries((this._resp && this._resp.headers) || {}).map(([k, v]) => `${k}: ${v}`).join("\r\n") : this.real.getAllResponseHeaders(); }
    abort() { if (!this.local) this.real.abort(); }
  }
  self.XMLHttpRequest = LocalAwareXHR;
}

// ---- ops ------------------------------------------------------------------
const ops = {
  async open({ url }) {
    srcUrl = url;
    freeAll();
    await openGraph(url);
    return { resolved: resolvedUrl !== url, engine: "rete-graph __RETE_VERSION__" };
  },
  async ioStats() { const s = self.__reteShim; return s ? { io: s.io, summary: s.summary(), asked: [...s.asked], ranges: [...s.ranges], wait: s.waitMode || null, note: s.cacheNote || null, ram: s.ramBytes, sab: typeof SharedArrayBuffer } : null; },
  async openLocal({ file, lazy }) {
    if (self.__reteShim) self.__reteShim.disable();
    freeAll();
    srcUrl = null;
    await rete.init();
    installLocalXhr();
    localFile = file;
    const reader = new FileReaderSync();
    const head = new Uint8Array(reader.readAsArrayBuffer(file.slice(0, PROBE)));
    headBytes = head;
    checkMagic(head);
    if (file.size <= SMALL && !lazy) {
      const all = new Uint8Array(reader.readAsArrayBuffer(file));
      bytesMode = true;
      fileStats = { schemaVersion: 1, fileLength: all.length, bytes: all.length, requests: 1, base: 0 };
      graph = await rete.open(all);
    } else {
      bytesMode = false; fileStats = null;
      graph = await rete.open(LOCAL_HOST + "f/" + encodeURIComponent(file.name));
    }
    return { local: true, size: file.size, engine: "rete-graph __RETE_VERSION__" };
  },
  async card() { return graph.card(); },
  // Everything the sidebar needs in one round trip.
  async meta() {
    return guarded(() => ({
      card: graph.card(),
      info: graph.info(),
      contentHash: graph.contentHash(),
      exampleCount: graph.examples().length,
      format: headBytes && headBytes.length > 4 ? headBytes[4] : null,
      stats: statsNow(),
    }));
  },
  async examples() { return graph.examples(); },
  async info() { return graph.info(); },
  async schema() { return guarded(() => graph.schema()); },
  async stats() { return statsNow(); },

  // SELECT -> rows handle (exact length, paged decode); ASK -> boolean;
  // CONSTRUCT/DESCRIBE -> triples handle (columns s p o).
  async run({ query }) {
    const form = queryForm(query);
    const t0 = performance.now();
    const exec = () => {
      if (form === "SELECT") {
        const rows = graph.queryRows(query);
        const id = nextHandle++;
        handles.set(id, { kind: "rows", rows, vars: rows.vars, query });
        return { form, handle: id, vars: rows.vars, length: rows.length };
      }
      const out = graph.query(query);
      if (form === "ASK") return { form, boolean: !!out };
      // Engines up to 0.7.8 ignored a top-level LIMIT on CONSTRUCT/DESCRIBE; the engine built
      // with this extension applies it. Kept as a guard: never show more triples than asked for.
      let data = out, note = null;
      const lim = query.replace(/#[^\n]*/g, " ").match(/\bLIMIT\s+(\d+)\s*$/i);
      if (lim && out.length > +lim[1]) {
        data = out.slice(0, +lim[1]);
        note = `The engine returned ${out.length} triples for LIMIT ${lim[1]}; showing the first ${lim[1]}.`;
      }
      const id = nextHandle++;
      handles.set(id, { kind: "triples", data, vars: ["s", "p", "o"], query });
      return { form, handle: id, vars: ["s", "p", "o"], length: data.length, note };
    };
    const res = await guarded(exec);
    res.ms = performance.now() - t0;
    res.warnings = graph.lastWarnings();
    if (res.note) res.warnings.push({ severity: "hint", message: res.note });
    res.stats = statsNow();
    return res;
  },

  async page({ handle, offset, limit }) {
    const h = handles.get(handle);
    if (!h) throw coded("result handle is gone", "handle");
    const t0 = performance.now();
    let rows;
    if (h.kind === "triples") {
      rows = h.data.slice(offset, offset + limit).map((t) => ({ s: ser(t[0]), p: ser(t[1]), o: ser(t[2]) }));
    } else {
      const read = () => h.rows.page(offset, limit).map((r) => serRow(r, h.vars));
      try { rows = read(); }
      catch (e) {
        if (!isExpired(e)) throw e;
        await reopen(); // handles were freed; run the query again
        h.rows = graph.queryRows(h.query);
        handles.set(handle, h);
        rows = read();
      }
    }
    return {
      rows,
      ms: performance.now() - t0,
      warnings: h.kind === "rows" ? h.rows.lastWarnings() : graph.lastWarnings(),
      stats: statsNow(),
    };
  },

  // Search box: label prefix first, then words anywhere. Rows are {label?, subject} terms.
  async search({ text, limit }) {
    limit = limit || 200;
    const t = String(text || "").trim();
    if (!t) return { rows: [], kind: "empty" };
    const out = [];
    let kind = "prefix";
    try { for (const r of graph.prefixSearch(t, limit)) out.push({ label: r.label, subject: r.subject }); } catch (_) {}
    if (!out.length) {
      kind = "text";
      try { for (const s of graph.textSearch(t.split(/\s+/), { limit })) out.push({ label: null, subject: s }); } catch (_) {}
    }
    return { rows: out, kind, stats: statsNow() };
  },

  async free({ handle }) {
    const h = handles.get(handle);
    if (h) { try { h.rows?.free?.(); } catch (_) {} handles.delete(handle); }
    return true;
  },
};

self.onmessage = async (ev) => {
  const { id, op, ...args } = ev.data || {};
  try {
    if (!ops[op]) throw new Error("unknown op " + op);
    const result = await ops[op](args);
    self.postMessage({ id, ok: true, result });
    if (self.__reteShim) self.__reteShim.flush();
  } catch (e) {
    self.postMessage({ id, ok: false, error: human(e) });
  }
};
self.postMessage({ id: 0, ok: true, result: "ready" });
