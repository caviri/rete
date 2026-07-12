(function () {
  "use strict";

  const CATALOG = window.RETE_PLAYGROUND_CATALOG;
  const state = {
    bytes: null,
    dataset: CATALOG.defaultDataset,
    mode: "sparql",
    family: "All",
    selectedExample: -1,
    colLabels: null,
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
    // Asyncify concurrent reads: the asyncified wasm variant fetches each remote
    // query's byte ranges in parallel (Promise.all of fetch), no cross-origin
    // isolation. ON by default and no longer user-exposed — the only opt-out is a
    // console `localStorage.setItem("asyncReadsOn","0")` escape hatch. Costs the
    // ~8 MB async wasm only when a REMOTE query first runs (embedded stays sync).
    asyncReadsOn: (() => { try { return localStorage.getItem("asyncReadsOn") !== "0"; } catch (e) { return true; } })(),
    // Map view: which slippy-tile basemap sits behind the geometry ("none" =
    // the offline equirectangular vectors). localStorage-backed so it persists.
    mapBasemap: (() => { try { return localStorage.getItem("mapBasemap") || "none"; } catch (e) { return "none"; } })(),
    remote: null,
    // Federation: extra sources the SPARQL query also runs against. Each is
    // {id, kind:"remote"|"memory"|"endpoint", label, url?, key?, endpoint?}.
    // Empty = single-source (today's behavior). A resident Graph per in-memory
    // partner is cached in fedGraphs so repeated federated runs don't re-decode.
    fedSources: [],
    fedGraphs: new Map(),
    // Live-endpoint mode: a SPARQL Protocol URL (e.g. a local `rete serve`)
    // that becomes the ONLY query target, with SPARQL *Update* enabled — the
    // playground as the editing UI over a live, updatable rete. null = off.
    liveEndpoint: null,
    // The last successful query result, kept so switching the Output type
    // re-renders it in the new view instead of re-running the query.
    lastResult: null,
    // Find-a-term modal: the chosen label predicate ("auto" or a single IRI),
    // and the predicate currently drilled into for a faceted value browse
    // ({iri, label}) or null when showing the term list.
    labelProp: "auto",
    facet: null,
    // The dataset builder's in-progress example rows (SPARQL/SHACL). The last
    // built+saved dataset itself is kept in `state.built` (declared above).
    buildEx: []
  };

  const RDF_TYPE = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>";

  // User-built datasets kept in this browser (IndexedDB): key -> raw .rete bytes.
  // They're merged into CATALOG at boot so they behave like the bundled ones
  // (selectable, queryable, with their own card + example library), and removed
  // again on delete. See the dataset-builder section near the bottom of this file.
  const userBytes = new Map();
  // Set once the wasm engine is initialized, so the live RDF validator doesn't
  // try to build before `wasm_bindgen()` has run (it fires on the first edit,
  // which can race the boot await).
  let wasmReady = false;

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
  // ASYNC mode (the asyncified wasm variant): every read is a concurrent
  // Promise.all of fetch, so each wasm query call must be driven through the
  // Asyncify suspend/rewind loop (wasm_bindgen.reteDrive). OFF: the wasm runs
  // synchronously over sync-XHR exactly as before. The flag is baked into the
  // worker blob at creation (self.__RETE_ASYNC), so flipping it recreates the worker.
  var ASYNC = !!self.__RETE_ASYNC;
  // Drive one wasm call: glue-in-loop in async mode (the wrapper is re-entered
  // each suspend), a plain sync call otherwise.
  function _drive(fn) { return ASYNC ? wasm_bindgen.reteDrive(fn) : Promise.resolve(fn()); }
  // Open (or reuse) a resident session. In async mode the OPEN is driven via
  // reteOpenRemote (wraps the pointer once after rewind — no garbage instance for
  // the FinalizationRegistry). content_hash()/stats() do no IO, so they stay sync.
  function _session(url) {
    var h = urlHash[url];
    if (h && sessions[h]) return Promise.resolve(sessions[h]);
    var openP = ASYNC ? wasm_bindgen.reteOpenRemote(url)
                      : Promise.resolve(new wasm_bindgen.RemoteGraph(url));
    return Promise.resolve(openP).then(function (g) {
      var hh = g.content_hash();
      urlHash[url] = hh;
      if (sessions[hh]) { try { g.free(); } catch (e) { /* dup URL */ } return sessions[hh]; }
      sessions[hh] = g;
      return sessions[hh];
    });
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
      self.__fetchSrc = m.fetchSrc;   // parallel fetch-worker source (pool)
      self.__poolSize = m.poolSize;   // ?workers=N, or null = auto
      ready = wasm_bindgen(m.bytes);
      ready.then(function () { self.postMessage({ type: "ready" }); });
      return;
    }
    if (m.type === "query") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () { return _session(m.url); }).then(function (g) {
        var before = JSON.parse(g.stats());
        return _drive(function () { return m.reason ? g.query_reasoned(m.query, m.format) : g.query(m.query, m.format); }).then(function (resStr) {
          var res = JSON.parse(resStr);
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
        });
      }).catch(function (err) {
        self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
      });
    }
    // Generic call to any *_url wasm export (schema_url, check_schema_url, …).
    // These do range-read IO (sync XHR, or driven Asyncify fetch), which is
    // worker-only — so the main thread MUST route them here.
    if (m.type === "call") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () {
        return _drive(function () { return wasm_bindgen[m.fn].apply(null, m.args); });
      }).then(function (json) {
        self.postMessage({ type: "result", id: m.id, ok: true, json: json, log: fetchLog });
      }).catch(function (err) {
        self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
      });
    }
    // Label-prefix entity search over the resident remote session: faults only
    // the label-index tiles, like a query.
    if (m.type === "psearch") {
      pReq = 0; pBytes = 0; pId = m.id; fetchLog = []; qStart = _now();
      Promise.resolve(ready).then(function () { return _session(m.url); }).then(function (g) {
        return _drive(function () { return g.prefix_search(m.prefix, m.limit || 12); });
      }).then(function (json) {
        self.postMessage({ type: "result", id: m.id, ok: true, json: json, log: fetchLog });
      }).catch(function (err) {
        self.postMessage({ type: "result", id: m.id, ok: false, error: String(err), log: fetchLog });
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
  function __readManyMultipart(url, offsets, lens) {
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
  }
  // Parallel fetch-worker pool (cross-origin isolated only): fetch the batch's
  // ranges across self.__poolSize workers into a SharedArrayBuffer and block on
  // Atomics until all land — the only way a synchronous engine parallelises
  // network in the browser. Returns the buffer, or null (→ multipart fallback)
  // on no-isolation / a failed range / a timeout.
  var __pool = null;
  function __ensurePool() {
    if (__pool) return __pool;
    var P = self.__poolSize || ((self.navigator && navigator.hardwareConcurrency) || 4);
    P = Math.max(1, Math.min(32, P));
    __pool = [];
    var u = URL.createObjectURL(new Blob([self.__fetchSrc], { type: "text/javascript" }));
    for (var i = 0; i < P; i++) __pool.push(new Worker(u));
    return __pool;
  }
  function __readManyPool(url, offs, lens) {
    try {
      var n = offs.length; if (!n) return null;
      var pos = new Array(n), total = 0;
      for (var i = 0; i < n; i++) { pos[i] = total; total += lens[i]; }
      var data = new Uint8Array(new SharedArrayBuffer(total));
      var ctrl = new Int32Array(new SharedArrayBuffer(8));
      var pool = __ensurePool(), P = pool.length, jobs = [];
      for (var w = 0; w < P; w++) jobs.push([]);
      for (var j = 0; j < n; j++) jobs[j % P].push({ off: offs[j], len: lens[j], pos: pos[j] });
      for (var w2 = 0; w2 < P; w2++) pool[w2].postMessage({ url: url, data: data.buffer, ctrl: ctrl.buffer, spans: jobs[w2] });
      var deadline = Date.now() + 120000;
      while (true) { var c = Atomics.load(ctrl, 0); if (c >= n) break; Atomics.wait(ctrl, 0, c, 3000); if (Date.now() > deadline) return null; }
      if (Atomics.load(ctrl, 1) !== 0) return null;
      if (self.reteProgress) self.reteProgress(total, { k: "par", n: n, b: total });
      return data;
    } catch (e) { return null; }
  }
  self.reteReadMany = function (url, offsets, lens) {
    if (self.crossOriginIsolated && typeof SharedArrayBuffer !== "undefined" && self.__fetchSrc) {
      var r = __readManyPool(url, offsets, lens);
      if (r) return r;
    }
    return __readManyMultipart(url, offsets, lens);
  };
})();`;

  // The parallel fetch worker (one per pool slot): pulls its assigned ranges
  // with synchronous XHR (sequential within the worker, parallel across the
  // pool), writes each into the shared buffer at its offset, then signals the
  // coordinator via Atomics. A 206-miss / short read flips the error flag.
  const FETCH_WORKER_SRC = `
self.onmessage = function (e) {
  var m = e.data, data = new Uint8Array(m.data), ctrl = new Int32Array(m.ctrl);
  for (var k = 0; k < m.spans.length; k++) {
    var s = m.spans[k];
    try {
      var x = new XMLHttpRequest();
      x.open("GET", m.url, false);
      x.responseType = "arraybuffer";
      x.setRequestHeader("Range", "bytes=" + s.off + "-" + (s.off + s.len - 1));
      x.send();
      if (x.status !== 206) { Atomics.store(ctrl, 1, 1); }
      else {
        var b = new Uint8Array(x.response);
        if (b.length < s.len) { Atomics.store(ctrl, 1, 1); }
        else { data.set(b.subarray(0, s.len), s.pos); }
      }
    } catch (err) { Atomics.store(ctrl, 1, 1); }
    Atomics.add(ctrl, 0, 1); Atomics.notify(ctrl, 0);
  }
};`;

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

  // Tear down the remote worker WITHOUT rejecting pending as "cancelled" — used
  // after a wasm trap (see below), where the query already failed with its real
  // error. A trapped wasm instance is poisoned and can't be reused, so the next
  // query must rebuild a fresh worker.
  function resetRemoteWorker() {
    if (remoteWorker) { remoteWorker.terminate(); remoteWorker = null; remoteReady = null; remoteResolveReady = null; }
    remoteOnProgress = null;
  }

  // A wasm trap that a memory limit *could* explain: an `unreachable` (Rust
  // abort, e.g. a failed allocation) or a memory access out of bounds. Kept
  // NARROW on purpose — generic errors (a plain RangeError, a stack overflow, a
  // bug) must NOT be mislabeled "out of memory". The caller also gates on the
  // dataset actually being large; a small file can't genuinely OOM the engine.
  function isEngineTrap(msg) {
    return /unreachable|out of memory|memory access out of bounds/i.test(String(msg || ""));
  }

  // Viewport-based "small device" check, for the memory-reclamation paths below.
  function isPhoneView() { return !!(window.matchMedia && window.matchMedia("(max-width: 560px)").matches); }

  // Free everything reclaimable that isn't needed right now, to keep a phone's
  // tab under iOS Safari's memory budget. A wasm heap only shrinks by discarding
  // the whole instance, so this tears the workers down — they rebuild lazily on
  // next use (a remote re-query re-fetches; the range cache, if on, makes that
  // cheap). No-op'd engines/graphs are skipped.
  function freeMobileMemory() {
    cancelRemote();        // the remote worker caches a RemoteGraph per URL — the big accumulator
    freeExploreEngines();  // DuckDB / SQLite WASM backends
    // The in-browser LLM (Ask AI) holds a large WebGPU/wasm model — drop it when
    // idle; the next Ask AI reloads it (weights come from the browser HTTP cache).
    if (llmWorker && !llmBusy) { try { llmWorker.terminate(); } catch (e) { /* ignore */ } llmWorker = null; llmLoaded = false; }
  }

  // The asyncified wasm variant (glue + bytes) lives in separate files so it costs
  // the default page nothing; fetched once, lazily, when the toggle is first used.
  let asyncGlueText = null, asyncWasmBytes = null;
  function loadAsyncAssets() {
    if (asyncGlueText && asyncWasmBytes) return Promise.resolve();
    return Promise.all([
      fetch("rete_wasm_async.js").then((r) => { if (!r.ok) throw new Error("async glue " + r.status); return r.text(); }),
      fetch("rete_wasm_async.wasm").then((r) => { if (!r.ok) throw new Error("async wasm " + r.status); return r.arrayBuffer(); }),
    ]).then((a) => { asyncGlueText = a[0]; asyncWasmBytes = new Uint8Array(a[1]); });
  }

  function ensureRemoteWorker() {
    if (remoteReady) return remoteReady; // built, or building (avoid a double-build race)
    const asyncOn = !!state.asyncReadsOn;
    remoteReady = (asyncOn ? loadAsyncAssets() : Promise.resolve()).then(() => {
      const glue = asyncOn ? asyncGlueText : document.getElementById("reteGlue").textContent;
      const flag = asyncOn ? "self.__RETE_ASYNC=true;\n" : "";
      const blob = new Blob([flag + rcPrelude() + glue + REMOTE_HARNESS + COALESCE_JS], { type: "text/javascript" });
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
      const rp = new Promise((res) => { remoteResolveReady = res; });
      remoteWorker.postMessage({
        type: "init", bytes: asyncOn ? asyncWasmBytes : b64ToBytes(RETE_WASM_B64),
        fetchSrc: FETCH_WORKER_SRC, poolSize: parallelWorkerCount()
      });
      return rp;
    });
    remoteReady.catch(() => { remoteWorker = null; remoteReady = null; }); // let a failed build retry
    return remoteReady;
  }

  function remoteSparql(url, query, fmt, reason) {
    return ensureRemoteWorker().then(() => new Promise((resolve, reject) => {
      const id = ++remoteSeq;
      remotePending.set(id, { resolve, reject });
      remoteWorker.postMessage({ type: "query", id, url, query, format: fmt || "table", reason: !!reason });
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
  function remoteRead(fn, urlOrArgs, el, caption, hint) {
    const args = Array.isArray(urlOrArgs) ? urlOrArgs : [urlOrArgs];
    const t0 = performance.now();
    const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[state.dataset]) || {};
    const ofSize = meta.size ? " of " + meta.size : "";
    let lastReq = 0, lastBytes = 0;
    // Compact, matching the SPARQL tab: the animated network spinner + ONE live-
    // updating line (requests · bytes · elapsed) — NOT a line per range request,
    // which floods the panel on a big lazy read (e.g. SHACL over a 4.56 GB graph
    // can make 1000+ range requests). The per-request detail stays available in
    // the "requests" inspector that runRemote logs.
    el.innerHTML =
      `<div class="range-read">` +
        netSpinner(caption || "querying remote…") +
        `<div class="cache-bar indeterminate"><div class="cache-bar-fill"></div></div>` +
        `<div class="range-read-meta" id="rrMeta"></div>` +
        (hint ? `<div class="range-read-hint">${esc(hint)}</div>` : "") +
      `</div>`;
    const metaEl = el.querySelector("#rrMeta");
    const paint = () => {
      const dt = (performance.now() - t0) / 1000;
      if (metaEl) metaEl.textContent = `⏳ ${lastReq} range request(s) · ${formatBytes(lastBytes)}${ofSize} fetched · ${dt.toFixed(1)}s`;
    };
    paint();
    const timer = setInterval(paint, 150);
    const prev = remoteOnProgress;
    remoteOnProgress = (m) => { lastReq = m.requests; lastBytes = m.bytes; paint(); };
    const cleanup = () => { clearInterval(timer); remoteOnProgress = prev; };
    return remoteCall(fn, ...args).then((out) => { cleanup(); return out; }, (e) => { cleanup(); throw e; });
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
    const wrap = b && b.closest && b.closest(".switch-ctl");
    if (wrap) wrap.classList.toggle("loading", !!on);
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
  // The predicate(s) to read as a human label. "auto" tries the common ones;
  // otherwise the single property chosen in the Find-a-term modal (state.labelProp).
  function labelPredicates() {
    const lp = state.labelProp;
    if (lp && lp !== "auto") return "<" + lp + ">";
    return "<http://www.w3.org/2000/01/rdf-schema#label> <http://www.w3.org/2004/02/skos/core#prefLabel> " +
      "<http://xmlns.com/foaf/0.1/name> <http://schema.org/name>";
  }
  function labelQueryFor(iris) {
    const values = iris.slice(0, 60).map((i) => "<" + i + ">").join(" ");
    return "SELECT ?s ?l WHERE { VALUES ?s { " + values + " }\n" +
      "  ?s ?p ?l . VALUES ?p { " + labelPredicates() + " }\n" +
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

  function enhanceEditor(id, lang, onChange) {
    if (!window.PlaygroundEditor) return;
    window.PlaygroundEditor.enhance(id, lang, {
      schema: () => state.schema,
      searchEntities: entitySearch,
      labelHints: labelHintsFor,
      resolveLabels: resolveLabels,
      onChange: onChange || null
    });
  }

  // ── Entity finder (the panel beside the SPARQL editor) ───────────────────
  // Type a human string; rete's bounded label index (prefix_search, no scan)
  // returns matching entities; click one to insert its <IRI> at the caret.
  function localName(iri) { return String(iri).replace(/[<>]/g, "").replace(/^.*[\/#:]/, "") || iri; }
  function insertAtCaret(id, text) {
    // Prefer the CodeMirror editor (inserts at its cursor); fall back to the raw
    // textarea if the editor isn't mounted.
    if (window.PlaygroundEditor && PlaygroundEditor.insert && PlaygroundEditor.editors[id]) {
      PlaygroundEditor.insert(id, text);
      return;
    }
    const ta = $(id);
    if (!ta) return;
    const s = ta.selectionStart || 0, e = ta.selectionEnd || 0;
    ta.value = ta.value.slice(0, s) + text + ta.value.slice(e);
    const caret = s + text.length;
    ta.setSelectionRange(caret, caret);
    ta.focus();
  }
  function efItemHtml(iri, label, kind) {
    return `<button type="button" class="ef-item" data-iri="${esc(iri)}" title="${esc(iri)}">` +
      `<span class="ef-label">${esc(label || localName(iri))}</span>` +
      `<span class="ef-iri"><span class="ef-kind ef-${kind}">${esc(kind)}</span> ${esc(localName(iri))}</span></button>`;
  }
  // A schema-term row: the insert button, plus (for predicates) a "values ›"
  // drill button that opens a faceted browse of the values that predicate takes.
  function efTermRowHtml(t) {
    const item = efItemHtml(t.iri, t.label, t.kind);
    if (t.kind !== "predicate") return item;
    return `<div class="ef-row">${item}` +
      `<button type="button" class="ef-drill" data-iri="${esc(t.iri)}" data-label="${esc(t.label || localName(t.iri))}"` +
      ` title="Browse the values this predicate takes — cached after the first read">values ›</button></div>`;
  }
  // Tier 1: classes + predicates from the cached schema (the card), matched by
  // their hint label, local name, or IRI substring — instant, no graph reads.
  function searchSchemaTerms(q, limit) {
    const sch = state.schema;
    if (!sch) return [];
    const ql = q.toLowerCase();
    const hints = (CATALOG.labelHints && CATALOG.labelHints[state.dataset]) || {};
    const out = [], seen = new Set();
    const consider = (rawIri, kind) => {
      const iri = String(rawIri).replace(/^<|>$/g, "");
      if (!iri || seen.has(iri)) return;
      const label = hints[iri] || "";
      const ln = localName(iri);
      if (!ql || (label && label.toLowerCase().includes(ql)) || ln.toLowerCase().includes(ql) || iri.toLowerCase().includes(ql)) {
        seen.add(iri);
        out.push({ iri, kind, label: label || ln });
      }
    };
    (sch.classes || []).forEach((c) => consider(c[0], "class"));
    const preds = new Set();
    (sch.relations || []).forEach((r) => preds.add(String(r[1])));
    preds.forEach((p) => consider(p, "predicate"));
    return out.slice(0, limit || 14);
  }
  // Remote datasets read the schema lazily (on Explore); fetch it once in the
  // background for the finder so tier 1 works in the console too.
  let efSchemaFetching = false;
  function ensureSchemaForFinder() {
    if (state.schema || !state.remote || efSchemaFetching) return;
    efSchemaFetching = true;
    remoteCall("schema_url", state.remote.url)
      .then((out) => { try { state.schema = JSON.parse(out.json); } catch (_e) { /* none */ } })
      .catch(() => {})
      .finally(() => { efSchemaFetching = false; if (!$("finderModal").classList.contains("hidden")) efSearch(); });
  }
  function renderFinder(schemaTerms, entityHits, q, entitiesLoading, browse) {
    const box = $("efResults");
    if (!box) return;
    let html = "";
    if (schemaTerms.length) {
      html += `<div class="ef-group">Schema — classes &amp; predicates${browse ? " · type to filter" : ""}</div>` +
        schemaTerms.map((t) => efTermRowHtml(t)).join("");
    }
    if (browse) {
      html += schemaTerms.length
        ? `<p class="ef-empty">Type to also search entities by label.</p>`
        : `<p class="ef-empty">${(state.remote && efSchemaFetching) ? "Reading the schema over HTTP range…" : "No schema to browse — type to search entities by label."}</p>`;
    } else if (entitiesLoading) {
      html += `<div class="ef-group">Entities</div><div class="ef-loading"><span class="spindle"></span> labels over range reads…</div>`;
    } else if (entityHits.length) {
      html += `<div class="ef-group">Entities</div>` +
        entityHits.map((h) => efItemHtml(String(h.subject).replace(/^<|>$/g, ""), h.label, "entity")).join("");
    } else if (!schemaTerms.length) {
      html += `<p class="ef-empty">No schema term or entity matches “${esc(q)}”.</p>`;
    }
    box.innerHTML = html;
    $$("#efResults .ef-item").forEach((b) => {
      b.onclick = () => { insertAtCaret("q", "<" + b.dataset.iri + ">"); closeFinder(); };
    });
    $$("#efResults .ef-drill").forEach((b) => {
      b.onclick = (e) => { e.stopPropagation(); openFacet(b.dataset.iri, b.dataset.label); };
    });
  }
  // ── Faceted value browse ─────────────────────────────────────────────────
  // Click "values ›" on a predicate to see the distinct objects it takes (with
  // human labels), then click one to drop it into the query. Each predicate's
  // values are read once and cached, so re-opening is instant — exactly the
  // "propose a predicate, then the values within it" flow. Works embedded
  // (synchronous) and remote-lazy (a single range-read SPARQL round trip).
  const facetCache = new Map(); // predicate IRI -> [{iri|lit, label}]
  function facetValueQuery(pred) {
    return "SELECT DISTINCT ?v ?l WHERE {\n" +
      "  ?s <" + pred + "> ?v .\n" +
      "  OPTIONAL { ?v ?lp ?l . VALUES ?lp { " + labelPredicates() + " } FILTER(isLiteral(?l)) }\n" +
      "} LIMIT 120";
  }
  function parseFacetRows(res) {
    const out = [], seen = new Set();
    (res.rows || []).forEach((r) => {
      const v = String(r.v || "");
      if (!v || seen.has(v)) return;
      seen.add(v);
      if (v.startsWith("<") && v.endsWith(">")) {
        const iri = v.slice(1, -1);
        const lm = String(r.l || "").match(/^"((?:\\.|[^"\\])*)"/);
        out.push({ iri, label: (lm ? lm[1] : "") || localName(iri) });
      } else {
        const lm = v.match(/^"((?:\\.|[^"\\])*)"/);
        out.push({ lit: v, label: lm ? lm[1] : v });
      }
    });
    return out.slice(0, 80);
  }
  function openFacet(iri, label) {
    state.facet = { iri, label: label || localName(iri) };
    const inp = $("efInput"); if (inp) inp.value = "";
    const cached = facetCache.get(iri);
    if (cached) { renderFacet(cached, false); return; }
    renderFacet([], true);
    const seq = ++efSeq;
    const apply = (vals) => {
      facetCache.set(iri, vals);
      if (state.facet && state.facet.iri === iri && seq === efSeq) renderFacet(vals, false);
    };
    if (state.remote) {
      remoteSparql(state.remote.url, facetValueQuery(iri), "table")
        .then((out) => apply(parseFacetRows(JSON.parse(out.json))))
        .catch(() => { if (state.facet && state.facet.iri === iri && seq === efSeq) renderFacet([], false); });
      return;
    }
    let vals = [];
    if (state.graph) { try { vals = parseFacetRows(JSON.parse(state.graph.query(facetValueQuery(iri), "table"))); } catch (_e) { vals = []; } }
    apply(vals);
  }
  function renderFacet(values, loading) {
    const box = $("efResults"); if (!box) return;
    const f = state.facet || {};
    const q = (($("efInput") || {}).value || "").trim().toLowerCase();
    let shown = values;
    if (q) shown = values.filter((x) => (x.label || "").toLowerCase().includes(q) || String(x.iri || x.lit || "").toLowerCase().includes(q));
    let html = `<button type="button" class="ef-back" id="efBack">‹ Back to terms</button>`;
    html += `<div class="ef-group">Values of <span class="ef-facet-pred">${esc(f.label || "")}</span></div>`;
    if (loading) {
      html += `<div class="ef-loading"><span class="spindle"></span> reading values${state.remote ? " over range reads" : ""}…</div>`;
    } else if (!shown.length) {
      html += `<p class="ef-empty">${values.length ? "No value matches your filter." : "No values found for this predicate."}</p>`;
    } else {
      html += shown.slice(0, 80).map((x) => {
        if (x.iri) return efItemHtml(x.iri, x.label, "entity");
        return `<button type="button" class="ef-item" data-lit="${esc(x.lit)}" title="${esc(x.lit)}">` +
          `<span class="ef-label">${esc(x.label)}</span>` +
          `<span class="ef-iri"><span class="ef-kind ef-literal">literal</span> ${esc(x.lit)}</span></button>`;
      }).join("");
    }
    box.innerHTML = html;
    const back = $("efBack");
    if (back) back.onclick = () => { state.facet = null; const inp = $("efInput"); if (inp) inp.value = ""; efSearch(); };
    $$("#efResults .ef-item").forEach((b) => {
      b.onclick = () => {
        insertAtCaret("q", b.dataset.iri ? ("<" + b.dataset.iri + ">") : b.dataset.lit);
        closeFinder();
      };
    });
  }
  // Two-tier search: schema terms (instant, cached) + entities by label
  // (synchronous for embedded; a lazy worker range-read for remote, with a
  // spinner and a sequence guard that drops out-of-order keystroke results).
  // An empty box browses the schema (classes & predicates) up front.
  let efSeq = 0;
  function efSearch() {
    const inp = $("efInput");
    if (!inp) return;
    // In faceted (value-drill) mode the input filters the loaded values
    // client-side; openFacet owns the (cached) read, so don't re-fetch here.
    if (state.facet) {
      const cached = facetCache.get(state.facet.iri);
      renderFacet(cached || [], !cached);
      return;
    }
    const q = (inp.value || "").trim();
    ensureSchemaForFinder();
    const schemaTerms = searchSchemaTerms(q, q ? 14 : 80);
    if (!q) { renderFinder(schemaTerms, [], "", false, true); return; }
    const seq = ++efSeq;
    if (state.remote) {
      renderFinder(schemaTerms, [], q, true, false);
      remotePrefixSearch(state.remote.url, q, 12).then((out) => {
        if (seq !== efSeq) return;
        let hits = []; try { hits = JSON.parse(out.json); } catch (_e) { hits = []; }
        renderFinder(schemaTerms, hits, q, false, false);
      }).catch(() => { if (seq === efSeq) renderFinder(schemaTerms, [], q, false, false); });
      return;
    }
    let hits = [];
    if (state.graph) { try { hits = JSON.parse(state.graph.prefix_search(q, 12)); } catch (_e) { hits = []; } }
    renderFinder(schemaTerms, hits, q, false, false);
  }
  function openFinder() {
    $("finderModal").classList.remove("hidden");
    state.facet = null; // always open on the term list
    const inp = $("efInput");
    if (inp) { try { inp.value = ""; inp.focus(); } catch (_e) { /* ignore */ } }
    efSearch();
  }
  function closeFinder() { $("finderModal").classList.add("hidden"); }

  function setEd(id, text) {
    if (window.PlaygroundEditor) window.PlaygroundEditor.setText(id, text);
    else { const t = $(id); if (t) t.value = text; }
  }

  // ── Link preview: a medium hover card that renders an http(s) result cell's
  // page in a sandboxed iframe (+ an always-working "Open ↗"). Some sites block
  // embedding (X-Frame-Options / CSP); the header + link still work there.
  let lpEl = null, lpShowTimer = null, lpHideTimer = null, lpUrl = null;
  function ensureLinkPreview() {
    if (lpEl) return lpEl;
    lpEl = document.createElement("div");
    lpEl.className = "link-preview hidden";
    lpEl.innerHTML =
      `<div class="lp-head"><span class="lp-host"></span>` +
      `<a class="lp-open" target="_blank" rel="noopener noreferrer">Open ↗</a></div>` +
      `<div class="lp-frame"><div class="lp-loading"><span class="spindle"></span></div>` +
      `<iframe class="lp-iframe" sandbox="allow-scripts allow-same-origin" referrerpolicy="no-referrer"></iframe></div>` +
      `<div class="lp-note">Scaled desktop preview — some sites block embedding; use Open ↗.</div>`;
    document.body.appendChild(lpEl);
    lpEl.addEventListener("mouseenter", () => clearTimeout(lpHideTimer));
    lpEl.addEventListener("mouseleave", hideLinkPreview);
    return lpEl;
  }
  function positionLinkPreview(anchor) {
    const r = anchor.getBoundingClientRect();
    const w = lpEl.offsetWidth || 420, h = lpEl.offsetHeight || 360;
    let left = Math.min(r.left, window.innerWidth - w - 8);
    let top = r.bottom + 8;
    if (top + h > window.innerHeight - 8) top = Math.max(8, r.top - h - 8);
    lpEl.style.left = Math.max(8, left) + "px";
    lpEl.style.top = top + "px";
  }
  function showLinkPreview(anchor, url) {
    ensureLinkPreview();
    if (lpUrl !== url) {
      lpUrl = url;
      let host = url;
      try { host = new URL(url).host.replace(/^www\./, ""); } catch (_e) { /* keep url */ }
      const target = httpsUpgrade(url);
      lpEl.querySelector(".lp-host").textContent = host;
      lpEl.querySelector(".lp-open").href = target;
      const frame = lpEl.querySelector(".lp-iframe");
      const loading = lpEl.querySelector(".lp-loading");
      loading.style.display = "flex";
      frame.style.visibility = "hidden";
      frame.onload = () => { loading.style.display = "none"; frame.style.visibility = "visible"; };
      frame.src = target;
    }
    lpEl.classList.remove("hidden");
    positionLinkPreview(anchor);
  }
  function hideLinkPreview() {
    clearTimeout(lpHideTimer);
    lpHideTimer = setTimeout(() => { if (lpEl) lpEl.classList.add("hidden"); }, 180);
  }
  function bindLinkPreviews() {
    document.body.addEventListener("mouseover", (e) => {
      const a = e.target.closest && e.target.closest(".iri-link");
      if (!a) return;
      clearTimeout(lpShowTimer); clearTimeout(lpHideTimer);
      lpShowTimer = setTimeout(() => showLinkPreview(a, a.dataset.url), 380);
    });
    document.body.addEventListener("mouseout", (e) => {
      const a = e.target.closest && e.target.closest(".iri-link");
      if (!a) return;
      clearTimeout(lpShowTimer);
      hideLinkPreview();
    });
  }

  // ---- Image thumbnail hover-zoom -------------------------------------------
  // Hovering an image cell (.cell-thumb — imageCell + IIIF) pops a larger, crisp
  // preview in a floating box, so you can read a scan/photo without leaving the table
  // or opening the lightbox. Positioned HARMONICALLY: anchored beside the thumb (right,
  // or left when there's no room), vertically centred on it, and always clamped inside
  // the viewport — never clipped by the table's own scroll/overflow. Reuses one fixed
  // element. A Commons `width=200` thumb is re-requested at a larger width so the zoom
  // is sharp, not upscaled.
  let tzEl = null, tzShowTimer = 0;
  function ensureThumbZoom() {
    if (tzEl) return tzEl;
    tzEl = document.createElement("img");
    tzEl.className = "thumb-zoom hidden";
    tzEl.alt = "";
    tzEl.addEventListener("error", hideThumbZoom);
    document.body.appendChild(tzEl);
    return tzEl;
  }
  function positionThumbZoom(img) {
    if (!tzEl) return;
    const r = img.getBoundingClientRect();
    const zw = tzEl.offsetWidth || 320, zh = tzEl.offsetHeight || 280;
    const m = 12, vw = window.innerWidth, vh = window.innerHeight;
    let left = r.right + m;                              // prefer to the right of the thumb
    if (left + zw + m > vw) left = r.left - m - zw;      // no room → to the left
    left = Math.max(m, Math.min(vw - zw - m, left));     // clamp horizontally
    let top = r.top + r.height / 2 - zh / 2;             // centre on the thumb
    top = Math.max(m, Math.min(vh - zh - m, top));       // clamp vertically
    tzEl.style.left = left + "px";
    tzEl.style.top = top + "px";
  }
  function showThumbZoom(img) {
    const src0 = img.currentSrc || img.src;
    if (!src0) return;
    const src = src0.replace(/([?&]width=)200\b/, (m, g1) => g1 + "900"); // sharper for Commons thumbs
    const z = ensureThumbZoom();
    z.onload = () => positionThumbZoom(img);
    z.src = src;
    z.classList.remove("hidden");
    positionThumbZoom(img);                              // place now (re-placed on load)
  }
  function hideThumbZoom() { if (tzEl) tzEl.classList.add("hidden"); }
  function bindThumbZoom() {
    document.body.addEventListener("mouseover", (e) => {
      const img = e.target.closest && e.target.closest(".cell-thumb");
      if (!img) return;
      clearTimeout(tzShowTimer);
      tzShowTimer = setTimeout(() => showThumbZoom(img), 200);
    });
    document.body.addEventListener("mouseout", (e) => {
      const img = e.target.closest && e.target.closest(".cell-thumb");
      if (!img) return;
      clearTimeout(tzShowTimer);
      hideThumbZoom();
    });
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
    // Phone: leaving remote-lazy mode for an in-memory graph — free a leftover
    // remote worker so the prior remote dataset's heap is reclaimed.
    if (isPhoneView() && remoteWorker) cancelRemote();
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
    $("dsDesc").innerHTML = catalogSource
      ? mdLite(infoRow.description)
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
    let bytes = null;
    if (userBytes.has(key)) bytes = userBytes.get(key);
    else { const b64 = RETE_DATASETS_B64[key]; if (b64) bytes = b64ToBytes(b64); }
    if (!bytes) {
      setStatus("dataset not available: " + key);
      return;
    }
    state.dataset = key;
    setDatasetName(key);
    loadBytes(bytes, "bundled");
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
    // Phone: switching to a DIFFERENT remote dataset — tear down the old remote
    // worker so the previous dataset's resident heap (the worker caches a
    // RemoteGraph per URL) doesn't accumulate. Desktop keeps it for fast
    // switch-back. Same-URL re-entry keeps the resident engine.
    if (isPhoneView() && state.remote && state.remote.url !== url) cancelRemote();
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
    // The source pill already says "remote (lazy)" — don't repeat it here.
    setStatus("queries range-fetch only what they touch");
    const info = datasetKey ? datasetInfo(datasetKey) : null;
    $("dsDesc").innerHTML = info ? mdLite(info.description) : "Remote graph, queried lazily over HTTP range: " + esc(url);
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
    // Derived datasets live one-folder-per-dataset on the CDN: <base>/<key>/<key>.rete.
    const tok = CATALOG.remoteToken ? "?token=" + CATALOG.remoteToken : "";
    return `${CATALOG.remoteBase}/${key}/${key}.rete${tok}`;
  }
  function isEmbedded(key) { return !!RETE_DATASETS_B64[key] || userBytes.has(key); }
  // A user-built dataset (kept in this browser), vs a bundled/remote catalog one.
  function isCustom(key) {
    const ex = CATALOG.datasetExtra && CATALOG.datasetExtra[key];
    return !!(ex && ex.custom);
  }
  // A key already taken by a bundled/remote dataset — a custom build may not shadow it.
  function keyIsReserved(key) {
    if (RETE_DATASETS_B64[key]) return true;
    const d = CATALOG.datasets.find((x) => x.key === key);
    return !!(d && !d.custom);
  }

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
  // Lightweight inline markdown → HTML (safe: escapes first, then adds a small
  // set of tags). Handles links, bold, code, italic, and — importantly for the
  // dataset descriptions — newlines, so a multi-paragraph description reads as
  // paragraphs instead of a wall of text. Returns inline HTML (no block tags),
  // so it's safe to drop inside a <p>/<div>.
  function mdLite(s) {
    return esc(String(s || ""))
      .replace(/\[([^\]]+)\]\((https?:\/\/[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
      .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
      .replace(/`([^`]+)`/g, "<code>$1</code>")
      .replace(/(^|[^*])\*([^*]+)\*(?!\*)/g, "$1<em>$2</em>")
      .replace(/\n{2,}/g, "<br><br>")
      .replace(/\n/g, "<br>");
  }

  function dsShortLabel(key) {
    const d = datasetInfo(key);
    if (!d) return key;
    // The short name is the first segment before a separator — a hyphen or
    // em/en-dash surrounded by spaces, or an opening parenthesis. (Some labels
    // use " — " which the old " - " split missed, so the whole long label leaked
    // through as the "short" name.)
    return d.label.split(/\s[—–-]\s|\s\(/)[0].trim();
  }

  // The "Datasets" browser: a sidebar list (left) + a detail/preview pane
  // (right). The selected dataset shows tags, the example kinds it supports, a
  // 3-mode source switch (bundled / cache / lazy), its metadata under "more",
  // and an example preview.
  let dsSelected = null;

  // Parse a human .rete size ("4.8 MB", "4.56 GB", "120 MB") back to bytes, so the
  // size tag can be coloured on a green→red weight ramp (heavier file = warmer tag).
  function sizeToBytes(s) {
    const m = /([\d.]+)\s*(B|KB|MB|GB|TB)/i.exec(String(s || ""));
    if (!m) return 0;
    const mult = { B: 1, KB: 1e3, MB: 1e6, GB: 1e9, TB: 1e12 }[m[2].toUpperCase()] || 1;
    return parseFloat(m[1]) * mult;
  }
  function sizeTier(s) {
    const b = sizeToBytes(s);
    if (!b) return "";
    if (b < 10e6) return "sz-xs";   // < 10 MB   — instant
    if (b < 100e6) return "sz-sm";  // 10–100 MB
    if (b < 1e9) return "sz-md";    // 100 MB–1 GB
    if (b < 5e9) return "sz-lg";    // 1–5 GB
    return "sz-xl";                 // ≥ 5 GB    — heavy
  }

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
      // The ⚡ shortcut range-queries the remote .rete straight away — every catalog
      // (non-custom) dataset lives in the bucket, so lazy mode always applies there.
      const lazyBtn = isCustom(d.key) ? "" :
        `<span class="ds-side-load" role="button" tabindex="0" data-lazy="${esc(d.key)}" title="Load now in lazy mode — range-query the remote .rete (only the bytes each query touches are fetched)">📥</span>`;
      return `<button type="button" class="ds-side-item${active ? " active" : ""}" data-ds="${esc(d.key)}">` +
        `<span class="ds-side-ico">${esc(ex.icon || "📊")}</span>` +
        `<span class="ds-side-name">${esc(dsShortLabel(d.key))}</span>` +
        `<span class="ds-side-size ${sizeTier(size)}${remote ? " remote" : ""}" title="${remote ? "remote-only · " : ""}.rete size">${remote ? "🛰 " : ""}${esc(size)}</span>` +
        lazyBtn +
        `</button>`;
    }).join("");
    $$("#dsSidebar .ds-side-item").forEach((b) => {
      b.onclick = () => { dsSelected = b.dataset.ds; renderDsSidebar(); renderDsDetail(dsSelected); };
    });
    // The ⚡ lazy shortcut loads that dataset over HTTP range immediately, without
    // first opening the detail pane + Load menu. Stop the click so the row's own
    // select handler (which would just preview it) doesn't also fire.
    $$("#dsSidebar .ds-side-load").forEach((el) => {
      const go = (e) => { e.stopPropagation(); e.preventDefault(); selectDatasetMode(el.dataset.lazy, "lazy"); closeSource(); };
      el.onclick = go;
      el.onkeydown = (e) => { if (e.key === "Enter" || e.key === " ") go(e); };
    });
  }

  function renderDsDetail(key) {
    const d = datasetInfo(key);
    const m = (CATALOG.datasetMeta && CATALOG.datasetMeta[key]) || {};
    const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[key]) || {};
    const remoteOnly = d.kind === "remote-lazy";
    const embedded = isEmbedded(key);
    const custom = isCustom(key);
    const sup = datasetSupports(key);
    const fmtTri = (t) => (t == null ? "—" : typeof t === "number" ? t.toLocaleString() : esc(t));
    const host = (u) => { try { return new URL(u).host.replace(/^www\./, ""); } catch (e) { return u; } };

    const badge = custom
      ? `<span class="ds-badge local">💾 Local · in this browser</span>`
      : remoteOnly
      ? `<span class="ds-badge remote">🛰 Remote-only · lazy</span>`
      : `<span class="ds-badge bundled">Bundled in page</span>`;
    // Descriptive tags + capability chips (a distinct colour family) in one row.
    const capChips = ["SPARQL", "SHACL", "Reasoning", "Reach", "Provenance", "Geo"]
      .filter((c) => sup[c])
      .map((c) => `<span class="ds-cap on">${esc(c)}</span>`).join("");
    const tags = (custom ? `<span class="ds-tag custom">local build</span>` : "") +
      (ex.tags || []).map((t) => `<span class="ds-tag">${esc(t)}</span>`).join("") +
      (m.license ? `<span class="ds-tag license">${esc(m.license)}</span>` : "") + capChips;

    const defMode = embedded ? "bundled" : "lazy";
    const hints = {
      bundled: custom
        ? "Loads the copy saved in this browser (IndexedDB) — instant, fully offline."
        : "Loads the copy embedded in this page — instant, fully offline.",
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
      (custom
        ? modeItem("bundled", "Open (local copy)", false)
        : modeItem("bundled", "Bundled", !embedded) +
          modeItem("cache", "Cache remote", false) +
          modeItem("lazy", "Lazy range", false)) +
      `</div></div>` +
      (custom ? `<button type="button" class="ds-delete" id="dsDeleteBtn" title="Remove this dataset from your browser">Delete</button>` : "");

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
      `<tr><td>Type</td><td>${custom ? "💾 Local · in this browser (IndexedDB)" : remoteOnly ? "🛰 Remote · lazy" : "Bundled"}${(!custom && embedded) ? " · also in bucket" : ""}</td></tr>` +
      `<tr><td>License</td><td>${esc(m.license || "—")}</td></tr>` +
      `<tr><td>Source</td><td>${m.source ? `<a href="${esc(m.source)}" target="_blank" rel="noopener">${esc(host(m.source))} ↗</a>` : "—"}</td></tr>` +
      `<tr><td>Provenance</td><td>${m.provenance ? esc(m.provenance) : "—"}</td></tr>` +
      `<tr><td>Bucket</td><td class="iri">${custom ? "— (local; export to publish)" : "playground/" + esc(key) + ".rete"}</td></tr>` +
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
    const delBtn = $("dsDeleteBtn");
    if (delBtn) delBtn.onclick = () => {
      if (!confirm(`Delete "${dsShortLabel(key)}" from this browser? Its .rete, card and examples are removed locally (export first if you want to keep them).`)) return;
      deleteUserDataset(key);
    };
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

  // A speed badge for an example, from the offline benchmark (CATALOG.perf =
  // median local query time in ms). Remote-lazy examples have no precomputed
  // time — it depends on the network and shows live when you run them.
  function perfBadge(dataset, label) {
    const ms = CATALOG.perf && CATALOG.perf[dataset] && CATALOG.perf[dataset][label];
    if (ms == null) {
      const d = datasetInfo(dataset);
      return (d && d.kind === "remote-lazy")
        ? `<span class="perf-badge lazy" title="Remote-lazy: query time depends on the network and is shown live when you run it.">🛰 lazy</span>`
        : "";
    }
    let tier = "instant";
    if (ms >= 60) tier = "heavy"; else if (ms >= 16) tier = "moderate"; else if (ms >= 4) tier = "fast";
    return `<span class="perf-badge ${tier}" title="Median local query time, benchmarked over 5 runs: ${ms} ms">${ms} ms</span>`;
  }

  // The result columns a query produces — shown in the example list and the
  // active-example strip so the output shape reads before running. Derived from
  // the query text (always correct, no catalog upkeep): the SELECT projection
  // (skipping vars *inside* an aggregate/expression, keeping its `AS ?name`),
  // `subject predicate object` for CONSTRUCT/DESCRIBE, `boolean` for ASK, or all
  // in-scope vars for SELECT *.
  function projectionCols(proj) {
    const cols = [];
    let i = 0;
    while (i < proj.length) {
      const c = proj[i];
      if (c === "(") {
        // A `(expr AS ?name)` group contributes only its alias — skip the inner
        // vars (e.g. ?p in `(COUNT(?p) AS ?people)`).
        let depth = 0, j = i;
        for (; j < proj.length; j++) {
          if (proj[j] === "(") depth++;
          else if (proj[j] === ")") { depth--; if (!depth) break; }
        }
        const a = /\bAS\s+[?$](\w+)/i.exec(proj.slice(i, j + 1));
        if (a) cols.push(a[1]);
        i = (j > i ? j : i) + 1;
      } else if (c === "?" || c === "$") {
        const v = /^[?$](\w+)/.exec(proj.slice(i));
        if (v) { cols.push(v[1]); i += v[0].length; } else i++;
      } else i++;
    }
    return cols;
  }
  function queryVars(s) {
    const seen = [], set = new Set();
    let m; const re = /[?$](\w+)/g;
    while ((m = re.exec(s))) if (!set.has(m[1])) { set.add(m[1]); seen.push(m[1]); }
    return seen;
  }
  function queryColumns(q) {
    const body = String(q || "").replace(/#[^\n]*/g, " "); // drop line comments
    const m = /\b(SELECT|CONSTRUCT|ASK|DESCRIBE)\b/i.exec(body);
    if (!m) return { form: "", cols: [] };
    const form = m[1].toLowerCase();
    if (form === "ask") return { form, cols: ["boolean"] };
    if (form === "construct" || form === "describe") return { form, cols: ["subject", "predicate", "object"] };
    const after = body.slice(m.index + m[0].length);
    const brace = after.indexOf("{");
    const proj = brace >= 0 ? after.slice(0, brace) : after; // SELECT … up to the WHERE block
    if (/^\s*(DISTINCT\s+|REDUCED\s+)?\*/i.test(proj)) return { form, cols: queryVars(body).slice(0, 12) };
    return { form, cols: projectionCols(proj) };
  }
  // One-line column list for display (SELECT vars get a leading "?").
  function columnsLabel(q) {
    const { form, cols } = queryColumns(q);
    if (!cols.length) return "";
    return (form === "select" ? cols.map((c) => "?" + c) : cols).join(" ");
  }

  function renderExamples() {
    updateSemanticTab();
    renderFamilyFilters();
    renderQuickExamples();
    const items = filteredExamples();
    if (!items.length) {
      $("examples").innerHTML = `<p class="microcopy">No matching examples for this dataset.</p>`;
      return;
    }
    $("examples").innerHTML = items.map(({ ex, index }) =>
      `<article class="example-card" data-family="${esc(ex.family)}">` +
        `<div class="ex-head">` +
          `<button type="button" class="example-button ${index === state.selectedExample ? "active" : ""}" data-example="${index}">` +
            `<span>${esc(ex.label)}</span>${perfBadge(state.dataset, ex.label)}` +
          `</button>` +
          `<button type="button" class="ex-copy" data-copy="${index}" title="Copy a short share link to this example" aria-label="Copy link to this example">🔗</button>` +
        `</div>` +
        `<div class="tagline">${esc(ex.family)} | ${esc(ex.tip)}</div>` +
        (columnsLabel(ex.q) ? `<div class="ex-cols">Columns: <code>${esc(columnsLabel(ex.q))}</code></div>` : "") +
      `</article>`
    ).join("");
    $$("#examples [data-example]").forEach((btn) => {
      btn.onclick = () => selectExample(Number(btn.dataset.example));
    });
    // Copy a short, index-based share link (#dataset=…&ex=N) for any example.
    $$("#examples [data-copy]").forEach((btn) => {
      btn.onclick = (e) => {
        e.stopPropagation();
        const i = Number(btn.dataset.copy);
        const url = location.origin + location.pathname + "#dataset=" +
          encodeURIComponent(state.dataset) + "&ex=" + i;
        const ok = () => {
          btn.textContent = "✓"; btn.title = "Copied!";
          setTimeout(() => { btn.textContent = "🔗"; btn.title = "Copy a short share link to this example"; }, 1200);
        };
        (navigator.clipboard ? navigator.clipboard.writeText(url) : Promise.reject())
          .then(ok).catch(() => { const qm = $("qmeta"); if (qm) qm.textContent = "Share link: " + url; });
      };
    });
  }

  // Clear the previous query's results so a freshly-picked example doesn't show
  // stale output. Called when an example is selected.
  function clearResults() {
    const out = $("out");
    if (out) out.innerHTML = `<p class="microcopy" style="padding:6px 2px">Press <b>Run Query</b> to evaluate this example.</p>`;
    const qm = $("qmeta"); if (qm) qm.textContent = "";
    const co = $("commOut"); if (co) co.innerHTML = "";
    const rb = $("reqLogBtn"); if (rb) rb.classList.add("hidden");
    state.lastResult = null;
  }

  function selectExample(index) {
    const ex = examplesForDataset()[index];
    if (!ex) return;
    state.selectedExample = index;
    state.colLabels = ex.cols || null;   // per-example friendly column headers
    setEd("q", ex.q);
    setView(ex.view || "table");
    setStrategy(ex.strategy || "whole");
    setMode("sparql");
    clearResults();
    // An example may declare federation partners (catalog keys) — a one-click
    // multi-source demo. Reset to just this dataset, then add each partner
    // (embedded → in-memory, remote-lazy → range-read).
    resetFed();
    if (Array.isArray(ex.fed) && ex.fed.length) {
      ex.fed.forEach((k) => {
        // An entry can be a dataset key, or {endpoint, label} for a live SPARQL endpoint.
        if (k && typeof k === "object" && k.endpoint) {
          state.fedSources.push({ id: "f" + (++fedSeq), kind: "endpoint", label: k.label || shortUrlLabel(k.endpoint), endpoint: k.endpoint });
          return;
        }
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
      perfBadge(state.dataset, sel.label) +
      (sel.tip ? `<span class="exd-tip">${esc(sel.tip)}</span>` : "") +
      (columnsLabel(sel.q) ? `<span class="exd-cols" title="The result columns this query returns">Columns: <code>${esc(columnsLabel(sel.q))}</code></span>` : "");
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
  // Asyncify concurrent reads are always on now (see state.asyncReadsOn above) and
  // no longer have a Settings toggle — ensureRemoteWorker() reads the flag directly.

  // ── SPARQL assistant: an in-browser WebGPU LLM (Transformers.js) ───────────
  // "✨ Ask AI" opens a chat that runs a small instruction model ENTIRELY in the
  // browser (WebGPU, no server) to draft SPARQL for the current dataset, grounded
  // in its description + real example queries. Weights download once from the HF
  // Hub and are cached by the browser; nothing leaves the machine.
  const AI_DEFAULT_MODEL = "onnx-community/gemma-3-1b-it-ONNX";
  function aiModelId() { try { return localStorage.getItem("aiModelId") || AI_DEFAULT_MODEL; } catch (e) { return AI_DEFAULT_MODEL; } }
  // Gemma 4 E2B (mobile) isn't an ONNX model: it runs through the webml-community
  // custom WebGPU kernels (the `Gemma4Mobile` class, on the main thread), not the
  // standard Transformers.js worker. Pick it by setting this id in Settings.
  const AI_GEMMA4 = "google/gemma-4-E2B-it-qat-mobile-transformers";
  const GEMMA4_URL = "https://webml-community-gemma-4-webgpu-kernels.static.hf.space/gemma-4-e2b.js";
  function aiIsGemma4() { return aiModelId() === AI_GEMMA4; }
  let aiGemma4 = null, aiGemma4Abort = null;
  // A module worker that imports Transformers.js from the CDN, loads the model on
  // WebGPU, and streams generated tokens back. Built as a blob so the page stays
  // self-contained; the heavy library + weights load only when the chat is opened.
  const LLM_WORKER_SRC =
    'import { AutoTokenizer, AutoModelForCausalLM, TextStreamer, env } from "https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.7.6";\n' +
    'env.allowLocalModels = false;\n' +
    // Import succeeded — tell the main thread the runtime is up (so a hang BEFORE
    // this point is unambiguously a failed/slow CDN import, caught by worker.onerror).
    'self.postMessage({ type: "booted" });\n' +
    'let tok = null, model = null, curId = null;\n' +
    'async function load(id) {\n' +
    '  if (model && curId === id) { self.postMessage({ type: "ready" }); return; }\n' +
    '  tok = null; model = null;\n' +
    '  const pc = (p) => self.postMessage({ type: "progress", data: p });\n' +
    '  self.postMessage({ type: "stage", stage: "tokenizer" });\n' +
    '  tok = await AutoTokenizer.from_pretrained(id, { progress_callback: pc });\n' +
    '  self.postMessage({ type: "stage", stage: "model" });\n' +
    '  model = await AutoModelForCausalLM.from_pretrained(id, { dtype: "q4f16", device: "webgpu", progress_callback: pc });\n' +
    '  curId = id;\n' +
    '  self.postMessage({ type: "ready" });\n' +
    '}\n' +
    'async function generate(messages) {\n' +
    '  const inputs = tok.apply_chat_template(messages, { add_generation_prompt: true, return_dict: true });\n' +
    '  const streamer = new TextStreamer(tok, { skip_prompt: true, skip_special_tokens: true,\n' +
    '    callback_function: (t) => self.postMessage({ type: "token", text: t }) });\n' +
    '  await model.generate({ ...inputs, max_new_tokens: 768, do_sample: false, repetition_penalty: 1.1, streamer });\n' +
    '  self.postMessage({ type: "done" });\n' +
    '}\n' +
    'self.onmessage = async (e) => {\n' +
    '  const m = e.data;\n' +
    '  try { if (m.type === "load") await load(m.id); else if (m.type === "generate") await generate(m.messages); }\n' +
    '  catch (err) { self.postMessage({ type: "error", error: String((err && err.message) || err) }); }\n' +
    '};\n';

  let llmWorker = null, llmLoaded = false, llmModelId = null, llmBusy = false;
  let aiHistory = [];           // [{role, content}] chat turns (excl. the system prompt)
  let aiOnToken = null, aiOnDone = null, aiOnError = null, aiOnProgress = null;
  let aiOnStage = null, aiOnLog = null, aiOnBooted = null;

  function ensureLlmWorker() {
    if (llmWorker) return llmWorker;
    llmWorker = new Worker(URL.createObjectURL(new Blob([LLM_WORKER_SRC], { type: "text/javascript" })), { type: "module" });
    llmWorker.onmessage = (e) => {
      const m = e.data;
      if (m.type === "progress") { if (aiOnProgress) aiOnProgress(m.data); }
      else if (m.type === "booted") { if (aiOnBooted) aiOnBooted(); }
      else if (m.type === "stage") { if (aiOnStage) aiOnStage(m.stage); }
      else if (m.type === "log") { if (aiOnLog) aiOnLog(m.text); }
      else if (m.type === "ready") { llmLoaded = true; if (aiOnDone && aiOnDone.__load) { const f = aiOnDone; aiOnDone = null; f(); } }
      else if (m.type === "token") { if (aiOnToken) aiOnToken(m.text); }
      else if (m.type === "done") { llmBusy = false; if (aiOnDone) { const f = aiOnDone; aiOnDone = null; f(); } }
      else if (m.type === "error") { llmBusy = false; if (aiOnError) aiOnError(m.error); }
    };
    // A module-worker import failure (CDN blocked, offline, bad URL) throws at the
    // TOP of the worker and is NOT caught by the try/catch inside onmessage — without
    // this the load hangs forever with no signal. Surface it as a load error instead.
    llmWorker.onerror = (e) => {
      llmBusy = false;
      const msg = (e && e.message) ||
        "worker failed to start — couldn't import transformers.js (offline, or the CDN is blocked).";
      if (aiOnError) aiOnError(msg);
    };
    llmWorker.onmessageerror = () => { if (aiOnError) aiOnError("worker message error (structured-clone failed)"); };
    return llmWorker;
  }

  // The system prompt: ground the model in THIS dataset's description, predicates
  // and (crucially) its real example queries, so it reuses the right prefixes.
  function aiSystemPrompt() {
    const key = state.dataset;
    const rec = (CATALOG.datasets || []).find((d) => d.key === key) || {};
    const exs = (CATALOG.examples[key] || []).slice(0, 6);
    const preds = [];
    try {
      ((state.schema && state.schema.relations) || []).forEach((r) => { if (r[1] && preds.indexOf(r[1]) < 0) preds.push(r[1]); });
    } catch (e) { /* no schema */ }
    let sys = 'You are an expert at writing SPARQL. Help the user query the RDF dataset "' + (rec.label || key) +
      '" in a browser SPARQL playground (the rete engine).\n\n';
    if (rec.description) sys += "About the dataset: " + rec.description.slice(0, 1400) + "\n\n";
    if (preds.length) sys += "Some predicates in this dataset:\n" + preds.slice(0, 40).join("\n") + "\n\n";
    if (exs.length) sys += "Example queries that WORK on this dataset — reuse their PREFIX lines and predicates:\n\n" +
      exs.map((e) => "# " + e.label + "\n" + (e.q || "").trim()).join("\n\n") + "\n\n";
    sys += 'When the user asks something, reply with exactly ONE SPARQL query inside a ```sparql code block, valid for THIS dataset and reusing its prefixes/predicates, then one short sentence explaining it. Do NOT invent predicates or prefixes that are not shown above. Prefer SELECT with a small LIMIT unless asked otherwise.';
    return sys;
  }
  function aiExtractSparql(text) {
    const m = /```(?:sparql|sql)?\s*([\s\S]*?)```/i.exec(text);
    if (m && m[1].trim()) return m[1].trim();
    const t = (text || "").trim();
    if (/^(PREFIX|SELECT|ASK|CONSTRUCT|DESCRIBE)\b/i.test(t)) return t;
    return null;
  }

  // ---- the chat modal ----
  let aiModalEl = null;
  function ensureAiModal() {
    if (aiModalEl) return aiModalEl;
    const el = document.createElement("div");
    el.className = "ai-modal hidden";
    el.innerHTML =
      '<div class="ai-modal-backdrop"></div>' +
      '<div class="ai-modal-box" role="dialog" aria-modal="true" aria-label="SPARQL AI assistant">' +
        '<div class="ai-modal-head"><span class="ai-modal-title">✨ SPARQL assistant</span>' +
          '<button class="ai-modal-close" type="button" aria-label="close">×</button></div>' +
        '<div class="ai-gate"></div>' +
        '<div class="ai-transcript"></div>' +
        '<form class="ai-input-row"><textarea class="ai-input" rows="2" placeholder="Ask in plain language — e.g. “the 10 most common predicates” or “manuscripts made in Ireland with an image”"></textarea>' +
          '<button class="ai-send" type="submit">Send</button></form>' +
      '</div>';
    document.body.appendChild(el);
    const close = () => el.classList.add("hidden");
    el.querySelector(".ai-modal-close").addEventListener("click", close);
    el.querySelector(".ai-modal-backdrop").addEventListener("click", close);
    document.addEventListener("keydown", (e) => { if (!el.classList.contains("hidden") && e.key === "Escape") close(); });
    el.querySelector(".ai-input-row").addEventListener("submit", (e) => { e.preventDefault(); aiSend(); });
    el.querySelector(".ai-input").addEventListener("keydown", (e) => {
      if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); aiSend(); }
    });
    aiModalEl = el;
    return el;
  }
  function aiAddMsg(role, html) {
    const t = aiModalEl.querySelector(".ai-transcript");
    const d = document.createElement("div");
    d.className = "ai-msg ai-" + role;
    d.innerHTML = html;
    t.appendChild(d);
    t.scrollTop = t.scrollHeight;
    return d;
  }
  function aiRenderGate() {
    const gate = aiModalEl.querySelector(".ai-gate");
    if (!navigator.gpu) {
      gate.innerHTML = '<p class="ai-warn">This assistant needs <b>WebGPU</b> — open the playground in Chrome or Edge (desktop) to use it.</p>';
      aiModalEl.querySelector(".ai-input-row").style.display = "none";
      return;
    }
    if (llmLoaded && llmModelId === aiModelId()) { gate.innerHTML = ""; aiModalEl.querySelector(".ai-input-row").style.display = ""; return; }
    aiModalEl.querySelector(".ai-input-row").style.display = "none";
    gate.innerHTML = '<p class="ai-gate-note">Runs <code>' + esc(aiModelId()) + '</code> entirely in your browser via WebGPU. The weights download once from the Hugging Face Hub (a few hundred MB to ~1 GB) and are cached — nothing is sent to a server.</p>' +
      '<button type="button" class="ai-load-btn">Load model</button><div class="ai-progress"></div>';
    gate.querySelector(".ai-load-btn").addEventListener("click", aiLoadModel);
  }
  function aiLoadModel() {
    const gate = aiModalEl.querySelector(".ai-gate");
    const prog = gate.querySelector(".ai-progress");
    gate.querySelector(".ai-load-btn").disabled = true;
    prog.innerHTML =
      '<div class="ai-stage">Starting the WebGPU runtime…</div>' +
      '<div class="ai-bars"></div>' +
      '<details class="ai-dl-log" open><summary>Download log</summary><div class="ai-log-lines"></div></details>';
    const stageEl = prog.querySelector(".ai-stage");
    const barsEl = prog.querySelector(".ai-bars");
    const logEl = prog.querySelector(".ai-log-lines");
    const bars = {}, seen = {};
    let stage = "init";
    const t0 = (typeof performance !== "undefined" ? performance.now() : 0);
    const elapsed = () => ((((typeof performance !== "undefined" ? performance.now() : 0) - t0)) / 1000).toFixed(0) + "s";
    const fmtMB = (b) => (b / 1048576).toFixed(1) + " MB";
    function log(line) {
      const d = document.createElement("div"); d.className = "ai-log-line";
      d.textContent = "[" + elapsed() + "] " + line;
      logEl.appendChild(d); logEl.scrollTop = logEl.scrollHeight;
    }
    function barFor(file) {
      if (bars[file]) return bars[file];
      const row = document.createElement("div"); row.className = "ai-bar-row";
      row.innerHTML = '<span class="ai-bar-name" title="' + esc(file) + '">' + esc(file) + '</span>' +
        '<span class="ai-bar-track"><span class="ai-bar-fill"></span></span>' +
        '<span class="ai-bar-pct">0%</span>';
      barsEl.appendChild(row);
      const b = { fill: row.querySelector(".ai-bar-fill"), pct: row.querySelector(".ai-bar-pct"), done: false };
      bars[file] = b; return b;
    }
    const allBarsDone = () => { const v = Object.values(bars); return v.length > 0 && v.every((x) => x.done); };

    // Gemma 4 path: import the custom-kernel runtime + run it on the main thread.
    if (aiIsGemma4()) {
      stageEl.textContent = "Loading the Gemma 4 WebGPU runtime (custom kernels)…";
      log("• importing Gemma4Mobile from the webml-community Space…");
      const onP = (p) => {
        const b = barFor("gemma-4-e2b");
        let pct = 0, label = "";
        if (typeof p === "number") pct = p <= 1 ? p * 100 : p;
        else if (p && typeof p === "object") {
          if (typeof p.progress === "number") pct = p.progress <= 1 ? p.progress * 100 : p.progress;
          else if (p.total) pct = ((p.loaded || 0) / p.total) * 100;
          label = p.text || p.status || p.file || p.name || "";
        }
        b.fill.style.width = Math.max(0, Math.min(100, pct)) + "%";
        b.pct.textContent = pct.toFixed(0) + "%";
        if (label && !seen[label]) { seen[label] = 1; stageEl.textContent = String(label); log("• " + label); }
      };
      import(/* @vite-ignore */ GEMMA4_URL)
        .then((mod) => {
          log("✓ Gemma4Mobile runtime loaded");
          stageEl.innerHTML = 'Downloading + compiling Gemma 4 E2B on WebGPU <span class="ai-spin">◴</span>';
          return mod.Gemma4Mobile.load(null, { onProgress: onP });
        })
        .then((model) => {
          aiGemma4 = model; llmLoaded = true;
          log("✓ Gemma 4 ready in " + elapsed());
          llmModelId = aiModelId(); aiRenderGate(); aiModalEl.querySelector(".ai-input").focus();
        })
        .catch((err) => {
          const m = String((err && err.message) || err);
          stageEl.innerHTML = '<span class="ai-warn">Failed to load Gemma 4: ' + esc(m) +
            '</span> <span class="ai-dim">— needs WebGPU (Chrome/Edge desktop); the custom kernels come from the webml-community Space.</span>';
          log("✗ " + m); gate.querySelector(".ai-load-btn").disabled = false;
        });
      return;
    }

    aiOnLog = (text) => log(text);
    aiOnBooted = () => log("✓ transformers.js runtime loaded");
    aiOnStage = (s) => {
      stage = s;
      stageEl.textContent = s === "tokenizer" ? "Downloading tokenizer…"
        : s === "model" ? "Downloading model weights (a few hundred MB to ~1 GB)…" : String(s);
      log("• " + stageEl.textContent);
    };
    aiOnProgress = (p) => {
      if (!p || !p.file) return;
      const file = p.file;
      if ((p.status === "initiate" || p.status === "download") && !seen[file]) {
        seen[file] = true; barFor(file); log("↓ " + file + (p.total ? " (" + fmtMB(p.total) + ")" : ""));
      } else if (p.status === "progress") {
        const b = barFor(file);
        const pct = typeof p.progress === "number" ? p.progress : (p.total ? (p.loaded / p.total) * 100 : 0);
        b.fill.style.width = Math.max(0, Math.min(100, pct)) + "%";
        b.pct.textContent = (p.total ? fmtMB(p.loaded || 0) + " / " + fmtMB(p.total) + "  " : "") + pct.toFixed(0) + "%";
      } else if (p.status === "done") {
        const b = barFor(file); b.fill.style.width = "100%"; b.pct.textContent = "✓"; b.done = true;
        log("✓ " + file);
        // After the last weight file downloads there's a silent WebGPU compile — say so
        // instead of looking frozen.
        if (stage === "model" && allBarsDone()) {
          stageEl.innerHTML = 'Compiling the model on WebGPU <span class="ai-spin">◴</span> ' +
            '<span class="ai-dim">— can take 10–60 s, no further downloads</span>';
          log("• all files downloaded — compiling on WebGPU…");
        }
      }
    };
    aiOnError = (err) => {
      stageEl.innerHTML = '<span class="ai-warn">Failed to load: ' + esc(err) +
        '</span> <span class="ai-dim">— try another model id in Settings, or check your network / WebGPU support.</span>';
      log("✗ " + err);
      gate.querySelector(".ai-load-btn").disabled = false;
    };
    const done = () => {
      log("✓ model ready in " + elapsed());
      llmModelId = aiModelId(); aiRenderGate(); aiModalEl.querySelector(".ai-input").focus();
    };
    done.__load = true; aiOnDone = done;
    log("• starting worker, importing transformers.js from the CDN…");
    ensureLlmWorker().postMessage({ type: "load", id: aiModelId() });
  }
  // Finalize a bot bubble: render the text, and if it contains a SPARQL block add a
  // "Use & run" button. Shared by the worker (TJS) and the Gemma 4 main-thread path.
  function aiFinishBubble(bubble, acc) {
    const sparql = aiExtractSparql(acc);
    let html = esc(acc);
    if (sparql) html += '<div class="ai-run-row"><button type="button" class="ai-run-btn">▶ Use &amp; run this query</button></div>';
    bubble.innerHTML = html;
    if (sparql) bubble.querySelector(".ai-run-btn").addEventListener("click", () => {
      if (window.PlaygroundEditor) window.PlaygroundEditor.setText("q", sparql);
      const ta = $("q"); if (ta) ta.value = sparql;
      aiModalEl.classList.add("hidden");
      try { runQuery(); } catch (e) { /* surfaced in the result pane */ }
    });
  }
  function aiSend() {
    if (llmBusy || !llmLoaded) return;
    const input = aiModalEl.querySelector(".ai-input");
    const q = input.value.trim();
    if (!q) return;
    input.value = "";
    aiAddMsg("user", esc(q));
    aiHistory.push({ role: "user", content: q });
    const bubble = aiAddMsg("bot", '<span class="ai-cursor">▌</span>');
    let acc = "";
    llmBusy = true;
    aiOnToken = (tok) => { acc += tok; bubble.innerHTML = esc(acc) + '<span class="ai-cursor">▌</span>'; aiModalEl.querySelector(".ai-transcript").scrollTop = 1e9; };
    aiOnError = (err) => { bubble.innerHTML = '<span class="ai-warn">' + esc(err) + '</span>'; llmBusy = false; };
    aiOnDone = () => { aiHistory.push({ role: "assistant", content: acc }); aiFinishBubble(bubble, acc); };
    // Prime via a user/assistant pair rather than a `system` role — some chat
    // templates (notably Gemma's) reject a system message; user/assistant is
    // accepted everywhere.
    const messages = [
      { role: "user", content: aiSystemPrompt() },
      { role: "assistant", content: "Understood — I'll answer each question with a single SPARQL query in a ```sparql code block, using only this dataset's prefixes and predicates." }
    ].concat(aiHistory.slice(-6));
    // Gemma 4 runs on the main thread (custom kernels) and streams an async iterable
    // of cumulative {text}; the TJS models stream tokens from the worker.
    if (aiIsGemma4() && aiGemma4) {
      aiGemma4Abort = (typeof AbortController !== "undefined") ? new AbortController() : null;
      (async () => {
        try {
          const stream = aiGemma4.generate(messages, { maxNewTokens: 1024, signal: aiGemma4Abort && aiGemma4Abort.signal });
          for await (const chunk of stream) {
            acc = (chunk && chunk.text != null) ? chunk.text : (typeof chunk === "string" ? chunk : acc);
            bubble.innerHTML = esc(acc) + '<span class="ai-cursor">▌</span>';
            aiModalEl.querySelector(".ai-transcript").scrollTop = 1e9;
          }
          aiHistory.push({ role: "assistant", content: acc }); aiFinishBubble(bubble, acc); llmBusy = false;
        } catch (err) {
          bubble.innerHTML = '<span class="ai-warn">' + esc(String((err && err.message) || err)) + '</span>'; llmBusy = false;
        }
      })();
      return;
    }
    ensureLlmWorker().postMessage({ type: "generate", messages });
  }
  function openAiModal() {
    ensureAiModal();
    aiModalEl.querySelector(".ai-modal-title").textContent = "✨ SPARQL assistant — " + (state.dataset || "");
    aiRenderGate();
    aiModalEl.classList.remove("hidden");
    if (llmLoaded) aiModalEl.querySelector(".ai-input").focus();
  }
  // Live refresh of the range-cache panel while Settings is open: the numbers grow
  // as a running query's worker writes fetched blocks to IndexedDB; a spindle shows
  // for ~1.5 s after each growth, so you watch the cache fill in real time.
  let rcLiveTimer = null, rcPrevTotal = -1, rcPrevCount = -1, rcLastGrowAt = 0;
  async function renderRangeCache() {
    const t = $("rangeCacheToggle"); if (t) t.checked = !!state.rangeCacheOn;
    const info = $("rangeCacheInfo");
    if (info) info.textContent = state.rangeCacheOn
      ? "On — fetched byte ranges (rete, DuckDB and SQLite) are saved to IndexedDB and reused after a reload. They persist across browser sessions until you clear them. Toggling recreates the query engines."
      : "Off — the lazy backends keep fetched bytes only for this session; a reload re-fetches. Turn on to persist ranges across reloads and sessions (experimental).";
    const items = await rangeCacheBreakdown();
    // Cap each file at its own size (block rounding can overshoot on tiny files).
    const totalBytes = items.reduce((a, m) => a + Math.min(m.bytes || 0, m.total || (m.bytes || 0)), 0);
    // Live: spindle for ~1.5 s after any growth so you SEE bytes landing.
    if (rcPrevTotal >= 0 && totalBytes > rcPrevTotal) rcLastGrowAt = performance.now();
    const changed = totalBytes !== rcPrevTotal || items.length !== rcPrevCount;
    rcPrevTotal = totalBytes; rcPrevCount = items.length;
    const live = performance.now() - rcLastGrowAt < 1500;
    const sz = $("rangeCacheSize");
    if (sz) sz.innerHTML = "Range cache: " + esc(formatBytes(totalBytes)) +
      (items.length ? " · " + items.length + " file(s)" : "") +
      (live ? ` <span class="spindle" aria-hidden="true"></span><span class="rc-caching">caching…</span>` : "");
    const list = $("rangeCacheList");
    if (!list) return;
    // Idle tick — nothing grew: leave the rows (and their Clear handlers) untouched.
    if (!changed && list.dataset.ready === "1") return;
    if (!items.length) {
      list.dataset.ready = "0";
      list.innerHTML = `<p class="cache-empty">${state.rangeCacheOn
        ? "No ranges cached yet — query a remote-lazy dataset and the byte ranges it touches are saved here, one row per file with how much of it you hold."
        : "Turn this on, then query a remote-lazy dataset: the ranges it fetches will be listed here per file, with the share of each file cached."}</p>`;
      return;
    }
    list.dataset.ready = "1";
    items.sort((a, b) => b.bytes - a.bytes);
    list.innerHTML = items.map((m) => {
      const pct = m.total ? Math.min(100, (m.bytes / m.total) * 100) : 0;
      const pctTxt = m.total ? (pct >= 9.95 ? pct.toFixed(0) : pct.toFixed(1)) + "%" : "size unknown";
      // Cap the shown cached bytes at the file size — the tally rounds each fetch
      // up to whole 1 MiB blocks, so a small file can accrue slightly more block
      // bytes than it contains (e.g. "2 MB / 1.5 MB"); clamp so it reads sanely.
      const held = m.total ? Math.min(m.bytes, m.total) : m.bytes;
      const size = formatBytes(held) + (m.total ? " / " + formatBytes(m.total) : "") + " · " + pctTxt;
      return `<div class="rc-brow"><div class="rc-bcol">` +
        `<div class="rc-bhead"><span class="ci-name" title="${esc(m.key)}">${esc(rcLabelForKey(m.key))}</span>` +
        `<span class="ci-size">${esc(size)}</span></div>` +
        `<div class="rc-bar"><span style="width:${pct.toFixed(1)}%"></span></div></div>` +
        `<button type="button" class="secondary" data-rcdel="${esc(m.key)}">Clear</button></div>`;
    }).join("");
    list.querySelectorAll("[data-rcdel]").forEach((b) => b.onclick = async () => {
      await clearRangeCacheKey(b.dataset.rcdel);
      renderRangeCache();
    });
  }
  // ?workers=N override for the fetch-worker pool (1..32; null = auto from cores).
  function parallelWorkerCount() {
    try { const v = new URL(location.href).searchParams.get("workers"); const n = parseInt(v, 10); return isNaN(n) ? null : Math.max(1, Math.min(32, n)); }
    catch (e) { return null; }
  }
  function setParallelParam(on) {
    try { sessionStorage.removeItem("coiReloaded"); } catch (e) { /* private mode */ }  // fresh attempt
    const u = new URL(location.href);
    if (on) u.searchParams.set("parallel", "1"); else u.searchParams.delete("parallel");
    location.href = u.toString();  // reload — the head script (un)registers the COI SW
  }
  function setParallelWorkers(n) {
    const u = new URL(location.href);
    if (n) u.searchParams.set("workers", String(n)); else u.searchParams.delete("workers");
    location.href = u.toString();
  }
  function renderParallel() {
    const iso = !!window.crossOriginIsolated;
    const wanted = /[?&]parallel(=1)?\b/.test(location.search);
    const t = $("parallelToggle"); if (t) t.checked = iso;
    const w = $("parallelWorkers"); if (w) { w.value = parallelWorkerCount() || ""; w.disabled = !iso; }
    const info = $("parallelInfo");
    if (!info) return;
    if (iso) {
      info.innerHTML = `<b>On</b> — each query fetches its byte ranges in parallel across ${parallelWorkerCount() || "auto"} fetch workers (cross-origin isolated). A big speedup for remote SPARQL on the 1 GB / lazy datasets; the CDN-loaded DuckDB/SQLite Explore backends may be limited in this mode.`;
    } else if (wanted) {
      // ?parallel=1 is set but isolation didn't engage — explain instead of a silent dead toggle.
      info.innerHTML = `<b>Couldn't enable.</b> The page isn't cross-origin isolated, so the checkbox won't stay on. Most likely your browser doesn't support COEP <code>credentialless</code> (<b>Safari</b> doesn't) — try <b>Chrome, Edge, or Firefox</b>. Reads stay sequential meanwhile. (If you're on a supported browser, a hard-reload usually completes the handshake.)`;
    } else {
      info.textContent = "Off — reads are sequential (one coalesced multi-range request at a time). Turn on to fetch each query's byte ranges in parallel: the page reloads into cross-origin isolation. Graph/SPARQL only — it may limit the CDN-loaded DuckDB/SQLite Explore backends.";
    }
  }
  function openSettings() {
    rcPrevTotal = -1; rcPrevCount = -1; rcLastGrowAt = 0;
    renderRangeCache(); renderParallel(); renderCacheList();
    $("settingsModal").classList.remove("hidden");
    // Poll while open so the cache size/bars tick up live as a running query caches.
    if (rcLiveTimer) clearInterval(rcLiveTimer);
    rcLiveTimer = setInterval(renderRangeCache, 600);
  }
  function closeSettings() {
    $("settingsModal").classList.add("hidden");
    if (rcLiveTimer) { clearInterval(rcLiveTimer); rcLiveTimer = null; }
  }

  const LIB_KEY = "rete.pg.libCollapsed";
  function setLibCollapsed(collapsed) {
    const shell = document.querySelector(".console-shell");
    if (shell) {
      shell.classList.toggle("lib-collapsed", collapsed);
      // On phones the details panel is a drawer that the mobile stylesheet keeps
      // CLOSED by default; `lib-open` is the explicit "opened" flag it keys off.
      // Toggling it here lets the ‹/› buttons drive the drawer. No effect on
      // desktop — all lib-open rules live inside the ≤860px media query.
      shell.classList.toggle("lib-open", !collapsed);
    }
    try { localStorage.setItem(LIB_KEY, collapsed ? "1" : "0"); } catch (_e) { /* ignore */ }
  }

  // ── Semantic search (RAG over a .rete) ─────────────────────────────────────
  // The "Semantic" tab. A Transformers.js feature-extraction worker (WebGPU, model
  // from the HF Hub — the same no-server pattern as Ask-AI) embeds the query; the
  // dataset's precomputed doc embeddings (a Float32 matrix on the bucket, one row
  // per entity, declared in CATALOG.rag) are cosine-ranked entirely in the browser.
  // Results link into the graph. Verified end-to-end headless in dev/playwright/check_rag.
  const RAG_WORKER_SRC =
    'import { pipeline, env } from "https://cdn.jsdelivr.net/npm/@huggingface/transformers@3.7.6";\n' +
    'env.allowLocalModels = false;\n' +
    'self.postMessage({ type: "booted" });\n' +
    'let ex = null, curId = null;\n' +
    'self.onmessage = async (e) => { const m = e.data; try {\n' +
    '  if (m.type === "load") {\n' +
    '    if (!ex || curId !== m.id) {\n' +
    '      let dev = "webgpu";\n' +
    '      try { ex = await pipeline("feature-extraction", m.id, { device: "webgpu", dtype: "q8" }); }\n' +
    '      catch (_) { dev = "wasm"; ex = await pipeline("feature-extraction", m.id, { device: "wasm", dtype: "q8" }); }\n' +
    // Some WebGPU backends (notably software SwiftShader) load fine but return
    // degenerate vectors — every text ~identical, so ranking is noise. Probe two
    // unrelated strings: healthy backends score ~0.8, degenerate ones >0.94. If it
    // fails (or NaNs), fall back to WASM, which is correct everywhere.
    '      if (dev === "webgpu") {\n' +
    '        try {\n' +
    '          const pa = (await ex("query: dog", { pooling: "mean", normalize: true })).data;\n' +
    '          const pb = (await ex("query: constitutional law", { pooling: "mean", normalize: true })).data;\n' +
    '          let c = 0; for (let i = 0; i < pa.length; i++) c += pa[i] * pb[i];\n' +
    '          if (!(c < 0.9)) ex = await pipeline("feature-extraction", m.id, { device: "wasm", dtype: "q8" });\n' +
    '        } catch (_) { ex = await pipeline("feature-extraction", m.id, { device: "wasm", dtype: "q8" }); }\n' +
    '      }\n' +
    '      curId = m.id;\n' +
    '    }\n' +
    '    self.postMessage({ type: "ready" });\n' +
    '  } else if (m.type === "embed") {\n' +
    '    const r = await ex(m.prefix + m.text, { pooling: "mean", normalize: true });\n' +
    '    self.postMessage({ type: "vec", vec: Array.from(r.data) });\n' +
    '  }\n' +
    '} catch (err) { self.postMessage({ type: "error", error: String((err && err.message) || err) }); } };\n';

  let ragWorker = null, ragOnMsg = null;
  const ragState = { key: null, emb: null, index: null, dim: 0, ready: false, loading: false, bound: false };

  function ensureRagWorker() {
    if (ragWorker) return ragWorker;
    ragWorker = new Worker(URL.createObjectURL(new Blob([RAG_WORKER_SRC], { type: "text/javascript" })), { type: "module" });
    ragWorker.onmessage = (e) => { if (ragOnMsg) ragOnMsg(e.data); };
    ragWorker.onerror = () => { if (ragOnMsg) ragOnMsg({ type: "error", error: "couldn't import transformers.js (offline, or the CDN is blocked)." }); };
    return ragWorker;
  }
  // One in-flight request at a time (load or embed); resolves on the matching reply.
  function ragCall(msg) {
    return new Promise((resolve, reject) => {
      ragOnMsg = (m) => {
        if (m.type === "vec") resolve(m.vec);
        else if (m.type === "ready") resolve(true);
        else if (m.type === "error") reject(new Error(m.error));
      };
      ensureRagWorker().postMessage(msg);
    });
  }
  // Show the Semantic rail tab only for datasets that carry a rag index.
  function updateSemanticTab() {
    const btn = $$('#modeTabs button[data-mode="semantic"]')[0];
    if (btn) btn.classList.toggle("hidden", !((CATALOG.rag || {})[state.dataset]));
  }

  async function ensureSemantic() {
    const out = $("semanticOut"), bar = $("semanticBar");
    if (!ragState.bound) {
      ragState.bound = true;
      $("semanticGo").onclick = runSemantic;
      $("semanticQ").addEventListener("keydown", (e) => { if (e.key === "Enter") runSemantic(); });
      const ab = $("semanticAnswerBtn"); if (ab) ab.onclick = ragAnswer;
      const sb = $("semanticSparqlBtn"); if (sb) sb.onclick = semanticToSparql;
    }
    const rag = (CATALOG.rag || {})[state.dataset];
    if (!rag) { bar.classList.add("hidden"); out.innerHTML = note("This dataset has no semantic index."); return; }
    renderSemanticExamples();
    if (ragState.ready && ragState.key === state.dataset) { bar.classList.remove("hidden"); return; }
    if (ragState.loading) return;
    ragState.loading = true;
    out.innerHTML = `<p class="microcopy">Loading the semantic model + ${(rag.count || 0).toLocaleString()} embeddings…</p>`;
    try {
      const [buf, idx] = await Promise.all([
        fetch(rag.emb).then((r) => r.arrayBuffer()),
        fetch(rag.index).then((r) => r.json()),
      ]);
      ragState.emb = new Float32Array(buf);
      ragState.index = idx;
      ragState.dim = ragState.emb.length / idx.length;
      await ragCall({ type: "load", id: rag.model });
      ragState.key = state.dataset; ragState.ready = true;
      bar.classList.remove("hidden");
      out.innerHTML = note(`Ready — ${idx.length.toLocaleString()} documents indexed (${ragState.dim}-dim, ${rag.model}, WebGPU). Type a natural-language query above.`);
    } catch (e) {
      out.innerHTML = ""; showError("semanticOut", "Semantic search unavailable: " + (e && e.message || e));
    } finally { ragState.loading = false; }
  }

  // Cosine-rank a query vector against the loaded embeddings and render. Also the
  // headless test hook (window.__ragRank) — the model embed is proven separately.
  function ragRank(qv, q) {
    const { emb, index, dim } = ragState;
    if (!emb || !qv) return [];
    const scope = ragScopeSet();                 // hybrid: restrict to the current query's IRIs
    const scored = [];
    for (let i = 0; i < index.length; i++) {
      if (scope && !scope.has(index[i].iri)) continue;
      let s = 0; const o = i * dim;
      for (let d = 0; d < dim; d++) s += qv[d] * emb[o + d];
      scored.push([s, i]);
    }
    scored.sort((a, b) => b[0] - a[0]);
    const rows = scored.slice(0, 20).map(([s, i]) => ({ score: s, iri: index[i].iri, title: index[i].title }));
    ragState.lastQuery = q; ragState.lastHits = rows;
    const scopeNote = scope ? ` <span class="ai-dim">(scoped to ${scope.size.toLocaleString()} IRIs from your query)</span>` : "";
    $("semanticOut").innerHTML = rows.length
      ? `<div class="banner">Top ${rows.length} for &ldquo;${esc(q)}&rdquo; — cosine over ${index.length.toLocaleString()} in-browser embeddings, no server.${scopeNote}</div>` +
        rows.map((r) => `<div style="padding:.4rem 0;border-bottom:1px solid var(--line,#ececec)"><b>${r.score.toFixed(3)}</b> &nbsp; <a href="${esc(r.iri)}" target="_blank" rel="noopener">${esc(r.title || r.iri)}</a></div>`).join("")
      : note("No results" + (scope ? " within the current query's IRIs — uncheck 'scope' or widen the query." : "."));
    const wrap = $("semanticAnswerWrap");
    if (wrap) { wrap.classList.toggle("hidden", !rows.length); const a = $("semanticAnswer"); if (a) a.innerHTML = ""; }
    return rows;
  }

  // Hybrid pre-filter: the set of IRIs bound by the current SPARQL result (any
  // column), used to restrict the vector ranking when "scope" is checked.
  function ragScopeSet() {
    const cb = $("semanticScope");
    if (!cb || !cb.checked) return null;
    const res = state.lastResult;
    const rws = res && (res.rows || res.bindings);
    if (!rws || !rws.length) return null;
    const set = new Set();
    for (const row of rws) for (const k in row) {
      let s = row[k]; if (s && s.value != null) s = s.value;
      if (typeof s !== "string") continue;
      s = s.replace(/^<|>$/g, "");               // strip N-Triples IRI brackets if present
      if (/^https?:\/\//.test(s)) set.add(s);
    }
    return set.size ? set : null;
  }

  // "Answer with AI": feed the top hits + query to the existing Gemma worker for a
  // grounded, cited answer (loads the model once, like Ask-AI). Browser-only-verifiable.
  async function ragAnswer() {
    if (llmBusy) return;
    const q = ragState.lastQuery, hits = ragState.lastHits || [];
    const ansEl = $("semanticAnswer");
    if (!q || !hits.length || !ansEl) return;
    if (aiIsGemma4() && !aiGemma4) { ansEl.innerHTML = note("Open ✨ Ask AI once to load the model, then try again."); return; }
    const ctx = hits.slice(0, 8).map((h, i) => `[${i + 1}] ${h.title}`).join("\n");
    const messages = [{ role: "user", content:
      `Using ONLY these retrieved records from the ${state.dataset} collection, answer the question. Cite records by [number]; if they don't cover it, say so.\n\nRecords:\n${ctx}\n\nQuestion: ${q}\n\nConcise, grounded answer:` }];
    let acc = "";
    const stream = () => {
      acc = ""; llmBusy = true; ansEl.innerHTML = '<span class="ai-cursor">▌</span>';
      aiOnToken = (t) => { acc += t; ansEl.innerHTML = esc(acc) + '<span class="ai-cursor">▌</span>'; };
      aiOnError = (e) => { ansEl.innerHTML = '<span class="ai-warn">' + esc(e) + '</span>'; llmBusy = false; };
      aiOnDone = () => { ansEl.innerHTML = esc(acc); llmBusy = false; };
      if (aiIsGemma4() && aiGemma4) {
        (async () => { try {
          for await (const c of aiGemma4.generate(messages, { maxNewTokens: 512 })) { acc = (c && c.text != null) ? c.text : acc; ansEl.innerHTML = esc(acc) + '<span class="ai-cursor">▌</span>'; }
          ansEl.innerHTML = esc(acc); llmBusy = false;
        } catch (e) { ansEl.innerHTML = '<span class="ai-warn">' + esc(String((e && e.message) || e)) + '</span>'; llmBusy = false; } })();
        return;
      }
      ensureLlmWorker().postMessage({ type: "generate", messages });
    };
    if (llmLoaded || (aiIsGemma4() && aiGemma4)) { stream(); return; }
    ansEl.innerHTML = '<span class="ai-dim">loading the AI model (first time)…</span>';
    const onLoad = () => { llmLoaded = true; stream(); };
    onLoad.__load = true; aiOnDone = onLoad;
    aiOnError = (e) => { ansEl.innerHTML = '<span class="ai-warn">model load failed: ' + esc(e) + '</span>'; };
    ensureLlmWorker().postMessage({ type: "load", id: aiModelId() });
  }

  async function runSemantic() {
    const q = ($("semanticQ").value || "").trim();
    if (!q) return;
    if (!ragState.ready || ragState.key !== state.dataset) { await ensureSemantic(); if (!ragState.ready) return; }
    const rag = (CATALOG.rag || {})[state.dataset];
    $("semanticOut").innerHTML = `<p class="microcopy">Embedding the query (WebGPU)…</p>`;
    try { const qv = await ragCall({ type: "embed", text: q, prefix: rag.queryPrefix || "query: " }); ragRank(qv, q); }
    catch (e) { $("semanticOut").innerHTML = ""; showError("semanticOut", "Query failed: " + (e && e.message || e)); }
  }
  // Clickable starter queries for the Semantic tab (CATALOG.rag[key].examples).
  function renderSemanticExamples() {
    const box = $("semanticExamples");
    if (!box) return;
    const rag = (CATALOG.rag || {})[state.dataset] || {};
    const exs = Array.isArray(rag.examples) ? rag.examples : [];
    if (!exs.length) { box.classList.add("hidden"); box.innerHTML = ""; return; }
    box.classList.remove("hidden");
    box.innerHTML = '<span style="font-size:.82em;color:var(--muted,#888);align-self:center">Try:</span>' +
      exs.map((q) => `<button type="button" class="chip" style="cursor:pointer" data-q="${esc(q)}">${esc(q)}</button>`).join("");
    $$("#semanticExamples button").forEach((b) => { b.onclick = () => { $("semanticQ").value = b.dataset.q; runSemantic(); }; });
  }

  // Hand the norms the semantic search surfaced to the SPARQL tab as a VALUES-bound
  // query, so the user can pull structured columns / ask follow-ups over exactly
  // those hits (CATALOG.rag[key].enrich.q, %IRIS% = the ranked result IRIs).
  function semanticToSparql() {
    const hits = ragState.lastHits || [];
    if (!hits.length) return;
    const rag = (CATALOG.rag || {})[state.dataset] || {};
    const enrich = rag.enrich || {};
    const values = hits.map((h) => "<" + h.iri + ">").join(" ");
    const tmpl = enrich.q || "SELECT ?s ?p ?o WHERE {\n  VALUES ?s { %IRIS% }\n  ?s ?p ?o\n}\nLIMIT 300";
    setEd("q", tmpl.replace("%IRIS%", values));
    state.colLabels = enrich.cols || null;
    state.selectedExample = null;
    setView("table");
    setStrategy("whole");
    setMode("sparql");
    runQuery();
  }
  try { window.__ragState = ragState; window.__ragRank = ragRank; window.__ragEnsure = ensureSemantic; } catch (e) { /* test hooks */ }

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
    if (mode === "semantic") ensureSemantic();
    if (mode === "schema" && state.remote && !state.schema) ensureRemoteSchema();
    updateResultVisibility();
    updateHash();
    if (mrbUpdate) mrbUpdate(); // the phone Run bar only shows in SPARQL mode
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
  // SQL Explore companions (parquet/duckdb/sqlite) still live on the HF Space at their
  // original playground/<...> paths — they are NOT migrated to R2 (huge, secondary), so
  // they use their own base/token, independent of the graph CDN (remoteBase = R2).
  function companionUrl(path) {
    const base = CATALOG.companionBase || CATALOG.remoteBase;
    const t = CATALOG.companionToken != null ? CATALOG.companionToken : CATALOG.remoteToken;
    const tok = t ? "?token=" + t : "";
    return `${base}/${path}${tok}`;
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
  function clearRangeCache() {
    return idbOpen().then((db) => new Promise((res) => {
      const t = db.transaction([RANGES, RMETA], "readwrite");
      t.objectStore(RANGES).clear(); t.objectStore(RMETA).clear();
      t.oncomplete = () => res(); t.onerror = () => res();
    })).catch(() => {});
  }
  // Per-file breakdown of the range cache: one row per cached .rete (its key is
  // the file's origin+path), with how many bytes of it are held vs its total
  // size. Read from rangeMeta, which the worker shim maintains. This is the
  // IndexedDB-backed (persistent) cache — it survives reloads and sessions.
  function rangeCacheBreakdown() {
    return idbOpen().then((db) => new Promise((res) => {
      const out = []; const t = db.transaction(RMETA).objectStore(RMETA).openCursor();
      t.onsuccess = (e) => {
        const c = e.target.result;
        if (c) { const v = c.value || {}; out.push({ key: c.key, bytes: v.bytes || 0, total: v.total || 0, blocks: (v.blocks || []).length }); c.continue(); }
        else res(out);
      };
      t.onerror = () => res(out);
    })).catch(() => []);
  }
  // Drop one file's ranges: delete its blocks (listed in its rangeMeta entry) and
  // the meta row, leaving other datasets' cached ranges intact.
  function clearRangeCacheKey(key) {
    return idbOpen().then((db) => new Promise((res) => {
      const g = db.transaction(RMETA).objectStore(RMETA).get(key);
      g.onsuccess = () => {
        const blocks = (g.result && g.result.blocks) || [];
        const tx = db.transaction([RANGES, RMETA], "readwrite");
        const rs = tx.objectStore(RANGES);
        blocks.forEach((b) => { try { rs.delete(key + "#" + b); } catch (e) { /* ignore */ } });
        tx.objectStore(RMETA).delete(key);
        tx.oncomplete = () => res(); tx.onerror = () => res();
      };
      g.onerror = () => res();
    })).catch(() => {});
  }
  // Map a range-cache key (a file's origin+path) back to a friendly dataset
  // label. The shim keys by origin+pathname; mirror that here over every catalog
  // dataset's resolved URL, then fall back to the bare filename.
  function rcUrlKey(url) {
    try { const u = new URL(url, location.href); return u.origin + u.pathname; }
    catch (e) { const s = String(url), q = s.indexOf("?"); return q < 0 ? s : s.slice(0, q); }
  }
  let _rcDsByKey = null;
  function rcLabelForKey(key) {
    if (!_rcDsByKey) {
      _rcDsByKey = new Map();
      (CATALOG.datasets || []).forEach((d) => { try { _rcDsByKey.set(rcUrlKey(remoteUrlFor(d.key)), d.key); } catch (e) { /* skip */ } });
    }
    const ds = _rcDsByKey.get(key);
    if (ds) return dsShortLabel(ds);
    const slash = String(key).lastIndexOf("/");
    return slash >= 0 ? String(key).slice(slash + 1) : String(key);
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
    if (!seg.dataset.wired) { $("sqlRun").onclick = runSql; seg.dataset.wired = "1"; }
    // A fresh dataset re-renders the backend buttons (gated on which companion
    // files this dataset actually ships — a big graph may skip the 2 GB SQLite)
    // and clears the editor/output so the default query + examples re-seed.
    if (state.sqlDataset !== state.dataset) {
      state.sqlDataset = state.dataset;
      const avail = SQL_BACKENDS.filter(([id]) =>
        id === "duck-parquet" ? comp.parquetDir : id === "duck-db" ? comp.duckdb : comp.sqlite);
      if (!avail.some(([id]) => id === state.sqlBackend)) state.sqlBackend = (avail[0] || SQL_BACKENDS[0])[0];
      seg.innerHTML = avail.map(([id, l]) => `<button type="button" data-sb="${esc(id)}">${esc(l)}</button>`).join("");
      seg.querySelectorAll("[data-sb]").forEach((b) => b.onclick = () => {
        seg.querySelectorAll("[data-sb]").forEach((x) => x.classList.toggle("active", x === b));
        state.sqlBackend = b.dataset.sb; renderSqlExamples();
      });
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

  // Refreshes the phone's sticky Run bar (set in wireEvents; null on desktop).
  let mrbUpdate = null;

  function setView(view) {
    // On a phone, a default "table" view becomes Cards — the table is the one
    // output that fights a small screen, and Cards renders the same rows
    // stacked. An example that declares any other view (map / graph / time / …)
    // keeps it, and the user can always switch back to Table by hand.
    if (view === "table" && window.matchMedia && window.matchMedia("(max-width: 560px)").matches) {
      view = "cards";
    }
    $("fmt").value = view;
  }

  // Output types that are all renderings of the SAME SELECT bindings (the engine
  // returns table rows; Graph/Map/Time just draw them differently). Switching
  // among these never needs the query to run again — only a re-render. The
  // serialization views (TTL/JSON-LD) are a different engine output, so they
  // still run.
  const ROW_VIEWS = new Set(["table", "cards", "graph", "map", "tiles", "time"]);

  // Changing the Output type re-renders the last result in the new view with no
  // re-run, whenever that's possible: the cached result must be row-shaped, the
  // new view a row view, and the query/strategy/dataset unchanged since it ran.
  // Anything else (a serialization target, an edited query, a stale or missing
  // cache) falls through to a normal run — which keeps the row cache, so a
  // round-trip through TTL/JSON-LD and back to a row view re-renders for free.
  // A CONSTRUCT or DESCRIBE produces an RDF graph (triples); SELECT/ASK don't.
  function isGraphQuery(q) {
    return /\b(CONSTRUCT|DESCRIBE)\b/i.test(String(q || "").split(/\bWHERE\b/i)[0]);
  }
  function onOutputTypeChange() {
    const fmt = $("fmt").value;
    // TTL / JSON-LD serialize an RDF graph — only CONSTRUCT/DESCRIBE makes one.
    // Say so up front (and don't run a query that can't serialize) when the
    // editor holds a SELECT/ASK.
    if ((fmt === "ttl" || fmt === "jsonld") && !isGraphQuery($("q").value)) {
      const name = fmt === "ttl" ? "Turtle (TTL)" : "JSON-LD";
      $("out").innerHTML = `<div class="note">${name} serializes an <b>RDF graph</b>, which only a <b>CONSTRUCT</b> or <b>DESCRIBE</b> query produces. This query is a <b>SELECT</b> — switch <b>Output</b> back to <b>Table</b>, or rewrite it as <code>CONSTRUCT { … } WHERE { … }</code> to export ${name}.</div>`;
      $("qmeta").textContent = `${name} needs a CONSTRUCT or DESCRIBE query`;
      updateResultVisibility();
      return;
    }
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
  // A dereferenceable web URL: http(s) with a real (dotted) host — excludes the
  // synthetic http://ex/… namespace (host "ex", no dot) so toy IRIs aren't linked.
  function looksWebUrl(v) {
    if (!/^https?:\/\//i.test(v)) return false;
    try { return new URL(v).host.indexOf(".") > 0; } catch (_e) { return false; }
  }
  // The page is served over HTTPS, so an http:// iframe/link is blocked as mixed
  // content — upgrade to https for the actual fetch/navigation (display keeps the
  // original IRI). Most RDF hosts (schema.org, wikidata.org, …) serve https.
  function httpsUpgrade(v) { return String(v).replace(/^http:\/\//i, "https://"); }
  // An image cell: a Wikimedia Commons file (wdt:P18 → Special:FilePath/…) or any
  // URL ending in an image extension. Rendered as a clickable thumbnail.
  function looksImageUrl(v) {
    if (!/^https?:\/\//i.test(v)) return false;
    return /commons\.wikimedia\.org\/wiki\/Special:FilePath\//i.test(v) ||
      // Coeli (MCNB / bioexplora) image URLs have no extension: a portraitMedia
      // redirect or a IIIF Image-API path. Both resolve to a CORS-open JPEG.
      (/\bcoeli\b/i.test(v) && /(portraitMedia|\/full\/|\/iiif\/)/i.test(v)) ||
      // Patrinum (BCUL) nanna thumbnails have no extension: /nanna/thumbnail/v2/<id>?redirect=1
      // 302s to a CORS-open JPEG; treat as an image so covers render inline.
      /patrinum\.ch\/nanna\/(thumbnail|record-thumb)\//i.test(v) ||
      // ECAL (BiblioMaker) book covers, also extension-less: …/BM_DOCUMENT_COVER_PAGE[_THUMBNAIL]/<id>
      /bibliomaker\.ch[:/][^"]*BM_DOCUMENT_COVER_PAGE/i.test(v) ||
      /\.(jpe?g|png|gif|svg|webp)$/i.test(String(v).split("?")[0]);
  }
  function thumbUrl(v) {
    const https = httpsUpgrade(v);
    // Commons FilePath takes ?width=N for a server-scaled thumbnail.
    return /Special:FilePath\//i.test(https) ? https + (https.includes("?") ? "&" : "?") + "width=200" : https;
  }
  // Force-renderers used when the user picks a type from a column's header
  // dropdown — they override the per-value heuristic in `autoCell`. The point is
  // to render, e.g., an image column whose URLs DON'T end in `.jpg` (a CDN/API
  // URL or a bare entity IRI), or to stop a long IRI column from linking.
  // When true, image cells load eagerly instead of `loading="lazy"`. Set while
  // rendering the *visible* cards (see cardsInner): a card's photo is the point
  // of the view, and the cards are tall, so native lazy-loading leaves most of
  // them blank until scrolled to. `onerror` marks a genuinely-missing image so
  // it shows a placeholder rather than an unexplained blank box.
  let mediaEager = false;
  function imageCell(t) {
    const url = httpsUpgrade(t.value);
    const loading = mediaEager ? "eager" : "lazy";
    return `<td class="iri thumb-cell"><a href="${esc(url)}" target="_blank" rel="noopener noreferrer" ` +
      `title="${esc(t.value)}"><img class="cell-thumb" src="${esc(thumbUrl(t.value))}" loading="${loading}" alt="" ` +
      `onerror="this.classList.add('cell-thumb-broken');this.alt='image unavailable'" /></a>` +
      `<div class="media-meta" data-murl="${esc(url)}" data-mkind="image"></div></td>`;
  }
  function linkCell(t) {
    const url = httpsUpgrade(t.value);
    const disp = shorten(t.value, 96);
    return `<td class="iri"><a class="iri-link" href="${esc(url)}" target="_blank" rel="noopener noreferrer" ` +
      `data-url="${esc(t.value)}">${esc(disp)}</a></td>`;
  }
  // A URL rendered as a call-to-action button (opens in a new tab) — e.g. a
  // record/detail page. Force it with the column's "Button" render type when a
  // plain link should read as an action. Non-URL values fall back to a link.
  function buttonCell(t) {
    const url = httpsUpgrade(t.value);
    const label = t.iri || looksWebUrl(t.value) ? "Open ↗" : shorten(t.value, 40);
    return `<td class="iri"><a class="cell-btn" href="${esc(url)}" target="_blank" rel="noopener noreferrer" ` +
      `title="${esc(t.value)}" data-url="${esc(t.value)}">${esc(label)}</a></td>`;
  }
  // ---- IIIF cells -----------------------------------------------------------
  // A IIIF *manifest* URL (e.g. biblissima prop:P196 → digi.vatlib.it/…/manifest.json)
  // is not itself an image, but it points at one. Render a placeholder link, then
  // (post-render, async) fetch the manifest and swap in its thumbnail. Works
  // cross-origin where the IIIF server sends CORS (the spec recommends it); on any
  // failure the link stays. Handles IIIF Presentation v2 and v3.
  function looksIiifUrl(v) {
    if (!/^https?:\/\//i.test(v)) return false;
    return /\/manifest(\.json)?(\?|#|$)/i.test(v) || /\/iiif\//i.test(v);
  }
  const iiifDocCache = new Map(); // manifest URL → Promise<{canvases:[{thumb,zoom,label}]} | null>
  // A IIIF image resource (v2 resource / v3 body) → {thumb,zoom} via its Image API
  // service (resizable) or the bare image id (static). Zoom capped at 1024px.
  function iiifImageUrls(res) {
    if (!res) return null;
    let s = res.service; s = Array.isArray(s) ? s[0] : s;
    const sid = s && (s.id || s["@id"]);
    if (sid) {
      const base = String(sid).replace(/\/$/, "");
      return { thumb: base + "/full/!256,256/0/default.jpg", zoom: base + "/full/!1024,1024/0/default.jpg" };
    }
    const id = res.id || res["@id"];
    return id ? { thumb: id, zoom: id } : null;
  }
  // A IIIF label (string, {@value}, or v3 language map) → plain text.
  function iiifLabel(o) {
    let l = o && o.label;
    if (!l) return "";
    if (typeof l === "string") return l;
    if (Array.isArray(l)) l = l[0];
    if (l && l["@value"]) return l["@value"];
    if (l && typeof l === "object") { const v = l[Object.keys(l)[0]]; return Array.isArray(v) ? v[0] : String(v); }
    return "";
  }
  // Every page/canvas of a manifest as {thumb,zoom,label} — IIIF Presentation v2
  // (sequences→canvases→images→resource) and v3 (items→items→items→body).
  function iiifCanvases(m) {
    const out = [];
    for (const seq of (m.sequences || [])) for (const cv of (seq.canvases || [])) {
      const img = (cv.images || [])[0];
      const u = img && iiifImageUrls(img.resource);
      if (u) out.push(Object.assign(u, { label: iiifLabel(cv) }));
    }
    if (!out.length) for (const cv of (m.items || [])) {
      const ann = cv.items && cv.items[0] && cv.items[0].items && cv.items[0].items[0];
      let body = ann && ann.body; body = Array.isArray(body) ? body[0] : body;
      const u = iiifImageUrls(body);
      if (u) out.push(Object.assign(u, { label: iiifLabel(cv) }));
    }
    if (!out.length) {                                  // last resort: the manifest's own thumbnail
      let tn = m.thumbnail; tn = Array.isArray(tn) ? tn[0] : tn;
      const t = typeof tn === "string" ? tn
        : tn && (tn.id || tn["@id"] || (tn.service && iiifImageUrls(tn) && iiifImageUrls(tn).thumb));
      if (t) out.push({ thumb: t, zoom: t, label: "" });
    }
    return out;
  }
  // General IIIF text: a string, {@value}, an array, or a v3 language map → plain text.
  function iiifText(v) {
    if (v == null) return "";
    if (typeof v === "string") return v;
    if (Array.isArray(v)) return v.map(iiifText).filter(Boolean).join("; ");
    if (v["@value"]) return v["@value"];
    if (typeof v === "object") { const k = Object.keys(v)[0]; return k ? iiifText(v[k]) : ""; }
    return String(v);
  }
  // Manifest-level metadata for the modal: title + key/value fields + attribution/rights.
  function iiifMeta(m, url) {
    const fields = [];
    (m.metadata || []).forEach((e) => {
      const value = iiifText(e.value);
      if (value) fields.push({ label: iiifText(e.label), value: value });
    });
    const attr = iiifText(m.attribution) || (m.requiredStatement && iiifText(m.requiredStatement.value));
    if (attr) fields.push({ label: "Attribution", value: attr });
    const rights = iiifText(m.license) || iiifText(m.rights);
    if (rights) fields.push({ label: "Rights", value: rights });
    return { label: iiifText(m.label), fields: fields.slice(0, 24), url: url };
  }
  function fetchIiifDoc(url) {
    const key = httpsUpgrade(url);
    if (iiifDocCache.has(key)) return iiifDocCache.get(key);
    const p = fetch(key, { headers: { Accept: "application/json,application/ld+json,*/*" } })
      .then((r) => (r.ok ? r.json() : null))
      .then((m) => (m ? { canvases: iiifCanvases(m), meta: iiifMeta(m, url) } : null))
      .catch(() => null);
    iiifDocCache.set(key, p);
    return p;
  }
  function iiifCell(t) {
    const url = httpsUpgrade(t.value);
    return `<td class="iri iiif-cell thumb-cell" data-iiif="${esc(t.value)}">` +
      `<a class="iri-link iiif-link" href="${esc(url)}" target="_blank" rel="noopener noreferrer" ` +
      `title="IIIF manifest — ${esc(t.value)}"><span class="spindle"></span> IIIF</a></td>`;
  }

  // ---- IIIF lightbox modal (click a cell → enlarged page + paging + metadata) ----
  let iiifModalEl = null, iiifModal = null; // iiifModal = { canvases, meta, idx }
  function ensureIiifModal() {
    if (iiifModalEl) return iiifModalEl;
    const el = document.createElement("div");
    el.className = "iiif-modal hidden";
    el.innerHTML =
      '<div class="iiif-modal-backdrop"></div>' +
      '<div class="iiif-modal-box" role="dialog" aria-modal="true" aria-label="IIIF page viewer">' +
        '<button class="iiif-modal-close" type="button" aria-label="close">×</button>' +
        '<div class="iiif-modal-stage">' +
          '<button class="iiif-modal-prev" type="button" aria-label="previous page">‹</button>' +
          '<img class="iiif-modal-img" alt="" />' +
          '<button class="iiif-modal-next" type="button" aria-label="next page">›</button>' +
        '</div>' +
        '<div class="iiif-modal-bar"><span class="iiif-modal-page"></span>' +
          '<a class="iiif-modal-link" target="_blank" rel="noopener noreferrer">IIIF manifest ↗</a></div>' +
        '<div class="iiif-modal-meta"></div>' +
      '</div>';
    document.body.appendChild(el);
    const close = () => el.classList.add("hidden");
    el.querySelector(".iiif-modal-close").addEventListener("click", close);
    el.querySelector(".iiif-modal-backdrop").addEventListener("click", close);
    el.querySelector(".iiif-modal-prev").addEventListener("click", () => iiifModalGo(-1));
    el.querySelector(".iiif-modal-next").addEventListener("click", () => iiifModalGo(1));
    document.addEventListener("keydown", (e) => {
      if (el.classList.contains("hidden")) return;
      if (e.key === "Escape") close();
      else if (e.key === "ArrowLeft") iiifModalGo(-1);
      else if (e.key === "ArrowRight") iiifModalGo(1);
    });
    iiifModalEl = el;
    return el;
  }
  function iiifModalGo(d) {
    if (!iiifModal) return;
    const n = iiifModal.canvases.length;
    iiifModal.idx = ((iiifModal.idx + d) % n + n) % n;
    renderIiifModal();
  }
  function renderIiifModal() {
    const el = iiifModalEl, st = iiifModal;
    if (!el || !st) return;
    const c = st.canvases[st.idx];
    const img = el.querySelector(".iiif-modal-img");
    img.src = httpsUpgrade(c.zoom || c.thumb);
    img.onerror = () => { img.onerror = null; img.src = httpsUpgrade(c.thumb); };
    el.querySelector(".iiif-modal-page").textContent =
      (c.label ? c.label + " · " : "") + "page " + (st.idx + 1) + " / " + st.canvases.length;
    const link = el.querySelector(".iiif-modal-link");
    link.href = httpsUpgrade(st.meta.url); link.title = st.meta.url;
    let mh = st.meta.label ? '<h3 class="iiif-modal-title">' + esc(st.meta.label) + "</h3>" : "";
    if (st.meta.fields.length) {
      mh += "<dl>" + st.meta.fields.map((f) =>
        (f.label ? "<dt>" + esc(f.label) + "</dt>" : "<dt></dt>") + "<dd>" + esc(f.value) + "</dd>").join("") + "</dl>";
    }
    el.querySelector(".iiif-modal-meta").innerHTML = mh;
    const single = st.canvases.length <= 1;
    el.querySelector(".iiif-modal-prev").style.visibility = single ? "hidden" : "";
    el.querySelector(".iiif-modal-next").style.visibility = single ? "hidden" : "";
  }
  function openIiifModal(doc, idx) {
    ensureIiifModal();
    iiifModal = { canvases: doc.canvases, meta: doc.meta, idx: idx || 0 };
    renderIiifModal();
    iiifModalEl.classList.remove("hidden");
  }

  // Turn one pending IIIF cell into a paged thumbnail: prev/next + a jump box for
  // quick in-table browsing; CLICKING the image opens the lightbox modal (enlarged
  // page, paging, the manifest link and its metadata). Pages fault on demand.
  function renderIiifViewer(td, url, doc) {
    const cs = doc.canvases; let i = 0;
    td.classList.add("iiif-ready");
    td.innerHTML = `<div class="iiif-viewer">` +
      `<button type="button" class="iiif-frame" title="click to enlarge"><img class="cell-thumb iiif-img" loading="lazy" alt="" /></button>` +
      `<div class="iiif-nav${cs.length <= 1 ? " single" : ""}"><button type="button" class="iiif-prev" aria-label="previous page">‹</button>` +
      `<input class="iiif-page" type="text" inputmode="numeric" aria-label="page number" /><span class="iiif-total">/ ${cs.length}</span>` +
      `<button type="button" class="iiif-next" aria-label="next page">›</button></div></div>`;
    const img = td.querySelector(".iiif-img"), page = td.querySelector(".iiif-page"), frame = td.querySelector(".iiif-frame");
    const show = (n) => { i = ((n % cs.length) + cs.length) % cs.length; img.src = httpsUpgrade(cs[i].thumb); page.value = String(i + 1); };
    const jump = () => { const v = parseInt(page.value, 10); if (isFinite(v)) show(v - 1); else page.value = String(i + 1); };
    td.querySelector(".iiif-prev").addEventListener("click", () => show(i - 1));
    td.querySelector(".iiif-next").addEventListener("click", () => show(i + 1));
    page.addEventListener("change", jump);
    page.addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); jump(); } });
    frame.addEventListener("click", () => openIiifModal(doc, i));
    img.addEventListener("error", () => { if (cs.length > 1 && i < cs.length - 1) show(i + 1); });
    show(0);
  }
  // The manifest couldn't be loaded (the IIIF server blocked the cross-origin
  // request, or it 404'd / wasn't valid JSON). Swap the spinner for a clear,
  // non-spinning state with a link to open the manifest directly.
  function iiifFailed(td, url) {
    const up = httpsUpgrade(url);
    td.classList.add("iiif-blocked");
    td.innerHTML = `<a class="iri-link iiif-fail" href="${esc(up)}" target="_blank" rel="noopener noreferrer" ` +
      `title="Couldn't load this IIIF manifest — the server likely blocks cross-origin (CORS) requests. Click to open it directly: ${esc(url)}">` +
      `⚠ IIIF blocked</a>`;
  }
  function hydrateIiif(scope) {
    // Class-based (not td-scoped): IIIF cells render inside table cells AND card
    // field divs — both carry .iiif-cell.
    (scope || document).querySelectorAll(".iiif-cell[data-iiif]").forEach((td) => {
      const url = td.getAttribute("data-iiif");
      td.removeAttribute("data-iiif"); // process once
      fetchIiifDoc(url).then((doc) => {
        if (!doc || !doc.canvases.length) { iiifFailed(td, url); return; }
        renderIiifViewer(td, url, doc);
      });
    });
  }
  // Inline 3D cells: load the <model-viewer> web component the first time one
  // appears; each <model-viewer> element then upgrades and lazy-loads its own .glb.
  function hydrateModel3d(scope) {
    if ((scope || document).querySelector(".model3d-cell model-viewer")) ensureModelViewer();
  }
  // ---- geo mini-map cells ---------------------------------------------------
  // A WKT geometry literal (geo:wktLiteral: POINT / POLYGON / LINESTRING …) drawn
  // as a small square locator map — the dot/shape on a light frame with lat-lon
  // ticks on the borders. A lone point sits on a cached world basemap tile (one
  // shared, cached request) for "where on Earth" context; a shape fits its own
  // bbox so you can read it. Fully offline for shapes (graticule only).
  let geoSeq = 0;
  const geoData = {}; // cell id → { wkt, fineIri } — read when the cell opens the map modal
  // The row currently being rendered (set by tableInner around each row's cells), so
  // a geo cell can find its feature's IRI without threading it through every renderer.
  let geoRowCtx = null;
  const GEO_FINE_PRED = "https://geoadmin.rete/prop/geomFine";
  // In a result row, find the geoadmin admin-area IRI (country/region/district) whose
  // geometry this is, so the map modal can fetch that feature's fine LOD on demand.
  // Returns null when the row carries no geoadmin entity → the modal shows the coarse
  // shape only. (Places are points; they have no finer LOD, so they're excluded.)
  function geoFeatureIri(row) {
    if (!row) return null;
    for (const k in row) {
      const v = row[k];
      if (typeof v !== "string") continue;
      const m = /^<?(https:\/\/geoadmin\.rete\/(?:country|region)\/[^>\s"]+)>?$/.exec(v);
      if (m) return m[1];
    }
    return null;
  }
  function looksWktGeo(v) { return WKT_RE.test(String(v)); }
  function geoDecimate(ring, cap) {
    if (ring.length <= cap) return ring;
    const step = ring.length / cap, out = [];
    for (let i = 0; i < ring.length - 1; i += step) out.push(ring[Math.floor(i)]);
    out.push(ring[ring.length - 1]);
    return out;
  }
  // ~`want` round-number ticks spanning [lo,hi] (… −90, 0, 90 …).
  function geoTicks(lo, hi, want) {
    const span = hi - lo;
    if (!(span > 0)) return [lo];
    const raw = span / want, mag = Math.pow(10, Math.floor(Math.log10(raw)));
    const nrm = raw / mag, step = (nrm >= 5 ? 5 : nrm >= 2 ? 2 : 1) * mag;
    const out = [];
    for (let v = Math.ceil(lo / step) * step; v <= hi + 1e-9 && out.length < 8; v += step)
      out.push(Math.round(v / step) * step);
    return out;
  }
  const fmtDeg = (v) => (Math.abs(v) < 1e-9 ? "0" : (v < 0 ? "−" : "") + (Math.abs(v) % 1 ? Math.abs(v).toFixed(1) : String(Math.abs(v)))) + "°";

  function geoCell(t) {
    let rings = wktRings(t.value);
    if (!rings.length) return `<td class="lit" title="${esc(t.value)}">${esc(shorten(t.value, 60))}</td>`;
    // Keep the largest rings (the visually significant ones) and decimate each, so
    // a many-island MULTIPOLYGON stays a cheap thumbnail rather than 10k SVG points.
    rings = rings.sort((a, b) => b.length - a.length).slice(0, 40).map((r) => geoDecimate(r, 120));
    let minX = 180, maxX = -180, minY = 90, maxY = -90;
    for (const r of rings) for (const [x, y] of r) {
      if (x < minX) minX = x; if (x > maxX) maxX = x; if (y < minY) minY = y; if (y > maxY) maxY = y;
    }
    const isPoint = rings.length === 1 && rings[0].length === 1;
    const ext = Math.max(maxX - minX, maxY - minY);
    const gutL = 16, gutB = 12, mapW = 86, mapH = 86, vbW = gutL + mapW, vbH = mapH + gutB;

    let sx, sy, bg = "", lonTicks, latTicks;
    if (isPoint || ext < 5e-4) {
      // Zoom to the point (region level) — a lone dot on a whole-world tile tells
      // you nothing. Centre the point in the cell and lay the spanning z6 tiles.
      const clon = (minX + maxX) / 2, clat = (minY + maxY) / 2, Z = 6;
      const wp = 256 * Math.pow(2, Z), world = Math.pow(2, Z);
      const oX = lon2wx(clon) * wp - mapW / 2, oY = lat2wy(clat) * wp - mapH / 2;
      sx = (lon) => gutL + (lon2wx(lon) * wp - oX);
      sy = (lat) => (lat2wy(lat) * wp - oY);
      const tx0 = Math.floor(oX / 256), tx1 = Math.floor((oX + mapW) / 256);
      const ty0 = Math.floor(oY / 256), ty1 = Math.floor((oY + mapH) / 256);
      for (let tx = tx0; tx <= tx1; tx++) for (let ty = ty0; ty <= ty1; ty++) {
        if (ty < 0 || ty >= world) continue;
        const wx = ((tx % world) + world) % world;
        bg += `<image href="https://a.basemaps.cartocdn.com/light_all/${Z}/${wx}/${ty}.png" x="${(gutL + tx * 256 - oX).toFixed(1)}" y="${(ty * 256 - oY).toFixed(1)}" width="256" height="256" preserveAspectRatio="none" />`;
      }
      lonTicks = []; latTicks = [];
    } else {
      // Local equirectangular fit to the geometry bbox (uniform, ~16% margin).
      const cx = (minX + maxX) / 2, cy = (minY + maxY) / 2, half = ext * 0.58;
      const loX = cx - half, loY = cy - half;
      sx = (lon) => gutL + ((lon - loX) / (2 * half)) * mapW;
      sy = (lat) => ((cy + half - lat) / (2 * half)) * mapH;
      lonTicks = geoTicks(loX, cx + half, 3); latTicks = geoTicks(loY, cy + half, 3);
      for (const lon of lonTicks) { const x = sx(lon).toFixed(1); bg += `<line class="geo-grid" x1="${x}" y1="0" x2="${x}" y2="${mapH}"/>`; }
      for (const lat of latTicks) { const y = sy(lat).toFixed(1); bg += `<line class="geo-grid" x1="${gutL}" y1="${y}" x2="${vbW}" y2="${y}"/>`; }
    }
    let geo = "";
    for (const r of rings) {
      if (r.length === 1) { const [x, y] = r[0]; geo += `<circle class="geo-pt" cx="${sx(x).toFixed(1)}" cy="${sy(y).toFixed(1)}" r="2.6"/>`; }
      else {
        const pts = r.map(([x, y]) => `${sx(x).toFixed(1)},${sy(y).toFixed(1)}`).join(" ");
        geo += /POLYGON/i.test(t.value) ? `<polygon class="geo-poly" points="${pts}"/>` : `<polyline class="geo-line" points="${pts}"/>`;
      }
    }
    let ticks = "";
    for (const lon of lonTicks) { const x = sx(lon); if (x < gutL - 1 || x > vbW + 1) continue; ticks += `<text class="geo-tick" x="${Math.min(vbW - 1, Math.max(gutL + 1, x)).toFixed(1)}" y="${vbH - 3}" text-anchor="middle">${esc(fmtDeg(lon))}</text>`; }
    for (const lat of latTicks) { const y = sy(lat); if (y < -1 || y > mapH + 1) continue; ticks += `<text class="geo-tick" x="${gutL - 2}" y="${Math.min(mapH - 1, Math.max(6, y + 2)).toFixed(1)}" text-anchor="end">${esc(fmtDeg(lat))}</text>`; }
    const id = "gc" + ++geoSeq;
    const fineIri = geoFeatureIri(geoRowCtx);
    geoData[id] = { wkt: t.value, fineIri };
    const title = (isPoint ? `POINT ${minX.toFixed(4)}, ${minY.toFixed(4)}`
      : `bbox ${minX.toFixed(3)},${minY.toFixed(3)} … ${maxX.toFixed(3)},${maxY.toFixed(3)}`) + " — click to open the map" +
      (fineIri ? " (fine detail on zoom)" : "");
    return `<td class="geo-cell" data-geo="${id}"${fineIri ? ' data-fine="1"' : ''} title="${esc(title)}">` +
      `<svg viewBox="0 0 ${vbW} ${vbH}" role="img" aria-label="map preview of ${esc(title)}">` +
      `<clipPath id="${id}"><rect x="${gutL}" y="0" width="${mapW}" height="${mapH}"/></clipPath>` +
      `<g clip-path="url(#${id})">${bg}${geo}</g>` +
      `<rect class="geo-bd" x="${gutL}" y="0" width="${mapW}" height="${mapH}"/>${ticks}</svg></td>`;
  }

  // ---- geometry map modal ---------------------------------------------------
  // Clicking a geo cell opens a full, pannable/zoomable Leaflet map (the library
  // is lazy-loaded from the CDN on first open, like model-viewer), fitted to the
  // geometry's bounds. Point → marker, line/polygon → vector overlay.
  let geoModalEl = null, geoMap = null, leafletP = null, geoModalSeq = 0;
  function loadLeaflet() {
    if (leafletP) return leafletP;
    leafletP = new Promise((resolve, reject) => {
      const css = document.createElement("link");
      css.rel = "stylesheet"; css.href = "https://unpkg.com/leaflet@1.9.4/dist/leaflet.css";
      document.head.appendChild(css);
      const s = document.createElement("script");
      s.src = "https://unpkg.com/leaflet@1.9.4/dist/leaflet.js";
      s.onload = () => resolve(window.L); s.onerror = () => reject(new Error("leaflet load failed"));
      document.head.appendChild(s);
    });
    return leafletP;
  }
  function ensureGeoModal() {
    if (geoModalEl) return geoModalEl;
    geoModalEl = document.createElement("div");
    geoModalEl.className = "geo-modal hidden";
    geoModalEl.innerHTML =
      `<div class="geo-modal-box"><div class="geo-modal-head"><span class="geo-modal-title"></span>` +
      `<span class="geo-lod" aria-live="polite"></span>` +
      `<button class="geo-modal-close" type="button" aria-label="Close">✕</button></div>` +
      `<div class="geo-modal-map"></div><div class="geo-modal-foot mono"></div></div>`;
    document.body.appendChild(geoModalEl);
    geoModalEl.addEventListener("click", (e) => {
      if (e.target === geoModalEl || e.target.closest(".geo-modal-close")) closeGeoModal();
    });
    document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeGeoModal(); });
    return geoModalEl;
  }
  function closeGeoModal() { if (geoModalEl) { geoModalEl.classList.add("hidden"); geoModalEl.classList.remove("geo-modal-full"); } }
  // Draw WKT rings as Leaflet vector layers (point → marker, polygon/line → vector).
  function geoLayers(L, rings, wkt) {
    const isPoly = /POLYGON/i.test(wkt);
    return rings.map((r) => r.length === 1
      ? L.circleMarker([r[0][1], r[0][0]], { radius: 8, color: "#fff", weight: 2, fillColor: "#c0392b", fillOpacity: .95 })
      : (isPoly ? L.polygon(r.map(([x, y]) => [y, x]), { color: "#147d69", weight: 2, fillOpacity: .15 })
                : L.polyline(r.map(([x, y]) => [y, x]), { color: "#147d69", weight: 2 })));
  }
  async function openGeoModal(entry) {
    const wkt = entry && typeof entry === "object" ? entry.wkt : entry;
    const fineIri = entry && typeof entry === "object" ? entry.fineIri : null;
    if (!wkt) return;
    ensureGeoModal();
    const rings = wktRings(wkt);
    if (!rings.length) return;
    const mySeq = ++geoModalSeq;
    const isPoint = rings.length === 1 && rings[0].length === 1;
    geoModalEl.classList.remove("geo-modal-full"); // single-cell view: normal size
    geoModalEl.querySelector(".geo-modal-title").textContent = isPoint ? "📍 Location" : "🗺 Geometry";
    geoModalEl.querySelector(".geo-modal-foot").textContent = shorten(wkt, 200);
    const lodEl = geoModalEl.querySelector(".geo-lod");
    if (lodEl) { lodEl.textContent = ""; lodEl.className = "geo-lod"; }
    geoModalEl.classList.remove("hidden");
    const mapDiv = geoModalEl.querySelector(".geo-modal-map");
    let L;
    try { L = await loadLeaflet(); } catch (_e) { mapDiv.innerHTML = `<div class="note">Couldn't load the map library (offline?). The coordinates are below.</div>`; return; }
    if (geoModalSeq !== mySeq) return; // a newer open superseded this one
    if (geoMap) { geoMap.remove(); geoMap = null; }
    mapDiv.innerHTML = "";
    geoMap = L.map(mapDiv, { scrollWheelZoom: true }).setView([0, 0], 2);
    L.tileLayer("https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png",
      { attribution: "© OpenStreetMap · © CARTO", maxZoom: 19, subdomains: "abcd" }).addTo(geoMap);
    let group = L.featureGroup(geoLayers(L, rings, wkt)).addTo(geoMap);
    try { geoMap.fitBounds(group.getBounds().pad(0.4), { maxZoom: 13 }); } catch (_e) { /* single point: keep default */ }
    setTimeout(() => { if (geoMap) geoMap.invalidateSize(); }, 60); // the modal just became visible

    // Multi-LOD: the cell + this first view use the coarse (~1 km) geometry. If this
    // feature has a finer LOD in geoadmin, fetch JUST this one polygon's detail on
    // demand (the remote-lazy payoff — zoom in, fetch only what you inspect) and swap
    // it in. Any miss leaves the coarse shape in place.
    if (fineIri && !isPoint && datasetInfo("geoadmin")) {
      if (lodEl) { lodEl.textContent = "✨ fetching detail…"; lodEl.className = "geo-lod loading"; }
      const q = `SELECT ?g WHERE { <${fineIri}> <${GEO_FINE_PRED}> ?g } LIMIT 1`;
      remoteSparql(remoteUrlFor("geoadmin"), q, "table").then((out) => {
        if (geoModalSeq !== mySeq || !geoMap) return; // modal closed or replaced meanwhile
        let parsed = null; try { parsed = JSON.parse(out.json); } catch (_e) { parsed = null; }
        const raw = parsed && parsed.rows && parsed.rows[0] && parsed.rows[0].g;
        const fineWkt = raw ? parseTerm(raw).value : null;
        const fineRings = fineWkt ? wktRings(fineWkt) : [];
        if (!fineRings.length) { if (lodEl) { lodEl.textContent = ""; lodEl.className = "geo-lod"; } return; }
        try { group.remove(); } catch (_e) {}
        group = L.featureGroup(geoLayers(L, fineRings, fineWkt)).addTo(geoMap);
        if (lodEl) { lodEl.textContent = "✨ fine detail · fetched on demand"; lodEl.className = "geo-lod done"; }
        const foot = geoModalEl.querySelector(".geo-modal-foot"); if (foot) foot.textContent = shorten(fineWkt, 200);
      }).catch(() => { if (lodEl && geoModalSeq === mySeq) { lodEl.textContent = ""; lodEl.className = "geo-lod"; } });
    }
  }

  // Open ALL of a SELECT result's geometries in the full interactive Leaflet
  // modal (pan + native pinch/scroll zoom), from the Map view's ⛶ button — the
  // static SVG map is fine at a glance, this is for exploring. Reuses the same
  // modal/machinery as the single-cell geo modal.
  // A Leaflet base-layer switcher (radio control, top-right) for the full-screen
  // map, built from the same BASEMAPS as the inline map's dropdown. Opens on the
  // inline map's current pick — falling back to Carto Light when that is "none",
  // since a slippy map needs tiles — and syncs a change back to
  // `state.mapBasemap` (+ localStorage + the inline map) so both views agree.
  function addBasemapSwitcher(L, map) {
    const layers = {};
    const idByLabel = {};
    let current = null;
    const prefer = state.mapBasemap && state.mapBasemap !== "none" ? state.mapBasemap : "light";
    for (const b of BASEMAPS) {
      if (!b.url) continue; // skip "none" (the offline vector view — no tiles)
      const opts = { attribution: b.attr, maxZoom: b.max || 19 };
      if (b.sub) opts.subdomains = b.sub;
      const layer = L.tileLayer(b.url, opts);
      layers[b.label] = layer;
      idByLabel[b.label] = b.id;
      if (b.id === prefer) current = layer;
    }
    if (!current) current = Object.values(layers)[0];
    current.addTo(map);
    L.control.layers(layers, null, { collapsed: true, position: "topright" }).addTo(map);
    map.on("baselayerchange", (e) => {
      const id = idByLabel[e.name];
      if (!id) return;
      state.mapBasemap = id;
      try { localStorage.setItem("mapBasemap", id); } catch (_e) { /* private mode */ }
      if (lastMapRes) renderMap(lastMapRes); // re-render the (hidden) inline map to match
    });
  }

  async function openResultMap(res) {
    if (!res || res.kind !== "select") return;
    const vars = res.vars || [], rows = res.rows || [];
    const geo = detectGeoCol(vars, rows);
    if (!geo) return;
    const labelCols = vars.filter((v) => v !== geo);
    ensureGeoModal();
    const mySeq = ++geoModalSeq;
    geoModalEl.querySelector(".geo-modal-title").textContent = "🗺 Map";
    geoModalEl.querySelector(".geo-modal-foot").textContent = "Pinch or scroll to zoom · drag to pan";
    const lodEl = geoModalEl.querySelector(".geo-lod"); if (lodEl) { lodEl.textContent = ""; lodEl.className = "geo-lod"; }
    geoModalEl.classList.add("geo-modal-full");
    geoModalEl.classList.remove("hidden");
    const mapDiv = geoModalEl.querySelector(".geo-modal-map");
    let L;
    try { L = await loadLeaflet(); } catch (_e) { mapDiv.innerHTML = `<div class="note">Couldn't load the map library (offline?).</div>`; return; }
    if (geoModalSeq !== mySeq) return;
    if (geoMap) { geoMap.remove(); geoMap = null; }
    mapDiv.innerHTML = "";
    geoMap = L.map(mapDiv, { scrollWheelZoom: true }).setView([20, 0], 2);
    addBasemapSwitcher(L, geoMap);
    const layers = [];
    for (const r of rows) {
      const raw = r[geo]; if (raw == null) continue;
      const wkt = parseTerm(raw).value; if (!WKT_RE.test(wkt)) continue;
      const rings = wktRings(wkt); if (!rings.length) continue;
      const label = (labelCols.length ? labelCols : [geo]).map((v) => termLabel(parseTerm(r[v]))).filter((s) => s !== "").join(" · ");
      geoLayers(L, rings, wkt).forEach((ly) => { if (label) ly.bindTooltip(label); layers.push(ly); });
    }
    if (!layers.length) return;
    const group = L.featureGroup(layers).addTo(geoMap);
    geoModalEl.querySelector(".geo-modal-title").textContent = `🗺 Map · ${layers.length} feature(s)`;
    try { geoMap.fitBounds(group.getBounds().pad(0.15)); } catch (_e) { /* keep default view */ }
    setTimeout(() => { if (geoMap) geoMap.invalidateSize(); }, 60);
  }

  // ---- 3D model cells -------------------------------------------------------
  // A cell whose value is a streamable mesh (.glb/.gltf/.ply/.splat) opens an
  // inline <model-viewer> lightbox — the web component is lazy-loaded from the CDN
  // only when first opened (like the AI runtime), so the page stays light. A cell
  // that is a 3D *viewer page* (INSCRIBE, PAITO, Sketchfab, a Nexus/3DHOP page) is
  // an HTML page, not a mesh, and is usually all-rights-reserved — it can't be
  // embedded, so it opens in a new tab instead.
  function looksMeshUrl(v) {
    return /^https?:\/\//i.test(v) && /\.(glb|gltf|ply|splat|ksplat)(\?|#|$)/i.test(v);
  }
  function looks3dViewerUrl(v) {
    if (!/^https?:\/\//i.test(v)) return false;
    return /\binscribercproject\.com\b/i.test(v)      // INSCRIBE 3DHOP tablet scans
        || /\bpaitoproject\.it\b/i.test(v)            // PAITO Project (Phaistos + HT)
        || /\bsketchfab\.com\/(3d-models|models)\//i.test(v)
        || /\.nxz(\?|#|$)/i.test(v)                   // Nexus multiresolution mesh
        || /\/3dhop\b/i.test(v);
  }
  function mesh3dCell(t) {
    const url = httpsUpgrade(t.value);
    // An inline, rotatable <model-viewer> right in the cell — drag to rotate, plus a
    // gentle auto-spin. The web component is lazy-loaded once (hydrateModel3d); each
    // viewer lazy-loads its .glb only when scrolled near the viewport (loading=lazy),
    // so a 60-row table doesn't fetch 60 meshes at once. The ⛶ opens the full lightbox.
    return `<td class="iri model3d-cell">` +
      `<model-viewer class="model3d-inline" src="${esc(url)}" camera-controls auto-rotate ` +
      `auto-rotate-delay="0" rotation-per-second="28deg" interaction-prompt="none" disable-zoom ` +
      `loading="lazy" reveal="auto" touch-action="pan-y" environment-image="neutral" ` +
      `shadow-intensity="0.6" alt="3D model"></model-viewer>` +
      `<button type="button" class="model3d-expand" data-mesh="${esc(url)}" ` +
      `title="Enlarge — ${esc(t.value)}" aria-label="Enlarge 3D model">⛶</button>` +
      `<div class="media-meta" data-murl="${esc(url)}" data-mkind="mesh"></div></td>`;
  }
  function viewer3dCell(t) {
    const url = httpsUpgrade(t.value);
    return `<td class="iri model3d-cell"><a class="model3d-link" href="${esc(url)}" target="_blank" rel="noopener noreferrer" ` +
      `title="Open the 3D viewer — ${esc(t.value)}">🧊 3D ↗</a></td>`;
  }
  function model3dCell(t) { return looksMeshUrl(t.value) ? mesh3dCell(t) : viewer3dCell(t); }

  let modelViewerLoading = null;
  function ensureModelViewer() {
    if (modelViewerLoading) return modelViewerLoading;
    modelViewerLoading = new Promise((resolve) => {
      try {
        if (window.customElements && customElements.get("model-viewer")) { resolve(true); return; }
        const s = document.createElement("script");
        s.type = "module";
        s.src = "https://cdn.jsdelivr.net/npm/@google/model-viewer@3.5.0/dist/model-viewer.min.js";
        s.onload = () => resolve(true);
        s.onerror = () => resolve(false);
        document.head.appendChild(s);
      } catch (_e) { resolve(false); }
    });
    return modelViewerLoading;
  }
  let model3dModalEl = null, model3dApply = null;
  // Lighting environments for the lightbox (image-based lighting). All verified to
  // load with CORS from the model-viewer shared assets; "neutral" is built in,
  // "flat" removes the environment for plain three-point lighting.
  const M3_ENVS = {
    sunrise: "https://modelviewer.dev/shared-assets/environments/spruit_sunrise_1k_HDR.jpg",
    studio: "https://modelviewer.dev/shared-assets/environments/aircraft_workshop_01_1k.hdr",
    hall: "https://modelviewer.dev/shared-assets/environments/music_hall_01_1k.hdr",
    outdoor: "https://modelviewer.dev/shared-assets/environments/whipple_creek_regional_park_04_1k.hdr",
  };
  function ensureModel3dModal() {
    if (model3dModalEl) return model3dModalEl;
    const el = document.createElement("div");
    el.className = "model3d-modal hidden";
    el.innerHTML =
      '<div class="model3d-backdrop"></div>' +
      '<div class="model3d-box" role="dialog" aria-modal="true" aria-label="3D model viewer">' +
        '<button class="model3d-close" type="button" aria-label="close">×</button>' +
        '<div class="model3d-stage"></div>' +
        '<div class="model3d-controls">' +
          '<label class="m3-ctl">Lighting <select class="m3-env">' +
            '<option value="neutral">Neutral</option>' +
            '<option value="sunrise">Sunrise</option>' +
            '<option value="studio">Studio</option>' +
            '<option value="hall">Concert hall</option>' +
            '<option value="outdoor">Outdoor</option>' +
            '<option value="flat">Flat</option>' +
          '</select></label>' +
          '<label class="m3-ctl">Brightness <input type="range" class="m3-exposure" min="0.2" max="2.6" step="0.05" value="1.1"></label>' +
          '<label class="m3-ctl">Shadow <input type="range" class="m3-shadow" min="0" max="1" step="0.05" value="1"></label>' +
          '<button type="button" class="m3-spin on" aria-pressed="true">⟳ auto-rotate</button>' +
        '</div>' +
        '<div class="model3d-foot"><span class="model3d-hint">drag to rotate · scroll to zoom</span>' +
          '<span class="model3d-size" title="real-world size of the model"></span>' +
          '<a class="model3d-src" target="_blank" rel="noopener noreferrer">open file ↗</a></div>' +
      '</div>';
    document.body.appendChild(el);
    const stage = el.querySelector(".model3d-stage");
    const envEl = el.querySelector(".m3-env"), expEl = el.querySelector(".m3-exposure"), shEl = el.querySelector(".m3-shadow");
    const spinEl = el.querySelector(".m3-spin");
    // Apply the current control values to the live <model-viewer> (called on every
    // control change and once after a model loads, so settings persist across opens).
    model3dApply = () => {
      const mv = stage.querySelector("model-viewer"); if (!mv) return;
      mv.setAttribute("exposure", expEl.value);
      mv.setAttribute("shadow-intensity", shEl.value);
      const v = envEl.value;
      if (v === "flat") mv.removeAttribute("environment-image");
      else mv.setAttribute("environment-image", v === "neutral" ? "neutral" : M3_ENVS[v]);
      if (spinEl.classList.contains("on")) mv.setAttribute("auto-rotate", "");
      else mv.removeAttribute("auto-rotate");
    };
    [envEl, expEl, shEl].forEach((c) => c.addEventListener("input", model3dApply));
    spinEl.addEventListener("click", () => {
      const on = !spinEl.classList.contains("on");
      spinEl.classList.toggle("on", on); spinEl.setAttribute("aria-pressed", String(on)); model3dApply();
    });
    const close = () => { el.classList.add("hidden"); stage.innerHTML = ""; };
    el.querySelector(".model3d-close").addEventListener("click", close);
    el.querySelector(".model3d-backdrop").addEventListener("click", close);
    document.addEventListener("keydown", (e) => { if (!el.classList.contains("hidden") && e.key === "Escape") close(); });
    model3dModalEl = el;
    return el;
  }
  function openModel3d(url) {
    const el = ensureModel3dModal();
    const stage = el.querySelector(".model3d-stage");
    stage.innerHTML = '<div class="model3d-loading">Loading 3D viewer…</div>';
    el.querySelector(".model3d-src").href = url;
    el.classList.remove("hidden");
    ensureModelViewer().then((ok) => {
      if (el.classList.contains("hidden")) return;        // closed before it loaded
      if (ok) {
        stage.innerHTML = '<model-viewer src="' + esc(url) + '" camera-controls auto-rotate touch-action="pan-y" ' +
          'shadow-intensity="1" exposure="1.1" environment-image="neutral" ' +
          'style="width:100%;height:100%;background:#15161a" alt="3D model"></model-viewer>' +
          '<div class="model3d-scalebar" style="display:none"><span class="scalebar-fill"></span>' +
          '<span class="scalebar-label"></span></div>';
        model3dApply();                                   // honour the current lighting controls
        const mv = stage.querySelector("model-viewer");
        const upd = () => updateScaleBar(mv, el);
        mv.addEventListener("load", () => {
          upd();
          try { el.querySelector(".model3d-size").textContent = "≈ " + fmtDims(mv.getDimensions()); } catch (_e) {}
        });
        mv.addEventListener("camera-change", upd);
      } else {
        stage.innerHTML = '<div class="model3d-loading">Couldn\'t load the 3D viewer (offline or CDN blocked). ' +
          '<a href="' + esc(url) + '" target="_blank" rel="noopener noreferrer">Open the file ↗</a></div>';
      }
    });
  }

  // ---- audio / video cells --------------------------------------------------
  // A direct media URL renders an inline native player. Audio loads on demand
  // (preload=none); video pulls only its metadata (a poster frame + duration).
  function looksAudioUrl(v) {
    return /^https?:\/\//i.test(v) && (
      /\.(mp3|wav|ogg|oga|flac|m4a|aac|opus)(\?|#|$)/i.test(v) ||
      // xeno-canto download links have no extension but serve audio/mpeg
      /xeno-canto\.org\/\d+\/download/i.test(v));
  }
  function looksVideoUrl(v) {
    return /^https?:\/\//i.test(v) && /\.(mp4|webm|ogv|m4v|mov)(\?|#|$)/i.test(v);
  }
  function audioCell(t) {
    const url = httpsUpgrade(t.value);
    return `<td class="iri media-cell"><audio class="cell-audio" controls preload="metadata" src="${esc(url)}"></audio>` +
      `<a class="media-src" href="${esc(url)}" target="_blank" rel="noopener noreferrer" title="${esc(t.value)}">↗</a>` +
      `<div class="media-meta" data-murl="${esc(url)}" data-mkind="audio"></div></td>`;
  }
  function videoCell(t) {
    const url = httpsUpgrade(t.value);
    return `<td class="iri media-cell"><video class="cell-video" controls preload="metadata" playsinline src="${esc(url)}"></video>` +
      `<div class="media-meta" data-murl="${esc(url)}" data-mkind="video"></div></td>`;
  }
  // A pre-rendered turntable spin (bucket `*-spin/<id>.webm`): a tiny looping clip
  // that auto-plays muted like a GIF — a lightweight preview that needs no WebGL.
  function looksSpinUrl(v) {
    return /^https?:\/\//i.test(v) && /-spin\/[^?#]*\.(webm|mp4)(\?|#|$)/i.test(v);
  }
  function spinCell(t) {
    const url = httpsUpgrade(t.value);
    return `<td class="iri media-cell"><video class="cell-video cell-spin" autoplay muted loop playsinline preload="metadata" src="${esc(url)}"></video>` +
      `<div class="media-meta" data-murl="${esc(url)}" data-mkind="video"></div></td>`;
  }

  // ---- media metadata captions + 3D scale bar -------------------------------
  function fmtBytes(n) { n = +n; if (!n || n < 0) return ""; return n >= 1048576 ? (n / 1048576).toFixed(1) + " MB" : n >= 1024 ? Math.round(n / 1024) + " KB" : n + " B"; }
  function fmtDur(s) { s = Math.round(+s || 0); if (!s) return ""; return Math.floor(s / 60) + ":" + String(s % 60).padStart(2, "0"); }
  function fmtExt(url) { const m = /\.([a-z0-9]{2,5})(\?|#|$)/i.exec(String(url).split("?")[0]); return m ? m[1].toUpperCase() : ""; }
  // A friendly file-type label from a MIME type — used when the URL has no
  // extension (e.g. an xeno-canto `/download` that serves `audio/mpeg`).
  const MIME_TYPE = { "audio/mpeg": "MP3", "audio/mp3": "MP3", "audio/wav": "WAV", "audio/x-wav": "WAV", "audio/ogg": "OGG", "audio/flac": "FLAC", "audio/aac": "AAC", "audio/mp4": "M4A", "audio/webm": "WEBM", "video/mp4": "MP4", "video/webm": "WEBM", "video/ogg": "OGV", "video/quicktime": "MOV", "image/jpeg": "JPEG", "image/png": "PNG", "image/webp": "WEBP", "image/gif": "GIF", "image/svg+xml": "SVG", "model/gltf-binary": "GLB", "model/gltf+json": "GLTF" };
  function mimeToType(ct) { if (!ct) return ""; ct = String(ct).split(";")[0].trim().toLowerCase(); if (MIME_TYPE[ct]) return MIME_TYPE[ct]; const sub = ct.split("/")[1] || ""; return sub ? sub.replace(/^x-/, "").toUpperCase().slice(0, 5) : ""; }
  function fmtDims(d) { const mx = Math.max(d.x, d.y, d.z); const u = mx < 0.01 ? 1000 : mx < 1 ? 100 : 1, s = mx < 0.01 ? "mm" : mx < 1 ? "cm" : "m"; const r = (v) => (v * u).toFixed(mx * u < 10 ? 1 : 0); return r(d.x) + "×" + r(d.y) + "×" + r(d.z) + " " + s; }
  function niceSI(x) { if (!isFinite(x) || x <= 0) return 0; const p = Math.pow(10, Math.floor(Math.log10(x))); const f = x / p; return (f >= 5 ? 5 : f >= 2 ? 2 : 1) * p; }
  function fmtSI(m) { if (m >= 1) return (Number.isInteger(m) ? m : m.toFixed(m < 10 ? 1 : 0)) + " m"; if (m >= 0.01) return Math.round(m * 100) + " cm"; return Math.round(m * 1000) + " mm"; }
  // Fill the per-cell caption: format · (resolution / duration / real-size) · file size.
  // file size via a best-effort HEAD (CORS permitting); intrinsic dims from the element.
  function hydrateMediaMeta(scope) {
    (scope || document).querySelectorAll(".media-meta[data-murl]").forEach((box) => {
      const url = box.getAttribute("data-murl"), kind = box.getAttribute("data-mkind");
      box.removeAttribute("data-murl");
      const parts = []; const f = fmtExt(url); if (f) parts.push(f);
      const render = () => { box.textContent = parts.filter(Boolean).join(" · "); box.title = box.textContent; };
      render();
      fetch(url, { method: "HEAD" }).then((r) => {
        // file type: from the URL extension, else the Content-Type header — so a
        // no-extension media URL (e.g. xeno-canto /download) still shows "MP3".
        if (!f) { const ct = mimeToType(r.headers.get("content-type")); if (ct && parts[0] !== ct) parts.unshift(ct); }
        const b = fmtBytes(r.headers.get("content-length")); if (b) parts.push(b);
        render();
      }).catch(() => {});
      const cell = box.closest("td"); if (!cell) return;
      if (kind === "image") {
        const img = cell.querySelector("img");
        if (img) { const g = () => { if (img.naturalWidth) { parts.splice(1, 0, img.naturalWidth + "×" + img.naturalHeight); render(); } }; (img.complete && img.naturalWidth) ? g() : img.addEventListener("load", g, { once: true }); }
      } else if (kind === "video") {
        const v = cell.querySelector("video");
        if (v) v.addEventListener("loadedmetadata", () => { const ins = [v.videoWidth ? v.videoWidth + "×" + v.videoHeight : "", fmtDur(v.duration)].filter(Boolean); parts.splice(1, 0, ...ins); render(); }, { once: true });
      } else if (kind === "audio") {
        const a = cell.querySelector("audio");
        if (a) a.addEventListener("loadedmetadata", () => { const d = fmtDur(a.duration); if (d) { parts.splice(1, 0, d); render(); } }, { once: true });
      } else if (kind === "mesh") {
        const mv = cell.querySelector("model-viewer");
        if (mv) mv.addEventListener("load", () => { try { parts.splice(1, 0, fmtDims(mv.getDimensions())); render(); } catch (_e) {} }, { once: true });
      }
    });
  }
  // A real-world scale bar for the lightbox: pixels-per-metre from the live camera
  // (orbit radius + vertical FOV), so a round SI length (1 mm … 1 m) tracks zoom.
  function updateScaleBar(mv, el) {
    const wrap = el.querySelector(".model3d-scalebar"); if (!mv || !wrap) return;
    try {
      const orbit = mv.getCameraOrbit();
      const fov = (((mv.getFieldOfView && mv.getFieldOfView()) || 30)) * Math.PI / 180;
      const stage = el.querySelector(".model3d-stage"); const vh = (stage && stage.clientHeight) || 400;
      const pxPerM = (vh / 2) / (orbit.radius * Math.tan(fov / 2));
      if (!isFinite(pxPerM) || pxPerM <= 0) { wrap.style.display = "none"; return; }
      const L = niceSI(70 / pxPerM), w = Math.round(L * pxPerM);
      if (!L || w < 8 || w > 460) { wrap.style.display = "none"; return; }
      wrap.style.display = "";
      wrap.querySelector(".scalebar-fill").style.width = w + "px";
      wrap.querySelector(".scalebar-label").textContent = fmtSI(L);
    } catch (_e) { wrap.style.display = "none"; }
  }

  // The default per-value heuristic (the behaviour for type "auto").
  function autoCell(t, raw) {
    if (t.iri) {
      const disp = shorten(t.value, 96);
      if (looksImageUrl(t.value)) return imageCell(t); // a Commons file or *.jpg/png/…
      if (looksIiifUrl(t.value)) return iiifCell(t);    // a IIIF manifest → fetch + show its thumbnail
      if (looksMeshUrl(t.value)) return mesh3dCell(t);  // a streamable mesh → inline 3D viewer
      if (looks3dViewerUrl(t.value)) return viewer3dCell(t); // a 3D viewer page → open in a tab
      if (looksAudioUrl(t.value)) return audioCell(t);  // a media file → inline player
      if (looksSpinUrl(t.value)) return spinCell(t);    // a pre-rendered turntable → looping spin
      if (looksVideoUrl(t.value)) return videoCell(t);
      if (looksWebUrl(t.value)) return linkCell(t);     // a dereferenceable web URL
      return `<td class="iri"${disp !== t.value ? ` title="${esc(t.value)}"` : ""}>${esc(disp)}</td>`;
    }
    if (looksWktGeo(t.value)) return geoCell(t);          // a WKT geometry → mini-map locator
    const num = t.datatype && NUM_DT.test(t.datatype);
    const lang = t.lang ? ` <span class="t-lang">@${esc(t.lang)}</span>` : "";
    return `<td class="lit${num ? " num" : ""}" title="${esc(raw)}">${esc(shorten(t.value, 110))}${lang}</td>`;
  }
  // A table cell for an RDF term. `type` (from the column header dropdown) forces
  // the rendering; "auto"/undefined uses the heuristic.
  function prettyCell(raw, type) {
    if (raw == null || raw === "") return `<td></td>`;
    const t = parseTerm(raw);
    switch (type) {
      case "text": {
        const lang = t.lang ? ` <span class="t-lang">@${esc(t.lang)}</span>` : "";
        return `<td class="lit" title="${esc(raw)}">${esc(shorten(t.value, 160))}${lang}</td>`;
      }
      case "image": return imageCell(t);
      case "iiif": return iiifCell(t);
      case "geo": return geoCell(t);
      case "model3d": return model3dCell(t);
      case "audio": return audioCell(t);
      case "video": return videoCell(t);
      case "spin": return spinCell(t);
      case "link": return linkCell(t);
      case "button": return buttonCell(t);
      case "number": {
        const n = Number(t.value);
        return `<td class="lit num" title="${esc(raw)}">${esc(isFinite(n) ? String(n) : t.value)}</td>`;
      }
      default: return autoCell(t, raw);
    }
  }

  // ---- per-column type override (the header dropdowns) -----------------------
  // Each rendered SELECT/triples table registers its data under a short id; a
  // column's dropdown stores a forced render type, and a delegated `change`
  // handler (see wireEvents) re-renders just that table in place.
  const COL_TYPES = [
    ["auto", "Auto"], ["text", "Text"], ["link", "Link"], ["button", "Button"],
    ["image", "Image"], ["iiif", "IIIF"], ["geo", "Map"], ["model3d", "3D"], ["audio", "Audio"], ["video", "Video"], ["spin", "Spin"], ["number", "Number"],
  ];
  const tableStates = new Map();
  let tableSeq = 0;

  function colTypeMenu(tid, col, cur) {
    cur = cur || "auto";
    const opts = COL_TYPES
      .map(([v, label]) => `<option value="${v}"${v === cur ? " selected" : ""}>${label}</option>`)
      .join("");
    return `<select class="coltype${cur !== "auto" ? " coltype-on" : ""}" data-tid="${tid}" ` +
      `data-col="${esc(col)}" title="Render this column as…" aria-label="Render type for column ${esc(col)}">${opts}</select>`;
  }

  // A friendly column header: a per-example custom label if given, else the
  // variable name prettified (camelCase / snake_case → "Title case"). The raw
  // variable name stays as the header's hover title so the query is still legible.
  function prettyColLabel(v) {
    return String(v).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ")
      .replace(/^./, (c) => c.toUpperCase());
  }
  function colLabel(st, v) { return (st.labels && st.labels[v]) || prettyColLabel(v); }

  // Build the inner HTML of a `.tbl` from its state (header dropdowns + collapsed
  // body), so the delegated change handler can rebuild it after a type switch.
  function tableInner(st) {
    const shown = st.rows.slice(0, st.cap);
    const head = `<tr>${st.vars
      .map((v) => `<th><div class="th-wrap"><span class="th-name" title="?${esc(v)}">${esc(shorten(colLabel(st, v), 24))}</span>${colTypeMenu(st.tid, v, st.types[v])}</div></th>`)
      .join("")}</tr>`;
    const rowHtmls = shown.map((row) => {
      geoRowCtx = row; // so a geo cell in this row can resolve its feature IRI
      const tr = `<tr>${st.vars.map((v) => prettyCell(row[v], st.types[v] || "auto")).join("")}</tr>`;
      geoRowCtx = null;
      return tr;
    });
    const hidden = Math.max(0, rowHtmls.length - TABLE_HEAD_ROWS);
    const body = rowHtmls
      .map((r, i) => (i < TABLE_HEAD_ROWS ? r : r.replace("<tr", `<tr class="tr-hidden"`)))
      .join("");
    return (st.note || "") +
      `<table><thead>${head}</thead><tbody>${body}</tbody></table>` +
      (hidden > 0
        ? `<button type="button" class="tbl-more secondary">Show ${Math.min(hidden, TABLE_MORE_STEP)} more (${hidden} hidden)</button>`
        : "");
  }

  // A collapsed table whose columns carry a type-override dropdown.
  function statefulTable(vars, rows, note) {
    const tid = "t" + ++tableSeq;
    const st = { tid, vars: vars || [], rows: rows || [], types: {}, cap: 500, note: note || "", labels: state.colLabels || null };
    tableStates.set(tid, st);
    // Bound the registry — only the few visible tables need their data kept.
    while (tableStates.size > 12) tableStates.delete(tableStates.keys().next().value);
    return `<div class="tbl" data-tid="${tid}">${tableInner(st)}</div>`;
  }

  function renderTable(vars, rows) {
    if (!(rows || []).length) return emptyState("rows");
    const note = (rows || []).length > 500
      ? `<p class="microcopy">Showing first 500 of ${rows.length} rows.</p>`
      : "";
    return statefulTable(vars || [], rows || [], note);
  }

  // ---- Cards view -------------------------------------------------------------
  // One card per result row, fields stacked label-over-value — phone-friendly
  // (media renders full-width), flowing as a CSS-columns masonry on wide screens.
  // Rendering reuses the table's cell machinery: `prettyCell` produces a `<td>`,
  // which is re-wrapped as the card field's value `<div>`, so every renderer
  // (image / IIIF / 3D / audio / geo / …) and the hydration observers work
  // unchanged. Per-field type overrides live in the SAME `st.types` map the
  // table header dropdowns use — the ⚙ Fields modal just edits it.
  const CARDS_HEAD = 24, CARDS_MORE_STEP = 48;
  function cellDiv(raw, type) {
    let h = prettyCell(raw, type);
    h = h.replace(/^<td(?=[\s>])/, "<div").replace(/<\/td>\s*$/, "</div>");
    return h.startsWith('<div class="')
      ? h.replace('<div class="', '<div class="cf-v ')
      : h.replace(/^<div/, '<div class="cf-v"');
  }
  // One card's field list. Shared by the cards grid and the focused, swipeable
  // single-card modal. `eager` forces this card's media to load now (grid cards
  // past the fold stay lazy; the focus modal always loads its media).
  function cardFieldsHtml(st, row, eager) {
    mediaEager = eager;
    geoRowCtx = row; // so a geo field in this card can resolve its feature IRI
    const fields = st.vars.map((v) => {
      const raw = row[v];
      if (raw == null || raw === "") return ""; // cards skip empty bindings
      return `<div class="cf"><div class="cf-k" title="?${esc(v)}">${esc(colLabel(st, v))}</div>${cellDiv(raw, st.types[v] || "auto")}</div>`;
    }).join("");
    geoRowCtx = null;
    mediaEager = false;
    return fields || `<div class="cf"><div class="cf-v">(empty row)</div></div>`;
  }
  function cardsInner(st) {
    const shown = st.rows.slice(0, st.cap);
    // Visible cards (before the "show more" fold) load their media eagerly so
    // every card on screen actually shows its photo; hidden ones stay lazy.
    // data-ci is the row index — tapping a card opens it in the focus modal.
    const cardHtmls = shown.map((row, i) =>
      `<article class="rcard" data-ci="${i}">${cardFieldsHtml(st, row, i < CARDS_HEAD)}</article>`
    );
    const hidden = Math.max(0, cardHtmls.length - CARDS_HEAD);
    const body = cardHtmls
      .map((c, i) => (i < CARDS_HEAD ? c : c.replace("<article class=\"rcard", "<article class=\"rcard rcard-hidden")))
      .join("");
    return (st.note || "") +
      `<div class="cards-bar"><button type="button" class="cards-fields secondary" data-tid="${st.tid}">⚙ Fields</button>` +
      `<span class="microcopy">${st.rows.length} card(s)</span></div>` +
      `<div class="cards-grid">${body}</div>` +
      (hidden > 0
        ? `<button type="button" class="cards-more secondary">Show ${Math.min(hidden, CARDS_MORE_STEP)} more (${hidden} hidden)</button>`
        : "");
  }
  function renderCards(vars, rows, note) {
    if (!(rows || []).length) return emptyState("rows");
    const tid = "t" + ++tableSeq;
    const st = { tid, kind: "cards", vars: vars || [], rows: rows || [], types: {}, cap: 500,
      note: note || ((rows || []).length > 500 ? `<p class="microcopy">Showing first 500 of ${rows.length} rows.</p>` : ""),
      labels: state.colLabels || null };
    tableStates.set(tid, st);
    while (tableStates.size > 12) tableStates.delete(tableStates.keys().next().value);
    return `<div class="cards" data-tid="${tid}">${cardsInner(st)}</div>`;
  }
  function renderCardsResult(res, progressive) {
    if (res.kind === "ask") {
      $("out").innerHTML = progressiveBanner(progressive) +
        `<div class="banner">ASK result: <strong>${esc(res.boolean)}</strong></div>`;
      return `ASK ${res.boolean}`;
    }
    if (res.kind === "select") {
      $("out").innerHTML = progressiveBanner(progressive) + renderCards(res.vars || [], res.rows || []);
      return `${(res.rows || []).length} row(s)`;
    }
    if (res.triples && res.triples.length) {
      // CONSTRUCT/DESCRIBE: same normalization as the triples table — one card
      // per triple, with the object field commonly the media one.
      const rows = res.triples.map((t) => ({ subject: t[0], predicate: t[1], object: t[2] }));
      $("out").innerHTML = renderCards(["subject", "predicate", "object"], rows);
      return `${res.triples.length} triple(s)`;
    }
    $("out").innerHTML = `<div class="note">Cards need result rows — run a <b>SELECT</b> (or a CONSTRUCT evaluated to triples).</div>`;
    return "cards: no rows";
  }
  function openCardsFields(tid) {
    const st = tableStates.get(tid);
    if (!st) return;
    $("cardsFieldsBody").innerHTML = st.vars.map((v) =>
      `<label class="cfm-row"><span class="cfm-name" title="?${esc(v)}">${esc(colLabel(st, v))}</span>` +
      `${colTypeMenu(tid, v, st.types[v])}</label>`).join("");
    $("cardsFieldsModal").classList.remove("hidden");
  }

  // Tap a card to open a swipeable carousel: every result row becomes a slide,
  // the current one centred with its neighbours peeking on the sides. Swiping
  // (native horizontal scroll-snap — momentum + snap for free) or ‹ ›/← → moves
  // between them. Media hydrates lazily per slide so a big result stays cheap.
  let cardFocus = null; // { tid, n, io } while open, null when closed
  function openCardFocus(tid, i) {
    const st = tableStates.get(tid);
    if (!st || !st.rows.length) return;
    const rows = st.rows.slice(0, st.cap);
    const track = $("cardFocusTrack");
    track.innerHTML = rows.map((row, k) =>
      // Lazy media (mediaEager=false): images self-lazy-load; IIIF/3D are
      // hydrated by the observer below only as a slide nears the viewport.
      `<article class="rcard cardfocus-slide" data-ci="${k}">${cardFieldsHtml(st, row, false)}</article>`
    ).join("");
    cardFocus = { tid, n: rows.length, io: null };
    // Hydrate a slide's IIIF/3D/media the moment it (or a neighbour) scrolls near.
    cardFocus.io = new IntersectionObserver((ents) => {
      ents.forEach((en) => {
        if (!en.isIntersecting) return;
        hydrateIiif(en.target); hydrateModel3d(en.target); hydrateMediaMeta(en.target);
        en.target.querySelectorAll("img.cell-thumb[loading='lazy']").forEach((im) => { im.loading = "eager"; });
        cardFocus.io.unobserve(en.target);
      });
    }, { root: track, rootMargin: "0px 150% 0px 150%" });
    $("cardFocusModal").classList.remove("hidden");
    document.body.classList.add("cardfocus-open");
    track.onscroll = onFocusScroll;
    centerFocusSlide(i, "instant"); // open on the tapped card
    // Observe + re-centre once the now-visible modal has laid out (slide widths
    // and offsets are only real after the modal stops being display:none).
    requestAnimationFrame(() => {
      if (!cardFocus) return;
      centerFocusSlide(i, "instant");
      [...track.children].forEach((s) => cardFocus.io.observe(s));
      updateFocusCount();
    });
    updateFocusCount();
  }
  function focusMetrics() { // pitch between slide centres + the first slide's centre
    const kids = $("cardFocusTrack").children;
    const first = kids[0];
    const pitch = kids.length > 1 ? kids[1].offsetLeft - first.offsetLeft
      : (first ? first.offsetWidth : 1);
    return { pitch: pitch || 1, firstCenter: first ? first.offsetLeft + first.offsetWidth / 2 : 0 };
  }
  function focusIndex() {
    if (!cardFocus) return 0;
    const track = $("cardFocusTrack");
    const { pitch, firstCenter } = focusMetrics();
    const viewCenter = track.scrollLeft + track.clientWidth / 2;
    return Math.max(0, Math.min(Math.round((viewCenter - firstCenter) / pitch), cardFocus.n - 1));
  }
  function centerFocusSlide(i, behavior) {
    const track = $("cardFocusTrack");
    i = Math.max(0, Math.min(i, cardFocus.n - 1));
    const slide = track.children[i];
    if (!slide) return;
    // Instant jump (programmatic smooth-scroll fights mandatory snap and is
    // flaky/paused off-screen); the .is-current scale/opacity transition gives
    // the visual cue, and a finger swipe animates natively via scroll momentum.
    track.scrollTo({ left: slide.offsetLeft - (track.clientWidth - slide.offsetWidth) / 2, behavior: behavior || "instant" });
    setFocusUi(i); // reflect the known target immediately (button nav)
  }
  // Update the counter, prev/next disabling, and which slide is "current" (the
  // scale/opacity cue) for a given index.
  function setFocusUi(i) {
    if (!cardFocus) return;
    const kids = $("cardFocusTrack").children;
    for (let k = 0; k < kids.length; k++) kids[k].classList.toggle("is-current", k === i);
    $("cardFocusCount").textContent = `${i + 1} / ${cardFocus.n}`;
    $("cardFocusPrev").disabled = i <= 0;
    $("cardFocusNext").disabled = i >= cardFocus.n - 1;
  }
  let focusScrollRaf = 0;
  function onFocusScroll() { // native-swipe driven: derive the index from position
    if (focusScrollRaf) return;
    focusScrollRaf = requestAnimationFrame(() => { focusScrollRaf = 0; updateFocusCount(); });
  }
  function updateFocusCount() { if (cardFocus) setFocusUi(focusIndex()); }
  function stepCardFocus(d) {
    if (!cardFocus) return;
    centerFocusSlide(focusIndex() + d, "instant");
  }
  function closeCardFocus() {
    $("cardFocusModal").classList.add("hidden");
    document.body.classList.remove("cardfocus-open");
    if (cardFocus && cardFocus.io) cardFocus.io.disconnect();
    const track = $("cardFocusTrack");
    track.onscroll = null;
    track.innerHTML = ""; // release the slides' media
    cardFocus = null;
  }

  function renderTriplesTable(triples) {
    if (!(triples || []).length) return emptyState("triples");
    const note = (triples || []).length > 500
      ? `<p class="microcopy">Showing first 500 of ${triples.length} triples.</p>`
      : "";
    // Normalize to the same {var: value} row shape so the object column gets a
    // type dropdown too (commonly an image).
    const rows = (triples || []).map((t) => ({ subject: t[0], predicate: t[1], object: t[2] }));
    return statefulTable(["subject", "predicate", "object"], rows, note);
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

  // Decode N-Triples string escapes properly — \uXXXX and \UXXXXXXXX (ANY script:
  // accents, CJK, emoji…) plus \t \n \r \b \f \" \\ \' \/. Universal: data that
  // carries escapes (e.g. "Genève" → "Genève") renders right instead of the
  // old behaviour that just stripped the backslash ("Genu00E8ve").
  function ntUnescape(s) {
    if (s.indexOf("\\") < 0) return s;
    return s.replace(/\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|[\s\S])/g, (m, e) => {
      const h = e[0];
      if (h === "u" || h === "U") {
        try { return String.fromCodePoint(parseInt(e.slice(1), 16)); } catch (_e) { return m; }
      }
      switch (h) {
        case "t": return "\t"; case "n": return "\n"; case "r": return "\r";
        case "b": return "\b"; case "f": return "\f";
        case '"': return '"'; case "\\": return "\\"; case "/": return "/"; case "'": return "'";
        default: return h;
      }
    });
  }
  // Parse a SPARQL JSON term string into {iri, value, datatype, lang}.
  function parseTerm(v) {
    const s = String(v == null ? "" : v);
    if (s.startsWith("<") && s.endsWith(">")) return { iri: true, value: s.slice(1, -1) };
    const m = /^"((?:[^"\\]|\\.)*)"(?:\^\^<([^>]+)>|@([\w-]+))?$/s.exec(s);
    if (m) return { iri: false, value: ntUnescape(m[1]), datatype: m[2] || null, lang: m[3] || null };
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

  // Slippy-tile basemaps. {z}/{x}/{y} (and optional {s} subdomain) are filled per
  // tile; all serve standard Web-Mercator XYZ tiles with open CORS. "none" keeps
  // the offline equirectangular vector view (no network), the default.
  const BASEMAPS = [
    { id: "none",  label: "None (offline)" },
    { id: "osm",   label: "OpenStreetMap", url: "https://tile.openstreetmap.org/{z}/{x}/{y}.png", attr: "© OpenStreetMap contributors", max: 19 },
    { id: "light", label: "Carto Light",   url: "https://{s}.basemaps.cartocdn.com/light_all/{z}/{x}/{y}.png", sub: "abcd", attr: "© OpenStreetMap · © CARTO", max: 20 },
    { id: "dark",  label: "Carto Dark",    url: "https://{s}.basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png",  sub: "abcd", attr: "© OpenStreetMap · © CARTO", max: 20 },
    { id: "sat",   label: "Esri Satellite",url: "https://server.arcgisonline.com/ArcGIS/rest/services/World_Imagery/MapServer/tile/{z}/{y}/{x}", attr: "Imagery © Esri", max: 19 },
    { id: "topo",  label: "OpenTopoMap",   url: "https://{s}.tile.opentopomap.org/{z}/{x}/{y}.png", sub: "abc", attr: "© OpenTopoMap (CC-BY-SA)", max: 17 },
  ];
  const clampLat = (v) => Math.max(-85.0511, Math.min(85.0511, v));
  const lon2wx = (lon) => (lon + 180) / 360;                                    // 0..1
  const lat2wy = (lat) => { const r = clampLat(lat) * Math.PI / 180; return (1 - Math.log(Math.tan(r) + 1 / Math.cos(r)) / Math.PI) / 2; };

  let lastMapRes = null; // kept so the basemap dropdown re-renders without re-querying

  function renderMap(res) {
    lastMapRes = res;
    if (res.kind !== "select") { $("out").innerHTML = note("Map needs SELECT rows with a geometry column."); return "no map"; }
    const vars = res.vars || [], rows = res.rows || [];
    const geo = detectGeoCol(vars, rows);
    if (!geo) { $("out").innerHTML = note("No geometry in this result — Map needs a WKT column (geo:wktLiteral: POINT / LINESTRING / POLYGON …)."); return "no geometry"; }
    const labelCols = vars.filter((v) => v !== geo);
    const feats = [];
    let minX = 180, maxX = -180, minY = 90, maxY = -90, n = 0;
    for (const r of rows) {
      const wkt = parseTerm(r[geo]).value; if (!WKT_RE.test(wkt)) continue;
      const rings = wktRings(wkt); if (!rings.length) continue;
      const isPoly = /POLYGON/i.test(wkt);
      // the tooltip value = every non-geometry column, so a year/name/count shows
      const label = (labelCols.length ? labelCols : [geo])
        .map((v) => termLabel(parseTerm(r[v]))).filter((s) => s !== "").join(" · ");
      feats.push({ rings, isPoly, label });
      for (const ring of rings) for (const [x, y] of ring) {
        if (x < minX) minX = x; if (x > maxX) maxX = x; if (y < minY) minY = y; if (y > maxY) maxY = y; n++;
      }
    }
    if (!feats.length) { $("out").innerHTML = note("No parseable geometry in this result."); return "no geometry"; }

    const W = 760, H = 420, pad = 14;
    const bmId = state.mapBasemap || "none";
    const base = BASEMAPS.find((b) => b.id === bmId) || BASEMAPS[0];
    const opts = BASEMAPS.map((b) => `<option value="${b.id}"${b.id === base.id ? " selected" : ""}>${esc(b.label)}</option>`).join("");
    const tools = `<div class="map-tools"><label class="map-base-pick">Basemap <select id="mapBasemap" aria-label="basemap">${opts}</select></label>` +
      `<button id="mapFullscreen" type="button" class="map-fs-btn" title="Open a full interactive map — pan and pinch/scroll to zoom">⛶ Explore full map</button></div>`;

    let px, py, bg = "", cap, hasBase = false;
    if (base.url) {
      // Web-Mercator: project to world pixels at a zoom that fits the bbox, then
      // lay the spanned XYZ tiles as SVG <image>s with the geometry drawn on top.
      hasBase = true;
      const padF = 0.12, maxZ = base.max || 19;
      const nx0 = lon2wx(minX), nx1 = lon2wx(maxX);
      const ny0 = lat2wy(maxY), ny1 = lat2wy(minY);            // north (maxY) → smaller y
      const fw = Math.max(nx1 - nx0, 1e-9), fh = Math.max(ny1 - ny0, 1e-9);
      const tiny = (maxX - minX) < 0.02 && (maxY - minY) < 0.02; // a single point / pin
      let z = Math.floor(Math.min(Math.log2((W * (1 - 2 * padF)) / (fw * 256)),
                                  Math.log2((H * (1 - 2 * padF)) / (fh * 256))));
      z = tiny ? 12 : Math.max(2, Math.min(maxZ, isFinite(z) ? z : maxZ));
      const wp = 256 * Math.pow(2, z), world = Math.pow(2, z);
      const oX = nx0 * wp - (W - fw * wp) / 2, oY = ny0 * wp - (H - fh * wp) / 2;
      px = (lon) => lon2wx(lon) * wp - oX;
      py = (lat) => lat2wy(lat) * wp - oY;
      const subs = base.sub ? base.sub.split("") : null;
      const tx0 = Math.floor(oX / 256), tx1 = Math.floor((oX + W) / 256);
      const ty0 = Math.floor(oY / 256), ty1 = Math.floor((oY + H) / 256);
      for (let tx = tx0; tx <= tx1; tx++) for (let ty = ty0; ty <= ty1; ty++) {
        if (ty < 0 || ty >= world) continue;                  // no vertical wrap
        const wx = ((tx % world) + world) % world;            // wrap longitude
        const u = base.url.replace("{z}", z).replace("{x}", wx).replace("{y}", ty)
                          .replace("{s}", subs ? subs[((tx % subs.length) + subs.length) % subs.length] : "");
        bg += `<image class="mtile" href="${esc(u)}" x="${(tx * 256 - oX).toFixed(1)}" y="${(ty * 256 - oY).toFixed(1)}" width="256" height="256" preserveAspectRatio="none" />`;
      }
      cap = `${feats.length} feature(s) · lon ${minX.toFixed(1)}…${maxX.toFixed(1)}, lat ${minY.toFixed(1)}…${maxY.toFixed(1)} · z${z} · ${base.attr}`;
    } else {
      // Offline: uniform-scale equirectangular auto-fit (no network).
      const dx = (maxX - minX) || 1, dy = (maxY - minY) || 1;
      const s = Math.min((W - 2 * pad) / dx, (H - 2 * pad) / dy);
      px = (x) => pad + (x - minX) * s + ((W - 2 * pad) - dx * s) / 2;
      py = (y) => H - pad - (y - minY) * s - ((H - 2 * pad) - dy * s) / 2; // invert lat
      cap = `${feats.length} feature(s) · lon ${minX.toFixed(1)}…${maxX.toFixed(1)}, lat ${minY.toFixed(1)}…${maxY.toFixed(1)} · equirectangular (offline)`;
    }

    // One <path> per feature — all of a (multi)polygon's rings become sub-paths of
    // a single element, so a 30-ring empire is 1 DOM node, not 30. Points stay as
    // circles. The label rides on data-label (read by one delegated handler) — no
    // per-ring <title>, whose native tooltip is both slow and a node each.
    let svg = "";
    for (const f of feats) {
      const lbl = esc(f.label);
      let d = "", dots = "";
      for (const ring of f.rings) {
        if (ring.length === 1) {
          const [x, y] = ring[0];
          dots += `<circle class="mpt" data-label="${lbl}" cx="${px(x).toFixed(1)}" cy="${py(y).toFixed(1)}" r="${hasBase ? 4 : 3}"/>`;
        } else {
          d += "M" + ring.map(([x, y]) => `${px(x).toFixed(1)} ${py(y).toFixed(1)}`).join("L") + (f.isPoly ? "Z" : "");
        }
      }
      if (d) svg += `<path class="${f.isPoly ? "mgeo" : "mline"}" data-label="${lbl}" d="${d}"/>`;
      svg += dots;
    }
    $("out").innerHTML = `<div class="mapview">${tools}` +
      `<svg class="${hasBase ? "has-base" : ""}" viewBox="0 0 ${W} ${H}" role="img" aria-label="map of results"><g class="mtiles">${bg}</g>${svg}</svg>` +
      `<div class="map-tooltip" hidden></div>` +
      `<div class="mapcap">${esc(cap)} — hover a feature for its value.</div></div>`;
    const sel = $("mapBasemap");
    if (sel) sel.addEventListener("change", () => {
      state.mapBasemap = sel.value;
      try { localStorage.setItem("mapBasemap", sel.value); } catch (e) { /* private mode */ }
      if (lastMapRes) renderMap(lastMapRes);
    });
    const fsBtn = $("mapFullscreen");
    if (fsBtn) fsBtn.onclick = () => openResultMap(lastMapRes);
    wireMapTooltip($("out").querySelector(".mapview"));
    return `${feats.length} mapped feature(s)`;
  }

  // ---- "Tiles" view: a PMTiles vector basemap PAIRED with rete (option B) ----------
  // A dataset may carry a PMTiles archive (tippecanoe, true per-zoom LOD, HTTP-range-
  // served) in CATALOG.pmtiles. The Tiles output renders ALL of its geometry as vector
  // tiles via protomaps-leaflet (Canvas on the Leaflet we already load — no WebGL), and
  // highlights the features the current SPARQL result names. Geometry rendering goes
  // through the tiles; rete stays the graph next to it, joined by the feature name.
  let tilesMap = null, protomapsP = null, tilesSeq = 0;
  function loadProtomaps() {
    if (protomapsP) return protomapsP;
    protomapsP = loadLeaflet().then(() => new Promise((resolve, reject) => {
      if (window.protomapsL) return resolve(window.protomapsL);
      const s = document.createElement("script");
      s.src = "https://unpkg.com/protomaps-leaflet@4.0.1/dist/protomaps-leaflet.js";
      s.onload = () => resolve(window.protomapsL);
      s.onerror = () => reject(new Error("protomaps-leaflet load failed"));
      document.head.appendChild(s);
    }));
    return protomapsP;
  }
  // ---- option C: tiles embedded INSIDE the .rete -----------------------------------
  // The standalone pmtiles library (PMTiles + a custom-Source interface); only needed
  // for the embedded case — the separate-.pmtiles case (B) uses protomaps-leaflet's URL.
  let pmtilesLibP = null;
  function loadPmtilesLib() {
    if (pmtilesLibP) return pmtilesLibP;
    pmtilesLibP = new Promise((resolve, reject) => {
      if (window.pmtiles) return resolve(window.pmtiles);
      const s = document.createElement("script");
      s.src = "https://unpkg.com/pmtiles@3.0.6/dist/pmtiles.js";
      s.onload = () => resolve(window.pmtiles);
      s.onerror = () => reject(new Error("pmtiles lib load failed"));
      document.head.appendChild(s);
    });
    return pmtilesLibP;
  }
  // Range-read a .rete file's 1 KB header and return the (offset, length) of its TILES
  // section (SectionKind 7), or null. The PMTiles archive lives INSIDE the .rete; this
  // finds where, so we can range-read tiles from the SAME url at that base (option C).
  async function reteTilesSection(reteUrl) {
    const r = await fetch(reteUrl, { headers: { Range: "bytes=0-1023" } });
    if (!r.ok && r.status !== 206) throw new Error("header read " + r.status);
    const dv = new DataView(await r.arrayBuffer());
    if (dv.getUint8(0) !== 0x52 || dv.getUint8(1) !== 0x45 || dv.getUint8(2) !== 0x54 || dv.getUint8(3) !== 0x45)
      throw new Error("not a .rete file");
    const sc = dv.getUint16(44, true);                 // section_count
    for (let i = 0; i < sc; i++) {
      const p = 64 + i * 24;                            // SECTION_DIR_OFFSET + i*ENTRY_LEN
      if (dv.getUint16(p, true) === 7) {               // SectionKind::Tiles
        return { offset: Number(dv.getBigUint64(p + 8, true)), length: Number(dv.getBigUint64(p + 16, true)) };
      }
    }
    return null;
  }
  // A pmtiles Source that range-reads an archive embedded inside a .rete at `base`:
  // every internal PMTiles offset is shifted into the .rete file.
  function reteSectionSource(reteUrl, base) {
    return {
      getKey: () => reteUrl + "#tiles@" + base,
      getBytes: async (offset, length, signal) => {
        const start = base + offset, end = start + length - 1;
        const r = await fetch(reteUrl, { headers: { Range: `bytes=${start}-${end}` }, signal });
        if (!r.ok && r.status !== 206) throw new Error("tiles range read " + r.status);
        return { data: await r.arrayBuffer() };
      },
    };
  }
  // The set of human names in a result (literal, non-geometry values) — the join key
  // to the tiles' shapeName/NAME, so result features light up on the basemap.
  function resultNameSet(res) {
    const out = new Set();
    for (const r of (res.rows || [])) for (const k in r) {
      const v = r[k]; if (typeof v !== "string" || !v || v[0] === "<") continue;
      const t = parseTerm(v); if (t.iri || WKT_RE.test(t.value)) continue;
      out.add(t.value);
    }
    return out;
  }
  // The lon/lat bbox of a result's geometry column, to fit the tile map to it.
  function resultBbox(res) {
    const vars = res.vars || [], rows = res.rows || [];
    const geo = detectGeoCol(vars, rows); if (!geo) return null;
    let minX = 180, maxX = -180, minY = 90, maxY = -90, found = false;
    for (const r of rows) {
      const t = parseTerm(r[geo] || ""); if (!t || !WKT_RE.test(t.value)) continue;
      for (const ring of wktRings(t.value)) for (const [x, y] of ring) {
        if (x < minX) minX = x; if (x > maxX) maxX = x; if (y < minY) minY = y; if (y > maxY) maxY = y; found = true;
      }
    }
    return found ? { minX, maxX, minY, maxY } : null;
  }
  function renderTiles(res) {
    const pm = CATALOG.pmtiles && CATALOG.pmtiles[state.dataset];
    if (!pm) {
      const avail = Object.keys(CATALOG.pmtiles || {}).join(", ") || "none";
      $("out").innerHTML = note(`The Tiles view renders a PMTiles vector basemap paired with the dataset. This one has none — available for: ${avail}.`);
      return "no vector tiles";
    }
    const mySeq = ++tilesSeq;
    const names = resultNameSet(res), bb = resultBbox(res);
    $("out").innerHTML = `<div class="tilesview"><div id="tilesMap" class="tiles-map"></div>` +
      `<div class="mapcap" id="tilesCap">Loading vector tiles (PMTiles)…</div></div>`;
    loadProtomaps().then(async (P) => {
      if (tilesSeq !== mySeq) return;                 // view switched away
      const L = window.L, mapDiv = document.getElementById("tilesMap");
      if (!L || !mapDiv) return;
      // Resolve the tile source: a PMTiles archive embedded INSIDE the .rete (option C —
      // one file = graph + tiles), or a separate .pmtiles URL (option B).
      let source = pm.url;
      if (pm.embedded) {
        const pmLib = await loadPmtilesLib();
        if (tilesSeq !== mySeq) return;
        const sec = await reteTilesSection(pm.url);    // pm.url = the .rete URL
        if (tilesSeq !== mySeq) return;
        if (!sec) throw new Error("no tiles section in the .rete");
        source = new pmLib.PMTiles(reteSectionSource(pm.url, sec.offset));
      }
      if (tilesMap) { tilesMap.remove(); tilesMap = null; }
      tilesMap = L.map(mapDiv, { scrollWheelZoom: true, minZoom: 0, maxZoom: 12, worldCopyJump: true }).setView([20, 0], 2);
      const poly = (fill, stroke, width, opacity) => new P.PolygonSymbolizer({ fill, stroke, width, opacity });
      const base = [
        { dataLayer: "countries", symbolizer: poly("#eaf0ed", "#b3c6bc", 0.8, 1) },
        { dataLayer: "regions", symbolizer: poly("rgba(0,0,0,0)", "#c9d6cf", 0.4, 1), minzoom: 3 },
        { dataLayer: "districts", symbolizer: poly("rgba(0,0,0,0)", "#dbe5e0", 0.25, 1), minzoom: 5 },
        { dataLayer: "places", symbolizer: new P.CircleSymbolizer({ radius: 1.6, fill: "#7f8f88" }), minzoom: 3 },
      ];
      // result features in accent, drawn over the base (one rule per polygon layer)
      const hit = (z, f) => f && f.props && names.has(f.props.shapeName);
      const hl = ["countries", "regions", "districts"].map((dl) => ({
        dataLayer: dl, symbolizer: poly("#147d69", "#0c5a4b", 1.6, 0.5), filter: hit,
      }));
      const layer = P.leafletLayer({ url: source, paintRules: base.concat(hl), maxDataZoom: 9, attribution: "© OpenStreetMap (geoBoundaries) · PMTiles" });
      layer.addTo(tilesMap);
      if (bb) { try { tilesMap.fitBounds([[bb.minY, bb.minX], [bb.maxY, bb.maxX]], { padding: [22, 22], maxZoom: 9 }); } catch (_e) {} }
      setTimeout(() => { if (tilesMap && tilesSeq === mySeq) tilesMap.invalidateSize(); }, 60);
      const cap = document.getElementById("tilesCap");
      if (cap) cap.textContent = pm.embedded
        ? `Vector tiles read from a section INSIDE this .rete (${pm.size}, kind-7 section) · ${names.size} result feature(s) highlighted · ONE immutable file = the RDF graph AND the map tiles, both HTTP-range-queryable. SPARQL hit the graph; this map range-read the tiles from the same file.`
        : `PMTiles vector basemap (${pm.label}, ${pm.size}) · ${names.size} result feature(s) highlighted · pan/zoom for ADM1→ADM2 detail (true per-zoom LOD). The tiles render the geometry; rete answers the query next to them.`;
    }).catch(() => { if (tilesSeq === mySeq) $("out").innerHTML = note("Couldn't load the vector-tile renderer (offline?)."); });
    return `vector tiles · ${names.size} feature(s) highlighted`;
  }

  // A snappy cursor-following tooltip for the map: one delegated mousemove on the
  // SVG, rAF-throttled, reading the hovered feature's data-label — instant, vs the
  // ~1s-delayed native <title> popup it replaces.
  function wireMapTooltip(mv) {
    if (!mv) return;
    const svgEl = mv.querySelector("svg"), tip = mv.querySelector(".map-tooltip");
    if (!svgEl || !tip) return;
    let pend = null, raf = 0;
    const apply = () => {
      raf = 0;
      if (!pend) { tip.hidden = true; return; }
      tip.textContent = pend.label;
      tip.hidden = false;
      const rect = mv.getBoundingClientRect();
      let lx = pend.x - rect.left + 14, ly = pend.y - rect.top + 14;
      if (lx + tip.offsetWidth + 6 > rect.width) lx = pend.x - rect.left - tip.offsetWidth - 14;
      if (ly + tip.offsetHeight + 6 > rect.height) ly = pend.y - rect.top - tip.offsetHeight - 14;
      tip.style.left = Math.max(2, lx) + "px";
      tip.style.top = Math.max(2, ly) + "px";
    };
    svgEl.addEventListener("mousemove", (ev) => {
      const el = ev.target.closest("[data-label]");
      pend = el ? { label: el.getAttribute("data-label"), x: ev.clientX, y: ev.clientY } : null;
      if (!raf) raf = requestAnimationFrame(apply);
    });
    svgEl.addEventListener("mouseleave", () => { pend = null; if (!raf) raf = requestAnimationFrame(apply); });
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
    if (fmt === "tiles") return renderTiles(res);
    if (fmt === "time") return renderTime(res);
    if (fmt === "cards") return renderCardsResult(res, progressive);

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
      // TTL / JSON-LD serialize an RDF graph, but a SELECT is a solution table —
      // there's no graph to write. Guide to CONSTRUCT/DESCRIBE instead of a table.
      if (fmt === "ttl" || fmt === "jsonld") {
        const name = fmt === "ttl" ? "Turtle" : "JSON-LD";
        $("out").innerHTML = `<div class="note">${name} serializes an <b>RDF graph</b>, but this query is a <b>SELECT</b> — it returns a solution table, not triples. Switch <b>Output</b> back to <b>Table</b>, or use a <b>CONSTRUCT</b> (or <b>DESCRIBE</b>) query to build a graph you can export as ${name}.</div>`;
        return `${(res.rows || []).length} row(s) · ${name} needs CONSTRUCT`;
      }
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
  function fedActive() { return state.fedSources.length > 0 || shardSources().length > 0; }

  // A SHARDED dataset (catalog `shards: [url0, url1, …]`) is one logical graph split
  // across independent .rete files (too big to build as one). shards[0] is the primary
  // (selfSource, via the dataset's url); the rest are intrinsic federation partners,
  // derived on the fly so every query fans across all shards (UNION) — the
  // sharded-rete model (one dataset bigger than any single file). resetFed never drops
  // them (they're the dataset, not user-added partners).
  function shardSources() {
    const d = datasetInfo(state.dataset);
    const sh = d && Array.isArray(d.shards) ? d.shards : null;
    if (!sh || sh.length < 2) return [];
    return sh.slice(1).map((u, i) => ({
      id: "shard" + (i + 1), kind: "remote", label: "shard " + (i + 1),
      url: u, key: state.dataset + "#s" + (i + 1), shard: true,
    }));
  }

  // The current dataset is always source #0, resolved at query time to whatever
  // it actually is — a lazy remote URL or the in-memory Graph handle.
  function selfSource() {
    const name = dsShortLabel(state.dataset) + " · this dataset";
    return state.remote
      ? { id: "self", kind: "remote", label: name, url: state.remote.url, self: true }
      : { id: "self", kind: "memory", label: name, self: true };
  }
  function allFedSources() { return [selfSource()].concat(shardSources()).concat(state.fedSources); }

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

  // ── Cross-source BGP join ────────────────────────────────────────────────
  // The union path (runFederatedUnion) runs the WHOLE query on each source and
  // merges. This instead decomposes ONE basic graph pattern ACROSS sources — by
  // predicate + variable provenance — and bound-joins them (VALUES injection), so
  // a query like `?p owl:sameAs ?wd . ?wd wdt:P19 ?birthplace` resolves ?p/?wd on a
  // .rete source and ?birthplace on a live Wikidata endpoint, joined on ?wd.
  // Proven end-to-end in dev/fedjoin.cjs. Parses flat BGPs only; bails (→ null,
  // → union fallback) on OPTIONAL/UNION/SERVICE/subselect/property-paths.
  const FED_TOK = /<[^>]*>|"(?:[^"\\]|\\.)*"(?:@[\w-]+|\^\^<[^>]*>)?|\?[A-Za-z0-9_]+|[A-Za-z0-9_]+:[A-Za-z0-9_./%~#-]*|[A-Za-z_][A-Za-z0-9_]*|[.;,]|\S/g;
  const fedIsVar = (t) => typeof t === "string" && t[0] === "?";
  const fedVn = (t) => t.slice(1);
  function fedParseTriples(where) {
    const toks = where.match(FED_TOK);
    if (!toks) return [];
    const out = []; let s = null, p = null, k = 0;
    for (const t of toks) {
      if (t === ".") { s = p = null; k = 0; continue; }
      if (t === ";") { p = null; k = 1; continue; }
      if (t === ",") { k = 2; continue; }
      if (k === 0) { s = t; k = 1; }
      else if (k === 1) { p = (t === "a") ? "rdf:type" : t; k = 2; }
      else out.push({ s, p, o: t });
    }
    return out;
  }
  function fedParse(q) {
    // strip SPARQL # comments — but not a # inside an <IRI> or a "literal"
    q = q.replace(/("(?:[^"\\]|\\.)*"|<[^>]*>)|#[^\n]*/g, (mm, keep) => keep || "");
    const prefixes = {}, prefixLines = []; let m;
    const re = /PREFIX\s+([A-Za-z0-9_-]*):\s*<([^>]*)>/gi;
    while ((m = re.exec(q))) { prefixes[m[1]] = m[2]; prefixLines.push(m[0]); }
    // `a` (rdf:type shorthand) becomes `rdf:type` in the generated per-source sub-BGPs,
    // so the sub-query needs the `rdf:` prefix DECLARED even if the user's query used `a`
    // and never wrote `PREFIX rdf:`. Without this the sub-query fails to parse, the join
    // throws, and it silently falls back to a UNION that matches nothing (0 rows).
    if (prefixes.rdf == null) {
      prefixes.rdf = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
      prefixLines.push("PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>");
    }
    const sm = /SELECT\s+(DISTINCT\s+|REDUCED\s+)?(.+?)\s+WHERE/is.exec(q);
    if (!sm) return null;
    const selVars = sm[2].trim() === "*" ? "*" : (sm[2].match(/\?[A-Za-z0-9_]+/g) || []).map(fedVn);
    if (selVars !== "*" && /\(/.test(sm[2])) return null;   // aggregates/expressions → union
    const wi = q.search(/WHERE\s*\{/i); if (wi < 0) return null;
    let i = q.indexOf("{", wi), depth = 0, end = -1;
    for (let j = i; j < q.length; j++) { if (q[j] === "{") depth++; else if (q[j] === "}") { depth--; if (!depth) { end = j; break; } } }
    if (end < 0) return null;
    let where = q.slice(i + 1, end);
    if (/\b(OPTIONAL|UNION|SERVICE|MINUS|GROUP\s+BY|BIND|VALUES)\b|\{/i.test(where)) return null;
    if (/[?\w):>]\s*[*+?]\s*[<?]/.test(where)) return null; // property paths
    const filters = [];
    where = where.replace(/FILTER\s*\(((?:[^()]|\([^()]*\))*)\)/gi, (mm, inner) => { filters.push("FILTER(" + inner + ")"); return " "; });
    const patterns = fedParseTriples(where);
    if (!patterns.length) return null;
    const lm = /\bLIMIT\s+(\d+)/i.exec(q);
    return { prefixes, prefixBlock: prefixLines.join("\n"), distinct: !!sm[1], selVars, patterns, filters, limit: lm ? +lm[1] : null };
  }
  function fedExpandIri(t, prefixes) {
    if (t[0] === "<" && t.endsWith(">")) return t.slice(1, -1);
    const m = /^([A-Za-z0-9_-]*):([A-Za-z0-9_./%~#-]*)$/.exec(t);
    if (m && prefixes[m[1]] != null) return prefixes[m[1]] + m[2];
    if (t === "rdf:type") return "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
    return t;
  }
  const fedPatVars = (pat) => [pat.s, pat.p, pat.o].filter(fedIsVar).map(fedVn);
  function fedRoute(parsed, sources) {
    const can = (src, pred) => src.preds === null || (pred !== null && src.preds.has(pred));
    const owner = {}, assign = new Array(parsed.patterns.length).fill(-1);
    parsed.patterns.forEach((pat, idx) => {
      const pred = fedIsVar(pat.p) ? null : fedExpandIri(pat.p, parsed.prefixes);
      let si = -1;
      if (fedIsVar(pat.s) && owner[fedVn(pat.s)] != null && can(sources[owner[fedVn(pat.s)]], pred)) si = owner[fedVn(pat.s)];
      if (si < 0 && pred !== null) {
        const sp = sources.findIndex((s) => s.preds !== null && s.preds.has(pred));
        si = sp >= 0 ? sp : sources.findIndex((s) => s.preds === null);
      }
      if (si < 0) si = 0;
      assign[idx] = si;
      [pat.s, pat.o].forEach((tm) => { if (fedIsVar(tm) && owner[fedVn(tm)] == null) owner[fedVn(tm)] = si; });
    });
    return assign;
  }
  function fedHashJoin(left, right, keys) {
    const idx = new Map();
    for (const r of right) { const k = JSON.stringify(keys.map((v) => r[v])); if (!idx.has(k)) idx.set(k, []); idx.get(k).push(r); }
    const out = [];
    for (const l of left) for (const r of (idx.get(JSON.stringify(keys.map((v) => l[v]))) || [])) out.push(Object.assign({}, l, r));
    return out;
  }
  async function fedJoinExec(parsed, sources, runOnSource, cap) {
    cap = cap || 250;
    const assign = fedRoute(parsed, sources);
    const order = [], groups = new Map();
    assign.forEach((si, i) => { if (!groups.has(si)) { groups.set(si, []); order.push(si); } groups.get(si).push(parsed.patterns[i]); });
    if (order.length < 2) return null;
    let table = [{}], bound = new Set();
    for (const si of order) {
      const pats = groups.get(si);
      const gVars = [...new Set(pats.flatMap(fedPatVars))];
      const shared = gVars.filter((v) => bound.has(v));
      let sub = parsed.prefixBlock + "\nSELECT DISTINCT " + gVars.map((v) => "?" + v).join(" ") + " WHERE {\n" +
        pats.map((p) => `  ${p.s} ${p.p} ${p.o} .`).join("\n") + "\n";
      parsed.filters.forEach((f) => { const fv = (f.match(/\?[A-Za-z0-9_]+/g) || []).map((x) => x.slice(1)); if (fv.every((v) => gVars.includes(v))) sub += "  " + f + "\n"; });
      if (shared.length) {
        const tuples = [...new Set(table.map((r) => JSON.stringify(shared.map((v) => r[v]))))].slice(0, cap).map((s) => JSON.parse(s)).filter((t) => t.every((x) => x != null));
        if (!tuples.length) return { vars: parsed.selVars === "*" ? [] : parsed.selVars, rows: [], groups: order.length };
        sub += "  VALUES (" + shared.map((v) => "?" + v).join(" ") + ") {\n" +
          tuples.map((t) => "    (" + t.join(" ") + ")").join("\n") + "\n  }\n";
      }
      sub += "}";
      const rows = await runOnSource(sources[si], sub);
      table = shared.length ? fedHashJoin(table, rows, shared)
        : (table.length === 1 && !Object.keys(table[0]).length) ? rows
        : (() => { const o = []; for (const l of table) for (const r of rows) o.push(Object.assign({}, l, r)); return o; })();
      gVars.forEach((v) => bound.add(v));
      if (!table.length) break;
    }
    let res = table;
    if (parsed.selVars !== "*") res = res.map((r) => { const o = {}; parsed.selVars.forEach((v) => { if (r[v] != null) o[v] = r[v]; }); return o; });
    const seen = new Set(), uniq = [];
    for (const r of res) { const k = JSON.stringify(r); if (!seen.has(k)) { seen.add(k); uniq.push(r); } }
    res = uniq;
    if (parsed.limit) res = res.slice(0, parsed.limit);
    return { vars: parsed.selVars === "*" ? [...new Set(res.flatMap((r) => Object.keys(r)))] : parsed.selVars, rows: res, groups: order.length, assign };
  }
  // Distinct-predicate set per source (cached). Endpoints are wildcards (null).
  function fedSourcePreds(src) {
    if (src.kind === "endpoint") return Promise.resolve(null);
    if (src._preds) return Promise.resolve(src._preds);
    return querySource(src, "SELECT DISTINCT ?p WHERE { ?s ?p ?o }", "select").then((r) => {
      const set = new Set();
      (r.rows || []).forEach((row) => { const p = row.p; if (p && p[0] === "<") set.add(p.slice(1, -1)); });
      src._preds = set;
      return set;
    });
  }

  // Try the cross-source join; on any miss, fall back to the union behaviour.
  function runFederated(q, fmt) {
    const parsed = fedParse(q);
    if (!parsed || !state.fedSources.length) return runFederatedUnion(q, fmt);
    const sources = allFedSources();
    $("out").innerHTML = netSpinner("planning a cross-source join…");
    updateResultVisibility();
    const t0 = performance.now();
    Promise.all(sources.map((s) => fedSourcePreds(s).then((preds) => Object.assign({}, s, { preds })).catch(() => Object.assign({}, s, { preds: null }))))
      .then((withPreds) => {
        const assign = fedRoute(parsed, withPreds);
        if (new Set(assign).size < 2) return runFederatedUnion(q, fmt); // all one source → union
        const run = (src, sub) => querySource(src, sub, "select").then((r) => r.rows || []);
        return fedJoinExec(parsed, withPreds, run).then((out) => {
          if (!out) return runFederatedUnion(q, fmt);
          fedRenderJoin(out, parsed, q, fmt, withPreds, performance.now() - t0);
        });
      })
      .catch(() => runFederatedUnion(q, fmt));
  }
  function fedRenderJoin(out, parsed, q, fmt, sources, dt) {
    const merged = { kind: "select", vars: out.vars, rows: out.rows };
    const renderFmt = ROW_VIEWS.has(fmt) && fmt !== "graph" ? fmt : "table";
    const summary = renderResult(merged, renderFmt);
    const rows = parsed.patterns.map((p, i) =>
      `<tr><td class="num">${out.assign[i] != null ? esc(sources[out.assign[i]].label) : "?"}</td><td>${esc(p.s + " " + p.p + " " + p.o)}</td></tr>`).join("");
    const banner = `<div class="fed-banner"><div class="fed-banner-head">Cross-source JOIN — ${out.rows.length} joined row(s) across ${out.groups} sources</div>` +
      `<table><tbody>${rows}</tbody></table></div>`;
    $("out").innerHTML = banner + $("out").innerHTML;
    state.lastResult = { res: merged, rowShaped: true, q, strategy: "federated", remote: false, dataset: state.dataset, federated: true, fedBannerHtml: banner };
    $("qmeta").textContent = `${summary} · cross-source join · ${dt.toFixed(0)} ms`;
    updateResultVisibility();
  }

  function runFederatedUnion(q, fmt) {
    const kind = detectQueryKind(q);
    const sources = allFedSources();
    $("commOut").innerHTML = "";
    $("reqLogBtn").classList.add("hidden");
    // LIVE progress across the sub-queries — else a long multi-shard federation is a
    // silent spinner for minutes (the sub-queries serialize on the one worker). The
    // worker resets its request/byte counters per query (keyed by id), so we sum the
    // latest per id for the running total, and tick a "done/N sources · req · bytes ·
    // elapsed" line. remoteOnProgress is the same global hook runRemote uses.
    $("out").innerHTML =
      `<div class="range-read">` +
        netSpinner(`federating across ${sources.length} sources…`) +
        `<div class="cache-bar indeterminate"><div class="cache-bar-fill"></div></div>` +
        `<div class="range-read-meta" id="fedMeta"></div>` +
      `</div>`;
    updateResultVisibility();
    const t0 = performance.now();
    const perId = new Map();
    let done = 0;
    const paint = () => {
      const el = document.getElementById("fedMeta"); if (!el) return;
      let req = 0, bytes = 0;
      perId.forEach((v) => { req += v.requests; bytes += v.bytes; });
      el.textContent = `${done}/${sources.length} sources answered · ${req} range request(s) · ` +
        `${formatBytes(bytes)} · ${((performance.now() - t0) / 1000).toFixed(1)}s`;
    };
    const progTimer = setInterval(paint, 250);
    const prevProg = remoteOnProgress;
    remoteOnProgress = (m) => { perId.set(m.id, { requests: m.requests || 0, bytes: m.bytes || 0 }); paint(); };
    const cleanupProg = () => { clearInterval(progTimer); remoteOnProgress = prevProg; };
    paint();
    const jobs = sources.map((src) =>
      Promise.resolve().then(() => querySource(src, q, kind))
        .then((r) => { done++; paint(); return { src, r, ok: true }; })
        .catch((e) => { done++; paint(); return { src, ok: false, error: String((e && e.message) || e) }; }));
    Promise.all(jobs).then((settled) => {
      cleanupProg();
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
    // Live-endpoint mode replaces every other source chip: the endpoint is
    // deliberately the ONLY target (reads and writes both go to it).
    if (state.liveEndpoint) {
      chips.innerHTML =
        `<span class="fed-chip fed-live" title="Live SPARQL endpoint — the only query target; SPARQL Update enabled (e.g. rete serve). ${esc(state.liveEndpoint)}">` +
        `<span class="fed-chip-name">🔌 ${esc((() => { try { return new URL(state.liveEndpoint).host; } catch (e) { return shortUrlLabel(state.liveEndpoint); } })())}</span>` +
        `<span class="fed-chip-kind">live · editable</span>` +
        `<button type="button" class="fed-x" data-liveremove="1" title="Disconnect the live endpoint" aria-label="Disconnect live endpoint">×</button></span>`;
      const runb = $("run");
      if (runb && runb.textContent !== "Cancel") runb.textContent = "Run on endpoint";
      return;
    }
    const self = `<span class="fed-chip fed-self" title="The dataset selected above"><span class="fed-chip-name">${esc(dsShortLabel(state.dataset))}</span>` +
      `<span class="fed-chip-kind">${state.remote ? "lazy" : "in-memory"}</span></span>`;
    const extra = state.fedSources.map((s) =>
      `<span class="fed-chip"><span class="fed-chip-name" title="${esc(s.label)}">${esc(s.label)}</span>` +
      `<span class="fed-chip-kind">${s.kind === "remote" ? "lazy" : s.kind === "endpoint" ? "endpoint" : "in-memory"}</span>` +
      `<button type="button" class="fed-x" data-fedremove="${s.id}" title="Remove this source" aria-label="Remove ${esc(s.label)}">×</button></span>`).join("");
    const sh = shardSources();
    const shardChip = sh.length
      ? `<span class="fed-chip fed-shards" title="This dataset is ONE logical graph split across ${sh.length + 1} independent .rete shards (too big to build as one file). Every query fans across all of them (UNION) and the rows are merged.">` +
        `<span class="fed-chip-name">⛓ ${sh.length + 1} shards</span><span class="fed-chip-kind">federated</span></span>`
      : "";
    chips.innerHTML = self + shardChip + extra +
      (fedActive() ? `<button type="button" class="fed-plan" id="fedPlanBtn" title="Dry run: preview WHICH source answers each triple pattern and the sub-queries that would run — without executing the join">🔍 Plan</button>` : "");
    const pb = $("fedPlanBtn"); if (pb) pb.onclick = () => fedDryRun($("q").value);
    const run = $("run");
    if (run && run.textContent !== "Cancel") run.textContent = fedActive() ? "Run federated" : "Run Query";
  }
  // Dry run: show the join plan (pattern → source routing + the per-source sub-queries)
  // WITHOUT executing the join. The only source contact is each source's predicate list
  // (metadata, cached and reused by the real run).
  function fedDryRun(q) {
    $("commOut").innerHTML = "";
    const parsed = fedParse(q);
    if (!parsed) {
      $("out").innerHTML = note("Not a flat BGP the cross-source planner handles (OPTIONAL / UNION / aggregates / property paths). Federation would run this as a UNION — the whole query on each source, results merged.");
      updateResultVisibility(); return;
    }
    const sources = allFedSources();
    $("out").innerHTML = netSpinner("planning the join (reading predicate lists)…");
    updateResultVisibility();
    Promise.all(sources.map((s) => fedSourcePreds(s).then((preds) => Object.assign({}, s, { preds })).catch(() => Object.assign({}, s, { preds: null }))))
      .then((withPreds) => {
        const assign = fedRoute(parsed, withPreds);
        const nSrc = new Set(assign).size;
        const routeRows = parsed.patterns.map((p, i) =>
          `<tr><td>${esc(withPreds[assign[i]].label)}</td><td><code>${esc(p.s + " " + p.p + " " + p.o)}</code></td></tr>`).join("");
        const order = [], groups = new Map();
        assign.forEach((si, i) => { if (!groups.has(si)) { groups.set(si, []); order.push(si); } groups.get(si).push(parsed.patterns[i]); });
        let bound = new Set(), subs = "";
        for (const si of order) {
          const pats = groups.get(si);
          const gVars = [...new Set(pats.flatMap(fedPatVars))];
          const shared = gVars.filter((v) => bound.has(v));
          let sub = parsed.prefixBlock + "\nSELECT DISTINCT " + gVars.map((v) => "?" + v).join(" ") + " WHERE {\n" +
            pats.map((p) => `  ${p.s} ${p.p} ${p.o} .`).join("\n") + "\n";
          parsed.filters.forEach((f) => { const fv = (f.match(/\?[A-Za-z0-9_]+/g) || []).map((x) => x.slice(1)); if (fv.every((v) => gVars.includes(v))) sub += "  " + f + "\n"; });
          if (shared.length) sub += "  VALUES (" + shared.map((v) => "?" + v).join(" ") + ") {  …keys bound by earlier sources, ≤250  }\n";
          sub += "}";
          subs += `<div class="fed-plan-step"><div class="fed-plan-src">${esc(withPreds[si].label)}${shared.length ? " · joins on " + shared.map((v) => "?" + v).join(", ") : " · seed (runs first)"}</div><pre>${esc(sub)}</pre></div>`;
          gVars.forEach((v) => bound.add(v));
        }
        const verdict = nSrc < 2
          ? `<p class="note">Every pattern routes to one source — this would run as a normal query / UNION, not a cross-source join.</p>`
          : `<p class="microcopy">Cross-source JOIN across <b>${nSrc}</b> sources — executed left-to-right (bound-join: each step's keys VALUES-injected into the next, ≤250 per hop). This is a preview; no join was run.</p>`;
        $("out").innerHTML = `<div class="fed-plan-out"><h4>Join plan — pattern routing</h4>${verdict}` +
          `<table class="fed-plan-route"><thead><tr><th>Source</th><th>Triple pattern</th></tr></thead><tbody>${routeRows}</tbody></table>` +
          (nSrc >= 2 ? `<h4>Sub-queries (one per source)</h4>${subs}` : "") + `</div>`;
        updateResultVisibility();
      })
      .catch((e) => { $("out").innerHTML = note("Plan failed: " + String((e && e.message) || e)); updateResultVisibility(); });
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
      // Live mode: the endpoint becomes the ONLY target with Update enabled,
      // instead of one more federated read source.
      if ($("fedEndpointLive") && $("fedEndpointLive").checked) {
        connectLiveEndpoint(ep);
        closeFedPop();
        return;
      }
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

  // --- live-endpoint mode --------------------------------------------------
  // Connect a SPARQL Protocol endpoint (a local `rete serve`, Fuseki, …) as
  // the ONLY query target, with SPARQL Update enabled: SELECT/ASK read it,
  // INSERT/DELETE/CLEAR write to it. Deep-linkable via #endpoint=<url>.
  function connectLiveEndpoint(url) {
    state.liveEndpoint = url;
    renderFedBar();
    updateHash();
    $("qmeta").textContent =
      "🔌 live endpoint connected — SELECT/ASK query it; INSERT DATA / DELETE … WHERE update it";
  }
  function disconnectLiveEndpoint() {
    state.liveEndpoint = null;
    renderFedBar();
    updateHash();
    $("qmeta").textContent = "";
  }

  /// Is the editor text a SPARQL *Update* (vs a query)? First keyword after
  /// the prologue (comments / PREFIX / BASE) decides.
  function isUpdateText(q) {
    let body = q.replace(/#[^\n]*/g, " ").trim();
    for (;;) {
      const m = body.match(/^(PREFIX\s+[A-Za-z0-9_.-]*:\s*<[^>]*>|BASE\s*<[^>]*>)\s*/i);
      if (!m) break;
      body = body.slice(m[0].length);
    }
    return /^(INSERT|DELETE|CLEAR|DROP|CREATE|LOAD|COPY|MOVE|ADD|WITH)\b/i.test(body);
  }

  async function runLiveEndpoint(q, fmt) {
    $("commOut").innerHTML = "";
    $("out").innerHTML = netSpinner("live endpoint…");
    updateResultVisibility();
    const t0 = performance.now();
    const ms = () => (performance.now() - t0).toFixed(0) + " ms";
    try {
      if (isUpdateText(q)) {
        const res = await fetch(state.liveEndpoint, {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: "update=" + encodeURIComponent(q),
        });
        if (!res.ok) {
          const detail = (await res.text()).slice(0, 300);
          throw new Error(`update rejected: HTTP ${res.status}${res.status === 401
            ? " — this endpoint guards updates with a Bearer token" : ""} ${detail}`);
        }
        $("out").innerHTML = note(
          "✓ update accepted by the live endpoint. Run a SELECT to see the new state — " +
          "or download the updated file from <code>…/snapshot.rete</code>.");
        $("qmeta").textContent = `✓ update applied · live endpoint · ${ms()}`;
        updateResultVisibility();
        return;
      }
      const sep = state.liveEndpoint.includes("?") ? "&" : "?";
      const res = await fetch(state.liveEndpoint + sep + "query=" + encodeURIComponent(q),
        { headers: { Accept: "application/sparql-results+json" } });
      if (!res.ok) throw new Error("HTTP " + res.status + " " + (await res.text()).slice(0, 300));
      const ct = res.headers.get("Content-Type") || "";
      if (ct.includes("n-triples")) {
        // CONSTRUCT/DESCRIBE arrive as N-Triples text — show them verbatim.
        const text = await res.text();
        $("out").innerHTML = `<pre>${esc(text)}</pre>`;
        $("qmeta").textContent =
          `${text.split("\n").filter(Boolean).length} triple(s) · live endpoint · ${ms()}`;
        updateResultVisibility();
        return;
      }
      const j = await res.json();
      let r;
      if (typeof j.boolean === "boolean") {
        r = { kind: "ask", boolean: j.boolean, vars: [], rows: [], triples: [] };
      } else {
        const vars = (j.head && j.head.vars) || [];
        const rows = ((j.results && j.results.bindings) || []).map((bnd) => {
          const o = {};
          vars.forEach((v) => { const t = endpointTerm(bnd[v]); if (t !== undefined) o[v] = t; });
          return o;
        });
        r = { kind: "select", vars, rows, triples: [] };
      }
      state.lastResult = { res: r, rowShaped: true, q, strategy: "endpoint", remote: false, dataset: "(live)" };
      const summary = renderResult(r, fmt === "graph" ? "table" : fmt);
      $("qmeta").textContent = `${summary} · live endpoint · ${ms()}`;
    } catch (e) {
      showError("out", "Live endpoint: " + (e.message || e));
      $("qmeta").textContent = "";
    }
  }

  function runQuery() {
    const q = $("q").value.trim();
    if (!q) return showError("out", "Enter a SPARQL query.");
    const fmt = $("fmt").value;
    // OWL 2 QL reasoning: rewrite the query to include subClassOf/subPropertyOf/
    // domain/range entailments (opt-in toggle; applies to the default strategy).
    const reason = !!($("owlReason") && $("owlReason").checked);
    // Live-endpoint mode overrides everything: one target, updates allowed.
    if (state.liveEndpoint) { runLiveEndpoint(q, fmt); return; }
    if (isUpdateText(q)) {
      return showError("out",
        "That's a SPARQL Update — the in-browser engine is read-only. Connect a live endpoint " +
        "(+ Add source → SPARQL endpoint → live mode; e.g. a local `rete serve data.rete`) to apply it.");
    }
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
        // Technical only — no dataset title (it's already named in the header/chip).
        $("qmeta").textContent = `⏳ querying · ${lastReq} request(s) · ` +
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
      // TTL / JSON-LD ask the worker to serialize (a CONSTRUCT carries res.text);
      // every other view wants table rows (graph/map/time derive from them).
      const remoteFmt = (fmt === "ttl" || fmt === "jsonld") ? fmt : "table";
      remoteSparql(state.remote.url, q, remoteFmt, reason).then((out) => {
        cleanup();
        state.lastRemoteLog = out.log || [];
        const res = JSON.parse(out.json);
        // Row-shaped unless we fetched a serialization — cache it so an Output
        // switch re-renders rather than re-runs.
        state.lastResult = { res, rowShaped: remoteFmt === "table", q, strategy: "remote", remote: true, dataset: state.dataset };
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
        } else if (/maximum call stack/i.test(msg)) {
          // We DID run the query — it reached this browser's WebAssembly
          // call-stack limit partway. iOS / iPad Safari (JavaScriptCore) sets
          // that limit far lower than desktop Chrome/Firefox, so a large or
          // structurally involved query can trip it on a phone while running
          // fine on a computer. Reset the (now-poisoned) instance and explain
          // plainly — no blaming the query's shape (an expensive but perfectly
          // ordinary query lands here too).
          resetRemoteWorker();
          $("qmeta").textContent = "reached this browser's limit";
          $("out").innerHTML =
            `<div class="note"><b>We ran this, but it reached what iPhone / iPad Safari can handle.</b> ` +
            `The query hit Safari's WebAssembly limit partway through — a limit on this device, not a problem with the query (it runs on a desktop browser).<br><br>` +
            `To fit it on a phone: a smaller <code>LIMIT</code>, a more selective pattern (a rarer value, or a country / year / type filter), or the <b>Progressive</b> strategy for overviews — or open this dataset on a desktop browser. ` +
            `The engine has been reset — run another query any time.</div>`;
        } else if (isEngineTrap(msg)) {
          // A wasm trap poisons the instance — rebuild a fresh worker either way.
          resetRemoteWorker();
          const meta = (CATALOG.datasetMeta && CATALOG.datasetMeta[state.dataset]) || {};
          const fileBytes = sizeToBytes(meta.size) || 0;
          // Only *call it* out-of-memory when memory is plausible — a large file
          // (≥150 MB). On a small dataset a trap is a real bug, not OOM: show the
          // raw error so it isn't hidden behind a misleading message.
          if (fileBytes >= 150e6) {
            $("qmeta").textContent = "out of memory";
            $("out").innerHTML =
              `<div class="note"><b>This query ran out of memory.</b> It needed more than the browser allows for a query over this remote dataset${meta.size ? ` (${esc(meta.size)})` : ""}. ` +
              `This is common on <b>iPhone / iPad Safari</b>, which caps per-tab memory well below a desktop browser.<br><br>` +
              `Try: a smaller <code>LIMIT</code>, a more selective pattern (a rarer value, or add a country / year / type filter), the <b>Progressive</b> strategy for counts/overviews, or open this dataset on a desktop browser. ` +
              `The engine has been reset — you can run another query now.</div>`;
          } else {
            $("qmeta").textContent = "";
            showError("out", "Query failed (engine reset — run again to retry): " + msg, e && e.stack);
          }
        } else {
          $("qmeta").textContent = "";
          showError("out", "Remote query failed: " + msg, e && e.stack);
        }
      });
      return;
    }

    if (!state.bytes) return showError("out", "Load a graph first.");
    // Defer the (synchronous) engine call one frame so the spinner paints first.
    setTimeout(() => runEmbeddedQuery(q, fmt, reason), 0);
  }

  function runEmbeddedQuery(q, fmt, reason) {
    const strategy = $("strategy").value;
    // graph / map / time / cards are renderings of SELECT bindings — ask the engine for table rows.
    const rowView = fmt === "graph" || fmt === "map" || fmt === "time" || fmt === "cards";
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
        raw = reason ? state.graph.query_reasoned(q, queryFmt) : state.graph.query(q, queryFmt);
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

  // SHACL output: validate once, then view the report as a paginated violations
  // TABLE (the default) or as RAW TEXT in a chosen serialization. Switching views
  // and paging never re-validate — paging slices the in-memory report client-side,
  // and each serialization is run at most once and cached.
  const SHACL_PAGE = 25;
  const shaclViewMode = () => state.shaclViewMode || "table";

  // Validate in one serialization (json for the table; the chosen format for text).
  // Local = synchronous wasm; remote = a single lazy range read. -> { text, meta }.
  function shaclRunFmt(shapes, fmt) {
    if (state.remote) {
      return remoteRead("shacl_url", [state.remote.url, shapes, null, fmt], $("shaclOut"),
        "Validating shapes over HTTP range…",
        "SHACL fetches only the shapes' target nodes and their triples — a selective lazy read.")
        .then((out) => ({ text: out.json, meta: "lazy · over HTTP range" }));
    }
    if (!state.bytes) return Promise.reject(new Error("Load a graph first."));
    const t0 = performance.now();
    const text = state.graph.shacl(shapes, null, fmt);
    return Promise.resolve({ text, meta: `${(performance.now() - t0).toFixed(1)} ms` });
  }

  function runShacl() {
    const shapes = $("shapeText").value.trim();
    if (!shapes) return showError("shaclOut", "Enter a SHACL shape.");
    // Fresh run — drop any cached report/text from the previous shapes.
    state.shaclState = { shapes, page: 0, report: null, meta: "", text: {} };
    renderShaclView();
  }

  // Render the active view, validating (and caching) only the serialization it needs.
  function renderShaclView() {
    const st = state.shaclState;
    if (!st) return;
    const fail = (e) => { $("shaclMeta").textContent = ""; showError("shaclOut", "Validation failed: " + String((e && e.message) || e)); };
    if (shaclViewMode() === "table") {
      if (st.report) { renderShaclTable(); return; }
      $("shaclMeta").textContent = "";
      shaclRunFmt(st.shapes, "json").then((r) => {
        try { st.report = JSON.parse(r.text); }
        catch (e) { showError("shaclOut", "Could not parse the validation report."); return; }
        st.meta = r.meta; st.page = 0; renderShaclTable();
      }).catch(fail);
    } else {
      const fmt = $("shaclFormat").value;
      const cached = st.text[fmt];
      if (cached) { renderShaclTextView(cached.text, fmt, cached.meta); return; }
      $("shaclMeta").textContent = "";
      shaclRunFmt(st.shapes, fmt).then((r) => { st.text[fmt] = r; renderShaclTextView(r.text, fmt, r.meta); }).catch(fail);
    }
  }

  function renderShaclTextView(text, fmt, meta) {
    $("shaclOut").innerHTML = `<pre>${esc(text)}</pre>`;
    const conforms = fmt === "json"
      ? /"conforms"\s*:\s*true/.test(text)
      : /^conforms:\s*true|sh:conforms\s+true/im.test(text);
    $("shaclMeta").textContent = `${conforms ? "conforms" : "report"} | ${meta}`;
    updateResultVisibility();
  }

  // The non-conforming report as a client-paginated table (focus / path / severity /
  // component / message), SHACL_PAGE results per page — Prev/Next just re-slice.
  function renderShaclTable() {
    const st = state.shaclState, report = st.report;
    if (report.conforms) {
      $("shaclOut").innerHTML = `<div class="banner">Conforms ✓ — no validation results.</div>`;
      $("shaclMeta").textContent = `conforms | ${st.meta || ""}`;
      updateResultVisibility();
      return;
    }
    const results = report.results || [];
    const total = results.length;
    const pages = Math.max(1, Math.ceil(total / SHACL_PAGE));
    st.page = Math.min(Math.max(st.page, 0), pages - 1);
    const start = st.page * SHACL_PAGE;
    const slice = results.slice(start, start + SHACL_PAGE);
    const localName = (s) => { s = String(s || ""); const m = s.split(/[#/]/); return m[m.length - 1] || s; };
    const rows = slice.map((r) =>
      `<tr><td class="iri">${esc(shorten(r.focusNode || ""))}</td>` +
      `<td class="iri">${esc(shorten(r.resultPath || ""))}</td>` +
      `<td>${esc(localName(r.resultSeverity || r.severity || ""))}</td>` +
      `<td>${esc(localName(r.sourceConstraintComponent || ""))}</td>` +
      `<td>${esc(shorten((r.resultMessage || r.messages || []).join(" "), 180))}</td></tr>`
    ).join("");
    const from = total ? start + 1 : 0, to = start + slice.length;
    $("shaclOut").innerHTML =
      `<div class="note">Does not conform — ${total.toLocaleString()} validation result(s).</div>` +
      `<div class="tbl"><table><thead><tr><th>focus</th><th>path</th><th>severity</th><th>component</th><th>message</th></tr></thead><tbody>${rows}</tbody></table></div>` +
      `<div class="entity-pager">` +
        `<button type="button" id="shaclPrev" class="secondary"${st.page <= 0 ? " disabled" : ""}>‹ Prev</button>` +
        `<span class="pager-info">${from.toLocaleString()}–${to.toLocaleString()} of ${total.toLocaleString()} · page ${(st.page + 1).toLocaleString()} / ${pages.toLocaleString()}</span>` +
        `<button type="button" id="shaclNext" class="secondary"${st.page + 1 >= pages ? " disabled" : ""}>Next ›</button>` +
      `</div>`;
    const prev = $("shaclPrev"), next = $("shaclNext");
    if (prev) prev.onclick = () => { st.page -= 1; renderShaclTable(); };
    if (next) next.onclick = () => { st.page += 1; renderShaclTable(); };
    $("shaclMeta").textContent = `${total.toLocaleString()} violation(s) | ${st.meta || ""}`;
    updateResultVisibility();
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

  function renderReach(results, reverse, metaSuffix) {
    const rows = results.map((r) => {
      if (r.error) return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td colspan="2">${esc(r.error)}</td></tr>`;
      const shown = (r.reached || []).slice(0, 250).map((x) => `<div class="iri">${esc(shorten(x, 90))}</div>`).join("");
      const more = r.count > 250 ? `<div class="microcopy">Showing first 250 of ${r.count}.</div>` : "";
      return `<tr><td class="iri">${esc(shorten(r.seed))}</td><td>${r.count}</td><td>${shown}${more}</td></tr>`;
    }).join("");
    $("reachMeta").textContent = `${results.length} seed(s) | ${reverse ? "reverse" : "forward"} | ${metaSuffix}`;
    $("reachOut").innerHTML = `<table><thead><tr><th>seed</th><th>count</th><th>reached</th></tr></thead><tbody>${rows}</tbody></table>`;
    updateResultVisibility();
  }
  function runReach() {
    const pred = $("reachPred").value.trim();
    const seeds = $("reachSeeds").value.split(",").map((s) => s.trim()).filter(Boolean);
    if (!pred || !seeds.length) return showError("reachOut", "Enter a predicate and at least one seed.");
    const reverse = $("reachReverse").checked;
    // Remote-lazy: walk the relation over HTTP range (reach_url, worker-routed).
    if (state.remote) {
      $("reachMeta").textContent = "";
      remoteRead("reach_url", [state.remote.url, pred, JSON.stringify(seeds), reverse], $("reachOut"),
        "Walking the relation over HTTP range…",
        "Reachability faults only the index tiles along the walk — a selective lazy read.")
        .then((out) => renderReach(JSON.parse(out.json), reverse, "lazy · over HTTP range"))
        .catch((e) => { $("reachMeta").textContent = ""; showError("reachOut", "Remote reach failed: " + String((e && e.message) || e)); });
      return;
    }
    if (!state.bytes) return showError("reachOut", "Load a graph first.");
    const t0 = performance.now();
    try {
      renderReach(JSON.parse(state.graph.reach(pred, JSON.stringify(seeds), reverse)), reverse, `${(performance.now() - t0).toFixed(1)} ms`);
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
    const subject = optText("whySubject");
    const predicate = optText("whyPredicate");
    const object = optText("whyObject");
    // Remote-lazy: why_url reports each matched triple's byte ranges, faulting
    // only the tiles it touches (worker-routed).
    if (state.remote) {
      $("whyMeta").textContent = "";
      remoteRead("why_url", [state.remote.url, subject || null, predicate || null, object || null], $("provOut"),
        "Tracing provenance over HTTP range…",
        "why_triples reports the byte ranges each matched triple was read from — a selective lazy read.")
        .then((out) => {
          const o = JSON.parse(out.json);
          renderProvenance(o);
          $("whyMeta").textContent = `${o.resultCount} match(es) | lazy · over HTTP range`;
          if (state.exploreReady) renderLayout();
          updateResultVisibility();
        })
        .catch((e) => { $("whyMeta").textContent = ""; showError("provOut", "Remote provenance failed: " + String((e && e.message) || e)); });
      return;
    }
    if (!state.bytes) return showError("provOut", "Load a graph first.");
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

  // ── Dataset builder ("Create a dataset" panel) ───────────────────────────
  // Assemble a brand-new .rete from pasted RDF (+ an optional ontology merged
  // into the same graph), attach a dataset card + example SPARQL/SHACL, build it
  // with the wasm engine, and persist the whole bundle to IndexedDB so it shows
  // up beside the bundled datasets and survives reloads — until the user deletes
  // it. Two export buttons let the user take the result out of the browser: the
  // raw .rete, and a JSON manifest (card + examples) to PR into the repo/plaza.

  const BUILD_FAMILY_DEFAULT = "Select";
  function buildFamilies() { return (CATALOG.families && CATALOG.families.length) ? CATALOG.families : ["Select"]; }

  function sanitizeKey(s) {
    return String(s || "").toLowerCase().normalize("NFKD")
      .replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48);
  }

  // The dataset-card form ↔ JSON code editor, kept in sync both ways. This flag
  // guards against the programmatic write of one side re-triggering the other.
  let cardSync = false;

  // Read the card FORM into the canonical card object (the shape the JSON mirror
  // shows). `key` is derived from the title when the key field is empty.
  function cardFromForm() {
    const title = ($("cardTitle").value || "").trim();
    const key = ($("cardKey").value || "").trim();
    return {
      key: sanitizeKey(key || title),
      title,
      icon: ($("cardIcon").value || "").trim(),
      license: ($("cardLicense").value || "").trim(),
      source: ($("cardSource").value || "").trim(),
      tags: ($("cardTags").value || "").split(",").map((s) => s.trim()).filter(Boolean),
      description: ($("cardDesc").value || "").trim(),
      provenance: ($("cardProvenance").value || "").trim()
    };
  }

  // Build's view of the card (adds the "Untitled graph" fallback + `desc` alias).
  function gatherCard() {
    const c = cardFromForm();
    return {
      title: c.title || c.key || "Untitled graph",
      key: c.key, icon: c.icon, license: c.license, source: c.source,
      tags: c.tags, desc: c.description, provenance: c.provenance
    };
  }

  // FORM → JSON: reflect the form into the code editor (unless the editor is the
  // one being typed into right now).
  function updateCardCode() {
    if (cardSync) return;
    const code = $("cardCode"); if (!code) return;
    cardSync = true;
    code.value = JSON.stringify(cardFromForm(), null, 2);
    code.classList.remove("invalid");
    setCardSyncMsg("in sync", false);
    cardSync = false;
  }

  // JSON → FORM: parse the editor and populate the form. Invalid JSON leaves the
  // form untouched and flags the editor.
  function applyCardCode() {
    if (cardSync) return;
    const code = $("cardCode"); if (!code) return;
    let obj;
    try { obj = JSON.parse(code.value || "{}"); }
    catch (e) { code.classList.add("invalid"); setCardSyncMsg("invalid JSON — form not updated", true); return; }
    if (obj === null || typeof obj !== "object" || Array.isArray(obj)) {
      code.classList.add("invalid"); setCardSyncMsg("expected a JSON object", true); return;
    }
    code.classList.remove("invalid");
    setCardSyncMsg("in sync", false);
    cardSync = true;
    const set = (id, v) => { const el = $(id); if (el && v != null) el.value = String(v); };
    if (obj.title != null) set("cardTitle", obj.title);
    if (obj.key != null) { set("cardKey", obj.key); const ck = $("cardKey"); if (ck) ck.dataset.auto = "0"; }
    set("cardIcon", obj.icon);
    set("cardLicense", obj.license);
    set("cardSource", obj.source);
    if (obj.tags != null) set("cardTags", Array.isArray(obj.tags) ? obj.tags.join(", ") : obj.tags);
    if (obj.description != null) set("cardDesc", obj.description);
    if (obj.provenance != null) set("cardProvenance", obj.provenance);
    cardSync = false;
  }

  function setCardSyncMsg(text, bad) {
    const el = $("cardCodeMsg"); if (!el) return;
    el.textContent = text; el.classList.toggle("invalid", !!bad);
  }

  // A card form field changed: keep the url-safe key auto-derived from the title
  // until the user edits the key, then mirror the form into the JSON.
  function onCardField(e) {
    const ck = $("cardKey");
    if (ck && e && e.target === $("cardTitle") && ck.dataset.auto !== "0") ck.value = sanitizeKey($("cardTitle").value);
    if (ck && e && e.target === ck) ck.dataset.auto = ck.value.trim() ? "0" : "1";
    updateCardCode();
  }

  // --- live RDF syntax validation for editors 1 (data) & 2 (ontology) ---------
  // The engine has no parse-only entry point, so validation = a trial build in
  // the chosen format (cheap for the small graphs the builder targets). It runs
  // debounced on every edit / format change; very large inputs are deferred to
  // the actual build to avoid rebuilding the whole graph on each keystroke.
  const VALIDATE_CAP = 3_000_000; // ~3 MB of text — above this, validate on build only
  let validateTimer = null;
  function firstLine(s) { return String(s).split("\n")[0].replace(/^Error:\s*/, "").slice(0, 140); }
  function setValidMsg(id, text, ok) {
    const el = $(id); if (!el) return;
    el.textContent = text || "";
    el.classList.toggle("valid", ok === true);
    el.classList.toggle("invalid", ok === false);
  }
  function validateSource(statusId, data, onto, fmt, merged) {
    if (!(data.trim() || onto.trim())) { setValidMsg(statusId, "", null); return; }
    if (!wasmReady || !RC()) { setValidMsg(statusId, "", null); return; }
    if ((data.length + onto.length) > VALIDATE_CAP) { setValidMsg(statusId, "large input — checked on build", null); return; }
    try {
      const info = JSON.parse(W().info(buildFromSources(data, onto, fmt)));
      setValidMsg(statusId, `✓ valid${merged ? " (merged)" : ""} · ${info.quads} triples`, true);
    } catch (e) {
      setValidMsg(statusId, "✗ " + firstLine(e), false);
    }
  }
  function runBuildValidation() {
    const fmt = $("buildFormat").value;
    const data = $("buildText").value || "";
    const onto = $("buildOntology").value || "";
    validateSource("buildDataValid", data, "", fmt, false);
    // Editor 2 validates in the same combination the build uses, so an ontology
    // that reuses the data's prefixes (Turtle) checks correctly.
    if (onto.trim()) validateSource("buildOntoValid", data, onto, fmt, !!data.trim());
    else setValidMsg("buildOntoValid", "", null);
  }
  function scheduleBuildValidation() { clearTimeout(validateTimer); validateTimer = setTimeout(runBuildValidation, 450); }

  // --- format conversion: JSON-LD / RDF/XML → N-Quads / N-Triples -------------
  // The wasm engine parses only nt/nq/ttl, so JSON-LD and RDF/XML are converted
  // in the browser (RDFConvert, rdfconv.js) before building. `toEngineText`
  // returns the converted text + the actual build format to hand to W().build.
  function RC() { return window.RDFConvert; }
  function toEngineText(text, fmt) {
    if (fmt === "jsonld") return { text: RC().jsonldToNQuads(text), fmt: "nq" };
    if (fmt === "rdfxml") return { text: RC().rdfxmlToNTriples(text, window.DOMParser), fmt: "nt" };
    return { text: text, fmt: fmt };
  }
  // Build the merged graph (data + optional ontology) in the chosen source
  // format. nt/nq/ttl sources are concatenated as text; jsonld/rdfxml sources
  // are each converted, then the resulting line-based forms are concatenated.
  function buildFromSources(data, onto, fmt) {
    const parts = [];
    let buildFmt = fmt;
    if (data.trim()) { const c = toEngineText(data, fmt); buildFmt = c.fmt; parts.push(c.text.replace(/\s+$/, "")); }
    if (onto.trim()) { const c = toEngineText(onto, fmt); buildFmt = c.fmt; parts.push(c.text.replace(/\s+$/, "")); }
    return W().build(parts.join("\n"), buildFmt);
  }

  // --- import a card / manifest JSON file into editor 3 (and step 4) ----------
  // Accepts either a flat card object {key,title,…} or a full manifest produced
  // by "Download manifest" (datasetMeta / datasetExtra / examples / shacl), in
  // which case the example rows are restored too.
  async function importCardFile(file) {
    if (!file) return;
    let obj;
    try { obj = JSON.parse(await file.text()); }
    catch (e) { setCardSyncMsg("invalid JSON file", true); return; }
    const isManifest = obj && (obj.datasetMeta || obj.datasetExtra || obj.examples || obj.shacl || obj.dataset);
    let card = obj;
    if (isManifest) {
      const key = obj.key || (obj.datasetMeta && Object.keys(obj.datasetMeta)[0]) ||
        (obj.dataset && obj.dataset.key) || (obj.examples && Object.keys(obj.examples)[0]) || "";
      const meta = (obj.datasetMeta && obj.datasetMeta[key]) || {};
      const extra = (obj.datasetExtra && obj.datasetExtra[key]) || {};
      const ds = obj.dataset || {};
      card = {
        key, title: ds.label || ds.title || key,
        icon: extra.icon || "", license: meta.license || "", source: meta.source || "",
        tags: extra.tags || [], description: ds.description || "", provenance: meta.provenance || ""
      };
      // Restore the example rows (step 4) from the manifest.
      const exs = (obj.examples && obj.examples[key]) || [];
      const shp = (obj.shacl && obj.shacl[key]) || [];
      if (exs.length || shp.length) {
        state.buildEx = exs.map((e) => ({ type: "sparql", family: e.family || BUILD_FAMILY_DEFAULT, view: e.view || "table", label: e.label || "", tip: e.tip || "", q: e.q || "" }))
          .concat(shp.map((s) => ({ type: "shacl", label: s.label || "", tip: s.tip || "", shape: s.shape || "" })));
        renderBuildExamples();
      }
    }
    const code = $("cardCode");
    if (code) { code.value = JSON.stringify(card, null, 2); applyCardCode(); }
    setCardSyncMsg(isManifest ? "imported manifest" : "imported card", false);
  }

  // Pull the current example-row DOM values back into state.buildEx (so a
  // re-render — adding/removing a row — never loses what was typed).
  function captureBuildExamples() {
    const rows = $$("#buildExamples .build-ex");
    rows.forEach((row, i) => {
      const e = state.buildEx[i]; if (!e) return;
      const v = (sel) => { const el = row.querySelector(sel); return el ? el.value : ""; };
      e.label = v(".bx-label"); e.tip = v(".bx-tip");
      if (e.type === "sparql") { e.family = v(".bx-family") || BUILD_FAMILY_DEFAULT; e.view = v(".bx-view") || "table"; e.q = v(".bx-q"); }
      else { e.shape = v(".bx-shape"); }
    });
  }

  function renderBuildExamples() {
    const box = $("buildExamples");
    if (!box) return;
    box.innerHTML = state.buildEx.map((e, i) => {
      const head = e.type === "sparql"
        ? `<span class="build-ex-kind">SPARQL</span>` +
          `<select class="bx-family">${buildFamilies().map((f) => `<option${f === e.family ? " selected" : ""}>${esc(f)}</option>`).join("")}</select>` +
          `<input class="bx-label" placeholder="Example title" value="${esc(e.label || "")}" />` +
          `<select class="bx-view"><option value="table"${e.view !== "graph" ? " selected" : ""}>table</option><option value="graph"${e.view === "graph" ? " selected" : ""}>graph</option></select>`
        : `<span class="build-ex-kind">SHACL</span>` +
          `<input class="bx-label" placeholder="Shape title" value="${esc(e.label || "")}" />`;
      const body = e.type === "sparql"
        ? `<textarea class="bx-q" spellcheck="false" placeholder="SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 25">${esc(e.q || "")}</textarea>`
        : `<textarea class="bx-shape" spellcheck="false" placeholder="@prefix sh: <http://www.w3.org/ns/shacl#> .\n[] a sh:NodeShape ; sh:targetClass ... .">${esc(e.shape || "")}</textarea>`;
      return `<div class="build-ex${e.type === "shacl" ? " shacl" : ""}" data-type="${e.type}">` +
        `<div class="build-ex-head">${head}<button type="button" class="build-ex-del" data-del="${i}" title="Remove">×</button></div>` +
        `<input class="bx-tip" placeholder="One-line description (the tip shown under the example)" value="${esc(e.tip || "")}" />` +
        body +
        `</div>`;
    }).join("");
    $$("#buildExamples .build-ex-del").forEach((b) => {
      b.onclick = () => { captureBuildExamples(); state.buildEx.splice(Number(b.dataset.del), 1); renderBuildExamples(); };
    });
  }

  function addBuildExample(type) {
    captureBuildExamples();
    state.buildEx.push(type === "shacl"
      ? { type: "shacl", label: "", tip: "", shape: "" }
      : { type: "sparql", family: BUILD_FAMILY_DEFAULT, view: "table", label: "", tip: "", q: "" });
    renderBuildExamples();
  }

  // --- user-dataset persistence (its own IndexedDB DB, kept separate from the
  // range/file cache DB so it never collides with the worker shim's v2 contract).
  const UDB = "playgroundDatasets", UDS = "datasets";
  function udbOpen() {
    return new Promise((res, rej) => {
      const r = indexedDB.open(UDB, 1);
      r.onupgradeneeded = () => { const db = r.result; if (!db.objectStoreNames.contains(UDS)) db.createObjectStore(UDS); };
      r.onsuccess = () => res(r.result);
      r.onerror = () => rej(r.error);
    });
  }
  function udbPut(rec) {
    return udbOpen().then((db) => new Promise((res, rej) => {
      const t = db.transaction(UDS, "readwrite"); t.objectStore(UDS).put(rec, rec.key);
      t.oncomplete = () => res(); t.onerror = () => rej(t.error);
    }));
  }
  function udbDelete(key) {
    return udbOpen().then((db) => new Promise((res) => {
      const t = db.transaction(UDS, "readwrite"); t.objectStore(UDS).delete(key);
      t.oncomplete = () => res(); t.onerror = () => res();
    }));
  }
  function udbAll() {
    return udbOpen().then((db) => new Promise((res) => {
      const out = []; const c = db.transaction(UDS).objectStore(UDS).openCursor();
      c.onsuccess = (e) => { const cur = e.target.result; if (cur) { out.push(cur.value); cur.continue(); } else res(out); };
      c.onerror = () => res(out);
    }));
  }

  // Splice a saved record into the live CATALOG so it behaves like a bundled
  // dataset (sidebar entry, card, example library), and stash its bytes.
  function mergeUserDataset(rec) {
    userBytes.set(rec.key, rec.bytes);
    const entry = { key: rec.key, label: rec.label, description: rec.description, custom: true };
    const i = CATALOG.datasets.findIndex((d) => d.key === rec.key);
    if (i >= 0) CATALOG.datasets[i] = entry; else CATALOG.datasets.push(entry);
    CATALOG.datasetMeta = CATALOG.datasetMeta || {};
    CATALOG.datasetExtra = CATALOG.datasetExtra || {};
    CATALOG.datasetMeta[rec.key] = rec.meta;
    CATALOG.datasetExtra[rec.key] = { icon: rec.extra.icon, tags: rec.extra.tags, custom: true };
    CATALOG.examples[rec.key] = rec.examples || [];
    CATALOG.shacl[rec.key] = rec.shacl || [];
  }
  function unmergeUserDataset(key) {
    userBytes.delete(key);
    const i = CATALOG.datasets.findIndex((d) => d.key === key);
    if (i >= 0) CATALOG.datasets.splice(i, 1);
    if (CATALOG.datasetMeta) delete CATALOG.datasetMeta[key];
    if (CATALOG.datasetExtra) delete CATALOG.datasetExtra[key];
    delete CATALOG.examples[key];
    delete CATALOG.shacl[key];
  }
  async function loadUserDatasets() {
    let recs = [];
    try { recs = await udbAll(); } catch (e) { return; }
    recs.sort((a, b) => (a.createdAt || 0) - (b.createdAt || 0));
    recs.forEach((r) => {
      if (!r || !r.key || !r.bytes) return;
      r.bytes = r.bytes instanceof Uint8Array ? r.bytes : new Uint8Array(r.bytes);
      mergeUserDataset(r);
    });
  }

  function setBuiltButtons(on) {
    ["buildOpen", "buildDownload", "buildManifest"].forEach((id) => { const b = $(id); if (b) b.disabled = !on; });
  }

  async function runBuild() {
    captureBuildExamples();
    const data = $("buildText").value || "";
    const onto = $("buildOntology").value || "";
    if (!data.trim() && !onto.trim()) return showError("buildOut", "Paste some RDF data first (step 1) — or open a file.");
    const card = gatherCard();
    if (!card.key) return showError("buildOut", "Give the dataset a title or a key first (step 3).");
    if (keyIsReserved(card.key)) return showError("buildOut", `The key “${esc(card.key)}” belongs to a bundled dataset — pick another in step 3.`);

    const fmt = $("buildFormat").value;
    const t0 = performance.now();
    let bytes, info;
    try {
      // Merge data + optional ontology into one graph (JSON-LD / RDF/XML are
      // converted to N-Quads / N-Triples first; nt/nq/ttl are concatenated).
      bytes = buildFromSources(data, onto, fmt);
      info = JSON.parse(W().info(bytes));
    } catch (e) {
      state.built = null; setBuiltButtons(false); $("buildMeta").textContent = "";
      return showError("buildOut", "Build failed: " + firstLine(e));
    }
    const dt = performance.now() - t0;

    const rec = {
      key: card.key,
      label: card.title,
      description: card.desc || "A custom graph built in the browser.",
      meta: {
        triples: info.quads,
        size: formatBytes(bytes.length),
        license: card.license,
        source: card.source,
        provenance: card.provenance
      },
      extra: { icon: card.icon || "📦", tags: card.tags },
      examples: state.buildEx.filter((e) => e.type === "sparql" && (e.q || "").trim())
        .map((e) => ({ family: e.family || BUILD_FAMILY_DEFAULT, label: e.label || "Example", view: e.view || "table", tip: e.tip || "", q: e.q })),
      shacl: state.buildEx.filter((e) => e.type === "shacl" && (e.shape || "").trim())
        .map((e) => ({ label: e.label || "Shape", tip: e.tip || "", shape: e.shape })),
      format: fmt,
      bytes,
      createdAt: Date.now(),
      custom: true
    };

    let saved = true;
    try { await udbPut(rec); } catch (e) { saved = false; }
    mergeUserDataset(rec);
    state.built = { bytes, key: rec.key, rec };
    setBuiltButtons(true);
    $("buildMeta").textContent = `${formatBytes(bytes.length)} · ${info.quads} triples · ${dt.toFixed(1)} ms`;
    $("buildOut").innerHTML =
      `<div class="banner">${saved ? "Saved" : "Built"} <strong>${esc(rec.key)}</strong> — ` +
        `${saved ? "stored in this browser and added to the dataset list." : "could not write to IndexedDB (private mode?), but it's loaded for this session."}</div>` +
      `<div class="metric-grid">` +
      metric("Triples", info.quads) +
      metric("Terms", info.terms) +
      metric("Pyramid levels", info.pyramidLevels) +
      metric("Examples", rec.examples.length + rec.shacl.length) +
      metric("Size", formatBytes(bytes.length)) +
      `</div>` +
      `<p class="microcopy">Open it in the console to query it, or export it: <strong>Download .rete</strong> for the file, ` +
      `<strong>Download manifest</strong> for the card + examples to PR into the repo or the plaza. ` +
      `In-browser builds write uncompressed sections (the wasm engine ships no zstd encoder); ` +
      `<code>rete build</code> on the CLI produces a smaller file from the same input.</p>`;
    renderBuildSaved();
    updateResultVisibility();
  }

  function triggerDownload(blob, filename) {
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url; a.download = filename;
    document.body.appendChild(a); a.click(); a.remove();
    URL.revokeObjectURL(url);
  }

  function downloadBuilt() {
    if (!state.built) return;
    triggerDownload(new Blob([state.built.bytes], { type: "application/octet-stream" }), state.built.key + ".rete");
  }

  // A self-describing JSON manifest: the catalog fragments (card + examples) plus
  // instructions to drop the sibling .rete into the repo/plaza. Mirrors the shape
  // of web/playground-src/catalog.js so the fragments paste straight in.
  function builtManifest() {
    const r = state.built && state.built.rec;
    if (!r) return null;
    return {
      "$schema": "rete-playground-dataset/v1",
      key: r.key,
      rete: { file: r.key + ".rete", bytes: r.bytes.length, format: r.format },
      dataset: { key: r.key, label: r.label, description: r.description },
      datasetMeta: { [r.key]: r.meta },
      datasetExtra: { [r.key]: { icon: r.extra.icon, tags: r.extra.tags } },
      examples: { [r.key]: r.examples },
      shacl: { [r.key]: r.shacl },
      instructions: [
        `Save the sibling ${r.key}.rete into web/ (or 'hf buckets cp' it to playground/${r.key}.rete for a remote-lazy dataset).`,
        `If embedding: add ("${r.key}", "${r.key}.rete") to DATASETS in scripts/build_playground.py.`,
        "Merge the dataset / datasetMeta / datasetExtra / examples / shacl fragments into web/playground-src/catalog.js.",
        "Run scripts/build_playground.py to regenerate docs/playground.html.",
        "To contribute to the plaza, include both this manifest and the .rete file in your PR."
      ]
    };
  }
  function downloadManifest() {
    const man = builtManifest();
    if (!man) return;
    triggerDownload(new Blob([JSON.stringify(man, null, 2)], { type: "application/json" }), state.built.key + ".manifest.json");
  }

  function openBuilt() {
    if (!state.built) return;
    loadDataset(state.built.key);
    setMode("sparql");
  }

  // The "Saved in this browser" list under the builder actions: every user
  // dataset with Open + Delete.
  function renderBuildSaved() {
    const box = $("buildSavedList");
    if (!box) return;
    const keys = [...userBytes.keys()];
    if (!keys.length) { box.innerHTML = ""; return; }
    box.innerHTML = `<h3>Saved in this browser · ${keys.length}</h3>` + keys.map((k) => {
      const m = (CATALOG.datasetMeta && CATALOG.datasetMeta[k]) || {};
      const ex = (CATALOG.datasetExtra && CATALOG.datasetExtra[k]) || {};
      const nEx = (CATALOG.examples[k] || []).length + (CATALOG.shacl[k] || []).length;
      const sub = [m.size, m.triples != null ? m.triples + " triples" : "", nEx ? nEx + " examples" : ""].filter(Boolean).join(" · ");
      return `<div class="saved-item">` +
        `<span class="saved-ico">${esc(ex.icon || "📦")}</span>` +
        `<div class="saved-main"><div class="saved-name">${esc(dsShortLabel(k))}</div><div class="saved-sub">${esc(sub)}</div></div>` +
        `<button type="button" class="secondary" data-load="${esc(k)}">Open</button>` +
        `<button type="button" class="ds-delete" data-del="${esc(k)}">Delete</button>` +
        `</div>`;
    }).join("");
    $$("#buildSavedList [data-load]").forEach((b) => { b.onclick = () => { loadDataset(b.dataset.load); setMode("sparql"); }; });
    $$("#buildSavedList [data-del]").forEach((b) => { b.onclick = () => deleteUserDataset(b.dataset.del); });
  }

  async function deleteUserDataset(key) {
    try { await udbDelete(key); } catch (e) { /* ignore */ }
    unmergeUserDataset(key);
    if (state.built && state.built.key === key) { state.built = null; setBuiltButtons(false); }
    if (state.dataset === key) loadDataset(CATALOG.defaultDataset);
    renderBuildSaved();
    if (!$("sourceModal").classList.contains("hidden")) {
      if (dsSelected === key) dsSelected = state.dataset;
      renderDsSidebar(); renderDsDetail(dsSelected);
    }
  }

  function resetBuilder() {
    setEd("buildText", "");
    setEd("buildOntology", "");
    ["cardTitle", "cardKey", "cardIcon", "cardLicense", "cardSource", "cardTags", "cardDesc", "cardProvenance"]
      .forEach((id) => { const el = $(id); if (el) el.value = ""; });
    const ck = $("cardKey"); if (ck) ck.dataset.auto = "1";
    state.buildEx = [];
    renderBuildExamples();
    state.built = null;
    setBuiltButtons(false);
    $("buildMeta").textContent = "";
    $("buildDataMeta").textContent = "";
    $("buildOut").innerHTML = "";
    const cc = $("cardCode"); if (cc) cc.classList.remove("invalid");
    setValidMsg("buildDataValid", "", null);
    setValidMsg("buildOntoValid", "", null);
    setCardSyncMsg("in sync", false);
    updateCardCode();
    updateResultVisibility();
  }

  async function loadBuildFile(file) {
    if (!file) return;
    try {
      const text = await file.text();
      setEd("buildText", text);
      const ext = (file.name.match(/\.(\w+)$/) || [])[1] || "";
      const fmt = { nq: "nq", nquads: "nq", ttl: "ttl", turtle: "ttl",
        jsonld: "jsonld", json: "jsonld", rdf: "rdfxml", owl: "rdfxml", xml: "rdfxml" }[ext.toLowerCase()] || "nt";
      $("buildFormat").value = fmt;
      // Seed an empty card from the file name on first open.
      const ct = $("cardTitle"), ck = $("cardKey");
      if (ct && !ct.value.trim()) { ct.value = file.name.replace(/\.(\w+)$/, ""); if (ck && ck.dataset.auto !== "0") ck.value = sanitizeKey(ct.value); updateCardCode(); }
      $("buildDataMeta").textContent = `${file.name} · ${formatBytes(file.size)} · ready to build`;
      runBuildValidation();
    } catch (e) {
      showError("buildOut", "File read failed: " + e.message);
    }
  }

  async function loadOntoFile(file) {
    if (!file) return;
    try {
      setEd("buildOntology", await file.text());
      // The Format select is shared by both editors. If the data editor is still
      // empty, adopt the ontology file's format so an ontology-only build works.
      const ext = (file.name.match(/\.(\w+)$/) || [])[1] || "";
      const fmt = { nq: "nq", nquads: "nq", ttl: "ttl", turtle: "ttl",
        jsonld: "jsonld", json: "jsonld", rdf: "rdfxml", owl: "rdfxml", xml: "rdfxml" }[ext.toLowerCase()];
      if (fmt && !($("buildText").value || "").trim()) $("buildFormat").value = fmt;
      runBuildValidation();
    } catch (e) { showError("buildOut", "Ontology read failed: " + e.message); }
  }

  // Turn a raw error into a friendly, personalized headline + what-to-do advice
  // + a tone. Transient/host hiccups reassure ("just try again"); engine snags ask
  // to retry-then-report; query mistakes nudge a fix without alarm.
  function classifyError(message) {
    const m = String(message || "");
    if (/could not determine length|short range|failed to fetch|networkerror|network error|load failed|status\s*0\b|status\s*5\d\d|timeout|connection|err_|range req|ignored Range/i.test(m)) {
      return { tone: "transient", emoji: "🔁", headline: "A hiccup with the remote connection",
        advice: "No worries — this is almost always a momentary blip on the dataset's host, not your query. Just run it again. If it keeps failing, give it a few seconds and retry; the technical details below are there if you need them." };
    }
    if (/runtimeerror|unreachable|null function|out of memory|memory access|table\.grow|rust_?panic|panicked|wasm/i.test(m)) {
      return { tone: "bug", emoji: "🐞", headline: "The engine tripped on this one",
        advice: "Try running it again first — these are often intermittent. If it keeps happening, it's a real bug and worth reporting: expand the technical details below, hit Copy, and send them to the developer. They contain everything needed to reproduce it — you don't need to do anything else." };
    }
    if (/^enter a |parse error|syntax|unexpected token|expected |no geometry|no temporal|no parseable|needs a |needs select/i.test(m)) {
      return { tone: "user", emoji: "✏️", headline: "Let's adjust the query", advice: m };
    }
    if (/load a graph|load a dataset|load a /i.test(m)) {
      return { tone: "user", emoji: "📂", headline: "Pick a dataset first", advice: m };
    }
    return { tone: "bug", emoji: "⚠️", headline: "That didn't go through",
      advice: "Give it another try. If it persists, open the technical details below, copy them, and share them with the developer so it can be fixed — no need to worry about wording it, the report has it all." };
  }

  // A copy-pasteable report with everything needed to reproduce: the error, the
  // dataset + load mode + source URL, the mode/strategy, the exact query, and the
  // environment. This is what the user hands the developer.
  function errorReport(message, tech) {
    const L = [];
    L.push("rete playground — error report");
    if (window.RETE_BUILD) L.push("build: " + window.RETE_BUILD);
    try { L.push("time: " + new Date().toISOString()); } catch (_e) { /* ignore */ }
    L.push("error: " + String(message || ""));
    const t = tech && String(tech);
    if (t && t !== String(message)) L.push("detail: " + t);
    L.push("dataset: " + (state.dataset || "?") + " · load: " + (state.activeSource || "?"));
    if (state.remote && state.remote.url) L.push("source: " + state.remote.url);
    L.push("mode: " + (state.mode || "?") + " · strategy: " + (($("strategy") && $("strategy").value) || "?"));
    const q = state.mode === "sparql" && $("q") ? ($("q").value || "").trim() : "";
    if (q) L.push("query:\n  " + q.replace(/\n/g, "\n  "));
    L.push("page: " + location.href);
    L.push("agent: " + navigator.userAgent);
    return L.join("\n");
  }

  function showError(targetId, message, tech) {
    const c = classifyError(message);
    const tech_html = c.tone === "user" ? "" :
      `<details class="err-tech"><summary>Technical details for the developer ` +
      `<button class="err-copy" type="button">Copy</button></summary>` +
      `<pre class="err-tech-body">${esc(errorReport(message, tech))}</pre>` +
      `<div class="err-tech-hint">How to reproduce: open this dataset, paste the query above, and run it.</div></details>`;
    $(targetId).innerHTML =
      `<div class="error-box err-${c.tone}">` +
      `<div class="err-headline"><span class="err-emoji">${c.emoji}</span>${esc(c.headline)}</div>` +
      `<div class="err-advice">${esc(c.advice)}</div>` +
      tech_html + `</div>`;
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
        if (h.dataset && h.dataset !== state.dataset && (RETE_DATASETS_B64[h.dataset] || userBytes.has(h.dataset))) loadDataset(h.dataset);
        setMode("sparql");
        closeHistory();
      };
    });
  }

  function updateHash() {
    const params = new URLSearchParams();
    params.set("dataset", state.dataset);
    if (state.liveEndpoint) params.set("endpoint", state.liveEndpoint);
    // Record HOW the dataset is loaded so a reload restores the same mode — a
    // remote-lazy graph is not embedded, so without this the deep link couldn't
    // tell it apart from a bundled one and fell back to the default dataset.
    const load = { remote: "lazy", cached: "cache", bundled: "bundled" }[state.activeSource];
    if (load) params.set("load", load);
    params.set("mode", state.mode);
    const q = $("q").value.trim();
    // Prefer a short link: if an unedited catalog example is active, share its
    // index (#…&ex=3) instead of the whole URL-encoded SPARQL. Fall back to the
    // full query when it was edited or is ad-hoc.
    const exList = examplesForDataset();
    const exi = state.selectedExample;
    if (exi != null && exi >= 0 && exList[exi] && (exList[exi].q || "").trim() === q) {
      params.set("ex", String(exi));
    } else if (q) {
      params.set("q", q);
    }
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

    // Phone: reclaim memory when the tab is backgrounded. iOS Safari is most
    // likely to jetsam a hidden tab, so freeing the wasm workers keeps the page
    // alive to return to. A short grace period avoids thrashing on a quick app
    // switch; coming back before it fires cancels the free.
    let mobileFreeTimer = null;
    document.addEventListener("visibilitychange", () => {
      if (!isPhoneView()) return;
      if (document.hidden) {
        if (mobileFreeTimer) clearTimeout(mobileFreeTimer);
        mobileFreeTimer = setTimeout(() => { mobileFreeTimer = null; freeMobileMemory(); }, 8000);
      } else if (mobileFreeTimer) {
        clearTimeout(mobileFreeTimer); mobileFreeTimer = null;
      }
    });
    // Copy button inside an error's "Technical details" expander: copy the report
    // (and don't let the click toggle the <details>).
    document.addEventListener("click", (e) => {
      const btn = e.target.closest && e.target.closest(".err-copy");
      if (!btn) return;
      e.preventDefault(); e.stopPropagation();
      const det = btn.closest(".err-tech");
      const pre = det && det.querySelector(".err-tech-body");
      const text = pre ? pre.textContent : "";
      const done = (ok) => { const o = btn.textContent; btn.textContent = ok ? "Copied ✓" : "Copy failed"; setTimeout(() => { btn.textContent = o; }, 1500); };
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text).then(() => done(true)).catch(() => done(false));
      } else { done(false); }
    });
    $("strategy").onchange = () => setStrategy($("strategy").value);
    // Switching the Output type re-renders the last result in the new view
    // (no re-run) when it can; otherwise it runs the query.
    $("fmt").onchange = onOutputTypeChange;
    // Phone: a sticky bottom Run bar (Run + an Output mirror) appears when the
    // real Run button scrolls out of view in SPARQL mode — tweak-and-rerun and
    // flip Table/Cards/Map without scrolling back up.
    {
      const mrb = $("mobileRunBar");
      if (mrb && window.matchMedia) {
        const mq = window.matchMedia("(max-width: 560px)");
        const mrbFmt = $("mrbFmt");
        mrbFmt.innerHTML = $("fmt").innerHTML; // same options, mirrored value
        // Scroll-driven (like the header condense), not an IntersectionObserver —
        // deterministic, and one rect read per scroll tick is cheap.
        const runOffScreen = () => {
          const r = $("run").getBoundingClientRect();
          return r.bottom <= 0 || r.top >= window.innerHeight;
        };
        // The bar is unwanted while browsing a tall result — it covers the very
        // images you're scrolling through. So mirror the mobile-browser toolbar:
        // while the Run button is off-screen, only SHOW the bar when the user
        // scrolls UP (heading back to edit/re-run) and HIDE it on scroll-down.
        let mrbLastY = window.scrollY;
        mrbUpdate = () => {
          const eligible = mq.matches && state.mode === "sparql" && runOffScreen();
          const y = window.scrollY, dy = y - mrbLastY;
          mrbLastY = y;
          let on = mrb.classList.contains("on");
          if (!eligible) on = false;          // Run in view (or wrong mode) → gone
          else if (dy > 4) on = false;        // scrolling down through results → hide
          else if (dy < -4) on = true;        // scrolling back up → reveal
          mrb.classList.toggle("on", on);
          mrb.setAttribute("aria-hidden", on ? "false" : "true");
          document.body.classList.toggle("mrb-open", on);
          if (on) mrbFmt.value = $("fmt").value;
        };
        window.addEventListener("scroll", mrbUpdate, { passive: true });
        window.addEventListener("resize", mrbUpdate, { passive: true });
        $("mrbRun").onclick = () => $("run").click();
        mrbFmt.onchange = () => { $("fmt").value = mrbFmt.value; onOutputTypeChange(); };
        $("fmt").addEventListener("change", () => { mrbFmt.value = $("fmt").value; });
      }
    }
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
      const live = e.target.closest("[data-liveremove]");
      if (live) return disconnectLiveEndpoint();
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
    // The dataset-header "ⓘ Details & source" button opens the panel (a modal on
    // phone/tablet); the backdrop closes it.
    { const b = $("dsDetailsBtn"); if (b) b.onclick = () => setLibCollapsed(false); }
    { const bd = $("libBackdrop"); if (bd) bd.onclick = () => setLibCollapsed(true); }

    // Phone: fold the Sources bar + secondary controls (Output / Strategy /
    // Labels / help / AI) into a ⚙ Settings modal, leaving a compact
    // Settings + Run row. Desktop keeps them inline (qs-controls is
    // display:contents there). The nodes are MOVED, not duplicated, so all their
    // existing wiring (by id) stays intact.
    (function setupQuerySettings() {
      const fedBar = $("fedBar"), qsControls = $("qsControls"), qsBody = $("qsBody"),
        modal = $("querySettingsModal");
      if (!fedBar || !qsControls || !qsBody || !modal || !window.matchMedia) return;
      const aiBtn = $("askAiBtn"), qsBtn2 = $("qsBtn"), edTools = $("edTools"), runBtn = $("run");
      const consoleControls = fedBar.parentNode, actionRow = qsControls.parentNode;
      const fedAnchor = fedBar.nextElementSibling;    // .action-row — restore before it
      const qsAnchor = qsControls.nextElementSibling; // #qmeta — restore before it
      const mq = window.matchMedia("(max-width: 560px)");
      const place = () => {
        if (mq.matches) {
          if (fedBar.parentNode !== qsBody) qsBody.appendChild(fedBar);
          if (qsControls.parentNode !== qsBody) qsBody.appendChild(qsControls);
          // Phone: Settings + SPARQL AI sit next to Find a term in the editor
          // toolbar (not in the bottom row / modal).
          if (qsBtn2 && edTools && qsBtn2.parentNode !== edTools) edTools.appendChild(qsBtn2);
          if (aiBtn && edTools && aiBtn.parentNode !== edTools) edTools.appendChild(aiBtn);
        } else {
          if (fedBar.parentNode !== consoleControls) consoleControls.insertBefore(fedBar, fedAnchor);
          if (qsControls.parentNode !== actionRow) actionRow.insertBefore(qsControls, qsAnchor);
          // Restore to the action row in original order: … AI, Settings, Run.
          if (aiBtn && runBtn && aiBtn.parentNode !== actionRow) actionRow.insertBefore(aiBtn, runBtn);
          if (qsBtn2 && runBtn && qsBtn2.parentNode !== actionRow) actionRow.insertBefore(qsBtn2, runBtn);
          modal.classList.add("hidden"); // never leave it open on desktop
        }
      };
      place();
      // Re-run after layout settles: if boot ran before the viewport width was
      // final (a wrong initial matchMedia read), this self-corrects. place() is
      // idempotent (it checks the parent before moving), so extra calls are free.
      if (typeof requestAnimationFrame === "function") requestAnimationFrame(place);
      setTimeout(place, 250);
      (mq.addEventListener ? mq.addEventListener.bind(mq, "change") : mq.addListener.bind(mq))(place);
      window.addEventListener("resize", place, { passive: true });
      const qsBtn = $("qsBtn");
      if (qsBtn) qsBtn.onclick = () => modal.classList.remove("hidden");
      $("qsClose").onclick = () => modal.classList.add("hidden");
      modal.addEventListener("click", (e) => { if (e.target === modal) modal.classList.add("hidden"); });
    })();
    // Close the dataset Load dropdown on any click outside it.
    document.addEventListener("click", (e) => {
      const menu = $("dsLoadMenu");
      if (menu && !e.target.closest(".ds-load")) menu.classList.add("hidden");
    });
    // Keep the top bar pinned; the dataset header sticks just below it and
    // condenses to a single line (title + metadata, no tagline) once scrolled —
    // BUT only when the main work area is tall enough to STAY scrolled after the
    // header shrinks. Condensing reclaims ~Δpx of height, which shortens the page;
    // if the content only just overflows the viewport, that shrink clamps scrollY
    // back across the trigger and the toggle oscillates (the flicker). `.workbench`
    // height is content-driven and does NOT change when the header condenses (it
    // only shifts up), so gating on it can't feed the loop — and when it's short,
    // the header simply never condenses.
    const dsHeader = document.querySelector(".ds-header");
    const topbar = document.querySelector(".topbar");
    const workbench = document.querySelector(".workbench");
    if (dsHeader) {
      const updateGeometry = () => {
        const tb = topbar ? topbar.offsetHeight : 0;
        dsHeader.style.top = tb + "px";
        // Expose the rail's top (both sticky headers + the console-shell's 12px top
        // padding) so the mode rail tracks the header as it condenses.
        document.documentElement.style.setProperty("--rail-top", tb + dsHeader.offsetHeight + 12 + "px");
      };
      const isPhone = () => !!(window.matchMedia && window.matchMedia("(max-width: 560px)").matches);
      const tagline = dsHeader.querySelector(".ds-tagline");
      let condensed = false;
      let enoughRoom = false;
      let saving = 0; // px the condense frees (the tagline) — sampled while expanded
      const measure = () => {
        if (isPhone()) {
          // Phone: condense as soon as you scroll, guarded only by "does the page
          // still scroll after the header shrinks?" — if condensing would make the
          // content fit the viewport, scrollY clamps to 0 and it'd un-condense (the
          // flicker). Compare the FULL (expanded) doc height against the viewport
          // plus what condensing frees. `saving` is the tagline height, sampled
          // while expanded so it's stable across the toggle.
          if (!condensed && tagline) saving = tagline.offsetHeight;
          const fullDocH = document.documentElement.scrollHeight + (condensed ? saving : 0);
          enoughRoom = (fullDocH - saving) > window.innerHeight + 16;
        } else {
          // Desktop: the work area must be ~1.7 viewports tall, so after the header
          // condenses there's still plenty of scroll room past the 10px trigger —
          // a page that only *just* overflows would clamp scrollY back across the
          // trigger and the toggle would oscillate. Measured off the workbench
          // (stable across condense), never the document height (which condense
          // itself changes).
          enoughRoom = !!workbench && workbench.offsetHeight > window.innerHeight * 1.7;
        }
      };
      const apply = () => {
        const y = window.scrollY;
        // Phone: condense the instant you scroll (hysteresis — expand only right
        // back at the top — so the near-top toggle can't chatter). Desktop keeps
        // the single 10px trigger, backed by the 1.7× room guard above.
        const want = isPhone()
          ? enoughRoom && (condensed ? y > 2 : y > 4)
          : enoughRoom && y > 10;
        if (want !== condensed) {
          condensed = want;
          dsHeader.classList.toggle("condensed", want);
          updateGeometry();
        }
      };
      measure();
      updateGeometry();
      apply();
      window.addEventListener("scroll", apply, { passive: true });
      window.addEventListener("resize", () => { measure(); updateGeometry(); apply(); }, { passive: true });
      // Re-measure once the condense/expand transition settles (final header height).
      dsHeader.addEventListener("transitionend", updateGeometry);
      // Rendering results changes the work-area height → re-check whether condensing
      // is allowed. This fires on real content/size changes, never on a condense
      // (the workbench's own height is unaffected), so it can't loop.
      if (workbench && "ResizeObserver" in window) {
        new ResizeObserver(() => { measure(); apply(); }).observe(workbench);
      }
    }
    $$("#exploreSeg button").forEach((btn) => {
      btn.onclick = () => setExploreView(btn.dataset.exp);
    });
    $("exampleSearch").oninput = renderExamples;
    $("urlLoad").onclick = loadFromUrl;
    $("fileInput").onchange = (e) => loadFromFile(e.target.files[0]);
    $("shareBtn").onclick = shareUrl;
    $("shaclRun").onclick = runShacl;
    // SHACL Table/Text view toggle — the format select only applies to Text view.
    $$("#shaclView button").forEach((b) => {
      b.onclick = () => {
        $$("#shaclView button").forEach((x) => x.classList.toggle("active", x === b));
        state.shaclViewMode = b.dataset.view;
        const lbl = $("shaclFormatLabel");
        if (lbl) lbl.classList.toggle("hidden", state.shaclViewMode !== "text");
        if (state.shaclState) renderShaclView();
      };
    });
    $("shaclFormat").onchange = () => { if (state.shaclState && shaclViewMode() === "text") renderShaclView(); };
    $("coherenceRun").onclick = runCoherence;
    $("reachRun").onclick = runReach;
    $("whyRun").onclick = runProvenance;
    $("buildRun").onclick = runBuild;
    $("buildDownload").onclick = downloadBuilt;
    $("buildManifest").onclick = downloadManifest;
    $("buildOpen").onclick = openBuilt;
    $("buildReset").onclick = resetBuilder;
    $("buildFile").onchange = (e) => loadBuildFile(e.target.files[0]);
    $("buildOntoFile").onchange = (e) => loadOntoFile(e.target.files[0]);
    $("buildFormat").onchange = scheduleBuildValidation;
    $("addSparqlEx").onclick = () => addBuildExample("sparql");
    $("addShaclEx").onclick = () => addBuildExample("shacl");
    // Dataset-card form ↔ JSON two-way sync: every card field mirrors into the
    // code editor; editing the JSON (or importing a card/manifest) drives the form.
    const ck = $("cardKey");
    if (ck) ck.dataset.auto = "1";
    ["cardTitle", "cardKey", "cardIcon", "cardLicense", "cardSource", "cardTags", "cardDesc", "cardProvenance"]
      .forEach((id) => { const el = $(id); if (el) el.oninput = onCardField; });
    const cardCode = $("cardCode");
    if (cardCode) cardCode.oninput = applyCardCode;
    const cardImport = $("cardImportFile");
    if (cardImport) cardImport.onchange = (e) => { importCardFile(e.target.files[0]); e.target.value = ""; };

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
    $("finderModal").addEventListener("click", (e) => {
      if (e.target === $("finderModal")) closeFinder();
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
      await clearRangeCache(); // "Clear all" wipes the persistent ranges too
      freeExploreEngines();
      renderCacheList();
      renderRangeCache();
      renderCacheCtl();
    };
    $("rangeCacheToggle").onchange = (e) => { setRangeCache(e.target.checked); renderRangeCache(); };
    $("clearRangeCacheBtn").onclick = async () => { await clearRangeCache(); renderRangeCache(); };
    { const ai = $("aiModelId"); if (ai) { ai.value = (() => { try { return localStorage.getItem("aiModelId") || ""; } catch (e) { return ""; } })();
      ai.onchange = () => { const v = ai.value.trim(); try { v ? localStorage.setItem("aiModelId", v) : localStorage.removeItem("aiModelId"); } catch (e) { /* private mode */ }
        llmLoaded = false; if (llmWorker) { llmWorker.terminate(); llmWorker = null; } }; } }
    { const b = $("askAiBtn"); if (b) b.onclick = openAiModal; }
    const parToggle = $("parallelToggle");
    if (parToggle) parToggle.onchange = (e) => setParallelParam(e.target.checked);
    const parWorkers = $("parallelWorkers");
    if (parWorkers) parWorkers.onchange = (e) => {
      const n = parseInt(e.target.value, 10);
      setParallelWorkers(isNaN(n) ? null : Math.max(1, Math.min(32, n)));
    };
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        $("strategyModal").classList.add("hidden");
        $("outputModal").classList.add("hidden");
        $("cardsFieldsModal").classList.add("hidden");
        $("querySettingsModal").classList.add("hidden");
        $("reqModal").classList.add("hidden");
        setLibCollapsed(true); // close the Details & source modal (phone/tablet)
        closeLibrary();
        closeHistory();
        closeSettings();
        closeSource();
        closeFinder();
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
     ["buildRun", "Build & save"]].forEach(([id, label]) => {
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

    // Column type dropdowns: pick a render type for a column (Auto/Text/Link/
    // Image/Number) and re-render just that table in place (delegated, so it
    // works for every dynamically-rendered SELECT/triples table).
    document.addEventListener("change", (e) => {
      const sel = e.target.closest && e.target.closest("select.coltype");
      if (!sel) return;
      const st = tableStates.get(sel.dataset.tid);
      if (!st) return;
      st.types[sel.dataset.col] = sel.value;
      // Re-render the owning view in place — a table, or a cards grid (whose
      // type selects live in the ⚙ Fields modal, so changes preview live).
      const wrap = document.querySelector(`.tbl[data-tid="${sel.dataset.tid}"], .cards[data-tid="${sel.dataset.tid}"]`);
      if (wrap) wrap.innerHTML = st.kind === "cards" ? cardsInner(st) : tableInner(st);
    });

    // Cards: the "Show more" reveal and the ⚙ Fields modal (delegated — cards
    // grids are rendered dynamically, like tables).
    document.addEventListener("click", (e) => {
      const more = e.target.closest && e.target.closest(".cards-more");
      if (more) {
        const wrap = more.closest(".cards");
        const hidden = $$(".rcard-hidden", wrap);
        hidden.slice(0, CARDS_MORE_STEP).forEach((c) => {
          c.classList.remove("rcard-hidden");
          // Force the just-revealed cards' images to load (they were lazy while
          // hidden) so they don't sit blank.
          $$("img.cell-thumb[loading='lazy']", c).forEach((im) => { im.loading = "eager"; });
        });
        const left = Math.max(0, hidden.length - CARDS_MORE_STEP);
        if (left === 0) more.remove();
        else more.textContent = `Show ${Math.min(left, CARDS_MORE_STEP)} more (${left} hidden)`;
        return;
      }
      const fields = e.target.closest && e.target.closest(".cards-fields");
      if (fields) { openCardsFields(fields.dataset.tid); return; }
      // Tap a card — but not a link, media, or control inside it — to open it
      // in the focus modal (swipeable single-card view).
      const card = e.target.closest && e.target.closest(".rcard");
      if (card && card.dataset.ci != null &&
          !(e.target.closest && e.target.closest("a, button, input, select, label, model-viewer, audio, video, .iiif-frame, .coltype"))) {
        const wrap = card.closest(".cards");
        if (wrap) openCardFocus(wrap.dataset.tid, +card.dataset.ci);
      }
    });
    $("cardsFieldsClose").onclick = () => $("cardsFieldsModal").classList.add("hidden");
    $("cardsFieldsModal").addEventListener("click", (e) => {
      if (e.target === $("cardsFieldsModal")) $("cardsFieldsModal").classList.add("hidden");
    });
    // Focus carousel: prev/next buttons, keyboard, and backdrop/✕ close.
    // Swiping is native — horizontal scroll-snap on the track (momentum + snap).
    $("cardFocusPrev").onclick = () => stepCardFocus(-1);
    $("cardFocusNext").onclick = () => stepCardFocus(1);
    $("cardFocusClose").onclick = closeCardFocus;
    $("cardFocusModal").addEventListener("click", (e) => {
      if (e.target === $("cardFocusModal")) closeCardFocus();
    });
    document.addEventListener("keydown", (e) => {
      if (!cardFocus) return;
      if (e.key === "Escape") closeCardFocus();
      else if (e.key === "ArrowLeft") stepCardFocus(-1);
      else if (e.key === "ArrowRight") stepCardFocus(1);
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
    // IIIF cells render their thumbnail asynchronously; hydrate them whenever a
    // result table appears or re-renders (debounced; processed-once per cell).
    try {
      const host = document.querySelector(".console-shell") || document.body;
      let pending = false;
      const obs = new MutationObserver(() => {
        if (pending) return;
        pending = true;
        setTimeout(() => { pending = false; hydrateIiif(host); hydrateModel3d(host); hydrateMediaMeta(host); }, 60);
      });
      obs.observe(host, { childList: true, subtree: true });
      // Delegated: the ⛶ on an inline 3D cell (or a 🧊 3D button) opens the full viewer.
      host.addEventListener("click", (e) => {
        const btn = e.target && e.target.closest && e.target.closest(".model3d-btn, .model3d-expand");
        if (btn) { e.preventDefault(); openModel3d(btn.getAttribute("data-mesh")); return; }
        const gc = e.target && e.target.closest && e.target.closest(".geo-cell[data-geo]");
        if (gc) { e.preventDefault(); openGeoModal(geoData[gc.getAttribute("data-geo")]); }
      });
    } catch (_e) { /* ignore */ }
    // The details panel (2nd sidebar) is a space-cramping overlay on a phone, so
    // it starts collapsed there regardless of the saved desktop preference; on
    // wider screens the saved preference wins.
    try {
      const narrow = window.matchMedia("(max-width: 860px)").matches;
      setLibCollapsed(narrow || localStorage.getItem(LIB_KEY) === "1");
    } catch (_e) { /* ignore */ }
    enhanceEditor("q", "sparql");
    enhanceEditor("shapeText", "ttl");
    enhanceEditor("buildText", "ttl", scheduleBuildValidation);
    enhanceEditor("buildOntology", "ttl", scheduleBuildValidation);
    setEd("buildText", BUILD_SAMPLE);
    renderBuildExamples();
    updateCardCode();

    // "Labels" decode switch — on by default; shows a human-label chip beside
    // each IRI in the query. The checkbox is `checked` in the HTML, so enable
    // decode to match on first mount.
    const decodeBtn = $("decodeToggle");
    if (decodeBtn && window.PlaygroundEditor) {
      decodeBtn.onchange = () => window.PlaygroundEditor.setDecode("q", decodeBtn.checked);
      if (decodeBtn.checked) window.PlaygroundEditor.setDecode("q", true);
    }
    // Find-a-term modal: a button opens it; the input is debounced (a remote
    // search is a range-read round trip, so don't fire one on every keystroke).
    const finderBtn = $("finderBtn");
    if (finderBtn) finderBtn.onclick = openFinder;
    // Wrap/no-wrap toggle for the query editor.
    const wrapBtn = $("wrapBtn");
    if (wrapBtn && window.PlaygroundEditor) {
      const syncWrapBtn = () => {
        const on = window.PlaygroundEditor.isWrapped("q");
        wrapBtn.setAttribute("aria-pressed", on ? "true" : "false");
        wrapBtn.classList.toggle("on", on);
        wrapBtn.textContent = on ? "⤶ Wrap" : "→ No wrap";
      };
      syncWrapBtn();
      wrapBtn.onclick = () => { window.PlaygroundEditor.toggleWrap("q"); syncWrapBtn(); };
    }
    const finderClose = $("finderModalClose");
    if (finderClose) finderClose.onclick = closeFinder;
    const efInput = $("efInput");
    if (efInput) {
      let efDebounce = null;
      efInput.oninput = () => { clearTimeout(efDebounce); efDebounce = setTimeout(efSearch, 180); };
    }
    // Label predicate: which property the decode chips + entity search read as a
    // human label. Changing it clears the decode cache so labels re-resolve.
    const labelProp = $("labelProp");
    if (labelProp) {
      labelProp.onchange = () => {
        state.labelProp = labelProp.value;
        if (window.PlaygroundEditor && PlaygroundEditor.clearLabels) PlaygroundEditor.clearLabels("q");
        facetCache.clear(); // cached value labels were read with the old predicate
        efSearch();
      };
    }
    // Hover preview + click-through for http(s) URLs in result tables.
    bindLinkPreviews();
    // Hover-zoom for image thumbnails in result tables.
    bindThumbZoom();

    await wasm_bindgen(b64ToBytes(RETE_WASM_B64));
    wasmReady = true;
    runBuildValidation(); // validate the seeded sample now that the engine is up

    // Pull any datasets the user built earlier (IndexedDB) into the live catalog
    // so they're selectable and the "Saved in this browser" list is populated.
    await loadUserDatasets();
    renderBuildSaved();

    const params = readHash();
    const ds = params.get("dataset");
    const load = params.get("load");
    // A deep-linked live endpoint (#endpoint=…) connects immediately and takes
    // over query routing — no catalog modal over it.
    const liveEp = params.get("endpoint");
    let bootShowCatalog = false;
    // Restore the deep-linked dataset in its load mode. Remote-lazy/cache datasets
    // aren't in RETE_DATASETS_B64, so the old embedded-only check silently fell
    // back to the default (scholar) on every reload of a remote dataset.
    if (ds && (datasetInfo(ds) || userBytes.has(ds))) {
      if (load === "lazy") enterRemote(remoteUrlFor(ds), ds);
      else if (load === "cache") await loadCachedRemote(ds);
      else selectDataset(ds); // bundled if embedded, else lazy (the safe default)
    } else {
      // No deep link: land on the dataset listing so the first thing the visitor
      // sees is the catalog (pick + load), not a pre-opened default. The default
      // still loads behind the modal so the console isn't empty if they dismiss it.
      loadDataset(CATALOG.defaultDataset);
      dsSelected = CATALOG.defaultDataset;
      bootShowCatalog = true;
    }

    const q = params.get("q");
    const exParam = params.get("ex");
    if (q) {
      setEd("q", q);
      state.selectedExample = -1;
      renderExamples();
    } else if (exParam != null && ds) {
      // Short deep link: #dataset=<key>&ex=<n> selects the catalog's Nth example
      // (its query, view and column headers) — no need to URL-encode the SPARQL.
      selectExample(parseInt(exParam, 10));
      renderExamples();
    }
    setMode(params.get("mode") || "sparql");
    if (liveEp) connectLiveEndpoint(liveEp);
    updateResultVisibility();
    // Open the catalog last, over a fully-rendered console (see the no-deep-link branch).
    if (bootShowCatalog && !liveEp) openSource();
  }

  boot().catch((e) => {
    setStatus("boot failed");
    showError("out", String(e && e.stack ? e.stack : e));
  });
})();
