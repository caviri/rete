// app.js — the explorer's window: the home (your files, your catalogs), and an
// open archive's view tabs, lazy tree, preview, graph and extract.
//
// All graph knowledge lives in rete-fs.js; all engine access goes through
// fs-worker.js (or the desktop app's commands). The graph view is the rete
// extension's own page, hosted by graph-host.js. This file is the chrome: it
// draws, opens files into panes, and keeps the traffic meter honest.

import {
  VIEWS, VIEW_BY_ID, makeContext, openFile, extract, candidateLabelPredicates,
  readSelfDescription, parseHeader, cardFromBytes, rangeFetch, humanBytes, humanCount, localName,
  formatLabel, seedQuery, runSparql, resultToFile, searchGraph,
} from "./rete-fs.js";
import {
  recentFiles, rememberFile, forgetFile, subscriptions, subscribe, unsubscribe, refresh,
  cardSummary, cachedCard, cacheCard, pictureInfo, hostOf, webUrl,
} from "./library.js";
import { proceduralSVG } from "./procgen.js";
import { isTauri, makeTauriWorkerShim, pickReteFile } from "./tauri-bridge.js";
import { initTicker } from "./ticker.js";

const $ = (sel) => document.querySelector(sel);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
};

// -------------------------------------------------------------------- state

const state = {
  worker: null,
  ctx: null,
  meta: null,
  view: "types",
  layout: "tree", // "tree" = nested, "icons" = one folder as tiles, "graph" = drawn
  lastLayout: "tree", // where a view tab returns to from the graph
  path: [],       // icons mode: the folder chain from the view root
  expanded: new Set(),
  selected: null,
  reqId: 0,
  pending: new Map(),
  traffic: { bytes: 0, requests: 0 },
  opening: false,
  thumbQueue: [],
  thumbActive: 0,
  results: null,   // search results currently replacing the tree, or null
  lastQuery: null, // survives switching views, so a draft is never lost
  graph: null,     // the GraphHost of the open archive, made on first use
  seed: null,      // what the graph last showed
  home: { catalog: "all", filter: "", shown: 120 },
};

// ------------------------------------------------------------------- engine

function bootWorker() {
  if (state.worker) state.worker.terminate();
  // The one line that differs between the two builds. In a browser this is a
  // Web Worker holding a wasm RemoteGraph; in the desktop app it is a shim over
  // Rust commands driving rete-core natively. Both speak the same message
  // protocol, so everything below here is identical.
  const w = isTauri() ? makeTauriWorkerShim() : new Worker("./js/fs-worker.js");
  w.onmessage = (e) => {
    const m = e.data || {};
    if (m.type === "progress") {
      state.traffic = { bytes: m.bytes, requests: m.requests };
      paintTraffic();
      return;
    }
    if (m.stats) {
      state.traffic = { bytes: m.stats.bytes, requests: m.stats.requests };
      paintTraffic();
    }
    const p = state.pending.get(m.reqId);
    if (!p) return;
    state.pending.delete(m.reqId);
    m.ok ? p.resolve(m) : p.reject(new Error(m.error || "worker error"));
  };
  w.onerror = (e) => {
    for (const [, p] of state.pending) p.reject(new Error(e.message || "worker crashed"));
    state.pending.clear();
  };
  state.worker = w;
  return w;
}

function send(msg) {
  const reqId = ++state.reqId;
  return new Promise((resolve, reject) => {
    state.pending.set(reqId, { resolve, reject });
    state.worker.postMessage({ ...msg, reqId }, msg.transfer || []);
  });
}

const engine = {
  async query(sparql, format = "table") {
    const r = await send({ type: "query", sparql, format });
    return r.json;
  },
  async prefix(prefix, limit) {
    const r = await send({ type: "prefix", prefix, limit });
    return r.results;
  },
  async text(words, limit) {
    const r = await send({ type: "text", words, limit });
    return r.results;
  },
};

// --------------------------------------------------------------------- open

const baseName = (p) => decodeURIComponent(String(p).split(/[?#]/)[0].split(/[\\/]/).pop() || String(p));
const shortName = (name) => String(name).replace(/\.rete$/i, "");

/**
 * Desktop open: one path for both local files and URLs, because the native side
 * reads them through the same `RangeReader`. It hands back the raw 1 KB header
 * so `parseHeader` — the browser's own — stays the single implementation.
 */
async function openNative(source, name) {
  if (state.opening) return null;
  state.opening = true;
  setStatus(`opening ${name}…`);
  resetPanes();
  try {
    bootWorker();
    const r = await send({ type: "open", source });
    let header = parseHeader(new Uint8Array(r.head));
    // A URL's section table is one more small read; a local file's stays as
    // its header's regions (the native side hands over the first 1 KB only).
    if (/^https?:/i.test(source) && !header.sectionsComplete) {
      header = (await readSelfDescription(source).catch(() => null))?.header ?? header;
    }
    let card = null;
    try { card = r.cardText ? JSON.parse(r.cardText) : null; } catch (_) { /* not fatal */ }
    const meta = {
      source: shortName(name), url: /^https?:/i.test(source) ? source : null, path: /^https?:/i.test(source) ? null : source,
      size: r.size, header, card, schema: r.schema, schemaError: r.schemaError,
    };
    finishOpen(meta);
    return { statements: header.quadCount };
  } catch (err) {
    setStatus(`could not open ${name}: ${err.message}`, true);
    return null;
  } finally {
    state.opening = false;
  }
}

async function openRemote(entry) {
  if (isTauri()) return openNative(entry.url, entry.name || baseName(entry.url));
  if (state.opening) return null;
  state.opening = true;
  const name = shortName(entry.name || baseName(entry.url));
  setStatus(`opening ${name}…`);
  resetPanes();
  try {
    // Three small range requests learn what the archive is, whatever it weighs.
    const desc = await readSelfDescription(entry.url);
    bootWorker();
    const opened = await send({ type: "open", mode: "remote", url: entry.url });
    finishOpen({
      source: name, url: entry.url, size: desc.size || entry.size || 0,
      header: desc.header, card: desc.card,
      schema: opened.schema, schemaError: opened.schemaError,
    });
    return { statements: desc.header.quadCount };
  } catch (err) {
    setStatus(`could not open ${name}: ${err.message}`, true);
    return null;
  } finally {
    state.opening = false;
  }
}

async function openLocal(file) {
  if (state.opening) return null;
  state.opening = true;
  setStatus(`reading ${file.name}…`);
  resetPanes();
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const header = parseHeader(bytes);
    const card = cardFromBytes(bytes, header);
    bootWorker();
    const copy = bytes.slice();
    const opened = await send({ type: "open", mode: "local", bytes: copy.buffer, transfer: [copy.buffer] });
    finishOpen({
      source: shortName(file.name), fileName: file.name, url: null, size: bytes.byteLength,
      header, card, schema: opened.schema, schemaError: opened.schemaError,
    });
    return { statements: header.quadCount };
  } catch (err) {
    setStatus(`could not open ${file.name}: ${err.message}`, true);
    return null;
  } finally {
    state.opening = false;
  }
}

/** A link from the omnibox: a .rete opens, a catalog is subscribed to. */
async function openLink(raw) {
  const text = raw.trim();
  if (!text) return;
  if (isTauri() && !/^https?:/i.test(text)) return openNative(text, baseName(text));
  const url = webUrl(text);
  if (!url) return setStatus("that is not a link — paste an http(s) address of a .rete file or a catalog", true);
  if (/\.rete([?#]|$)/i.test(url)) return openRemote({ url, name: baseName(url) });
  // Not obviously either: the first bytes say. A .rete starts with RETE.
  let head = null;
  try { head = await rangeFetch(url, 0, 3); } catch (_) { /* let the catalog fetch say why */ }
  if (head && new TextDecoder().decode(head) === "RETE") return openRemote({ url, name: baseName(url) });
  await addCatalog(url);
}

function finishOpen(meta) {
  state.meta = meta;
  state.ctx = makeContext({ engine, meta });
  state.expanded = new Set();
  state.selected = null;
  dropGraph();

  const h = meta.header;
  $("#archive-name").textContent = meta.source;
  $("#archive-name").title = meta.card?.title || meta.url || meta.path || meta.source;
  $("#archive-facts").textContent = [
    humanBytes(meta.size),
    `${humanCount(h.quadCount)} statements`,
    `${humanCount(h.termCount)} terms`,
    `format ${formatLabel(h.version)}`,
    meta.url ? hostOf(meta.url) : meta.path ? "this computer" : "local file",
  ].join(" · ");

  // Remembered on the home, with what its picture needs.
  const summary = cardSummary(meta.card, h);
  if (meta.url) cacheCard(meta.url, summary);
  rememberFile({
    name: meta.source,
    title: meta.card?.title ?? null,
    url: meta.url,
    path: meta.path ?? null,
    local: !meta.url && !meta.path,
    fileName: meta.fileName ?? null,
    size: meta.size,
    statements: h.quadCount,
    format: formatLabel(h.version),
    summary: meta.url ? null : summary,
  });
  const thumb = $("#archive-thumb");
  thumb.innerHTML = proceduralSVG(pictureInfo(summary, { key: meta.source }), { theme: "dark", labels: false });

  $("#window").hidden = false;
  $("#empty").hidden = true;
  paintTraffic();
  paintLabelPicker();

  if (meta.schemaError) {
    setStatus(`opened — no baked schema (${meta.schemaError}); Types and Predicates will be empty`, true);
  } else {
    setStatus("opened");
  }
  if (state.layout === "graph") setLayout(state.lastLayout);
  selectView(h.hasQuads ? state.view : state.view === "graphs" ? "types" : state.view);
}

function resetPanes() {
  // A new archive starts a new traffic budget, and the old file's size must not
  // survive as the denominator — that reads as a nonsense percentage.
  state.meta = null;
  state.traffic = { bytes: 0, requests: 0 };
  paintTraffic();
  $("#tree").innerHTML = "";
  $("#preview").innerHTML = "";
  $("#preview-title").textContent = "";
  $("#preview-sub").textContent = "";
  $("#downloads").innerHTML = "";
  state.selected = null;
  paintExtractBar({});
}

// --------------------------------------------------------------------- home

/** A picture for a file: drawn from its card's classes and links (js/procgen.js). */
function picture(summary, entry, labels = true) {
  return proceduralSVG(pictureInfo(summary, entry), { theme: "dark", labels });
}

function paintHome() {
  paintFiles();
  paintCatalogs();
}

function paintFiles() {
  const box = $("#files");
  box.innerHTML = "";
  const files = recentFiles();
  $("#files-note").textContent = files.length ? `${files.length} opened here, most recent first` : "";
  if (!files.length) {
    box.append(el("div", "empty-note", "Files you open — from a catalog, a link or this computer — show up here."));
    return;
  }
  for (const f of files) {
    const summary = f.url ? cachedCard(f.url) : f.summary;
    const card = makeCard({
      title: f.title || f.name,
      blurb: f.title && f.title !== f.name ? f.name : null,
      meta: [
        f.size ? humanBytes(f.size) : null,
        f.statements != null ? `${humanCount(f.statements)} statements` : null,
        f.url ? hostOf(f.url) : f.path ? "this computer" : "local file",
        when(f.openedAt),
      ],
      kind: f.url ? "remote" : "local",
      svg: picture(summary, { key: f.name, url: f.url }),
    });
    const x = el("button", "card-x", "×");
    x.title = "Remove from your files";
    x.onclick = (e) => {
      e.stopPropagation();
      forgetFile(f.id);
      paintFiles();
    };
    card.append(x);
    card.onclick = () => reopen(f);
    box.append(card);
  }
}

/** Open a remembered file again: a link or a desktop path directly, a browser's local file by asking for it. */
async function reopen(f) {
  if (f.url) return openRemote({ url: f.url, name: f.name });
  if (f.path && isTauri()) return openNative(f.path, f.name);
  setStatus(`choose ${f.fileName || f.name} again — a browser cannot reopen a local file by itself`);
  $("#file").click();
}

function paintCatalogs() {
  const subs = subscriptions();
  const chips = $("#cat-chips");
  chips.innerHTML = "";
  if (state.home.catalog !== "all" && !subs.some((s) => s.url === state.home.catalog)) state.home.catalog = "all";
  if (subs.length > 1) {
    const all = el("button", "chip all" + (state.home.catalog === "all" ? " on" : ""));
    all.append(document.createTextNode("All"), el("span", "n", String(subs.reduce((n, s) => n + s.datasets.length, 0))));
    all.onclick = () => ((state.home.catalog = "all"), (state.home.shown = 120), paintCatalogs());
    chips.append(all);
  }
  for (const sub of subs) {
    const on = state.home.catalog === sub.url || (subs.length === 1 && state.home.catalog === "all");
    const chip = el("span", "chip" + (on ? " on" : ""));
    chip.setAttribute("role", "tab");
    chip.tabIndex = 0;
    chip.append(document.createTextNode(sub.name), el("span", "n", sub.fetchedAt ? String(sub.datasets.length) : "…"));
    const again = el("button", "chip-act", "↻");
    again.title = "Fetch this catalog again";
    again.onclick = async (e) => {
      e.stopPropagation();
      again.disabled = true;
      await refresh(sub.url);
      paintCatalogs();
    };
    const off = el("button", "chip-act", "×");
    off.title = "Unsubscribe";
    off.onclick = (e) => {
      e.stopPropagation();
      unsubscribe(sub.url);
      setStatus(`unsubscribed from ${sub.name}`);
      paintCatalogs();
    };
    chip.append(again, off);
    chip.onclick = () => ((state.home.catalog = sub.url), (state.home.shown = 120), paintCatalogs());
    chips.append(chip);
  }

  const picked = state.home.catalog === "all" ? subs : subs.filter((s) => s.url === state.home.catalog);
  const about = $("#cat-about");
  about.innerHTML = "";
  if (!subs.length) {
    about.textContent = "No catalogs. Paste the link of one above — any JSON list of .rete links will do.";
  } else if (picked.length === 1) {
    const s = picked[0];
    about.append(document.createTextNode(`${s.description ? `${s.description} · ` : ""}from `));
    const a = el("a", null, hostOf(s.url));
    a.href = s.url;
    a.target = "_blank";
    a.rel = "noopener";
    about.append(a);
    if (s.error) about.append(el("span", "bad", ` · last fetch failed: ${s.error}`));
  }

  // One list across the catalogs picked: the same file listed twice shows once.
  const seen = new Set();
  const rows = [];
  for (const s of picked) {
    for (const d of s.datasets) {
      if (seen.has(d.url)) continue;
      seen.add(d.url);
      rows.push({ d, from: s.name });
    }
  }
  const q = state.home.filter.trim().toLowerCase();
  const shown = q
    ? rows.filter(({ d }) => [d.name, d.key, d.blurb, d.license, ...(d.tags ?? []), cachedCard(d.url)?.title].some((v) => v && String(v).toLowerCase().includes(q)))
    : rows;

  const box = $("#catalog");
  box.innerHTML = "";
  if (!shown.length) {
    box.append(el("div", "empty-note", q ? `Nothing in ${picked.length > 1 ? "these catalogs" : "this catalog"} matches “${q}”.` : subs.some((s) => !s.fetchedAt) ? "Fetching the catalog…" : "This catalog lists no .rete files."));
    return;
  }
  for (const { d, from } of shown.slice(0, state.home.shown)) box.append(catalogCard(d, picked.length > 1 ? from : null));
  if (shown.length > state.home.shown) {
    const more = el("button", "btn empty-note", `Show ${Math.min(120, shown.length - state.home.shown)} more of ${shown.length - state.home.shown}`);
    more.onclick = () => ((state.home.shown += 120), paintCatalogs());
    box.append(more);
  }
}

function catalogCard(d, from) {
  const summary = cachedCard(d.url);
  const card = makeCard({
    title: summary?.title || d.name,
    blurb: d.blurb,
    meta: [d.sizeText, d.triples ? `${d.triples} triples` : null, d.license, from],
    sizeFirst: true,
    icon: d.icon,
    svg: d.thumbnail ? null : picture(summary, d),
    img: d.thumbnail,
  });
  card.title = d.url;
  card.onclick = () => openRemote({ url: d.url, name: d.key || d.name });
  if (!summary && !d.thumbnail) watchCard(card, d);
  return card;
}

function makeCard({ title, blurb, meta, kind, svg, img, icon, sizeFirst }) {
  const card = el("article", "card");
  card.tabIndex = 0;
  card.setAttribute("role", "button");
  card.addEventListener("keydown", (e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), card.click()));
  const thumb = el("div", "thumb");
  if (img) {
    const i = new Image();
    i.alt = "";
    i.loading = "lazy";
    i.src = img;
    thumb.append(i);
  } else thumb.innerHTML = svg || "";
  if (icon) thumb.append(el("span", "icon-badge", icon));
  if (kind) thumb.append(el("span", "kind", kind));
  const body = el("div", "card-body");
  body.append(el("h3", "card-title", title));
  if (blurb) body.append(el("p", "card-blurb", blurb));
  const m = el("div", "card-meta");
  meta.filter(Boolean).forEach((t, i) => m.append(el("span", i === 0 && sizeFirst ? "sz" : null, t)));
  body.append(m);
  body.querySelector(".card-title").title = title;
  card.append(thumb, body);
  return card;
}

// Pictures for catalog entries: each needs the file's card (two small range
// requests), so they are read only for cards that scroll into view, a few at a
// time, and kept — a published file's name changes with its contents.
const cardQueue = [];
let cardActive = 0;
const CARD_CONCURRENCY = 4;
const cardObserver = "IntersectionObserver" in window
  ? new IntersectionObserver((entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        cardObserver.unobserve(e.target);
        const job = e.target.__cardJob;
        if (job) cardQueue.push(job), pumpCards();
      }
    }, { rootMargin: "200px" })
  : null;

function watchCard(card, d) {
  card.__cardJob = { card, d };
  if (cardObserver) cardObserver.observe(card);
}

function pumpCards() {
  while (cardActive < CARD_CONCURRENCY && cardQueue.length) {
    const { card, d } = cardQueue.shift();
    if (cachedCard(d.url)) continue;
    cardActive++;
    readSelfDescription(d.url, { sections: false, picture: true })
      .then(({ header, card: c }) => {
        const summary = cardSummary(c, header);
        cacheCard(d.url, summary);
        if (!card.isConnected) return;
        card.querySelector(".thumb").firstChild?.remove();
        card.querySelector(".thumb").insertAdjacentHTML("afterbegin", picture(summary, d));
        if (summary.title) {
          const t = card.querySelector(".card-title");
          t.textContent = summary.title;
          t.title = summary.title;
        }
      })
      .catch(() => cacheCard(d.url, { none: true, failed: true }))
      .finally(() => {
        cardActive--;
        pumpCards();
      });
  }
}

async function addCatalog(url) {
  setStatus(`reading the catalog at ${hostOf(url)}…`);
  try {
    const sub = await subscribe(url);
    state.home.catalog = sub.url;
    state.home.shown = 120;
    setStatus(`subscribed to ${sub.name}: ${sub.datasets.length} files`);
    goHome();
  } catch (err) {
    setStatus(`could not subscribe: ${err.message}`, true);
  }
}

/** Catalogs not fetched for a while are fetched again, in the background. */
async function freshenCatalogs() {
  const stale = subscriptions().filter((s) => !s.fetchedAt || Date.now() - s.fetchedAt > 6 * 3600_000);
  for (const s of stale) {
    await refresh(s.url);
    if (!$("#empty").hidden) paintCatalogs();
  }
}

const when = (t) => {
  if (!t) return null;
  const s = (Date.now() - t) / 1000;
  if (s < 90) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 86400 * 30) return `${Math.round(s / 86400)} d ago`;
  return new Date(t).toISOString().slice(0, 10);
};

// -------------------------------------------------------------------- views

function paintViewTabs() {
  const bar = $("#views");
  bar.innerHTML = "";
  for (const v of VIEWS) {
    const b = el("button", "viewtab" + (v.id === state.view && state.layout !== "graph" ? " on" : ""));
    b.append(el("span", "vi", v.icon), document.createTextNode(v.label));
    b.title = v.hint;
    b.onclick = () => selectView(v.id);
    bar.append(b);
  }
  $("#view-hint").textContent = state.layout === "graph"
    ? "Double-click a node to bring in its links; right-click for more."
    : VIEW_BY_ID.get(state.view).hint;
}

async function selectView(id) {
  state.view = id;
  state.expanded = new Set();
  state.path = [];
  state.results = null; // a view change is a fresh start, not a filtered one
  if (state.layout === "graph") setLayout(state.lastLayout);
  paintViewTabs();
  await renderPane();
}

/** Draw whichever layout is active, from scratch. */
async function renderPane() {
  if (state.layout === "graph") return;
  const pane = $("#tree");
  // Abandon thumbnails queued for the folder we are leaving; in-flight ones
  // finish harmlessly against detached nodes.
  state.thumbQueue = [];
  pane.innerHTML = "";

  const view = VIEW_BY_ID.get(state.view);
  if (view && view.custom === "sparql") {
    $("#crumbs").hidden = true;
    pane.className = "tree";
    renderQueryPanel(pane);
    return;
  }
  if (state.results) {
    $("#crumbs").hidden = true;
    pane.className = "tree";
    renderSearchResults(pane);
    return;
  }
  if (state.layout === "icons") {
    $("#crumbs").hidden = false;
    paintCrumbs();
    pane.className = "tree grid";
    await renderGrid(pane, state.path[state.path.length - 1] || null);
  } else {
    $("#crumbs").hidden = true;
    pane.className = "tree";
    const host = el("div", "level");
    pane.append(host);
    await renderLevel(host, null, 0);
  }
}

function setLayout(mode) {
  if (mode !== "graph") state.lastLayout = mode;
  state.layout = mode;
  $("#layout").querySelectorAll("button").forEach((x) => x.classList.toggle("on", x.dataset.mode === mode));
  const graph = mode === "graph";
  $("#panes").hidden = graph;
  $("#stage").hidden = !graph;
  $("#extract").hidden = graph;
  paintViewTabs();
}

// -------------------------------------------------------------------- graph

/**
 * What the graph shows for a selection: the entity, a sample of the class, of
 * the predicate, of the named graph — or, with nothing selected, the schema
 * (a sample when the file has none).
 */
function seedFor(item) {
  if (item?.iri && (item.resource || item.kind === "file") && !item.special) return { iri: item.iri, label: item.label || item.name };
  if (item?.iri && item.kind === "dir" && item.view === "types") return { cls: item.iri, label: item.label || item.name };
  if (item?.iri && item.kind === "dir" && item.view === "predicates") return { predicate: item.iri, label: item.name };
  if (item?.iri && item.kind === "dir" && item.view === "graphs") return { graph: item.iri, label: item.name };
  return state.meta?.schema?.classes?.length ? { schema: true } : { sample: true };
}

const sameSeed = (a, b) => !!a && !!b && JSON.stringify(a) === JSON.stringify(b);

async function showGraph(seed) {
  if (!state.meta) return;
  setLayout("graph");
  if (!state.graph) {
    const { GraphHost } = await import("./graph-host.js");
    state.graph = new GraphHost({
      frame: $("#graph-frame"),
      ctx: state.ctx,
      dataset: state.meta.source,
      onOpenFile: (file) => openLocal(file),
      onStatus: (t) => setStatus(t),
    });
  }
  const host = state.graph;
  state.seed = seed;
  paintSeeds(seed, "");
  try {
    const note = await host.show(seed);
    if (state.graph !== host) return; // another archive was opened meanwhile
    paintSeeds(seed, note);
    setStatus("graph ready");
  } catch (err) {
    if (state.graph !== host) return;
    paintSeeds(seed, "");
    setStatus(`graph: ${err.message}`, true);
  }
}

function paintSeeds(current, note) {
  const bar = $("#seeds");
  bar.innerHTML = "";
  bar.append(el("span", null, "Draw:"));
  const chip = (label, seed, title) => {
    const b = el("button", "seed" + (sameSeed(seed, current) ? " on" : ""), label);
    if (title) b.title = title;
    b.onclick = () => showGraph(seed);
    bar.append(b);
  };
  if (state.meta?.schema?.classes?.length) chip("◇ Schema", { schema: true }, "The classes and how they connect, as boxes listing their properties");
  chip("◌ Sample", { sample: true }, "A few hundred statements from the start of the file");
  const sel = seedFor(state.selected);
  if (!sel.schema && !sel.sample) {
    const what = sel.iri ? "◉" : sel.cls ? "▣ instances of" : sel.predicate ? "≡" : "⬒";
    chip(`${what} ${sel.label}`, sel, "The item selected in the tree");
  }
  if (current?.query) chip("⌘ query", current, "The last query shown as a graph");
  bar.append(el("span", "seed-note", note || ""));
}

function dropGraph() {
  state.graph?.destroy();
  state.graph = null;
  state.seed = null;
}

// ------------------------------------------------------------ sparql panel

function renderQueryPanel(host) {
  const wrap = el("div", "qpanel");

  const ta = document.createElement("textarea");
  ta.className = "qbox";
  ta.spellcheck = false;
  ta.value = state.lastQuery || seedQuery(state.ctx);
  ta.setAttribute("aria-label", "SPARQL query");

  const bar = el("div", "qbar");
  const run = el("button", "qrun", "▶ Run");
  const note = el("span", "qnote", "⌘/Ctrl + Enter");
  bar.append(run, note);

  wrap.append(ta, bar);
  host.append(wrap);

  const go = async () => {
    const q = ta.value.trim();
    if (!q) return;
    state.lastQuery = q;
    run.disabled = true;
    run.textContent = "running…";
    const started = performance.now();
    try {
      const result = await runSparql(state.ctx, q);
      showQueryResult(result, Math.round(performance.now() - started), q);
      setStatus("query ok");
    } catch (err) {
      // A SPARQL error is the normal case while writing one — show it where the
      // results go, not as a transient status line that scrolls away.
      $("#preview-title").textContent = "Query error";
      $("#preview-sub").textContent = "";
      $("#downloads").innerHTML = "";
      $("#preview").innerHTML = "";
      $("#preview").append(el("div", "err", err.message));
      setStatus("query failed", true);
    } finally {
      run.disabled = false;
      run.textContent = "▶ Run";
    }
  };

  run.onclick = go;
  ta.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter") { e.preventDefault(); go(); }
  });
  ta.focus();
}

function showQueryResult(result, ms, query) {
  const pane = $("#preview");
  pane.innerHTML = "";
  $("#downloads").innerHTML = "";

  if (result.kind === "text") {
    $("#preview-title").textContent = "Result";
    $("#preview-sub").textContent = `${result.format} · ${ms} ms`;
    pane.append(renderPre(result.text));
  } else {
    $("#preview-title").textContent = `${humanCount(result.rows.length)} row${result.rows.length === 1 ? "" : "s"}`;
    $("#preview-sub").textContent = `${result.vars.join(", ")} · ${ms} ms`;
    pane.append(renderTable(result));
  }

  // SELECT and CONSTRUCT results open in the view (the page the Claude extension shows): as a graph, a table with
  // rich cells and a chart of each column, cards, or a map when they hold geometries, positions or .pmtiles layers.
  // A lone ASK boolean does not.
  if (!/^\s*(?:#[^\n]*\n\s*)*(?:(?:PREFIX|BASE)[^\n]*\n\s*)*ASK\b/i.test(query)) {
    const hinted = (/^\s*#\s*view\s*:\s*(graph|table|cards|map)\s*$/im.exec(query) || [])[1];
    const tabs = [["graph", "◉ Graph", "Draw these results as a graph"], ["table", "▦ Table", "These results as a table: pictures, maps and links in the cells, a chart of each column"], ["cards", "▤ Cards", "These results as cards, one per row"]];
    if (hinted === "map" || hasGeo(result)) tabs.push(["map", "⌖ Map", "These results on a map (OpenStreetMap basemap)"]);
    for (const [view, label, title] of tabs) {
      const b = el("button", "dl graphit" + (view === hinted ? " hinted" : ""), label);
      b.title = title + (view === hinted ? " — the view this query names (# view:)" : "");
      b.onclick = () => showGraph({ query, title: "query results", view });
      $("#downloads").append(b);
    }
  }
  for (const fmt of result.kind === "text" ? ["ttl"] : ["csv", "json"]) {
    const f = resultToFile(result, fmt);
    const b = el("button", "dl", `↓ ${f.ext.toUpperCase()}`);
    b.onclick = () => download(`query.${f.ext}`, f.mime, f.body);
    $("#downloads").append(b);
  }
}

// ------------------------------------------------------------------ search

function renderSearchResults(host) {
  const { term, hits, via, labels } = state.results;
  const head = el("div", "searchhead");
  head.append(el("span", null, `${humanCount(hits.length)} match${hits.length === 1 ? "" : "es"} for “${term}”`));
  const clear = el("button", "more", "clear");
  clear.onclick = async () => {
    state.results = null;
    $("#search").value = "";
    await renderPane();
  };
  head.append(clear);
  host.append(head);

  if (via) host.append(el("div", "note", `via ${via} index`));
  if (!hits.length) {
    host.append(el("div", "note", "Nothing matched. Full-text needs a file built with --text-index; otherwise only label prefixes are searchable."));
    return;
  }

  const byIri = new Map((labels || []).map((h) => [h.subject, h.label]));
  const level = el("div", "level");
  host.append(level);
  const items = hits.map((iri) => ({
    view: state.view,
    id: `res:${iri}`,
    name: localName(iri),
    label: byIri.get(iri) || null,
    kind: "dir",
    resource: true,
    iri,
    trail: [],
  }));
  for (const item of items) level.append(rowFor(item, 0));
}

async function doSearch(term) {
  if (!state.ctx) return;
  const q = term.trim();
  if (state.layout === "graph") setLayout(state.lastLayout);
  if (!q) {
    state.results = null;
    await renderPane();
    return;
  }
  setStatus(`searching “${q}”…`);
  try {
    const { hits, via, labels } = await searchGraph(state.ctx, q);
    state.results = { term: q, hits, via, labels };
    await renderPane();
    setStatus(hits.length ? `${humanCount(hits.length)} matches` : "no matches");
  } catch (err) {
    setStatus(`search failed: ${err.message}`, true);
  }
}

// ------------------------------------------------------------- icons layout

function paintCrumbs() {
  const bar = $("#crumbs");
  bar.innerHTML = "";
  const mk = (label, depth) => {
    const b = el("button", "crumb", label);
    b.onclick = async () => {
      state.path = state.path.slice(0, depth);
      await renderPane();
    };
    return b;
  };
  bar.append(mk(VIEW_BY_ID.get(state.view).label, 0));
  state.path.forEach((n, i) => {
    bar.append(el("span", "sep", "›"));
    bar.append(mk(n.label || n.name, i + 1));
  });
  if (state.path.length) {
    const up = el("button", "crumb up", "↑ up");
    up.onclick = async () => {
      state.path.pop();
      await renderPane();
    };
    bar.append(up);
  }
}

async function renderGrid(host, node) {
  const loading = el("div", "loading", "reading…");
  host.append(loading);
  let res;
  try {
    res = await VIEW_BY_ID.get(state.view).list(state.ctx, node);
  } catch (err) {
    loading.className = "err";
    loading.textContent = err.message;
    return;
  }
  loading.remove();

  if (res.note) host.append(el("div", "note gridnote", res.note));

  const grid = el("div", "tiles");
  host.append(grid);
  const painted = res.items.map((item) => {
    const tile = tileFor(item);
    grid.append(tile);
    return { item, tile };
  });

  if (res.more) {
    const more = el("button", "more", "load next page…");
    more.onclick = async () => {
      more.remove();
      await renderGrid(host, { ...node, offset: res.nextOffset });
    };
    host.append(more);
  }

  if (res.decorate) {
    const generation = `${state.view}|${state.path.length}|${state.layout}`;
    res.decorate((deco) => {
      if (`${state.view}|${state.path.length}|${state.layout}` !== generation) return;
      for (const { item, tile } of painted) {
        const d = item.iri && deco.get(item.iri);
        if (!d) continue;
        if (d.label && !item.label) {
          item.label = d.label;
          const cap = tile.querySelector(".tcap");
          if (cap) { cap.textContent = d.label; cap.title = item.name; }
        }
        if (d.image && !item.image) {
          item.image = d.image;
          const art = tile.querySelector(".tart");
          if (art) mountThumb(art, d.image);
        }
      }
    });
  }
}

/**
 * Attach a thumbnail, fetched by a small background queue in DOM order.
 *
 * Two tidier-looking approaches both failed here. `loading="lazy"` never fires
 * for an element that is not yet in the document, and an IntersectionObserver
 * rooted on the `overflow:auto` pane proved unreliable — it reports nothing in a
 * short viewport, leaving every tile blank. A bounded FIFO has no geometry
 * dependence at all: top tiles fill first, at most THUMB_CONCURRENCY requests
 * are in flight, and navigating away drops the rest.
 */
const THUMB_CONCURRENCY = 6;

function mountThumb(art, url) {
  const img = new Image();
  img.alt = "";
  art.append(img);
  state.thumbQueue.push({ img, url });
  pumpThumbs();
}

function pumpThumbs() {
  while (state.thumbActive < THUMB_CONCURRENCY && state.thumbQueue.length) {
    const { img, url } = state.thumbQueue.shift();
    state.thumbActive++;
    const done = () => { state.thumbActive--; pumpThumbs(); };
    img.onload = () => { if (img.parentElement) img.parentElement.classList.add("has"); done(); };
    img.onerror = () => { img.remove(); done(); };
    img.src = url;
  }
}

function tileFor(item) {
  const tile = el("button", "tile" + (item.kind === "dir" ? " dir" : " file"));
  const art = el("div", "tart");
  art.append(el("span", "tglyph", item.kind === "dir" ? "▣" : fileIcon(item)));
  const cap = el("span", "tcap", item.label || item.name);
  if (item.label) cap.title = item.name;
  const sub = el("span", "tsub", item.detail || "");
  tile.append(art, cap, sub);

  tile.onclick = async () => {
    state.selected = item;
    paintExtractBar(item);
    // A resource is both: it opens as a file *and* it can be walked into.
    if (item.iri || item.special) {
      showFile(item).catch(() => {});
    }
    if (item.kind === "dir") {
      state.path.push(item);
      await renderPane();
    }
  };
  return tile;
}

// --------------------------------------------------------------------- tree

async function renderLevel(host, node, depth) {
  const loading = el("div", "loading", "reading…");
  loading.style.paddingLeft = `${8 + depth * 14}px`;
  host.append(loading);
  let res;
  try {
    res = await VIEW_BY_ID.get(state.view).list(state.ctx, node);
  } catch (err) {
    loading.className = "err";
    loading.textContent = err.message;
    return;
  }
  loading.remove();

  if (res.note) {
    const n = el("div", "note", res.note);
    n.style.paddingLeft = `${8 + depth * 14}px`;
    host.append(n);
  }
  const painted = res.items.map((item) => {
    const wrap = rowFor(item, depth);
    host.append(wrap);
    return { item, wrap };
  });

  // Names land as local names first, then improve when labels arrive. Nothing
  // waits on this, and a view change abandons it.
  if (res.decorate) {
    const generation = state.view;
    res.decorate((deco) => {
      if (state.view !== generation) return;
      for (const { item, wrap } of painted) {
        const d = item.iri && deco.get(item.iri);
        if (!d) continue;
        if (d.label && !item.label) {
          item.label = d.label;
          const nameEl = wrap.querySelector(":scope > .row > .name");
          if (nameEl) {
            nameEl.textContent = d.label;
            nameEl.title = item.name;
          }
        }
        if (d.image && !item.image) item.image = d.image;
      }
    });
  }

  if (res.more) {
    const more = el("button", "more", `load next ${humanCount(200)}…`);
    more.style.marginLeft = `${8 + depth * 14}px`;
    more.onclick = async () => {
      more.disabled = true;
      more.textContent = "reading…";
      const next = { ...node, offset: res.nextOffset };
      const sub = el("div", "level");
      more.replaceWith(sub);
      await renderLevel(sub, next, depth);
    };
    host.append(more);
  }
}

function rowFor(item, depth) {
  const wrap = el("div", "node");
  const row = el("div", "row" + (item.kind === "dir" ? " dir" : " file"));
  row.style.paddingLeft = `${8 + depth * 14}px`;

  const twisty = el("span", "twisty", item.kind === "dir" ? "▸" : "");
  const icon = el("span", "icon", item.kind === "dir" ? "▣" : fileIcon(item));
  const name = el("span", "name", item.label ? item.label : item.name);
  if (item.label) name.title = item.name;
  const detail = el("span", "detail", item.detail || "");

  row.append(twisty, icon, name, detail);
  wrap.append(row);

  const kids = el("div", "level");
  kids.hidden = true;
  wrap.append(kids);

  const toggle = async () => {
    if (item.kind !== "dir") return;
    if (!kids.hidden) {
      kids.hidden = true;
      twisty.textContent = "▸";
      return;
    }
    twisty.textContent = "▾";
    kids.hidden = false;
    if (!kids.dataset.loaded) {
      kids.dataset.loaded = "1";
      await renderLevel(kids, item, depth + 1);
    }
  };

  // The twisty owns expansion; the row owns selection and preview. A resource is
  // both a folder and a file, so clicking its name must not collapse it.
  twisty.onclick = (e) => {
    e.stopPropagation();
    toggle();
  };

  row.onclick = async () => {
    document.querySelectorAll(".row.sel").forEach((r) => r.classList.remove("sel"));
    row.classList.add("sel");
    state.selected = item;
    paintExtractBar(item);

    if (item.iri || item.special) {
      await showFile(item).catch(() => {});
      if (item.resource && kids.hidden && !kids.dataset.loaded) await toggle();
      return;
    }
    await toggle();
  };
  // Double-click draws it.
  row.ondblclick = (e) => {
    if (!item.iri || item.special) return;
    e.preventDefault();
    state.selected = item;
    showGraph(seedFor(item));
  };

  return wrap;
}

function fileIcon(item) {
  if (item.special === "section") return "▮";
  if (item.special === "card") return "≣";
  if (item.special === "shape") return "◇";
  if (item.special === "ptable") return "≡";
  if (item.backrefs) return "↩";
  return "○";
}

// ------------------------------------------------------------------ toolbar

function paintLabelPicker() {
  const sel = $("#labelpred");
  sel.innerHTML = "";
  const auto = el("option", null, "auto (label, prefLabel, name, title…)");
  auto.value = "";
  sel.append(auto);

  let candidates = [];
  try {
    candidates = candidateLabelPredicates(state.ctx);
  } catch (_) { /* no schema → no candidates, auto still works */ }

  for (const c of candidates.slice(0, 80)) {
    const o = el("option", null, `${c.name} — ${humanCount(c.count)}`);
    o.value = c.iri;
    o.title = c.iri;
    sel.append(o);
  }
  sel.disabled = false;
  sel.value = "";
}

/**
 * Back to the home. The open archive is deliberately left resident — the
 * worker keeps its handle and its warmed block cache, so re-opening the same
 * file costs nothing and the traffic meter keeps counting from where it was.
 */
function goHome() {
  $("#window").hidden = true;
  $("#empty").hidden = false;
  paintHome();
  window.scrollTo(0, 0);
  setStatus(state.meta ? `${state.meta.source} is still open — pick another, or reopen it` : "pick a file, drop a .rete, or paste a link");
}

function wireHome() {
  for (const id of ["home", "back"]) {
    const b = $(`#${id}`);
    if (b) b.addEventListener("click", goHome);
  }
  $("#cat-filter").addEventListener("input", (e) => {
    state.home.filter = e.target.value;
    state.home.shown = 120;
    paintCatalogs();
  });
  $("#cat-add").addEventListener("submit", (e) => {
    e.preventDefault();
    const url = $("#cat-url").value.trim();
    if (!url) return;
    addCatalog(url).then(() => ($("#cat-url").value = ""));
  });
}

function wireToolbar() {
  const search = $("#search");
  if (search) {
    search.addEventListener("keydown", (e) => {
      if (e.key === "Enter") doSearch(search.value);
      if (e.key === "Escape") { search.value = ""; doSearch(""); }
    });
    // A cleared <input type=search> (the ✕) should restore the tree.
    search.addEventListener("search", () => { if (!search.value.trim()) doSearch(""); });
  }

  $("#labelpred").addEventListener("change", async (e) => {
    if (!state.ctx) return;
    const pick = e.target.value;
    state.ctx.labelPredicates = pick ? [pick] : DEFAULT_LABEL_PREDICATES.slice();
    state.ctx.forgetLabels();
    await renderPane();
  });

  $("#layout").addEventListener("click", async (e) => {
    const b = e.target.closest("button[data-mode]");
    if (!b || b.dataset.mode === state.layout) return;
    if (b.dataset.mode === "graph") {
      if (!state.ctx) return;
      // The graph keeps what it shows until something else is asked for.
      const seed = seedFor(state.selected);
      if (state.graph && state.seed && (sameSeed(seed, state.seed) || seed.schema || seed.sample)) {
        setLayout("graph");
        paintSeeds(state.seed, "");
        return;
      }
      return showGraph(seed);
    }
    setLayout(b.dataset.mode);
    state.path = [];
    if (state.ctx) await renderPane();
  });
}

// The picker's "auto" setting restores whatever rete-fs shipped as the default,
// captured once from a throwaway context so the list lives in one place.
const DEFAULT_LABEL_PREDICATES = makeContext({ engine: null, meta: null }).labelPredicates;

// ------------------------------------------------------------------ preview

async function showFile(item) {
  const pane = $("#preview");
  pane.innerHTML = "";
  $("#preview-title").textContent = item.name;
  $("#preview-sub").textContent = "reading…";
  $("#downloads").innerHTML = "";

  let file;
  try {
    file = await openFile(state.ctx, item);
  } catch (err) {
    $("#preview-sub").textContent = "";
    pane.append(el("div", "err", err.message));
    return;
  }

  $("#preview-title").textContent = file.title;
  $("#preview-sub").textContent = file.subtitle || "";

  // Anything with an IRI draws: an entity with its links, a class or a
  // predicate as a sample.
  if (item.iri && !item.special) {
    const g = el("button", "dl graphit", "◉ Graph");
    g.title = "Draw it in the graph view";
    g.onclick = () => {
      state.selected = item;
      showGraph(seedFor(item));
    };
    $("#downloads").append(g);
  }
  for (const d of file.downloads || []) {
    const b = el("button", "dl", `↓ ${d.label}`);
    b.onclick = () => download(`${slug(file.title)}.${d.ext}`, d.mime, d.body());
    $("#downloads").append(b);
  }

  const tabs = el("div", "tabs");
  const body = el("div", "tabbody");
  file.tabs.forEach((t, i) => {
    const b = el("button", "tab" + (i === 0 ? " on" : ""), t.label);
    b.onclick = () => {
      tabs.querySelectorAll(".tab").forEach((x) => x.classList.remove("on"));
      b.classList.add("on");
      body.innerHTML = "";
      body.append(renderTab(t));
    };
    tabs.append(b);
  });
  pane.append(tabs, body);
  body.append(renderTab(file.tabs[0]));
  if (file.note) pane.append(el("div", "note", file.note));
}

function renderTab(tab) {
  if (tab.kind === "properties") return renderProperties(tab);
  if (tab.kind === "table") return renderTable(tab);
  if (tab.kind === "json") return renderPre(JSON.stringify(tab.value, null, 2));
  if (tab.kind === "sectionmap") return renderSectionMap(tab);
  return renderPre(tab.value || "");
}

function renderProperties(tab) {
  const box = el("div");
  const t = el("table", "props");
  for (const p of tab.props) {
    const tr = el("tr");
    tr.append(termCell(p.predicate, "pred"), termCell(p.object, "obj"));
    t.append(tr);
  }
  box.append(t);
  if (tab.refs && tab.refs.length) {
    box.append(el("h4", null, `Referenced by (${tab.refs.length})`));
    const t2 = el("table", "props");
    for (const r of tab.refs) {
      const tr = el("tr");
      tr.append(termCell(r.subject, "obj"), termCell(r.predicate, "pred"));
      t2.append(tr);
    }
    box.append(t2);
  }
  return box;
}

function termCell(term, cls) {
  const td = el("td", cls);
  if (term.iri) {
    const a = el("a", "iri", localName(term.value));
    a.href = "#";
    a.title = term.value;
    a.onclick = (e) => {
      e.preventDefault();
      showFile({ view: state.view, kind: "file", name: localName(term.value), iri: term.value });
    };
    td.append(a);
  } else {
    td.append(el("span", "lit", term.value));
    if (term.lang) td.append(el("span", "tag", `@${term.lang}`));
    else if (term.datatype) td.append(el("span", "tag", localName(term.datatype)));
  }
  return td;
}

/** Whether a result has something to map: a WKT geometry, latitude and longitude columns, or a .pmtiles layer. */
function hasGeo(result) {
  const WKT = /^\s*(<[^>\s]*>\s*)?(POINT|LINESTRING|POLYGON|MULTIPOINT|MULTILINESTRING|MULTIPOLYGON|GEOMETRYCOLLECTION)\s*[ZM]*\s*\(/i;
  if (result.vars.some((v) => /(^|[^a-z])lat(itude)?$/i.test(v)) && result.vars.some((v) => /(^|[^a-z])(lon|lng|long|longitude)$/i.test(v))) return true;
  return (result.rows || []).slice(0, 200).some((row) => row.some((cell) => WKT.test(String(cell)) || /\.pmtiles$/i.test(String(cell).split(/[?#]/)[0])));
}

function renderTable(tab) {
  const wrap = el("div", "tablewrap");
  const t = el("table", "grid");
  const head = el("tr");
  for (const v of tab.vars) head.append(el("th", null, v));
  t.append(head);
  tab.rows.forEach((row, ri) => {
    const tr = el("tr");
    row.forEach((cell, ci) => {
      const td = el("td");
      const iri = tab.iris && tab.iris[ri] && tab.iris[ri][ci];
      if (iri) {
        const a = el("a", "iri", cell);
        a.href = "#";
        a.title = iri;
        a.onclick = (e) => {
          e.preventDefault();
          showFile({ view: state.view, kind: "file", name: cell, iri });
        };
        td.append(a);
      } else {
        td.textContent = cell;
      }
      tr.append(td);
    });
    t.append(tr);
  });
  wrap.append(t);
  return wrap;
}

function renderSectionMap(tab) {
  const box = el("div");
  box.append(el("p", "blurb", tab.blurb));

  const bar = el("div", "extent");
  const inner = el("div", "extent-fill");
  const left = (tab.section.offset / (tab.fileSize || 1)) * 100;
  const width = (tab.section.length / (tab.fileSize || 1)) * 100;
  inner.style.left = `${left}%`;
  inner.style.width = `${Math.max(width, 0.25)}%`;
  bar.append(inner);
  box.append(bar, el("div", "extent-legend", `0 ──── the whole file (${humanBytes(tab.fileSize)}) ──── ${humanBytes(tab.fileSize)}`));

  const t = el("table", "props");
  for (const [k, v] of tab.rows) {
    const tr = el("tr");
    tr.append(el("td", "pred", k), el("td", "obj", v));
    t.append(tr);
  }
  box.append(t);
  return box;
}

const renderPre = (text) => {
  const p = el("pre", "code");
  p.textContent = text;
  return p;
};

// ------------------------------------------------------------------ extract

function paintExtractBar(item) {
  const bar = $("#extract");
  bar.innerHTML = "";
  const extractable = item.kind === "dir" && item.iri &&
    ["types", "predicates", "graphs"].includes(item.view);
  if (!extractable) {
    bar.append(el("span", "muted", "select a folder to extract"));
    return;
  }
  bar.append(el("span", "muted", `extract ${item.name} →`));
  for (const fmt of ["csv", "json", "nt"]) {
    const b = el("button", "dl", fmt.toUpperCase());
    b.onclick = async () => {
      b.disabled = true;
      const was = b.textContent;
      b.textContent = "…";
      try {
        const out = await extract(state.ctx, item, { format: fmt, limit: Number($("#limit").value) || 5000 });
        download(out.filename, out.mime, out.body);
        setStatus(`extracted ${humanCount(out.count)} rows → ${out.filename}`);
      } catch (err) {
        setStatus(`extract failed: ${err.message}`, true);
      } finally {
        b.disabled = false;
        b.textContent = was;
      }
    };
    bar.append(b);
  }
  const lim = el("input");
  lim.id = "limit";
  lim.type = "number";
  lim.value = "5000";
  lim.min = "1";
  lim.title = "row cap";
  bar.append(lim);
}

function download(filename, mime, body) {
  const blob = new Blob([body], { type: mime });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = filename;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 5000);
}

const slug = (s) => String(s).replace(/[^\w.-]+/g, "_").slice(0, 60) || "extract";

// ------------------------------------------------------------------- status

function setStatus(text, bad = false) {
  const s = $("#status");
  s.textContent = text;
  s.classList.toggle("bad", bad);
}

function paintTraffic() {
  const { bytes, requests } = state.traffic;
  const size = state.meta ? state.meta.size : 0;
  const pct = size ? ((bytes / size) * 100).toFixed(4) : "0";
  $("#traffic").textContent =
    `${humanBytes(bytes)} fetched in ${humanCount(requests)} requests` +
    (size ? ` — ${pct}% of ${humanBytes(size)}` : "");
}

// --------------------------------------------------------------------- boot

const showVeil = (on) => {
  $("#dropveil").hidden = !on;
  const dz = $("#drop");
  if (dz) dz.classList.toggle("over", on);
};

function wireOpeners() {
  $("#omnibox").addEventListener("submit", (e) => {
    e.preventDefault();
    openLink($("#url").value);
  });

  // Desktop: the OS owns file picking, and a dropped file arrives as a *path*
  // rather than a Blob — Tauri intercepts the webview's HTML5 drag events and
  // re-emits them, so both routes go through the native open.
  if (isTauri()) {
    $("#file").parentElement.addEventListener("click", async (e) => {
      e.preventDefault();
      try {
        const path = await pickReteFile();
        if (path) await openNative(path, baseName(path));
      } catch (err) {
        setStatus(`could not open: ${err.message}`, true);
      }
    });

    const ev = window.__TAURI__ && window.__TAURI__.event;
    if (ev && typeof ev.listen === "function") {
      ev.listen("tauri://drag-enter", () => showVeil(true));
      ev.listen("tauri://drag-over", () => showVeil(true));
      ev.listen("tauri://drag-leave", () => showVeil(false));
      ev.listen("tauri://drag-drop", (e) => {
        showVeil(false);
        const path = e.payload && e.payload.paths && e.payload.paths[0];
        if (!path) return;
        if (!/\.rete$/i.test(path)) {
          setStatus(`not a .rete file: ${baseName(path)}`, true);
          return;
        }
        openNative(path, baseName(path));
      });
    }
    return;
  }

  // Browser. The listeners sit on the window rather than the home's dropzone,
  // because that element is hidden once an archive is open and dropping a
  // second file has to keep working.
  const stop = (e) => { e.preventDefault(); e.stopPropagation(); };
  let depth = 0; // dragenter/leave fire per element crossed; count to avoid flicker

  window.addEventListener("dragenter", (e) => {
    stop(e);
    // Ignore drags that carry no file (text selections, links).
    const dt = e.dataTransfer;
    if (dt && dt.types && !Array.from(dt.types).includes("Files")) return;
    depth++;
    showVeil(true);
  });
  window.addEventListener("dragover", (e) => { stop(e); if (e.dataTransfer) e.dataTransfer.dropEffect = "copy"; });
  window.addEventListener("dragleave", (e) => {
    stop(e);
    depth = Math.max(0, depth - 1);
    if (depth === 0) showVeil(false);
  });
  window.addEventListener("drop", (e) => {
    stop(e);
    depth = 0;
    showVeil(false);
    const f = e.dataTransfer && e.dataTransfer.files && e.dataTransfer.files[0];
    if (!f) return;
    if (!/\.rete$/i.test(f.name)) {
      setStatus(`not a .rete file: ${f.name}`, true);
      return;
    }
    openLocal(f);
  });

  $("#file").addEventListener("change", (e) => {
    const f = e.target.files && e.target.files[0];
    if (f) openLocal(f);
    e.target.value = "";
  });
}

// A handle for poking at an open archive from the console — this is an
// experiment, and being able to run `reteFs.ctx.select("…")` by hand is worth
// more than the purity of not leaking a global.
window.reteFs = state;

paintHome();
paintViewTabs();
wireOpeners();
wireToolbar();
wireHome();
initTicker();
setStatus("pick a file, drop a .rete, or paste a link");
freshenCatalogs();
