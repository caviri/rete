(function () {
  "use strict";

  const CATALOG = window.RETE_PLAYGROUND_CATALOG;
  const state = {
    bytes: null,
    dataset: CATALOG.defaultDataset,
    mode: "sparql",
    family: "All",
    selectedExample: -1,
    activeSource: "bundled",
    schema: null,
    lastProgressive: null,
    lastProvenance: null,
    built: null,
    exploreClass: null,
    // A resident wasm Graph handle for in-memory queries: opened once per load so
    // repeated queries skip re-copying the buffer + re-decoding the dictionary.
    graph: null,
    exploreReady: false,
    explorePage: 0,
    exploreCount: 0,
    exploreCols: null,
    // Explore backend: "native" (rete engine) or a companion encoding queried via
    // a CDN-loaded engine ("duck-parquet" | "duck-db" | "sqlite"). Only datasets
    // with CATALOG.companions show the switch; native stays fully offline.
    exploreBackend: "native",
    exploreNativeMeta: "",
    // Explore "SQL" sub-tab: which companion engine the editor runs against, and
    // the dataset its editor content belongs to (to reset on dataset switch).
    sqlBackend: "duck-parquet",
    sqlDataset: null,
    // Persistent incremental range cache (opt-in via Settings, localStorage-backed).
    rangeCacheOn: (() => { try { return localStorage.getItem("rangeCacheOn") === "1"; } catch (e) { return false; } })(),
    remote: null,
    // Federation: extra sources the SPARQL query also runs against. Each is
    // {id, kind:"remote"|"memory"|"endpoint", label, url?, key?, endpoint?}.
    // Empty = single-source (today's behavior). A resident Graph per in-memory
    // partner is cached in fedGraphs so repeated federated runs don't re-decode.
    fedSources: [],
    fedGraphs: new Map(),
    // The last successful query result, kept so switching the Output type
    // re-renders it in the new view instead of re-running the query.
    lastResult: null
  };

  const RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

  // --- Remote lazy SPARQL worker ----------------------------------------
  // The engine is synchronous and wasm can't block on fetch, so remote
  // range-querying uses synchronous XHR — allowed only inside a Web Worker.
  // We build that worker from the page's own inlined wasm glue (the #reteGlue
  // script's source) plus a tiny harness, so the offline single-file page
  // gains real lazy remote querying with no extra files. Same mechanism
  // DuckDB uses over httpfs: fetch only the bytes a query touches.
  const REMOTE_HARNESS = `
;(function () {
  var ready = null, pReq = 0, pBytes = 0, pId = 0, qStart = 0, fetchLog = [];
  // Resident remote sessions: a RemoteGraph that keeps the file's block cache +
  // faulted index tiles + decoded dictionary alive across queries, so re-running
  // or refining a query on the same dataset refetches almost nothing. Keyed by
  // the file's CONTENT HASH (not the URL), so two URLs of the same file share one
  // cache — the same key a future IndexedDB store would use across reloads.
  var sessions = {};   // content_hash -> RemoteGraph
  var urlHash = {};    // url -> content_hash (avoid re-opening a known URL)
  function _session(url) {
    var h = urlHash[url];
    if (h && sessions[h]) return sessions[h];
    var g = new wasm_bindgen.RemoteGraph(url); // opens once, reads the header
    h = g.content_hash();
    urlHash[url] = h;
    if (sessions[h]) { g.free(); return sessions[h]; } // same file via another URL
    sessions[h] = g;
    return sessions[h];
  }
  function _now() { return (typeof performance !== "undefined" ? performance.now() : Date.now()); }
  self._reteLog = function (e) { e.t = (_now() - qStart) | 0; if (fetchLog.length < 6000) fetchLog.push(e); };
  // The wasm calls reteProgress(bytes) after every physical range fetch (the
  // multipart hook also passes metadata). We tally a running count + a per-fetch
  // log and forward progress, so a long query shows live, not a frozen "querying…".
  self.reteProgress = function (b, meta) {
    pReq++; pBytes += (b || 0);
    self._reteLog(meta || { k: "range", b: (b || 0) });
    self.postMessage({ type: "progress", id: pId, requests: pReq, bytes: pBytes });
  };
  self.onmessage = function (e) {
    var m = e.data;
    if (m.type === "init") {
      ready = wasm_bindgen(m.bytes);
      ready.then(function () { self.postMessage({ type: "ready" }); });
      return;
    }
    if (m.type === "query") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () {
        try {
          var g = _session(m.url);
          var before = JSON.parse(g.stats());
          var res = JSON.parse(g.query(m.query, m.format));
          var after = JSON.parse(g.stats());
          // Per-query physical traffic is the delta (a cache hit adds ~0); carry
          // the session-cumulative too so the UI can show what the cache saved.
          res.remote = {
            fileLength: after.fileLength,
            bytes: after.bytes - before.bytes,
            requests: after.requests - before.requests,
            sessionBytes: after.bytes,
            sessionRequests: after.requests,
            cached: (after.requests - before.requests) === 0
          };
          self.postMessage({ type: "result", id: m.id, ok: true, json: JSON.stringify(res), log: fetchLog });
        } catch (err) {
          self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
        }
      });
    }
    // Generic call to any *_url wasm export (schema_url, check_schema_url, …).
    // These do synchronous range-read XHR, which is worker-only — so the main
    // thread MUST route them here, never call wasm_bindgen.*_url directly.
    if (m.type === "call") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () {
        try {
          var json = wasm_bindgen[m.fn].apply(null, m.args);
          self.postMessage({ type: "result", id: m.id, ok: true, json: json, log: fetchLog });
        } catch (err) {
          self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
        }
      });
    }
    // Label-prefix entity search over the resident remote session: faults only
    // the label-index tiles (sync range XHR, worker-only), like a query.
    if (m.type === "psearch") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () {
        try {
          var g = _session(m.url);
          var json = g.prefix_search(m.prefix, m.limit || 12);
          self.postMessage({ type: "result", id: m.id, ok: true, json: json, log: fetchLog });
        } catch (err) {
          self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
        }
      });
    }
  };
})();`;

  // Multi-range coalescing. The wasm engine already batches the byte ranges a
  // query needs (read_coalesced → read_many) and calls globalThis.reteReadMany
  // when present; without it the worker falls back to one synchronous XHR per
  // range (the sequential RTTs). This hook fetches ALL the ranges in ONE request
  // — RFC 7233 multipart/byteranges — collapsing N round trips into one. It must
  // be synchronous (the engine calls it from sync wasm) → worker-only sync XHR.
  // Returns one Uint8Array with the ranges concatenated in order, or null to
  // fall back (e.g. a host that ignores multi-range). Binary-safe, regex-free.
  const COALESCE_JS = `
;(function () {
  function boundaryOf(ct) {
    var i = ct.indexOf("boundary=");
    if (i < 0) return null;
    var b = ct.slice(i + 9).trim();
    if (b.charAt(0) === '"') b = b.slice(1, b.indexOf('"', 1));
    else { var sc = b.indexOf(";"); if (sc >= 0) b = b.slice(0, sc); }
    return b.trim();
  }
  function idx(hay, needle, from) {
    outer: for (var i = from; i <= hay.length - needle.length; i++) {
      for (var j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
      return i;
    }
    return -1;
  }
  function parseByteranges(u8, ct) {
    var bnd = boundaryOf(ct);
    if (!bnd) return null;
    var enc = new TextEncoder();
    var dash = enc.encode("--" + bnd);
    var sep = enc.encode("\\r\\n\\r\\n");
    var crlfDash = enc.encode("\\r\\n--" + bnd);
    var parts = [];
    var pos = idx(u8, dash, 0);
    if (pos < 0) return null;
    pos += dash.length;
    for (;;) {
      if (u8[pos] === 45 && u8[pos + 1] === 45) break;
      var hend = idx(u8, sep, pos);
      if (hend < 0) break;
      var bodyStart = hend + 4;
      var next = idx(u8, crlfDash, bodyStart);
      if (next < 0) next = u8.length;
      parts.push(u8.subarray(bodyStart, next));
      pos = next + crlfDash.length;
      if (pos >= u8.length) break;
    }
    return parts;
  }
  self.__parseByteranges = parseByteranges;
  self.reteReadMany = function (url, offsets, lens) {
    try {
      var n = offsets.length;
      if (n < 2) return null;
      var spec = [], total = 0;
      for (var i = 0; i < n; i++) { var o = offsets[i], l = lens[i]; spec.push(o + "-" + (o + l - 1)); total += l; }
      var xhr = new XMLHttpRequest();
      xhr.open("GET", url, false);
      xhr.responseType = "arraybuffer";
      xhr.setRequestHeader("Range", "bytes=" + spec.join(","));
      xhr.send();
      if (xhr.status !== 206) return null;
      var ct = xhr.getResponseHeader("Content-Type") || "";
      if (ct.indexOf("multipart/byteranges") < 0) return null;
      var parts = parseByteranges(new Uint8Array(xhr.response), ct);
      if (!parts || parts.length !== n) return null;
      var out = new Uint8Array(total), p = 0;
      for (var k = 0; k < n; k++) { if (parts[k].length !== lens[k]) return null; out.set(parts[k], p); p += lens[k]; }
      if (self.reteProgress) self.reteProgress(total, { k: "multi", n: n, b: total, r: spec }); // 1 request, N ranges
      return out;
    } catch (e) { return null; }
  };
})();`;

  // --- Persistent incremental range cache (opt-in) ----------------------
  // Injected at the top of each engine worker (rete, DuckDB-WASM, sql.js-httpvfs)
  // — all three read via SYNCHRONOUS XMLHttpRequest. We subclass XMLHttpRequest and
  // serve single-range GETs from an in-memory 1 MiB-block mirror that is warmed from
  // IndexedDB at startup; newly fetched blocks are persisted asynchronously. ANY
  // error in the cached path falls back to a real request (so a bug can't break a
  // query), and the shim only installs when the flag (baked in at worker creation,
  // from the Settings toggle) is on. Regex-free so the backtick string needs no
  // escaping beyond the literal CRLF (\\r\\n) in getAllResponseHeaders.
  const RANGE_CACHE_SHIM = `
;(function(){
  if(!self.__RC_ON) return;
  if(typeof XMLHttpRequest==="undefined" || typeof indexedDB==="undefined") return;
  var RealXHR=XMLHttpRequest, BLOCK=self.__RC_BLOCK||1048576, DBN=self.__RC_DB||"playgroundCache",
      RANGES="ranges", RMETA="rangeMeta", WARMCAP=self.__RC_WARMCAP||100663296;
  var mirror=Object.create(null), totals=Object.create(null), dirty=[], flushTimer=null;
  function keyOf(url){ try{ var u=new URL(url, self.location&&self.location.href); return u.origin+u.pathname; }catch(e){ var su=String(url), q=su.indexOf("?"); return q<0?su:su.slice(0,q); } }
  function openDB(){ return new Promise(function(res,rej){ var r=indexedDB.open(DBN,2); r.onupgradeneeded=function(){ var db=r.result; ["files","meta",RANGES,RMETA].forEach(function(s){ if(!db.objectStoreNames.contains(s)) db.createObjectStore(s); }); }; r.onsuccess=function(){res(r.result);}; r.onerror=function(){rej(r.error);}; }); }
  openDB().then(function(db){ var metas=[]; var c=db.transaction(RMETA).objectStore(RMETA).openCursor(); c.onsuccess=function(e){ var cur=e.target.result; if(cur){ var v=cur.value||{}; v.key=cur.key; metas.push(v); cur.continue(); } else warm(db,metas); }; c.onerror=function(){}; }).catch(function(){});
  function warm(db,metas){ metas.sort(function(a,b){ return (b.lastUsed||0)-(a.lastUsed||0); }); var budget=WARMCAP, want=[]; for(var i=0;i<metas.length;i++){ var m=metas[i]; totals[m.key]=m.total; var bl=m.blocks||[]; for(var j=0;j<bl.length;j++){ if(budget<=0) break; want.push(m.key+"#"+bl[j]); budget-=BLOCK; } } if(!want.length) return; var st=db.transaction(RANGES).objectStore(RANGES); want.forEach(function(k){ var g=st.get(k); g.onsuccess=function(){ if(g.result) mirror[k]=new Uint8Array(g.result); }; }); }
  function scheduleFlush(){ if(flushTimer) return; flushTimer=setTimeout(flush,800); }
  function flush(){ flushTimer=null; if(!dirty.length) return; var items=dirty; dirty=[]; openDB().then(function(db){ var tx=db.transaction([RANGES,RMETA],"readwrite"), rs=tx.objectStore(RANGES), ms=tx.objectStore(RMETA), byKey=Object.create(null); items.forEach(function(it){ try{ rs.put(it.b, it.k+"#"+it.i); }catch(e){} (byKey[it.k]=byKey[it.k]||[]).push(it.i); }); Object.keys(byKey).forEach(function(k){ var g=ms.get(k); g.onsuccess=function(){ var m=g.result||{total:totals[k]||0,blocks:[],bytes:0}; var seen=Object.create(null); (m.blocks||[]).forEach(function(b){seen[b]=1;}); byKey[k].forEach(function(b){ if(!seen[b]){seen[b]=1;m.blocks.push(b);m.bytes=(m.bytes||0)+BLOCK;} }); m.total=totals[k]||m.total; m.lastUsed=Date.now(); try{ms.put(m,k);}catch(e){} }; }); }).catch(function(){}); }
  function parseBR(r){ if(!r||r.indexOf("bytes=")!==0||r.indexOf(",")>=0) return null; var rest=r.slice(6), dash=rest.indexOf("-"); if(dash<1) return null; var s=parseInt(rest.slice(0,dash),10), es=rest.slice(dash+1); if(es==="") return null; var e=parseInt(es,10); if(isNaN(s)||isNaN(e)||e<s) return null; return [s,e]; }
  function totalOf(cr){ if(!cr) return null; var sl=cr.lastIndexOf("/"); if(sl<0) return null; var t=parseInt(cr.slice(sl+1),10); return isNaN(t)?null:t; }
  function fetchSpan(url,key,b0,b1){ var b=b0; while(b<=b1){ if(mirror[key+"#"+b]){ b++; continue; } var s=b; while(b<=b1 && !mirror[key+"#"+b]) b++; var e=b-1, as=s*BLOCK, ae=(e+1)*BLOCK-1; var x=new RealXHR(); x.open("GET",url,false); x.setRequestHeader("Range","bytes="+as+"-"+ae); x.responseType="arraybuffer"; x.send(); if(x.status!==206) throw new Error("rc status "+x.status); var buf=new Uint8Array(x.response), t=totalOf(x.getResponseHeader("Content-Range")); if(t!=null) totals[key]=t; for(var bb=s;bb<=e;bb++){ var off=(bb-s)*BLOCK; if(off>=buf.length) break; var u=buf.slice(off, Math.min(off+BLOCK, buf.length)); mirror[key+"#"+bb]=u; dirty.push({k:key,i:bb,b:u}); } scheduleFlush(); } }
  function serve(url,s,e){ var key=keyOf(url), b0=Math.floor(s/BLOCK), b1=Math.floor(e/BLOCK); fetchSpan(url,key,b0,b1); var out=new Uint8Array(e-s+1), p=0; for(var b=b0;b<=b1;b++){ var blk=mirror[key+"#"+b]; if(!blk) throw new Error("rc miss"); var bs=b*BLOCK, from=Math.max(s,bs)-bs, to=Math.min(e,bs+blk.length-1)-bs; for(var i=from;i<=to;i++) out[p++]=blk[i]; } return { bytes:out.subarray(0,p), total:totals[key], start:s }; }
  class CachedXHR extends RealXHR {
    open(method,url,async){ this.__m=(method||"GET").toUpperCase(); this.__u=url; this.__sync=(async===false); this.__range=null; this.__cached=false; this.__resp=null; this.__cr=null; return super.open(method,url,async); }
    setRequestHeader(name,value){ if(String(name).toLowerCase()==="range") this.__range=value; return super.setRequestHeader(name,value); }
    send(body){
      if(this.__m==="GET" && this.__sync && this.__range){
        var br=parseBR(this.__range);
        if(br){ try{ var r=serve(this.__u,br[0],br[1]); this.__cached=true; this.__resp=r.bytes.slice().buffer; this.__cr="bytes "+r.start+"-"+(r.start+r.bytes.length-1)+"/"+(r.total!=null?r.total:"*"); return; }catch(e){} }
      }
      return super.send(body);
    }
    get status(){ return this.__cached?206:super.status; }
    get statusText(){ return this.__cached?"Partial Content":super.statusText; }
    get readyState(){ return this.__cached?4:super.readyState; }
    get response(){ return this.__cached?this.__resp:super.response; }
    get responseText(){ return this.__cached?"":super.responseText; }
    getResponseHeader(name){ if(this.__cached){ var n=String(name).toLowerCase(); if(n==="content-range") return this.__cr; if(n==="content-length") return String(this.__resp.byteLength); return null; } return super.getResponseHeader(name); }
    getAllResponseHeaders(){ return this.__cached ? ("content-range: "+this.__cr+"\\r\\n") : super.getAllResponseHeaders(); }
  }
  self.XMLHttpRequest = CachedXHR;
})();`;

  // Worker prelude that bakes the toggle state + block/cap constants in front of
  // the shim. Read at worker-creation time; flipping the toggle recreates workers.
  function rcPrelude() {
    // Empty when off, so a worker built with rcPrelude() is byte-identical to before
    // — the cache feature is fully inert (and the default path untouched) unless on.
    if (!state.rangeCacheOn) return "";
    return "self.__RC_ON=true;self.__RC_BLOCK=1048576;self.__RC_DB=\"playgroundCache\";self.__RC_WARMCAP=100663296;\n" +
      RANGE_CACHE_SHIM + "\n";
  }

  let remoteWorker = null, remoteReady = null, remoteResolveReady = null, remoteSeq = 0;
  let remoteOnProgress = null;
  const remotePending = new Map();

  // Hard-cancel a running remote query: a synchronous wasm query can't be
  // interrupted cooperatively, so we terminate the worker (it rebuilds on the
  // next query) and reject anything in flight.
  function cancelRemote() {
    if (remoteWorker) { remoteWorker.terminate(); remoteWorker = null; remoteReady = null; remoteResolveReady = null; }
    remotePending.forEach((p) => p.reject(new Error("cancelled")));
    remotePending.clear();
    remoteOnProgress = null;
  }

  function ensureRemoteWorker() {
    if (remoteWorker) return remoteReady;
    const glue = document.getElementById("reteGlue").textContent;
    const blob = new Blob([rcPrelude() + glue + REMOTE_HARNESS + COALESCE_JS], { type: "text/javascript" });
    remoteWorker = new Worker(URL.createObjectURL(blob));
    remoteWorker.onmessage = (e) => {
      const m = e.data;
      if (m.type === "ready") { if (remoteResolveReady) remoteResolveReady(); return; }
      if (m.type === "progress") { if (remoteOnProgress) remoteOnProgress(m); return; }
      if (m.type === "result") {
        const p = remotePending.get(m.id);
        if (!p) return;
        remotePending.delete(m.id);
        if (m.ok) p.resolve({ json: m.json, log: m.log || [] });
        else { const err = new Error(m.error); err.log = m.log || []; p.reject(err); }
      }
    };
    remoteReady = new Promise((res) => { remoteResolveReady = res; });
    remoteWorker.postMessage({ type: "init", bytes: b64ToBytes(RETE_WASM_B64) });
    return remoteReady;
  }

  function remoteSparql(url, query, fmt) {
    return ensureRemoteWorker().then(() => new Promise((resolve, reject) => {
      const id = ++remoteSeq;
      remotePending.set(id, { resolve, reject });
      remoteWorker.postMessage({ type: "query", id, url, query, format: fmt || "table" });
    }));
  }

  // Run any *_url wasm export (schema_url, check_schema_url, …) in the worker —
  // they use synchronous range-read XHR, which a document can't do. Resolves to
  // { json, log } like remoteSparql.
  function remoteCall(fn, ...args) {
    return ensureRemoteWorker().then(() => new Promise((resolve, reject) => {
      const id = ++remoteSeq;
      remotePending.set(id, { resolve, reject });
      remoteWorker.postMessage({ type: "call", id, fn, args });
    }));
  }

  // Run a single-arg remote read (schema_url / check_schema_url) with LIVE
  // progress painted into `el`: an animated bar, a running range-request + bytes
  // + elapsed line, and a step log fed by the worker's per-fetch events. The
  // first read also spins up the query worker, so the feedback matters. Resolves
  // the remoteCall promise ({ json, log }).
  function remoteRead(fn, url, el, caption, hint) {
    const t0 = performance.now();
    const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[state.dataset]) || {};
    const ofSize = meta.size ? " of " + meta.size : "";
    let lastReq = 0, lastBytes = 0, logged = 0;
    el.innerHTML =
      `<div class="range-read">` +
        `<div class="range-read-cap">${esc(caption)}</div>` +
        `<div class="cache-bar indeterminate"><div class="cache-bar-fill"></div></div>` +
        `<div class="range-read-meta" id="rrMeta"></div>` +
        (hint ? `<div class="range-read-hint">${esc(hint)}</div>` : "") +
        `<div class="cache-steps" id="rrSteps"><div class="cache-step active">Starting the query worker…</div></div>` +
      `</div>`;
    const metaEl = el.querySelector("#rrMeta");
    const stepsEl = el.querySelector("#rrSteps");
    const paint = () => {
      const dt = (performance.now() - t0) / 1000;
      if (metaEl) metaEl.textContent = `${lastReq} range request(s) · ${formatBytes(lastBytes)}${ofSize} fetched · ${dt.toFixed(1)}s`;
    };
    paint();
    const timer = setInterval(paint, 150);
    const prev = remoteOnProgress;
    remoteOnProgress = (m) => {
      lastReq = m.requests; lastBytes = m.bytes;
      if (stepsEl && m.requests > logged) {
        stepsEl.querySelectorAll(".cache-step.active").forEach((s) => s.classList.replace("active", "done"));
        stepsEl.insertAdjacentHTML("beforeend",
          `<div class="cache-step active">Range request #${m.requests} — ${formatBytes(m.bytes)} fetched</div>`);
        logged = m.requests;
      }
      paint();
    };
    const cleanup = () => { clearInterval(timer); remoteOnProgress = prev; };
    return remoteCall(fn, url).then((out) => { cleanup(); return out; }, (e) => { cleanup(); throw e; });
  }

  const BUILD_SAMPLE = `# Paste N-Triples here (or open a file), pick the format, then Build.
<http://ex/Alice> <http://ex/knows> <http://ex/Bob> .
<http://ex/Bob> <http://ex/knows> <http://ex/Carol> .
<http://ex/Alice> <http://ex/age> "30"^^<http://www.w3.org/2001/XMLSchema#integer> .
<http://ex/Carol> <http://ex/worksAt> <http://ex/AcmeLabs> .
`;

  const $ = (id) => document.getElementById(id);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const W = () => wasm_bindgen;

  function esc(value) {
    return String(value == null ? "" : value)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");
  }

  function shorten(value, max = 82) {
    const s = String(value == null ? "" : value);
    if (s.length <= max) return s;
    const iri = s.match(/^<(.+)>$/);
    if (iri) {
      const body = iri[1];
      const cut = Math.max(body.lastIndexOf("/"), body.lastIndexOf("#"));
      if (cut >= 0 && body.length - cut < max - 8) return "<..." + body.slice(cut) + ">";
    }
    return s.slice(0, Math.max(0, max - 3)) + "...";
  }

  function formatBytes(n) {
    const v = Number(n || 0);
    if (v < 1024) return v + " B";
    if (v < 1024 * 1024) return (v / 1024).toFixed(1) + " KB";
    return (v / 1024 / 1024).toFixed(1) + " MB";
  }

  function b64ToBytes(b64) {
    const bin = atob(b64);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  function setStatus(text) {
    $("meta").textContent = text;
  }

  function sourceLabel() {
    if (state.activeSource === "file") return "local file";
    if (state.activeSource === "url") return "url";
    if (state.activeSource === "built") return "built in browser";
    if (state.activeSource === "remote") return "remote (lazy)";
    if (state.activeSource === "cached") return "remote (cached)";
    return "bundled";
  }

  function updateSourcePill() {
    $("sourcePill").textContent = sourceLabel();
  }

  function datasetInfo(key) {
    return CATALOG.datasets.find((d) => d.key === key) || CATALOG.datasets[0];
  }

  // --- Code editors: delegated to the PlaygroundEditor component (editor.js).
  // It only reads/writes the textarea TEXT, so it cannot change what a query
  // does — the run path still evaluates the textarea's literal value. The
  // component adds syntax highlight, keyword/schema/entity autocomplete, and
  // hover tooltips. See web/playground-src/editor.js.
  const EDITORS = (window.PlaygroundEditor && window.PlaygroundEditor.editors) || {};

  // Entity search powering the editor's autocomplete: matching labels from the
  // loaded graph's label index (a bounded binary search, no triple scan).
  // Returns [] when nothing is in memory (e.g. a remote-lazy dataset), so
  // autocomplete falls back to keywords + schema. Pure read — never evaluates.
  function entitySearch(prefix) {
    if (!state.graph) return [];
    try { return JSON.parse(state.graph.prefix_search(prefix, 8)); } catch (_e) { return []; }
  }

  // Predefined per-dataset IRI -> label hints (instant, used by the editor's
  // "Labels" decode toggle for previews — see CATALOG.labelHints).
  function labelHintsFor() {
    return (CATALOG.labelHints && CATALOG.labelHints[state.dataset]) || null;
  }

  // A small spinner ("spindle") on the decode toggle while a remote label lookup
  // is in flight (it can take a few range reads on a multi-GB file).
  let decodeBusy = 0;
  function setDecodeLoading(on) {
    const b = $("decodeToggle");
    if (b) b.classList.toggle("loading", !!on);
  }
  function parseLabelRows(res) {
    const out = {};
    (res.rows || []).forEach((r) => {
      const s = String(r.s || "").replace(/^<|>$/g, "");
      if (!s || out[s] != null) return;
      const lm = String(r.l || "").match(/^"((?:\\.|[^"\\])*)"/);
      out[s] = lm ? lm[1] : String(r.l || "");
    });
    return out;
  }
  function labelQueryFor(iris) {
    const values = iris.slice(0, 60).map((i) => "<" + i + ">").join(" ");
    return "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n" +
      "PREFIX skos: <http://www.w3.org/2004/02/skos/core#>\n" +
      "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\n" +
      "SELECT ?s ?l WHERE { VALUES ?s { " + values + " }\n" +
      "  ?s ?p ?l . VALUES ?p { rdfs:label skos:prefLabel foaf:name }\n" +
      "  FILTER(isLiteral(?l)) }";
  }
  // Best-effort live IRI -> label resolution for the decode toggle. Embedded
  // graphs resolve synchronously; a remote-lazy graph routes the SAME label
  // query through the worker (HTTP range reads) and returns a Promise — so the
  // toggle is fully lazy-mode compatible. Predefined labelHints already cover the
  // showcase IRIs, so the live lookup only fires for un-hinted ones.
  function resolveLabels(iris) {
    if (!iris || !iris.length) return {};
    if (state.remote) {
      decodeBusy++; setDecodeLoading(true);
      return remoteSparql(state.remote.url, labelQueryFor(iris), "table")
        .then((out) => parseLabelRows(JSON.parse(out.json)))
        .catch(() => ({}))
        .finally(() => { if (--decodeBusy <= 0) { decodeBusy = 0; setDecodeLoading(false); } });
    }
    if (!state.graph || !state.bytes) return {};
    try { return parseLabelRows(JSON.parse(state.graph.query(labelQueryFor(iris), "table"))); }
    catch (_e) { return {}; }
  }

  // Entity (label-prefix) search over a remote-lazy graph: RemoteGraph.prefix_search
  // does synchronous range-read XHR (faults only the label-index tiles it needs),
  // which is worker-only — so route it through the query worker like remoteSparql.
  function remotePrefixSearch(url, prefix, limit) {
    return ensureRemoteWorker().then(() => new Promise((resolve, reject) => {
      const id = ++remoteSeq;
      remotePending.set(id, { resolve, reject });
      remoteWorker.postMessage({ type: "psearch", id, url, prefix, limit: limit || 12 });
    }));
  }

  function enhanceEditor(id, lang) {
    if (!window.PlaygroundEditor) return;
    window.PlaygroundEditor.enhance(id, lang, {
      schema: () => state.schema,
      searchEntities: entitySearch,
      labelHints: labelHintsFor,
      resolveLabels: resolveLabels
    });
  }

  // ── Entity finder (the panel beside the SPARQL editor) ───────────────────
  // Type a human string; rete's bounded label index (prefix_search, no scan)
  // returns matching entities; click one to insert its <IRI> at the caret.
  function localName(iri) { return String(iri).replace(/[<>]/g, "").replace(/^.*[\/#:]/, "") || iri; }
  function insertAtCaret(id, text) {
    const ta = $(id);
    if (!ta) return;
    const s = ta.selectionStart || 0, e = ta.selectionEnd || 0;
    ta.value = ta.value.slice(0, s) + text + ta.value.slice(e);
    const caret = s + text.length;
    ta.setSelectionRange(caret, caret);
    if (window.PlaygroundEditor && PlaygroundEditor.editors[id]) PlaygroundEditor.editors[id].refresh();
    ta.focus();
  }
  function renderFinder(hits, q) {
    const box = $("efResults");
    if (!box) return;
    if (!q) {
      box.innerHTML = `<p class="ef-empty">Type a name to search this graph's entities by their label.</p>`;
      return;
    }
    if (!hits.length) {
      box.innerHTML = `<p class="ef-empty">No entities match “${esc(q)}”.</p>`;
      return;
    }
    box.innerHTML = hits.map((h) => {
      const iri = String(h.subject).replace(/^<|>$/g, "");
      return `<button type="button" class="ef-item" data-iri="${esc(iri)}" title="${esc(iri)}">` +
        `<span class="ef-label">${esc(h.label || localName(iri))}</span>` +
        `<span class="ef-iri">${esc(localName(iri))}</span></button>`;
    }).join("");
    $$("#efResults .ef-item").forEach((b) => {
      b.onclick = () => insertAtCaret("q", "<" + b.dataset.iri + ">");
    });
  }
  // Search by label. Embedded graphs answer synchronously; a remote-lazy graph
  // faults its label-index tiles over HTTP range (worker), so a spinner shows and
  // a sequence guard drops out-of-order results from earlier keystrokes.
  let efSeq = 0;
  function efSearch() {
    const inp = $("efInput");
    if (!inp) return;
    const q = (inp.value || "").trim();
    if (!q) { renderFinder([], ""); return; }
    const seq = ++efSeq;
    if (state.remote) {
      const box = $("efResults");
      if (box) box.innerHTML = `<div class="ef-loading"><span class="spindle"></span> searching “${esc(q)}” over range reads…</div>`;
      remotePrefixSearch(state.remote.url, q, 15).then((out) => {
        if (seq !== efSeq) return;
        let hits = []; try { hits = JSON.parse(out.json); } catch (_e) { hits = []; }
        renderFinder(hits, q);
      }).catch(() => { if (seq === efSeq) renderFinder([], q); });
      return;
    }
    let hits = [];
    if (state.graph) { try { hits = JSON.parse(state.graph.prefix_search(q, 15)); } catch (_e) { hits = []; } }
    renderFinder(hits, q);
  }

  function setEd(id, text) {
    if (window.PlaygroundEditor) window.PlaygroundEditor.setText(id, text);
    else { const t = $(id); if (t) t.value = text; }
  }

  // Show the loaded dataset's short name on the topbar chip (which opens the
  // Datasets browser). Replaces the old <select> dropdown.
  function setDatasetName(key) {
    const d = datasetInfo(key);
    $("dsName").textContent = d ? d.label.split(" - ")[0] : key;
  }

  // The dataset header band: a full title and a one-line sentence, with the
  // graph metadata pill sitting to its right.
  function firstSentence(text, max) {
    if (!text) return "";
    const m = text.match(/^(.+?[.!?])(\s|$)/);
    let s = (m ? m[1] : text).trim();
    const cap = max || 170;
    if (s.length > cap) s = s.slice(0, cap - 1).replace(/\s+\S*$/, "") + "…";
    return s;
  }

  function setDatasetHeader(title, tagline) {
    const t = $("dsTitle"); if (t) t.textContent = title || "—";
    const g = $("dsTagline"); if (g) g.textContent = tagline || "";
  }

  // Switch the Explore sub-tab (Entity tables / Community / File byte map / SQL).
  function setExploreView(view) {
    $$("#exploreSeg button").forEach((b) => b.classList.toggle("active", b.dataset.exp === view));
    $$(".explore-sub").forEach((p) => p.classList.toggle("active", p.dataset.exp === view));
    if (view === "sql") ensureExploreSql();
  }

  function renderDatasetOptions() {
    setDatasetName(state.dataset);
  }

  // Yield to the event loop so the UI can repaint between synchronous WASM calls.
  const tick = () => new Promise((r) => setTimeout(r, 0));

  // Opening a large cached graph runs several synchronous WASM passes (each
  // re-parses the file) that block the UI for seconds. `onPhase` lets the caller
  // (the cache modal) narrate the steps; a `tick()` before each heavy call lets
  // that label paint before the engine blocks the thread.
  async function loadBytes(bytes, source, onPhase) {
    state.bytes = bytes;
    state.activeSource = source;
    state.remote = null; // an in-memory load leaves remote-lazy mode
    state.exploreReady = false;
    state.exploreBackend = "native"; state.exploreNativeMeta = ""; freeExploreEngines();
    state.lastResult = null; // a new graph invalidates any cached result
    // Switching datasets drops federation partners; caching the *current* one
    // (source === "cached") keeps them — its self-source just becomes in-memory.
    if (source !== "cached") resetFed();
    updateSourcePill();

    // Open the file ONCE into a resident handle; every later in-memory query
    // reuses it instead of re-copying the buffer and re-decoding the dictionary.
    if (onPhase) { onPhase("Opening file & loading dictionaries…"); await tick(); }
    if (state.graph) { state.graph.free(); state.graph = null; }
    state.graph = new (W().Graph)(bytes);
    const info = JSON.parse(state.graph.info());
    // info() already carries the named-graph count, so we avoid a second full
    // open just to call graph_names() (a meaningful saving on a big cached file).
    const graphText = info.namedGraphs ? " | graphs " + info.namedGraphs : "";
    setStatus(`${info.quads} quads | ${info.terms} terms | ${info.pyramidLevels} pyramid levels${graphText}`);

    // Prefer the schema already baked into the file (read from its ~KB schema
    // block, instant) over recomputing it by scanning every triple — only files
    // built before the schema pyramid existed need the scan fallback.
    let schema;
    try {
      if (onPhase) { onPhase("Reading the packed schema…"); await tick(); }
      schema = JSON.parse(W().schema_packed(bytes));
    } catch (_e) {
      if (onPhase) { onPhase("Building schema (classes & relations)…"); await tick(); }
      schema = JSON.parse(state.graph.schema());
    }
    state.schema = schema;
    state.exploreClass = null;
    state.exploreReady = false;
    renderSchema(schema);
    if (state.mode === "explore") ensureExplore();
    renderProgressiveInfo(null);
    renderProvenanceSummary(null);
    renderReachDefaults();
    renderShaclExamples();
    renderProvenanceDefaults();

    const infoRow = datasetInfo(state.dataset);
    const catalogSource = source === "bundled" || source === "cached";
    $("dsDesc").textContent = catalogSource
      ? infoRow.description
      : "Custom graph loaded into the same in-browser engine.";
    if (catalogSource && infoRow) {
      setDatasetHeader(infoRow.label, firstSentence(infoRow.description));
    } else {
      const cn = source === "file" ? "Local file" : source === "url" ? "Custom .rete" : "Custom graph";
      $("dsName").textContent = cn;
      setDatasetHeader(cn, "Custom graph loaded into the same in-browser engine.");
    }
  }

  function loadDataset(key) {
    const b64 = RETE_DATASETS_B64[key];
    if (!b64) {
      setStatus("dataset not embedded: " + key);
      return;
    }
    state.dataset = key;
    setDatasetName(key);
    loadBytes(b64ToBytes(b64), "bundled");
    renderExamples();
    const list = examplesForDataset();
    if (list.length) selectExample(0);
    updateHash();
  }

  async function loadFromUrl() {
    const url = $("remoteUrl").value.trim();
    if (!url) return;
    setStatus("downloading...");
    try {
      const res = await fetch(url);
      if (!res.ok) throw new Error(res.status + " " + res.statusText);
      const buf = new Uint8Array(await res.arrayBuffer());
      loadBytes(buf, "url");
      closeSource();
    } catch (e) {
      showError("out", "URL load failed: " + e.message);
    }
  }

  // Enter remote lazy mode: query a remote .rete over HTTP range via the
  // worker, no full download. Only the SPARQL tab applies (the other tabs need
  // the whole graph in memory). `datasetKey` ties it to a catalog entry so its
  // example query library shows; a custom URL (no key) gets no library.
  function enterRemote(url, datasetKey) {
    if (!url) return;
    state.bytes = null;
    if (state.graph) { state.graph.free(); state.graph = null; }
    resetFed(); // switching to a remote dataset drops federation partners
    state.remote = { url };
    state.activeSource = "remote";
    state.schema = null;
    state.exploreReady = false;
    state.exploreClass = null;
    state.exploreBackend = "native"; state.exploreNativeMeta = ""; freeExploreEngines();
    if (datasetKey) {
      state.dataset = datasetKey;
      setDatasetName(datasetKey);
    }
    state.selectedExample = -1;
    updateSourcePill();
    setStatus("remote (lazy) — queries range-fetch only what they touch");
    const info = datasetKey ? datasetInfo(datasetKey) : null;
    $("dsDesc").textContent = info ? info.description : "Remote graph, queried lazily over HTTP range: " + url;
    setDatasetHeader(info ? info.label : "Remote .rete (lazy)",
      info ? firstSentence(info.description) : "Remote graph, queried lazily over HTTP range — only the bytes each query touches are fetched.");
    renderExamples();
    // Catalog-driven example panels are independent of the (lazy, unloaded)
    // bytes — refresh them here too, or the SHACL / Reach / Provenance tabs keep
    // the PREVIOUS dataset's content (e.g. scholar's "Paper integrity" shape
    // lingering on wikidata-1GB). The bundled/cached paths get this via loadBytes.
    renderShaclExamples();
    renderReachDefaults();
    renderProvenanceDefaults();
    closeSource();
    setMode("sparql");
    // Load the dataset's first example query automatically (parity with bundled).
    if (examplesForDataset().length) selectExample(0);
    const lib = examplesForDataset().length
      ? "Pick an example from the library, or write your own."
      : "Write a SPARQL query (a bound subject keeps the fetch small). No example library for a custom URL.";
    $("out").innerHTML = `<div class="note">Connected to a remote .rete, queried lazily — ` +
      `each query fetches only the dictionary chunks and index tiles it touches (the first also ` +
      `pulls the header and directories). ${lib} Other tabs need a graph loaded into memory.</div>`;
  }

  function connectRemote() {
    enterRemote($("remoteUrl").value.trim(), null);
  }

  // Every dataset is mirrored in the bucket at playground/<key>.rete, so any of
  // them can be cached or range-queried. Remote-only datasets carry their own
  // `url`; the rest derive it from remoteBase.
  function remoteUrlFor(key) {
    const d = datasetInfo(key);
    if (d && d.url) return d.url;
    const tok = CATALOG.remoteToken ? "?token=" + CATALOG.remoteToken : "";
    return `${CATALOG.remoteBase}/playground/${key}.rete${tok}`;
  }
  function isEmbedded(key) { return !!RETE_DATASETS_B64[key]; }

  // Downloaded-remote cache: fetch the whole .rete once, keep the bytes, then
  // query it in memory on later loads (the "cache" mode of the source switch).
  const remoteCache = new Map();
  // Stream a fetch so we can report download progress (bytes received vs the
  // Content-Length, when the server provides it).
  async function fetchWithProgress(url, onProgress) {
    const res = await fetch(url);
    if (!res.ok) throw new Error(res.status + " " + res.statusText);
    const total = Number(res.headers.get("content-length")) || 0;
    if (!res.body || !res.body.getReader) {
      const buf = new Uint8Array(await res.arrayBuffer());
      onProgress(buf.length, total || buf.length);
      return buf;
    }
    const reader = res.body.getReader();
    const chunks = [];
    let received = 0;
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      chunks.push(value);
      received += value.length;
      onProgress(received, total);
    }
    const out = new Uint8Array(received);
    let pos = 0;
    for (const c of chunks) { out.set(c, pos); pos += c.length; }
    return out;
  }

  function openCacheModal(key) {
    const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[key]) || {};
    $("cacheName").textContent = dsShortLabel(key);
    $("cacheSub").textContent = `Downloading the whole .rete${meta.size ? " (" + meta.size + ")" : ""} into memory — queried in-page once it's here.`;
    $("cacheBar").classList.add("indeterminate");
    $("cacheBarFill").style.width = "";
    $("cachePct").textContent = "";
    $("cacheBytes").textContent = "0 B";
    $("cacheModal").classList.remove("hidden");
  }
  function updateCacheProgress(received, total) {
    if (total > 0) {
      $("cacheBar").classList.remove("indeterminate");
      const pct = Math.min(100, Math.round((received / total) * 100));
      $("cacheBarFill").style.width = pct + "%";
      $("cachePct").textContent = pct + "%";
      $("cacheBytes").textContent = `${formatBytes(received)} / ${formatBytes(total)}`;
    } else {
      $("cacheBytes").textContent = formatBytes(received);
    }
  }
  function closeCacheModal() { $("cacheModal").classList.add("hidden"); }

  // Parse "98 MB" / "1.04 GB" / "375 KB" → megabytes, for a rough prep estimate.
  function sizeToMB(s) {
    const m = /([\d.]+)\s*(KB|MB|GB|TB|B)/i.exec(String(s || ""));
    if (!m) return 0;
    const v = parseFloat(m[1]);
    return ({ B: v / 1e6, KB: v / 1e3, MB: v, GB: v * 1e3, TB: v * 1e6 })[m[2].toUpperCase()] || v;
  }

  // Switch the cache modal from "downloading" to "preparing the in-memory graph",
  // with a rough time estimate and a running step log (the opens block the UI, so
  // each step's label is painted just before the engine freezes the thread).
  function enterCachePreparing(key) {
    const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[key]) || {};
    const est = Math.max(1, Math.round(sizeToMB(meta.size) * 0.05));
    $("cacheBar").classList.remove("indeterminate");
    $("cacheBarFill").style.width = "100%";
    $("cachePct").textContent = "downloaded";
    $("cacheBytes").textContent = meta.size || "";
    $("cacheSub").textContent = `Now opening the file for in-memory queries — the schema is read straight ` +
      `from the file's packed index (no scan); the page pauses briefly while the dictionary loads ` +
      `(~${est}s for ${meta.size || "this graph"}).`;
    $("cacheSteps").innerHTML = `<div class="cache-step done">Downloaded ${esc(meta.size || "")}</div>`;
  }
  function setCacheStep(label) {
    const el = $("cacheSteps");
    if (!el) return;
    el.querySelectorAll(".cache-step.active").forEach((s) => s.classList.replace("active", "done"));
    el.insertAdjacentHTML("beforeend", `<div class="cache-step active">${esc(label)}</div>`);
  }

  async function loadCachedRemote(key) {
    state.dataset = key;
    setDatasetName(key);
    try {
      let bytes = remoteCache.get(key);
      if (!bytes) {
        setStatus("downloading " + key + " …");
        openCacheModal(key);
        bytes = await fetchWithProgress(remoteUrlFor(key), updateCacheProgress);
        remoteCache.set(key, bytes);
      } else {
        openCacheModal(key);
      }
      enterCachePreparing(key);
      await tick();
      await loadBytes(bytes, "cached", setCacheStep);
      renderExamples();
      const list = examplesForDataset();
      if (list.length) selectExample(0);
      updateHash();
      setCacheStep("Ready ✓");
      await tick();
      closeCacheModal();
    } catch (e) {
      closeCacheModal();
      showError("out", "Cache download failed: " + (e.message || e));
    }
  }

  // Load a dataset in one of three modes: bundled (embedded bytes), cache
  // (download the remote once, keep it), lazy (range-query the remote).
  function selectDatasetMode(key, mode) {
    if (mode === "lazy") return enterRemote(remoteUrlFor(key), key);
    if (mode === "cache") return loadCachedRemote(key);
    return loadDataset(key);
  }
  // Default mode for non-modal callers (history, hash): bundled if embedded,
  // else lazy over HTTP range.
  function selectDataset(key) {
    const d = datasetInfo(key);
    if (isEmbedded(key) && !(d && d.kind === "remote-lazy")) loadDataset(key);
    else enterRemote(remoteUrlFor(key), key);
  }

  // Which playground tabs a dataset can showcase, derived from the catalog.
  function datasetSupports(key) {
    const exs = CATALOG.examples[key] || [];
    const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[key]) || {};
    return {
      SPARQL: exs.length > 0,
      SHACL: (CATALOG.shacl[key] || []).length > 0,
      Reasoning: !!ex.reasoning,
      Reach: !!CATALOG.reach[key],
      Provenance: !!CATALOG.provenance[key],
      Geo: exs.some((e) => e.family === "Geo") || exs.some((e) => /\bgeof:/.test(e.q || ""))
    };
  }

  // Tiny markdown for descriptions: **bold**, `code`, *italic* (input escaped).
  function mdLite(s) {
    return esc(String(s || ""))
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, "$1<em>$2</em>");
  }

  function dsShortLabel(key) {
    const d = datasetInfo(key);
    return d ? d.label.split(" - ")[0] : key;
  }

  // The "Datasets" browser: a sidebar list (left) + a detail/preview pane
  // (right). The selected dataset shows tags, the example kinds it supports, a
  // 3-mode source switch (bundled / cache / lazy), its metadata under "more",
  // and an example preview.
  let dsSelected = null;

  function renderDsSidebar() {
    const q = ($("dsSearch").value || "").trim().toLowerCase();
    const items = CATALOG.datasets.filter((d) => {
      if (!q) return true;
      const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[d.key]) || {};
      return [d.label, d.description, (ex.tags || []).join(" ")].join(" ").toLowerCase().includes(q);
    });
    if (!items.length) {
      $("dsSidebar").innerHTML = `<p class="microcopy" style="padding:8px">No matching datasets.</p>`;
      return;
    }
    $("dsSidebar").innerHTML = items.map((d) => {
      const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[d.key]) || {};
      const m = (CATALOG.datasetMeta && CATALOG.datasetMeta[d.key]) || {};
      const remote = d.kind === "remote-lazy";
      const active = d.key === dsSelected;
      const size = m.size || "—";
      return `<button type="button" class="ds-side-item${active ? " active" : ""}" data-ds="${esc(d.key)}">` +
        `<span class="ds-side-ico">${esc(ex.icon || "📊")}</span>` +
        `<span class="ds-side-name">${esc(dsShortLabel(d.key))}</span>` +
        `<span class="ds-side-size${remote ? " remote" : ""}" title="${remote ? "remote-only · " : ""}.rete size">${remote ? "🛰 " : ""}${esc(size)}</span>` +
        `</button>`;
    }).join("");
    $$("#dsSidebar .ds-side-item").forEach((b) => {
      b.onclick = () => { dsSelected = b.dataset.ds; renderDsSidebar(); renderDsDetail(dsSelected); };
    });
  }

  function renderDsDetail(key) {
    const d = datasetInfo(key);
    const m = (CATALOG.datasetMeta && CATALOG.datasetMeta[key]) || {};
    const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[key]) || {};
    const remoteOnly = d.kind === "remote-lazy";
    const embedded = isEmbedded(key);
    const sup = datasetSupports(key);
    const fmtTri = (t) => (t == null ? "—" : typeof t === "number" ? t.toLocaleString() : esc(t));
    const host = (u) => { try { return new URL(u).host.replace(/^www\./, ""); } catch (e) { return u; } };

    const badge = remoteOnly
      ? `<span class="ds-badge remote">🛰 Remote-only · lazy</span>`
      : `<span class="ds-badge bundled">Bundled in page</span>`;
    // Descriptive tags + capability chips (a distinct colour family) in one row.
    const capChips = ["SPARQL", "SHACL", "Reasoning", "Reach", "Provenance", "Geo"]
      .filter((c) => sup[c])
      .map((c) => `<span class="ds-cap on">${esc(c)}</span>`).join("");
    const tags = (ex.tags || []).map((t) => `<span class="ds-tag">${esc(t)}</span>`).join("") +
      (m.license ? `<span class="ds-tag license">${esc(m.license)}</span>` : "") + capChips;

    const defMode = embedded ? "bundled" : "lazy";
    const hints = {
      bundled: "Loads the copy embedded in this page — instant, fully offline.",
      cache: "Downloads the whole .rete from the bucket once, then queries it in memory (cached this session).",
      lazy: "Range-queries the remote .rete over HTTP — only the bytes each query touches are fetched."
    };
    const modeItem = (mode, label, dis) =>
      `<button type="button" data-mode="${mode}"${dis ? " disabled" : ""} class="ds-load-item${mode === defMode ? " preferred" : ""}">` +
      `<span class="ds-load-item-top">${esc(label)}${mode === defMode ? `<span class="ds-pref-tag">preferred</span>` : ""}</span>` +
      `<span class="ds-load-item-hint">${esc(hints[mode])}</span></button>`;
    const loadMenu = `<div class="ds-load">` +
      `<button type="button" class="ds-load-btn" id="dsLoadBtn" aria-haspopup="true" aria-expanded="false"><span class="ds-eject-ic" aria-hidden="true">⏏</span>Load<span class="ds-load-caret" aria-hidden="true">⌄</span></button>` +
      `<div class="ds-load-menu hidden" id="dsLoadMenu">` +
      modeItem("bundled", "Bundled", !embedded) +
      modeItem("cache", "Cache remote", false) +
      modeItem("lazy", "Lazy range", false) +
      `</div></div>`;

    // Preview: the examples this dataset ships, each tagged by kind (SPARQL /
    // SHACL) with a one-line "what it's about" and an expandable query/shape —
    // multiline bodies open on demand instead of being clipped.
    const previewItems = [];
    (CATALOG.examples[key] || []).forEach((e) =>
      previewItems.push({ type: "SPARQL", fam: e.family || "", label: e.label, tip: e.tip || "", code: e.q || "" }));
    (CATALOG.shacl[key] || []).forEach((e) =>
      previewItems.push({ type: "SHACL", fam: "Shape", label: e.label, tip: e.tip || "", code: e.shape || "" }));
    const preview = previewItems.length
      ? previewItems.map((e) => {
          const tag = `<span class="ds-prev-tag ${e.type.toLowerCase()}">${esc(e.type)}</span>` +
            (e.fam ? `<span class="ds-prev-fam">${esc(e.fam)}</span>` : "");
          return `<div class="ds-prev-item">` +
            `<div class="ds-prev-head">${tag}<span class="ds-prev-label">${esc(e.label)}</span></div>` +
            (e.tip ? `<div class="ds-prev-tip">${esc(e.tip)}</div>` : "") +
            `<details class="ds-prev-det"><summary>Show ${e.type === "SHACL" ? "shape" : "query"}</summary>` +
            `<pre class="ds-prev-q">${esc((e.code || "").trim())}</pre></details>` +
            `</div>`;
        }).join("")
      : `<p class="microcopy">No examples for this dataset.</p>`;

    const metaTable = `<table class="ds-meta-table"><tbody>` +
      `<tr><td>Triples</td><td class="num">${fmtTri(m.triples)}</td></tr>` +
      `<tr><td>.rete size</td><td class="num">${esc(m.size || "—")}</td></tr>` +
      `<tr><td>Type</td><td>${remoteOnly ? "🛰 Remote · lazy" : "Bundled"}${embedded ? " · also in bucket" : ""}</td></tr>` +
      `<tr><td>License</td><td>${esc(m.license || "—")}</td></tr>` +
      `<tr><td>Source</td><td>${m.source ? `<a href="${esc(m.source)}" target="_blank" rel="noopener">${esc(host(m.source))} ↗</a>` : "—"}</td></tr>` +
      `<tr><td>Provenance</td><td>${m.provenance ? esc(m.provenance) : "—"}</td></tr>` +
      `<tr><td>Bucket</td><td class="iri">playground/${esc(key)}.rete</td></tr>` +
      `</tbody></table>`;

    $("dsDetail").innerHTML =
      `<div class="ds-detail-head">` +
        `<div class="ds-ico-tile">${esc(ex.icon || "📊")}</div>` +
        `<div class="ds-detail-head-main"><h2>${esc(dsShortLabel(key))}</h2><div class="ds-detail-sub">${badge}</div></div>` +
        loadMenu +
      `</div>` +
      `<div class="ds-tags">${tags}</div>` +
      `<p class="ds-desc">${mdLite(d.description)}</p>` +
      `<details class="ds-more-block"><summary>More — metadata &amp; provenance</summary>${metaTable}</details>` +
      `<div class="ds-section-label">Examples · ${previewItems.length}</div>` +
      `<div class="ds-preview">${preview}</div>`;

    $("dsLoadBtn").onclick = (e) => {
      e.stopPropagation();
      const menu = $("dsLoadMenu");
      const nowHidden = menu.classList.toggle("hidden");
      $("dsLoadBtn").setAttribute("aria-expanded", String(!nowHidden));
    };
    $$("#dsLoadMenu button").forEach((b) => {
      b.onclick = () => { if (b.disabled) return; selectDatasetMode(key, b.dataset.mode); closeSource(); };
    });
  }

  function openSource() {
    if (!dsSelected || !datasetInfo(dsSelected)) dsSelected = state.dataset;
    $("dsSearch").value = "";
    $("dsSearch").oninput = renderDsSidebar;
    renderDsSidebar();
    renderDsDetail(dsSelected);
    $("sourceModal").classList.remove("hidden");
  }

  function closeSource() {
    $("sourceModal").classList.add("hidden");
  }

  async function loadFromFile(file) {
    if (!file) return;
    try {
      const buf = new Uint8Array(await file.arrayBuffer());
      loadBytes(buf, "file");
      setStatus(`${file.name} | ${formatBytes(buf.byteLength)} | custom file`);
    } catch (e) {
      showError("out", "File load failed: " + e.message);
    }
  }

  function examplesForDataset() {
    return CATALOG.examples[state.dataset] || [];
  }

  function filteredExamples() {
    const q = $("exampleSearch").value.trim().toLowerCase();
    return examplesForDataset()
      .map((ex, index) => ({ ex, index }))
      .filter(({ ex }) => state.family === "All" || ex.family === state.family)
      .filter(({ ex }) => {
        if (!q) return true;
        return [ex.label, ex.family, ex.tip, ex.q].join(" ").toLowerCase().includes(q);
      });
  }

  function renderFamilyFilters() {
    const families = ["All"].concat(CATALOG.families);
    $("familyFilters").innerHTML = families.map((family) =>
      `<button type="button" data-family="${esc(family)}" class="${family === state.family ? "active" : ""}">${esc(family)}</button>`
    ).join("");
    $$("#familyFilters button").forEach((btn) => {
      btn.onclick = () => {
        state.family = btn.dataset.family;
        renderExamples();
      };
    });
  }

  function renderExamples() {
    renderFamilyFilters();
    renderQuickExamples();
    const items = filteredExamples();
    if (!items.length) {
      $("examples").innerHTML = `<p class="microcopy">No matching examples for this dataset.</p>`;
      return;
    }
    $("examples").innerHTML = items.map(({ ex, index }) =>
      `<article class="example-card" data-family="${esc(ex.family)}">` +
        `<button type="button" class="example-button ${index === state.selectedExample ? "active" : ""}" data-example="${index}">` +
          `<span>${esc(ex.label)}</span>` +
        `</button>` +
        `<div class="tagline">${esc(ex.family)} | ${esc(ex.tip)}</div>` +
      `</article>`
    ).join("");
    $$("#examples [data-example]").forEach((btn) => {
      btn.onclick = () => selectExample(Number(btn.dataset.example));
    });
  }

  function selectExample(index) {
    const ex = examplesForDataset()[index];
    if (!ex) return;
    state.selectedExample = index;
    setEd("q", ex.q);
    setView(ex.view || "table");
    setStrategy(ex.strategy || "whole");
    setMode("sparql");
    // An example may declare federation partners (catalog keys) — a one-click
    // multi-source demo. Reset to just this dataset, then add each partner
    // (embedded → in-memory, remote-lazy → range-read).
    resetFed();
    if (Array.isArray(ex.fed) && ex.fed.length) {
      ex.fed.forEach((k) => {
        if (k === state.dataset || !datasetInfo(k)) return;
        state.fedSources.push(isEmbedded(k)
          ? { id: "f" + (++fedSeq), kind: "memory", label: dsShortLabel(k), key: k }
          : { id: "f" + (++fedSeq), kind: "remote", label: dsShortLabel(k), url: remoteUrlFor(k), key: k });
      });
      renderFedBar();
    }
    $("exampleInfo").innerHTML =
      `<div><strong>${esc(ex.label)}</strong></div>` +
      `<div>${esc(ex.family)}</div>` +
      `<div>${esc(ex.tip)}</div>`;
    closeLibrary();
    renderExamples();
    updateHash();
  }

  // The quick-suggestion row above the editor: the dataset's first 1–2 examples
  // as one-tap chips (the 2nd hides on a narrow editor), plus a button that opens
  // the full Query Library modal.
  function renderQuickExamples() {
    const quick = $("exampleQuick");
    if (!quick) return;
    const all = examplesForDataset();
    const chips = all.slice(0, 2).map((ex, i) =>
      `<button type="button" class="ex-quick-chip${i === 1 ? " opt2" : ""}${state.selectedExample === i ? " active" : ""}" ` +
        `data-example="${i}" title="${esc(ex.tip || ex.label)}">` +
        `<span class="eqfam">${esc(ex.family || "")}</span><span class="eqlabel">${esc(ex.label)}</span></button>`).join("");
    quick.innerHTML = chips +
      `<button type="button" id="libraryBtn" class="ex-quick-lib" title="Browse the full query library">` +
      `⊞ Library${all.length ? " · " + all.length : ""}</button>`;
    $$("#exampleQuick [data-example]").forEach((b) => { b.onclick = () => selectExample(Number(b.dataset.example)); });
    $("libraryBtn").onclick = openLibrary;
    renderExampleDesc(all);
  }

  // The always-visible description of the active example, shown inline under the
  // quick-chip row (so the full explanation reads without hovering a chip). Falls
  // back to the first example as a preview when nothing is selected yet.
  function renderExampleDesc(all) {
    const box = $("exampleDesc");
    if (!box) return;
    const list = all || examplesForDataset();
    const sel = (state.selectedExample != null && list[state.selectedExample]) || list[0];
    if (!sel) { box.innerHTML = ""; box.classList.add("hidden"); return; }
    box.classList.remove("hidden");
    box.innerHTML =
      `<span class="exd-fam">${esc(sel.family || "Query")}</span>` +
      `<span class="exd-label">${esc(sel.label)}</span>` +
      (sel.tip ? `<span class="exd-tip">${esc(sel.tip)}</span>` : "");
  }

  function openLibrary() { renderExamples(); $("libraryModal").classList.remove("hidden"); }
  function closeLibrary() { $("libraryModal").classList.add("hidden"); }
  function openHistory() { renderHistory(); $("historyModal").classList.remove("hidden"); }
  function closeHistory() { $("historyModal").classList.add("hidden"); }

  // Settings → cached companion files: list each with its size, delete one or all.
  function humanCacheLabel(m) {
    if (m.label) return m.label;
    const p = String(m.key).split("::");
    return p[0] + " · " + p.slice(1).join("/");
  }
  async function renderCacheList() {
    const list = $("cacheList"), totalEl = $("cacheTotal");
    if (!list) return;
    const items = await idbListMeta();
    if (!items.length) {
      list.innerHTML = `<p class="cache-empty">Nothing cached yet. In <b>Explore</b>, pick a DuckDB or SQLite backend and click “Cache locally”.</p>`;
      if (totalEl) totalEl.textContent = "";
      return;
    }
    let total = 0;
    list.innerHTML = items.map((m) => {
      total += m.size || 0;
      return `<div class="cache-row"><span class="ci-name">${esc(humanCacheLabel(m))}</span>` +
        `<span class="ci-size">${formatBytes(m.size || 0)}</span>` +
        `<button type="button" class="secondary" data-del="${esc(m.key)}">Delete</button></div>`;
    }).join("");
    if (totalEl) totalEl.textContent = `${items.length} file(s) · ${formatBytes(total)} total.`;
    list.querySelectorAll("[data-del]").forEach((b) => b.onclick = async () => {
      await idbDelKey(b.dataset.del);
      freeExploreEngines();
      renderCacheList();
      renderCacheCtl();
    });
  }
  // Flip the persistent range cache; recreate the engines so their worker prelude
  // picks up the new flag (next query rebuilds them).
  function setRangeCache(on) {
    state.rangeCacheOn = !!on;
    try { localStorage.setItem("rangeCacheOn", on ? "1" : "0"); } catch (e) { /* private mode */ }
    freeExploreEngines();
    cancelRemote();
  }
  async function renderRangeCache() {
    const t = $("rangeCacheToggle"); if (t) t.checked = !!state.rangeCacheOn;
    const info = $("rangeCacheInfo");
    if (info) info.textContent = state.rangeCacheOn
      ? "On — fetched byte ranges (rete, DuckDB and SQLite) are saved to IndexedDB and reused after a reload. Toggling recreates the query engines."
      : "Off — the lazy backends keep fetched bytes only for this session; a reload re-fetches. Turn on to persist ranges across reloads (experimental).";
    const sz = $("rangeCacheSize");
    if (sz) sz.textContent = "Range cache: " + formatBytes(await rangeCacheSize());
  }
  function openSettings() { renderRangeCache(); renderCacheList(); $("settingsModal").classList.remove("hidden"); }
  function closeSettings() { $("settingsModal").classList.add("hidden"); }

  const LIB_KEY = "rete.pg.libCollapsed";
  function setLibCollapsed(collapsed) {
    const shell = document.querySelector(".console-shell");
    if (shell) shell.classList.toggle("lib-collapsed", collapsed);
    try { localStorage.setItem(LIB_KEY, collapsed ? "1" : "0"); } catch (_e) { /* ignore */ }
  }

  function setMode(mode) {
    state.mode = mode;
    $$("#modeTabs button").forEach((btn) => btn.classList.toggle("active", btn.dataset.mode === mode));
    $$(".panel").forEach((panel) => panel.classList.toggle("active", panel.dataset.panel === mode));
    // Sidebar sections are contextual: a section tagged with data-modes only
    // shows for the active tab, so the column stays short (no inner scrollbar).
    // Untagged sections (Source, History) are always visible.
    $$(".library-panel section[data-modes]").forEach((sec) =>
      sec.classList.toggle("hidden", !sec.dataset.modes.split(" ").includes(mode)));
    if (mode === "explore") ensureExplore();
    if (mode === "schema" && state.remote && !state.schema) ensureRemoteSchema();
    updateResultVisibility();
    updateHash();
  }

  // Lazy Schema: read the class/relation summary from the schema pyramid (the
  // card) over HTTP range — a Schema view of a remote graph, no download.
  function ensureRemoteSchema() {
    if (!state.remote || state.schema) return;
    // schema_url does synchronous range XHR → run in the worker, with live progress.
    remoteRead("schema_url", state.remote.url, $("schemaOut"),
      "Reading the schema pyramid over HTTP range…",
      "The schema block (classes & relations) reads in ~2–3 small range requests — no download.").then((out) => {
      const schema = JSON.parse(out.json);
      state.schema = schema;
      renderSchema(schema);
      const r = schema.remote || {};
      $("schemaOut").innerHTML = `<div class="banner">${(schema.classes || []).length} classes and ${(schema.relations || []).length} class-level relations — read from the schema pyramid over HTTP range (${formatBytes(r.bytes || 0)} of ${formatBytes(r.fileLength || 0)}, ${r.requests || 0} request(s), no download).</div>`;
    }).catch((e) => {
      const msg = String(e && e.message || e);
      if (/no schema pyramid/i.test(msg)) {
        $("schemaOut").innerHTML = `<div class="note">This graph carries no schema pyramid (no typed classes), so there's no schema to summarize over range. Use <strong>Cache remote</strong> to compute one by scanning.</div>`;
      } else {
        showError("schemaOut", "Remote schema failed: " + msg);
      }
    });
  }

  // --- Explore: entity tables + the community pyramid -------------------
  function ensureExplore() {
    updateExploreBackends();
    if (state.exploreReady) return;
    // Remote-lazy: Explore the .rete over HTTP range (entity tables fetched
    // tile-by-tile). In-memory: the whole graph is loaded, query it directly.
    if (state.remote) return ensureExploreRemote();
    if (!state.bytes) return;
    state.exploreReady = true;
    renderExploreClasses();
    renderPyramid();
    renderLayout();
  }

  // Explore a remote .rete without downloading it. The class list comes from the
  // baked schema pyramid (the same ~2-range summary the Schema tab reads — no
  // scan), and each class drills into a bounded entity query fetched over HTTP
  // range. The community pyramid and byte map are computed over the whole file,
  // so they point at "Cache remote" instead of showing a stale in-memory graph.
  function ensureExploreRemote() {
    state.exploreReady = true;
    $("pyramidNote").textContent =
      "The community pyramid is computed over the whole graph — use “Cache remote” (the source switch) to download this .rete once, then view it here.";
    $("pyramidLegend").innerHTML = "";
    $("pyramidViz").innerHTML = "";
    $("pyramidLevels").innerHTML = "";
    $("layoutNote").textContent =
      "The byte map needs the whole file — use “Cache remote” to download it once, then view the layout here.";
    $("layoutLegend").innerHTML = "";
    $("layoutViz").innerHTML = "";
    $("layoutTable").innerHTML = "";
    if (state.schema) return renderRemoteExploreClasses();
    $("exploreTable").innerHTML = "";
    // schema_url does synchronous range XHR → run in the worker, with live progress.
    remoteRead("schema_url", state.remote.url, $("exploreClasses"),
      "Reading the schema over HTTP range…",
      "The schema block (classes & relations) reads in ~2–3 small range requests — no download.").then((out) => {
      state.schema = JSON.parse(out.json);
      renderRemoteExploreClasses();
    }).catch((e) => {
      const msg = String(e && e.message || e);
      if (/no schema pyramid/i.test(msg)) {
        $("exploreClasses").innerHTML =
          `<p class="microcopy">This remote graph carries no schema pyramid, so its classes can't be ` +
          `listed without scanning the whole file. Use <strong>Cache remote</strong> to explore it fully.</p>`;
        $("exploreTable").innerHTML = "";
      } else {
        showError("exploreTable", "Remote schema read failed: " + msg);
      }
    });
  }

  // Class buttons for a remote graph, sourced from the baked schema (no scan).
  function renderRemoteExploreClasses() {
    const classes = ((state.schema && state.schema.classes) || [])
      .slice().sort((a, b) => b[1] - a[1]).slice(0, 12);
    if (!classes.length) {
      $("exploreClasses").innerHTML = `<p class="microcopy">This remote graph's schema lists no typed classes.</p>`;
      $("exploreTable").innerHTML = "";
      return;
    }
    renderClassButtons(classes);
  }

  // The predicate this dataset uses for typing — rdf:type by default, but some
  // graphs use another (e.g. Wikidata's "instance of" = wdt:P31). Declared per
  // dataset in the catalog as `typePredicate`.
  function currentTypePredicate() {
    const d = datasetInfo(state.dataset);
    return (d && d.typePredicate) || RDF_TYPE;
  }

  // Top classes for the Explore tab. For rdf:type we reuse the (fast, single-pass)
  // schema summary; for a custom type predicate we derive the top classes live via
  // SPARQL — a scan, heavier on big files, but the only way without a baked schema
  // that already knows the predicate.
  function exploreClassList() {
    const tp = currentTypePredicate();
    if (tp === RDF_TYPE) return ((state.schema && state.schema.classes) || []).slice(0, 12);
    try {
      const res = JSON.parse(state.graph.query(
        `SELECT ?c (COUNT(?s) AS ?n) WHERE { ?s ${tp} ?c } GROUP BY ?c ORDER BY DESC(?n) LIMIT 12`, "table"));
      return (res.rows || []).map((r) => [r.c, (String(r.n).match(/\d+/) || ["?"])[0]]);
    } catch (e) { return []; }
  }

  function renderExploreClasses() {
    const tp = currentTypePredicate();
    const classes = exploreClassList();
    if (!classes.length) {
      const via = tp === RDF_TYPE ? "rdf:type" : shorten(localName(tp), 24);
      $("exploreClasses").innerHTML =
        `<p class="microcopy">No ${esc(via)} classes in this graph — showing raw triples.</p>`;
      const res = JSON.parse(state.graph.query("SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 300", "table"));
      $("exploreTable").innerHTML = renderTable(res.vars || [], res.rows || []);
      return;
    }
    renderClassButtons(classes);
  }

  // How many entities one Explore page shows. Each page is a bounded, lazy fetch
  // (a DISTINCT ?s … LIMIT/OFFSET page, then a VALUES props fetch), so paging a
  // remote class range-reads only that page — never the whole class.
  const ENTITY_PAGE = 25;

  // One query helper for both Explore paths: a Promise of the parsed result, run
  // in memory (sync) or over HTTP range (the worker) when the graph is remote.
  function exploreQuery(q) {
    if (state.remote) {
      return remoteSparql(state.remote.url, q, "table").then((out) => {
        state.lastRemoteLog = out.log || [];
        updateReqLogBtn();
        return JSON.parse(out.json);
      });
    }
    return Promise.resolve(JSON.parse(state.graph.query(q, "table")));
  }

  // --- Explore backend switch -------------------------------------------
  // The same class, queried via the rete engine or the Parquet/DuckDB/SQLite
  // companions, so you can compare them. The SQL engines load lazily from a CDN
  // on first use (the one online, opt-in feature — native exploration is offline).
  // Datasets without a CATALOG.companions[key] entry never show the switch.
  const EXPLORE_BACKENDS = [
    { id: "native", label: "rete (native)" },
    { id: "duck-parquet", label: "DuckDB · Parquet" },
    { id: "duck-db", label: "DuckDB · .duckdb" },
    { id: "sqlite", label: "SQLite" },
  ];
  function currentCompanion() {
    return (CATALOG.companions && CATALOG.companions[state.dataset]) || null;
  }
  // Build a bucket URL the same way remoteUrlFor does (remoteBase + ?token).
  function companionUrl(path) {
    const tok = CATALOG.remoteToken ? "?token=" + CATALOG.remoteToken : "";
    return `${CATALOG.remoteBase}/${path}${tok}`;
  }
  // Map the Explore-selected class (a term like `<…#Class>`) to its companion
  // table. Bare-IRI compare so the `<>` wrapping doesn't matter.
  function tableForClass(comp, clsTerm) {
    if (!comp || !comp.tables) return null;
    const iri = String(clsTerm || "").replace(/^<|>$/g, "");
    return comp.tables.find((t) => t.classIri === iri) || null;
  }

  // Lazy DuckDB-WASM (Parquet + .duckdb share one engine), loaded from jsDelivr.
  let _duck = null;
  async function ensureDuck() {
    if (_duck) return _duck;
    const duckdb = await import("https://cdn.jsdelivr.net/npm/@duckdb/duckdb-wasm@1.29.0/+esm");
    const b = await duckdb.selectBundle(duckdb.getJsDelivrBundles());
    // With the range cache on, build our own worker (mirrors duckdb.createWorker for
    // a cross-origin URL) so the shim installs before DuckDB takes over XMLHttpRequest;
    // with it off, use duckdb's own createWorker so the default path is unchanged.
    const worker = state.rangeCacheOn
      ? new Worker(URL.createObjectURL(new Blob(
          [rcPrelude() + "importScripts(" + JSON.stringify(b.mainWorker) + ");"], { type: "text/javascript" })))
      : await duckdb.createWorker(b.mainWorker);
    const db = new duckdb.AsyncDuckDB(new duckdb.ConsoleLogger(), worker);
    await db.instantiate(b.mainModule, b.pthreadWorker);
    const conn = await db.connect();
    // Keep HTTP-fetched Parquet/DB metadata + objects cached across queries in this
    // session, so re-running a query over the same file refetches little or nothing.
    for (const s of ["SET enable_http_metadata_cache=true", "SET enable_object_cache=true"]) {
      try { await conn.query(s); } catch (e) { /* older duckdb-wasm: setting absent */ }
    }
    _duck = { duckdb, db, conn, attached: null };
    return _duck;
  }
  // Attach the .duckdb over httpfs (range reads); re-attach when the URL changes.
  async function ensureDuckAttach(comp) {
    const d = await ensureDuck();
    const url = companionUrl(comp.duckdb);
    if (d.attached === url) return d;
    if (d.attached) { try { await d.conn.query("DETACH wd"); } catch (e) { /* not attached */ } }
    await d.conn.query(`ATTACH '${url}' AS wd (READ_ONLY)`);
    d.attached = url;
    return d;
  }
  // DuckDB Arrow result -> [columns, rows-as-objects] (nested LIST/MAP -> JSON).
  function duckRows(res) {
    const cols = res.schema.fields.map((f) => f.name);
    const rows = res.toArray().map((r) => {
      const o = r.toJSON();
      for (const k of cols) if (o[k] && typeof o[k] === "object") o[k] = JSON.stringify(o[k]);
      return o;
    });
    return [cols, rows];
  }
  // Lazy sql.js-httpvfs over the remote SQLite (range page reads). The CDN worker
  // is wrapped in a same-origin blob (a bare `new Worker(cdnUrl)` is blocked).
  let _sqlitePromise = null;
  function ensureSqlite(comp) {
    if (_sqlitePromise) return _sqlitePromise;
    _sqlitePromise = (async () => {
      const mod = await import("https://esm.sh/sql.js-httpvfs@0.8.12");
      const createDbWorker = mod.createDbWorker || (mod.default && mod.default.createDbWorker);
      if (!createDbWorker) throw new Error("createDbWorker export not found");
      const WBASE = "https://cdn.jsdelivr.net/npm/sql.js-httpvfs@0.8.12/dist";
      const workerSrc = await (await fetch(`${WBASE}/sqlite.worker.js`)).text();
      const workerUrl = URL.createObjectURL(new Blob([rcPrelude() + workerSrc], { type: "text/javascript" }));
      return createDbWorker(
        [{ from: "inline", config: { serverMode: "full", url: companionUrl(comp.sqlite), requestChunkSize: 4096 } }],
        workerUrl, `${WBASE}/sql-wasm.wasm`);
    })();
    return _sqlitePromise;
  }

  // --- Local cache (IndexedDB) ------------------------------------------
  // "Cache locally" stores a whole companion file so a backend can query it
  // without range reads and across page reloads; the Settings panel lists +
  // deletes them. Two stores: `files` (the bytes) + `meta` (small {size,…}) so
  // listing and existence checks never read the big buffers. Keys:
  //   "<dataset>::duckdb" | "<dataset>::sqlite" | "<dataset>::parquet::<file>".
  // Stores: `files`/`meta` (whole-file "Cache locally"), and `ranges`/`rangeMeta`
  // (the persistent incremental range cache the worker shim writes — see below).
  const CACHE_DB = "playgroundCache", FILES = "files", META = "meta", RANGES = "ranges", RMETA = "rangeMeta";
  const CACHE_DB_VERSION = 2;
  function idbCreateStores(db) {
    [FILES, META, RANGES, RMETA].forEach((s) => { if (!db.objectStoreNames.contains(s)) db.createObjectStore(s); });
  }
  function idbOpen() {
    return new Promise((res, rej) => {
      const r = indexedDB.open(CACHE_DB, CACHE_DB_VERSION);
      r.onupgradeneeded = () => idbCreateStores(r.result);
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
  }
  // Total bytes held by the incremental range cache (summed from rangeMeta).
  function rangeCacheSize() {
    return idbOpen().then((db) => new Promise((res) => {
      let total = 0; const t = db.transaction(RMETA).objectStore(RMETA).openCursor();
      t.onsuccess = (e) => { const c = e.target.result; if (c) { total += (c.value && c.value.bytes) || 0; c.continue(); } else res(total); };
      t.onerror = () => res(0);
    })).catch(() => 0);
  }
  function clearRangeCache() {
    return idbOpen().then((db) => new Promise((res) => {
      const t = db.transaction([RANGES, RMETA], "readwrite");
      t.objectStore(RANGES).clear(); t.objectStore(RMETA).clear();
      t.oncomplete = () => res(); t.onerror = () => res();
    })).catch(() => {});
  }
  function idbReq(store, mode, fn) {
    return idbOpen().then((db) => new Promise((res, rej) => {
      const q = fn(db.transaction(store, mode).objectStore(store));
      q.onsuccess = () => res(q.result); q.onerror = () => rej(q.error);
    }));
  }
  const idbGetFile = (k) => idbReq(FILES, "readonly", (s) => s.get(k)).then((v) => v || null).catch(() => null);
  const idbGetMeta = (k) => idbReq(META, "readonly", (s) => s.get(k)).then((v) => v || null).catch(() => null);
  async function idbPutFile(k, bytes, meta) {
    const db = await idbOpen();
    return new Promise((res, rej) => {
      const t = db.transaction([FILES, META], "readwrite");
      t.objectStore(FILES).put(bytes, k); t.objectStore(META).put(meta, k);
      t.oncomplete = () => res(); t.onerror = () => rej(t.error);
    });
  }
  async function idbDelKey(k) {
    const db = await idbOpen();
    return new Promise((res) => {
      const t = db.transaction([FILES, META], "readwrite");
      t.objectStore(FILES).delete(k); t.objectStore(META).delete(k);
      t.oncomplete = () => res(); t.onerror = () => res();
    });
  }
  function idbListMeta() {
    return idbOpen().then((db) => new Promise((res) => {
      const out = []; const t = db.transaction(META).objectStore(META).openCursor();
      t.onsuccess = (e) => { const c = e.target.result; if (c) { out.push(Object.assign({ key: c.key }, c.value)); c.continue(); } else res(out); };
      t.onerror = () => res(out);
    })).catch(() => []);
  }
  async function idbClearAll() {
    const db = await idbOpen();
    return new Promise((res) => {
      const t = db.transaction([FILES, META], "readwrite");
      t.objectStore(FILES).clear(); t.objectStore(META).clear();
      t.oncomplete = () => res(); t.onerror = () => res();
    });
  }

  // The single cached file the active backend + selected class needs to run local.
  function cacheTarget() {
    const comp = currentCompanion(); if (!comp) return null;
    const ds = state.dataset, b = state.exploreBackend;
    if (b === "duck-db") return { key: `${ds}::duckdb`, url: companionUrl(comp.duckdb), label: dsShortLabel(ds) + " · .duckdb", sizeHint: comp.duckdbSize || "" };
    if (b === "sqlite") return { key: `${ds}::sqlite`, url: companionUrl(comp.sqlite), label: dsShortLabel(ds) + " · .sqlite", sizeHint: comp.sqliteSize || "" };
    if (b === "duck-parquet") {
      const t = tableForClass(comp, state.exploreClass); if (!t) return null;
      return { key: `${ds}::parquet::${t.file}`, url: companionUrl(comp.parquetDir + "/" + t.file), label: dsShortLabel(ds) + " · " + t.file, sizeHint: "" };
    }
    return null;
  }

  // Cached-engine handles, reset on dataset switch.
  let _sqliteFull = null;   // { key, db } — sql.js full in-memory DB from a cached buffer
  let _sqlMod = null;       // memoized sql.js module
  async function ensureSqliteFull(bytes) {
    if (!_sqlMod) {
      const initSqlJs = (await import("https://esm.sh/sql.js@1.12.0")).default;
      _sqlMod = await initSqlJs({ locateFile: (f) => "https://esm.sh/sql.js@1.12.0/dist/" + f });
    }
    return new _sqlMod.Database(new Uint8Array(bytes));
  }
  function sqlFullPage(db, tableName, offset) {
    const r = db.exec(`SELECT * FROM "${tableName}" LIMIT ${ENTITY_PAGE} OFFSET ${offset}`);
    const cols = r.length ? r[0].columns : [];
    const rows = r.length ? r[0].values.map((v) => Object.fromEntries(cols.map((c, i) => [c, v[i]]))) : [];
    return { cols, rows };
  }
  // Register a cached buffer into DuckDB's virtual FS once (idempotent per name).
  async function duckRegisterCached(d, vfsName, bytes) {
    d.registered = d.registered || new Set();
    if (!d.registered.has(vfsName)) { await d.db.registerFileBuffer(vfsName, new Uint8Array(bytes)); d.registered.add(vfsName); }
    return vfsName;
  }

  // Drop the lazy engines on dataset switch (or cache delete) so they re-initialize.
  function freeExploreEngines() {
    if (_duck) { try { _duck.conn.close(); _duck.db.terminate(); } catch (e) { /* already gone */ } _duck = null; }
    _sqlitePromise = null;
    if (_sqliteFull) { try { _sqliteFull.db.close(); } catch (e) { /* already closed */ } _sqliteFull = null; }
  }

  function setBackendMeta(text) {
    const el = $("exploreBackendMeta"); if (el) el.textContent = text || "";
  }
  // Stash the native baseline (rows · ms · bytes) so a companion result can show
  // it alongside for comparison.
  function setExploreNativeMeta(rows, ms, ent, res) {
    let bytes = 0, reqs = 0, remote = false;
    [ent, res].forEach((r) => { if (r && r.remote) { remote = true; bytes += r.remote.bytes || 0; reqs += r.remote.requests || 0; } });
    state.exploreNativeMeta = `rete: ${rows} rows · ${ms.toFixed(0)} ms` + (remote ? ` · ${formatBytes(bytes)} · ${reqs} req` : "");
    if (state.exploreBackend === "native") setBackendMeta(state.exploreNativeMeta);
  }

  // Show + wire the backend switch when this dataset ships companions; reset to
  // native on (re)entry. Idempotent — safe to call whenever Explore is shown.
  function updateExploreBackends() {
    const row = $("exploreBackendRow"), seg = $("exploreBackendSeg");
    if (!row || !seg) return;
    const comp = currentCompanion();
    row.hidden = !comp;
    const sqlTab = $("exploreSqlTab"); if (sqlTab) sqlTab.hidden = !comp;
    if (!comp) { state.exploreBackend = "native"; return; }
    if (!seg.dataset.wired) {
      seg.innerHTML = EXPLORE_BACKENDS.map((b) =>
        `<button type="button" data-be="${esc(b.id)}">${esc(b.label)}</button>`).join("");
      seg.querySelectorAll("[data-be]").forEach((btn) => {
        btn.onclick = () => {
          seg.querySelectorAll("[data-be]").forEach((b) => b.classList.toggle("active", b === btn));
          state.exploreBackend = btn.dataset.be;
          renderCacheCtl();
          if (state.exploreClass) loadEntityPage(0); // re-run the current class
          else if (state.exploreBackend === "native") setBackendMeta(state.exploreNativeMeta || "");
        };
      });
      seg.dataset.wired = "1";
    }
    state.exploreBackend = "native";
    seg.querySelectorAll("[data-be]").forEach((b) => b.classList.toggle("active", b.dataset.be === "native"));
    setBackendMeta(state.exploreNativeMeta || "");
    renderCacheCtl();
  }

  // One page of a class via a companion encoding: a bounded SELECT over its table,
  // rendered like the native entity table, with rows · ms (and the native baseline).
  async function loadEntityPageColumnar(page) {
    const comp = currentCompanion();
    const table = tableForClass(comp, state.exploreClass);
    const backend = state.exploreBackend;
    if (!table) {
      showError("exploreTable", "This class has no companion table — switch back to rete (native), " +
        "or pick a class that has one (" + (comp.tables || []).map((t) => localName(t.classIri)).join(", ") + ").");
      setBackendMeta("");
      return;
    }
    renderCacheCtl(); // reflect cache state for this backend + class
    const offset = page * ENTITY_PAGE;
    const backendName = { "duck-parquet": "DuckDB·Parquet", "duck-db": "DuckDB·.duckdb", "sqlite": "SQLite" }[backend] || backend;
    const target = cacheTarget();
    $("exploreTable").innerHTML = netSpinner("querying " + backendName + "…");
    const t0 = performance.now();
    try {
      let cols, rows, usingCache = false;
      if (backend === "sqlite") {
        if (_sqliteFull && target && _sqliteFull.key === target.key) {
          ({ cols, rows } = sqlFullPage(_sqliteFull.db, table.name, offset)); usingCache = true;
        } else if (target && await idbGetMeta(target.key)) {
          // Cached: load the whole DB into sql.js once, then page it in memory.
          if (_sqliteFull) { try { _sqliteFull.db.close(); } catch (e) { /* ignore */ } }
          _sqliteFull = { key: target.key, db: await ensureSqliteFull(await idbGetFile(target.key)) };
          ({ cols, rows } = sqlFullPage(_sqliteFull.db, table.name, offset)); usingCache = true;
        } else {
          const w = await ensureSqlite(comp);
          rows = await w.db.query(`SELECT * FROM "${table.name}" LIMIT ${ENTITY_PAGE} OFFSET ${offset}`);
          cols = rows.length ? Object.keys(rows[0]) : [];
        }
      } else {
        const d = await ensureDuck();
        let ref;
        if (backend === "duck-db") {
          const vfs = "cache_" + state.dataset + ".duckdb";
          if (d.attached === "cache:" + vfs) { usingCache = true; }
          else if (target && await idbGetMeta(target.key)) {
            await duckRegisterCached(d, vfs, await idbGetFile(target.key));
            if (d.attached) { try { await d.conn.query("DETACH wd"); } catch (e) { /* not attached */ } }
            await d.conn.query(`ATTACH '${vfs}' AS wd (READ_ONLY)`);
            d.attached = "cache:" + vfs; usingCache = true;
          } else { await ensureDuckAttach(comp); }
          ref = `wd."${table.name}"`;
        } else { // duck-parquet
          const vfs = "cache_" + state.dataset + "_" + table.file;
          if (d.registered && d.registered.has(vfs)) { ref = `read_parquet('${vfs}')`; usingCache = true; }
          else if (target && await idbGetMeta(target.key)) {
            await duckRegisterCached(d, vfs, await idbGetFile(target.key));
            ref = `read_parquet('${vfs}')`; usingCache = true;
          } else { ref = `read_parquet('${companionUrl(comp.parquetDir + "/" + table.file)}')`; }
        }
        const res = await d.conn.query(`SELECT * FROM ${ref} LIMIT ${ENTITY_PAGE} OFFSET ${offset}`);
        [cols, rows] = duckRows(res);
      }
      renderColumnarPage(table, cols, rows, page, backendName, performance.now() - t0, usingCache);
    } catch (e) {
      showError("exploreTable", backendName + " query failed: " + String(e && e.message || e) +
        (backend === "sqlite" ? " — sql.js-httpvfs is finicky from a CDN; try “Cache locally”." : ""));
      setBackendMeta("");
    }
  }

  // Render a companion-backend page: entity + label + the leading named columns
  // (the table is frequency-ordered, so the first few are the common ones), the
  // shared pager, and the rows · ms line with the native baseline for comparison.
  function renderColumnarPage(table, cols, rows, page, label, ms, usingCache) {
    const drop = new Set(["types", "extra"]);
    const ordered = ["entity", "label"].filter((c) => cols.includes(c))
      .concat(cols.filter((c) => !drop.has(c) && c !== "entity" && c !== "label"));
    const shown = ordered.slice(0, 9);
    const cell = (v) => {
      if (v == null) return "";
      let s = String(v);
      if (s.startsWith("[")) {
        try { const a = JSON.parse(s); s = a.slice(0, 3).map((x) => termLabel(parseTerm(String(x)))).join("; ") + (a.length > 3 ? ` (+${a.length - 3})` : ""); }
        catch (e) { /* not JSON, show raw */ }
      }
      return shorten(s, 60);
    };
    const head = `<tr>${shown.map((c) => `<th>${esc(shorten(c, 22))}</th>`).join("")}</tr>`;
    const body = rows.map((r) => `<tr>${shown.map((c) => `<td>${esc(cell(r[c]))}</td>`).join("")}</tr>`).join("");
    const total = table.entities || 0;
    const pages = Math.max(1, Math.ceil(total / ENTITY_PAGE));
    const from = rows.length ? page * ENTITY_PAGE + 1 : 0;
    const to = page * ENTITY_PAGE + rows.length;
    $("exploreTable").innerHTML =
      (rows.length
        ? `<div class="tbl"><table><thead>${head}</thead><tbody>${body}</tbody></table></div>`
        : `<p class="microcopy">No rows on this page.</p>`) +
      `<div class="entity-pager">` +
        `<button type="button" id="entPrev" class="secondary"${page <= 0 ? " disabled" : ""}>‹ Prev</button>` +
        `<span class="pager-info">${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()} ` +
        `${esc(table.name)} · page ${(page + 1).toLocaleString()} / ${pages.toLocaleString()}</span>` +
        `<button type="button" id="entNext" class="secondary"${page + 1 >= pages ? " disabled" : ""}>Next ›</button>` +
      `</div>` +
      `<p class="microcopy">${usingCache ? "Served from your local cached copy" : "Companion table, read lazily over HTTP range"} — object values are N-Triples term tokens (lists shown truncated).</p>`;
    const prev = $("entPrev"), next = $("entNext");
    if (prev) prev.onclick = () => loadEntityPage(page - 1);
    if (next) next.onclick = () => loadEntityPage(page + 1);
    setBackendMeta(`${label}: ${rows.length} rows · ${ms.toFixed(0)} ms${usingCache ? " · cached" : ""}` +
      (state.exploreNativeMeta ? "   ·   " + state.exploreNativeMeta : ""));
  }

  // The "Cache locally / Remove" control for the active backend's target file.
  async function renderCacheCtl() {
    const el = $("exploreCacheCtl"); if (!el) return;
    const target = cacheTarget();
    if (!target) { el.innerHTML = ""; return; }
    const meta = await idbGetMeta(target.key);
    el.innerHTML = meta
      ? `<span>cached ✓ ${formatBytes(meta.size || 0)}</span><button type="button" class="secondary" data-cache="remove">Remove</button>`
      : `<button type="button" class="secondary" data-cache="add">Cache locally${target.sizeHint ? " (" + esc(target.sizeHint) + ")" : ""}</button>`;
    const btn = el.querySelector("[data-cache]");
    if (btn) btn.onclick = () => (btn.dataset.cache === "add" ? cacheCurrentTarget() : removeCurrentTarget());
  }
  // Download the active backend's whole file into IndexedDB (with progress), then
  // re-run the page so it's served from the local copy.
  async function cacheCurrentTarget() {
    const target = cacheTarget(); if (!target) return;
    const el = $("exploreCacheCtl");
    try {
      const bytes = await fetchWithProgress(target.url, (got, total) => {
        if (el) el.innerHTML = `<span>caching… ${total ? Math.round(100 * got / total) + "%" : formatBytes(got)}</span>`;
      });
      await idbPutFile(target.key, bytes, { size: bytes.byteLength, label: target.label, dataset: state.dataset, backend: state.exploreBackend });
      await renderCacheCtl();
      if (state.exploreClass) loadEntityPage(state.explorePage || 0); // now served from cache
      if (!$("settingsModal").classList.contains("hidden")) renderCacheList();
    } catch (e) {
      if (el) el.innerHTML = `<span class="microcopy">cache failed: ${esc(String(e && e.message || e))}</span>`;
    }
  }
  // Delete the active backend's cached file and fall back to lazy range reads.
  async function removeCurrentTarget() {
    const target = cacheTarget(); if (!target) return;
    await idbDelKey(target.key);
    freeExploreEngines(); // drop any in-memory copy registered from the cache
    await renderCacheCtl();
    if (state.exploreClass) loadEntityPage(state.explorePage || 0);
    if (!$("settingsModal").classList.contains("hidden")) renderCacheList();
  }

  // --- Explore "SQL" sub-tab: a small SQL console over the companion tables ----
  // The companion tables are exposed under the same names across engines (DuckDB
  // views / native SQLite tables), so "SELECT * FROM Class_" runs on all three.
  // Cached files (Settings) are used when present, otherwise lazy HTTP range reads.
  const SQL_BACKENDS = [["duck-parquet", "DuckDB·Parquet"], ["duck-db", "DuckDB·.duckdb"], ["sqlite", "SQLite"]];
  function sqlExampleText(ex, backend) {
    if (!ex) return "";
    if (backend === "sqlite") return ex.sqlite || "";
    return (ex.duck || "").replace("{T}", `"${(ex.table && ex.table.name) || ""}"`);
  }
  function renderSqlExamples() {
    const box = $("sqlExamples"), comp = currentCompanion();
    if (!box || !comp) return;
    const exs = comp.examples || [];
    box.innerHTML = exs.map((e, i) => `<button type="button" data-ex="${i}">${esc(e.label)}</button>`).join("");
    box.querySelectorAll("[data-ex]").forEach((b) => b.onclick = () => {
      $("sqlEditor").value = sqlExampleText(exs[+b.dataset.ex], state.sqlBackend);
      runSql();
    });
  }
  function ensureExploreSql() {
    const comp = currentCompanion(); if (!comp) return;
    const seg = $("sqlBackendSeg"); if (!seg) return;
    if (!seg.dataset.wired) {
      seg.innerHTML = SQL_BACKENDS.map(([id, l]) => `<button type="button" data-sb="${esc(id)}">${esc(l)}</button>`).join("");
      seg.querySelectorAll("[data-sb]").forEach((b) => b.onclick = () => {
        seg.querySelectorAll("[data-sb]").forEach((x) => x.classList.toggle("active", x === b));
        state.sqlBackend = b.dataset.sb; renderSqlExamples();
      });
      $("sqlRun").onclick = runSql;
      seg.dataset.wired = "1";
    }
    // A fresh dataset clears the editor/output so the default query + examples re-seed.
    if (state.sqlDataset !== state.dataset) {
      state.sqlDataset = state.dataset;
      $("sqlEditor").value = ""; $("sqlOut").innerHTML = ""; $("sqlMeta").textContent = "";
    }
    seg.querySelectorAll("[data-sb]").forEach((b) => b.classList.toggle("active", b.dataset.sb === state.sqlBackend));
    renderSqlExamples();
    if (!$("sqlEditor").value.trim()) {
      const first = (comp.tables && comp.tables[0] && comp.tables[0].name) || "Class_";
      $("sqlEditor").value = `SELECT * FROM "${first}" LIMIT 25`;
    }
  }

  // Prepare an engine for arbitrary SQL: register friendly views over the cached
  // or lazy companion tables, then return { cached, run(sql) -> {cols, rows} }.
  async function sqlRunner(backend) {
    const comp = currentCompanion(), ds = state.dataset;
    if (backend === "sqlite") {
      const key = `${ds}::sqlite`;
      if (await idbGetMeta(key)) {
        if (!(_sqliteFull && _sqliteFull.key === key)) {
          if (_sqliteFull) { try { _sqliteFull.db.close(); } catch (e) { /* ignore */ } }
          _sqliteFull = { key, db: await ensureSqliteFull(await idbGetFile(key)) };
        }
        const db = _sqliteFull.db;
        return { cached: true, run: async (sql) => { const r = db.exec(sql); const cols = r.length ? r[0].columns : []; return { cols, rows: r.length ? r[0].values.map((v) => Object.fromEntries(cols.map((c, i) => [c, v[i]]))) : [] }; } };
      }
      const w = await ensureSqlite(comp);
      return { cached: false, run: async (sql) => { const rows = await w.db.query(sql); return { cols: rows.length ? Object.keys(rows[0]) : [], rows }; } };
    }
    const d = await ensureDuck();
    let cached = false;
    if (backend === "duck-db") {
      const key = `${ds}::duckdb`, vfs = "cache_" + ds + ".duckdb";
      if (d.attached === "cache:" + vfs) cached = true;
      else if (await idbGetMeta(key)) {
        await duckRegisterCached(d, vfs, await idbGetFile(key));
        if (d.attached) { try { await d.conn.query("DETACH wd"); } catch (e) { /* not attached */ } }
        await d.conn.query(`ATTACH '${vfs}' AS wd (READ_ONLY)`); d.attached = "cache:" + vfs; cached = true;
      } else { await ensureDuckAttach(comp); }
      for (const t of (comp.tables || [])) await d.conn.query(`CREATE OR REPLACE VIEW "${t.name}" AS SELECT * FROM wd."${t.name}"`);
    } else { // duck-parquet
      for (const t of (comp.tables || [])) {
        const vfs = "cache_" + ds + "_" + t.file;
        let src;
        if (d.registered && d.registered.has(vfs)) { src = `read_parquet('${vfs}')`; cached = true; }
        else if (await idbGetMeta(`${ds}::parquet::${t.file}`)) { await duckRegisterCached(d, vfs, await idbGetFile(`${ds}::parquet::${t.file}`)); src = `read_parquet('${vfs}')`; cached = true; }
        else src = `read_parquet('${companionUrl(comp.parquetDir + "/" + t.file)}')`;
        await d.conn.query(`CREATE OR REPLACE VIEW "${t.name}" AS SELECT * FROM ${src}`);
      }
    }
    return { cached, run: (sql) => d.conn.query(sql).then((res) => { const [cols, rows] = duckRows(res); return { cols, rows }; }) };
  }

  function renderSqlResult(cols, rows, ms, cached) {
    const cell = (v) => {
      if (v == null) return "";
      const s = typeof v === "object" ? JSON.stringify(v) : String(v);
      return esc(shorten(s, 80));
    };
    if (!cols.length) {
      $("sqlOut").innerHTML = `<p class="microcopy">Query ran — no columns returned.</p>`;
    } else {
      const head = `<tr>${cols.map((c) => `<th>${esc(c)}</th>`).join("")}</tr>`;
      const body = rows.map((r) => `<tr>${cols.map((c) => `<td>${cell(r[c])}</td>`).join("")}</tr>`).join("");
      $("sqlOut").innerHTML = `<div class="tbl"><table><thead>${head}</thead><tbody>${body}</tbody></table></div>`;
    }
    $("sqlMeta").textContent = `${rows.length} rows · ${ms.toFixed(0)} ms${cached ? " · cached" : ""}`;
  }

  async function runSql() {
    const sql = $("sqlEditor").value.trim(); if (!sql) return;
    $("sqlOut").innerHTML = netSpinner("running SQL on " + state.sqlBackend + "…");
    $("sqlMeta").textContent = "";
    const t0 = performance.now();
    try {
      const runner = await sqlRunner(state.sqlBackend);
      const { cols, rows } = await runner.run(sql);
      renderSqlResult(cols, rows, performance.now() - t0, runner.cached);
    } catch (e) {
      showError("sqlOut", "SQL failed: " + String(e && e.message || e));
      $("sqlMeta").textContent = "";
    }
  }

  // Render the class chips and wire each to open that class at page 0. Shared by
  // the in-memory and remote class lists. `classes` is [[iri, count], …].
  function renderClassButtons(classes) {
    const hit = classes.find(([c]) => c === state.exploreClass);
    if (!hit) { state.exploreClass = classes[0][0]; state.exploreCount = Number(classes[0][1]) || 0; }
    else state.exploreCount = Number(hit[1]) || 0;
    $("exploreClasses").innerHTML = classes.map(([c, n]) =>
      `<button type="button" data-cls="${esc(c)}" data-count="${esc(n)}" class="${c === state.exploreClass ? "active" : ""}">` +
        `${esc(shorten(localName(c), 22))} (${esc(n)})` +
      `</button>`).join("");
    $$("#exploreClasses [data-cls]").forEach((btn) => {
      btn.onclick = () => {
        $$("#exploreClasses [data-cls]").forEach((b) => b.classList.toggle("active", b === btn));
        openExploreClass(btn.dataset.cls, btn.dataset.count);
      };
    });
    openExploreClass(state.exploreClass, state.exploreCount);
  }

  // Open a class fresh: reset to page 0 and recompute its (stable) column set.
  function openExploreClass(cls, count) {
    state.exploreClass = cls;
    state.exploreCount = Number(count) || 0;
    state.exploreCols = null;
    loadEntityPage(0);
  }

  // Fetch one page of a class's entities, then their properties, and render it:
  // the lazy, paginated entity table (the HF dataset-viewer pattern). In remote
  // mode each page range-reads only the tiles those 25 entities touch.
  function loadEntityPage(page) {
    state.explorePage = page;
    // A companion backend (Parquet/DuckDB/SQLite) takes a different path entirely.
    if (state.exploreBackend && state.exploreBackend !== "native") return loadEntityPageColumnar(page);
    const cls = state.exploreClass;
    const tp = currentTypePredicate();
    if (state.remote) $("exploreTable").innerHTML = netSpinner("fetching page over range…");
    const t0 = performance.now();
    exploreQuery(`SELECT DISTINCT ?s WHERE { ?s ${tp} ${cls} } ORDER BY ?s LIMIT ${ENTITY_PAGE} OFFSET ${page * ENTITY_PAGE}`)
      .then((ent) => {
        const ids = (ent.rows || []).map((r) => r.s).filter(Boolean);
        if (!ids.length) { renderEntityPage(cls, [], []); setExploreNativeMeta(0, performance.now() - t0, ent); return null; }
        return exploreQuery(`SELECT ?s ?p ?o WHERE { VALUES ?s { ${ids.join(" ")} } ?s ?p ?o }`)
          .then((res) => { renderEntityPage(cls, ids, res.rows || []); setExploreNativeMeta(ids.length, performance.now() - t0, ent, res); });
      })
      .catch((e) => showError("exploreTable", "Explore failed: " + String(e)));
  }

  // Pivot one page's rows into the entity table + pager — entities down the
  // rows, their most frequent properties across the columns (the column set is
  // cached per class in `state.exploreCols` so it stays stable as you page).
  function renderEntityPage(cls, ids, rows) {
    const tp = currentTypePredicate();
    const entities = new Map();
    ids.forEach((s) => entities.set(s, new Map()));
    const predCount = new Map();
    for (const row of rows) {
      if (row.p === tp) continue;
      if (!entities.has(row.s)) entities.set(row.s, new Map());
      const props = entities.get(row.s);
      if (!props.has(row.p)) props.set(row.p, []);
      props.get(row.p).push(row.o);
      predCount.set(row.p, (predCount.get(row.p) || 0) + 1);
    }
    if (!state.exploreCols) {
      state.exploreCols = Array.from(predCount.entries())
        .sort((a, b) => b[1] - a[1]).slice(0, 8).map(([p]) => p);
    }
    const cols = state.exploreCols;
    const cell = (vals) => {
      if (!vals) return "";
      // Clean, HF-style cells: local names for IRIs, the value for literals.
      const shown = vals.slice(0, 3).map((v) => termLabel(parseTerm(v))).join("; ");
      return vals.length > 3 ? `${shown} (+${vals.length - 3})` : shown;
    };
    const head = `<tr><th>${esc(localName(cls))}</th>` +
      cols.map((c) => `<th>${esc(shorten(localName(c), 20))}</th>`).join("") + `</tr>`;
    const rowHtmls = ids.map((s) => {
      const props = entities.get(s) || new Map();
      return `<tr><td class="iri">${esc(shorten(s, 42))}</td>` +
        cols.map((c) => `<td>${esc(cell(props.get(c)))}</td>`).join("") + `</tr>`;
    });
    const total = state.exploreCount || 0;
    const pages = Math.max(1, Math.ceil(total / ENTITY_PAGE));
    const page = state.explorePage;
    const from = ids.length ? page * ENTITY_PAGE + 1 : 0;
    const to = page * ENTITY_PAGE + ids.length;
    const body = ids.length
      ? `<div class="tbl"><table><thead>${head}</thead><tbody>${rowHtmls.join("")}</tbody></table></div>`
      : `<p class="microcopy">No entities on this page.</p>`;
    $("exploreTable").innerHTML = body +
      `<div class="entity-pager">` +
        `<button type="button" id="entPrev" class="secondary"${page <= 0 ? " disabled" : ""}>‹ Prev</button>` +
        `<span class="pager-info">${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()} ` +
        `${esc(localName(cls))} · page ${(page + 1).toLocaleString()} / ${pages.toLocaleString()}</span>` +
        `<button type="button" id="entNext" class="secondary"${page + 1 >= pages ? " disabled" : ""}>Next ›</button>` +
      `</div>` +
      `<p class="microcopy">Top ${cols.length} properties shown — open the SPARQL tab for full values` +
        `${state.remote ? " · each page is fetched lazily over HTTP range" : ""}.</p>`;
    const prev = $("entPrev"), next = $("entNext");
    if (prev) prev.onclick = () => loadEntityPage(page - 1);
    if (next) next.onclick = () => loadEntityPage(page + 1);
  }

  // The "cluster of clusters": outer circles are the coarsest dendrogram
  // round; nested circles are the next finer round's communities they merge.
  function renderPyramid() {
    let tree;
    try {
      tree = JSON.parse(state.graph.pyramid_tree());
    } catch (e) {
      $("pyramidNote").textContent = "pyramid error: " + String(e);
      return;
    }
    if (!tree.rounds) {
      $("pyramidNote").textContent = "This graph has no community structure (one community holds everything).";
      $("pyramidViz").innerHTML = "";
      $("pyramidLevels").innerHTML = "";
      return;
    }
    const chain = tree.levels.map((l) => l.length).reverse().join(" → ");
    $("pyramidNote").textContent =
      `A community is a group of subjects more densely connected to each other than to the rest ` +
      `of the graph, found by repeated Louvain clustering. Each clustering round merges ` +
      `communities into coarser ones — the pyramid. This file: ${tree.rounds} round(s), ` +
      `coarsest → finest ${chain} communities. These are the same rounds the “Split by ` +
      `community” Round field selects, and the units the pyramid summary aggregates.`;
    $("pyramidLegend").innerHTML =
      `<span class="lg"><span class="sw sw-pyr-outer"></span>outer circle = one coarsest-round community (area ∝ member nodes)</span>` +
      `<span class="lg"><span class="sw sw-pyr-inner"></span>nested bubble = a finer-round community it absorbs — the cluster of clusters</span>` +
      `<span class="lg">hover any circle for its exact node and triple counts</span>`;

    const outer = tree.levels[tree.rounds - 1].slice().sort((a, b) => b.nodes - a.nodes);
    const inner = tree.rounds >= 2 ? tree.levels[tree.rounds - 2] : null;
    const children = new Map();
    if (inner) {
      for (const c of inner) {
        if (!children.has(c.parent)) children.set(c.parent, []);
        children.get(c.parent).push(c);
      }
    }
    const shown = outer.slice(0, 24);
    const totalNodes = shown.reduce((a, c) => a + c.nodes, 0) || 1;
    const width = 920;
    const items = shown.map((c) => ({ c, R: Math.max(24, Math.sqrt(c.nodes / totalNodes) * 240) }));
    let x = 14, y = 16, rowH = 0;
    for (const it of items) {
      const d = it.R * 2 + 14;
      if (x + d > width) { x = 14; y += rowH + 14; rowH = 0; }
      it.cx = x + it.R;
      it.cy = y + it.R;
      x += d;
      rowH = Math.max(rowH, it.R * 2);
    }
    const height = y + rowH + 16;
    let svg = `<svg viewBox="0 0 ${width} ${Math.max(height, 140)}" role="img" aria-label="Community pyramid">`;
    for (const it of items) {
      svg += `<circle class="pyr-outer" cx="${it.cx.toFixed(1)}" cy="${it.cy.toFixed(1)}" r="${it.R.toFixed(1)}">` +
        `<title>round ${tree.rounds - 1} community C${it.c.id}: ${it.c.nodes} nodes, ${it.c.triples} triples</title></circle>`;
      const kids = (children.get(it.c.id) || []).sort((a, b) => b.nodes - a.nodes).slice(0, 40);
      const kTotal = kids.reduce((a, k) => a + k.nodes, 0) || 1;
      kids.forEach((k, i) => {
        const angle = i * 2.399963;
        const dist = (it.R * 0.6) * Math.sqrt((i + 0.5) / kids.length);
        const r = Math.max(3, Math.sqrt(k.nodes / kTotal) * it.R * 0.42);
        svg += `<circle class="pyr-inner" cx="${(it.cx + Math.cos(angle) * dist).toFixed(1)}" ` +
          `cy="${(it.cy + Math.sin(angle) * dist).toFixed(1)}" r="${r.toFixed(1)}">` +
          `<title>round ${tree.rounds - 2} community C${k.id}: ${k.nodes} nodes, ${k.triples} triples</title></circle>`;
      });
      if (it.R >= 30) {
        svg += `<text class="pyr-label" x="${it.cx.toFixed(1)}" y="${(it.cy - it.R + 14).toFixed(1)}" text-anchor="middle">C${it.c.id}</text>`;
      }
    }
    svg += `</svg>`;
    $("pyramidViz").innerHTML = svg +
      (outer.length > 24 ? `<p class="microcopy" style="padding:4px 10px">Showing the 24 largest of ${outer.length} top-level communities.</p>` : "");
    $("pyramidLevels").innerHTML =
      `<table><thead><tr><th>round</th><th>communities</th><th>largest (nodes)</th><th>largest (triples)</th></tr></thead><tbody>` +
      tree.levels.map((l, r) =>
        `<tr><td>${r}${r === tree.rounds - 1 ? " (coarsest)" : r === 0 ? " (finest)" : ""}</td>` +
        `<td>${l.length}</td><td>${Math.max(...l.map((c) => c.nodes))}</td>` +
        `<td>${Math.max(...l.map((c) => c.triples))}</td></tr>`).join("") +
      `</tbody></table>`;
  }

  // The byte map: every byte of the file as a wrapped grid of cells, colored
  // by the section it belongs to — where the data physically lives.
  const LAYOUT_COLORS = {
    header: "#17211d",
    metadata: "#7b5ea7",
    dictionary: "#147d69",
    directory: "#9fb5ac",
    pyramid: "#b98112",
    "named-graphs": "#235c7c",
    framing: "#e3e9e6"
  };
  const TILE_COLORS = ["#c84f2f", "#e0876a"];

  // Byte ranges the last Provenance run touched — the heat overlay.
  function touchedRanges() {
    const results = (state.lastProvenance && state.lastProvenance.results) || [];
    const out = [];
    for (const r of results.slice(0, 500)) {
      const p = r.provenance || {};
      for (const key of ["dictionaryRange", "indexSectionRange"]) {
        if (p[key]) out.push(p[key]);
      }
      if (p.tile && p.tile.range) out.push(p.tile.range);
    }
    return out;
  }

  function renderLayout() {
    if (!state.bytes) return;
    let lay;
    try {
      lay = JSON.parse(state.graph.file_layout());
    } catch (e) {
      $("layoutNote").textContent = "layout error: " + String(e);
      return;
    }
    const segs = lay.segments;
    const total = lay.fileLength || 1;
    // Pre-index tiles for alternating shades.
    let tileSeq = 0;
    segs.forEach((s) => { if (s.kind === "tile") s.tile = tileSeq++; });

    // Cell size: each square is exactly `perCell` bytes. Auto picks the
    // smallest power of two that keeps the grid under ~1536 cells; an explicit
    // choice that would exceed 4096 cells falls back to auto with a note.
    const MAX_CELLS = 4096;
    const choice = $("layoutCell").value;
    const autoSize = () => {
      let s = 16;
      while (Math.ceil(total / s) > 1536) s *= 2;
      return s;
    };
    let perCell = choice === "auto" ? autoSize() : Number(choice);
    let fellBackCell = false;
    if (Math.ceil(total / perCell) > MAX_CELLS) {
      perCell = autoSize();
      fellBackCell = true;
    }
    const cells = Math.max(1, Math.ceil(total / perCell));
    const cols = Math.min(96, Math.max(24, Math.ceil(Math.sqrt(cells * 3))));
    const size = 9;
    const rows = Math.ceil(cells / cols);

    // Heat overlay: cells intersecting the byte ranges of the last
    // Provenance run (what a remote client would actually fetch).
    const hot = touchedRanges();
    const isHot = (lo, hi) => hot.some((r) => r.offset < hi && lo < r.offset + r.len);

    // Dominant-section coloring: a cell takes the section owning most of its
    // bytes; its opacity is that section's share, so paler = a boundary cell.
    let si = 0;
    let svg = `<svg viewBox="0 0 ${cols * size} ${rows * size}" role="img" aria-label="File byte map" style="max-width:100%">`;
    for (let i = 0; i < cells; i++) {
      const lo = i * perCell;
      const hi = Math.min(total, lo + perCell);
      while (si < segs.length && segs[si].offset + segs[si].len <= lo) si++;
      let best = null, bestBytes = 0, covered = 0;
      for (let j = si; j < segs.length && segs[j].offset < hi; j++) {
        const ov = Math.min(hi, segs[j].offset + segs[j].len) - Math.max(lo, segs[j].offset);
        if (ov > 0) {
          covered += ov;
          if (ov > bestBytes) { bestBytes = ov; best = segs[j]; }
        }
      }
      const framingBytes = (hi - lo) - covered;
      const useFraming = framingBytes > bestBytes;
      const frac = (useFraming ? framingBytes : bestBytes) / (hi - lo);
      const color = useFraming || !best ? LAYOUT_COLORS.framing
        : best.kind === "tile" ? TILE_COLORS[best.tile % 2]
        : (LAYOUT_COLORS[best.kind] || LAYOUT_COLORS.framing);
      const label = (useFraming || !best
        ? "container framing (section directories, length fields)"
        : `${best.label} — bytes ${best.offset}–${best.offset + best.len} (${formatBytes(best.len)})`) +
        ` | cell: bytes ${lo}–${hi}` + (frac < 1 ? ` (${Math.round(frac * 100)}% of cell)` : "");
      const heat = isHot(lo, hi);
      svg += `<rect x="${(i % cols) * size}" y="${Math.floor(i / cols) * size}" width="${size - 1}" height="${size - 1}" ` +
        `fill="${color}" fill-opacity="${(0.35 + 0.65 * frac).toFixed(2)}"` +
        (heat ? ` stroke="#17211d" stroke-width="1.4"` : "") +
        `><title>${esc(label + (heat ? " | touched by your last Provenance query" : ""))}</title></rect>`;
    }
    svg += `</svg>`;
    $("layoutViz").innerHTML = svg;
    $("layoutNote").textContent =
      `Each square is exactly ${formatBytes(perCell)} of the ${formatBytes(total)} file, in byte order ` +
      `(left→right, top→bottom)${fellBackCell ? " — the requested cell size was too fine for this file, using auto" : ""}. ` +
      `A paler square spans a section boundary. This is the surface a range query navigates: read the ` +
      `header, then jump straight to the squares you need.` +
      (hot.length ? " Outlined squares are the bytes your last Provenance query touched." :
        " Run a Provenance example and come back: the touched bytes get outlined.");
    const legendKinds = [
      ["header", "header"], ["metadata", "metadata"], ["dictionary", "dictionary"],
      ["directory", "tile directories"], ["tile", "index tiles (alternating per tile)"],
      ["pyramid", "pyramid summary"], ["named-graphs", "named graphs"], ["framing", "framing"]
    ];
    $("layoutLegend").innerHTML = legendKinds
      .filter(([k]) => k === "framing" || k === "tile" || segs.some((s) => s.kind === k))
      .map(([k, label]) =>
        `<span class="lg"><span class="sw" style="background:${k === "tile" ? TILE_COLORS[0] : LAYOUT_COLORS[k]}"></span>${esc(label)}</span>`)
      .join("") +
      (hot.length ? `<span class="lg"><span class="sw" style="background:#fff;border:2px solid #17211d"></span>touched by last Provenance query</span>` : "");
    // Per-kind byte totals.
    const sums = new Map();
    segs.forEach((s) => sums.set(s.kind, (sums.get(s.kind) || 0) + s.len));
    const coveredTotal = Array.from(sums.values()).reduce((a, b) => a + b, 0);
    sums.set("framing", Math.max(0, total - coveredTotal));
    $("layoutTable").innerHTML = collapsedTable(
      `<tr><th>section</th><th>bytes</th><th>share</th></tr>`,
      Array.from(sums.entries()).sort((a, b) => b[1] - a[1]).map(([k, n]) =>
        `<tr><td>${esc(k)}</td><td>${formatBytes(n)}</td><td>${(100 * n / total).toFixed(1)}%</td></tr>`)
    );
  }

  function updateResultVisibility() {
    $$(".result-pane").forEach((pane) => pane.classList.add("hidden"));
    if (state.mode === "sparql") {
      $("out").classList.remove("hidden");
      if ($("commOut").innerHTML.trim()) $("commOut").classList.remove("hidden");
    } else if (state.mode === "shacl") {
      $("shaclOut").classList.remove("hidden");
    } else if (state.mode === "reach") {
      $("reachOut").classList.remove("hidden");
    } else if (state.mode === "schema") {
      $("schemaOut").classList.remove("hidden");
    } else if (state.mode === "coherence") {
      $("coherenceOut").classList.remove("hidden");
    } else if (state.mode === "provenance") {
      $("provOut").classList.remove("hidden");
    } else if (state.mode === "build") {
      $("buildOut").classList.remove("hidden");
    }
  }

  function setView(view) {
    $("fmt").value = view;
  }

  // Output types that are all renderings of the SAME SELECT bindings (the engine
  // returns table rows; Graph/Map/Time just draw them differently). Switching
  // among these never needs the query to run again — only a re-render. The
  // serialization views (TTL/JSON-LD) are a different engine output, so they
  // still run.
  const ROW_VIEWS = new Set(["table", "graph", "map", "time"]);

  // Changing the Output type re-renders the last result in the new view with no
  // re-run, whenever that's possible: the cached result must be row-shaped, the
  // new view a row view, and the query/strategy/dataset unchanged since it ran.
  // Anything else (a serialization target, an edited query, a stale or missing
  // cache) falls through to a normal run — which keeps the row cache, so a
  // round-trip through TTL/JSON-LD and back to a row view re-renders for free.
  function onOutputTypeChange() {
    const fmt = $("fmt").value;
    const c = state.lastResult;
    // A federated result is row-shaped and self-contained — re-render it (with its
    // per-source banner) in the new view rather than re-running every source.
    if (c && c.federated && c.q === $("q").value.trim() && ROW_VIEWS.has(fmt)) {
      const renderFmt = (fmt === "graph" && c.res.kind !== "construct") ? "table" : fmt;
      const summary = renderResult(c.res, renderFmt);
      $("out").innerHTML = (c.fedBannerHtml || "") + $("out").innerHTML;
      $("qmeta").textContent = `${summary} · re-rendered from the last federated result (no re-run)`;
      updateResultVisibility();
      return;
    }
    const sameStrategy = !c ? false : c.remote ? true : c.strategy === $("strategy").value;
    const reusable = !!c && c.rowShaped && ROW_VIEWS.has(fmt) &&
      c.q === $("q").value.trim() && c.remote === !!state.remote &&
      c.dataset === state.dataset && sameStrategy;
    if (!reusable) return runQuery();
    // Remote Graph has no local bytes to expand a CONSTRUCT, so the run path
    // renders it as a table — match that when re-rendering from cache.
    const viewFmt = c.remote && fmt === "graph" ? "table" : fmt;
    const summary = renderResult(c.res, viewFmt);
    $("qmeta").textContent = `${summary} · re-rendered from the last result (no re-run)`;
    updateResultVisibility();
  }

  function setStrategy(strategy) {
    $("strategy").value = strategy || "whole";
    const noRound = $("strategy").value !== "community";
    $("roundWrap").classList.toggle("hidden", noRound);
    $("roundHelp").classList.toggle("hidden", noRound);
  }

  // How many rows a table shows before its "Show more" button, and how many
  // each click reveals.
  const TABLE_HEAD_ROWS = 12;
  const TABLE_MORE_STEP = 50;

  /// Wrap table row strings into a collapsed table: the first TABLE_HEAD_ROWS
  /// rows show; the rest hide behind a "Show more" button (a delegated click
  /// handler in wireEvents reveals them in steps).
  function collapsedTable(headRowHtml, rowHtmls, note) {
    const hidden = Math.max(0, rowHtmls.length - TABLE_HEAD_ROWS);
    const body = rowHtmls
      .map((r, i) => (i < TABLE_HEAD_ROWS ? r : r.replace("<tr", `<tr class="tr-hidden"`)))
      .join("");
    return (note || "") +
      `<div class="tbl"><table><thead>${headRowHtml}</thead><tbody>${body}</tbody></table>` +
      (hidden > 0
        ? `<button type="button" class="tbl-more secondary">Show ${Math.min(hidden, TABLE_MORE_STEP)} more (${hidden} hidden)</button>`
        : "") +
      `</div>`;
  }

  // A clear empty state beats a bare header row — especially for custom queries
  // on remote datasets, where "did it work?" and "matched nothing" look alike.
  function emptyState(what) {
    return `<div class="note">The query ran successfully but matched <strong>no ${esc(what)}</strong>. ` +
      `Check bound IRIs and prefixes, or relax a FILTER — the graph just has nothing for this pattern.</div>`;
  }

  // Friendly table cell for an RDF term: strip the quotes + datatype IRI from a
  // literal (keep the value), show a language tag compactly, drop the <> from an
  // IRI. The full canonical term (with datatype) is kept on hover, so nothing is
  // lost — `"113.149"^^<…#decimal>` renders as `113.149`, `"Bemelen"@en` as
  // `Bemelen @en`, `<http://…/Q5>` as `http://…/Q5`.
  const NUM_DT = /#(decimal|double|float|integer|int|long|short|byte|nonNegativeInteger|nonPositiveInteger|positiveInteger|negativeInteger|unsignedLong|unsignedInt|unsignedShort|unsignedByte)$/;
  function prettyCell(raw) {
    if (raw == null || raw === "") return `<td></td>`;
    const t = parseTerm(raw);
    if (t.iri) {
      const disp = shorten(t.value, 96);
      return `<td class="iri"${disp !== t.value ? ` title="${esc(t.value)}"` : ""}>${esc(disp)}</td>`;
    }
    const num = t.datatype && NUM_DT.test(t.datatype);
    const lang = t.lang ? ` <span class="t-lang">@${esc(t.lang)}</span>` : "";
    return `<td class="lit${num ? " num" : ""}" title="${esc(raw)}">${esc(shorten(t.value, 110))}${lang}</td>`;
  }

  function renderTable(vars, rows) {
    if (!(rows || []).length) return emptyState("rows");
    const cap = 500;
    const shown = (rows || []).slice(0, cap);
    const head = `<tr>${(vars || []).map((v) => `<th>${esc(v)}</th>`).join("")}</tr>`;
    const rowHtmls = shown.map((row) =>
      `<tr>${(vars || []).map((v) => prettyCell(row[v])).join("")}</tr>`);
    const note = (rows || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${rows.length} rows.</p>`
      : "";
    return collapsedTable(head, rowHtmls, note);
  }

  function renderTriplesTable(triples) {
    if (!(triples || []).length) return emptyState("triples");
    const cap = 500;
    const shown = (triples || []).slice(0, cap);
    const rowHtmls = shown.map((t) =>
      `<tr>${prettyCell(t[0])}${prettyCell(t[1])}${prettyCell(t[2])}</tr>`);
    const note = (triples || []).length > cap
      ? `<p class="microcopy">Showing first ${cap} of ${triples.length} triples.</p>`
      : "";
    return collapsedTable(`<tr><th>subject</th><th>predicate</th><th>object</th></tr>`, rowHtmls, note);
  }

  function triplesForGraph(res) {
    if (res.triples) return res.triples;
    if (res.kind !== "select") return [];
    const vars = res.vars || [];
    if (vars.length >= 3) return res.rows.map((r) => [r[vars[0]], r[vars[1]], r[vars[2]]]);
    if (vars.length === 2) return res.rows.map((r) => [r[vars[0]], "related", r[vars[1]]]);
    return [];
  }

  function renderGraph(triples) {
    const out = $("out");
    if (!triples || !triples.length) {
      out.innerHTML = `<div class="note">Graph view needs triples. Use a CONSTRUCT query or a SELECT with at least two columns.</div>`;
      return "graph: 0 edges";
    }

    const cap = 90;
    const nodeMap = new Map();
    const nodes = [];
    const edges = [];
    const addNode = (term) => {
      if (!nodeMap.has(term) && nodes.length < cap) {
        const i = nodes.length;
        const angle = i * 2.399963;
        const radius = 34 + 7 * Math.sqrt(i);
        nodeMap.set(term, i);
        nodes.push({
          term,
          label: shorten(term, 28),
          x: 460 + Math.cos(angle) * radius,
          y: 260 + Math.sin(angle) * radius
        });
      }
      return nodeMap.get(term);
    };

    triples.forEach((t) => {
      const s = addNode(String(t[0]));
      const o = addNode(String(t[2]));
      if (s != null && o != null) edges.push({ s, o, p: String(t[1]) });
    });

    for (let iter = 0; iter < 110; iter++) {
      for (let i = 0; i < nodes.length; i++) {
        for (let j = i + 1; j < nodes.length; j++) {
          const a = nodes[i], b = nodes[j];
          const dx = a.x - b.x || 0.01;
          const dy = a.y - b.y || 0.01;
          const d2 = dx * dx + dy * dy;
          const f = Math.min(260 / d2, 0.035);
          a.x += dx * f; a.y += dy * f;
          b.x -= dx * f; b.y -= dy * f;
        }
      }
      edges.forEach((e) => {
        const a = nodes[e.s], b = nodes[e.o];
        const dx = b.x - a.x, dy = b.y - a.y;
        a.x += dx * 0.012; a.y += dy * 0.012;
        b.x -= dx * 0.012; b.y -= dy * 0.012;
      });
      nodes.forEach((n) => {
        n.x += (460 - n.x) * 0.01;
        n.y += (260 - n.y) * 0.01;
        n.x = Math.max(28, Math.min(892, n.x));
        n.y = Math.max(28, Math.min(492, n.y));
      });
    }

    let svg = `<svg viewBox="0 0 920 520" role="img" aria-label="Graph result">`;
    svg += `<defs><marker id="arrow" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0,0 L7,3.5 L0,7 z" fill="#99a991"></path></marker></defs>`;
    edges.forEach((e, i) => {
      const a = nodes[e.s], b = nodes[e.o];
      svg += `<line class="gedge" data-s="${e.s}" data-t="${e.o}" x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" marker-end="url(#arrow)"></line>`;
      if (i < 70) {
        svg += `<text class="gedge-label" data-s="${e.s}" data-t="${e.o}" x="${((a.x + b.x) / 2).toFixed(1)}" y="${((a.y + b.y) / 2).toFixed(1)}">${esc(shorten(e.p, 22))}</text>`;
      }
    });
    nodes.forEach((n, i) => {
      svg += `<g class="gnodeg" data-i="${i}"><circle class="gnode" cx="${n.x.toFixed(1)}" cy="${n.y.toFixed(1)}" r="7"><title>${esc(n.term)}</title></circle><text class="gnode-label" x="${(n.x + 10).toFixed(1)}" y="${(n.y + 4).toFixed(1)}">${esc(n.label)}</text></g>`;
    });
    svg += `</svg>`;

    const truncated = nodeMap.size >= cap;
    out.innerHTML = `<p class="microcopy">${nodes.length} nodes | ${edges.length} edges | drag nodes to adjust layout.</p>` +
      (truncated ? `<div class="note">Graph capped at ${cap} nodes for legibility.</div>` : "") +
      `<div class="graphwrap">${svg}</div>`;
    enableGraphDrag(out.querySelector("svg"), nodes);
    return `graph: ${nodes.length} nodes, ${edges.length} edges`;
  }

  function enableGraphDrag(svg, nodes) {
    if (!svg) return;
    let dragging = null;
    const point = (ev) => {
      const rect = svg.getBoundingClientRect();
      return {
        x: (ev.clientX - rect.left) / rect.width * 920,
        y: (ev.clientY - rect.top) / rect.height * 520
      };
    };
    $$(".gnodeg", svg).forEach((g) => {
      g.addEventListener("mousedown", (ev) => {
        dragging = Number(g.dataset.i);
        svg.classList.add("grabbing");
        ev.preventDefault();
      });
    });
    svg.addEventListener("mousemove", (ev) => {
      if (dragging == null) return;
      const p = point(ev);
      const n = nodes[dragging];
      n.x = Math.max(28, Math.min(892, p.x));
      n.y = Math.max(28, Math.min(492, p.y));
      const g = svg.querySelector(`.gnodeg[data-i="${dragging}"]`);
      g.querySelector("circle").setAttribute("cx", n.x.toFixed(1));
      g.querySelector("circle").setAttribute("cy", n.y.toFixed(1));
      g.querySelector("text").setAttribute("x", (n.x + 10).toFixed(1));
      g.querySelector("text").setAttribute("y", (n.y + 4).toFixed(1));
      $$(`line.gedge[data-s="${dragging}"], line.gedge[data-t="${dragging}"]`, svg).forEach((line) => {
        const a = nodes[Number(line.dataset.s)], b = nodes[Number(line.dataset.t)];
        line.setAttribute("x1", a.x.toFixed(1));
        line.setAttribute("y1", a.y.toFixed(1));
        line.setAttribute("x2", b.x.toFixed(1));
        line.setAttribute("y2", b.y.toFixed(1));
      });
      $$(`text.gedge-label[data-s="${dragging}"], text.gedge-label[data-t="${dragging}"]`, svg).forEach((txt) => {
        const a = nodes[Number(txt.dataset.s)], b = nodes[Number(txt.dataset.t)];
        txt.setAttribute("x", ((a.x + b.x) / 2).toFixed(1));
        txt.setAttribute("y", ((a.y + b.y) / 2).toFixed(1));
      });
    });
    const end = () => {
      dragging = null;
      svg.classList.remove("grabbing");
    };
    svg.addEventListener("mouseup", end);
    svg.addEventListener("mouseleave", end);
  }

  function renderProgressiveInfo(meta) {
    state.lastProgressive = meta;
    if (!meta) {
      $("progressiveInfo").innerHTML = `<div>Run a Summary-family example with the progressive strategy.</div>`;
      return;
    }
    $("progressiveInfo").innerHTML =
      `<div class="metric-grid">` +
      metric("Exact", meta.exact ? "yes" : "no") +
      metric("Index skipped", meta.readsIndex ? "no" : "yes") +
      metric("Bytes", formatBytes(meta.bytes)) +
      metric("Range reads", String(meta.requests || 0)) +
      `</div>` +
      `<div>Shape: <code>${esc(meta.queryShape || "summary")}</code></div>` +
      (meta.predicate ? `<div>Predicate: <span class="iri">${esc(shorten(meta.predicate))}</span></div>` : "");
  }

  function metric(label, value) {
    return `<div class="metric"><strong>${esc(value)}</strong><span>${esc(label)}</span></div>`;
  }

  function progressiveBanner(meta) {
    if (!meta) return "";
    return `<div class="meta-strip">` +
      `<span class="meta-chip"><strong>exact</strong> ${meta.exact ? "yes" : "no"}</span>` +
      `<span class="meta-chip"><strong>index</strong> ${meta.readsIndex ? "read" : "skipped"}</span>` +
      `<span class="meta-chip"><strong>bytes</strong> ${formatBytes(meta.bytes)}</span>` +
      `<span class="meta-chip"><strong>ranges</strong> ${esc(meta.requests || 0)}</span>` +
      `</div>`;
  }

  // --- Map & Time views: render SELECT bindings geographically / temporally ---

  // Parse a SPARQL JSON term string into {iri, value, datatype, lang}.
  function parseTerm(v) {
    const s = String(v == null ? "" : v);
    if (s.startsWith("<") && s.endsWith(">")) return { iri: true, value: s.slice(1, -1) };
    const m = /^"((?:[^"\\]|\\.)*)"(?:\^\^<([^>]+)>|@([\w-]+))?$/s.exec(s);
    if (m) return { iri: false, value: m[1].replace(/\\(.)/g, "$1"), datatype: m[2] || null, lang: m[3] || null };
    return { iri: false, value: s, datatype: null };
  }
  const termLabel = (t) => t.iri ? shorten(localName(t.value) || t.value, 60) : shorten(t.value, 60);

  const WKT_RE = /\b(POINT|LINESTRING|POLYGON|MULTIPOINT|MULTILINESTRING|MULTIPOLYGON|GEOMETRYCOLLECTION)\b\s*[ZM]*\s*\(/i;
  function detectGeoCol(vars, rows) {
    let best = null, hi = 0;
    for (const v of vars) {
      let h = 0;
      for (const r of rows) { const t = parseTerm(r[v]); if (t.value && WKT_RE.test(t.value)) h++; }
      if (h > hi) { hi = h; best = v; }
    }
    return hi > 0 ? best : null;
  }
  // Innermost coordinate rings of a WKT string, each as [[lon,lat],...].
  function wktRings(s) {
    return (s.match(/\(([^()]*)\)/g) || []).map((g) =>
      g.replace(/[()]/g, "").trim().split(",")
        .map((p) => p.trim().split(/\s+/).map(Number))
        .filter((a) => a.length >= 2 && isFinite(a[0]) && isFinite(a[1]))
        .map((a) => [a[0], a[1]])
    ).filter((r) => r.length);
  }

  function termYear(t) {
    if (!t || t.value == null) return null;
    const typed = t.datatype && /gYear|gYearMonth|\bdate\b|dateTime/i.test(t.datatype);
    if (typed) { const m = /^(-?\d{1,6})/.exec(t.value); return m ? parseInt(m[1], 10) : null; }
    const iso = /^(-?\d{1,6})-\d{2}(-\d{2})?/.exec(t.value); if (iso) return parseInt(iso[1], 10);
    return null;
  }
  // The best year column: a typed temporal column, else a plausible year integer.
  function detectTimeCol(vars, rows) {
    let best = null, hi = 0;
    for (const v of vars) {
      let h = 0; for (const r of rows) if (termYear(parseTerm(r[v])) != null) h++;
      if (h > hi) { hi = h; best = v; }
    }
    if (hi > 0) return best;
    // fall back to a bare-integer column that looks like years (spread, year-range)
    for (const v of vars) {
      const ys = rows.map((r) => { const t = parseTerm(r[v]); return /^-?\d{1,6}$/.test(t.value) ? parseInt(t.value, 10) : null; }).filter((y) => y != null);
      if (ys.length >= Math.max(1, rows.length * 0.5)) {
        const mn = Math.min(...ys), mx = Math.max(...ys), uniq = new Set(ys).size;
        if (mn >= -12000 && mx <= 2200 && uniq > 1 && (mx >= 1000 || mn < 0)) return v;
      }
    }
    return null;
  }
  // Permissive year extraction once a temporal column is chosen: typed gYear/date,
  // ISO date, OR a bare integer in a plausible year range (e.g. ex:year 1914).
  function extractYear(t) {
    const y = termYear(t);
    if (y != null) return y;
    if (t && /^-?\d{1,6}$/.test(t.value)) { const n = parseInt(t.value, 10); if (n >= -12000 && n <= 2200) return n; }
    return null;
  }
  const fmtYear = (y) => y < 0 ? `${-y} BCE` : `${y}`;
  const note = (m) => `<div class="note">${esc(m)}</div>`;

  function renderMap(res) {
    if (res.kind !== "select") { $("out").innerHTML = note("Map needs SELECT rows with a geometry column."); return "no map"; }
    const vars = res.vars || [], rows = res.rows || [];
    const geo = detectGeoCol(vars, rows);
    if (!geo) { $("out").innerHTML = note("No geometry in this result — Map needs a WKT column (geo:wktLiteral: POINT / LINESTRING / POLYGON …)."); return "no geometry"; }
    const labelCol = vars.find((v) => v !== geo) || geo;
    const feats = [];
    let minX = 180, maxX = -180, minY = 90, maxY = -90, n = 0;
    for (const r of rows) {
      const wkt = parseTerm(r[geo]).value; if (!WKT_RE.test(wkt)) continue;
      const rings = wktRings(wkt); if (!rings.length) continue;
      const isPoly = /POLYGON/i.test(wkt);
      feats.push({ rings, isPoly, label: termLabel(parseTerm(r[labelCol])) });
      for (const ring of rings) for (const [x, y] of ring) {
        if (x < minX) minX = x; if (x > maxX) maxX = x; if (y < minY) minY = y; if (y > maxY) maxY = y; n++;
      }
    }
    if (!feats.length) { $("out").innerHTML = note("No parseable geometry in this result."); return "no geometry"; }
    const W = 760, H = 420, pad = 14;
    const dx = (maxX - minX) || 1, dy = (maxY - minY) || 1;
    const sx = (W - 2 * pad) / dx, sy = (H - 2 * pad) / dy, s = Math.min(sx, sy);
    const px = (x) => pad + (x - minX) * s + ((W - 2 * pad) - dx * s) / 2;
    const py = (y) => H - pad - (y - minY) * s - ((H - 2 * pad) - dy * s) / 2; // invert lat
    let svg = "";
    for (const f of feats) {
      const title = `<title>${esc(f.label)}</title>`;
      for (const ring of f.rings) {
        if (ring.length === 1) {
          const [x, y] = ring[0];
          svg += `<circle class="mpt" cx="${px(x).toFixed(1)}" cy="${py(y).toFixed(1)}" r="3">${title}</circle>`;
        } else {
          const pts = ring.map(([x, y]) => `${px(x).toFixed(1)},${py(y).toFixed(1)}`).join(" ");
          svg += f.isPoly
            ? `<polygon class="mgeo" points="${pts}">${title}</polygon>`
            : `<polyline class="mline" points="${pts}">${title}</polyline>`;
        }
      }
    }
    const cap = `${feats.length} feature(s) · lon ${minX.toFixed(1)}…${maxX.toFixed(1)}, lat ${minY.toFixed(1)}…${maxY.toFixed(1)} · equirectangular (offline)`;
    $("out").innerHTML = `<div class="mapview"><svg viewBox="0 0 ${W} ${H}" role="img" aria-label="map of results">${svg}</svg>` +
      `<div class="mapcap">${esc(cap)} — hover a feature for its label.</div></div>`;
    return `${feats.length} mapped feature(s)`;
  }

  function renderTime(res) {
    if (res.kind !== "select") { $("out").innerHTML = note("Time needs SELECT rows with a year/date column."); return "no time"; }
    const vars = res.vars || [], rows = res.rows || [];
    const col = detectTimeCol(vars, rows);
    if (!col) { $("out").innerHTML = note("No temporal column in this result — Time needs a year/date value (xsd:gYear, xsd:date, or a year integer)."); return "no temporal data"; }
    const labelCol = vars.find((v) => v !== col) || col;
    const byYear = new Map();
    for (const r of rows) {
      const y = extractYear(parseTerm(r[col])); if (y == null) continue;
      if (!byYear.has(y)) byYear.set(y, []);
      byYear.get(y).push(termLabel(parseTerm(r[labelCol])));
    }
    const years = [...byYear.keys()];
    if (!years.length) { $("out").innerHTML = note("No datable rows in this result."); return "no temporal data"; }
    const min = Math.min(...years), max = Math.max(...years), span = max - min + 1;
    let bucket = 1; for (const sz of [1, 2, 5, 10, 20, 25, 50, 100, 200, 500, 1000]) { if (Math.ceil(span / sz) <= 140) { bucket = sz; break; } }
    const nb = Math.ceil(span / bucket);
    const buckets = Array.from({ length: nb }, (_, i) => ({ from: min + i * bucket, to: min + i * bucket + bucket - 1, count: 0, items: [] }));
    let totalItems = 0;
    for (const [y, items] of byYear) {
      const bi = Math.floor((y - min) / bucket); const b = buckets[bi];
      b.count += items.length; totalItems += items.length;
      for (const it of items) if (b.items.length < 40) b.items.push(it);
    }
    const sorted = buckets.map((b) => b.count).filter((c) => c > 0).sort((a, b) => a - b);
    const q = (p) => sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] : 0;
    const t1 = q(0.25), t2 = q(0.5), t3 = q(0.75);
    const shade = (c) => c === 0 ? 0 : c <= t1 ? 1 : c <= t2 ? 2 : c <= t3 ? 3 : 4;
    const cols = Math.min(nb, 30);
    const cells = buckets.map((b) => {
      const yr = bucket === 1 ? fmtYear(b.from) : `${fmtYear(b.from)}–${fmtYear(b.to)}`;
      const tip = `${yr}: ${b.count} item(s)` + (b.items.length ? "\n" + b.items.slice(0, 20).map((x) => "• " + x).join("\n") + (b.count > 20 ? `\n…(+${b.count - 20} more)` : "") : "");
      return `<div class="tcell l${shade(b.count)}" title="${esc(tip)}"></div>`;
    }).join("");
    const legend = `<span class="tleg-lab">less</span>${[0, 1, 2, 3, 4].map((l) => `<span class="tcell l${l}"></span>`).join("")}<span class="tleg-lab">more</span>`;
    $("out").innerHTML = `<div class="timeview"><div class="taxis"><span>${esc(fmtYear(min))}</span>` +
      `<span class="tmid">${esc(col)} · ${bucket === 1 ? "per year" : "per " + bucket + " yr"}</span><span>${esc(fmtYear(max))}</span></div>` +
      `<div class="tgrid" style="grid-template-columns:repeat(${cols}, 1fr)">${cells}</div>` +
      `<div class="tlegend">${legend}</div></div>`;
    return `${totalItems} dated item(s) · ${fmtYear(min)}–${fmtYear(max)}`;
  }

  function renderResult(res, fmt) {
    const progressive = res.progressive || null;
    renderProgressiveInfo(progressive);

    if (fmt === "map") return renderMap(res);
    if (fmt === "time") return renderTime(res);

    if (fmt === "graph") {
      let triples = triplesForGraph(res);
      if (!triples.length && res.kind === "construct" && res.format) {
        const rerun = JSON.parse(state.graph.query($("q").value, "table"));
        triples = triplesForGraph(rerun);
      }
      return renderGraph(triples);
    }

    if (res.kind === "ask") {
      $("out").innerHTML = progressiveBanner(progressive) +
        `<div class="banner">ASK result: <strong>${esc(res.boolean)}</strong></div>`;
      return `ASK ${res.boolean}`;
    }

    if (res.kind === "select") {
      $("out").innerHTML = progressiveBanner(progressive) + renderTable(res.vars || [], res.rows || []);
      return `${(res.rows || []).length} row(s)`;
    }

    if (res.format === "ttl" || res.format === "jsonld") {
      $("out").innerHTML = `<pre>${esc(res.text || "")}</pre>`;
      return `CONSTRUCT ${res.format}`;
    }

    $("out").innerHTML = renderTriplesTable(res.triples || []);
    return `${(res.triples || []).length} triple(s)`;
  }

  // A playful network spinner shown while a query runs: a hub firing packets out
  // to nodes (byte ranges in flight), edges flowing, nodes pulsing.
  function netSpinner(caption) {
    const hub = [100, 70];
    const sats = [[40, 36], [162, 38], [26, 106], [174, 100], [100, 14], [100, 126]];
    let edges = "", pkts = "";
    let nodes = `<circle class="ns-hub" cx="${hub[0]}" cy="${hub[1]}" r="7"/>`;
    sats.forEach(([x, y], i) => {
      edges += `<line class="ns-edge" x1="${hub[0]}" y1="${hub[1]}" x2="${x}" y2="${y}"/>`;
      nodes += `<circle class="ns-node" cx="${x}" cy="${y}" r="4.5" style="animation-delay:${(i * 0.17).toFixed(2)}s"/>`;
      // Packets travel inward — from the outer nodes to the hub (bytes arriving).
      pkts += `<circle class="ns-pkt" r="2.6"><animateMotion dur="${(0.7 + i * 0.13).toFixed(2)}s" ` +
        `repeatCount="indefinite" path="M${x},${y} L${hub[0]},${hub[1]}"/></circle>`;
    });
    return `<div class="netspin"><svg viewBox="0 0 200 140" role="img" aria-label="querying">` +
      edges + pkts + nodes + `</svg><div class="ns-cap">${esc(caption || "querying…")}</div></div>`;
  }

  // The "requests" inspector: shows/hides the button by the run bar, and renders a
  // modal listing the byte-range fetches a remote query made (worker fetch log).
  function updateReqLogBtn() {
    const btn = $("reqLogBtn");
    if (!btn) return;
    const n = (state.lastRemoteLog || []).length;
    btn.classList.toggle("hidden", n === 0);
    btn.textContent = `⊞ ${n} request${n === 1 ? "" : "s"}`;
  }

  function openReqLog() {
    const log = state.lastRemoteLog || [];
    const totalBytes = log.reduce((a, e) => a + (e.b || 0), 0);
    const totalRanges = log.reduce((a, e) => a + (e.k === "multi" ? (e.n || 0) : 1), 0);
    const last = log.length ? log[log.length - 1].t : 0;
    const head = `<div class="reqlog-stat">` +
      `<span><b>${log.length}</b> HTTP request(s)</span><span><b>${totalRanges}</b> byte-range(s)</span>` +
      `<span><b>${formatBytes(totalBytes)}</b> fetched</span><span><b>${last} ms</b> total</span></div>`;
    const rows = log.map((e, i) => {
      const kind = e.k === "multi" ? `multipart ×${e.n}` : "range";
      const rs = e.k === "multi" ? (e.r || []) : [];
      const ranges = rs.length ? esc(rs.slice(0, 6).join(", ") + (rs.length > 6 ? ` … (+${rs.length - 6})` : "")) : "—";
      return `<tr><td class="num">${i + 1}</td><td>${kind}</td><td class="num">${formatBytes(e.b || 0)}</td>` +
        `<td class="num">${e.t} ms</td><td class="mono">${ranges}</td></tr>`;
    }).join("");
    $("reqLogBody").innerHTML = head +
      `<div class="tbl"><table><thead><tr><th class="num">#</th><th>kind</th><th class="num">bytes</th>` +
      `<th class="num">at</th><th>byte ranges (start-end)</th></tr></thead>` +
      `<tbody>${rows || `<tr><td colspan="5">No requests logged.</td></tr>`}</tbody></table></div>`;
    $("reqModal").classList.remove("hidden");
  }

  // --- Federation: one SPARQL query across several sources ----------------
  // "Federation is a kind of SPARQL." When the user adds sources, the same query
  // runs against each one independently and the results are merged — UNION+dedup
  // for SELECT, logical OR for ASK, triple-union for CONSTRUCT — mirroring the
  // `rete federate` CLI. Each source keeps its own lazy reader: a remote .rete via
  // the range-read worker, an in-memory dataset via a resident Graph, a live
  // SPARQL endpoint via fetch. No source is downloaded wholesale.
  let fedSeq = 0;
  function fedActive() { return state.fedSources.length > 0; }

  // The current dataset is always source #0, resolved at query time to whatever
  // it actually is — a lazy remote URL or the in-memory Graph handle.
  function selfSource() {
    const name = dsShortLabel(state.dataset) + " · this dataset";
    return state.remote
      ? { id: "self", kind: "remote", label: name, url: state.remote.url, self: true }
      : { id: "self", kind: "memory", label: name, self: true };
  }
  function allFedSources() { return [selfSource()].concat(state.fedSources); }

  function detectQueryKind(q) {
    const body = q.replace(/^\s*(?:#.*\n|PREFIX\s+[^\n]*\n|BASE\s+[^\n]*\n)*/i, "");
    const m = /\b(SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i.exec(body);
    return m ? m[1].toLowerCase() : "select";
  }

  function shortUrlLabel(url) {
    try {
      const u = new URL(url);
      const seg = u.pathname.split("/").filter(Boolean).pop();
      return seg ? seg.replace(/\.rete$/, "") : u.hostname;
    } catch (e) { return url.length > 30 ? url.slice(0, 28) + "…" : url; }
  }

  // A resident wasm Graph for an in-memory federation partner (a bundled or
  // already-cached dataset), opened once and kept in state.fedGraphs.
  function fedGraphFor(src) {
    if (src.self) {
      if (!state.graph) throw new Error("load this dataset into memory first");
      return state.graph;
    }
    if (state.fedGraphs.has(src.key)) return state.fedGraphs.get(src.key);
    const bytes = remoteCache.get(src.key) ||
      (RETE_DATASETS_B64[src.key] && b64ToBytes(RETE_DATASETS_B64[src.key]));
    if (!bytes) throw new Error("source is not in memory");
    const g = new (W().Graph)(bytes);
    state.fedGraphs.set(src.key, g);
    return g;
  }

  // SPARQL-protocol JSON binding → the engine's term-string form.
  function endpointTerm(b) {
    if (!b) return undefined;
    if (b.type === "uri") return "<" + b.value + ">";
    if (b.type === "bnode") return "_:" + b.value;
    let s = '"' + String(b.value).replace(/\\/g, "\\\\").replace(/"/g, '\\"') + '"';
    if (b["xml:lang"]) return s + "@" + b["xml:lang"];
    if (b.datatype) return s + "^^<" + b.datatype + ">";
    return s;
  }
  async function queryEndpoint(src, q, kind) {
    const sep = src.endpoint.includes("?") ? "&" : "?";
    const res = await fetch(src.endpoint + sep + "query=" + encodeURIComponent(q),
      { headers: { Accept: "application/sparql-results+json" } });
    if (!res.ok) throw new Error(res.status + " " + res.statusText);
    const j = await res.json();
    if (kind === "ask") return { kind: "ask", boolean: !!j.boolean, vars: [], rows: [], triples: [] };
    const vars = (j.head && j.head.vars) || [];
    const rows = ((j.results && j.results.bindings) || [])
      .map((bnd) => { const o = {}; vars.forEach((v) => { const t = endpointTerm(bnd[v]); if (t !== undefined) o[v] = t; }); return o; });
    return { kind: "select", vars, rows, triples: [] };
  }

  // Run the query against one source, normalized to a common result shape.
  function querySource(src, q, kind) {
    if (src.kind === "endpoint") {
      return queryEndpoint(src, q, kind).then((r) => Object.assign(r, { bytes: 0, requests: 0 }));
    }
    if (src.kind === "remote") {
      return remoteSparql(src.url, q, "table").then((out) => {
        const r = JSON.parse(out.json), rem = r.remote || {};
        return { kind: r.kind || kind, vars: r.vars || [], rows: r.rows || [],
          boolean: r.boolean, triples: r.triples || [], bytes: rem.bytes || 0, requests: rem.requests || 0 };
      });
    }
    return new Promise((resolve) => {
      const r = JSON.parse(fedGraphFor(src).query(q, "table"));
      resolve({ kind: r.kind || kind, vars: r.vars || [], rows: r.rows || [],
        boolean: r.boolean, triples: r.triples || [], bytes: 0, requests: 0 });
    });
  }

  function fedBanner(settled, merged, kind) {
    const one = (r) => kind === "ask" ? (r.boolean ? "true" : "false")
      : (kind === "construct" || kind === "describe") ? `${(r.triples || []).length} triple(s)`
      : `${(r.rows || []).length} row(s)`;
    const lines = settled.map((s) => {
      const name = esc(s.src.label);
      if (!s.ok) return `<tr><td>${name}</td><td class="fed-err" colspan="2">error — ${esc(s.error)}</td></tr>`;
      const cost = s.src.kind === "remote" ? `${formatBytes(s.r.bytes || 0)} · ${s.r.requests || 0} req`
        : s.src.kind === "endpoint" ? "live endpoint" : "in-memory";
      return `<tr><td>${name}</td><td class="num">${one(s.r)}</td><td class="num">${cost}</td></tr>`;
    }).join("");
    const total = kind === "ask" ? `ASK = ${merged.boolean}`
      : (kind === "construct" || kind === "describe") ? `${merged.triples.length} merged triple(s)`
      : `${merged.rows.length} merged row(s)`;
    return `<div class="fed-banner"><div class="fed-banner-head">Federated across ${settled.length} source(s) — ${esc(total)}</div>` +
      `<table><tbody>${lines}</tbody></table></div>`;
  }

  function runFederated(q, fmt) {
    const kind = detectQueryKind(q);
    const sources = allFedSources();
    $("commOut").innerHTML = "";
    $("reqLogBtn").classList.add("hidden");
    $("out").innerHTML = netSpinner(`federating across ${sources.length} sources…`);
    updateResultVisibility();
    const t0 = performance.now();
    const jobs = sources.map((src) =>
      Promise.resolve().then(() => querySource(src, q, kind))
        .then((r) => ({ src, r, ok: true }))
        .catch((e) => ({ src, ok: false, error: String((e && e.message) || e) })));
    Promise.all(jobs).then((settled) => {
      const dt = performance.now() - t0;
      const oks = settled.filter((s) => s.ok);
      let merged;
      if (kind === "ask") {
        merged = { kind: "ask", boolean: oks.some((s) => s.r.boolean === true || s.r.boolean === "true") };
      } else if (kind === "construct" || kind === "describe") {
        const seen = new Set(), triples = [];
        oks.forEach((s) => (s.r.triples || []).forEach((t) => {
          const k = JSON.stringify(t); if (!seen.has(k)) { seen.add(k); triples.push(t); }
        }));
        merged = { kind: "construct", triples };
      } else {
        const vars = [];
        oks.forEach((s) => (s.r.vars || []).forEach((v) => { if (!vars.includes(v)) vars.push(v); }));
        const seen = new Set(), rows = [];
        oks.forEach((s) => (s.r.rows || []).forEach((row) => {
          const k = vars.map((v) => row[v] == null ? "" : row[v]).join("");
          if (!seen.has(k)) { seen.add(k); rows.push(row); }
        }));
        merged = { kind: "select", vars, rows };
      }
      const renderFmt = (fmt === "graph" && kind !== "construct" && kind !== "describe") ? "table" : fmt;
      const summary = renderResult(merged, renderFmt);
      const banner = fedBanner(settled, merged, kind);
      $("out").innerHTML = banner + $("out").innerHTML;
      const totalBytes = oks.reduce((a, s) => a + (s.r.bytes || 0), 0);
      state.lastResult = { res: merged, rowShaped: true, q, strategy: "federated",
        remote: false, dataset: state.dataset, federated: true, fedBannerHtml: banner };
      $("qmeta").textContent = `${summary} · federated ${sources.length} source(s) · ${formatBytes(totalBytes)} ranged · ${dt.toFixed(0)} ms`;
      saveHistory({ query: q, format: fmt, strategy: "federated",
        dataset: "(federated ×" + sources.length + ")", ts: Date.now(), resultSummary: summary });
    });
  }

  // --- Federation source picker (the "+ Add source" popover) --------------
  function renderFedBar() {
    const chips = $("fedChips");
    if (!chips) return;
    const self = `<span class="fed-chip fed-self" title="The dataset selected above"><span class="fed-chip-name">${esc(dsShortLabel(state.dataset))}</span>` +
      `<span class="fed-chip-kind">${state.remote ? "lazy" : "in-memory"}</span></span>`;
    const extra = state.fedSources.map((s) =>
      `<span class="fed-chip"><span class="fed-chip-name" title="${esc(s.label)}">${esc(s.label)}</span>` +
      `<span class="fed-chip-kind">${s.kind === "remote" ? "lazy" : s.kind === "endpoint" ? "endpoint" : "in-memory"}</span>` +
      `<button type="button" class="fed-x" data-fedremove="${s.id}" title="Remove this source" aria-label="Remove ${esc(s.label)}">×</button></span>`).join("");
    chips.innerHTML = self + extra;
    const run = $("run");
    if (run && run.textContent !== "Cancel") run.textContent = fedActive() ? "Run federated" : "Run Query";
  }
  function resetFed() {
    state.fedSources = [];
    state.fedGraphs.forEach((g) => { try { g.free(); } catch (e) { /* already freed */ } });
    state.fedGraphs.clear();
    renderFedBar();
  }
  function populateFedCatalog() {
    const sel = $("fedCatalog");
    if (!sel) return;
    sel.innerHTML = CATALOG.datasets
      .filter((d) => d.key !== state.dataset)
      .map((d) => `<option value="${esc(d.key)}">${esc(dsShortLabel(d.key))}${d.kind === "remote-lazy" ? " — remote" : ""}</option>`)
      .join("");
  }
  function openFedPop() { populateFedCatalog(); $("fedPop").classList.remove("hidden"); }
  function closeFedPop() { const p = $("fedPop"); if (p) p.classList.add("hidden"); }
  function setFedMode(mode) {
    $$("#fedModes button").forEach((b) => b.classList.toggle("active", b.dataset.fedmode === mode));
    $$(".fed-pop-body [data-fedbody]").forEach((b) => b.classList.toggle("hidden", b.dataset.fedbody !== mode));
  }
  function confirmAddFed() {
    const active = document.querySelector("#fedModes button.active");
    const mode = active ? active.dataset.fedmode : "catalog";
    let src = null;
    if (mode === "catalog") {
      const key = $("fedCatalog").value;
      if (!key) return;
      const canMemory = isEmbedded(key) || remoteCache.has(key);
      src = ($("fedCatalogLazy").checked || !canMemory)
        ? { id: "f" + (++fedSeq), kind: "remote", label: dsShortLabel(key), url: remoteUrlFor(key), key }
        : { id: "f" + (++fedSeq), kind: "memory", label: dsShortLabel(key), key };
    } else if (mode === "link") {
      const url = $("fedLinkUrl").value.trim();
      if (!url) return;
      src = { id: "f" + (++fedSeq), kind: "remote", label: shortUrlLabel(url), url };
    } else {
      const ep = $("fedEndpoint").value.trim();
      if (!ep) return;
      src = { id: "f" + (++fedSeq), kind: "endpoint", label: shortUrlLabel(ep), endpoint: ep };
    }
    const dup = state.fedSources.some((s) =>
      (src.url && s.url === src.url) || (src.endpoint && s.endpoint === src.endpoint) ||
      (src.key && s.key === src.key && s.kind === src.kind));
    if (!dup) { state.fedSources.push(src); renderFedBar(); }
    closeFedPop();
  }
  function removeFedSource(id) {
    state.fedSources = state.fedSources.filter((s) => s.id !== id);
    renderFedBar();
  }

  function runQuery() {
    const q = $("q").value.trim();
    if (!q) return showError("out", "Enter a SPARQL query.");
    const fmt = $("fmt").value;
    if (fedActive()) return runFederated(q, fmt);
    // Clear the previous message and show the network spinner. We deliberately
    // KEEP state.lastResult: the reuse guard already requires the query text,
    // dataset, strategy and remote-ness to match before re-rendering, so a stale
    // cache is never reused — and keeping it lets a switch to a serialization
    // view (TTL/JSON-LD) and back to a row view re-render with no re-run.
    $("commOut").innerHTML = "";
    $("reqLogBtn").classList.add("hidden");
    $("out").innerHTML = netSpinner(state.remote ? "querying remote…" : "querying…");
    updateResultVisibility();

    // Remote lazy mode: route through the worker (range reads), render async with
    // LIVE progress — a 1 GB graph can take many range fetches, so show running
    // request count, bytes fetched (of the file size) and elapsed, plus a Cancel.
    if (state.remote) {
      const t0 = performance.now();
      const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[state.dataset]) || {};
      const ofSize = meta.size ? " of " + meta.size : "";
      const dsName = dsShortLabel(state.dataset);
      let lastReq = 0, lastBytes = 0;
      const showProg = () => {
        const dt = (performance.now() - t0) / 1000;
        $("qmeta").textContent = `⏳ querying ${dsName} — ${lastReq} request(s) · ` +
          `${formatBytes(lastBytes)}${ofSize} fetched · ${dt.toFixed(1)}s`;
      };
      const runBtn = $("run");
      const prevLabel = runBtn.textContent;
      runBtn.textContent = "Cancel";
      runBtn.onclick = cancelRemote;
      showProg();
      const timer = setInterval(showProg, 250);
      const cleanup = () => {
        clearInterval(timer);
        remoteOnProgress = null;
        runBtn.textContent = prevLabel;
        runBtn.onclick = runQuery;
      };
      // Just record the latest tally; the 250 ms timer paints it — so a query
      // firing thousands of fetches doesn't thrash the DOM.
      remoteOnProgress = (m) => { lastReq = m.requests; lastBytes = m.bytes; };
      remoteSparql(state.remote.url, q, "table").then((out) => {
        cleanup();
        state.lastRemoteLog = out.log || [];
        const res = JSON.parse(out.json);
        // Remote always asks the worker for table rows, so the result is always
        // row-shaped — cache it so an Output-type switch re-renders, not re-runs.
        state.lastResult = { res, rowShaped: true, q, strategy: "remote", remote: true, dataset: state.dataset };
        const summary = renderResult(res, fmt === "graph" ? "table" : fmt);
        const r = res.remote || {};
        const dt = performance.now() - t0;
        updateReqLogBtn();
        // Show this query's PHYSICAL fetch (cache misses only) plus what the
        // resident session has cached so far — so a re-run visibly drops to ~0.
        const cacheNote = r.cached
          ? " — served from cache, 0 new bytes"
          : (r.sessionBytes != null ? ` · ${formatBytes(r.sessionBytes)} cached this session` : "");
        $("qmeta").textContent = `${summary} | ${r.requests || 0} range req · ` +
          `${formatBytes(r.bytes || 0)} of ${formatBytes(r.fileLength || 0)} fetched${cacheNote} · ${dt.toFixed(0)} ms`;
        saveHistory({ query: q, format: fmt, strategy: "remote", dataset: "(remote)", ts: Date.now(), resultSummary: summary });
      }).catch((e) => {
        cleanup();
        if (e && e.log) state.lastRemoteLog = e.log;
        updateReqLogBtn();
        const msg = String(e.message || e);
        if (msg === "cancelled") {
          $("qmeta").textContent = "cancelled";
          $("out").innerHTML = `<div class="note">Query cancelled — the worker was stopped. Run again to retry.</div>`;
        } else {
          $("qmeta").textContent = "";
          showError("out", "Remote query failed: " + msg);
        }
      });
      return;
    }

    if (!state.bytes) return showError("out", "Load a graph first.");
    // Defer the (synchronous) engine call one frame so the spinner paints first.
    setTimeout(() => runEmbeddedQuery(q, fmt), 0);
  }

  function runEmbeddedQuery(q, fmt) {
    const strategy = $("strategy").value;
    // graph / map / time are renderings of SELECT bindings — ask the engine for table rows.
    const rowView = fmt === "graph" || fmt === "map" || fmt === "time";
    const queryFmt = strategy === "progressive" || rowView ? "table" : fmt;
    const t0 = performance.now();
    try {
      let raw;
      let fellBack = false;
      if (strategy === "progressive") {
        // Progressive is a contract, not a speedup: answer exactly from the
        // pyramid summary or don't. Shapes that need index/dictionary bytes
        // (any query returning values) fall back to the whole index — run,
        // and *say so* rather than refusing.
        try {
          // The progressive (summary-only) reader stays a free function — it
          // reads small ranges from the buffer, not the resident index.
          raw = W().progressive_query(state.bytes, q);
        } catch (pe) {
          const m = String(pe);
          if (m.includes("not exactly answerable") || m.includes("no pyramid summary")) {
            raw = state.graph.query(q, queryFmt);
            fellBack = true;
          } else {
            throw pe;
          }
        }
      } else if (strategy === "community") {
        const roundText = $("round").value.trim();
        raw = state.graph.query_communities(q, roundText === "" ? undefined : Number(roundText));
      } else {
        raw = state.graph.query(q, queryFmt);
      }
      const res = JSON.parse(raw);
      const summary = renderResult(res, strategy !== "whole" && fmt === "graph" ? "table" : fmt);
      // Cache row-shaped results (the engine returned table rows) so switching
      // the Output type re-renders this result instead of re-running the query.
      // Progressive is excluded — its summary answers re-run cheaply.
      if (queryFmt === "table" && strategy !== "progressive") {
        state.lastResult = { res, rowShaped: true, q, strategy, remote: false, dataset: state.dataset };
      }
      const dt = performance.now() - t0;
      $("qmeta").textContent = `${summary} | ${dt.toFixed(1)} ms${fellBack ? " | fell back to whole index" : ""}`;
      if (fellBack) {
        $("out").innerHTML =
          `<div class="note">Not summary-answerable: this query returns values (titles, scores, …), ` +
          `which live in the dictionary and triple index — the pyramid summary holds only community ` +
          `structure and per-predicate counts, so the progressive contract (answer from the summary ` +
          `alone, never touch the index) cannot apply. <strong>Ran the whole index instead.</strong> ` +
          `Progressive shines on shapes like the “Predicate totals” example.</div>` +
          $("out").innerHTML;
        $("progressiveInfo").innerHTML =
          `<div>Fell back to the whole index — this query needs index bytes the summary does not hold.</div>`;
      }
      if (strategy === "community") renderCommunityPartials(res.communities);
      saveHistory({ query: q, format: fmt, strategy, dataset: state.dataset, ts: Date.now(), resultSummary: summary });
      updateHash();
    } catch (e) {
      $("qmeta").textContent = "";
      let msg = String(e);
      if (strategy === "progressive") {
        msg += " — Progressive answers COUNT/ASK shapes straight from the pyramid summary.";
      }
      showError("out", msg);
      renderProgressiveInfo(null);
    }
  }

  function renderCommunityPartials(parts) {
    if (!parts || !parts.length) return;
    const total = parts.reduce((a, p) => a + p.rows, 0);
    const contributing = parts.filter((p) => p.rows > 0);
    $("commOut").innerHTML =
      `<div class="banner">Subject stars computed per pyramid community, recombined with global ` +
      `joins, modifiers applied once: ${contributing.length} of ${parts.length} communities ` +
      `contributed ${total} partial row(s) — the merged result is identical to the whole-index answer.</div>` +
      collapsedTable(
        `<tr><th>community</th><th>subjects</th><th>partial rows</th></tr>`,
        contributing.map((p) =>
          `<tr><td>C${p.community}</td><td>${p.subjects}</td><td>${p.rows}</td></tr>`)
      );
    updateResultVisibility();
  }

  function renderShaclExamples() {
    const list = CATALOG.shacl[state.dataset] || [];
    if (!list.length) {
      $("shaclExamples").innerHTML = `<p class="microcopy">No SHACL examples for this dataset.</p>`;
      setEd("shapeText", "");
      return;
    }
    $("shaclExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-shacl="${i}">${esc(ex.label)}</button><div class="tagline">${esc(ex.tip)}</div></article>`
    ).join("");
    $$("#shaclExamples [data-shacl]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.shacl)];
        setEd("shapeText", ex.shape);
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.tip)}</div>`;
        setMode("shacl");
      };
    });
    setEd("shapeText", list[0].shape);
  }

  function renderShaclJson(report, raw) {
    if (report.conforms) {
      return `<div class="banner">Conforms. No validation results.</div><pre>${esc(raw)}</pre>`;
    }
    const rows = (report.results || []).slice(0, 250).map((r) =>
      `<tr><td class="iri">${esc(shorten(r.focusNode || ""))}</td><td class="iri">${esc(shorten(r.resultPath || ""))}</td><td>${esc(shorten(r.sourceConstraintComponent || ""))}</td><td>${esc(shorten((r.messages || []).join(" "), 120))}</td></tr>`
    ).join("");
    return `<div class="note">Does not conform: ${(report.results || []).length} validation result(s).</div>` +
      `<table><thead><tr><th>focus</th><th>path</th><th>component</th><th>message</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function runShacl() {
    if (!state.bytes) return showError("shaclOut", "Load a graph first.");
    const shapes = $("shapeText").value.trim();
    if (!shapes) return showError("shaclOut", "Enter a SHACL shape.");
    const fmt = $("shaclFormat").value;
    const t0 = performance.now();
    try {
      const text = state.graph.shacl(shapes, null, fmt);
      const dt = performance.now() - t0;
      if (fmt === "json") {
        const report = JSON.parse(text);
        $("shaclOut").innerHTML = renderShaclJson(report, text);
        $("shaclMeta").textContent = `${report.conforms ? "conforms" : "violations"} | ${dt.toFixed(1)} ms`;
      } else {
        $("shaclOut").innerHTML = `<pre>${esc(text)}</pre>`;
        $("shaclMeta").textContent = `${text.startsWith("conforms: true") ? "conforms" : "report"} | ${dt.toFixed(1)} ms`;
      }
      updateResultVisibility();
    } catch (e) {
      $("shaclMeta").textContent = "";
      showError("shaclOut", String(e));
    }
  }

  function runCoherence() {
    const remote = state.remote && state.remote.url;
    if (!state.bytes && !remote) return showError("coherenceOut", "Load a graph first.");
    const t0 = performance.now();
    const block = (title, sub, coherent, points) => {
      const items = (points || []).map((p) =>
        `<li><code>${esc(p.kind)}</code> — ${esc(p.detail)}</li>`).join("");
      const verdict = coherent ? "coherent ✓" : `${(points || []).length} incoherent point(s)`;
      return `<section class="coherence-block"><h3>${esc(title)}</h3>` +
        `<p class="microcopy">${esc(sub)}</p>` +
        `<p><strong>${verdict}</strong></p>` +
        (items ? `<ul>${items}</ul>` : "") + `</section>`;
    };
    if (remote) {
      // Lazy: Tier-0 schema coherence read from the schema pyramid (the card)
      // over HTTP range. check_schema_url does synchronous range XHR → worker.
      remoteRead("check_schema_url", remote, $("coherenceOut"),
        "Checking schema coherence over HTTP range…",
        "Tier-0: subClassOf cycles + unsatisfiable classes, read in ~2–3 range requests — no index or dictionary bytes.").then((out) => {
        const schema = JSON.parse(out.json);
        const dt = performance.now() - t0;
        const r = schema.remote || {};
        $("coherenceOut").innerHTML =
          block("Schema (Tier-0, index-free)",
            "subClassOf cycles + unsatisfiable classes, read from the schema pyramid over HTTP range — no download, and no index/dictionary bytes.",
            schema.coherent, schema.schemaPoints) +
          `<section class="coherence-block"><p class="microcopy">The full instance-level reasoner materializes the whole graph, so it needs the dataset cached in memory (use <strong>Cache remote</strong>). The Tier-0 verdict above came from the card alone.</p></section>`;
        $("coherenceMeta").textContent =
          `${schema.coherent ? "schema coherent" : "schema incoherent"} | ` +
          `${formatBytes(r.bytes || 0)} of ${formatBytes(r.fileLength || 0)} · ${r.requests || 0} req · ${dt.toFixed(0)} ms`;
        updateResultVisibility();
      }).catch((e) => {
        const msg = String(e && e.message || e);
        if (/no schema pyramid/i.test(msg)) {
          $("coherenceOut").innerHTML = `<div class="note">This graph carries no schema pyramid (no typed classes), so there's nothing to check at the schema tier. Use <strong>Cache remote</strong> to run the full instance reasoner.</div>`;
          $("coherenceMeta").textContent = "";
        } else {
          showError("coherenceOut", "Remote coherence failed: " + msg);
        }
        updateResultVisibility();
      });
      return;
    }
    try {
      const schema = JSON.parse(W().check_schema(state.bytes));
      const full = JSON.parse(state.graph.reason(null));
      const dt = performance.now() - t0;
      $("coherenceOut").innerHTML =
        block("Schema (Tier-0, index-free)", "subClassOf cycles + unsatisfiable classes, from the schema pyramid", schema.coherent, schema.schemaPoints) +
        block("Full reasoner (instance-level)", `${full.inferredCount} triple(s) entailed; disjoint-class / sameAs / functional clashes`, full.coherent, full.inconsistencies);
      const ok = schema.coherent && full.coherent;
      $("coherenceMeta").textContent = `${ok ? "coherent" : "incoherent"} | ${dt.toFixed(1)} ms`;
      updateResultVisibility();
    } catch (e) {
      $("coherenceMeta").textContent = "";
      showError("coherenceOut", String(e));
    }
  }

  function renderReachDefaults() {
    const cfg = CATALOG.reach[state.dataset] || {};
    $("reachPred").value = cfg.pred || "";
    $("reachSeeds").value = cfg.seeds || "";
    $("reachReverse").checked = false;
    const list = cfg.examples || [];
    $("reachExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-reach="${i}">${esc(ex.label)}</button><div class="tagline">${esc(ex.pred)} | ${ex.reverse ? "reverse" : "forward"}</div></article>`
    ).join("");
    $$("#reachExamples [data-reach]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.reach)];
        $("reachPred").value = ex.pred;
        $("reachSeeds").value = ex.seeds;
        $("reachReverse").checked = !!ex.reverse;
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.pred)}</div>`;
        setMode("reach");
      };
    });
  }

  function runReach() {
    if (!state.bytes) return showError("reachOut", "Load a graph first.");
    const pred = $("reachPred").value.trim();
    const seeds = $("reachSeeds").value.split(",").map((s) => s.trim()).filter(Boolean);
    if (!pred || !seeds.length) return showError("reachOut", "Enter a predicate and at least one seed.");
    const reverse = $("reachReverse").checked;
    const t0 = performance.now();
    try {
      const results = JSON.parse(state.graph.reach(pred, JSON.stringify(seeds), reverse));
      const dt = performance.now() - t0;
      const rows = results.map((r) => {
        if (r.error) return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td colspan="2">${esc(r.error)}</td></tr>`;
        const shown = (r.reached || []).slice(0, 250).map((x) => `<div class="iri">${esc(shorten(x, 90))}</div>`).join("");
        const more = r.count > 250 ? `<div class="microcopy">Showing first 250 of ${r.count}.</div>` : "";
        return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td>${r.count}</td><td>${shown}${more}</td></tr>`;
      }).join("");
      $("reachMeta").textContent = `${results.length} seed(s) | ${reverse ? "reverse" : "forward"} | ${dt.toFixed(1)} ms`;
      $("reachOut").innerHTML = `<table><thead><tr><th>seed</th><th>count</th><th>reached</th></tr></thead><tbody>${rows}</tbody></table>`;
      updateResultVisibility();
    } catch (e) {
      $("reachMeta").textContent = "";
      showError("reachOut", String(e));
    }
  }

  function renderSchema(schema) {
    const classes = schema.classes || [];
    const relations = schema.relations || [];
    $("schemaSummary").innerHTML =
      `<div class="metric-grid">${metric("classes", classes.length)}${metric("relations", relations.length)}</div>` +
      `<div>${classes.slice(0, 5).map((c) => `<span class="chip">${esc(shorten(c[0], 38))} (${esc(c[1])})</span>`).join(" ")}</div>`;
    $("classes").innerHTML = `<div class="chip-list">` + classes.slice(0, 80)
      .map((c) => `<span class="chip">${esc(shorten(c[0], 50))} <strong>${esc(c[1])}</strong></span>`)
      .join("") + `</div>`;
    $("relations").innerHTML = renderTable(["subjectClass", "predicate", "objectClass", "count"],
      relations.slice(0, 120).map((r) => ({
        subjectClass: r[0],
        predicate: r[1],
        objectClass: r[2],
        count: String(r[3])
      })));
    $("ontologyDiagram").innerHTML = renderOntologyDiagram(classes, relations);
    $("schemaOut").innerHTML = `<div class="banner">${classes.length} classes and ${relations.length} class-level relations.</div>`;
  }

  function localName(term) {
    const m = String(term).match(/[\/#]([^\/#>]+)>?$/);
    return m ? m[1] : String(term).replace(/[<>]/g, "");
  }

  // UML-style schema: each class is a box whose rows are its datatype
  // properties (relations whose object class is "(literal)"); object
  // properties between shown classes are drawn as labelled edges.
  function renderOntologyDiagram(classes, relations) {
    if (!classes.length) return `<div class="note">No rdf:type-derived classes found.</div>`;
    const top = classes.slice(0, 8);
    const idx = new Map(top.map((c, i) => [c[0], i]));

    // Per-class datatype properties (top 5 by count) + object edges.
    const attrs = top.map(() => []);
    const edgeMap = new Map(); // "s>t" -> {s, t, preds: Map(pred -> count)}
    relations.forEach((r) => {
      const [sc, p, oc, n] = r;
      if (!idx.has(sc)) return;
      if (oc === "(literal)") {
        attrs[idx.get(sc)].push([p, Number(n)]);
      } else if (idx.has(oc)) {
        const s = idx.get(sc), t = idx.get(oc);
        const key = s + ">" + t;
        if (!edgeMap.has(key)) edgeMap.set(key, { s, t, preds: new Map() });
        const e = edgeMap.get(key);
        e.preds.set(p, (e.preds.get(p) || 0) + Number(n));
      }
    });
    attrs.forEach((list) => list.sort((a, b) => b[1] - a[1]).splice(5));

    // Grid layout: up to 4 columns; row height grows with the tallest box.
    const cols = Math.min(4, top.length);
    const boxW = 196;
    const gapX = 36;
    const gapY = 46;
    const headH = 24;
    const rowH = 13;
    const width = 24 + cols * boxW + (cols - 1) * gapX + 24;
    const boxH = (i) => headH + 7 + attrs[i].length * rowH + (attrs[i].length ? 5 : 0);
    const boxes = top.map((c, i) => {
      const col = i % cols;
      const row = Math.floor(i / cols);
      return { iri: c[0], count: c[1], i, col, row, w: boxW, h: boxH(i) };
    });
    const rowHeights = [];
    boxes.forEach((b) => {
      rowHeights[b.row] = Math.max(rowHeights[b.row] || 0, b.h);
    });
    let y = 18;
    const rowY = rowHeights.map((h) => {
      const at = y;
      y += h + gapY;
      return at;
    });
    boxes.forEach((b) => {
      b.x = 24 + b.col * (boxW + gapX);
      b.y = rowY[b.row];
    });
    const height = y - gapY + 18;

    const anchor = (b) => ({ x: b.x + b.w / 2, y: b.y + b.h / 2 });
    let svg = `<svg viewBox="0 0 ${width} ${Math.max(height, 160)}" role="img" aria-label="Schema diagram">`;
    svg += `<defs><marker id="sarrow" markerWidth="7" markerHeight="7" refX="6" refY="3.5" orient="auto"><path d="M0,0 L7,3.5 L0,7 z" fill="#9fb5ac"></path></marker></defs>`;

    // Edges beneath the boxes. Self-references (e.g. Person coauthor Person)
    // are drawn as a small loop on top of the box.
    const edges = Array.from(edgeMap.values()).slice(0, 18);
    edges.forEach((e) => {
      const label = Array.from(e.preds.entries())
        .sort((a, b) => b[1] - a[1])
        .slice(0, 2)
        .map(([p]) => localName(p))
        .join(", ");
      if (e.s === e.t) {
        const b = boxes[e.s];
        const cx = b.x + b.w - 26;
        const cy = b.y;
        svg += `<path class="cls-edge" d="M ${cx - 12} ${cy} C ${cx - 12} ${cy - 26}, ${cx + 12} ${cy - 26}, ${cx + 12} ${cy}" marker-end="url(#sarrow)"></path>`;
        svg += `<text class="cls-edge-label" x="${cx}" y="${cy - 28}" text-anchor="middle">${esc(shorten(label, 22))}</text>`;
        return;
      }
      const a = anchor(boxes[e.s]);
      const b = anchor(boxes[e.t]);
      svg += `<line class="cls-edge" x1="${a.x.toFixed(1)}" y1="${a.y.toFixed(1)}" x2="${b.x.toFixed(1)}" y2="${b.y.toFixed(1)}" marker-end="url(#sarrow)"></line>`;
      svg += `<text class="cls-edge-label" x="${((a.x + b.x) / 2).toFixed(1)}" y="${((a.y + b.y) / 2 - 4).toFixed(1)}" text-anchor="middle">${esc(shorten(label, 26))}</text>`;
    });

    boxes.forEach((b) => {
      svg += `<g><title>${esc(b.iri)} (${esc(b.count)} instances)</title>`;
      svg += `<rect class="cls-box" x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="6"></rect>`;
      svg += `<rect class="cls-head" x="${b.x}" y="${b.y}" width="${b.w}" height="${headH}" rx="6"></rect>`;
      svg += `<rect class="cls-head" x="${b.x}" y="${b.y + headH - 6}" width="${b.w}" height="6"></rect>`;
      svg += `<text class="cls-title" x="${b.x + 9}" y="${b.y + 16}">${esc(shorten(localName(b.iri), 18))}</text>`;
      svg += `<text class="cls-count" x="${b.x + b.w - 9}" y="${b.y + 16}" text-anchor="end">${esc(b.count)}</text>`;
      attrs[b.i].forEach(([p, n], j) => {
        const ay = b.y + headH + 14 + j * rowH;
        svg += `<text class="cls-attr" x="${b.x + 9}" y="${ay}">${esc(shorten(localName(p), 20))}</text>`;
        svg += `<text class="cls-attr-count" x="${b.x + b.w - 9}" y="${ay}" text-anchor="end">${esc(n)}</text>`;
      });
      svg += `</g>`;
    });
    svg += `</svg>`;
    return svg;
  }

  function renderProvenanceDefaults() {
    const cfg = CATALOG.provenance[state.dataset] || {};
    $("whySubject").value = cfg.subject || "";
    $("whyPredicate").value = cfg.predicate || "";
    $("whyObject").value = cfg.object || "";
    const list = cfg.examples || [];
    $("provExamples").innerHTML = list.map((ex, i) =>
      `<article class="example-card"><button type="button" class="example-button" data-prov="${i}">${esc(ex.label)}</button>` +
      `<div class="tagline">${esc(ex.tip)}</div></article>`).join("");
    $$("#provExamples [data-prov]").forEach((btn) => {
      btn.onclick = () => {
        const ex = list[Number(btn.dataset.prov)];
        $("whySubject").value = ex.subject || "";
        $("whyPredicate").value = ex.predicate || "";
        $("whyObject").value = ex.object || "";
        $("exampleInfo").innerHTML = `<strong>${esc(ex.label)}</strong><div>${esc(ex.tip)}</div>`;
        setMode("provenance");
        runProvenance();
      };
    });
  }

  function optText(id) {
    const v = $(id).value.trim();
    return v ? v : undefined;
  }

  function runProvenance() {
    if (!state.bytes) return showError("provOut", "Load a graph first.");
    const subject = optText("whySubject");
    const predicate = optText("whyPredicate");
    const object = optText("whyObject");
    const t0 = performance.now();
    try {
      const out = JSON.parse(state.graph.why_triples(subject, predicate, object));
      const dt = performance.now() - t0;
      renderProvenance(out);
      $("whyMeta").textContent = `${out.resultCount} match(es) | ${dt.toFixed(1)} ms`;
      // Refresh the Explore byte map so the touched ranges light up there.
      if (state.exploreReady) renderLayout();
      updateResultVisibility();
    } catch (e) {
      $("whyMeta").textContent = "";
      showError("provOut", String(e));
    }
  }

  function renderRange(range) {
    if (!range) return "absent";
    return `${formatBytes(range.len)} @ ${range.offset}..${range.end}`;
  }

  function renderProvenance(out) {
    state.lastProvenance = out;
    renderProvenanceSummary(out);
    const rows = (out.results || []).slice(0, 250).map((r) => {
      const p = r.provenance || {};
      return `<tr>` +
        `<td class="iri">${esc(shorten(r.terms.subject, 80))}</td>` +
        `<td class="iri">${esc(shorten(r.terms.predicate, 70))}</td>` +
        `<td class="iri">${esc(shorten(r.terms.object, 80))}</td>` +
        `<td>${esc(p.indexPermutation)} / ${esc(p.indexSection)}` +
        `<span class="cell-note">payload ${esc(renderRange(p.indexSectionRange))}</span></td>` +
        `<td>${esc(renderRange(p.dictionaryRange))}</td>` +
        `<td>${esc(renderRange(p.indexRange))}</td>` +
        `<td>${esc(p.tile && p.tile.available ? p.tile.id : "not_materialized")}</td>` +
      `</tr>`;
    }).join("");
    $("provOut").innerHTML =
      `<div class="banner">${out.resultCount} result(s) matched by the selected triple pattern.</div>` +
      `<table><thead><tr><th>subject</th><th>predicate</th><th>object</th><th>index section</th><th>dictionary range</th><th>index container</th><th>tile</th></tr></thead><tbody>${rows}</tbody></table>`;
  }

  function renderProvenanceSummary(out) {
    if (!out) {
      $("provSummary").innerHTML = `<div>Run Provenance mode to see index permutation and byte ranges.</div>`;
      return;
    }
    const first = (out.results || [])[0];
    if (!first) {
      $("provSummary").innerHTML = `<div>No matches for the current pattern.</div>`;
      return;
    }
    const p = first.provenance;
    $("provSummary").innerHTML =
      `<div class="metric-grid">` +
      metric("matches", out.resultCount) +
      metric("index", p.indexPermutation) +
      metric("section", p.indexSection) +
      metric("tile", p.tile.available ? "available" : p.tile.reason) +
      `</div>` +
      `<div>Dictionary: ${esc(renderRange(p.dictionaryRange))}</div>` +
      `<div>Index container: ${esc(renderRange(p.indexRange))}</div>` +
      `<div>Selected payload: ${esc(renderRange(p.indexSectionRange))}</div>` +
      `<div>Pyramid: ${esc(renderRange(p.pyramidRange))}</div>`;
  }

  function buildFileName() {
    const base = (state.built && state.built.name) || "graph";
    return base.replace(/\.(nt|nq|nquads|ttl|turtle|txt)$/i, "") + ".rete";
  }

  function runBuild() {
    const text = $("buildText").value;
    if (!text.trim()) return showError("buildOut", "Paste some RDF first (or open a file).");
    const fmt = $("buildFormat").value;
    const t0 = performance.now();
    try {
      const bytes = W().build(text, fmt);
      const dt = performance.now() - t0;
      state.built = { bytes, name: (state.built && state.built.name) || "graph" };
      const info = JSON.parse(W().info(bytes));
      $("buildDownload").disabled = false;
      $("buildOpen").disabled = false;
      $("buildMeta").textContent = `${formatBytes(bytes.length)} | ${dt.toFixed(1)} ms`;
      $("buildOut").innerHTML =
        `<div class="banner">Built <strong>${esc(buildFileName())}</strong> — a complete, queryable .rete file.</div>` +
        `<div class="metric-grid">` +
        metric("Quads", info.quads) +
        metric("Terms", info.terms) +
        metric("Pyramid levels", info.pyramidLevels) +
        metric("Named graphs", info.namedGraphs) +
        metric("Size", formatBytes(bytes.length)) +
        `</div>` +
        `<p class="microcopy">Download it, or open it in this console to query it immediately. ` +
        `In-browser builds write uncompressed sections (the wasm engine ships no zstd encoder); ` +
        `<code>rete build</code> produces a smaller file from the same input.</p>`;
      updateResultVisibility();
    } catch (e) {
      state.built = null;
      $("buildDownload").disabled = true;
      $("buildOpen").disabled = true;
      $("buildMeta").textContent = "";
      showError("buildOut", "Build failed: " + String(e));
    }
  }

  function downloadBuilt() {
    if (!state.built) return;
    const blob = new Blob([state.built.bytes], { type: "application/octet-stream" });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = buildFileName();
    document.body.appendChild(a);
    a.click();
    a.remove();
    URL.revokeObjectURL(url);
  }

  function openBuilt() {
    if (!state.built) return;
    loadBytes(state.built.bytes, "built");
    setStatus(`${buildFileName()} | ${formatBytes(state.built.bytes.byteLength)} | built in browser`);
    $("dsDesc").textContent = "Graph built from RDF text in this session — query it like any dataset.";
    setDatasetHeader(buildFileName(), "Graph built from RDF text in this session — query it like any dataset.");
    setMode("sparql");
  }

  async function loadBuildFile(file) {
    if (!file) return;
    try {
      const text = await file.text();
      setEd("buildText", text);
      state.built = { bytes: null, name: file.name };
      $("buildDownload").disabled = true;
      $("buildOpen").disabled = true;
      const ext = (file.name.match(/\.(\w+)$/) || [])[1] || "";
      const fmt = { nq: "nq", nquads: "nq", ttl: "ttl", turtle: "ttl" }[ext.toLowerCase()] || "nt";
      $("buildFormat").value = fmt;
      $("buildMeta").textContent = `${file.name} | ${formatBytes(file.size)} | ready to build`;
    } catch (e) {
      showError("buildOut", "File read failed: " + e.message);
    }
  }

  function showError(targetId, message) {
    $(targetId).innerHTML = `<div class="error-box">${esc(message)}</div>`;
    updateResultVisibility();
  }

  const HIST_KEY = "rete.playground.history";
  function loadHistory() {
    try {
      return JSON.parse(localStorage.getItem(HIST_KEY) || "[]");
    } catch (_e) {
      return [];
    }
  }

  function saveHistory(entry) {
    let history = loadHistory();
    history.unshift(entry);
    history = history.slice(0, 18);
    try {
      localStorage.setItem(HIST_KEY, JSON.stringify(history));
    } catch (_e) {
      return;
    }
    renderHistory();
  }

  function updateHistCount() {
    const n = loadHistory().length;
    const b = $("histCount");
    if (!b) return;
    b.textContent = n > 99 ? "99+" : String(n);
    b.classList.toggle("hidden", n === 0);
  }

  function renderHistory() {
    updateHistCount();
    const history = loadHistory();
    if (!history.length) {
      $("histList").innerHTML = `<div>No runs yet.</div>`;
      return;
    }
    $("histList").innerHTML = history.map((h, i) =>
      `<article class="history-item" data-hist="${i}">` +
      `<div class="mono">${esc(shorten((h.query || "").replace(/\s+/g, " "), 90))}</div>` +
      `<div>${esc(h.dataset)} | ${esc(h.strategy)} | ${esc(h.resultSummary || "")}</div>` +
      `</article>`
    ).join("");
    $$("#histList [data-hist]").forEach((el) => {
      el.onclick = () => {
        const h = loadHistory()[Number(el.dataset.hist)];
        if (!h) return;
        setEd("q", h.query || "");
        setView(h.format || "table");
        setStrategy(h.strategy || "whole");
        if (h.dataset && h.dataset !== state.dataset && RETE_DATASETS_B64[h.dataset]) loadDataset(h.dataset);
        setMode("sparql");
        closeHistory();
      };
    });
  }

  function updateHash() {
    const params = new URLSearchParams();
    params.set("dataset", state.dataset);
    params.set("mode", state.mode);
    const q = $("q").value.trim();
    if (q) params.set("q", q);
    history.replaceState(null, "", "#" + params.toString());
  }

  function readHash() {
    return new URLSearchParams(location.hash.replace(/^#/, ""));
  }

  async function shareUrl() {
    updateHash();
    const url = location.href;
    try {
      await navigator.clipboard.writeText(url);
      $("shareBtn").title = "Copied";
    } catch (_e) {
      $("qmeta").textContent = "Share URL: " + url;
    }
  }

  // Run the primary action of whichever panel is active (the Ctrl/Cmd+Enter target).
  function runActiveMode() {
    ({
      sparql: runQuery, shacl: runShacl, reach: runReach,
      provenance: runProvenance, coherence: runCoherence, build: runBuild
    }[state.mode] || runQuery)();
  }

  function wireEvents() {
    $("buildBtn").onclick = () => setMode("build");
    $("run").onclick = runQuery;
    $("strategy").onchange = () => setStrategy($("strategy").value);
    // Switching the Output type re-renders the last result in the new view
    // (no re-run) when it can; otherwise it runs the query.
    $("fmt").onchange = onOutputTypeChange;
    // Federation: the "+ Add source" popover + its source chips.
    $("fedAdd").onclick = (e) => {
      e.stopPropagation();
      $("fedPop").classList.contains("hidden") ? openFedPop() : closeFedPop();
    };
    $("fedAddConfirm").onclick = confirmAddFed;
    $("fedAddCancel").onclick = closeFedPop;
    $$("#fedModes button").forEach((b) => { b.onclick = () => setFedMode(b.dataset.fedmode); });
    $("fedPop").addEventListener("click", (e) => e.stopPropagation());
    $("fedChips").addEventListener("click", (e) => {
      const x = e.target.closest("[data-fedremove]");
      if (x) removeFedSource(x.getAttribute("data-fedremove"));
    });
    document.addEventListener("click", (e) => {
      if (!e.target.closest(".fed-add-wrap")) closeFedPop();
    });
    $$("#modeTabs button[data-mode]").forEach((btn) => {
      btn.onclick = () => setMode(btn.dataset.mode);
    });
    $("histBtn").onclick = openHistory;
    $("libCollapse").onclick = () => setLibCollapsed(true);
    $("libExpand").onclick = () => setLibCollapsed(false);
    // Close the dataset Load dropdown on any click outside it.
    document.addEventListener("click", (e) => {
      const menu = $("dsLoadMenu");
      if (menu && !e.target.closest(".ds-load")) menu.classList.add("hidden");
    });
    // Keep the top bar pinned; the dataset header sticks just below it and
    // condenses to a single line (title + metadata, no tagline) once scrolled.
    const dsHeader = document.querySelector(".ds-header");
    const topbar = document.querySelector(".topbar");
    if (dsHeader) {
      const setTop = () => {
        const tb = topbar ? topbar.offsetHeight : 0;
        dsHeader.style.top = tb + "px";
        // The mode rail sits just below both (sticky) headers — expose their
        // combined height PLUS the console-shell's 12px top padding (so this
        // equals the rail's actual top) for the rail's CSS top/min-height. It
        // tracks the dataset header as it condenses on scroll.
        document.documentElement.style.setProperty("--rail-top", tb + dsHeader.offsetHeight + 12 + "px");
      };
      setTop();
      window.addEventListener("resize", setTop, { passive: true });
      const onScroll = () => { dsHeader.classList.toggle("condensed", window.scrollY > 10); setTop(); };
      window.addEventListener("scroll", onScroll, { passive: true });
      onScroll();
    }
    $$("#exploreSeg button").forEach((btn) => {
      btn.onclick = () => setExploreView(btn.dataset.exp);
    });
    $("exampleSearch").oninput = renderExamples;
    $("urlLoad").onclick = loadFromUrl;
    $("fileInput").onchange = (e) => loadFromFile(e.target.files[0]);
    $("shareBtn").onclick = shareUrl;
    $("shaclRun").onclick = runShacl;
    $("coherenceRun").onclick = runCoherence;
    $("reachRun").onclick = runReach;
    $("whyRun").onclick = runProvenance;
    $("buildRun").onclick = runBuild;
    $("buildDownload").onclick = downloadBuilt;
    $("buildOpen").onclick = openBuilt;
    $("buildFile").onchange = (e) => loadBuildFile(e.target.files[0]);

    $("strategyHelp").onclick = () => $("strategyModal").classList.remove("hidden");
    $("roundHelp").onclick = () => $("strategyModal").classList.remove("hidden");
    $("outputHelp").onclick = () => $("outputModal").classList.remove("hidden");
    $("outputModalClose").onclick = () => $("outputModal").classList.add("hidden");
    $("outputModal").addEventListener("click", (e) => {
      if (e.target === $("outputModal")) $("outputModal").classList.add("hidden");
    });
    $("layoutCell").onchange = renderLayout;
    $("dsButton").onclick = openSource;
    $("sourceModalClose").onclick = closeSource;
    $("remoteConnect").onclick = connectRemote;
    $("sourceModal").addEventListener("click", (e) => {
      if (e.target === $("sourceModal")) closeSource();
    });
    $("strategyModalClose").onclick = () => $("strategyModal").classList.add("hidden");
    $("strategyModal").addEventListener("click", (e) => {
      if (e.target === $("strategyModal")) $("strategyModal").classList.add("hidden");
    });
    $("reqLogBtn").onclick = openReqLog;
    $("reqModalClose").onclick = () => $("reqModal").classList.add("hidden");
    $("reqModal").addEventListener("click", (e) => {
      if (e.target === $("reqModal")) $("reqModal").classList.add("hidden");
    });
    $("libraryModalClose").onclick = closeLibrary;
    $("libraryModal").addEventListener("click", (e) => {
      if (e.target === $("libraryModal")) closeLibrary();
    });
    $("historyModalClose").onclick = closeHistory;
    $("historyModal").addEventListener("click", (e) => {
      if (e.target === $("historyModal")) closeHistory();
    });
    $("settingsBtn").onclick = openSettings;
    $("settingsModalClose").onclick = closeSettings;
    $("settingsModal").addEventListener("click", (e) => {
      if (e.target === $("settingsModal")) closeSettings();
    });
    $("clearCacheAll").onclick = async () => {
      await idbClearAll();
      freeExploreEngines();
      renderCacheList();
      renderCacheCtl();
    };
    $("rangeCacheToggle").onchange = (e) => { setRangeCache(e.target.checked); renderRangeCache(); };
    $("clearRangeCacheBtn").onclick = async () => { await clearRangeCache(); renderRangeCache(); };
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        $("strategyModal").classList.add("hidden");
        $("outputModal").classList.add("hidden");
        $("reqModal").classList.add("hidden");
        closeLibrary();
        closeHistory();
        closeSettings();
        closeSource();
      }
      // Ctrl/Cmd+Enter runs the active panel's primary action from anywhere.
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
        e.preventDefault();
        runActiveMode();
      }
    });

    // Surface the shortcut on each panel's primary button.
    const shortcut = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘↵" : "Ctrl+↵";
    [["run", "Run query"], ["shaclRun", "Validate"], ["reachRun", "Run reach"],
     ["whyRun", "Explain matches"], ["coherenceRun", "Check coherence"],
     ["buildRun", "Build .rete"]].forEach(([id, label]) => {
      const b = $(id);
      if (b) b.title = `${label} (${shortcut})`;
    });

    // Collapsed tables: every "Show more" button reveals the next step of
    // hidden rows (delegated, so it works for any dynamically-rendered table).
    document.addEventListener("click", (e) => {
      const btn = e.target.closest(".tbl-more");
      if (!btn) return;
      const wrap = btn.closest(".tbl");
      const hidden = $$("tr.tr-hidden", wrap);
      hidden.slice(0, TABLE_MORE_STEP).forEach((tr) => tr.classList.remove("tr-hidden"));
      const left = Math.max(0, hidden.length - TABLE_MORE_STEP);
      if (left === 0) btn.remove();
      else btn.textContent = `Show ${Math.min(left, TABLE_MORE_STEP)} more (${left} hidden)`;
    });
    $("clearHist").onclick = () => {
      localStorage.removeItem(HIST_KEY);
      renderHistory();
    };

    const drop = $("dropZone");
    ["dragenter", "dragover"].forEach((ev) => {
      drop.addEventListener(ev, (e) => {
        e.preventDefault();
        drop.classList.add("drag");
      });
    });
    ["dragleave", "drop"].forEach((ev) => {
      drop.addEventListener(ev, (e) => {
        e.preventDefault();
        drop.classList.remove("drag");
      });
    });
    drop.addEventListener("drop", (e) => {
      const file = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
      loadFromFile(file);
    });
  }

  async function boot() {
    renderDatasetOptions();
    wireEvents();
    renderHistory();
    try { setLibCollapsed(localStorage.getItem(LIB_KEY) === "1"); } catch (_e) { /* ignore */ }
    enhanceEditor("q", "sparql");
    enhanceEditor("shapeText", "ttl");
    enhanceEditor("buildText", "ttl");
    setEd("buildText", BUILD_SAMPLE);

    // "Labels" decode toggle: float a human label over each IRI in the query.
    const decodeBtn = $("decodeToggle");
    if (decodeBtn && window.PlaygroundEditor) {
      decodeBtn.onclick = () => {
        const on = window.PlaygroundEditor.toggleDecode("q");
        decodeBtn.classList.toggle("active", on);
        decodeBtn.setAttribute("aria-pressed", on ? "true" : "false");
      };
    }
    // Entity finder panel beside the editor (debounced — a remote search is a
    // range-read round trip, so don't fire one on every keystroke).
    const efInput = $("efInput");
    if (efInput) {
      let efDebounce = null;
      efInput.oninput = () => { clearTimeout(efDebounce); efDebounce = setTimeout(efSearch, 180); };
      renderFinder([], "");
    }

    await wasm_bindgen(b64ToBytes(RETE_WASM_B64));

    const params = readHash();
    const ds = params.get("dataset") || CATALOG.defaultDataset;
    if (RETE_DATASETS_B64[ds]) state.dataset = ds;
    loadDataset(state.dataset);

    const q = params.get("q");
    if (q) {
      setEd("q", q);
      state.selectedExample = -1;
      renderExamples();
    }
    setMode(params.get("mode") || "sparql");
    updateResultVisibility();
  }

  boot().catch((e) => {
    setStatus("boot failed");
    showError("out", String(e && e.stack ? e.stack : e));
  });
})();
