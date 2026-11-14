// catalog.js — the plaza front page.
//
// Loads the manifest and paints a placeholder card for every dataset at once,
// from manifest fields alone (title, blurb, tags, icon). Then it reads each
// dataset's embedded Card live — two HTTP range requests, never the graph — at
// most MAX_READS at a time, and fills the cards in as they arrive. The
// fingerprint artwork is drawn lazily, only for cards that scroll into view.
//
// One failing or slow dataset never blocks the rest: every read has a timeout
// and falls back to the manifest.
//
// Browsing is faceted: values inside one group widen (OR), groups narrow each
// other (AND), and each value carries the count it would yield against the
// OTHER groups' selections, so a number is never a lie about what a click does.
// The whole view (query, filters, sort, layout) lives in the URL.
//
// window.plaza (bottom of the file) is the programmatic surface that
// js/webmcp.js is written against. Keep it stable.
import { readReteCard, liteCardFromHeader, fmtBytes, plainDescription } from "./rete-card.js";
import { imageInfoFromCard } from "./procgen.js";
import { renderFingerprint } from "./procgen-p5.js";
import { derivedFacets, FILTERABLE } from "./facets.js";
import { detectProviders } from "./providers.js";
import { usedOntologies } from "./vocabs.js";
import { mountSearch, buildDoc, scoreDoc, queryTokens, highlight, fold, escapeHtml as esc, FIELD_LABEL } from "./search.js";

const ART = { w: 520, h: 325 };
const MAX_READS = 6;            // concurrent card reads
const READ_TIMEOUT_MS = 25000;  // a slow host must not hold a slot forever
const READY_TIMEOUT_MS = 15000; // plaza.ready never waits longer than this
const CACHE_PREFIX = "plaza:card:v1:";
const SIDEBAR_ROWS = 6;

const $ = (sel, el = document) => el.querySelector(sel);
const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const abs = (u) => { try { return new URL(u, document.baseURI).href; } catch { return String(u); } };
const pageUrl = (key) => abs(`dataset.html?key=${encodeURIComponent(key)}`);

const cardsEl = $("#cards");
const emptyEl = $("#empty");
const countEl = $("#count");
const facetsEl = $("#facets");
const activeEl = $("#active");
const clearBtn = $("#clearAll");
const sortEl = $("#sort");
const qEl = $("#q");

// ── state ───────────────────────────────────────────────────────────────────
let entries = [];                 // records, in manifest order
const byKey = new Map();
let query = "";
let sortBy = "relevance";
let view = "grid";
const selected = new Map();       // groupId -> Set(value)
let manifestLoaded = false;

// ── facet model ─────────────────────────────────────────────────────────────
const SIZE_BUCKETS = [
  { label: "under 1 MB", test: (n) => n != null && n < 1e6 },
  { label: "1 – 50 MB", test: (n) => n != null && n >= 1e6 && n < 5e7 },
  { label: "50 – 500 MB", test: (n) => n != null && n >= 5e7 && n < 5e8 },
  { label: "over 500 MB", test: (n) => n != null && n >= 5e8 },
];

// "GeoSPARQL" is both a derived FEATURE (the file carries geometry) and a
// VOCABULARY (it imports the ontology): the sidebar wording tells them apart.
const FEATURE_LABEL = {
  GeoSPARQL: "has geometry",
  temporal: "has time",
  multilingual: "multilingual",
  incoherent: "schema defects",
  "header-only": "no embedded card",
};

const GROUPS = [
  { id: "type", label: "Type", open: true, of: (r) => [categoryOf(r) === "ontology" ? "ontology" : "dataset"] },
  { id: "kind", label: "Delivery", open: true, of: (r) => (r.facets || []).filter((f) => f === "remote" || f === "bundled") },
  { id: "feature", label: "Features", open: true, display: (v) => FEATURE_LABEL[v] || v,
    of: (r) => (r.facets || []).filter((f) => FILTERABLE.has(f) && f !== "remote" && f !== "bundled") },
  { id: "vocab", label: "Built with", open: true, of: (r) => (r.ontologies || []).map((o) => o.name) },
  { id: "topic", label: "Topic", open: true, of: (r) => r.entry.tags || [] },
  { id: "licence", label: "Licence", open: false, of: (r) => { const l = (r.card && r.card.license) || r.entry.license; return l ? [l] : []; } },
  { id: "size", label: "File size", open: false, of: (r) => { const b = SIZE_BUCKETS.find((s) => s.test(r.size)); return b ? [b.label] : []; } },
  { id: "provider", label: "Connected to", open: false, of: (r) => (r.providers || []).map((p) => p.name) },
];
const GROUP_BY_ID = Object.fromEntries(GROUPS.map((g) => [g.id, g]));
const openGroups = new Set(GROUPS.filter((g) => g.open).map((g) => g.id));

const valuesOf = (rec, gid) => {
  try { return GROUP_BY_ID[gid].of(rec) || []; } catch { return []; }
};
const labelOf = (gid, v) => { const g = GROUP_BY_ID[gid]; return g && g.display ? g.display(v) : v; };

// A dataset's category: its manifest `category`, else derived (ontology tags, or
// an owl-class-dominated schema).
function categoryOf(rec) {
  if (rec.entry.category) return rec.entry.category;
  const tags = (rec.entry.tags || []).map((t) => t.toLowerCase());
  if (tags.some((t) => ["ontology", "owl", "schema", "obo", "rdfs"].includes(t))) return "ontology";
  const cls = (rec.card && rec.card.classes) || [];
  if (cls.length) {
    const total = cls.reduce((a, c) => a + (c[1] || 0), 0) || 1;
    const owl = cls
      .filter(([iri]) => /2002\/07\/owl#(Class|Restriction|Axiom)|rdf-schema#Class|#Ontology/.test(iri))
      .reduce((a, c) => a + (c[1] || 0), 0);
    if (owl / total > 0.5) return "ontology";
  }
  return "graph";
}

// ── pure view evaluation (shared by the UI and plaza.search) ────────────────
/** Does `rec` satisfy every group in `filters` except `skip`? */
function passesFacets(rec, filters, skip) {
  for (const [gid, set] of filters) {
    if (!set.size || gid === skip) continue;
    const vals = valuesOf(rec, gid);
    if (!vals.some((v) => set.has(v))) return false;
  }
  return true;
}

const tri = (r) => (r.card && (r.card.triple_count ?? r.card.quad_count)) || 0;
const nameOf = (r) => String(r.entry.title || r.entry.key);
const SORTERS = {
  relevance: () => 0, // manifest order (stable, curated)
  name: (a, b) => nameOf(a.rec).localeCompare(nameOf(b.rec)),
  triples: (a, b) => tri(b.rec) - tri(a.rec),
  size: (a, b) => (b.rec.size || 0) - (a.rec.size || 0),
  vocabs: (a, b) => (b.rec.ontologies || []).length - (a.rec.ontologies || []).length,
};

/** The records a view shows, in order: [{ rec, score, via }]. Does not touch the UI. */
function evalView({ q = "", filters = new Map(), sort = "relevance" }) {
  const qt = queryTokens(q);
  const rawFolded = fold(q).trim();
  const out = [];
  entries.forEach((rec, i) => {
    if (!passesFacets(rec, filters, null)) return;
    let score = 0, via = "";
    if (qt.length) {
      const r = rec.doc ? scoreDoc(rec.doc, qt, rawFolded) : null;
      if (!r) return;
      score = r.score; via = r.via;
    }
    out.push({ rec, score, via, i });
  });
  const cmp = SORTERS[sort] || SORTERS.relevance;
  out.sort((a, b) => {
    if (sort === "relevance" && qt.length) return b.score - a.score || a.i - b.i;
    return cmp(a, b) || (qt.length ? b.score - a.score : 0) || a.i - b.i;
  });
  return out;
}

const currentView = () => ({ q: query, filters: selected, sort: sortBy });

/** [value, count] for one group against the OTHER groups + the query. Seeded with
 *  every value in the corpus, so an option the filters reduce to zero shows as 0
 *  rather than vanishing (a vanishing checkbox reads as a bug). */
function countsFor(gid, v = currentView(), ignoreQuery = false) {
  const tally = new Map();
  for (const rec of entries) for (const x of valuesOf(rec, gid)) tally.set(x, 0);
  const qt = ignoreQuery ? [] : queryTokens(v.q);
  const raw = fold(v.q).trim();
  for (const rec of entries) {
    if (!passesFacets(rec, v.filters, gid)) continue;
    if (qt.length && !(rec.doc && scoreDoc(rec.doc, qt, raw))) continue;
    for (const x of valuesOf(rec, gid)) tally.set(x, (tally.get(x) || 0) + 1);
  }
  return [...tally.entries()].sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])));
}

// ── records ─────────────────────────────────────────────────────────────────
function localName(iri) {
  const s = String(iri || "").replace(/[#/]+$/, "");
  const i = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return i >= 0 ? s.slice(i + 1) : s;
}

/** Recompute everything derived from the card (or, before it arrives, the manifest). */
function derive(rec) {
  const card = rec.card || {};
  rec.facets = derivedFacets(card, rec.entry);
  rec.providers = detectProviders(card, rec.entry);
  rec.ontologies = usedOntologies(card, rec.entry);
  const e = rec.entry;
  const desc = plainDescription(card.description || e.blurb || "");
  rec.doc = buildDoc({
    title: [e.title, card.title && card.title !== e.title ? card.title : null],
    key: [e.key],
    tags: e.tags || [],
    vocab: rec.ontologies.map((o) => o.name),
    licence: [card.license || e.license],
    classes: (card.classes || []).slice(0, 120).map(([iri]) => localName(iri)),
    predicates: (card.predicates || []).slice(0, 120).map(([iri]) => localName(iri)),
    other: [
      ...rec.facets.filter((f) => f !== "remote" && f !== "bundled").map((f) => FEATURE_LABEL[f] || f),
      ...rec.providers.map((p) => p.name),
      categoryOf(rec),
      e.kind === "remote-lazy" ? "remote" : "bundled",
    ],
    text: [e.blurb, desc],
  });
  rec.paintKey = null;
}

// ── card reads: bounded pool, sessionStorage cache, timeouts ────────────────
const readQueue = [];
let reading = 0;
const pending = () => entries.filter((r) => r.state === "pending" || r.state === "loading").length;

function cacheGet(url) {
  try { const s = sessionStorage.getItem(CACHE_PREFIX + url); return s ? JSON.parse(s) : null; } catch { return null; }
}
function cachePut(url, val) {
  try { sessionStorage.setItem(CACHE_PREFIX + url, JSON.stringify(val)); } catch { /* full or blocked: fine */ }
}
const withTimeout = (p, ms) =>
  Promise.race([p, new Promise((_, rej) => setTimeout(() => rej(new Error(`timed out after ${ms / 1000}s`)), ms))]);

function enqueueRead(rec, front = false) {
  if (rec.state !== "pending") return;
  const at = readQueue.indexOf(rec);
  if (at >= 0) { if (!front) return; readQueue.splice(at, 1); }
  front ? readQueue.unshift(rec) : readQueue.push(rec);
  pump();
}
function pump() {
  while (reading < MAX_READS && readQueue.length) {
    const rec = readQueue.shift();
    if (rec.state !== "pending") continue;
    rec.state = "loading";
    reading++;
    loadCard(rec).finally(() => { reading--; pump(); });
  }
}

async function loadCard(rec) {
  const url = abs(rec.entry.rete);
  try {
    let got = cacheGet(url);
    if (!got) {
      got = await withTimeout(readReteCard(url), READ_TIMEOUT_MS);
      cachePut(url, { header: got.header, card: got.card, size: got.size });
    }
    rec.header = got.header;
    rec.size = got.size;
    rec.rawCard = got.card || null;
    rec.card = got.card || liteCardFromHeader(got.header, rec.entry);
    rec.state = "ok";
  } catch (err) {
    // CORS / offline / not-a-rete / slow host: fall back to the manifest.
    rec.card = { _unreachable: true, title: rec.entry.title, description: rec.entry.blurb };
    rec.rawCard = null;
    rec.error = String(err && err.message ? err.message : err);
    rec.state = "error";
  }
  derive(rec);
  rec._settle();
  scheduleRender();
  maybeDrawArt(rec);
}

// ── artwork: drawn lazily, one at a time ────────────────────────────────────
const artQueue = [];
let drawing = false;
function maybeDrawArt(rec) {
  if (!rec.visible || rec.img || rec.artQueued || rec.state === "pending" || rec.state === "loading") return;
  rec.artQueued = true;
  artQueue.push(rec);
  drainArt();
}
async function drainArt() {
  if (drawing) return;
  drawing = true;
  while (artQueue.length) {
    const rec = artQueue.shift();
    try {
      const mode = categoryOf(rec) === "ontology" ? "ontology" : "dataset";
      rec.img = await renderFingerprint(imageInfoFromCard(rec.card, rec.entry, rec.header, mode), { theme: "dark", ...ART });
    } catch { rec.img = null; }
    rec.artQueued = false;
    if (rec.img && rec.node) {
      const art = rec.node.querySelector(".pz-art");
      let im = art.querySelector("img");
      if (!im) { im = document.createElement("img"); im.alt = ""; im.decoding = "async"; art.prepend(im); }
      im.onload = () => { im.classList.add("in"); art.querySelector(".pz-art-ph")?.remove(); };
      im.src = rec.img;
    }
    await new Promise((r) => setTimeout(r, 0)); // let the browser paint between sketches
  }
  drawing = false;
}

const io = "IntersectionObserver" in window
  ? new IntersectionObserver((list) => {
      for (const e of list) {
        const rec = byKey.get(e.target.dataset.key);
        if (!rec) continue;
        rec.visible = e.isIntersecting;
        if (e.isIntersecting) { enqueueRead(rec, true); maybeDrawArt(rec); }
      }
    }, { rootMargin: "300px 0px" })
  : null;

// ── card nodes: created once, repainted in place ────────────────────────────
function splitTitle(t) {
  const s = String(t || "");
  const m = s.match(/^(.*?)\s+[—–-]\s+(.*)$/);
  return m ? { name: m[1].trim(), sub: m[2].trim() } : { name: s.trim(), sub: "" };
}

function makeNode(rec) {
  const a = document.createElement("a");
  a.className = "pz-card is-pending";
  a.href = `dataset.html?key=${encodeURIComponent(rec.entry.key)}`;
  a.dataset.key = rec.entry.key;
  a.innerHTML = `<div class="pz-art"><span class="pz-art-ph" aria-hidden="true">${esc(rec.entry.icon || "")}</span><span class="pz-kind"></span></div><div class="pz-body"></div>`;
  rec.node = a;
  if (io) io.observe(a); else { rec.visible = true; }
}

function paintNode(rec, qt) {
  const key = `${qt.join(" ")}|${rec.state}`;
  if (rec.paintKey === key) return;
  rec.paintKey = key;
  const { entry, card } = rec;
  const loaded = rec.state === "ok" || rec.state === "error";
  const { name, sub } = splitTitle(entry.title || entry.key);
  const blurb = plainDescription((card && card.description) || entry.blurb || "");
  const triples = card && (card.triple_count ?? card.quad_count);
  const terms = card && card.term_count;
  const license = (card && card.license) || entry.license;
  const size = fmtBytes(rec.size);
  const kind = categoryOf(rec) === "ontology" ? "ontology" : "dataset";

  rec.node.classList.toggle("is-pending", !loaded);
  rec.node.querySelector(".pz-kind").textContent = kind === "ontology" ? "ontology" : rec.entry.kind === "remote-lazy" ? "remote" : "bundled";
  rec.node.setAttribute("aria-label", name);

  const meta = loaded
    ? [
        triples != null ? `<span><b>${fmt(triples)}</b> triples</span>` : "",
        size ? `<span><b>${size}</b></span>` : "",
        terms != null ? `<span><b>${fmt(terms)}</b> terms</span>` : "",
        license ? `<span>${highlight(license, qt)}</span>` : "",
        rec.state === "error" ? `<span title="${esc(rec.error)}">card unreachable</span>` : "",
        card && card._lite ? `<span title="the file has no embedded card">header only</span>` : "",
      ].join("")
    : `<span class="pz-skel">00,000 triples</span><span class="pz-skel">00 MB</span>`;

  rec.node.querySelector(".pz-body").innerHTML = `
    <h3 class="pz-title">${highlight(name, qt)}</h3>
    ${sub ? `<p class="pz-sub">${highlight(sub, qt)}</p>` : ""}
    <p class="pz-blurb">${highlight(blurb, qt)}</p>
    <div class="pz-meta">${meta}</div>
    <div class="pz-tags">${(entry.tags || []).slice(0, 4).map((t) => `<span class="pz-chip">${highlight(t, qt)}</span>`).join("")}</div>`;
}

// ── render ──────────────────────────────────────────────────────────────────
let rafId = 0;
function scheduleRender() {
  if (rafId) return;
  rafId = requestAnimationFrame(() => { rafId = 0; renderNow(); });
}

let lastShown = [];
function renderNow() {
  const result = evalView(currentView());
  lastShown = result;
  const qt = queryTokens(query);
  const shownSet = new Set(result.map((r) => r.rec));

  // cards: paint, show/hide, and reorder only what moved
  for (const rec of entries) if (!rec.node) makeNode(rec);
  const order = result.map((r) => r.rec).concat(entries.filter((r) => !shownSet.has(r)));
  order.forEach((rec, i) => {
    rec.node.hidden = !shownSet.has(rec);
    if (shownSet.has(rec)) paintNode(rec, qt);
    if (cardsEl.children[i] !== rec.node) cardsEl.insertBefore(rec.node, cardsEl.children[i] || null);
  });
  cardsEl.dataset.view = view;

  renderFacets();
  renderActive();
  renderSideToggle();
  renderPicker();
  renderHeroStats();
  renderHint();
  renderVocabs(result);

  const n = result.length;
  const left = pending();
  const filtered = query || [...selected.values()].some((s) => s.size);
  countEl.innerHTML = `${fmt(n)} ${n === 1 ? "dataset" : "datasets"}` +
    (filtered ? `<small>of ${fmt(entries.length)}</small>` : "") +
    (left ? `<small>reading ${fmt(left)} card${left === 1 ? "" : "s"}…</small>` : "");

  sortEl.options[0].textContent = qt.length ? "Best match" : "Featured";
  sortEl.value = sortBy;
  $("#viewGrid").setAttribute("aria-pressed", String(view === "grid"));
  $("#viewList").setAttribute("aria-pressed", String(view === "list"));

  const none = manifestLoaded && n === 0;
  emptyEl.hidden = !none;
  if (none) {
    const parts = [];
    if (query) parts.push(`the search <q>${esc(query)}</q>`);
    for (const [gid, set] of selected) for (const v of set) parts.push(`${esc(GROUP_BY_ID[gid].label.toLowerCase())}: ${esc(labelOf(gid, v))}`);
    $("#emptyMsg").innerHTML = (parts.length ? `Nothing in the plaza fits ${parts.join(", ")}.` : "The plaza has no datasets yet.") +
      (left ? ` ${left} card${left === 1 ? " is" : "s are"} still loading.` : "");
    $("#emptyClear").hidden = !parts.length;
  }
  search.refresh();
}

// ── sidebar ─────────────────────────────────────────────────────────────────
function toggle(gid, value, push = true) {
  const set = selected.get(gid) || new Set();
  set.has(value) ? set.delete(value) : set.add(value);
  set.size ? selected.set(gid, set) : selected.delete(gid);
  writeURL(push);
  renderNow();
}

function renderFacets() {
  const focused = document.activeElement && document.activeElement.dataset && document.activeElement.dataset.fk;
  facetsEl.innerHTML = "";
  for (const g of GROUPS) {
    const counts = countsFor(g.id);
    if (!counts.length) continue;
    const sel = selected.get(g.id) || new Set();
    const det = document.createElement("details");
    det.className = "pz-group";
    det.open = openGroups.has(g.id) || sel.size > 0;
    det.innerHTML = `<summary>${esc(g.label)}${sel.size ? `<span class="n">${sel.size}</span>` : ""}</summary>`;
    det.addEventListener("toggle", () => { det.open ? openGroups.add(g.id) : openGroups.delete(g.id); });

    const box = document.createElement("div");
    box.className = "pz-opts";
    let show = counts.slice(0, SIDEBAR_ROWS);
    for (const c of counts) if (sel.has(c[0]) && !show.includes(c)) show.push(c); // a filter you cannot see is a filter you cannot remove
    for (const [value, n] of show) box.appendChild(optButton(g.id, value, n, sel.has(value), "pz-opt"));
    if (counts.length > show.length) {
      const more = document.createElement("button");
      more.type = "button";
      more.className = "pz-link pz-more";
      more.textContent = `Show all ${counts.length}`;
      more.onclick = () => openPicker(g.id, more);
      box.appendChild(more);
    }
    det.appendChild(box);
    facetsEl.appendChild(det);
  }
  if (focused) facetsEl.querySelector(`[data-fk="${CSS.escape(focused)}"]`)?.focus({ preventScroll: true });
}

function optButton(gid, value, n, on, cls) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = cls + (on ? " on" : "");
  b.dataset.fk = `${gid}|${value}`;
  b.setAttribute("role", "checkbox");
  b.setAttribute("aria-checked", String(on));
  if (!n && !on) b.disabled = true;
  const shown = labelOf(gid, value);
  b.title = shown === value ? value : `${shown} (${value})`;
  b.innerHTML = `<span class="box" aria-hidden="true">${on ? "✓" : ""}</span><span class="lab">${esc(shown)}</span><span class="n" aria-label="${n} datasets">${n}</span>`;
  b.onclick = () => toggle(gid, value);
  return b;
}

// ── facet picker (a group's full value list, searchable) ────────────────────
const picker = $("#facetModal");
let pickerGid = null, pickerQuery = "", pickerOpener = null;

function openPicker(gid, opener) {
  pickerGid = gid; pickerQuery = ""; pickerOpener = opener || document.activeElement;
  $("#facetSearch").value = "";
  $("#facetTitle").textContent = GROUP_BY_ID[gid].label;
  picker.hidden = false;
  renderPicker();
  setTimeout(() => $("#facetSearch").focus(), 30);
}
function closePicker() {
  picker.hidden = true; pickerGid = null;
  try { pickerOpener?.focus(); } catch { /* element gone */ }
}
function renderPicker() {
  if (!pickerGid) return;
  const g = GROUP_BY_ID[pickerGid];
  const sel = selected.get(pickerGid) || new Set();
  const counts = countsFor(pickerGid);
  const qt = queryTokens(pickerQuery);
  const hits = counts.filter(([v]) => { const f = fold(labelOf(pickerGid, v)); return qt.every((t) => f.includes(t)); });
  hits.sort((a, b) => Number(sel.has(b[0])) - Number(sel.has(a[0])) || b[1] - a[1] || String(a[0]).localeCompare(String(b[0])));
  const list = $("#facetList");
  const keep = list.scrollTop;
  list.innerHTML = hits.length ? "" : `<div class="pz-picker-empty">Nothing matches “${esc(pickerQuery)}”.</div>`;
  for (const [value, n] of hits) list.appendChild(optButton(pickerGid, value, n, sel.has(value), "pz-opt"));
  list.scrollTop = keep;
  const shown = lastShown.length;
  $("#facetSummary").textContent = `${sel.size} selected · ${shown} of ${entries.length} datasets`;
  $("#facetClear").disabled = !sel.size;
  void g;
}
$("#facetSearch")?.addEventListener("input", (e) => { pickerQuery = e.target.value.trim(); renderPicker(); });
$("#facetClear")?.addEventListener("click", () => { if (pickerGid) selected.delete(pickerGid); writeURL(true); renderNow(); });
$("#facetDone")?.addEventListener("click", closePicker);
$("#facetX")?.addEventListener("click", closePicker);
picker?.addEventListener("click", (e) => { if (e.target === picker) closePicker(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape" && pickerGid) closePicker(); });

// ── active filters, clearing, toolbar ───────────────────────────────────────
function renderActive() {
  activeEl.innerHTML = "";
  let any = false;
  for (const [gid, set] of selected) {
    for (const value of set) {
      any = true;
      const tok = document.createElement("span");
      tok.className = "pz-chip on";
      tok.innerHTML = `<em>${esc(GROUP_BY_ID[gid].label)}</em>${esc(labelOf(gid, value))}<button type="button" aria-label="Remove filter ${esc(labelOf(gid, value))}">&times;</button>`;
      tok.querySelector("button").onclick = () => toggle(gid, value);
      activeEl.appendChild(tok);
    }
  }
  clearBtn.hidden = !any;
}

function clearAll() {
  selected.clear(); query = ""; qEl.value = ""; search.syncClear();
  writeURL(true); renderNow();
}
clearBtn.onclick = clearAll;
$("#emptyClear").onclick = clearAll;
sortEl.onchange = () => { sortBy = sortEl.value; writeURL(true); renderNow(); };
$("#viewGrid").onclick = () => setView("grid");
$("#viewList").onclick = () => setView("list");
function setView(v) {
  view = v;
  try { localStorage.setItem("plaza-view", v); } catch { /* fine */ }
  writeURL(false); renderNow();
}

const side = $("#side");
const sideToggle = $("#sideToggle");
sideToggle.addEventListener("click", () => {
  const open = side.classList.toggle("open");
  sideToggle.setAttribute("aria-expanded", String(open));
});
function renderSideToggle() {
  const n = [...selected.values()].reduce((a, s) => a + s.size, 0);
  const badge = $("#sideToggleN");
  badge.hidden = !n;
  badge.textContent = String(n);
}

// ── hero ────────────────────────────────────────────────────────────────────
function renderHeroStats() {
  const withCard = entries.filter((r) => r.card && !r.card._unreachable);
  const triples = withCard.reduce((a, r) => a + (r.card.triple_count ?? r.card.quad_count ?? 0), 0);
  const bytes = entries.reduce((a, r) => a + (r.size || 0), 0);
  const vocabs = new Set(entries.flatMap((r) => (r.state === "ok" ? r.ontologies || [] : []).map((o) => o.name)));
  const left = pending();
  const done = entries.length - left;
  const el = $("#heroStats");
  const stat = (b, s) => `<div class="pz-stat"><b>${esc(String(b))}</b><span>${s}</span></div>`;
  el.innerHTML =
    stat(fmt(entries.length), entries.length === 1 ? "dataset" : "datasets") +
    stat(triples ? fmt(triples) : "…", "triples") +
    stat(bytes ? fmtBytes(bytes) : "…", "of graph, never downloaded") +
    stat(vocabs.size || "…", "vocabularies") +
    (left ? `<div class="pz-stats-note" aria-hidden="true">${done} of ${entries.length} cards read</div>` : "");
}

let hintKey = "";
function renderHint() {
  const topics = countsFor("topic", { q: "", filters: new Map(), sort: "relevance" }).filter(([, n]) => n).slice(0, 7);
  const sel = selected.get("topic") || new Set();
  const key = topics.map((t) => t[0]).join("|") + "#" + [...sel].join("|");
  if (key === hintKey) return;
  hintKey = key;
  const el = $("#hint");
  el.innerHTML = topics.length ? `<span>Browse by topic</span>` : "";
  for (const [value] of topics) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pz-chip" + (sel.has(value) ? " on" : "");
    b.setAttribute("aria-pressed", String(sel.has(value)));
    b.textContent = value;
    b.onclick = () => { toggle("topic", value); document.getElementById("results").scrollIntoView({ block: "start" }); };
    el.appendChild(b);
  }
}

function renderVocabs(result) {
  const m = new Map();
  for (const { rec } of result) for (const o of rec.ontologies || []) m.set(o.name, (m.get(o.name) || 0) + 1);
  const top = [...m.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, 30);
  const box = $("#vocabs");
  box.hidden = !top.length;
  $("#vocabRow").innerHTML = top
    .map(([name, n]) => `<a class="pz-chip" href="ontology.html?id=${encodeURIComponent(name)}">${esc(name)} <em>${n}</em></a>`)
    .join("");
}

// ── URL state ───────────────────────────────────────────────────────────────
const SORT_IDS = new Set(Object.keys(SORTERS));
function readURL() {
  const p = new URLSearchParams(location.search);
  query = p.get("q") || "";
  const s = p.get("sort");
  sortBy = SORT_IDS.has(s) ? s : "relevance";
  let stored = null;
  try { stored = localStorage.getItem("plaza-view"); } catch { /* fine */ }
  view = p.get("view") === "list" ? "list" : p.get("view") === "grid" ? "grid" : stored === "list" ? "list" : "grid";
  selected.clear();
  for (const g of GROUPS) {
    const vals = p.getAll(g.id).filter(Boolean);
    if (vals.length) selected.set(g.id, new Set(vals));
  }
  qEl.value = query;
  search.syncClear();
}
let urlTimer = 0;
function writeURL(push) {
  clearTimeout(urlTimer);
  const p = new URLSearchParams();
  if (query) p.set("q", query);
  for (const g of GROUPS) for (const v of selected.get(g.id) || []) p.append(g.id, v);
  if (sortBy !== "relevance") p.set("sort", sortBy);
  if (view === "list") p.set("view", "list");
  const qs = p.toString();
  const url = location.pathname + (qs ? "?" + qs : "") + location.hash;
  if (url === location.pathname + location.search + location.hash) return;
  try { history[push ? "pushState" : "replaceState"](null, "", url); } catch { /* sandboxed */ }
}
window.addEventListener("popstate", () => { readURL(); renderNow(); });

// ── search wiring ───────────────────────────────────────────────────────────
const goToResults = () => document.getElementById("results").scrollIntoView({ block: "start" });

const search = mountSearch({
  input: qEl,
  panel: $("#ac"),
  clearBtn: $("#qClear"),
  onQuery: (q) => {
    query = q.trim() ? q : "";
    scheduleRender();
    clearTimeout(urlTimer);
    urlTimer = setTimeout(() => writeURL(false), 250);
  },
  onCommit: () => { writeURL(false); goToResults(); },
  suggest: (raw) => {
    const qt = queryTokens(raw);
    const out = [];
    const ds = [];
    const rf = fold(raw).trim();
    for (const rec of entries) {
      const r = rec.doc && scoreDoc(rec.doc, qt, rf);
      if (r) ds.push({ rec, ...r });
    }
    ds.sort((a, b) => b.score - a.score);
    for (const { rec, via } of ds.slice(0, 5)) {
      const t = tri(rec);
      out.push({
        type: "dataset",
        label: rec.entry.title || rec.entry.key,
        meta: via && FIELD_LABEL[via] ? `via ${FIELD_LABEL[via]}` : t ? `${fmt(t)} triples` : "",
        pick: () => { location.href = `dataset.html?key=${encodeURIComponent(rec.entry.key)}`; },
      });
    }
    const kinds = { topic: "tag", vocab: "vocabulary", licence: "licence", provider: "provider", feature: "feature" };
    for (const [gid, type] of Object.entries(kinds)) {
      for (const [value, n] of countsFor(gid, currentView(), true)) {
        if (!n) continue;
        const words = fold(labelOf(gid, value)).split(/[^p{L}p{N}]+/u);
        if (!qt.every((t) => words.some((w) => w.startsWith(t)))) continue;
        out.push({
          type, label: labelOf(gid, value), meta: `${n} dataset${n > 1 ? "s" : ""}`,
          pick: () => { const set = selected.get(gid); if (!set || !set.has(value)) toggle(gid, value); goToResults(); },
        });
      }
    }
    return out;
  },
});

// ── boot ────────────────────────────────────────────────────────────────────
function startRecord(entry) {
  const rec = { entry, key: entry.key, card: null, rawCard: null, header: null, size: null, img: null, state: "pending", visible: false };
  rec.ready = new Promise((res) => { rec._settle = res; });
  // From the manifest alone: facets from tags/kind/licence, a search doc, a node.
  rec.card = null;
  derive(rec);
  return rec;
}

const ready = (async function main() {
  readURL();
  let manifest;
  try {
    manifest = await fetch("plaza.json").then((r) => { if (!r.ok) throw new Error(`HTTP ${r.status}`); return r.json(); });
  } catch (err) {
    manifestLoaded = true;
    countEl.textContent = "The catalogue could not be loaded.";
    console.error("plaza.json:", err);
    renderNow();
    return;
  }
  entries = (manifest.datasets || []).map(startRecord);
  for (const rec of entries) byKey.set(rec.key, rec);
  manifestLoaded = true;
  renderNow();
  // Cards in manifest order; the ones scrolled into view jump the queue.
  for (const rec of entries) enqueueRead(rec);
  await Promise.race([
    Promise.all(entries.map((r) => r.ready)),
    new Promise((res) => setTimeout(res, READY_TIMEOUT_MS)),
  ]);
})();

// ── window.plaza: the programmatic API ──────────────────────────────────────
function summary(rec) {
  const c = rec.card || {};
  const e = rec.entry;
  const desc = plainDescription(c.description || e.blurb || "");
  return {
    key: e.key,
    title: e.title || e.key,
    description: desc.length > 600 ? desc.slice(0, 597) + "…" : desc,
    category: categoryOf(rec),
    tags: [...(e.tags || [])],
    licence: c.license || e.license || null,
    triples: c.triple_count ?? c.quad_count ?? null,
    terms: c.term_count ?? null,
    bytes: rec.size ?? null,
    vocabularies: (rec.ontologies || []).map((o) => o.name),
    features: valuesOf(rec, "feature"),
    delivery: e.kind === "remote-lazy" ? "remote" : "bundled",
    url: abs(e.rete),
    page: pageUrl(e.key),
  };
}

/** { groupId: [values] } -> Map<groupId, Set>; unknown groups dropped, values
 *  matched case-insensitively to the ones the catalogue really has. */
function normFilters(obj) {
  const m = new Map();
  for (const [gid, raw] of Object.entries(obj || {})) {
    if (!GROUP_BY_ID[gid]) continue;
    const known = new Map();
    for (const rec of entries) for (const v of valuesOf(rec, gid)) known.set(fold(v), v);
    const set = new Set();
    for (const v of [].concat(raw || [])) { const s = String(v); set.add(known.get(fold(s)) || s); }
    if (set.size) m.set(gid, set);
  }
  return m;
}

window.plaza = {
  ready,
  list: () => entries.map(summary),
  search({ query: q = "", filters = {}, sort = "relevance", limit } = {}) {
    const res = evalView({ q: String(q || ""), filters: normFilters(filters), sort: SORT_IDS.has(sort) ? sort : "relevance" });
    return (Number.isFinite(limit) && limit > 0 ? res.slice(0, limit) : res).map((r) => summary(r.rec));
  },
  /** Facet values and counts. With no argument: over the whole catalogue. With
   *  { query, filters }: counted the way the sidebar counts them. */
  facets({ query: q = "", filters = {} } = {}) {
    const v = { q: String(q || ""), filters: normFilters(filters), sort: "relevance" };
    const out = {};
    for (const g of GROUPS) {
      out[g.id] = { label: g.label, values: countsFor(g.id, v).map(([value, count]) => ({ value, count })) };
    }
    return out;
  },
  applyView({ query: q, filters, sort } = {}) {
    if (q !== undefined) { query = String(q || ""); qEl.value = query; search.syncClear(); }
    if (filters !== undefined) { selected.clear(); for (const [k, v] of normFilters(filters)) selected.set(k, v); }
    if (sort !== undefined) sortBy = SORT_IDS.has(sort) ? sort : "relevance";
    writeURL(true);
    renderNow();
    return { count: lastShown.length };
  },
  async card(key) {
    const rec = byKey.get(key);
    if (!rec) return null;
    if (rec.state === "pending") enqueueRead(rec, true);
    await Promise.race([rec.ready, new Promise((res) => setTimeout(res, READ_TIMEOUT_MS + 2000))]);
    return rec.rawCard || null;
  },
  open(key) { location.href = pageUrl(key); },
};
