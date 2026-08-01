// app.js — the archive window: catalog, view tabs, lazy tree, preview, extract.
//
// All graph knowledge lives in rete-fs.js; all engine access goes through
// fs-worker.js. This file is only chrome: it draws a tree, opens files into a
// preview pane, and keeps the traffic meter honest.

import {
  VIEWS, VIEW_BY_ID, makeContext, openFile, extract, candidateLabelPredicates,
  readSelfDescription, parseHeader, humanBytes, humanCount, localName, parseTerm,
  seedQuery, runSparql, resultToFile, searchGraph,
} from "./rete-fs.js";
import { isTauri, makeTauriWorkerShim, pickReteFile } from "./tauri-bridge.js";
import { initTicker } from "./ticker.js";

const $ = (sel) => document.querySelector(sel);
const el = (tag, cls, text) => {
  const n = document.createElement(tag);
  if (cls) n.className = cls;
  if (text != null) n.textContent = text;
  return n;
};

// ------------------------------------------------------------------ catalog

// A spread of real archives on R2, chosen to make the metaphor sweat: a tiny one
// that opens instantly, mid-size ones with rich class structure, and a few far
// too large to download — the whole point of browsing in place.
//
// Every entry here carries a baked schema pyramid (a nonzero schemaMetaLen in
// its header), because that is what the Types and Predicates views read. Files
// built with `--no-pyramid` — gharchive, orcid, dblp, the wikidata-xxl shards —
// still open and still browse under Namespace and Sections, but their class
// list cannot be had without scanning, so they are not what to open first.
const CATALOG = [
  { key: "worldcup", name: "World Cup 2022", url: "https://data.graphplaza.com/worldcup/worldcup.rete", size: 227000, blurb: "tiny — opens instantly" },
  { key: "lombardi", name: "Lombardi networks", url: "https://data.graphplaza.com/lombardi/lombardi.rete", size: 1040326, blurb: "art + conspiracy diagrams" },
  { key: "z-anatomy", name: "Z-Anatomy", url: "https://data.graphplaza.com/z-anatomy/z-anatomy.rete", size: 5650000, blurb: "human anatomy, 3D" },
  { key: "boe", name: "BOE (Spanish law)", url: "https://data.graphplaza.com/boe/boe.rete", size: 6960000, blurb: "12,330 laws, ELI" },
  { key: "mtg", name: "Magic: The Gathering", url: "https://data.graphplaza.com/mtg/mtg.rete", size: 16310000, blurb: "34,633 cards, images, rulings" },
  { key: "ror", name: "ROR organizations", url: "https://data.graphplaza.com/ror/ror.rete", size: 30050000, blurb: "111,068 research orgs" },
  { key: "bioexplora", name: "Bioexplora (MCNB)", url: "https://data.graphplaza.com/bioexplora/bioexplora.rete", size: 47550000, blurb: "natural history specimens" },
  { key: "arxiu", name: "Arxius en Línia", url: "https://data.graphplaza.com/arxiu/arxiu.rete", size: 72247508, blurb: "629,875 archival units" },
  { key: "farmacos-es", name: "Medicines (Spain)", url: "https://data.graphplaza.com/farmacos-es/farmacos-es.rete", size: 291680000, blurb: "25,485 medicines" },
  { key: "wikidata", name: "Wikidata (1 GB slice)", url: "https://data.graphplaza.com/wikidata-1GB/wikidata.rete", size: 1490440000, blurb: "1.4 GB · 120M quads" },
  { key: "gbif-birds", name: "GBIF birds", url: "https://data.graphplaza.com/gbif-birds/gbif-birds.rete", size: 1532500000, blurb: "334M quads of occurrences" },
  { key: "bne", name: "Biblioteca Nacional (BNE)", url: "https://data.graphplaza.com/bne-full/bne-full.rete", size: 3653580000, blurb: "3.4 GB · 267M quads" },
  { key: "causenet", name: "CauseNet", url: "https://data.graphplaza.com/causenet-full-typed/causenet-full-typed.rete", size: 6386310000, blurb: "5.9 GB of claimed causality" },
  { key: "databnf", name: "data.bnf.fr", url: "https://data.graphplaza.com/databnf-full/databnf-full.rete", size: 7685950000, blurb: "7.2 GB · 673M quads — the stress test" },
];

// -------------------------------------------------------------------- state

const state = {
  worker: null,
  ctx: null,
  meta: null,
  view: "types",
  layout: "tree", // "tree" = nested and expandable, "icons" = one folder as tiles
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

/**
 * Desktop open: one path for both local files and URLs, because the native side
 * reads them through the same `RangeReader`. It hands back the raw 1 KB header
 * so `parseHeader` — the browser's own — stays the single implementation.
 */
async function openNative(source, name) {
  if (state.opening) return;
  state.opening = true;
  setStatus(`opening ${name}…`);
  resetPanes();
  try {
    bootWorker();
    const r = await send({ type: "open", source });
    finishOpen({
      source: name,
      url: source,
      size: r.size,
      header: parseHeader(new Uint8Array(r.head)),
      card: r.cardText ? JSON.parse(r.cardText) : null,
      schema: r.schema,
      schemaError: r.schemaError,
    });
  } catch (err) {
    setStatus(`could not open: ${err.message}`, true);
  } finally {
    state.opening = false;
  }
}

async function openRemote(entry) {
  if (isTauri()) return openNative(entry.url, entry.name);
  if (state.opening) return;
  state.opening = true;
  setStatus(`opening ${entry.name}…`);
  resetPanes();
  try {
    // Two range requests learn what the archive is, whatever it weighs.
    const desc = await readSelfDescription(entry.url);
    bootWorker();
    const opened = await send({ type: "open", mode: "remote", url: entry.url });
    finishOpen({
      source: entry.name, url: entry.url, size: desc.size || entry.size,
      header: desc.header, card: desc.card,
      schema: opened.schema, schemaError: opened.schemaError,
    });
  } catch (err) {
    setStatus(`could not open: ${err.message}`, true);
  } finally {
    state.opening = false;
  }
}

async function openLocal(file) {
  if (state.opening) return;
  state.opening = true;
  setStatus(`reading ${file.name}…`);
  resetPanes();
  try {
    const bytes = new Uint8Array(await file.arrayBuffer());
    const header = parseHeader(bytes);
    let card = null;
    const metaSection = header.sections.find((s) => s.kind === 1);
    if (metaSection && metaSection.length) {
      try {
        card = JSON.parse(new TextDecoder().decode(
          bytes.subarray(metaSection.offset, metaSection.offset + metaSection.length)
        ));
      } catch (_) { /* unparseable card is not fatal */ }
    }
    bootWorker();
    const copy = bytes.slice();
    const opened = await send({ type: "open", mode: "local", bytes: copy.buffer, transfer: [copy.buffer] });
    finishOpen({
      source: file.name, url: null, size: bytes.byteLength,
      header, card, schema: opened.schema, schemaError: opened.schemaError,
    });
  } catch (err) {
    setStatus(`could not open: ${err.message}`, true);
  } finally {
    state.opening = false;
  }
}

function finishOpen(meta) {
  state.meta = meta;
  state.ctx = makeContext({ engine, meta });
  state.expanded = new Set();
  state.selected = null;

  $("#archive-name").textContent = meta.source;
  $("#archive-facts").textContent = [
    humanBytes(meta.size),
    `${humanCount(meta.header.quadCount)} quads`,
    `${humanCount(meta.header.termCount)} terms`,
    `format 0x${meta.header.version.toString(16).padStart(2, "0")}`,
  ].join(" · ");
  $("#window").hidden = false;
  $("#empty").hidden = true;
  paintTraffic();
  paintLabelPicker();

  if (meta.schemaError) {
    setStatus(`opened — no baked schema (${meta.schemaError}); Types and Predicates will be empty`, true);
  } else {
    setStatus("opened");
  }
  selectView(meta.header.hasQuads ? state.view : state.view === "graphs" ? "types" : state.view);
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

// -------------------------------------------------------------------- views

function paintViewTabs() {
  const bar = $("#views");
  bar.innerHTML = "";
  for (const v of VIEWS) {
    const b = el("button", "viewtab" + (v.id === state.view ? " on" : ""));
    b.append(el("span", "vi", v.icon), document.createTextNode(v.label));
    b.title = v.hint;
    b.onclick = () => selectView(v.id);
    bar.append(b);
  }
  $("#view-hint").textContent = VIEW_BY_ID.get(state.view).hint;
}

async function selectView(id) {
  state.view = id;
  state.expanded = new Set();
  state.path = [];
  state.results = null; // a view change is a fresh start, not a filtered one
  paintViewTabs();
  await renderPane();
}

/** Draw whichever layout is active, from scratch. */
async function renderPane() {
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
      showQueryResult(result, Math.round(performance.now() - started));
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

function showQueryResult(result, ms) {
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
 * Back to the welcome screen. The open archive is deliberately left resident —
 * the worker keeps its handle and its warmed block cache, so re-opening the
 * same file from the catalog costs nothing and the traffic meter keeps counting
 * from where it was.
 */
function goHome() {
  $("#window").hidden = true;
  $("#empty").hidden = false;
  setStatus(state.meta ? `${state.meta.source} still open — pick another, or reopen it` : "pick an archive, drop a .rete, or paste a URL");
}

function wireHome() {
  for (const id of ["home", "back"]) {
    const b = $(`#${id}`);
    if (b) b.addEventListener("click", goHome);
  }
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
    state.layout = b.dataset.mode;
    $("#layout").querySelectorAll("button").forEach((x) => x.classList.toggle("on", x === b));
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

function paintCatalog() {
  const list = $("#catalog");
  for (const entry of CATALOG) {
    const b = el("button", "cat");
    b.append(el("span", "cat-name", entry.name));
    b.append(el("span", "cat-size", humanBytes(entry.size)));
    b.append(el("span", "cat-blurb", entry.blurb));
    b.onclick = () => openRemote(entry);
    list.append(b);
  }
}

const showVeil = (on) => {
  $("#dropveil").hidden = !on;
  const dz = $("#drop");
  if (dz) dz.classList.toggle("over", on);
};

const baseName = (p) => String(p).split(/[\\/]/).pop() || String(p);

function wireDrop() {
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

    $("#url-open").addEventListener("click", () => {
      const url = $("#url").value.trim();
      if (url) openNative(url, baseName(url));
    });
    $("#url").addEventListener("keydown", (e) => { if (e.key === "Enter") $("#url-open").click(); });
    return;
  }

  // Browser. The listeners sit on the window rather than the landing-screen
  // dropzone, because that element is hidden once an archive is open and
  // dropping a second file has to keep working.
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
  });
  $("#url-open").addEventListener("click", () => {
    const url = $("#url").value.trim();
    if (url) openRemote({ name: baseName(url), url, size: 0 });
  });
  $("#url").addEventListener("keydown", (e) => { if (e.key === "Enter") $("#url-open").click(); });
}

// A handle for poking at an open archive from the console — this is an
// experiment, and being able to run `reteFs.ctx.select("…")` by hand is worth
// more than the purity of not leaking a global.
window.reteFs = state;

paintCatalog();
paintViewTabs();
wireDrop();
wireToolbar();
wireHome();
initTicker();
setStatus("pick an archive, drop a .rete, or paste a URL");
