// rete landing page: live demo, motion and small interactions.
//
// Nothing here is specific to one dataset except the curated list of example
// questions (copied from the playground catalog). Cards, bars, tables and the
// graph are all chosen from the shape of the result itself.

const $ = (s, r = document) => r.querySelector(s);
const $$ = (s, r = document) => Array.from(r.querySelectorAll(s));
const REDUCED = matchMedia("(prefers-reduced-motion: reduce)").matches;
const R2 = "https://data.graphplaza.com";

/* ───────────────────────── Curated graphs ───────────────────────── */

const GRAPHS = [
  {
    key: "linked-jazz", name: "Linked Jazz", ico: "🎷",
    url: `${R2}/linked-jazz/linked-jazz.rete`,
    blurb: "Who knew whom in jazz, rebuilt from oral-history interviews: 54 interviewed musicians and about 2,000 people they mention.",
    facts: { triples: "9,466", license: "CC BY-SA 3.0" },
    questions: [
      { label: "Musicians, with photos", q: `PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX dbo: <http://dbpedia.org/ontology/>
SELECT ?person ?name ?photo WHERE {
  ?person foaf:name ?name ; dbo:thumbnail ?photo .
} LIMIT 60` },
      { label: "Most talked-about musicians", q: `PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT ?name (COUNT(?s) AS ?mentions) WHERE {
  ?s ?rel ?person .
  FILTER(?rel != foaf:name && ?rel != <http://dbpedia.org/ontology/thumbnail>)
  ?person foaf:name ?name .
} GROUP BY ?name ORDER BY DESC(?mentions) LIMIT 15` },
      { label: "Mary Lou Williams' collaborators", view: "graph", q: `PREFIX foaf: <http://xmlns.com/foaf/0.1/>
PREFIX dbo: <http://dbpedia.org/ontology/>
PREFIX mo: <http://purl.org/ontology/mo/>
PREFIX lj: <http://linkedjazz.org/ontology/>
CONSTRUCT {
  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .
  ?other foaf:name ?name ; dbo:thumbnail ?photo .
} WHERE {
  <http://dbpedia.org/resource/Mary_Lou_Williams> ?p ?other .
  FILTER(?p = mo:collaborated_with || ?p = lj:playedTogether)
  ?other foaf:name ?name .
  OPTIONAL { ?other dbo:thumbnail ?photo }
}` },
      { label: "Who Count Basie reaches by word of mouth", q: `PREFIX rel: <http://purl.org/vocab/relationship/>
PREFIX foaf: <http://xmlns.com/foaf/0.1/>
SELECT DISTINCT ?reached ?name WHERE {
  <http://dbpedia.org/resource/Count_Basie> rel:knowsOf+ ?reached .
  ?reached foaf:name ?name .
} ORDER BY ?name LIMIT 100` },
    ],
  },
  {
    key: "bph", name: "Hermetic books", ico: "📜",
    url: `${R2}/bph/bph.rete`,
    blurb: "The Embassy of the Free Mind in Amsterdam: 2,307 books on alchemy, mysticism and esotericism, with their covers.",
    facts: { triples: "5.74 M" },
    questions: [
      { label: "Alchemy books, with covers", q: `PREFIX schema: <http://schema.org/>
PREFIX dct: <http://purl.org/dc/terms/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?book ?cover ?title ?author ?year WHERE {
  ?book dct:subject <https://data.embassyofthefreemind.com/category/alchemy> ;
        schema:image ?cover ; rdfs:label ?title ;
        dct:creator ?author ; schema:datePublished ?year
}
ORDER BY ?year
LIMIT 40` },
      { label: "Most-represented authors", q: `PREFIX schema: <http://schema.org/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?author (COUNT(?b) AS ?books) WHERE {
  ?b schema:author ?a . ?a rdfs:label ?author
}
GROUP BY ?author
ORDER BY DESC(?books)
LIMIT 20` },
      { label: "How alchemy books relate", view: "graph", q: `PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX dct: <http://purl.org/dc/terms/>
PREFIX schema: <http://schema.org/>
PREFIX efmo: <https://data.embassyofthefreemind.com/ontology#>
CONSTRUCT {
  ?b rdfs:label ?bl ; efmo:relatedBook ?r ; schema:image ?cover .
  ?r rdfs:label ?rl .
} WHERE {
  ?b dct:subject <https://data.embassyofthefreemind.com/category/alchemy> ;
     efmo:relatedBook ?r ; rdfs:label ?bl .
  ?r rdfs:label ?rl .
  OPTIONAL { ?b schema:image ?cover }
}` },
    ],
  },
  {
    key: "davidrumsey", name: "Old maps", ico: "🗺️",
    url: `${R2}/davidrumsey/davidrumsey.rete`,
    blurb: "The David Rumsey Map Collection: 150,017 maps with images, dates and places.",
    facts: { triples: "5.15 M" },
    questions: [
      { label: "Star charts, oldest first", q: `PREFIX dct: <http://purl.org/dc/terms/>
PREFIX drm: <https://w3id.org/rete/davidrumsey#>
PREFIX edm: <http://www.europeana.eu/schemas/edm/>
SELECT ?thumb ?title ?year ?author WHERE {
  ?m a drm:CelestialMap ; dct:title ?title ; edm:preview ?thumb .
  OPTIONAL { ?m drm:year ?year }
  OPTIONAL { ?m dct:creator ?author }
}
ORDER BY ?year LIMIT 40` },
      { label: "Wall maps over two meters wide", q: `PREFIX dct: <http://purl.org/dc/terms/>
PREFIX drm: <https://w3id.org/rete/davidrumsey#>
PREFIX edm: <http://www.europeana.eu/schemas/edm/>
SELECT ?thumb ?title ?year ?wcm WHERE {
  ?m a drm:WallMap ; dct:title ?title ; drm:objWidthCm ?wcm ; edm:preview ?thumb .
  OPTIONAL { ?m drm:year ?year }
  FILTER(?wcm > 200)
}
ORDER BY DESC(?wcm) LIMIT 30` },
      { label: "Atlases with the most maps", q: `PREFIX dct: <http://purl.org/dc/terms/>
SELECT ?title (COUNT(?m) AS ?maps) WHERE {
  ?m dct:isPartOf ?pub .
  ?pub dct:title ?title .
} GROUP BY ?title ORDER BY DESC(?maps) LIMIT 15` },
    ],
  },
  {
    key: "hugging-face", name: "Hugging Face Hub", ico: "🤗",
    url: `${R2}/hugging-face/hugging-face.rete`,
    blurb: "2.9 million models, 1.4 million Spaces and the people behind them, with which model was fine-tuned from which.",
    facts: { triples: "85.1 M" },
    questions: [
      { label: "Llama-3.1-8B: parents, children, Spaces", view: "graph", q: `PREFIX hf: <https://w3id.org/rete/huggingface#>
CONSTRUCT {
  <https://huggingface.co/meta-llama/Llama-3.1-8B> hf:finetunedFrom ?parent .
  ?child hf:finetunedFrom <https://huggingface.co/meta-llama/Llama-3.1-8B> .
  ?space hf:usesModel <https://huggingface.co/meta-llama/Llama-3.1-8B> .
} WHERE {
  { <https://huggingface.co/meta-llama/Llama-3.1-8B> hf:finetunedFrom ?parent }
  UNION
  { SELECT ?child WHERE { ?child hf:finetunedFrom <https://huggingface.co/meta-llama/Llama-3.1-8B> } LIMIT 25 }
  UNION
  { SELECT ?space WHERE { ?space hf:usesModel <https://huggingface.co/meta-llama/Llama-3.1-8B> } LIMIT 25 }
}` },
      { label: "Mistral AI's people, by followers", q: `PREFIX hf: <https://w3id.org/rete/huggingface#>
PREFIX schema: <https://schema.org/>
SELECT ?person ?name ?followers WHERE {
  ?person hf:memberOf <https://huggingface.co/mistralai> ;
          hf:followerCount ?followers .
  OPTIONAL { ?person schema:name ?name }
}
ORDER BY DESC(?followers) LIMIT 25` },
      { label: "The most fine-tuned models", q: `PREFIX hf: <https://w3id.org/rete/huggingface#>
SELECT ?base (COUNT(*) AS ?finetunes) WHERE {
  ?model hf:finetunedFrom ?base .
}
GROUP BY ?base ORDER BY DESC(?finetunes) LIMIT 15` },
    ],
  },
];

const SAMPLE_TTL = `@prefix s:    <https://example.org/solar/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

s:Earth    rdfs:label "Earth"    ; s:orbits s:Sun     ; s:radiusKm 6371 .
s:Mars     rdfs:label "Mars"     ; s:orbits s:Sun     ; s:radiusKm 3390 .
s:Jupiter  rdfs:label "Jupiter"  ; s:orbits s:Sun     ; s:radiusKm 69911 .
s:Moon     rdfs:label "Moon"     ; s:orbits s:Earth   ; s:radiusKm 1737 .
s:Phobos   rdfs:label "Phobos"   ; s:orbits s:Mars    ; s:radiusKm 11 .
s:Deimos   rdfs:label "Deimos"   ; s:orbits s:Mars    ; s:radiusKm 6 .
s:Io       rdfs:label "Io"       ; s:orbits s:Jupiter ; s:radiusKm 1822 .
s:Europa   rdfs:label "Europa"   ; s:orbits s:Jupiter ; s:radiusKm 1561 .
s:Ganymede rdfs:label "Ganymede" ; s:orbits s:Jupiter ; s:radiusKm 2634 .
s:Sun      rdfs:label "Sun" .
`;

/* ───────────────────────── Small helpers ───────────────────────── */

function esc(s) {
  return String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
}
// Decimal units, as the catalog and file managers print them.
function fmtBytes(n) {
  if (!Number.isFinite(n)) return "?";
  if (n < 1000) return `${n} B`;
  const u = ["KB", "MB", "GB", "TB"];
  let v = n / 1000, i = 0;
  while (v >= 1000 && i < u.length - 1) { v /= 1000; i++; }
  return `${v >= 100 ? v.toFixed(0) : v >= 10 ? v.toFixed(1) : v.toFixed(2)} ${u[i]}`;
}
function fmtPct(p) {
  if (p >= 10) return `${p.toFixed(0)}%`;
  if (p >= 0.1) return `${p.toFixed(1)}%`;
  if (p >= 0.001) return `${p.toFixed(3)}%`;
  return `${p.toExponential(1)}%`;
}
function fmtMs(ms) { return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(ms < 10000 ? 1 : 0)} s`; }
function h(tag, attrs = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v == null || v === false) continue;
    if (k === "class") e.className = v;
    else if (k === "style" && typeof v === "object") for (const [p, x] of Object.entries(v)) e.style.setProperty(p, x);
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "html") e.innerHTML = v;
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k.nodeType ? k : document.createTextNode(String(k)));
  return e;
}

/* ───────────────────────── RDF terms ───────────────────────── */

const XSD = "http://www.w3.org/2001/XMLSchema#";
const NUMERIC = new Set(["integer", "decimal", "double", "float", "int", "long", "short", "byte",
  "nonNegativeInteger", "positiveInteger", "unsignedInt", "unsignedLong", "negativeInteger", "nonPositiveInteger"].map((t) => XSD + t));

function unescapeLit(s) {
  return s.replace(/\\(u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|.)/g, (_, c) => {
    if (c[0] === "u" || c[0] === "U") return String.fromCodePoint(parseInt(c.slice(1), 16));
    return { n: "\n", t: "\t", r: "\r", b: "\b", f: "\f", '"': '"', "'": "'", "\\": "\\" }[c] ?? c;
  });
}
function parseTerm(s) {
  if (s == null) return null;
  if (typeof s !== "string") return { t: "lit", v: String(s) };
  if (s.startsWith("<<")) return { t: "triple", v: s };
  if (s[0] === "<" && s.endsWith(">")) return { t: "iri", v: s.slice(1, -1) };
  if (s.startsWith("_:")) return { t: "bnode", v: s };
  if (s[0] === '"') {
    const m = s.match(/^"([\s\S]*)"(?:@([A-Za-z0-9-]+)|\^\^<([^>]*)>)?$/);
    if (m) {
      const t = { t: "lit", v: unescapeLit(m[1]), lang: m[2], dt: m[3] };
      if (t.dt && NUMERIC.has(t.dt)) t.num = Number(t.v);
      return t;
    }
  }
  return { t: "lit", v: s };
}
const isNum = (t) => t && t.t === "lit" && Number.isFinite(t.num);

const COMMON_PREFIXES = {
  rdf: "http://www.w3.org/1999/02/22-rdf-syntax-ns#", rdfs: "http://www.w3.org/2000/01/rdf-schema#",
  owl: "http://www.w3.org/2002/07/owl#", xsd: XSD, foaf: "http://xmlns.com/foaf/0.1/",
  dct: "http://purl.org/dc/terms/", dc: "http://purl.org/dc/elements/1.1/", schema: "https://schema.org/",
  skos: "http://www.w3.org/2004/02/skos/core#", prov: "http://www.w3.org/ns/prov#",
  dbo: "http://dbpedia.org/ontology/", dbr: "http://dbpedia.org/resource/",
  wd: "http://www.wikidata.org/entity/", wdt: "http://www.wikidata.org/prop/direct/",
  geo: "http://www.opengis.net/ont/geosparql#",
};
let prefixes = { ...COMMON_PREFIXES };
function readPrefixes(q) {
  const p = { ...COMMON_PREFIXES };
  for (const m of q.matchAll(/PREFIX\s+([\w-]*):\s*<([^>]*)>/gi)) p[m[1]] = m[2];
  prefixes = p;
}
function curie(iri) {
  let best = null;
  for (const [k, ns] of Object.entries(prefixes)) {
    if (ns && iri.startsWith(ns) && (!best || ns.length > best[1].length)) best = [k, ns];
  }
  if (!best) return null;
  const local = iri.slice(best[1].length);
  return /^[\w.-]*$/.test(local) ? `${best[0]}:${local}` : null;
}
function humanize(iri) {
  try {
    const u = iri.replace(/[/#]+$/, "");
    const cut = Math.max(u.lastIndexOf("/"), u.lastIndexOf("#"));
    let local = decodeURIComponent(u.slice(cut + 1));
    if (!local) return iri;
    if (/^[0-9a-f-]{6,}$/i.test(local) || /^\d+$/.test(local)) {
      const prev = u.slice(0, cut);
      const seg = prev.slice(Math.max(prev.lastIndexOf("/"), prev.lastIndexOf("#")) + 1);
      local = seg ? `${seg}/${local}` : local;
    }
    return local.replace(/_/g, " ");
  } catch { return iri; }
}
const LABEL_RE = /(^|[:#/_-])(name|label|title|preflabel|caption|headline)$|^(name|label|title)$/i;
const IMG_EXT = /\.(png|jpe?g|gif|webp|avif|svg)(\?|#|$)/i;
const IMG_VAR = /(img|image|photo|thumb|picture|depiction|logo|preview|painting|icon|poster|render)/i;
function imageOf(term, name = "") {
  if (!term) return null;
  const v = term.v;
  if (term.t === "lit" && /^data:image\//.test(v)) return v;
  if ((term.t === "iri" || term.t === "lit") && /^https?:\/\//.test(v)) {
    if (IMG_EXT.test(v) || IMG_VAR.test(name)) return v;
  }
  return null;
}

/* Render a term as inline DOM. IRIs explore on click; literals show as text. */
function termNode(t, { short = false } = {}) {
  if (!t) return h("span", { class: "hint" }, "—");
  if (t.t === "iri") {
    const label = short ? humanize(t.v) : (curie(t.v) || humanize(t.v));
    const frag = document.createDocumentFragment();
    frag.append(h("button", { class: "term-iri", type: "button", title: `Explore ${t.v}`, "data-iri": t.v }, label));
    if (/^https?:/.test(t.v)) frag.append(h("a", { class: "ext", href: t.v, target: "_blank", rel: "noopener", title: "Open the IRI in a new tab", "aria-label": "Open in a new tab" }, "↗"));
    return frag;
  }
  if (t.t === "bnode") return h("span", { class: "hint" }, t.v);
  if (isNum(t)) return h("span", { class: "num" }, t.num.toLocaleString());
  if (/^data:image\//.test(t.v)) return h("span", { class: "hint" }, `image (${fmtBytes(t.v.length * 0.75)})`);
  const s = h("span", { class: "lit" }, t.v.length > 600 ? t.v.slice(0, 600) + "…" : t.v);
  if (t.lang) s.append(h("span", { class: "lang" }, t.lang));
  return s;
}

/* ───────────────────────── Engine (worker) ───────────────────────── */

// Every new worker is a new "epoch": files opened, registered or built in an
// earlier worker have to be opened again (the playground does the same).
class Engine {
  constructor() { this.w = null; this.seq = 0; this.epoch = 0; this.pending = new Map(); this.onProgress = () => {}; this.ready = null; }
  start() {
    if (this.w) return this.ready;
    this.epoch++;
    this.w = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
    // The worker posts "boot" once its module has loaded. A worker that cannot
    // load (its engine import fails, or the browser has no module workers)
    // only fires `error`, so the wait has to end there too; otherwise the page
    // says "Loading the engine…" for ever.
    this.ready = new Promise((res, rej) => { this._boot = res; this._bootFail = rej; });
    const timer = setTimeout(() => this.fail(new Error("The engine did not start. Reload the page; if it keeps happening, this browser may not run module workers.")), 30000);
    this.ready.then(() => clearTimeout(timer), () => clearTimeout(timer));
    this.w.onmessage = (e) => {
      const m = e.data;
      if (m.type === "boot") return this._boot();
      if (m.type === "progress") return this.onProgress(m);
      const p = this.pending.get(m.id);
      if (!p) return;
      this.pending.delete(m.id);
      m.ok ? p.res(m.value) : p.rej(new Error(m.error));
    };
    this.w.onerror = (e) => { this.fail(new Error(e.message || "The engine could not start.")); };
    return this.ready;
  }
  fail(err) {
    if (this._bootFail) { this._bootFail(err); this._bootFail = null; }
    for (const p of this.pending.values()) p.rej(err);
    this.pending.clear();
    if (this.w) this.w.terminate();
    this.w = null;
  }
  stop() { this.fail(Object.assign(new Error("Stopped."), { stopped: true })); }
  async call(type, payload = {}, transfer = []) {
    this.start();
    const id = ++this.seq;
    return new Promise((res, rej) => {
      this.pending.set(id, { res, rej });
      this.w.postMessage({ ...payload, type, id }, transfer);
    });
  }
}
const engine = new Engine();

/* ───────────────────────── Demo state ───────────────────────── */

const state = {
  graph: null,          // current entry of GRAPHS or a custom graph
  question: null,       // current question object
  view: "cards",
  lastView: "cards",
  result: null,         // { kind, vars, rows | triples }
  trail: [],            // [{ label, query, result, iri }]
  running: false,
  token: 0,             // bumps when a run is superseded
  spans: new Map(),     // url -> [{a, z, gen}]
  spanUrl: null,        // the file the engine is reading right now
  gen: 0,
};
const custom = []; // graphs the visitor brought

const el = {
  graphs: $("#graphs"), about: $("#about"), byo: $("#byo"), questions: $("#questions"),
  strip: $("#strip"), stripEnd: $("#stripEnd"), meterText: $("#meterText"),
  status: $("#status"), run: $("#runBtn"), crumbs: $("#crumbs"),
  cards: $("#vCards"), table: $("#vTable"), graphView: $("#vGraph"), sparql: $("#vSparql"),
  q: $("#q"), hl: $("#hl"), pg: $("#pgLink"),
};

function allGraphs() { return [...GRAPHS, ...custom]; }

function renderGraphChips() {
  el.graphs.replaceChildren(
    ...allGraphs().map((g) => h("button", {
      class: "chip", type: "button", role: "radio", "aria-checked": String(state.graph === g),
      onclick: () => selectGraph(g),
    }, h("span", { class: "ico", "aria-hidden": "true" }, g.ico || "◆"), g.name,
      g.size ? h("small", {}, g.size) : null)),
    h("button", {
      class: "chip chip-add", type: "button", role: "radio", "aria-checked": String(!state.graph),
      onclick: () => selectGraph(null),
    }, h("span", { class: "ico", "aria-hidden": "true" }, "＋"), "Your graph"),
  );
}

function renderAbout() {
  const g = state.graph;
  if (!g) {
    el.about.replaceChildren(h("p", {}, "Open a ", h("code", {}, ".rete"), " file by link or from your device, or paste a few triples and build one right here."));
    return;
  }
  const facts = h("span", { class: "facts" });
  if (g.facts?.triples) facts.append(h("span", {}, h("b", {}, g.facts.triples), " triples"));
  if (g.size) facts.append(h("span", {}, h("b", {}, g.size), " file"));
  if (g.facts?.license) facts.append(h("span", {}, g.facts.license));
  if (!g.custom) facts.append(h("a", { href: `docs/d/${g.key}.html` }, "About this graph →"));
  if (g.inMemory) {
    facts.append(h("a", { href: "#byo", onclick: (e) => { e.preventDefault(); el.byo.hidden = !el.byo.hidden; } }, "Edit the RDF"));
    if (g.download) facts.append(h("a", { href: g.download, download: "my-graph.rete" }, "Download .rete"));
  }
  el.about.replaceChildren(h("p", {}, g.blurb || ""), facts);
}

function renderQuestions() {
  const qs = state.graph ? state.graph.questions : [];
  el.questions.closest(".console-row").hidden = !qs.length;
  el.questions.replaceChildren(...qs.map((q) => h("button", {
    class: "chip", type: "button", role: "radio", "aria-checked": String(state.question === q),
    onclick: () => { selectQuestion(q); restart(); },
  }, q.label)));
}

/* Drop whatever is running (its result no longer matters) and run anew. */
function cancel() {
  if (!state.running) return;
  state.token++;
  engine.stop();
  setBusy(false);
}
function restart(q, opts) { cancel(); return run(q, opts); }

async function selectGraph(g) {
  cancel();
  state.graph = g;
  state.question = null;
  state.trail = [];
  renderGraphChips();
  renderAbout();
  el.byo.hidden = !!g;
  renderQuestions();
  drawStrip();
  if (!g) { clearResults("Bring a graph to start."); setMeter(""); return; }
  const first = g.questions[0];
  if (first) { selectQuestion(first); run(); }
}

/* Open (or re-open, after a worker restart) the selected graph. */
async function ensureOpen(g) {
  if (g.info && g.epoch === engine.epoch) return { info: g.info, fresh: false };
  setStatus("Opening the file…");
  state.spanUrl = g.url;
  const info = await (g.opener ? g.opener() : engine.call("open", { url: g.url }));
  g.info = info;
  g.epoch = engine.epoch;
  if (!g.size) { g.size = fmtBytes(info.fileLength); renderGraphChips(); renderAbout(); }
  return { info, fresh: true };
}

function selectQuestion(q) {
  state.question = q;
  state.trail = [];
  setQuery(q.q);
  // A question can ask for its view (a CONSTRUCT wants the graph). Otherwise
  // leave the graph view for cards, but keep a table if that was the choice.
  if (state.view !== "sparql") setView(q.view || (state.view === "graph" ? "cards" : state.view));
  $$(".chip", el.questions).forEach((c, i) => c.setAttribute("aria-checked", String(state.graph.questions[i] === q)));
}

/* ───────────────────────── Running queries ───────────────────────── */

function setStatus(text, err = false) {
  el.status.textContent = text;
  el.status.classList.toggle("err", err);
}
function setBusy(b) {
  state.running = b;
  el.run.classList.toggle("busy", b);
  $(".run-txt", el.run).textContent = b ? "Stop" : "Run";
}

async function run(queryText, opts = {}) {
  if (state.running) { cancel(); setStatus("Stopped."); clearResults("Stopped. Press Run to try again."); return; }
  const g = state.graph;
  if (!g) return;
  const token = ++state.token;
  const query = queryText ?? el.q.value;
  readPrefixes(query);
  setBusy(true);
  showSkeleton();
  try {
    if (!engine.w) setStatus("Loading the engine…");
    await engine.start();
    state.gen++;
    const { info, fresh } = await ensureOpen(g);
    if (token !== state.token) return;
    setStatus("Running…");
    state.spanUrl = g.url;
    const r = await engine.call("query", g.inMemory ? { key: g.key, query } : { url: g.url, query });
    if (token !== state.token) return;
    r.fileLength = r.fileLength || info.fileLength;
    if (fresh && !g.inMemory) { r.openBytes = info.openBytes; r.openRequests = info.openRequests; }
    const result = r.result;
    if (opts.explore) {
      state.trail.push({ label: opts.label, iri: opts.iri, query, result, r });
    } else {
      state.trail = [{ label: state.question?.label || "Your query", query, result, r }];
    }
    showResult(result, r);
  } catch (err) {
    if (token === state.token && !err.stopped) showError(err);
  } finally {
    if (token === state.token) setBusy(false);
  }
}

function showResult(result, r) {
  state.result = result;
  const n = result.kind === "select" ? result.rows.length : result.kind === "construct" ? result.triples.length : 1;
  setStatus(result.kind === "ask" ? fmtMs(r.ms) : `${n.toLocaleString()} ${result.kind === "construct" ? "triples" : n === 1 ? "row" : "rows"} · ${fmtMs(r.ms)}`);
  setMeter(r);
  drawStrip();
  renderCrumbs();
  renderAll();
  updatePlaygroundLink();
}

function showError(err) {
  const msg = String(err.message || err);
  let hint = "";
  if (/xhr|network|fetch|cors|range|status/i.test(msg)) hint = "\n\nIf this is your own file, check that the host allows range requests and CORS.";
  setStatus("Error", true);
  const box = h("div", { class: "err-box" }, h("b", {}, "The query did not run. "), msg + hint);
  for (const v of [el.cards, el.table]) v.replaceChildren(box.cloneNode(true));
  graphView.set([], []);
  setMeter(null);
}

function clearResults(msg) {
  state.result = null;
  for (const v of [el.cards, el.table]) v.replaceChildren(h("div", { class: "empty" }, msg));
  graphView.set([], []);
  el.crumbs.hidden = true;
}
function showSkeleton() {
  const sk = h("div", { class: "skeleton", "aria-hidden": "true" }, ...Array.from({ length: 6 }, () => h("div")));
  el.cards.replaceChildren(sk);
  el.table.replaceChildren(sk.cloneNode(true));
}

/* Explore one IRI: what does the graph say about it? */
function explore(iri) {
  if (!state.graph) return;
  const q = `SELECT ?p ?o WHERE {\n  <${iri}> ?p ?o .\n} LIMIT 300`;
  setQuery(q);
  if (state.view === "sparql") setView(state.lastView);
  restart(q, { explore: true, iri, label: humanize(iri) });
}
function exploreIncoming(iri) {
  const q = `SELECT ?s ?p WHERE {\n  ?s ?p <${iri}> .\n} LIMIT 100`;
  setQuery(q);
  restart(q, { explore: true, iri: null, label: `links to ${humanize(iri)}` });
}

function renderCrumbs() {
  if (state.trail.length < 2) { el.crumbs.hidden = true; return; }
  el.crumbs.hidden = false;
  const items = [];
  state.trail.forEach((c, i) => {
    if (i) items.push(h("span", { class: "sep", "aria-hidden": "true" }, "›"));
    const last = i === state.trail.length - 1;
    items.push(h("button", {
      type: "button", title: c.iri || c.label, "aria-current": last ? "step" : null,
      onclick: last ? null : () => {
        state.trail = state.trail.slice(0, i + 1);
        setQuery(c.query);
        showResult(c.result, c.r);
      },
    }, c.label));
  });
  el.crumbs.replaceChildren(...items);
}

document.addEventListener("click", (e) => {
  const b = e.target.closest("[data-iri]");
  if (b && b.closest("#console")) { e.preventDefault(); explore(b.dataset.iri); }
});

/* ───────────────────────── Result views ───────────────────────── */

function renderAll() {
  const r = state.result;
  if (!r) return;
  const t = [performance.now()];
  renderCards(r); t.push(performance.now());
  renderTable(r); t.push(performance.now());
  renderGraph(r); t.push(performance.now());
  if (/debug/.test(location.hash)) console.log(`[landing] render cards ${(t[1] - t[0]) | 0} ms, table ${(t[2] - t[1]) | 0} ms, graph ${(t[3] - t[2]) | 0} ms`);
}

function columnInfo(vars, rows) {
  const parsed = rows.map((row) => Object.fromEntries(vars.map((v) => [v, parseTerm(row[v])])));
  const info = {};
  for (const v of vars) {
    const vals = parsed.map((p) => p[v]).filter(Boolean);
    info[v] = {
      numeric: vals.length > 0 && vals.every(isNum),
      iri: vals.length > 0 && vals.every((t) => t.t === "iri"),
      image: vals.length > 0 && vals.some((t) => imageOf(t, v)),
      literal: vals.length > 0 && vals.every((t) => t.t === "lit" && !isNum(t)),
    };
  }
  return { parsed, info };
}

function renderCards(r) {
  const box = el.cards;
  if (r.kind === "ask") {
    box.replaceChildren(h("div", { class: "stats" }, h("div", { class: "stat" }, h("b", {}, r.boolean ? "Yes" : "No"), h("span", {}, "ASK"))));
    return;
  }
  if (r.kind === "construct") { box.replaceChildren(subjectCards(r.triples)); return; }
  const { vars, rows } = r;
  if (!rows.length) { box.replaceChildren(h("div", { class: "empty" }, h("b", {}, "No results"), "The graph has nothing that matches this pattern.")); return; }
  const last = state.trail[state.trail.length - 1];
  if (last?.iri && vars.length === 2 && vars[0] === "p" && vars[1] === "o") { box.replaceChildren(entitySheet(last.iri, rows)); return; }

  const { parsed, info } = columnInfo(vars, rows);
  const nums = vars.filter((v) => info[v].numeric);
  const anyImage = vars.some((v) => info[v].image);

  // Triple-shaped rows (subject, predicate, object): one card per subject.
  if (vars.length === 3 && info[vars[1]].iri && parsed.every((p) => p[vars[0]] && p[vars[0]].t !== "lit")) {
    box.replaceChildren(subjectCards(rows.map((r) => [r[vars[0]], r[vars[1]], r[vars[2]]])));
    return;
  }

  // One row of numbers: big figures.
  if (rows.length === 1 && nums.length === vars.length) {
    box.replaceChildren(h("div", { class: "stats" }, ...vars.map((v) => h("div", { class: "stat" }, h("b", {}, parsed[0][v].num.toLocaleString()), h("span", {}, `?${v}`)))));
    return;
  }
  // A ranking: labels and one count.
  if (rows.length >= 2 && nums.length === 1 && vars.length <= 3 && !anyImage) {
    const nv = nums[0];
    const others = vars.filter((v) => v !== nv);
    const max = Math.max(...parsed.map((p) => p[nv]?.num || 0)) || 1;
    box.replaceChildren(h("div", { class: "bars" }, ...parsed.slice(0, 200).map((p, i) => {
      const main = others[0] ? p[others[0]] : null;
      const sub = others[1] ? p[others[1]] : null;
      const lbl = h("span", { class: "lbl" });
      if (main) lbl.append(main.t === "iri" ? termNode(main, { short: true }) : (main.v ?? ""));
      if (sub) lbl.append(h("small", {}, sub.t === "iri" ? humanize(sub.v) : sub.v));
      return h("div", { class: "bar", style: { "--i": Math.min(i, 30), "--w": ((p[nv]?.num || 0) / max * 100).toFixed(1) } },
        h("span", { class: "rank" }, i + 1), lbl, h("span", { class: "val" }, (p[nv]?.num ?? 0).toLocaleString()));
    })));
    return;
  }

  // General case: one card per row. A column with the same value in every row
  // is context for the whole result, so it goes above the cards, not on each.
  const constant = rows.length > 1
    ? vars.filter((v) => rows.every((r) => r[v] != null && r[v] === rows[0][v]))
    : [];
  const live = vars.filter((v) => !constant.includes(v));
  const imgVar = live.find((v) => info[v].image);
  const titleVar = live.find((v) => v !== imgVar && info[v].literal && LABEL_RE.test(v))
    || live.find((v) => v !== imgVar && info[v].literal);
  const entityVar = live.find((v) => v !== imgVar && info[v].iri);
  const rest = live.filter((v) => v !== imgVar && v !== titleVar && v !== entityVar);
  const grid = h("div", { class: `cards${imgVar ? " has-img" : ""}` });
  parsed.slice(0, 150).forEach((p, i) => {
    const img = imgVar ? imageOf(p[imgVar], imgVar) : null;
    const title = titleVar && p[titleVar] ? p[titleVar].v : entityVar && p[entityVar] ? humanize(p[entityVar].v) : `Row ${i + 1}`;
    const card = h("article", { class: "card", style: { "--i": Math.min(i, 24) } });
    if (img) card.append(h("div", { class: "card-img" }, h("img", { src: img, alt: title, loading: "lazy", decoding: "async", onerror: (e) => e.target.closest(".card-img").remove() })));
    const body = h("div", { class: "card-body" }, h("h4", { class: "card-title" }, title));
    const dl = h("dl");
    for (const v of rest.slice(0, 6)) {
      if (!p[v] || imageOf(p[v], v)) continue;
      dl.append(h("dt", {}, v), h("dd", {}, termNode(p[v], { short: true })));
    }
    if (dl.childNodes.length) body.append(dl);
    card.append(body);
    if (entityVar && p[entityVar]) {
      const ent = p[entityVar].v;
      card.append(h("div", { class: "card-foot" },
        h("button", { class: "btn btn-ghost btn-sm", type: "button", "data-iri": ent }, "Explore →"),
        /^https?:/.test(ent) ? h("a", { class: "btn btn-ghost btn-sm", href: ent, target: "_blank", rel: "noopener", "aria-label": "Open the IRI in a new tab" }, "↗") : null));
    }
    grid.append(card);
  });
  const ctxLine = constant.length
    ? h("div", { class: "context" }, ...constant.map((v) => h("span", {}, h("span", { class: "hint" }, `?${v} `), termNode(parsed[0][v]))))
    : null;
  box.replaceChildren(...[ctxLine, grid].filter(Boolean));
  if (rows.length > 150) box.append(h("p", { class: "table-foot" }, `Showing 150 of ${rows.length} rows. The table view has more.`));
}

/* Cards for CONSTRUCT output: one card per subject. */
function subjectCards(triples) {
  const by = new Map();
  for (const [s, p, o] of triples) {
    if (!by.has(s)) by.set(s, []);
    by.get(s).push([parseTerm(p), parseTerm(o)]);
  }
  const grid = h("div", { class: "cards" });
  let i = 0, anyImg = false;
  for (const [s, po] of by) {
    const st = parseTerm(s);
    const label = po.find(([p, o]) => o.t === "lit" && LABEL_RE.test(p.v));
    const img = po.map(([p, o]) => imageOf(o, p.v)).find(Boolean);
    anyImg ||= !!img;
    const card = h("article", { class: "card", style: { "--i": Math.min(i++, 24) } });
    if (img) card.append(h("div", { class: "card-img" }, h("img", { src: img, alt: "", loading: "lazy", decoding: "async", onerror: (e) => e.target.closest(".card-img").remove() })));
    const dl = h("dl");
    for (const [p, o] of po.slice(0, 8)) {
      if (o === label?.[1] || imageOf(o, p.v)) continue;
      dl.append(h("dt", {}, curie(p.v) || humanize(p.v)), h("dd", {}, termNode(o, { short: true })));
    }
    card.append(h("div", { class: "card-body" }, h("h4", { class: "card-title" }, label ? label[1].v : humanize(st.v)), dl));
    if (st.t === "iri") card.append(h("div", { class: "card-foot" }, h("button", { class: "btn btn-ghost btn-sm", type: "button", "data-iri": st.v }, "Explore →")));
    grid.append(card);
  }
  if (anyImg) grid.classList.add("has-img");
  return grid;
}

/* One resource, all its properties. */
function entitySheet(iri, rows) {
  const po = rows.map((r) => [parseTerm(r.p), parseTerm(r.o)]);
  const label = po.find(([p, o]) => o.t === "lit" && LABEL_RE.test(p.v) && (!o.lang || /^en/.test(o.lang)))
    || po.find(([p, o]) => o.t === "lit" && LABEL_RE.test(p.v));
  const img = po.map(([p, o]) => imageOf(o, p.v)).find(Boolean);
  const groups = new Map();
  for (const [p, o] of po) {
    if (!groups.has(p.v)) groups.set(p.v, []);
    groups.get(p.v).push(o);
  }
  const props = h("dl", { class: "props" });
  let i = 0;
  for (const [p, os] of groups) {
    const dd = h("dd");
    for (const o of os.slice(0, 40)) dd.append(h("span", {}, imageOf(o, p) && o.t === "lit" ? "image" : termNode(o, { short: false })));
    if (os.length > 40) dd.append(h("span", { class: "hint" }, `+${os.length - 40} more`));
    props.append(h("div", { class: "prop", style: { "--i": Math.min(i++, 30) } }, h("dt", { title: p }, curie(p) || humanize(p)), dd));
  }
  return h("div", { class: `entity${img ? " with-img" : ""}` },
    img ? h("div", { class: "entity-img" }, h("img", { src: img, alt: "", decoding: "async" })) : null,
    h("div", {},
      h("div", { class: "entity-head" }, h("h3", {}, label ? label[1].v : humanize(iri)), h("p", { class: "iri" }, iri)),
      props,
      h("div", { class: "entity-actions" },
        h("button", { class: "btn btn-ghost btn-sm", type: "button", onclick: () => exploreIncoming(iri) }, "What links here"),
        /^https?:/.test(iri) ? h("a", { class: "btn btn-ghost btn-sm", href: iri, target: "_blank", rel: "noopener" }, "Open IRI ↗") : null)));
}

/* Table */
let sortState = { v: null, dir: 1 };
function renderTable(r) {
  const box = el.table;
  let vars, rows;
  if (r.kind === "ask") { box.replaceChildren(h("div", { class: "empty" }, h("b", {}, String(r.boolean)))); return; }
  if (r.kind === "construct") { vars = ["subject", "predicate", "object"]; rows = r.triples.map(([s, p, o]) => ({ subject: s, predicate: p, object: o })); }
  else ({ vars, rows } = r);
  if (!rows.length) { box.replaceChildren(h("div", { class: "empty" }, "No rows.")); return; }
  const { parsed, info } = columnInfo(vars, rows);
  let order = parsed.map((_, i) => i);
  if (sortState.v && vars.includes(sortState.v)) {
    const v = sortState.v;
    order.sort((a, b) => {
      const x = parsed[a][v], y = parsed[b][v];
      if (!x) return 1; if (!y) return -1;
      if (isNum(x) && isNum(y)) return (x.num - y.num) * sortState.dir;
      return String(x.v).localeCompare(String(y.v)) * sortState.dir;
    });
  }
  const CAP = 500;
  const thead = h("thead", {}, h("tr", {}, ...vars.map((v) => h("th", {
    scope: "col", onclick: () => { sortState = { v, dir: sortState.v === v ? -sortState.dir : 1 }; renderTable(r); },
    "aria-sort": sortState.v === v ? (sortState.dir > 0 ? "ascending" : "descending") : null,
  }, `?${v}`, sortState.v === v ? h("span", { class: "sort" }, sortState.dir > 0 ? "▲" : "▼") : null))));
  const tbody = h("tbody");
  for (const i of order.slice(0, CAP)) {
    const p = parsed[i];
    tbody.append(h("tr", {}, ...vars.map((v) => {
      const t = p[v];
      const img = imageOf(t, v);
      const td = h("td", { class: info[v].numeric ? "n" : null });
      if (img) td.append(h("img", { src: img, alt: "", loading: "lazy", decoding: "async" }));
      else td.append(termNode(t));
      return td;
    })));
  }
  box.replaceChildren(h("div", { class: "table-wrap" }, h("table", {}, thead, tbody)));
  if (rows.length > CAP) box.append(h("p", { class: "table-foot" }, `Showing ${CAP} of ${rows.length.toLocaleString()} rows.`));
}

/* Graph: CONSTRUCT triples, an explored resource, or links between IRI columns. */
function renderGraph(r) {
  const nodes = new Map(), links = [];
  const node = (t, extra = {}) => {
    const id = t.t === "lit" ? `lit:${t.v}:${links.length}` : t.v;
    if (!nodes.has(id)) nodes.set(id, { id, kind: t.t, label: t.t === "lit" ? t.v : humanize(t.v), iri: t.t === "iri" ? t.v : null, ...extra });
    return nodes.get(id);
  };
  const addTriple = (s, p, o) => {
    const S = node(s);
    if (o.t === "lit" && LABEL_RE.test(p.v)) { S.label = o.v; S.named = true; return; }
    const img = imageOf(o, p.v);
    if (img) { S.img = img; return; }
    if (o.t === "lit" && o.v.length > 80) return;
    const O = node(o);
    links.push({ s: S.id, t: O.id, label: curie(p.v) || humanize(p.v) });
  };
  if (r.kind === "construct") {
    for (const [s, p, o] of r.triples.slice(0, 600)) addTriple(parseTerm(s), parseTerm(p), parseTerm(o));
  } else if (r.kind === "select" && r.rows.length) {
    const last = state.trail[state.trail.length - 1];
    const [v0, v1, v2] = r.vars;
    const tripleShaped = r.vars.length === 3 && r.rows.every((row) => parseTerm(row[v1])?.t === "iri" && parseTerm(row[v0])?.t !== "lit");
    if (last?.iri && v0 === "p" && v1 === "o") {
      const c = node({ t: "iri", v: last.iri }, { center: true });
      for (const row of r.rows.slice(0, 200)) addTriple({ t: "iri", v: c.id }, parseTerm(row.p), parseTerm(row.o));
    } else if (tripleShaped) {
      for (const row of r.rows.slice(0, 600)) addTriple(parseTerm(row[v0]), parseTerm(row[v1]), parseTerm(row[v2]));
    } else {
      const { parsed, info } = columnInfo(r.vars, r.rows);
      const iriVars = r.vars.filter((v) => parsed.some((p) => p[v]?.t === "iri") && !info[v].image);
      const labelVar = r.vars.find((v) => info[v].literal && LABEL_RE.test(v)) || r.vars.find((v) => info[v].literal);
      const imgVar = r.vars.find((v) => info[v].image);
      for (const p of parsed.slice(0, 300)) {
        const ts = iriVars.map((v) => [v, p[v]]).filter(([, t]) => t && t.t === "iri");
        if (ts[0]) {
          const n0 = node(ts[0][1]);
          if (labelVar && p[labelVar]) n0.label = p[labelVar].v;
          if (imgVar && p[imgVar]) n0.img = imageOf(p[imgVar], imgVar);
        }
        for (let i = 1; i < ts.length; i++) {
          const a = node(ts[i - 1][1]), b = node(ts[i][1]);
          if (a.id !== b.id) links.push({ s: a.id, t: b.id, label: ts[i][0] });
        }
      }
      if (!links.length) { graphView.set([], [], "This result has no links between resources to draw. Try a CONSTRUCT question, or tap Explore on a card."); return; }
    }
  }
  graphView.set([...nodes.values()], links);
}

/* ───────────────────────── Force-directed graph ───────────────────────── */

const graphView = (() => {
  const canvas = $("#graphCanvas");
  const stage = canvas.parentElement;
  const ctx = canvas.getContext("2d");
  let nodes = [], links = [], byId = new Map(), msg = "";
  let w = 0, hgt = 0, dpr = 1, alpha = 0, raf = 0;
  let view = { x: 0, y: 0, k: 1 };
  const imgs = new Map();
  const pointers = new Map();
  let drag = null, pan = null, pinch = null, moved = 0, hover = null;

  function colors() {
    const cs = getComputedStyle(document.documentElement);
    return { accent: cs.getPropertyValue("--accent").trim(), accent2: cs.getPropertyValue("--accent-2").trim(),
      fg: cs.getPropertyValue("--fg").trim(), muted: cs.getPropertyValue("--muted").trim(),
      panel: cs.getPropertyValue("--panel").trim(), border: cs.getPropertyValue("--border").trim() };
  }
  function resize() {
    const r = stage.getBoundingClientRect();
    dpr = Math.min(window.devicePixelRatio || 1, 2);
    w = r.width; hgt = r.height;
    canvas.width = Math.max(1, w * dpr); canvas.height = Math.max(1, hgt * dpr);
    draw();
  }
  function set(ns, ls, message = "") {
    msg = message;
    const old = new Map(nodes.map((n) => [n.id, n]));
    nodes = ns.map((n, i) => {
      const o = old.get(n.id);
      const a = (i / Math.max(1, ns.length)) * Math.PI * 2;
      const rad = 40 + Math.sqrt(i) * 28;
      return { ...n, x: o?.x ?? Math.cos(a) * rad, y: o?.y ?? Math.sin(a) * rad, vx: 0, vy: 0,
        r: n.center ? 16 : n.kind === "lit" ? 5 : n.img ? 14 : 9 };
    });
    byId = new Map(nodes.map((n) => [n.id, n]));
    links = ls.filter((l) => byId.has(l.s) && byId.has(l.t)).map((l) => ({ ...l, a: byId.get(l.s), b: byId.get(l.t) }));
    const deg = new Map();
    for (const l of links) { deg.set(l.a, (deg.get(l.a) || 0) + 1); deg.set(l.b, (deg.get(l.b) || 0) + 1); }
    for (const n of nodes) if (!n.center && n.kind === "iri" && !n.img) n.r = Math.min(18, 7 + Math.sqrt(deg.get(n) || 1) * 2);
    for (const n of nodes) if (n.img && !imgs.has(n.img)) {
      const im = new Image(); im.decoding = "async"; im.src = n.img; im.onload = () => draw(); imgs.set(n.img, im);
    }
    // Settle the layout before the first frame, then fit it to the stage and
    // let it breathe a little.
    alpha = 1;
    const warm = nodes.length > 150 ? 160 : 260;
    for (let i = 0; i < warm && alpha > 0.02; i++) step();
    fit();
    alpha = 0;
    kick(0.12);
  }
  function fit() {
    if (!nodes.length || !w) { view = { x: 0, y: 0, k: 1 }; return; }
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
    for (const n of nodes) { x0 = Math.min(x0, n.x - n.r); y0 = Math.min(y0, n.y - n.r); x1 = Math.max(x1, n.x + n.r); y1 = Math.max(y1, n.y + n.r + 16); }
    const pad = 36;
    const k = Math.min((w - pad * 2) / Math.max(1, x1 - x0), (hgt - pad * 2) / Math.max(1, y1 - y0), 1.6);
    view = { k: Math.max(0.25, k), x: -(x0 + x1) / 2, y: -(y0 + y1) / 2 };
  }
  function kick(a = 0.6) { alpha = Math.max(alpha, a); if (!raf) raf = requestAnimationFrame(tick); }
  function tick() {
    raf = 0;
    if (stage.offsetParent === null) return; // hidden: resume when shown
    step();
    draw();
    if (alpha > 0.005 || drag) raf = requestAnimationFrame(tick);
  }
  function step() {
    const n = nodes.length;
    if (!n) return;
    const rep = n > 200 ? 900 : 2200;
    for (let i = 0; i < n; i++) {
      const a = nodes[i];
      for (let j = i + 1; j < n; j++) {
        const b = nodes[j];
        let dx = b.x - a.x, dy = b.y - a.y, d2 = dx * dx + dy * dy || 0.01;
        if (d2 > 160000) continue;
        const f = (rep / d2) * alpha;
        const d = Math.sqrt(d2);
        dx /= d; dy /= d;
        a.vx -= dx * f; a.vy -= dy * f; b.vx += dx * f; b.vy += dy * f;
      }
    }
    for (const l of links) {
      const dx = l.b.x - l.a.x, dy = l.b.y - l.a.y, d = Math.sqrt(dx * dx + dy * dy) || 1;
      const target = l.b.kind === "lit" ? 60 : 110;
      const f = ((d - target) / d) * 0.08 * alpha;
      l.a.vx += dx * f; l.a.vy += dy * f; l.b.vx -= dx * f; l.b.vy -= dy * f;
    }
    for (const nd of nodes) {
      nd.vx -= nd.x * 0.004 * alpha; nd.vy -= nd.y * 0.004 * alpha;
      if (nd === drag?.node) { nd.vx = nd.vy = 0; continue; }
      nd.vx *= 0.82; nd.vy *= 0.82;
      nd.x += nd.vx; nd.y += nd.vy;
    }
    alpha *= 0.985;
  }
  function toScreen(x, y) { return [w / 2 + (x + view.x) * view.k, hgt / 2 + (y + view.y) * view.k]; }
  function toWorld(sx, sy) { return [(sx - w / 2) / view.k - view.x, (sy - hgt / 2) / view.k - view.y]; }
  function draw() {
    const c = colors();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, hgt);
    if (!nodes.length) {
      ctx.fillStyle = c.muted; ctx.font = "14px system-ui, sans-serif"; ctx.textAlign = "center";
      wrapText(msg || "Run a query to draw its graph.", w / 2, hgt / 2 - 10, Math.min(w - 40, 420), 20);
      return;
    }
    const showEdgeLabels = view.k > 1.6 || links.length < 16;
    ctx.lineWidth = 1.2;
    ctx.font = "11px ui-monospace, Consolas, monospace";
    ctx.textAlign = "center";
    for (const l of links) {
      const [x1, y1] = toScreen(l.a.x, l.a.y), [x2, y2] = toScreen(l.b.x, l.b.y);
      const hot = hover && (l.a === hover || l.b === hover);
      ctx.strokeStyle = hot ? c.accent : c.border;
      ctx.globalAlpha = hot ? 1 : 0.9;
      ctx.beginPath(); ctx.moveTo(x1, y1); ctx.lineTo(x2, y2); ctx.stroke();
      // arrow head
      const ang = Math.atan2(y2 - y1, x2 - x1), rr = l.b.r * view.k + 3;
      const ax = x2 - Math.cos(ang) * rr, ay = y2 - Math.sin(ang) * rr;
      ctx.fillStyle = ctx.strokeStyle;
      ctx.beginPath(); ctx.moveTo(ax, ay);
      ctx.lineTo(ax - Math.cos(ang - 0.4) * 7, ay - Math.sin(ang - 0.4) * 7);
      ctx.lineTo(ax - Math.cos(ang + 0.4) * 7, ay - Math.sin(ang + 0.4) * 7);
      ctx.fill();
      if (showEdgeLabels || hot) {
        ctx.fillStyle = c.muted; ctx.globalAlpha = hot ? 1 : 0.75;
        ctx.fillText(l.label, (x1 + x2) / 2, (y1 + y2) / 2 - 4);
      }
    }
    ctx.globalAlpha = 1;
    const showLabels = nodes.length < 90 || view.k > 1.3;
    for (const n of nodes) {
      const [x, y] = toScreen(n.x, n.y);
      const r = n.r * Math.max(0.7, Math.min(view.k, 1.6));
      if (n.kind === "lit") {
        ctx.fillStyle = c.panel; ctx.strokeStyle = c.accent2; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill(); ctx.stroke();
      } else {
        const im = n.img && imgs.get(n.img);
        if (im && im.complete && im.naturalWidth) {
          ctx.save(); ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.clip();
          const s = Math.max(r * 2 / im.naturalWidth, r * 2 / im.naturalHeight);
          ctx.drawImage(im, x - im.naturalWidth * s / 2, y - im.naturalHeight * s / 2, im.naturalWidth * s, im.naturalHeight * s);
          ctx.restore();
          ctx.strokeStyle = n === hover ? c.accent2 : c.accent; ctx.lineWidth = 2.5;
          ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.stroke();
        } else {
          ctx.fillStyle = n.center ? c.accent2 : c.accent;
          ctx.globalAlpha = n.kind === "bnode" ? 0.5 : 1;
          ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fill();
          ctx.globalAlpha = 1;
          if (n === hover) { ctx.strokeStyle = c.fg; ctx.lineWidth = 2; ctx.stroke(); }
        }
      }
      if (showLabels || n === hover || n.center) {
        ctx.font = `${n.center ? "600 13px" : "12px"} system-ui, sans-serif`;
        ctx.fillStyle = c.fg;
        const t = n.label.length > 28 ? n.label.slice(0, 27) + "…" : n.label;
        ctx.lineWidth = 3; ctx.strokeStyle = c.panel; ctx.strokeText(t, x, y + r + 14);
        ctx.fillText(t, x, y + r + 14);
      }
    }
  }
  function wrapText(text, x, y, max, lh) {
    const words = text.split(" "); let line = "";
    for (const word of words) {
      const test = line ? line + " " + word : word;
      if (ctx.measureText(test).width > max && line) { ctx.fillText(line, x, y); y += lh; line = word; } else line = test;
    }
    ctx.fillText(line, x, y);
  }
  function pick(sx, sy) {
    const [x, y] = toWorld(sx, sy);
    let best = null, bd = Infinity;
    for (const n of nodes) {
      const d = Math.hypot(n.x - x, n.y - y);
      if (d < n.r + 10 / view.k && d < bd) { best = n; bd = d; }
    }
    return best;
  }
  const pos = (e) => { const r = canvas.getBoundingClientRect(); return [e.clientX - r.left, e.clientY - r.top]; };
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, pos(e));
    moved = 0;
    if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinch = { d: Math.hypot(a[0] - b[0], a[1] - b[1]), k: view.k };
      drag = null; pan = null; return;
    }
    const [sx, sy] = pos(e);
    const n = pick(sx, sy);
    if (n) { drag = { node: n }; canvas.classList.add("dragging"); kick(0.3); }
    else pan = { sx, sy, x: view.x, y: view.y };
  });
  canvas.addEventListener("pointermove", (e) => {
    const p = pos(e);
    if (pointers.has(e.pointerId)) {
      const prev = pointers.get(e.pointerId);
      moved += Math.abs(p[0] - prev[0]) + Math.abs(p[1] - prev[1]);
      pointers.set(e.pointerId, p);
    }
    if (pinch && pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      view.k = Math.max(0.25, Math.min(4, pinch.k * Math.hypot(a[0] - b[0], a[1] - b[1]) / pinch.d));
      draw(); return;
    }
    if (drag) { const [x, y] = toWorld(...p); drag.node.x = x; drag.node.y = y; kick(0.25); return; }
    if (pan) { view.x = pan.x + (p[0] - pan.sx) / view.k; view.y = pan.y + (p[1] - pan.sy) / view.k; draw(); return; }
    const n = pick(...p);
    if (n !== hover) { hover = n; canvas.style.cursor = n ? "pointer" : ""; draw(); }
  });
  const end = (e) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (drag && moved < 6 && drag.node.iri) explore(drag.node.iri);
    drag = null; pan = null; canvas.classList.remove("dragging");
  };
  canvas.addEventListener("pointerup", end);
  canvas.addEventListener("pointercancel", end);
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault();
    const [sx, sy] = pos(e);
    const [wx, wy] = toWorld(sx, sy);
    view.k = Math.max(0.25, Math.min(4, view.k * Math.exp(-e.deltaY * 0.0015)));
    const [nx, ny] = toWorld(sx, sy);
    view.x += nx - wx; view.y += ny - wy;
    draw();
  }, { passive: false });
  new ResizeObserver(resize).observe(stage);
  return { set, show: () => { resize(); fit(); draw(); kick(0.1); }, redraw: draw };
})();

/* ───────────────────────── Views & editor ───────────────────────── */

function setView(v) {
  if (v !== "sparql") state.lastView = v;
  state.view = v;
  $$(".seg-views button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.view === v)));
  $$(".view").forEach((p) => { p.hidden = p.dataset.view !== v; });
  if (v === "graph") graphView.show();
  if (v === "sparql") autosize();
}
$$(".seg-views button").forEach((b) => b.addEventListener("click", () => setView(b.dataset.view)));

const KW = "SELECT|CONSTRUCT|ASK|DESCRIBE|WHERE|FILTER|OPTIONAL|UNION|MINUS|GRAPH|SERVICE|BIND|VALUES|PREFIX|BASE|DISTINCT|REDUCED|ORDER|BY|GROUP|HAVING|LIMIT|OFFSET|ASC|DESC|AS|COUNT|SUM|MIN|MAX|AVG|SAMPLE|GROUP_CONCAT|SEPARATOR|STR|LANG|LANGMATCHES|DATATYPE|BOUND|IRI|URI|BNODE|CONCAT|STRLEN|UCASE|LCASE|CONTAINS|STRSTARTS|STRENDS|STRBEFORE|STRAFTER|YEAR|MONTH|DAY|NOW|REGEX|REPLACE|SUBSTR|IF|COALESCE|EXISTS|NOT|IN|ROUND|ABS|FROM|NAMED";
const TOKEN = new RegExp([
  "(?<com>#[^\\n]*)",
  "(?<iri><[^<>\"{}|^`\\\\\\s]*>)",
  "(?<str>\"(?:[^\"\\\\\\n]|\\\\.)*\"|'(?:[^'\\\\\\n]|\\\\.)*')",
  "(?<var>[?$][A-Za-z_][\\w]*)",
  `(?<kw>\\b(?:${KW})\\b|\\ba\\b)`,
  "(?<pn>\\b[A-Za-z][\\w-]*:[\\w.-]*|:[\\w-]+)",
  "(?<num>\\b\\d+(?:\\.\\d+)?\\b)",
].join("|"), "gi");
function highlight(src) {
  let out = "", last = 0;
  for (const m of src.matchAll(TOKEN)) {
    out += esc(src.slice(last, m.index));
    const g = Object.entries(m.groups).find(([, v]) => v != null)[0];
    out += `<span class="hl-${g}">${esc(m[0])}</span>`;
    last = m.index + m[0].length;
  }
  return out + esc(src.slice(last)) + (src.endsWith("\n") ? " " : "");
}
function autosize() {
  el.q.style.height = "auto";
  el.q.style.height = `${el.q.scrollHeight + 2}px`;
}
function setQuery(q) {
  el.q.value = q;
  el.hl.innerHTML = highlight(q);
  if (state.view === "sparql") autosize();
  updatePlaygroundLink();
}
el.q.addEventListener("input", () => { el.hl.innerHTML = highlight(el.q.value); autosize(); updatePlaygroundLink(); });
el.q.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); runFromEditor(); }
  if (e.key === "Tab" && !e.shiftKey) {
    e.preventDefault();
    const { selectionStart: s, selectionEnd: t } = el.q;
    el.q.setRangeText("  ", s, t, "end");
    el.q.dispatchEvent(new Event("input"));
  }
});
// After a run from the editor, show the result in the view that suits it.
function bestView(r) {
  if (r.kind === "construct") return "graph";
  return state.lastView === "graph" ? "cards" : state.lastView;
}
function runFromEditor() {
  state.trail = [];
  run(el.q.value).then(() => { if (state.result) setView(bestView(state.result)); });
}
el.run.addEventListener("click", () => (state.view === "sparql" ? runFromEditor() : run()));
$("#resetBtn").addEventListener("click", () => state.question && setQuery(state.question.q));
$("#copyBtn").addEventListener("click", (e) => copy(el.q.value, e.currentTarget));
function updatePlaygroundLink() {
  const g = state.graph;
  el.pg.href = g && !g.custom
    ? `docs/playground.html#dataset=${encodeURIComponent(g.key)}&mode=sparql&q=${encodeURIComponent(el.q.value)}`
    : "docs/playground.html";
}
async function copy(text, btn) {
  try { await navigator.clipboard.writeText(text); const t = btn.textContent; btn.textContent = "Copied"; setTimeout(() => (btn.textContent = t), 1200); }
  catch { /* clipboard blocked */ }
}

/* ───────────────────────── The byte strip ───────────────────────── */

function spansFor(url) {
  if (!url) return null;
  if (!state.spans.has(url)) state.spans.set(url, []);
  return state.spans.get(url);
}
engine.onProgress = (m) => {
  const list = spansFor(state.spanUrl);
  if (!list) return;
  for (const [a, z] of m.spans) list.push({ a, z, gen: state.gen });
  setStatus(`Reading… ${fmtBytes(m.bytes)} in ${m.requests} request${m.requests === 1 ? "" : "s"}`);
  if (state.graph && state.graph.url === state.spanUrl) drawStrip();
};

function drawStrip() {
  const c = el.strip, ctx = c.getContext("2d");
  const r = c.getBoundingClientRect();
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  c.width = Math.max(1, r.width * dpr); c.height = Math.max(1, r.height * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  const W = r.width, H = r.height;
  const cs = getComputedStyle(document.documentElement);
  const accent = cs.getPropertyValue("--accent").trim(), old = cs.getPropertyValue("--accent-deep").trim();
  const g = state.graph;
  ctx.clearRect(0, 0, W, H);
  // faint tick marks every 10%
  ctx.fillStyle = cs.getPropertyValue("--border").trim();
  for (let i = 1; i < 10; i++) ctx.fillRect(Math.round(W * i / 10), H - 5, 1, 5);
  if (!g) { el.stripEnd.textContent = ""; return; }
  if (g.inMemory) {
    ctx.fillStyle = accent; ctx.globalAlpha = 0.6; ctx.fillRect(0, 4, W, H - 14); ctx.globalAlpha = 1;
    el.stripEnd.textContent = g.info ? fmtBytes(g.info.fileLength) : "";
    return;
  }
  const L = g.info?.fileLength || 0;
  el.stripEnd.textContent = L ? fmtBytes(L) : "";
  if (!L) return;
  for (const s of spansFor(g.url)) {
    const fresh = s.gen === state.gen;
    const x0 = (s.a / L) * W, x1 = (s.z / L) * W;
    const wd = Math.max(2, x1 - x0);
    ctx.fillStyle = fresh ? accent : old;
    ctx.globalAlpha = fresh ? 1 : 0.35;
    ctx.fillRect(x0, 4, wd, H - 14);
    if (fresh && !REDUCED) { ctx.globalAlpha = 0.18; ctx.fillRect(x0 - 3, 0, wd + 6, H - 6); }
  }
  ctx.globalAlpha = 1;
}
addEventListener("resize", () => drawStrip());

function setMeter(r) {
  const t = el.meterText;
  if (r === "") { t.textContent = "Pick a way to bring your graph."; return; }
  if (!r) { t.textContent = "Each mark on the bar is a part of the file this page fetched."; return; }
  if (r.inMemory) {
    t.innerHTML = `The whole graph is in this tab: <b>${fmtBytes(r.fileLength)}</b>, built a moment ago. Nothing goes over the network.`;
    return;
  }
  const L = r.fileLength || 0;
  // The first query on a file also pays for opening it; count that in.
  const bytes = r.bytes + (r.openBytes || 0), reqs = r.requests + (r.openRequests || 0);
  const pct = L ? (bytes / L) * 100 : 0;
  t.innerHTML = bytes === 0
    ? `Answered from parts of the file this page already had: <b>0 bytes</b> fetched, ${fmtMs(r.ms)}.`
    : `Read <b>${fmtBytes(bytes)}</b> of a <b>${fmtBytes(L)}</b> file <span class="pct">(${fmtPct(pct)})</span> in ${reqs} request${reqs === 1 ? "" : "s"}, ${fmtMs(r.ms)}${r.openBytes ? ", opening the file included" : ""}.`;
}

/* ───────────────────────── Bring your own graph ───────────────────────── */

const DEFAULT_QS = (big) => [
  { label: "A first look", q: "SELECT ?s ?p ?o WHERE {\n  ?s ?p ?o .\n} LIMIT 50" },
  { label: "As a graph", view: "graph", q: "SELECT ?s ?p ?o WHERE {\n  ?s ?p ?o .\n} LIMIT 150" },
  ...(big ? [] : [
    { label: "Predicates by use", q: "SELECT ?p (COUNT(*) AS ?n) WHERE {\n  ?s ?p ?o .\n} GROUP BY ?p ORDER BY DESC(?n) LIMIT 25" },
    { label: "Classes by size", q: "SELECT ?class (COUNT(?s) AS ?n) WHERE {\n  ?s a ?class .\n} GROUP BY ?class ORDER BY DESC(?n) LIMIT 25" },
  ]),
];
function questionsFromCard(card, big) {
  const qs = [];
  const list = card?.example_queries || card?.examples || [];
  list.slice(0, 6).forEach((ex, i) => {
    const q = typeof ex === "string" ? ex : ex.query || ex.q;
    if (!q) return;
    const label = (typeof ex === "object" && (ex.label || ex.title)) || (q.match(/^#\s*(.+)$/m)?.[1]) || `Starter query ${i + 1}`;
    qs.push({ label, q });
  });
  return qs.length ? qs : DEFAULT_QS(big);
}

async function addCustom(g) {
  cancel();
  setStatus("Opening the file…");
  try {
    await engine.start();
    state.gen++;
    state.spanUrl = g.url || null;
    const info = await g.opener();
    g.info = info;
    g.epoch = engine.epoch;
    g.size = fmtBytes(info.fileLength);
    const big = info.fileLength > 150 * 1024 * 1024;
    g.questions = questionsFromCard(info.card, big);
    if (info.card?.title) g.blurb = `${info.card.title}. ${info.card.description || ""}`.slice(0, 280);
    custom.push(g);
    await selectGraph(g);
    return info;
  } catch (err) {
    showError(err);
    throw err;
  }
}

$$(".byo .seg button").forEach((b) => b.addEventListener("click", () => {
  $$(".byo .seg button").forEach((x) => x.setAttribute("aria-selected", String(x === b)));
  $$(".byo-pane").forEach((p) => { p.hidden = p.dataset.pane !== b.dataset.byo; });
}));
$("#byoUrl").addEventListener("submit", (e) => {
  e.preventDefault();
  const url = $("#byoUrlInput").value.trim();
  if (!url) return;
  const name = decodeURIComponent(url.split("/").pop() || "graph").replace(/\.rete$/, "");
  addCustom({ key: `url:${url}`, custom: true, name, ico: "🔗", url, blurb: url,
    opener: () => engine.call("open", { url }) }).catch(() => {});
});
let localN = 0;
function openLocal(file) {
  if (!file) return;
  const url = `rete-local:${++localN}/${file.name}`;
  addCustom({ key: url, custom: true, name: file.name.replace(/\.rete$/, ""), ico: "💾", url,
    blurb: `${file.name}, read from your device.`,
    opener: () => engine.call("local", { url, file }) }).catch(() => {});
}
$("#byoFile").addEventListener("change", (e) => openLocal(e.target.files[0]));
const drop = $("#drop");
["dragenter", "dragover"].forEach((t) => drop.addEventListener(t, (e) => { e.preventDefault(); drop.classList.add("over"); }));
["dragleave", "drop"].forEach((t) => drop.addEventListener(t, () => drop.classList.remove("over")));
drop.addEventListener("drop", (e) => { e.preventDefault(); openLocal(e.dataTransfer.files[0]); });
$("#byoText").value = SAMPLE_TTL;
let builtN = 0;
$("#byoPaste").addEventListener("submit", async (e) => {
  e.preventDefault();
  const key = `mem:${++builtN}`;
  const text = $("#byoText").value, format = $("#byoFormat").value;
  const g = { key, custom: true, inMemory: true, name: builtN > 1 ? `My graph ${builtN}` : "My graph", ico: "✏️",
    blurb: "Built in this tab from the RDF you pasted.",
    opener: async () => {
      const info = await engine.call("build", { key, text, format });
      if (g.download) URL.revokeObjectURL(g.download);
      g.download = URL.createObjectURL(new Blob([info.bytes], { type: "application/octet-stream" }));
      const dl = $("#byoDownload");
      dl.href = g.download; dl.hidden = false;
      return info;
    } };
  try { await addCustom(g); } catch { /* shown */ }
});

/* ───────────────────────── Page motion ───────────────────────── */

// Header turns solid once the hero is behind it.
const topBar = $("#top");
const hero = $(".hero");
function onScrollHeader() { topBar.classList.toggle("solid", scrollY > hero.offsetHeight - 70); }
addEventListener("scroll", onScrollHeader, { passive: true });
onScrollHeader();

// Theme toggle (shares the docs' localStorage key).
$("#themeBtn").addEventListener("click", () => {
  const root = document.documentElement;
  const dark = root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches;
  root.dataset.theme = dark ? "light" : "dark";
  try { localStorage.setItem("theme", root.dataset.theme); } catch { /* private mode */ }
  drawStrip(); graphView.redraw();
});

// Reveal on scroll.
const revealer = new IntersectionObserver((entries) => {
  for (const e of entries) if (e.isIntersecting) { e.target.classList.add("in"); revealer.unobserve(e.target); }
}, { rootMargin: "0px 0px -8% 0px", threshold: 0.12 });
$$(".reveal").forEach((n) => revealer.observe(n));

// Active section in the nav.
const navLinks = $$(".nav a");
const sectionSpy = new IntersectionObserver((entries) => {
  for (const e of entries) if (e.isIntersecting) navLinks.forEach((a) => a.classList.toggle("on", a.getAttribute("href") === `#${e.target.id}`));
}, { rootMargin: "-45% 0px -50% 0px" });
["try", "how", "use", "apps", "support"].forEach((id) => sectionSpy.observe(document.getElementById(id)));

// Hero: "Query it from …" cycles through the places a .rete can be queried.
(function rotator() {
  const box = $("#rot");
  if (!box) return;
  const words = ["a browser", "Python", "JavaScript", "R", "Java", "the command line", "Claude", "a notebook"];
  let i = 0, word = $(".rot-word", box);
  const fit = (w) => { box.style.width = `${w.getBoundingClientRect().width + 6}px`; };
  fit(word);
  if (REDUCED) return;
  setInterval(() => {
    if (document.hidden) return;
    i = (i + 1) % words.length;
    const next = h("span", { class: "rot-word in" }, words[i]);
    next.style.position = "absolute"; next.style.left = "0"; next.style.top = "0";
    word.classList.add("out");
    box.append(next);
    fit(next);
    const old = word;
    setTimeout(() => { old.remove(); next.style.position = ""; next.classList.remove("in"); }, 460);
    word = next;
  }, 2000);
})();

// Parallax: elements drift at their own speed relative to the viewport.
const parallax = $$("[data-parallax]");
let pRaf = 0;
function doParallax() {
  pRaf = 0;
  const vh = innerHeight;
  for (const n of parallax) {
    const speed = parseFloat(n.dataset.parallax);
    const r = n.parentElement.getBoundingClientRect();
    if (r.bottom < -200 || r.top > vh + 200) continue;
    const mid = r.top + r.height / 2 - vh / 2;
    n.style.transform = `translate3d(0, ${(mid * speed).toFixed(1)}px, 0)`;
  }
}
if (!REDUCED) {
  addEventListener("scroll", () => { if (!pRaf) pRaf = requestAnimationFrame(doParallax); }, { passive: true });
  doParallax();
}

// How-it-works, step 3: the file lights up a few blocks at a time.
(function queryArt() {
  const g = $(".art-query .cells");
  const cols = 8, rows = 9, cw = 18, ch = 16, x0 = 24, y0 = 42;
  const cells = [];
  for (let r = 0; r < rows; r++) for (let c = 0; c < cols; c++) {
    const rect = document.createElementNS("http://www.w3.org/2000/svg", "rect");
    rect.setAttribute("x", x0 + c * (cw + 2)); rect.setAttribute("y", y0 + r * (ch + 2));
    rect.setAttribute("width", cw); rect.setAttribute("height", ch); rect.setAttribute("rx", 3);
    g.append(rect); cells.push(rect);
  }
  cells[0].classList.add("hit"); cells[1].classList.add("hit");
  if (REDUCED) { [13, 30, 51].forEach((i) => cells[i].classList.add("hit")); return; }
  let timer = 0;
  const tick = () => {
    cells.forEach((c, i) => { if (i > 1) c.classList.remove("hit"); });
    const n = 3 + Math.floor(Math.random() * 2);
    for (let k = 0; k < n; k++) cells[2 + Math.floor(Math.random() * (cells.length - 2))].classList.add("hit");
  };
  new IntersectionObserver(([e]) => {
    if (e.isIntersecting && !timer) { tick(); timer = setInterval(tick, 1600); }
    else if (!e.isIntersecting && timer) { clearInterval(timer); timer = 0; }
  }).observe(g.closest("svg"));
})();

// Code tabs.
$$(".seg-code button").forEach((b) => b.addEventListener("click", () => {
  $$(".seg-code button").forEach((x) => x.setAttribute("aria-selected", String(x === b)));
  $$(".code").forEach((p) => { p.hidden = p.dataset.code !== b.dataset.code; });
}));
$("#copyCode").addEventListener("click", (e) => {
  const pane = $$(".code").find((p) => !p.hidden);
  copy(pane.textContent.split("\n").filter((l) => !/^\s*(#|\/\/)/.test(l)).join("\n").trim(), e.currentTarget);
});

/* ───────────────────────── Hero network ───────────────────────── */

(function heroNet() {
  const canvas = $("#net");
  const ctx = canvas.getContext("2d");
  let W = 0, H = 0, dpr = 1, nodes = [], edges = [], adj = [], pulses = [], visible = true, raf = 0, t0 = performance.now();
  let mx = 0, my = 0, tmx = 0, tmy = 0;

  function build() {
    const r = canvas.getBoundingClientRect();
    W = r.width; H = r.height;
    dpr = Math.min(window.devicePixelRatio || 1, W < 700 ? 1.5 : 2);
    canvas.width = W * dpr; canvas.height = H * dpr;
    const count = Math.round(Math.min(150, Math.max(46, (W * H) / 9000)));
    nodes = Array.from({ length: count }, () => {
      const z = 0.25 + Math.random() * 0.75;
      return { ax: Math.random() * W, ay: Math.random() * H * 1.5, z,
        amp: 6 + Math.random() * 18, sp: 0.00012 + Math.random() * 0.0003, ph: Math.random() * 6.28, lit: 0, x: 0, y: 0 };
    });
    edges = []; adj = nodes.map(() => []);
    nodes.forEach((a, i) => {
      const near = nodes.map((b, j) => [j, (a.ax - b.ax) ** 2 + (a.ay - b.ay) ** 2]).filter(([j]) => j !== i).sort((p, q) => p[1] - q[1]).slice(0, 3);
      for (const [j] of near) if (!adj[i].includes(j) && Math.abs(a.z - nodes[j].z) < 0.45) { edges.push([i, j]); adj[i].push(j); adj[j].push(i); }
    });
    frame(performance.now());
  }
  function spawnPulse() {
    let i = Math.floor(Math.random() * nodes.length);
    const path = [i];
    for (let k = 0; k < 5; k++) {
      const next = adj[i].filter((j) => !path.includes(j));
      if (!next.length) break;
      i = next[Math.floor(Math.random() * next.length)];
      path.push(i);
    }
    if (path.length > 2) pulses.push({ path, t: 0, warm: Math.random() < 0.25 });
  }
  function frame(now) {
    const t = now - t0;
    const sy = REDUCED ? 0 : scrollY;
    mx += (tmx - mx) * 0.05; my += (tmy - my) * 0.05;
    for (const n of nodes) {
      n.x = n.ax + Math.cos(t * n.sp + n.ph) * n.amp + mx * n.z * 26;
      n.y = n.ay + Math.sin(t * n.sp * 1.3 + n.ph) * n.amp - sy * (0.15 + n.z * 0.45) + my * n.z * 18;
      n.lit *= 0.965;
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    for (const [i, j] of edges) {
      const a = nodes[i], b = nodes[j];
      if ((a.y < -40 && b.y < -40) || (a.y > H + 40 && b.y > H + 40)) continue;
      const lit = Math.max(a.lit, b.lit);
      ctx.strokeStyle = lit > 0.05 ? `rgba(127,220,199,${0.12 + lit * 0.6})` : `rgba(160,220,200,${0.05 + a.z * 0.1})`;
      ctx.lineWidth = 0.6 + a.z * 0.8;
      ctx.beginPath(); ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.stroke();
    }
    for (const n of nodes) {
      if (n.y < -20 || n.y > H + 20) continue;
      const r = 1.2 + n.z * 2.6 + n.lit * 3;
      ctx.fillStyle = n.lit > 0.05 ? `rgba(${n.warm ? "229,126,94" : "127,220,199"},${0.5 + n.lit * 0.5})` : `rgba(210,240,230,${0.25 + n.z * 0.5})`;
      ctx.beginPath(); ctx.arc(n.x, n.y, r, 0, 6.283); ctx.fill();
    }
    // pulses travel along a path, lighting each node they reach
    for (const p of pulses) {
      p.t += 0.022;
      const seg = Math.floor(p.t), f = p.t - seg;
      if (seg >= p.path.length - 1) { p.done = true; continue; }
      const a = nodes[p.path[seg]], b = nodes[p.path[seg + 1]];
      if (f < 0.05) { a.lit = 1; a.warm = p.warm; }
      const x = a.x + (b.x - a.x) * f, y = a.y + (b.y - a.y) * f;
      const g = ctx.createRadialGradient(x, y, 0, x, y, 12);
      g.addColorStop(0, p.warm ? "rgba(229,126,94,.95)" : "rgba(127,220,199,.95)");
      g.addColorStop(1, "rgba(127,220,199,0)");
      ctx.fillStyle = g; ctx.beginPath(); ctx.arc(x, y, 12, 0, 6.283); ctx.fill();
      if (seg === p.path.length - 2 && f > 0.95) { b.lit = 1; b.warm = p.warm; }
    }
    pulses = pulses.filter((p) => !p.done);
  }
  function loop(now) {
    raf = 0;
    if (!visible || document.hidden) return;
    if (Math.random() < 0.03 && pulses.length < 6) spawnPulse();
    frame(now);
    raf = requestAnimationFrame(loop);
  }
  function startLoop() { if (!raf && !REDUCED) raf = requestAnimationFrame(loop); }
  new IntersectionObserver(([e]) => { visible = e.isIntersecting; if (visible) startLoop(); }).observe(canvas);
  document.addEventListener("visibilitychange", startLoop);
  addEventListener("pointermove", (e) => { if (e.pointerType === "mouse") { tmx = e.clientX / innerWidth - 0.5; tmy = e.clientY / innerHeight - 0.5; } }, { passive: true });
  let rt = 0;
  addEventListener("resize", () => { clearTimeout(rt); rt = setTimeout(build, 150); });
  build();
  startLoop();
})();

/* ───────────────────────── Start ───────────────────────── */

renderGraphChips();
state.graph = GRAPHS[0];
renderGraphChips();
renderAbout();
renderQuestions();
selectQuestion(GRAPHS[0].questions[0]);
clearResults("Scroll here and the first question runs on its own. Or press Run.");
drawStrip();

// Load the engine when the demo comes near, and run the first question once.
let autoRan = false;
new IntersectionObserver(([e]) => {
  if (e.isIntersecting && !autoRan) {
    autoRan = true;
    run();
  }
}, { rootMargin: "200px 0px" }).observe($("#console"));
