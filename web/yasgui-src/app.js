// yasgui-wasm — a Yasgui-style SPARQL IDE where the "endpoint" is a .rete file:
// paste a URL (read lazily over HTTP range) or open a local file, and every
// query runs in a WebAssembly engine inside this page. UI after Yasgui
// (github.com/TriplyDB/Yasgui); engine and worker protocol are rete's.
//
// Globals provided by the built page: CM (CodeMirror 6 bundle), wasm_bindgen
// (no-modules glue, reused as worker source), RETE_WASM_B64, CATALOG,
// BUILD_STAMP.

"use strict";

/* ---------------------------------------------------------------- utils */

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s).replace(/[&<>"']/g, (c) =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

function fmtBytes(n) {
  if (n == null || isNaN(n)) return "?";
  if (n < 1024) return `${n} B`;
  if (n < 1048576) return `${(n / 1024).toFixed(1)} KB`;
  if (n < 1073741824) return `${(n / 1048576).toFixed(1)} MB`;
  return `${(n / 1073741824).toFixed(2)} GB`;
}
const fmtMs = (ms) => (ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`);
const fmtInt = (n) => Number(n).toLocaleString("en-US");

function b64ToBytes(b64) {
  const bin = atob(b64);
  const u = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) u[i] = bin.charCodeAt(i);
  return u;
}

let toastTimer = null;
function toast(msg) {
  const t = $("toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, 2200);
}

/* ------------------------------------------------------ prefix shortening */

const WELL_KNOWN_PREFIXES = {
  rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
  rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  owl: "http://www.w3.org/2002/07/owl#",
  xsd: "http://www.w3.org/2001/XMLSchema#",
  skos: "http://www.w3.org/2004/02/skos/core#",
  schema: "https://schema.org/",
  sdo: "http://schema.org/",
  foaf: "http://xmlns.com/foaf/0.1/",
  dcterms: "http://purl.org/dc/terms/",
  dce: "http://purl.org/dc/elements/1.1/",
  prov: "http://www.w3.org/ns/prov#",
  geo: "http://www.opengis.net/ont/geosparql#",
  wgs84: "http://www.w3.org/2003/01/geo/wgs84_pos#",
  sh: "http://www.w3.org/ns/shacl#",
  void: "http://rdfs.org/ns/void#",
  dcat: "http://www.w3.org/ns/dcat#",
  eli: "http://data.europa.eu/eli/ontology#",
  gvp: "http://vocab.getty.edu/ontology#",
  ulan: "http://vocab.getty.edu/ulan/",
  wd: "http://www.wikidata.org/entity/",
  wdt: "http://www.wikidata.org/prop/direct/",
  dwc: "http://rs.tdwg.org/dwc/terms/",
  crm: "http://www.cidoc-crm.org/cidoc-crm/",
};

// PREFIX declarations in the current query win over the well-known table.
function prefixesFromQuery(q) {
  const out = {};
  const re = /PREFIX\s+([\w-]*):\s*<([^>]*)>/gi;
  let m;
  while ((m = re.exec(q))) out[m[1]] = m[2];
  return out;
}

function shortenIri(iri, queryPrefixes) {
  const maps = [queryPrefixes || {}, WELL_KNOWN_PREFIXES];
  let best = null;
  for (const map of maps) {
    for (const [p, ns] of Object.entries(map)) {
      if (iri.startsWith(ns) && ns.length > (best ? best.ns.length : 0)) best = { p, ns };
    }
  }
  if (best) {
    const local = iri.slice(best.ns.length);
    if (local && !/[/#]/.test(local)) return `${best.p}:${local}`;
  }
  const m = iri.match(/[#/]([^#/]+)[/#]?$/);
  return m ? m[1] : iri;
}

/* -------------------------------------------------------- term rendering */

// Terms arrive in the engine's "table" lexical form: IRIs as `<…>`, literals
// as `"…"` / `"…"@lang` / `"…"^^<dt>`, blank nodes as `_:x`.
function parseTerm(s) {
  if (s == null) return null;
  s = String(s);
  if (s.startsWith("<") && s.endsWith(">")) return { kind: "iri", value: s.slice(1, -1) };
  if (s.startsWith("_:")) return { kind: "bnode", value: s };
  const m = s.match(/^"([\s\S]*)"(?:@([\w-]+)|\^\^<(.+)>)?$/);
  if (m) return { kind: "lit", value: m[1], lang: m[2] || null, dt: m[3] || null };
  return { kind: "plain", value: s };
}

function termHtml(s, queryPrefixes) {
  const t = parseTerm(s);
  if (!t) return `<span class="nullcell">—</span>`;
  if (t.kind === "iri") {
    const short = shortenIri(t.value, queryPrefixes);
    const href = /^https?:/.test(t.value) ? ` href="${esc(t.value)}" target="_blank" rel="noopener"` : "";
    return `<a class="iri"${href} title="${esc(t.value)}">${esc(short)}</a>`;
  }
  if (t.kind === "bnode") return `<span class="bnode">${esc(t.value)}</span>`;
  if (t.kind === "lit") {
    const tag = t.lang
      ? `<span class="ttag">@${esc(t.lang)}</span>`
      : t.dt && !t.dt.endsWith("#string")
        ? `<span class="ttag" title="${esc(t.dt)}">^^${esc(shortenIri(t.dt, queryPrefixes))}</span>`
        : "";
    return `<span class="lit" title="${esc(t.value)}">${esc(t.value)}</span>${tag}`;
  }
  return esc(t.value);
}

// Plain lexical value for CSV / filtering / sorting.
function termText(s) {
  const t = parseTerm(s);
  return t ? t.value : "";
}

/* ------------------------------------------------------------ the engine */

const NUMERIC_DT = /#(integer|decimal|double|float|int|long|short|byte|nonNegativeInteger|positiveInteger|unsignedInt|unsignedLong|gYear)$/;

const engine = {
  worker: null,
  seq: 0,
  pending: new Map(), // reqId → {resolve, reject}
  openKeys: new Set(),
  progressTotal: 0,
  onProgress: null,
  initPromise: null,

  boot() {
    const src =
      document.getElementById("reteGlue").textContent +
      "\n" +
      document.getElementById("workerSrc").textContent;
    const blob = new Blob([src], { type: "text/javascript" });
    this.worker = new Worker(URL.createObjectURL(blob));
    this.openKeys = new Set();
    this.progressTotal = 0;
    this.worker.onmessage = (e) => {
      const m = e.data || {};
      if (m.type === "progress") {
        this.progressTotal = m.bytes;
        if (this.onProgress) this.onProgress(m.bytes);
        return;
      }
      const p = this.pending.get(m.reqId);
      if (!p) return;
      this.pending.delete(m.reqId);
      if (m.ok) p.resolve(m);
      else p.reject(new Error(m.error || "engine error"));
    };
    this.worker.onerror = (e) => {
      const msg = `engine worker crashed: ${e.message || e.type || e}`;
      for (const p of this.pending.values()) p.reject(new Error(msg));
      this.pending.clear();
    };
    const bytes = b64ToBytes(RETE_WASM_B64);
    this.initPromise = this.call({ type: "init", wasm: bytes.buffer }, [bytes.buffer]);
    return this.initPromise;
  },

  call(msg, transfer) {
    return new Promise((resolve, reject) => {
      msg.reqId = ++this.seq;
      this.pending.set(msg.reqId, { resolve, reject });
      this.worker.postMessage(msg, transfer || []);
    });
  },

  // Sync-XHR queries can't be interrupted: stopping means killing the worker.
  // Open handles die with it; graphs simply reopen on the next run.
  stop() {
    if (this.worker) this.worker.terminate();
    for (const p of this.pending.values()) p.reject(new Error("stopped"));
    this.pending.clear();
    this.boot();
  },
};

/* ------------------------------------------------------------- tab state */

const STORE_KEY = "rete.yasgui.v1";
const DEFAULT_QUERY = "SELECT * WHERE {\n  ?sub ?pred ?obj .\n} LIMIT 10\n";

let tabs = [];
let activeId = null;
const files = new Map(); // fileKey → ArrayBuffer (session only — cannot persist)
let tabSeq = 0;

const active = () => tabs.find((t) => t.id === activeId) || tabs[0];

function newTab(opts = {}) {
  const t = {
    id: `t${Date.now().toString(36)}${(++tabSeq).toString(36)}`,
    name: opts.name || nextTabName(),
    endpoint: opts.endpoint !== undefined ? opts.endpoint : defaultEndpoint(),
    query: opts.query !== undefined ? opts.query : DEFAULT_QUERY,
    reason: !!opts.reason,
    // session-only:
    chip: null, results: null, view: "table",
    filter: "", page: 1, sort: null, editorState: null,
  };
  tabs.push(t);
  return t;
}

function nextTabName() {
  for (let i = tabs.length + 1; ; i++) {
    const name = `Query ${i}`;
    if (!tabs.some((t) => t.name === name)) return name;
  }
}

function defaultEndpoint() {
  const a = active();
  if (a && a.endpoint && a.endpoint.mode === "url") return { ...a.endpoint };
  return CATALOG.length ? { mode: "url", url: CATALOG[0].url } : null;
}

function epKey(ep) {
  return ep.mode === "url" ? `url:${ep.url}` : ep.fileKey;
}
function epDisplay(ep) {
  if (!ep) return "";
  return ep.mode === "url" ? ep.url : `file: ${ep.fileName} (${fmtBytes(ep.size)})`;
}

function persist() {
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify({
      v: 1,
      active: activeId,
      tabs: tabs.map((t) => ({
        id: t.id, name: t.name, query: t.query, reason: t.reason,
        endpoint: t.endpoint
          ? t.endpoint.mode === "url"
            ? t.endpoint
            : { mode: "file", fileName: t.endpoint.fileName, size: t.endpoint.size, detached: true }
          : null,
      })),
    }));
  } catch (_) { /* storage full/blocked — session still works */ }
}
let persistTimer = null;
const persistSoon = () => { clearTimeout(persistTimer); persistTimer = setTimeout(persist, 300); };

function restore() {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return false;
    const s = JSON.parse(raw);
    if (!s || !Array.isArray(s.tabs) || !s.tabs.length) return false;
    for (const st of s.tabs) {
      const t = newTab({ name: st.name, endpoint: st.endpoint, query: st.query, reason: st.reason });
      t.id = st.id || t.id;
    }
    activeId = tabs.some((t) => t.id === s.active) ? s.active : tabs[0].id;
    return true;
  } catch (_) { return false; }
}

/* --------------------------------------------------------------- editor */

let view = null; // single EditorView; per-tab EditorStates swapped in/out

const SPARQL_KEYWORDS = (
  "SELECT CONSTRUCT ASK DESCRIBE WHERE FROM PREFIX BASE DISTINCT REDUCED " +
  "ORDER BY ASC DESC LIMIT OFFSET GROUP HAVING OPTIONAL FILTER UNION MINUS " +
  "GRAPH SERVICE SILENT BIND VALUES AS NOT EXISTS IN a TRUE FALSE"
).split(" ");
const SPARQL_FUNCTIONS = (
  "COUNT SUM AVG MIN MAX SAMPLE GROUP_CONCAT STR LANG LANGMATCHES DATATYPE " +
  "BOUND IRI URI BNODE RAND ABS CEIL FLOOR ROUND CONCAT STRLEN UCASE LCASE " +
  "ENCODE_FOR_URI CONTAINS STRSTARTS STRENDS STRBEFORE STRAFTER YEAR MONTH " +
  "DAY HOURS MINUTES SECONDS TIMEZONE TZ NOW UUID STRUUID MD5 REGEX REPLACE " +
  "COALESCE IF STRLANG STRDT sameTerm isIRI isURI isBLANK isLITERAL isNUMERIC"
).split(" ");

function sparqlCompletions(context) {
  const word = context.matchBefore(/[\w]*$/);
  const line = context.state.doc.lineAt(context.pos);
  const before = line.text.slice(0, context.pos - line.from);
  // "PREFIX xx" → offer known namespace declarations
  if (/PREFIX\s+[\w-]*:?$/i.test(before)) {
    return {
      from: line.from + before.search(/[\w-]*:?$/),
      options: Object.entries(WELL_KNOWN_PREFIXES).map(([p, ns]) => ({
        label: `${p}: <${ns}>`, type: "namespace",
      })),
    };
  }
  if (!word || (word.from === word.to && !context.explicit)) return null;
  return {
    from: word.from,
    options: [
      ...SPARQL_KEYWORDS.map((k) => ({ label: k, type: "keyword" })),
      ...SPARQL_FUNCTIONS.map((k) => ({ label: k, type: "function" })),
    ],
  };
}

function editorExtensions() {
  const T = CM.tags;
  const highlight = CM.HighlightStyle.define([
    { tag: T.keyword, color: "#0f4d8f", fontWeight: "700" },
    { tag: T.operatorKeyword, color: "#0f4d8f", fontWeight: "700" },
    { tag: T.string, color: "#14795f" },
    { tag: T.comment, color: "#8b949e", fontStyle: "italic" },
    { tag: T.number, color: "#a85424" },
    { tag: T.bool, color: "#a85424" },
    { tag: T.variableName, color: "#b75501" },
    { tag: T.atom, color: "#2f6f8f" },
    { tag: T.operator, color: "#66746e" },
    { tag: [T.url, T.literal], color: "#2f6f8f" },
  ]);
  const theme = CM.EditorView.theme({
    "&": { fontSize: "13.5px", background: "#fff", height: "100%" },
    "&.cm-focused": { outline: "none" },
    ".cm-scroller": {
      fontFamily: "'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace",
      lineHeight: "1.6", minHeight: "215px", maxHeight: "44vh",
    },
    ".cm-content": { padding: "10px 2px", caretColor: "#1b1f23" },
    ".cm-gutters": { background: "#f7f9f8", color: "#9aa5a0", border: "none", borderRight: "1px solid #e3e7e5" },
    ".cm-activeLine": { background: "rgba(20,125,105,.05)" },
    ".cm-activeLineGutter": { background: "rgba(20,125,105,.09)" },
    ".cm-selectionBackground, &.cm-focused .cm-selectionBackground": { background: "rgba(20,125,105,.16)" },
    ".cm-tooltip": { border: "1px solid #d7ddda", borderRadius: "7px", background: "#fff" },
    ".cm-tooltip-autocomplete > ul": {
      fontFamily: "'Cascadia Mono','SF Mono',Consolas,ui-monospace,monospace",
      fontSize: "12.5px", maxHeight: "15em",
    },
    ".cm-tooltip-autocomplete > ul > li[aria-selected]": { background: "#e3f0ec", color: "#0d5a4b" },
  });
  return [
    CM.lineNumbers(),
    CM.history(),
    CM.drawSelection(),
    CM.highlightActiveLine(),
    CM.highlightActiveLineGutter(),
    CM.bracketMatching(),
    CM.closeBrackets(),
    CM.indentOnInput(),
    CM.indentUnit.of("  "),
    CM.StreamLanguage.define(CM.sparql),
    CM.syntaxHighlighting(highlight),
    CM.autocompletion({ override: [sparqlCompletions] }),
    CM.placeholder("Write a SPARQL query… (Ctrl+Enter runs it)"),
    theme,
    CM.keymap.of([
      { key: "Mod-Enter", run: () => { runQuery(); return true; } },
      ...CM.closeBracketsKeymap,
      ...CM.defaultKeymap,
      ...CM.historyKeymap,
      ...CM.completionKeymap,
      CM.indentWithTab,
    ]),
    CM.EditorView.updateListener.of((u) => {
      if (u.docChanged) {
        const t = active();
        if (t) { t.query = u.state.doc.toString(); persistSoon(); }
      }
    }),
  ];
}

function editorStateFor(tab) {
  return CM.EditorState.create({ doc: tab.query || "", extensions: editorExtensions() });
}

function mountEditor() {
  const t = active();
  view = new CM.EditorView({ state: editorStateFor(t), parent: $("editor") });
}

function switchEditorTo(tab) {
  const prev = active();
  if (prev && view) prev.editorState = view.state;
  view.setState(tab.editorState || editorStateFor(tab));
}

/* ------------------------------------------------------------- rendering */

function renderTabs() {
  const bar = $("tabs");
  bar.innerHTML = "";
  for (const t of tabs) {
    const el = document.createElement("div");
    el.className = "tab" + (t.id === activeId ? " active" : "");
    el.title = epDisplay(t.endpoint);
    const name = document.createElement("span");
    name.className = "tabname";
    name.textContent = t.name;
    el.appendChild(name);
    const close = document.createElement("button");
    close.className = "tabclose";
    close.textContent = "×";
    close.title = "Close tab";
    close.onclick = (e) => { e.stopPropagation(); closeTab(t.id); };
    el.appendChild(close);
    el.onclick = () => activateTab(t.id);
    el.ondblclick = (e) => {
      e.preventDefault();
      const inp = document.createElement("input");
      inp.className = "tabrename";
      inp.value = t.name;
      const done = () => { t.name = inp.value.trim() || t.name; persistSoon(); renderTabs(); };
      inp.onblur = done;
      inp.onkeydown = (ev) => { if (ev.key === "Enter") inp.blur(); if (ev.key === "Escape") { inp.value = t.name; inp.blur(); } };
      name.replaceWith(inp);
      inp.focus(); inp.select();
    };
    bar.appendChild(el);
  }
}

function renderEndpoint() {
  const t = active();
  const inp = $("endpoint");
  inp.value = epDisplay(t.endpoint);
  inp.classList.toggle("isfile", !!(t.endpoint && t.endpoint.mode === "file"));
  $("reasonToggle").checked = !!t.reason;
  const chip = $("dsChip");
  if (t.chip) { chip.textContent = t.chip; chip.hidden = false; }
  else if (t.endpoint && t.endpoint.detached) { chip.textContent = "uploaded file — re-attach it (⬆) to query"; chip.hidden = false; }
  else chip.hidden = true;
}

function activateTab(id) {
  if (id === activeId) return;
  const next = tabs.find((t) => t.id === id);
  if (!next) return;
  switchEditorTo(next);
  activeId = id;
  renderTabs(); renderEndpoint(); renderResults();
  persistSoon();
}

function closeTab(id) {
  const idx = tabs.findIndex((t) => t.id === id);
  if (idx < 0) return;
  tabs.splice(idx, 1);
  if (!tabs.length) { const t = newTab(); activeId = t.id; view.setState(editorStateFor(t)); }
  else if (id === activeId) {
    const next = tabs[Math.max(0, idx - 1)];
    activeId = next.id;
    view.setState(next.editorState || editorStateFor(next));
  }
  renderTabs(); renderEndpoint(); renderResults();
  persistSoon();
}

/* --------------------------------------------------------------- results */

function setRunning(on) {
  const btn = $("runBtn");
  btn.classList.toggle("running", on);
  btn.title = on ? "Stop" : "Run (Ctrl+Enter)";
  btn.innerHTML = on
    ? `<svg viewBox="0 0 24 24" width="17" height="17"><rect x="6" y="6" width="12" height="12" rx="1.5" fill="#fff"/></svg>`
    : `<svg viewBox="0 0 24 24" width="19" height="19"><path d="M8 5.5v13l11-6.5z" fill="#fff"/></svg>`;
}

function statsLine(t) {
  const r = t.results;
  if (!r) return "";
  if (r.error) return "";
  const env = r.envelope;
  const n = env.kind === "select" ? env.rows.length
    : env.kind === "construct" ? env.triples.length : null;
  let s = `<b>${n == null ? "" : fmtInt(n) + (env.kind === "construct" ? " triples" : " results")}</b>`;
  if (n == null) s = "";
  s += `${s ? " in " : "Took "}${fmtMs(r.ms)}`;
  if (r.reason) s += ` · 🧠 reasoned`;
  if (r.traffic && r.traffic.requests > 0) {
    s += ` · fetched ${fmtBytes(r.traffic.bytes)} in ${r.traffic.requests} range request${r.traffic.requests === 1 ? "" : "s"} (of a ${fmtBytes(r.traffic.fileLength)} file)`;
  } else if (r.traffic) {
    s += ` · 0 bytes fetched (cache)`;
  } else if (r.remote === false) {
    s += ` · in-memory`;
  }
  return s;
}

function tableRows(t) {
  const env = t.results.envelope;
  const qp = prefixesFromQuery(t.query);
  let vars, rows;
  if (env.kind === "select") {
    vars = env.vars;
    rows = env.rows.map((r) => vars.map((v) => (v in r ? r[v] : null)));
  } else {
    vars = ["subject", "predicate", "object"];
    rows = env.triples;
  }
  // filter
  const f = t.filter.trim().toLowerCase();
  let view_ = rows;
  if (f) view_ = rows.filter((r) => r.some((c) => c != null && String(c).toLowerCase().includes(f)));
  // sort
  if (t.sort) {
    const { col, dir } = t.sort;
    const key = (c) => {
      if (c == null) return { n: null, s: "" };
      const p = parseTerm(c);
      const num = p.kind === "lit" && (p.dt == null || NUMERIC_DT.test(p.dt)) ? parseFloat(p.value) : NaN;
      return { n: isNaN(num) ? null : num, s: (p.value || "").toLowerCase() };
    };
    view_ = [...view_].sort((a, b) => {
      const ka = key(a[col]), kb = key(b[col]);
      let cmp;
      if (ka.n != null && kb.n != null) cmp = ka.n - kb.n;
      else cmp = ka.s < kb.s ? -1 : ka.s > kb.s ? 1 : 0;
      return dir === "desc" ? -cmp : cmp;
    });
  }
  return { vars, rows, filtered: view_, qp };
}

const PAGE_SIZE = 50;

function renderResults() {
  const t = active();
  const body = $("resultsBody");
  const pager = $("pager");
  const stats = $("resultStats");
  $("filterBox").value = t.filter;
  document.querySelectorAll(".rtab").forEach((b) =>
    b.classList.toggle("active", b.dataset.view === t.view));
  pager.hidden = true;

  if (!t.results) {
    stats.innerHTML = "";
    body.innerHTML = `<div class="placeholder">No response yet — pick a dataset, write a query, press <b>▶</b>.</div>`;
    return;
  }
  const r = t.results;
  if (r.error) {
    stats.innerHTML = "";
    const cors = /xhr|status|fetch|network|range|cross|denied|http/i.test(r.error) && t.endpoint && t.endpoint.mode === "url";
    body.innerHTML = `<div class="errbox"><b>Error</b><pre>${esc(r.error)}</pre>${
      cors ? `<div class="hint">Remote .rete files must be served with <b>CORS</b> (<code>Access-Control-Allow-Origin</code>) and <b>HTTP Range</b> support (<code>Accept-Ranges: bytes</code>, status 206). R2 / S3 / most object stores can do both.</div>` : ""
    }</div>`;
    return;
  }
  stats.innerHTML = statsLine(t);

  if (t.view === "response") {
    let text = r.raw;
    let note = "";
    if (text.length > 2_000_000) {
      text = text.slice(0, 2_000_000);
      note = `<div class="hint">Response truncated for display (${fmtBytes(r.raw.length)} total) — use the JSON download for the full body.</div>`;
    } else {
      try { text = JSON.stringify(JSON.parse(text), null, 2); } catch (_) { /* show as-is */ }
    }
    body.innerHTML = `${note}<pre class="rawjson">${esc(text)}</pre>`;
    return;
  }

  const env = r.envelope;
  if (env.kind === "ask") {
    body.innerHTML = `<div class="askbox ${env.boolean ? "yes" : "no"}">${env.boolean}</div>`;
    return;
  }
  if (env.kind === "construct" && env.format && env.text != null) {
    body.innerHTML = `<pre class="rawjson">${esc(env.text)}</pre>`;
    return;
  }

  const { vars, rows, filtered, qp } = tableRows(t);
  if (!rows.length) {
    body.innerHTML = `<div class="placeholder">Query ran fine — zero results.</div>`;
    return;
  }
  const pages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  if (t.page > pages) t.page = pages;
  const start = (t.page - 1) * PAGE_SIZE;
  const slice = filtered.slice(start, start + PAGE_SIZE);

  const head = vars.map((v, i) => {
    const dir = t.sort && t.sort.col === i ? t.sort.dir : null;
    return `<th data-col="${i}" title="Sort">${esc(env.kind === "select" ? "?" + v : v)}<span class="sortmark">${dir === "asc" ? " ▲" : dir === "desc" ? " ▼" : ""}</span></th>`;
  }).join("");
  const trs = slice.map((row, ri) =>
    `<tr><td class="rownum">${start + ri + 1}</td>${row.map((c) => `<td>${c == null ? `<span class="nullcell">—</span>` : termHtml(c, qp)}</td>`).join("")}</tr>`
  ).join("");
  body.innerHTML = `<div class="tablewrap"><table class="rs"><thead><tr><th class="rownum">#</th>${head}</tr></thead><tbody>${trs}</tbody></table></div>`;

  body.querySelectorAll("th[data-col]").forEach((th) => {
    th.onclick = () => {
      const col = +th.dataset.col;
      t.sort = t.sort && t.sort.col === col && t.sort.dir === "asc"
        ? { col, dir: "desc" }
        : t.sort && t.sort.col === col && t.sort.dir === "desc" ? null : { col, dir: "asc" };
      renderResults();
    };
  });

  if (pages > 1 || filtered.length !== rows.length) {
    pager.hidden = false;
    const btn = (p, label, dis) =>
      `<button class="pbtn" data-p="${p}" ${dis ? "disabled" : ""}>${label}</button>`;
    pager.innerHTML =
      `<span>Showing ${fmtInt(start + 1)}–${fmtInt(Math.min(start + PAGE_SIZE, filtered.length))} of ${fmtInt(filtered.length)}${filtered.length !== rows.length ? ` (filtered from ${fmtInt(rows.length)})` : ""}</span>` +
      `<span class="pbtns">${btn(t.page - 1, "‹", t.page <= 1)}<span class="pcur">${t.page} / ${pages}</span>${btn(t.page + 1, "›", t.page >= pages)}</span>`;
    pager.querySelectorAll(".pbtn").forEach((b) => {
      b.onclick = () => { t.page = +b.dataset.p; renderResults(); };
    });
  }
}

/* ------------------------------------------------------------ run / open */

let inFlight = false;

async function ensureOpen(t) {
  const ep = t.endpoint;
  if (!ep) throw new Error("no endpoint — paste a .rete URL or open a local file");
  if (ep.detached) throw new Error(`this tab used the uploaded file "${ep.fileName}" — re-attach it with ⬆ open file`);
  const key = epKey(ep);
  if (engine.openKeys.has(key)) return key;
  $("resultStats").innerHTML = `<span class="spin"></span> opening dataset…`;
  let reply;
  if (ep.mode === "url") {
    reply = await engine.call({ type: "open", key, mode: "remote", url: ep.url });
  } else {
    const buf = files.get(ep.fileKey);
    if (!buf) throw new Error(`the uploaded file "${ep.fileName}" is gone — re-attach it with ⬆ open file`);
    const copy = buf.slice(0); // keep the original for reopens after a Stop
    reply = await engine.call({ type: "open", key, mode: "local", bytes: copy }, [copy]);
  }
  engine.openKeys.add(key);
  t.chip = reply.remote
    ? `${fmtBytes(reply.stats ? reply.stats.fileLength : 0)} file · remote, read lazily over HTTP range`
    : `${fmtInt(reply.info ? reply.info.quads : 0)} triples · in-memory`;
  renderEndpoint();
  return key;
}

async function runQuery() {
  if (inFlight) return;
  const t = active();
  const sparql = view.state.doc.toString();
  t.query = sparql;
  if (!sparql.trim()) { toast("empty query"); return; }
  inFlight = true;
  setRunning(true);
  const statsEl = $("resultStats");
  const base = engine.progressTotal;
  engine.onProgress = (bytes) => {
    statsEl.innerHTML = `<span class="spin"></span> running… ${fmtBytes(bytes - base)} fetched`;
  };
  try {
    const key = await ensureOpen(t);
    statsEl.innerHTML = `<span class="spin"></span> running…`;
    const reply = await engine.call({ type: "query", key, sparql, reason: !!t.reason });
    const envelope = JSON.parse(reply.json);
    t.results = {
      envelope, raw: reply.json, ms: reply.ms,
      traffic: reply.traffic, remote: reply.remote, reason: !!t.reason, error: null,
    };
    t.page = 1; t.sort = null; t.filter = "";
    if (envelope.kind === "construct" && !envelope.triples) envelope.triples = [];
  } catch (err) {
    if (String(err.message) === "stopped") {
      t.results = { error: "stopped — the worker was killed mid-query; open datasets will reopen on the next run", envelope: null };
    } else {
      t.results = { error: String(err.message || err), envelope: null };
    }
  } finally {
    engine.onProgress = null;
    inFlight = false;
    setRunning(false);
    renderResults();
    persistSoon();
  }
}

/* ------------------------------------------------------- downloads/share */

function download(name, text, type) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], { type }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 5000);
}

function csvEscape(v) {
  return /[",\n\r]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v;
}

function downloadCsv() {
  const t = active();
  if (!t.results || t.results.error) { toast("nothing to download"); return; }
  const env = t.results.envelope;
  if (env.kind === "ask") { download("result.csv", `boolean\n${env.boolean}\n`, "text/csv"); return; }
  const { vars, filtered } = tableRows(t);
  const lines = [vars.map(csvEscape).join(",")];
  for (const r of filtered) lines.push(r.map((c) => csvEscape(c == null ? "" : termText(c))).join(","));
  download(`${t.name.replace(/\W+/g, "_") || "results"}.csv`, lines.join("\r\n") + "\r\n", "text/csv");
}

function downloadJson() {
  const t = active();
  if (!t.results || t.results.error) { toast("nothing to download"); return; }
  download(`${t.name.replace(/\W+/g, "_") || "results"}.json`, t.results.raw, "application/json");
}

function shareLink() {
  const t = active();
  const params = new URLSearchParams();
  params.set("query", t.query);
  if (t.endpoint && t.endpoint.mode === "url") params.set("endpoint", t.endpoint.url);
  if (t.reason) params.set("reason", "1");
  const url = `${location.origin}${location.pathname}#${params.toString()}`;
  (navigator.clipboard ? navigator.clipboard.writeText(url) : Promise.reject())
    .then(() => toast("link copied — anyone opening it gets this tab"))
    .catch(() => { prompt("Copy this link:", url); });
}

function tabFromHash() {
  const h = location.hash.replace(/^#/, "");
  if (!h || (!h.includes("query=") && !h.includes("endpoint="))) return null;
  try {
    const p = new URLSearchParams(h);
    const endpoint = p.get("endpoint");
    return {
      name: "shared",
      query: p.get("query") || DEFAULT_QUERY,
      endpoint: endpoint ? { mode: "url", url: endpoint } : defaultEndpoint(),
      reason: p.get("reason") === "1",
    };
  } catch (_) { return null; }
}

/* ------------------------------------------------------- files & catalog */

function attachFile(file) {
  file.arrayBuffer().then((buf) => {
    const t = active();
    const fileKey = `file:${file.name}:${file.size}:${file.lastModified}`;
    files.set(fileKey, buf);
    t.endpoint = { mode: "file", fileKey, fileName: file.name, size: file.size };
    t.chip = null;
    renderEndpoint();
    persistSoon();
    toast(`${file.name} attached — it stays in this browser, nothing is uploaded`);
  });
}

function renderCatalog() {
  const pop = $("catalogPop");
  pop.innerHTML = CATALOG.map((c, i) => `
    <div class="catitem" data-i="${i}">
      <div class="cathead"><b>${esc(c.name)}</b><span class="catsize">${esc(c.size || "")}</span></div>
      <div class="catblurb">${esc(c.blurb)}</div>
    </div>`).join("");
  pop.querySelectorAll(".catitem").forEach((el) => {
    el.onclick = () => {
      const c = CATALOG[+el.dataset.i];
      const t = active();
      t.endpoint = { mode: "url", url: c.url };
      t.chip = null;
      if (c.query && (!view.state.doc.toString().trim() || view.state.doc.toString() === DEFAULT_QUERY)) {
        view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: c.query } });
        t.query = c.query;
      }
      pop.hidden = true;
      renderEndpoint();
      persistSoon();
    };
  });
}

/* ------------------------------------------------------------------ init */

function wireUi() {
  $("addTab").onclick = () => {
    const t = newTab();
    activateTab(t.id);
  };

  const inp = $("endpoint");
  inp.addEventListener("change", () => {
    const t = active();
    const v = inp.value.trim();
    if (t.endpoint && t.endpoint.mode === "file" && v === epDisplay(t.endpoint)) return;
    t.endpoint = v ? { mode: "url", url: v } : null;
    t.chip = null;
    renderEndpoint();
    persistSoon();
  });
  inp.addEventListener("keydown", (e) => { if (e.key === "Enter") { inp.blur(); runQuery(); } });

  $("uploadBtn").onclick = () => $("fileInput").click();
  $("fileInput").onchange = (e) => { if (e.target.files[0]) attachFile(e.target.files[0]); e.target.value = ""; };

  $("catalogBtn").onclick = (e) => {
    e.stopPropagation();
    const pop = $("catalogPop");
    pop.hidden = !pop.hidden;
  };
  document.addEventListener("click", (e) => {
    if (!$("catalogPop").hidden && !$("catalogPop").contains(e.target)) $("catalogPop").hidden = true;
  });

  $("reasonToggle").onchange = (e) => { active().reason = e.target.checked; persistSoon(); };

  $("runBtn").onclick = () => {
    if (inFlight) {
      engine.stop();
      // tabs whose graphs lived in the dead worker reopen lazily on next run
      for (const t of tabs) t.chip = null;
    } else runQuery();
  };

  document.querySelectorAll(".rtab").forEach((b) => {
    b.onclick = () => { active().view = b.dataset.view; renderResults(); };
  });
  $("filterBox").addEventListener("input", () => {
    const t = active();
    t.filter = $("filterBox").value;
    t.page = 1;
    renderResults();
  });
  $("dlCsv").onclick = downloadCsv;
  $("dlJson").onclick = downloadJson;
  $("shareBtn").onclick = shareLink;

  // drag & drop a .rete anywhere
  let dragDepth = 0;
  window.addEventListener("dragenter", (e) => { e.preventDefault(); dragDepth++; $("dropOverlay").hidden = false; });
  window.addEventListener("dragleave", () => { if (--dragDepth <= 0) { dragDepth = 0; $("dropOverlay").hidden = true; } });
  window.addEventListener("dragover", (e) => e.preventDefault());
  window.addEventListener("drop", (e) => {
    e.preventDefault();
    dragDepth = 0;
    $("dropOverlay").hidden = true;
    const f = e.dataTransfer.files && e.dataTransfer.files[0];
    if (f) attachFile(f);
  });
}

function init() {
  $("buildStamp").textContent = BUILD_STAMP;
  const had = restore();
  const shared = tabFromHash();
  if (shared) {
    const t = newTab(shared);
    activeId = t.id;
  } else if (!had) {
    const first = CATALOG[0] || {};
    const t = newTab({ endpoint: first.url ? { mode: "url", url: first.url } : null, query: first.query || DEFAULT_QUERY });
    activeId = t.id;
  }
  mountEditor();
  renderTabs();
  renderEndpoint();
  renderCatalog();
  renderResults();
  wireUi();
  setRunning(false);
  engine.boot().catch((e) => {
    $("resultsBody").innerHTML = `<div class="errbox"><b>Engine failed to start</b><pre>${esc(String(e.message || e))}</pre></div>`;
  });
  persist();
}

init();
