// dataset.js — one dataset's page.
//
//   header   title, description, key figures, licence, format, source, hash, actions
//   query    SPARQL editor + starter picker + entity search + results table, backed
//            by the WASM worker (plaza-worker.js; its message protocol is at the
//            top of that file). Remote files are read lazily over HTTP range.
//   schema   UML diagram of the card's classes and class links
//   profile  classes, predicates, vocabularies, connections, signals
//   files    the .rete, its shards, companions, DuckDB table explorer
//   about    full description, provenance, raw card
//
// window.plazaDataset (see the bottom of the file) is the programmatic surface
// that js/webmcp.js is written against. Keep it stable.
import { readReteCard, liteCardFromHeader, fmtBytes, plainDescription, formatLabel } from "./rete-card.js";
import { imageInfoFromCard } from "./procgen.js";
import { renderFingerprint } from "./procgen-p5.js";
import { mountSchemaUML } from "./schema-uml.js";
import { derivedFacets } from "./facets.js";
import { detectProviders } from "./providers.js";
import { mountTableExplorer } from "./tables-duckdb.js";
import { usedOntologies } from "./vocabs.js";

// ── small helpers ───────────────────────────────────────────────────────────
const $ = (sel, el = document) => el.querySelector(sel);
const $$ = (sel, el = document) => [...el.querySelectorAll(sel)];
const fmt = (n) => (n == null ? "–" : Intl.NumberFormat().format(n));
const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const enc = encodeURIComponent;
const httpsify = (u) => {
  const s = String(u || "");
  return /^http:\/\/(localhost|127\.0\.0\.1)/i.test(s) ? s : s.replace(/^http:\/\//i, "https://");
};
const absUrl = (u) => { try { return new URL(u, location.href).href; } catch (_) { return String(u); } };
const isHttp = (u) => /^https?:\/\//i.test(String(u || ""));
const stripAngles = (s) => String(s == null ? "" : s).replace(/^<|>$/g, "");
const reduceMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const PLAYGROUND = "https://caviri.github.io/rete/docs/playground.html";
const DOCS = "https://caviri.github.io/rete/docs/";

let REMOTE_TOKEN = ""; // from the manifest; appended to bucket URLs that lack one
const withToken = (u) => {
  const s = String(u || "");
  if (!REMOTE_TOKEN || !/hf\.space|katospiegel/i.test(s) || /[?&]token=/.test(s)) return s;
  return s + (s.includes("?") ? "&" : "?") + "token=" + REMOTE_TOKEN;
};

async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch (_) { /* fall through */ }
  try {
    const ta = document.createElement("textarea");
    ta.value = text; ta.style.cssText = "position:fixed;left:-9999px";
    document.body.appendChild(ta); ta.select();
    const ok = document.execCommand("copy");
    ta.remove();
    return ok;
  } catch (_) { return false; }
}
function flash(btn, msg, ok = true) {
  const old = btn.dataset.label || (btn.dataset.label = btn.textContent);
  btn.textContent = ok ? msg : "Copy failed";
  clearTimeout(btn._t);
  btn._t = setTimeout(() => (btn.textContent = old), 1400);
}

// ── prefixes and term display ───────────────────────────────────────────────
const WELL_KNOWN = {
  rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
  rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  owl: "http://www.w3.org/2002/07/owl#",
  xsd: "http://www.w3.org/2001/XMLSchema#",
  skos: "http://www.w3.org/2004/02/skos/core#",
  dct: "http://purl.org/dc/terms/",
  dc: "http://purl.org/dc/elements/1.1/",
  foaf: "http://xmlns.com/foaf/0.1/",
  schema: "http://schema.org/",
  prov: "http://www.w3.org/ns/prov#",
  geo: "http://www.opengis.net/ont/geosparql#",
  wgs: "http://www.w3.org/2003/01/geo/wgs84_pos#",
  time: "http://www.w3.org/2006/time#",
  void: "http://rdfs.org/ns/void#",
  dcat: "http://www.w3.org/ns/dcat#",
  org: "http://www.w3.org/ns/org#",
  wd: "http://www.wikidata.org/entity/",
  wdt: "http://www.wikidata.org/prop/direct/",
  wikibase: "http://wikiba.se/ontology#",
  cito: "http://purl.org/spar/cito/",
  np: "http://www.nanopub.org/nschema#",
  npx: "http://purl.org/nanopub/x/",
  obo: "http://purl.obolibrary.org/obo/",
  oboInOwl: "http://www.geneontology.org/formats/oboInOwl#",
  bibo: "http://purl.org/ontology/bibo/",
  gvp: "http://vocab.getty.edu/ontology#",
  sh: "http://www.w3.org/ns/shacl#",
};

/** prefix -> namespace for this dataset: well-known ones plus its card vocabularies. */
function buildPrefixes(card) {
  const byNs = new Map(Object.entries(WELL_KNOWN).map(([p, ns]) => [ns, p]));
  const used = new Set(Object.keys(WELL_KNOWN));
  const nsList = [...((card && card.vocabularies) || [])];
  const base = card && card.signals && card.signals.base_iri;
  if (base && isHttp(base) && /[#/]$/.test(base) && !nsList.includes(base)) nsList.push(base);
  for (const ns0 of nsList) {
    const ns = String(ns0);
    if (!isHttp(ns) || byNs.has(ns)) continue;
    const seg = ns.replace(/[#/]+$/, "").split(/[#/]/).filter(Boolean).pop() || "ns";
    let p = seg.toLowerCase().replace(/[^a-z0-9]/g, "").replace(/^(\d)/, "n$1").slice(0, 10) || "ns";
    if (/^(www|ex|example|purl|ns|vocab|ontology|resource|data|id)$/.test(p) && ns.split("/").length > 3) {
      p = (ns.split("/")[2] || p).replace(/^www\./, "").split(".")[0].replace(/[^a-z0-9]/gi, "").toLowerCase() || p;
    }
    let q = p, i = 2;
    while (used.has(q)) q = p + i++;
    used.add(q); byNs.set(ns, q);
  }
  return Object.fromEntries([...byNs].map(([ns, p]) => [p, ns]));
}

let PREFIXES = { ...WELL_KNOWN };
let NS_LIST = []; // [ns, prefix] longest first
function setPrefixes(map, extra) {
  PREFIXES = { ...map, ...(extra || {}) };
  NS_LIST = Object.entries(PREFIXES).map(([p, ns]) => [ns, p]).sort((a, b) => b[0].length - a[0].length);
}
setPrefixes(WELL_KNOWN);

function shorten(iri) {
  const s = stripAngles(iri);
  for (const [ns, p] of NS_LIST) {
    if (s.startsWith(ns) && s.length > ns.length) {
      const local = s.slice(ns.length);
      if (!/[\s<>"{}|\\^`]/.test(local)) return p + ":" + local;
    }
  }
  return s;
}

/** PREFIX declarations a query text already carries, so results use them too. */
function queryPrefixes(sparql) {
  const out = {};
  for (const m of String(sparql || "").matchAll(/PREFIX\s+([A-Za-z][\w-]*)?:\s*<([^>]*)>/gi)) out[m[1] || ""] = m[2];
  return out;
}

const NUM_DT = /#(integer|int|long|short|decimal|double|float|nonNegativeInteger|positiveInteger|negativeInteger|unsignedInt|unsignedLong)$/;

function unescapeLit(s) {
  return s.replace(/\\(["\\nrt])/g, (_, c) => ({ n: "\n", r: "\r", t: "\t", '"': '"', "\\": "\\" }[c]));
}
/** An N-Triples style term as the engine returns it -> { kind, value, lang, dt }. */
function parseTerm(t) {
  if (t == null || t === "") return { kind: "none", value: "" };
  const s = String(t);
  if (s[0] === "<" && s.endsWith(">")) return { kind: "iri", value: s.slice(1, -1) };
  if (s.startsWith("_:")) return { kind: "bnode", value: s };
  const m = s.match(/^"([\s\S]*)"(?:@([A-Za-z][\w-]*)|\^\^<([^>]+)>)?$/);
  if (m) return { kind: "lit", value: unescapeLit(m[1]), lang: m[2] || "", dt: m[3] || "" };
  return { kind: "lit", value: s, lang: "", dt: "" };
}
function cellHtml(t) {
  const p = parseTerm(t);
  if (p.kind === "none") return `<span class="dz-null">–</span>`;
  if (p.kind === "iri") {
    const label = esc(shorten(p.value));
    return isHttp(p.value)
      ? `<a class="dz-iri" href="${esc(httpsify(p.value))}" target="_blank" rel="noopener" title="${esc(p.value)}">${label}</a>`
      : `<span class="dz-iri" title="${esc(p.value)}">${label}</span>`;
  }
  if (p.kind === "bnode") return `<span class="dz-bnode">${esc(p.value)}</span>`;
  const tag = p.lang ? ` <span class="dz-tag">@${esc(p.lang)}</span>` : p.dt && !/#string$/.test(p.dt) ? ` <span class="dz-tag" title="${esc(p.dt)}">${esc(shorten(p.dt))}</span>` : "";
  return `<span class="dz-lit${NUM_DT.test(p.dt) ? " num" : ""}">${esc(p.value)}</span>${tag}`;
}
const plainValue = (t) => { const p = parseTerm(t); return p.kind === "none" ? "" : p.value; };
function sortKey(t) {
  const p = parseTerm(t);
  if (p.kind === "none") return { n: null, s: "" };
  const v = p.kind === "iri" ? shorten(p.value) : p.value;
  const n = p.kind === "lit" && v.trim() !== "" && Number.isFinite(Number(v)) ? Number(v) : null;
  return { n, s: v.toLowerCase() };
}

// ── engine: one worker, one open file ───────────────────────────────────────
function explain(raw) {
  const m = String(raw || "unknown error");
  if (/older|reads 0x0\d and newer|format 0x0[0-4]/i.test(m))
    return { title: "This file uses an older format than the in-browser engine reads.", detail: m, raw: m };
  if (/newer than the in-browser engine/i.test(m))
    return { title: "This file is newer than the engine bundled with this page. Reload the page to fetch a current build.", detail: m, raw: m };
  if (/not a \.rete file|bad magic/i.test(m))
    return { title: "The URL did not return a .rete file.", detail: m, raw: m };
  if (/HTTP \d+|Failed to fetch|NetworkError|network|XHR|timed? ?out/i.test(m))
    return { title: "The host did not return the file. It may be offline, or may not allow cross-origin range requests.", detail: m, raw: m };
  if (/Web Workers?/i.test(m))
    return { title: "This browser could not start the engine worker.", detail: m, raw: m };
  if (/unreachable|out of memory|memory access|RuntimeError|allocation/i.test(m))
    return { title: "The engine ran out of memory or hit an internal error. It was restarted; try a smaller query.", detail: m, raw: m };
  return { title: m, detail: "", raw: m };
}

function createEngine(entry, getBundledUrl) {
  let worker = null, gen = 0, rpcId = 0, pending = new Map();
  let state = "idle", error = null, openP = null, info = null, openedAt = 0, nameSearchOff = "";
  const listeners = new Set();
  const set = (s, e) => { state = s; error = e || null; listeners.forEach((f) => { try { f(state, error); } catch (_) {} }); };
  const failPending = (err) => { for (const p of pending.values()) p.rej(err); pending = new Map(); };

  function spawn() {
    const g = ++gen;
    if (worker) { try { worker.terminate(); } catch (_) {} worker = null; }
    failPending(new Error("Cancelled"));
    let resolveOpen;
    openP = new Promise((r) => (resolveOpen = r));
    try {
      // Classic worker: it importScripts the no-modules WASM build.
      worker = new Worker(new URL("./plaza-worker.js", import.meta.url));
    } catch (e) {
      set("error", explain("Web Workers are unavailable: " + ((e && e.message) || e)));
      resolveOpen({ ok: false });
      return;
    }
    set("opening");
    const w = worker;
    w.onmessage = (ev) => {
      if (g !== gen) return;
      const m = ev.data || {};
      if (m.type === "opened") {
        if (m.ok) { info = m.info || null; openedAt = Date.now(); set("ready"); resolveOpen({ ok: true }); }
        else { set("error", explain(m.error)); resolveOpen({ ok: false }); }
        return;
      }
      const p = pending.get(m.reqId);
      if (!p) return;
      pending.delete(m.reqId);
      m.ok ? p.res(m) : p.rej(new Error(m.error || "engine error"));
    };
    w.onerror = (ev) => {
      if (g !== gen) return;
      const err = explain((ev && ev.message) || "The engine worker failed.");
      if (state !== "ready") { set("error", err); resolveOpen({ ok: false }); }
      failPending(new Error(err.title));
    };
    (async () => {
      try {
        if (entry.kind === "remote-lazy") {
          w.postMessage({ type: "open", mode: "remote", url: httpsify(withToken(absUrl(entry.rete))) });
        } else {
          const res = await fetch(getBundledUrl());
          if (!res.ok) throw new Error(`HTTP ${res.status} fetching ${entry.rete}`);
          const buf = await res.arrayBuffer();
          if (g !== gen) return;
          w.postMessage({ type: "open", mode: "local", bytes: buf }, [buf]);
        }
      } catch (e) {
        if (g !== gen) return;
        set("error", explain((e && e.message) || e));
        resolveOpen({ ok: false });
      }
    })();
  }

  async function ensureReady() {
    if (state === "idle") spawn();
    const r = await openP;
    if (!r.ok) throw new Error(error ? error.title : "The engine is not available.");
  }
  async function rpc(msg) {
    await ensureReady();
    const w = worker, g = gen;
    return new Promise((res, rej) => {
      const reqId = ++rpcId;
      pending.set(reqId, { res, rej });
      w.postMessage({ ...msg, reqId });
      if (g !== gen) rej(new Error("Cancelled"));
    });
  }
  const needsRestart = (m) => /unreachable|out of memory|memory access|RuntimeError|allocation/i.test(m);

  return {
    get state() { return state; },
    get error() { return error; },
    get info() { return info; },
    onChange(f) { listeners.add(f); },
    start() { if (state === "idle") spawn(); },
    restart() { spawn(); },
    ready: ensureReady,
    async prefix(text, limit = 10) {
      // A name search on a file without a usable label index can scan far more
      // than it should, and the worker answers one message at a time. Bound it:
      // on timeout the worker is restarted and name search is switched off.
      if (nameSearchOff) throw new Error(nameSearchOff);
      if (S.running) throw new Error("A query is running. Name search works again when it finishes.");
      let timer;
      const guard = new Promise((_, rej) => {
        timer = setTimeout(() => {
          nameSearchOff = "Name search took longer than " + PREFIX_TIMEOUT_MS / 1000 + " seconds and was stopped. This file probably has no label index, so names cannot be searched; use a SPARQL query.";
          spawn();
          rej(new Error(nameSearchOff));
        }, PREFIX_TIMEOUT_MS);
      });
      try {
        const m = await Promise.race([rpc({ type: "prefix", prefix: text, limit }), guard]);
        return (m.results || []).map((r) => ({ iri: stripAngles(r.subject), label: r.label == null ? "" : String(r.label) }));
      } finally { clearTimeout(timer); }
    },
    async query(sparql) {
      const t0 = performance.now();
      let m;
      try { m = await rpc({ type: "query", sparql }); }
      catch (e) {
        if (needsRestart(String(e && e.message))) { spawn(); throw new Error(explain(e.message).title + " (" + e.message + ")"); }
        throw e;
      }
      const ms = Math.round(performance.now() - t0);
      let out;
      try { out = JSON.parse(m.json); } catch (_) { throw new Error("The engine returned an unreadable result."); }
      return normalise(out, m, ms);
    },
  };
}

/** The engine's "table" JSON -> { kind, columns, rows: string[][], boolean?, stats }. */
function normalise(out, msg, ms) {
  const rs = (out && (out.remote || out.stats)) || (msg && msg.stats) || null;
  const stats = {
    bytes: rs && rs.bytes != null ? Number(rs.bytes) : null,
    requests: rs && rs.requests != null ? Number(rs.requests) : null,
    ms,
  };
  if (!out || out.kind === "ask") return { kind: "ask", columns: [], rows: [], boolean: !!(out && out.boolean), truncated: false, stats };
  if (out.kind === "construct") {
    return { kind: "construct", columns: ["subject", "predicate", "object"], rows: (out.triples || []).map((t) => t.map((x) => (x == null ? "" : String(x)))), truncated: false, stats };
  }
  const vars = out.vars || [];
  return { kind: "select", columns: vars, rows: (out.rows || []).map((row) => vars.map((v) => (row[v] == null ? "" : String(row[v])))), truncated: false, stats };
}

// ── starter queries ─────────────────────────────────────────────────────────
function genericStarters(hasGraphs, typePred) {
  const q = hasGraphs
    ? [
        { title: "Sample statements", sparql: `SELECT ?g ?s ?p ?o WHERE {\n  GRAPH ?g { ?s ?p ?o }\n} LIMIT 20` },
        { title: "Most used predicates", sparql: `SELECT ?p (COUNT(*) AS ?n) WHERE {\n  GRAPH ?g { ?s ?p ?o }\n} GROUP BY ?p ORDER BY DESC(?n) LIMIT 25` },
        { title: "Most used classes", sparql: `SELECT ?c (COUNT(*) AS ?n) WHERE {\n  GRAPH ?g { ?s ${typePred} ?c }\n} GROUP BY ?c ORDER BY DESC(?n) LIMIT 25` },
      ]
    : [
        { title: "Sample statements", sparql: `SELECT ?s ?p ?o WHERE {\n  ?s ?p ?o\n} LIMIT 20` },
        { title: "Most used predicates", sparql: `SELECT ?p (COUNT(*) AS ?n) WHERE {\n  ?s ?p ?o\n} GROUP BY ?p ORDER BY DESC(?n) LIMIT 25` },
        { title: "Most used classes", sparql: `SELECT ?c (COUNT(*) AS ?n) WHERE {\n  ?s ${typePred} ?c\n} GROUP BY ?c ORDER BY DESC(?n) LIMIT 25` },
      ];
  return q.map((x) => ({ ...x, group: "Quick look" }));
}

const DIMENSIONS = { overview: "Overview", graphs: "Named graphs", schema: "Schema", structure: "Structure", content: "Content" };
function buildStarters(card, hasGraphs, typePred) {
  const out = [...genericStarters(hasGraphs, typePred)];
  const seen = new Set(out.map((s) => s.sparql.trim()));
  const push = (s) => { const k = s.sparql.trim(); if (!k || seen.has(k)) return; seen.add(k); out.push(s); };
  for (const q of (card && card.queries) || []) {
    if (q && q.sparql) push({ title: q.title || q.id || "Query", question: q.question || "", group: DIMENSIONS[q.dimension] || (q.dimension ? q.dimension[0].toUpperCase() + q.dimension.slice(1) : "From the card"), sparql: q.sparql });
  }
  // example_queries are strings; a leading "# title" comment names them.
  for (const e of (card && card.example_queries) || []) {
    const text = typeof e === "string" ? e : e && e.sparql;
    if (!text) continue;
    const m = String(text).match(/^\s*#\s*(.+)\n/);
    const title = (e && e.title) || (m ? m[1].replace(/\s+$/, "") : "Example");
    push({ title: title.length > 90 ? title.slice(0, 89) + "…" : title, group: "Examples", sparql: String(text).trim() });
  }
  return out;
}

// ── tiny markdown (card descriptions) ───────────────────────────────────────
function mdInline(t) {
  return esc(t)
    .replace(/`([^`\n]+)`/g, "<code>$1</code>")
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^)\s]+|mailto:[^)\s]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/\*\*([^*\n]+)\*\*/g, "<strong>$1</strong>")
    .replace(/(^|[\s(])\*(?!\s)([^*\n]*?[^*\s])\*(?=$|[\s).,;:!?])/g, "$1<em>$2</em>");
}
function mdLite(src) {
  const lines = String(src || "").replace(/\r\n?/g, "\n").split("\n");
  const out = [];
  let para = [], list = null, code = null;
  const flushPara = () => { if (para.length) out.push(`<p>${mdInline(para.join(" "))}</p>`); para = []; };
  const flushList = () => { if (list) out.push(`<${list.t}>${list.items.map((i) => `<li>${mdInline(i)}</li>`).join("")}</${list.t}>`); list = null; };
  for (const line of lines) {
    if (/^\s*(```|~~~)/.test(line)) {
      if (code) { out.push(`<pre class="pz-code">${esc(code.join("\n"))}</pre>`); code = null; }
      else { flushPara(); flushList(); code = []; }
      continue;
    }
    if (code) { code.push(line); continue; }
    let m;
    if (!line.trim()) { flushPara(); flushList(); continue; }
    if ((m = line.match(/^\s{0,3}(#{1,6})\s+(.*)$/))) { flushPara(); flushList(); out.push(`<h3>${mdInline(m[2])}</h3>`); continue; }
    if ((m = line.match(/^\s*([-+*]|\d+[.)])\s+(.*)$/))) {
      flushPara();
      const t = /\d/.test(m[1]) ? "ol" : "ul";
      if (!list || list.t !== t) { flushList(); list = { t, items: [] }; }
      list.items.push(m[2]);
      continue;
    }
    flushList(); para.push(line.trim());
  }
  if (code) out.push(`<pre class="pz-code">${esc(code.join("\n"))}</pre>`);
  flushPara(); flushList();
  return out.join("");
}

function splitTitle(t) {
  const s = String(t || "");
  const m = s.match(/^(.*?)\s+[—–-]\s+(.*)$/);
  return m ? { name: m[1].trim(), sub: m[2].trim() } : { name: s.trim(), sub: "" };
}

// ── state ───────────────────────────────────────────────────────────────────
const root = $("#root");
const S = {
  key: "", entry: null, manifest: null,
  header: null, card: null, size: null, status: "none", cardErr: null, readInfo: null,
  hasCard: false, hasGraphs: false, starters: [], engine: null,
  running: false, runToken: 0, result: null, sortCol: -1, sortDir: 0, shown: 0,
};

let resolveReady;
const readyP = new Promise((r) => (resolveReady = r));

// ── the programmatic surface (webmcp.js reads this) ─────────────────────────
window.plazaDataset = {
  ready: readyP,
  summary() { return S.entry ? summaryOf() : null; },
  card() { return S.hasCard ? S.card : null; },
  schema() { return schemaOf(); },
  examples() { return S.starters.map((s) => ({ title: s.title, sparql: s.sparql })); },
  async query(sparql, opts = {}) {
    await readyP;
    if (!S.engine) throw new Error("This dataset page did not load; there is nothing to query.");
    const q = String(sparql || "").trim();
    if (!q) throw new Error("Empty query.");
    const limit = Math.max(1, Math.min(10000, Number(opts.limit) || 1000));
    await S.engine.ready();
    // opts.timeoutMs: give up, restart the engine (a running query cannot be
    // interrupted any other way) and reject, so a caller is never left waiting
    // on a query that scans a multi-GB remote file.
    const timeoutMs = Number(opts.timeoutMs) || 0;
    let timer = null;
    const run = S.engine.query(q);
    run.catch(() => {});
    const r = await (timeoutMs
      ? Promise.race([
          run,
          new Promise((_, reject) => {
            timer = setTimeout(() => {
              S.engine.restart();
              reject(new Error(`The query did not finish in ${Math.round(timeoutMs / 1000)} s and was stopped. Make it more selective (bind a subject or a predicate, add LIMIT to a SELECT).`));
            }, timeoutMs);
          }),
        ])
      : run
    ).finally(() => clearTimeout(timer));
    const truncated = r.rows.length > limit;
    return { columns: r.columns.slice(), rows: truncated ? r.rows.slice(0, limit) : r.rows, truncated, stats: r.stats };
  },
  async suggest(text, opts = {}) {
    await readyP;
    if (!S.engine) return [];
    const limit = Math.max(1, Math.min(50, Number(opts.limit) || 10));
    return S.engine.prefix(String(text || "").trim(), limit);
  },
  async showQuery(sparql, opts = {}) {
    await readyP;
    const ed = $("#sparql");
    if (!ed) throw new Error("The query editor is not available on this page.");
    ed.value = String(sparql || "");
    syncEditorHeight();
    $("#starter").value = "";
    $("#query").scrollIntoView({ behavior: reduceMotion() ? "auto" : "smooth", block: "start" });
    if (opts.run) await runVisible();
  },
};

/** Statements in the file: quads when it has named graphs (triple_count counts the default graph only). */
function statementCount() {
  const { card, header } = S;
  const q = card.quad_count != null ? card.quad_count : header ? header.quadCount : null;
  if (S.hasGraphs && q != null) return q;
  return card.triple_count != null ? card.triple_count : q;
}

/** Bytes of the whole dataset: all shards for a sharded one, else the file. */
function totalSize() {
  const { entry } = S;
  if (entry.shards && entry.size != null) return entry.size;
  return S.size != null ? S.size : entry.size != null ? entry.size : null;
}

function summaryOf() {
  const { entry, card, header } = S;
  const onts = S.hasCard ? usedOntologies(card, entry).map((o) => o.name) : [];
  return {
    key: entry.key,
    title: card.title || entry.title || entry.key,
    description: plainDescription(card.description || entry.blurb || ""),
    category: categoryOf(),
    tags: (entry.tags || []).slice(),
    licence: card.license || entry.license || null,
    triples: statementCount(),
    terms: card.term_count != null ? card.term_count : header ? header.termCount : null,
    bytes: totalSize(),
    vocabularies: onts,
    features: derivedFacets(card, entry).filter((f) => !(card.license || entry.license) || f !== (card.license || entry.license)),
    delivery: entry.shards ? "sharded" : entry.kind === "remote-lazy" ? "remote" : "bundled",
    url: absUrl(entry.rete),
    page: absUrl(`dataset.html?key=${enc(entry.key)}`),
  };
}
function schemaOf() {
  const card = S.card || {};
  const map = (list) => (list || []).map(([iri, count]) => ({ iri: stripAngles(iri), label: shorten(iri), count }));
  const used = {};
  const keep = new Set(["rdf", "rdfs", "owl", "xsd"]);
  for (const [p, ns] of Object.entries(PREFIXES)) {
    if (keep.has(p) || (card.vocabularies || []).includes(ns)) used[p] = ns;
  }
  return { classes: map(card.classes), predicates: map(card.predicates), prefixes: used };
}
function categoryOf() {
  const { entry } = S;
  return entry.category || ((entry.tags || []).some((t) => ["ontology", "owl", "schema", "obo", "rdfs"].includes(String(t).toLowerCase())) ? "ontology" : "dataset");
}

// ── main ────────────────────────────────────────────────────────────────────
main().catch((e) => {
  console.error(e);
  root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-err" role="alert"><strong>This page failed to load.</strong> ${esc((e && e.message) || e)}</div></div>`;
  resolveReady();
});

async function main() {
  S.key = new URLSearchParams(location.search).get("key") || "";
  let manifest;
  try {
    manifest = await fetch("plaza.json").then((r) => { if (!r.ok) throw new Error("HTTP " + r.status); return r.json(); });
  } catch (e) {
    root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-err" role="alert"><strong>The plaza listing could not be loaded.</strong> ${esc(e.message || e)} <a href="index.html">Back to all datasets.</a></div></div>`;
    resolveReady();
    return;
  }
  REMOTE_TOKEN = manifest.remoteToken || "";
  S.manifest = manifest;
  const entry = manifest.datasets.find((d) => d.key === S.key);
  if (!entry) {
    document.title = "Unknown dataset · rete plaza";
    root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-warn" role="alert"><strong>No dataset called <code>${esc(S.key)}</code>.</strong> <a href="index.html${S.key ? "?q=" + enc(S.key) : ""}">Search the plaza${S.key ? " for it" : ""}.</a></div></div>`;
    resolveReady();
    return;
  }
  S.entry = entry;
  document.title = `${splitTitle(entry.title || entry.key).name} · rete plaza`;

  // 1. the card: one or two range requests, never the graph.
  let r = null;
  try {
    r = await Promise.race([
      readReteCard(absUrl(entry.rete)),
      sleep(25000).then(() => { throw new Error("The host did not answer within 25 seconds."); }),
    ]);
  } catch (e) {
    S.cardErr = String((e && e.message) || e);
  }
  if (r) {
    S.header = r.header; S.size = r.size; S.status = r.status; S.readInfo = r;
    S.hasCard = r.status === "full" && !!r.card;
    S.card = r.card || liteCardFromHeader(r.header, entry);
    if (r.status === "error") S.cardErr = r.cardError || "The card could not be decoded.";
  } else {
    S.status = "unreachable";
    S.card = liteCardFromHeader({ quadCount: null, termCount: null, version: entry.formatVersion || null }, entry);
  }
  S.hasGraphs = !!(S.header && S.header.hasNamedGraphs) || (S.card.named_graph_count || 0) > 0;
  setPrefixes(buildPrefixes(S.card));
  const typePred = entry.typePredicate || "a";
  S.starters = buildStarters(S.hasCard ? S.card : null, S.hasGraphs, typePred);

  S.engine = createEngine(entry, () => absUrl(entry.rete));
  render();
  resolveReady();
  afterRender();
}

// ── render ──────────────────────────────────────────────────────────────────
const FILE_NOTE_BYTES = 200e6;
const PREFIX_TIMEOUT_MS = 15000;
const LAZY_OPEN_BYTES = 300e6; // above this the engine opens on first use

function render() {
  const { entry, card, header } = S;
  const t = splitTitle(card.title || entry.title || entry.key);
  const desc = plainDescription(card.description || entry.blurb || "");
  const cat = categoryOf();
  const lic = card.license || entry.license;
  const triples = statementCount();
  const terms = card.term_count != null ? card.term_count : header ? header.termCount : null;
  const graphs = card.named_graph_count || (header && header.namedGraphCount) || 0;
  const fver = header ? header.version : entry.formatVersion;
  const size = totalSize();
  const dlSize = S.size != null ? S.size : entry.size;
  const facets = derivedFacets(card, entry).filter((f) => !["remote", "bundled", "header-only"].includes(f) && f !== lic && f !== card.license);
  const delivery = entry.shards ? "sharded" : entry.kind === "remote-lazy" ? "remote" : "bundled";
  const source = card.source || (entry.links || []).find((l) => /source/i.test(l.label))?.url;
  const otherLinks = (entry.links || []).filter((l) => !/playground/i.test(l.label) && !/^source$/i.test(l.label) && l.url !== source);
  const fileUrl = httpsify(withToken(absUrl(entry.rete)));
  const hash = header && header.contentHash;
  const coh = card.coherence;
  const cohChip = coh && typeof coh.coherent === "boolean"
    ? `<span class="pz-chip ${coh.coherent ? "pz-chip-accent" : "dz-chip-warn"}">${coh.coherent ? "coherent" : `incoherent: ${fmt(coh.inconsistency_count || 0)}`}</span>` : "";
  const hasSchema = (card.classes && card.classes.length) || (card.class_links && card.class_links.length);

  root.innerHTML = `
  <section class="dz-hero" aria-labelledby="h1">
    <div class="wrap dz-hero-grid">
      <div class="dz-hero-text">
        <p class="pz-kicker">${cat === "ontology" ? "Ontology" : "Dataset"} · <span class="mono">${esc(entry.key)}</span></p>
        <h1 id="h1">${esc(t.name)}</h1>
        ${t.sub ? `<p class="dz-sub">${esc(t.sub)}</p>` : ""}
        ${desc ? `<div class="dz-desc" id="desc"><p>${esc(desc)}</p></div>${desc.length > 380 ? `<button type="button" class="pz-link" id="descMore" aria-expanded="false" aria-controls="desc">Read more</button>` : ""}` : ""}

        <div class="dz-chips">
          <span class="pz-chip pz-chip-accent">${esc(delivery === "sharded" ? "sharded, remote" : delivery)}</span>
          ${lic ? `<span class="pz-chip" title="Licence">${esc(lic)}</span>` : ""}
          ${cohChip}
          ${facets.map((f) => `<span class="pz-chip">${esc(f)}</span>`).join("")}
        </div>

        <div class="dz-stats" aria-label="Key figures">
          ${triples != null ? `<div class="pz-stat"><b>${fmt(triples)}</b><span>${entry.shards ? "triples in shard 1" : graphs ? "quads" : "triples"}</span></div>` : ""}
          ${terms != null ? `<div class="pz-stat"><b>${fmt(terms)}</b><span>terms</span></div>` : ""}
          ${graphs ? `<div class="pz-stat"><b>${fmt(graphs)}</b><span>named graphs</span></div>` : ""}
          ${size != null ? `<div class="pz-stat"><b>${esc(fmtBytes(size))}</b><span>${entry.shards ? "all shards" : "file size"}</span></div>` : ""}
          ${fver != null ? `<div class="pz-stat"><b>${esc(formatLabel(fver))}</b><span>format</span></div>` : ""}
        </div>

        <dl class="dz-meta">
          ${source ? `<div><dt>Source</dt><dd><a href="${esc(httpsify(source))}" target="_blank" rel="noopener">${esc(source.replace(/^https?:\/\//, "").replace(/\/$/, ""))}</a></dd></div>` : ""}
          ${hash ? `<div><dt>Content hash</dt><dd><code title="${esc(hash)}">${esc(hash.slice(0, 12))}…</code> <button type="button" class="dz-copy" data-copy="${esc(hash)}" aria-label="Copy the full content hash">Copy</button></dd></div>` : ""}
          ${card.source_date || card.version ? `<div><dt>Version</dt><dd>${esc(card.version || card.source_date)}</dd></div>` : ""}
        </dl>

        <div class="dz-actions-top">
          <a class="pz-btn pz-btn-primary" href="#query">Query this graph</a>
          <a class="pz-btn" href="${esc(fileUrl)}" ${isHttp(fileUrl) ? 'target="_blank" rel="noopener"' : ""} download>Download${entry.shards ? " shard 1" : ""}${dlSize != null ? ` (${esc(fmtBytes(dlSize))})` : ""}</a>
          <button type="button" class="pz-btn" id="copyUrl" data-copy="${esc(fileUrl)}">Copy ${entry.shards ? "shard 1 URL" : "file URL"}</button>
          <a class="pz-btn" href="${PLAYGROUND}#dataset=${enc(entry.key)}&amp;mode=sparql" target="_blank" rel="noopener">Open in playground</a>
        </div>
        ${size != null && size > FILE_NOTE_BYTES ? `<p class="dz-fine">This file is large. Queries read it in place, so downloading is rarely needed.</p>` : ""}
        ${otherLinks.length ? `<p class="dz-links">${otherLinks.map((l) => `<a href="${esc(httpsify(l.url))}" target="_blank" rel="noopener">${esc(l.label)} ↗</a>`).join(" ")}</p>` : ""}
      </div>
      <figure class="dz-art" aria-hidden="true">
        <div class="dz-art-img" id="art"></div>
        <figcaption>${hasSchema ? "Schema portrait: classes sized by instance count and coloured by vocabulary; arrows are the relations between them, the strongest ones named." : (card.predicates && card.predicates.length) ? "The card lists no classes, so this shows its most used properties." : "This file has no schema profile, so only its size is shown."}</figcaption>
      </figure>
    </div>
  </section>

  <div class="wrap dz-layout">
    <nav class="dz-nav" aria-label="On this page" id="secnav">
      <a href="#query">Query</a><a href="#schema">Schema</a><a href="#profile">Profile</a><a href="#files">Files</a><a href="#about">About</a>
    </nav>
    <div class="dz-content">
      ${degradedBanner()}
      ${querySection()}
      ${schemaSection(hasSchema)}
      ${profileSection()}
      ${filesSection()}
      ${aboutSection()}
    </div>
  </div>`;
}

function degradedBanner() {
  const { status, cardErr, entry } = S;
  const retry = `<button type="button" class="pz-btn pz-btn-sm" onclick="location.reload()">Try again</button>`;
  if (status === "unreachable")
    return `<div class="dz-banner dz-banner-err" role="alert"><div><strong>The file could not be reached.</strong> ${esc(String(cardErr || "").replace(/.?$/, "."))} The host may be offline, or may not allow cross-origin range requests. This page shows what the plaza listing knows; the figures from inside the file, the schema and the query box need the file.</div>${retry}</div>`;
  if (status === "none")
    return `<div class="dz-banner dz-banner-info"><div><strong>This file has no embedded card.</strong> The counts below come from its header. The title, description and licence come from the plaza listing, and there is no class or predicate profile. The starter queries in the query box list classes and predicates directly from the data.</div></div>`;
  if (status === "too-large")
    return `<div class="dz-banner dz-banner-info"><div><strong>The card in this file is too large to read here.</strong> It is ${esc(fmtBytes(S.readInfo.cardRange.length))}, over the ${esc(fmtBytes(1.5e6))} this page fetches, so only header figures and listing data are shown. The query box still works.</div></div>`;
  if (status === "error")
    return `<div class="dz-banner dz-banner-warn"><div><strong>The card could not be read.</strong> ${esc(cardErr || "")} Header figures and listing data are shown instead.</div>${retry}</div>`;
  return "";
}

function querySection() {
  const { entry, header } = S;
  let note;
  if (entry.shards) {
    const first = entry.shards[0] || entry.rete;
    note = `<div class="dz-banner dz-banner-info" id="shardNote"><div><strong>Sharded graph.</strong> This graph is split across ${entry.shards.length} independent files. This page opens only the first one (<code>${esc(first.split("/").pop())}</code>), so answers cover that shard alone. Use the <a href="${PLAYGROUND}#dataset=${enc(entry.key)}&amp;mode=sparql" target="_blank" rel="noopener">playground</a> to query all shards together.</div></div>`;
  } else if (entry.kind === "remote-lazy") {
    note = `<p class="dz-lede">Reads the file in place over HTTP range requests and fetches only the bytes a query touches. Selective lookups are cheap; whole-graph aggregates read much more.</p>`;
  } else {
    note = `<p class="dz-lede">This small file is loaded into the page (${esc(fmtBytes(S.size != null ? S.size : entry.size) || "")}) and queried locally.</p>`;
  }
  const options = [];
  const groups = new Map();
  S.starters.forEach((s, i) => { if (!groups.has(s.group)) groups.set(s.group, []); groups.get(s.group).push([i, s]); });
  for (const [g, items] of groups) options.push(`<optgroup label="${esc(g)}">${items.map(([i, s]) => `<option value="${i}">${esc(s.title)}</option>`).join("")}</optgroup>`);

  return `
  <section class="dz-sec" id="query" aria-labelledby="h-query">
    <div class="dz-sec-head">
      <h2 id="h-query">Query</h2>
      <span class="dz-agent" id="agentBadge" hidden title="This page registers WebMCP tools that browser agents can call">Agent tools available</span>
      <span class="dz-engine" id="engine" data-state="idle" role="status" aria-live="polite"><i aria-hidden="true"></i><span id="engineTxt">Engine starts when the page is ready</span></span>
    </div>
    ${note}
    <div class="dz-q pz-panel">
      <div class="dz-find">
        <label for="ac">Find an entity by name</label>
        <div class="dz-ac">
          <input id="ac" class="pz-input" type="search" role="combobox" aria-expanded="false" aria-controls="aclist" aria-autocomplete="list" autocomplete="off" spellcheck="false" placeholder="Type at least two letters of a label" />
          <div class="dz-aclist" id="aclist" role="listbox" aria-label="Matching entities" hidden></div>
        </div>
      </div>
      <div class="dz-pick">
        <label for="starter">Starter queries</label>
        <select id="starter" class="pz-input">
          <option value="">Choose a starter…</option>
          ${options.join("")}
        </select>
        <p class="dz-fine" id="starterQ"></p>
      </div>
      <div class="dz-editor-wrap">
        <label class="pz-sr" for="sparql">SPARQL query</label>
        <textarea id="sparql" class="dz-editor" spellcheck="false" autocapitalize="off" autocomplete="off" rows="8" aria-describedby="runHint">${esc(S.starters[0].sparql)}</textarea>
      </div>
      <p class="dz-banner dz-banner-warn" id="constructWarn" hidden>In this engine build LIMIT is not applied to CONSTRUCT, so on a large file it can read most of the graph. Sample with a SELECT and LIMIT instead, or use Cancel if it runs long.</p>
      <div class="dz-run">
        <button type="button" class="pz-btn pz-btn-primary" id="run">Run query</button>
        <button type="button" class="pz-btn" id="cancel" hidden>Cancel</button>
        <button type="button" class="pz-btn" id="shareQ">Copy link to this query</button>
        <span class="dz-fine" id="runHint"><kbd class="pz-kbd">Ctrl</kbd> + <kbd class="pz-kbd">Enter</kbd> runs</span>
      </div>
      <div class="dz-banner dz-banner-err" id="engineErr" role="alert" hidden></div>
      <div class="dz-status" id="status" role="status" aria-live="polite"></div>
      <div class="dz-res" id="res" hidden>
        <div class="dz-res-bar">
          <div class="dz-res-stat" id="resStat"></div>
          <div class="dz-res-tools" id="resTools">
            <button type="button" class="pz-btn pz-btn-sm" data-export="csv">Copy CSV</button>
            <button type="button" class="pz-btn pz-btn-sm" data-export="tsv">Copy TSV</button>
            <button type="button" class="pz-btn pz-btn-sm" data-export="download">Download CSV</button>
          </div>
        </div>
        <div class="dz-table-wrap" id="tableWrap" tabindex="0" role="region" aria-label="Query results"></div>
        <button type="button" class="pz-btn pz-btn-sm" id="moreRows" hidden>Show more rows</button>
      </div>
    </div>
  </section>`;
}

function schemaSection(hasSchema) {
  let body;
  if (hasSchema) {
    body = `<div class="dz-schema">
        <div id="schemaGraph" class="schema-graph"></div>
        <div id="schemaInfo" class="schema-info"></div>
      </div>
      <p class="dz-fine">Boxes are classes and arrows are object properties, drawn from the card. Click a class to list its literal properties; scroll to zoom.</p>`;
  } else {
    body = `<p class="dz-empty">${noProfileText("schema diagram")}</p>${starterButtons([["Most used classes", 2], ["Most used predicates", 1]])}`;
  }
  return `
  <section class="dz-sec" id="schema" aria-labelledby="h-schema">
    <div class="dz-sec-head"><h2 id="h-schema">Schema</h2>${hasSchema ? `<button type="button" class="pz-btn pz-btn-sm" id="schemaFs">Open full screen</button>` : ""}</div>
    ${body}
  </section>`;
}

function noProfileText(what) {
  switch (S.status) {
    case "none": return `This file has no embedded card, so there is no ${what} to show. The query box can still list its classes and predicates.`;
    case "too-large": return `The card is too large to read here, so there is no ${what}. The query box can still list classes and predicates.`;
    case "error": return `The card could not be read, so there is no ${what}.`;
    case "unreachable": return `The file could not be reached, so there is no ${what}.`;
    default: return `The card carries no class profile, so there is no ${what}. The query box can still list classes and predicates.`;
  }
}
function starterButtons(pairs) {
  if (S.status === "unreachable") return "";
  return `<p class="dz-chips">${pairs.map(([label, idx]) => `<button type="button" class="pz-chip" data-starter="${idx}">${esc(label)}</button>`).join("")}</p>`;
}

function bars(pairs, kind) {
  const top = pairs.slice(0, 200);
  const max = Math.max(1, ...top.map((p) => p[1]));
  return `<ul class="dz-bars" data-kind="${kind}">${top.map(([iri, n], i) => `
    <li${i >= 12 ? ' class="dz-more" hidden' : ""}>
      <button type="button" data-iri="${esc(stripAngles(iri))}" title="${esc(stripAngles(iri))}. Put a query for it in the editor.">
        <span class="lbl">${esc(shorten(iri))}</span><span class="n">${fmt(n)}</span>
        <span class="track"><i style="width:${Math.max(2, (n / max) * 100)}%"></i></span>
      </button>
    </li>`).join("")}</ul>${top.length > 12 ? `<button type="button" class="pz-link dz-showall">Show all ${top.length}</button>` : ""}`;
}

function profileSection() {
  const { card, entry } = S;
  const classes = card.classes || [], preds = card.predicates || [];
  const onts = S.hasCard ? usedOntologies(card, entry) : [];
  const raw = card.vocabularies || [];
  const provs = S.hasCard ? detectProviders(card, entry) : [];
  const sig = card.signals || {};
  const langs = (card.languages || []).filter((l) => l[0]);
  const dts = (card.datatypes || []).slice(0, 8);
  const sigRows = [];
  if (sig.geo_wkt || sig.geo_latlong) sigRows.push(["Geometry", sig.geo_wkt ? "WKT literals" : "latitude and longitude"]);
  if (sig.temporal_extent) sigRows.push(["Time span", `${esc(sig.temporal_extent[0])} to ${esc(sig.temporal_extent[1])}`]);
  if (sig.spatial_bbox) sigRows.push(["Bounding box", `[${sig.spatial_bbox.map((x) => Number(x).toFixed(2)).join(", ")}]`]);
  if (sig.default_lang) sigRows.push(["Default language", esc(sig.default_lang)]);
  if (langs.length) sigRows.push(["Languages", `${esc(langs.slice(0, 8).map((l) => l[0]).join(", "))}${langs.length > 8 ? ` and ${langs.length - 8} more` : ""}`]);
  if (dts.length) sigRows.push(["Datatypes", dts.map((d) => `<code>${esc(shorten(d[0]))}</code>`).join(" ")]);
  if (sig.label_predicate) sigRows.push(["Label predicate", `<code>${esc(shorten(sig.label_predicate))}</code>`]);

  let body;
  if (!classes.length && !preds.length) {
    body = `<p class="dz-empty">${noProfileText("class or predicate profile")}</p>${starterButtons([["Most used classes", 2], ["Most used predicates", 1]])}`;
  } else {
    body = `<div class="dz-cols">
        ${classes.length ? `<div><h3>Classes</h3>${bars(classes, "class")}</div>` : ""}
        ${preds.length ? `<div><h3>Predicates</h3>${bars(preds, "pred")}</div>` : ""}
      </div>
      <p class="dz-fine">Counts come from the card${card.truncated ? ` (top ${fmt(card.top_n || 100)} of each)` : ""}. Click a row to put a query for it in the editor.</p>`;
  }
  return `
  <section class="dz-sec" id="profile" aria-labelledby="h-profile">
    <div class="dz-sec-head"><h2 id="h-profile">Profile</h2></div>
    ${body}
    ${onts.length || raw.length ? `<h3>Vocabularies</h3>
      <p class="dz-chips">${onts.map((o) => `<a class="pz-chip" href="ontology.html?id=${enc(o.name)}">${esc(o.name)}</a>`).join("")}</p>
      ${raw.length ? `<details class="dz-details"><summary>${raw.length} namespaces in the card</summary>
        <ul class="dz-ns">${raw.map((ns) => { const p = Object.entries(PREFIXES).find(([, v]) => v === ns); return `<li><code>${p ? esc(p[0]) + ":" : ""}</code> <span class="mono">${esc(ns)}</span></li>`; }).join("")}</ul></details>` : ""}` : ""}
    ${S.hasCard ? `<h3>Connected to</h3>${provs.length
      ? `<p class="dz-chips">${provs.map((pv) => `<a class="pz-chip" href="${esc(httpsify(pv.url))}" target="_blank" rel="noopener">${esc(pv.name)}${pv.note ? ` <em>${esc(pv.note)}</em>` : ""}</a>`).join("")}</p>
         <p class="dz-fine">External identifier providers, detected from the IRIs this dataset uses and from curated cross-references.</p>`
      : `<p class="dz-fine">No external identifier providers detected.</p>`}` : ""}
    ${sigRows.length ? `<h3>Signals</h3><dl class="dz-kv">${sigRows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>` : ""}
  </section>`;
}

function fileRow(type, label, url, meta, extra = "") {
  const u = httpsify(withToken(absUrl(url)));
  return `<li class="dz-file">
    <span class="dz-ft dz-ft-${esc(type)}">${esc(type)}</span>
    <span class="dz-file-main"><span class="dz-file-name">${esc(label)}</span>${meta ? `<span class="dz-fine">${meta}</span>` : ""}</span>
    <span class="dz-file-act">${extra}<a class="pz-btn pz-btn-sm" href="${esc(u)}" target="_blank" rel="noopener">${/\/$/.test(u) ? "Open" : "Download"}</a><button type="button" class="pz-btn pz-btn-sm" data-copy="${esc(u)}">Copy URL</button></span>
  </li>`;
}
function filesSection() {
  const { entry, header } = S;
  const size = S.size != null ? S.size : entry.size;
  const rows = [];
  if (entry.shards) {
    entry.shards.forEach((u, i) => rows.push(fileRow("rete", `Shard ${i + 1}: ${u.split("/").pop()}`, u, i === 0 ? "The shard this page queries" : "Not queried on this page")));
  } else {
    rows.push(fileRow("rete", entry.rete.split("/").pop(), entry.rete, [size != null ? fmtBytes(size) : "", header ? `format ${formatLabel(header.version)}` : entry.formatVersion != null ? `format ${formatLabel(entry.formatVersion)}` : "", header ? `${fmt(header.quadCount)} statements` : ""].filter(Boolean).join(" · ")));
  }
  for (const c of entry.companions || []) {
    rows.push(fileRow(c.kind, c.label || c.kind, c.url, [c.size, c.note, c.verified === false ? "link by convention, not checked" : ""].filter(Boolean).map(esc).join(" · ")));
  }
  if (entry.pmtiles) rows.push(fileRow("pmtiles", "Vector tiles", entry.pmtiles, "PMTiles basemap for map output"));
  const hasParquet = (entry.companions || []).some((c) => c.kind === "parquet");
  return `
  <section class="dz-sec" id="files" aria-labelledby="h-files">
    <div class="dz-sec-head"><h2 id="h-files">Files</h2></div>
    <ul class="dz-files">${rows.join("")}</ul>
    ${hasParquet ? `<h3>Explore the tables</h3><div id="tblExplore" class="dz-tbl"></div>` : ""}
  </section>`;
}

function aboutSection() {
  const { card, header, entry, readInfo } = S;
  const rows = [];
  const link = (u) => `<a href="${esc(httpsify(u))}" target="_blank" rel="noopener">${esc(String(u).replace(/^https?:\/\//, ""))}</a>`;
  if (card.source) rows.push(["Source", isHttp(card.source) ? link(card.source) : esc(card.source)]);
  if (card.publisher && card.publisher.name) rows.push(["Publisher", esc(card.publisher.name)]);
  if (Array.isArray(card.creators) && card.creators.length) rows.push(["Creators", card.creators.map((c) => (c.orcid ? `<a href="${esc(httpsify(c.orcid))}" target="_blank" rel="noopener">${esc(c.name || c.orcid)}</a>` : esc(c.name || String(c)))).join(", ")]);
  if (card.source_date || card.version) rows.push(["Version", esc(card.version || card.source_date)]);
  if (card.created) rows.push(["Created", esc(card.created)]);
  if (card.doi) rows.push(["DOI", link(card.doi)]);
  if (card.cite_as) rows.push(["Cite as", `${esc(card.cite_as)} <button type="button" class="dz-copy" data-copy="${esc(card.cite_as)}">Copy</button>`]);
  if (Array.isArray(card.derived_from) && card.derived_from.length) rows.push(["Derived from", card.derived_from.map((u) => (isHttp(u) ? link(u) : esc(u))).join("<br>")]);
  if (card.canonical_url) rows.push(["Canonical URL", link(card.canonical_url)]);
  if (card.license || entry.license) rows.push(["Licence", esc(card.license || entry.license)]);
  if (Array.isArray(card.keywords) && card.keywords.length) rows.push(["Keywords", card.keywords.map((k) => `<span class="pz-chip">${esc(k)}</span>`).join(" ")]);
  if (card.signals && card.signals.base_iri) rows.push(["Base IRI", `<code>${esc(card.signals.base_iri)}</code>`]);
  if (header) rows.push(["Content hash", `<code>${esc(header.contentHash)}</code> <button type="button" class="dz-copy" data-copy="${esc(header.contentHash)}">Copy</button>`]);
  if (card.extra && card.extra.conversion && card.extra.conversion.converter) rows.push(["Conversion", esc(card.extra.conversion.converter)]);
  const readLine = readInfo
    ? `Reading this page's card cost ${esc(fmtBytes(readInfo.bytesFetched))} in ${readInfo.requests} request${readInfo.requests === 1 ? "" : "s"}. The graph itself was not fetched.`
    : "";
  const descFull = S.hasCard && card.description ? card.description : "";
  return `
  <section class="dz-sec" id="about" aria-labelledby="h-about">
    <div class="dz-sec-head"><h2 id="h-about">About</h2></div>
    ${descFull ? `<div class="dz-md">${mdLite(descFull)}</div>` : ""}
    ${rows.length ? `<dl class="dz-kv">${rows.map(([k, v]) => `<dt>${k}</dt><dd>${v}</dd>`).join("")}</dl>` : `<p class="dz-fine">No provenance fields in the card.</p>`}
    ${readLine ? `<p class="dz-fine">${readLine}</p>` : ""}
    ${S.hasCard ? `<details class="dz-details" id="rawCard"><summary>Raw card JSON</summary><div class="dz-raw"><button type="button" class="pz-btn pz-btn-sm" id="copyRaw">Copy JSON</button><pre class="pz-code" id="rawPre"></pre></div></details>` : ""}
  </section>`;
}

// ── after render: behaviour ─────────────────────────────────────────────────
function afterRender() {
  const { entry, card } = S;
  wireCommon();
  wireQuery();
  wireProfile();
  wireNav();

  // artwork
  (async () => {
    try {
      const info = imageInfoFromCard(card, entry, S.header);
      const url = await renderFingerprint(info, { theme: "dark", w: 960, h: 600, labels: true });
      const el = $("#art");
      if (el && url) el.innerHTML = `<img src="${url}" alt="" width="960" height="600" decoding="async" />`;
    } catch (_) { /* artwork is decoration */ }
  })();

  // schema
  const g = $("#schemaGraph"), i = $("#schemaInfo");
  if (g && i) { try { mountSchemaUML(card, g, i).catch(() => {}); } catch (_) {} }
  wireSchemaFs();

  // companion tables
  const tbl = $("#tblExplore");
  if (tbl) { try { mountTableExplorer(tbl, entry, REMOTE_TOKEN); } catch (_) {} }

  // engine: start now unless the file was unreachable
  if (S.status === "unreachable") {
    paintEngine("error", { title: "The file could not be reached, so there is nothing to query.", detail: S.cardErr || "", raw: S.cardErr || "" });
    $("#run").disabled = true; $("#ac").disabled = true;
  } else {
    S.engine.onChange(paintEngine);
    const start = () => S.engine.start();
    paintEngine("idle");
    // Small files open at once. Opening a big one reads megabytes of index
    // (about 8 MB for the 1.4 GB Wikidata slice), so it waits for the first
    // sign of use: focus in the query box, a run, or a call to the API.
    const size = S.size != null ? S.size : entry.size || 0;
    if (size <= LAZY_OPEN_BYTES) {
      if ("requestIdleCallback" in window) requestIdleCallback(start, { timeout: 1500 }); else setTimeout(start, 200);
    } else {
      $("#engineTxt").textContent = "Large file: opens when you use the query box";
      $(".dz-q").addEventListener("focusin", start, { once: true });
      $(".dz-q").addEventListener("pointerdown", start, { once: true });
    }
  }
}

function wireCommon() {
  document.addEventListener("click", async (e) => {
    const b = e.target.closest("[data-copy]");
    if (!b) return;
    const ok = await copyText(b.dataset.copy);
    flash(b, "Copied", ok);
  });
  const more = $("#descMore");
  if (more) more.addEventListener("click", () => {
    const d = $("#desc"), open = d.classList.toggle("open");
    more.textContent = open ? "Show less" : "Read more";
    more.setAttribute("aria-expanded", String(open));
  });
  const raw = $("#rawCard");
  if (raw) {
    raw.addEventListener("toggle", () => { if (raw.open && !$("#rawPre").textContent) $("#rawPre").textContent = JSON.stringify(S.card, null, 2); });
    $("#copyRaw").addEventListener("click", async (e) => flash(e.currentTarget, "Copied", await copyText(JSON.stringify(S.card, null, 2))));
  }
  document.addEventListener("plaza:webmcp", (e) => showAgentBadge(e.detail));
  showAgentBadge();
}
function showAgentBadge(detail) {
  const d = detail || window.plazaWebMCP;
  const b = $("#agentBadge");
  if (b && d && d.supported && d.tools && d.tools.length) { b.hidden = false; b.title = `Browser agents can use ${d.tools.length} tools on this page: ${d.tools.join(", ")}`; }
}

function wireNav() {
  const links = $$("#secnav a");
  const secs = links.map((a) => $(a.getAttribute("href"))).filter(Boolean);
  if (!("IntersectionObserver" in window)) return;
  const io = new IntersectionObserver((ents) => {
    const vis = ents.filter((x) => x.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
    if (!vis) return;
    links.forEach((a) => (a.getAttribute("href") === "#" + vis.target.id ? a.setAttribute("aria-current", "true") : a.removeAttribute("aria-current")));
  }, { rootMargin: "-80px 0px -60% 0px" });
  secs.forEach((s) => io.observe(s));
}

function wireSchemaFs() {
  const btn = $("#schemaFs"), modal = $("#modal");
  if (!btn || !modal) return;
  let opener = null;
  const close = () => { modal.hidden = true; $("#modalCard").innerHTML = ""; if (opener) opener.focus(); };
  btn.addEventListener("click", () => {
    opener = btn;
    $("#modalCard").innerHTML = `
      <button class="modal-x" id="modalX" aria-label="Close">&times;</button>
      <h2 class="dz-modal-h">Schema: ${esc(splitTitle(S.card.title || S.entry.title || S.entry.key).name)}</h2>
      <div class="dz-schema modal-schema"><div id="schemaGraphFs" class="schema-graph"></div><div id="schemaInfoFs" class="schema-info"></div></div>`;
    modal.hidden = false;
    $("#modalX").onclick = close;
    $("#modalX").focus();
    mountSchemaUML(S.card, $("#schemaGraphFs"), $("#schemaInfoFs")).catch(() => {});
  });
  modal.addEventListener("click", (e) => { if (e.target === modal) close(); });
  document.addEventListener("keydown", (e) => { if (e.key === "Escape" && !modal.hidden) close(); });
}

function wireProfile() {
  document.addEventListener("click", (e) => {
    const sh = e.target.closest(".dz-showall");
    if (sh) {
      const ul = sh.previousElementSibling;
      const hidden = $$(".dz-more", ul);
      const show = hidden[0] && hidden[0].hidden;
      hidden.forEach((li) => (li.hidden = !show));
      sh.textContent = show ? "Show fewer" : `Show all ${$$("li", ul).length}`;
      return;
    }
    const b = e.target.closest(".dz-bars button");
    if (b) {
      const iri = b.dataset.iri, kind = b.closest(".dz-bars").dataset.kind;
      const t = `<${iri}>`;
      const q = S.hasGraphs
        ? kind === "class" ? `SELECT ?s WHERE {\n  GRAPH ?g { ?s ${S.entry.typePredicate || "a"} ${t} }\n} LIMIT 50` : `SELECT ?s ?o WHERE {\n  GRAPH ?g { ?s ${t} ?o }\n} LIMIT 50`
        : kind === "class" ? `SELECT ?s WHERE {\n  ?s ${S.entry.typePredicate || "a"} ${t}\n} LIMIT 50` : `SELECT ?s ?o WHERE {\n  ?s ${t} ?o\n} LIMIT 50`;
      window.plazaDataset.showQuery(q, { run: false });
      $("#sparql").focus({ preventScroll: true });
      return;
    }
    const st = e.target.closest("[data-starter]");
    if (st) {
      const s = S.starters[+st.dataset.starter];
      if (s) { window.plazaDataset.showQuery(s.sparql, { run: true }); }
    }
  });
}

// ── query block ─────────────────────────────────────────────────────────────
const ENGINE_TEXT = {
  idle: "Engine starts when the page is ready",
  opening: "Opening the file…",
  ready: "Ready",
  error: "Cannot open this file",
};
function paintEngine(state, err) {
  const el = $("#engine");
  if (!el) return;
  el.dataset.state = state;
  $("#engineTxt").textContent = state === "opening" && S.entry.kind === "remote-lazy" ? "Opening the file: reading its header…" : state === "opening" ? "Loading the file into the engine…" : ENGINE_TEXT[state] || state;
  const box = $("#engineErr");
  const runBtn = $("#run");
  if (state === "error") {
    const e = err || S.engine.error || { title: "Unknown error", detail: "", raw: "" };
    box.hidden = false;
    box.innerHTML = `<div><strong>${esc(e.title)}</strong>${e.detail && e.detail !== e.title ? `<details class="dz-details"><summary>Engine message</summary><pre class="pz-code">${esc(e.raw || e.detail)}</pre></details>` : ""}
      <p class="dz-fine">The rest of the page still works. You can <a href="${PLAYGROUND}#dataset=${enc(S.entry.key)}&amp;mode=sparql" target="_blank" rel="noopener">try the playground</a>${S.entry.rete ? ` or <a href="${esc(httpsify(absUrl(S.entry.rete)))}">download the file</a>` : ""}.</p></div>
      ${S.status === "unreachable" ? "" : `<button type="button" class="pz-btn pz-btn-sm" id="retryEngine">Try again</button>`}`;
    const rb = $("#retryEngine");
    if (rb) rb.onclick = () => { box.hidden = true; S.engine.restart(); };
    runBtn.disabled = true;
    $("#ac").disabled = true;
  } else {
    box.hidden = true;
    if (!S.running) runBtn.disabled = false;
    $("#ac").disabled = false;
  }
}

function syncEditorHeight() {
  const ed = $("#sparql");
  if (!ed) return;
  const lines = Math.min(22, Math.max(8, ed.value.split("\n").length + 1));
  ed.rows = lines;
  const big = (S.size != null ? S.size : (S.entry && S.entry.size) || 0) > 100e6;
  const w = $("#constructWarn");
  if (w) w.hidden = !(big && /^\s*(?:(?:PREFIX|BASE)\b[^\n]*\n|#[^\n]*\n|\s*\n)*\s*CONSTRUCT\b/i.test(ed.value));
}

function writeHash(q) {
  try { history.replaceState(null, "", location.pathname + location.search + "#q=" + enc(q)); } catch (_) {}
}
function queryFromUrl() {
  const h = new URLSearchParams(location.hash.replace(/^#/, "")).get("q");
  return h || new URLSearchParams(location.search).get("q") || "";
}

let tick = 0;
function setStatus(html, busy) {
  const s = $("#status");
  s.innerHTML = html ? `${busy ? `<span class="dz-spin" aria-hidden="true"></span>` : ""}${html}` : "";
}

async function runVisible() {
  if (S.running) return;
  const ed = $("#sparql");
  const q = ed.value.trim();
  if (!q) { setStatus("Write a query first."); return; }
  writeHash(q);
  setQueryPrefixes(q);
  S.running = true;
  const token = ++S.runToken;
  $("#run").disabled = true;
  $("#cancel").hidden = false;
  const t0 = performance.now();
  const phase = () => {
    const st = S.engine.state;
    const secs = ((performance.now() - t0) / 1000).toFixed(1);
    return st === "ready" ? `Reading from the file and evaluating… ${secs} s` : st === "opening" ? `Opening the file first… ${secs} s` : `Starting the engine… ${secs} s`;
  };
  setStatus(phase(), true);
  tick = setInterval(() => { if (token === S.runToken) setStatus(phase(), true); }, 250);
  try {
    await S.engine.ready();
    const out = await S.engine.query(q);
    if (token !== S.runToken) return;
    showResult(out);
  } catch (e) {
    if (token !== S.runToken) return;
    showError(e);
  } finally {
    if (token === S.runToken) {
      clearInterval(tick);
      S.running = false;
      $("#cancel").hidden = true;
      $("#run").disabled = S.engine.state === "error";
    }
  }
}
function setQueryPrefixes(q) {
  setPrefixes(buildPrefixes(S.card), queryPrefixes(q));
}

function cancelRun() {
  if (!S.running) return;
  S.runToken++;
  clearInterval(tick);
  S.running = false;
  $("#cancel").hidden = true;
  $("#run").disabled = false;
  setStatus("Cancelled. The engine is reopening the file; the next query waits for it.");
  S.engine.restart();
}

function showError(e) {
  S.result = null;
  $("#res").hidden = true;
  const msg = String((e && e.message) || e);
  setStatus(`<div class="dz-banner dz-banner-err" role="alert"><div><strong>The query failed.</strong><pre class="dz-errpre">${esc(msg)}</pre></div></div>`);
}

function fmtStats(r, shown) {
  const parts = [];
  const rows = r.kind === "ask" ? "ASK" : `${fmt(r.rows.length)} row${r.rows.length === 1 ? "" : "s"}`;
  parts.push(`<strong>${rows}</strong>`);
  if (r.stats.bytes != null) parts.push(`${esc(fmtBytes(r.stats.bytes))} read`);
  if (r.stats.requests != null) parts.push(`${fmt(r.stats.requests)} request${r.stats.requests === 1 ? "" : "s"}`);
  parts.push(`${(r.stats.ms / 1000).toFixed(r.stats.ms < 10000 ? 2 : 1)} s`);
  return parts.join(" · ");
}

function showResult(r) {
  S.result = r; S.sortCol = -1; S.sortDir = 0; S.order = r.rows.map((_, i) => i); S.shown = 500;
  setStatus("");
  const res = $("#res");
  res.hidden = false;
  $("#resStat").innerHTML = fmtStats(r);
  $("#resTools").hidden = r.kind === "ask" || !r.rows.length;
  if (r.kind === "ask") {
    $("#tableWrap").innerHTML = `<p class="dz-ask">The answer is <strong>${r.boolean ? "true" : "false"}</strong>.</p>`;
    $("#moreRows").hidden = true;
    return;
  }
  if (!r.rows.length) {
    $("#tableWrap").innerHTML = `<p class="dz-empty">No rows. ${S.hasGraphs ? "This graph keeps its data in named graphs, so patterns usually need a <code>GRAPH ?g { … }</code> wrapper." : "Check the IRIs and prefixes in the pattern."}</p>`;
    $("#moreRows").hidden = true;
    return;
  }
  paintTable();
  const first = res.querySelector("table");
  if (first && !reduceMotion()) { /* keep the editor in view; results sit just below it */ }
}

function paintTable() {
  const r = S.result;
  const idx = S.order.slice(0, S.shown);
  const head = r.columns.map((c, i) => {
    const dir = S.sortCol === i ? (S.sortDir > 0 ? "ascending" : "descending") : "none";
    return `<th scope="col" aria-sort="${dir}"><button type="button" data-col="${i}">${esc(c)}<span class="dz-sort" aria-hidden="true">${dir === "ascending" ? "▲" : dir === "descending" ? "▼" : ""}</span></button></th>`;
  }).join("");
  const body = idx.map((ri) => `<tr>${r.rows[ri].map((c) => `<td>${cellHtml(c)}</td>`).join("")}</tr>`).join("");
  $("#tableWrap").innerHTML = `<table class="dz-table"><thead><tr>${head}</tr></thead><tbody>${body}</tbody></table>`;
  const more = $("#moreRows");
  more.hidden = S.shown >= S.order.length;
  more.textContent = `Show more rows (${fmt(Math.min(500, S.order.length - S.shown))} of ${fmt(S.order.length - S.shown)} left)`;
}

function sortBy(col) {
  const r = S.result;
  if (S.sortCol === col) S.sortDir = S.sortDir === 1 ? -1 : S.sortDir === -1 ? 0 : 1; else { S.sortCol = col; S.sortDir = 1; }
  if (S.sortDir === 0) { S.sortCol = -1; S.order = r.rows.map((_, i) => i); }
  else {
    const keys = r.rows.map((row) => sortKey(row[col]));
    const dir = S.sortDir;
    S.order = r.rows.map((_, i) => i).sort((a, b) => {
      const x = keys[a], y = keys[b];
      if (x.n != null && y.n != null && x.n !== y.n) return (x.n - y.n) * dir;
      if (x.s === "" && y.s !== "") return 1; // empty values last either way
      if (y.s === "" && x.s !== "") return -1;
      return x.s < y.s ? -dir : x.s > y.s ? dir : a - b;
    });
  }
  paintTable();
}

function toDelimited(sep) {
  const r = S.result;
  const q = (v) => { const s = String(v); return sep === "," ? (/[",\n\r]/.test(s) ? `"${s.replace(/"/g, '""')}"` : s) : s.replace(/[\t\r\n]+/g, " "); };
  const rows = S.order.map((i) => r.rows[i]);
  return [r.columns.map(q).join(sep), ...rows.map((row) => row.map((c) => q(plainValue(c))).join(sep))].join("\n");
}

function wireQuery() {
  const ed = $("#sparql");
  syncEditorHeight();
  $("#run").addEventListener("click", runVisible);
  $("#cancel").addEventListener("click", cancelRun);
  ed.addEventListener("keydown", (e) => { if ((e.ctrlKey || e.metaKey) && e.key === "Enter") { e.preventDefault(); runVisible(); } });
  ed.addEventListener("input", syncEditorHeight);

  $("#starter").addEventListener("change", (e) => {
    const s = S.starters[+e.target.value];
    $("#starterQ").textContent = s && s.question ? s.question : "";
    if (!s) return;
    ed.value = s.sparql;
    syncEditorHeight();
    ed.focus({ preventScroll: true });
  });
  $("#shareQ").addEventListener("click", async (e) => {
    const q = ed.value.trim();
    const url = location.origin + location.pathname + location.search + "#q=" + enc(q);
    writeHash(q);
    flash(e.currentTarget, "Link copied", await copyText(url));
  });
  $("#tableWrap").addEventListener("click", (e) => { const b = e.target.closest("th button"); if (b) sortBy(+b.dataset.col); });
  $("#moreRows").addEventListener("click", () => { S.shown += 500; paintTable(); });
  $("#resTools").addEventListener("click", async (e) => {
    const b = e.target.closest("[data-export]");
    if (!b || !S.result) return;
    const kind = b.dataset.export;
    if (kind === "download") {
      const blob = new Blob([toDelimited(",")], { type: "text/csv;charset=utf-8" });
      const a = document.createElement("a");
      a.href = URL.createObjectURL(blob); a.download = `${S.key}-results.csv`;
      document.body.appendChild(a); a.click(); a.remove();
      setTimeout(() => URL.revokeObjectURL(a.href), 2000);
      return;
    }
    flash(b, "Copied", await copyText(toDelimited(kind === "csv" ? "," : "\t")));
  });

  // query from the URL: shown, not run (a link should not trigger a heavy read by itself)
  const fromUrl = queryFromUrl();
  if (fromUrl) {
    ed.value = fromUrl; syncEditorHeight();
    setStatus(`This query came from the link. Check it, then press <strong>Run query</strong>.`);
  }
  wireAutocomplete();
}

function describeQuery(iri) {
  const t = /^_:/.test(iri) ? iri : `<${iri}>`;
  return S.hasGraphs
    ? `SELECT ?g ?p ?o WHERE {\n  GRAPH ?g { ${t} ?p ?o }\n} LIMIT 200`
    : `SELECT ?p ?o WHERE {\n  ${t} ?p ?o\n} LIMIT 200`;
}

function insertAtCursor(ed, text) {
  const a = ed.selectionStart ?? ed.value.length, b = ed.selectionEnd ?? a;
  ed.value = ed.value.slice(0, a) + text + ed.value.slice(b);
  ed.selectionStart = ed.selectionEnd = a + text.length;
  syncEditorHeight();
}

function wireAutocomplete() {
  const input = $("#ac"), list = $("#aclist"), ed = $("#sparql");
  let items = [], active = -1, timer = 0, seq = 0;
  const hide = () => { list.hidden = true; input.setAttribute("aria-expanded", "false"); input.removeAttribute("aria-activedescendant"); };
  const showMsg = (msg) => { items = []; active = -1; list.innerHTML = `<div class="dz-ac-msg">${msg}</div>`; list.hidden = false; input.setAttribute("aria-expanded", "true"); };
  const paint = () => {
    list.innerHTML = items.map((it, i) => `<div class="dz-ac-item${i === active ? " on" : ""}" role="option" id="acopt${i}" aria-selected="${i === active}" data-i="${i}">
        <span class="lab">${esc(it.label || shorten(it.iri))}</span><span class="iri">${esc(shorten(it.iri))}</span>
        <button type="button" class="dz-ac-ins" data-i="${i}" tabindex="-1" title="Insert the IRI at the cursor in the editor">Insert IRI</button></div>`).join("") +
      `<div class="dz-ac-foot">Enter shows its statements. Shift+Enter inserts the IRI.</div>`;
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    if (active >= 0) input.setAttribute("aria-activedescendant", "acopt" + active); else input.removeAttribute("aria-activedescendant");
  };
  const pick = (it, insert) => {
    if (!it) return;
    hide();
    input.value = it.label || shorten(it.iri);
    if (insert) {
      insertAtCursor(ed, /^_:/.test(it.iri) ? it.iri : `<${it.iri}>`);
      ed.focus();
    } else {
      window.plazaDataset.showQuery(describeQuery(it.iri), { run: true });
    }
  };
  input.addEventListener("input", () => {
    clearTimeout(timer);
    const text = input.value.trim();
    if (text.length < 2) return hide();
    const mine = ++seq;
    timer = setTimeout(async () => {
      if (S.engine.state !== "ready") showMsg(S.engine.state === "error" ? "The engine could not open this file." : "Waiting for the file to open…");
      try {
        const res = await S.engine.prefix(text, 25);
        if (mine !== seq) return;
        items = res; active = res.length ? 0 : -1;
        if (!res.length) showMsg("No matching labels. The file may have no label index; a SPARQL query can still find it.");
        else paint();
      } catch (e) {
        if (mine !== seq) return;
        const m = String((e && e.message) || e);
        showMsg(/prefix|label|text index|not available|no index/i.test(m) ? "This file has no label index, so names cannot be searched. Use a SPARQL query instead." : esc(m));
      }
    }, 200);
  });
  input.addEventListener("keydown", (e) => {
    if (list.hidden || !items.length) { if (e.key === "Escape") hide(); return; }
    if (e.key === "ArrowDown") { active = Math.min(active + 1, items.length - 1); paint(); e.preventDefault(); }
    else if (e.key === "ArrowUp") { active = Math.max(active - 1, 0); paint(); e.preventDefault(); }
    else if (e.key === "Enter") { pick(items[active], e.shiftKey); e.preventDefault(); }
    else if (e.key === "Escape") hide();
  });
  list.addEventListener("mousedown", (e) => e.preventDefault());
  list.addEventListener("click", (e) => {
    const ins = e.target.closest(".dz-ac-ins");
    const row = e.target.closest(".dz-ac-item");
    if (!row) return;
    pick(items[+row.dataset.i], !!ins);
  });
  document.addEventListener("click", (e) => { if (!e.target.closest(".dz-ac")) hide(); });
  input.addEventListener("blur", () => setTimeout(hide, 120));
}
