// library.js — what the welcome screen shows: the files you opened, and the
// catalogs you subscribe to.
//
// A catalog is a JSON file anyone can publish next to their graphs: a list of
// .rete files with a name, a line about each, a size. The explorer reads the
// shape rete's own catalog has (clients/relay/catalog.json: { datasets: [...] }),
// the plaza's (docs/plaza/plaza.json), a bare list of entries, or a bare list
// of URLs — so subscribing to a catalog is pasting its link.
//
// Everything here is kept in this browser (localStorage) — or, in the desktop
// app, in its webview's storage — and nothing breaks without it: a private
// window simply starts with the default catalog every time.

const KEY = "rete-explorer.v1";
const MAX_FILES = 40;
const MAX_CARDS = 400;

/** rete's own catalog: every published graph. Its JSON is regenerated with the catalog (scripts/export_space_catalog.py). */
export const DEFAULT_CATALOG = {
  url: "https://raw.githubusercontent.com/caviri/rete/main/clients/relay/catalog.json",
  name: "rete catalog",
};

// ------------------------------------------------------------------ storage

function load() {
  try {
    const raw = localStorage.getItem(KEY);
    const s = raw ? JSON.parse(raw) : null;
    if (s && typeof s === "object") return { files: s.files ?? [], catalogs: s.catalogs ?? null, cards: s.cards ?? {} };
  } catch (_) { /* no storage, or a value from another version: start afresh */ }
  return { files: [], catalogs: null, cards: {} };
}

function save(s) {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch (_) {
    // Full or blocked. The thumbnails' card cache is the one part that grows: drop it and retry once.
    try {
      localStorage.setItem(KEY, JSON.stringify({ ...s, cards: {} }));
    } catch (_) { /* nothing to do: the session still works */ }
  }
}

const state = load();
if (!state.catalogs) state.catalogs = [{ url: DEFAULT_CATALOG.url, name: DEFAULT_CATALOG.name, datasets: [], fetchedAt: null }];

// --------------------------------------------------------------- your files

/** The files opened here, most recent first. */
export const recentFiles = () => state.files.slice();

/**
 * Remember an opened file. Remote files keep their URL (they reopen with a
 * click); local ones their name and, in the desktop app, their path — a browser
 * cannot reopen a local file by itself, so it asks for it again.
 */
export function rememberFile(entry) {
  const id = entry.url ? `url:${entry.url}` : entry.path ? `path:${entry.path}` : `local:${entry.name}:${entry.size}`;
  const prev = state.files.find((f) => f.id === id);
  state.files = [{ ...prev, ...entry, id, openedAt: Date.now() }, ...state.files.filter((f) => f.id !== id)].slice(0, MAX_FILES);
  save(state);
  return id;
}

export function forgetFile(id) {
  state.files = state.files.filter((f) => f.id !== id);
  save(state);
}

// ----------------------------------------------------------------- catalogs

export const subscriptions = () => state.catalogs.slice();

/** Subscribe to a catalog by its link: fetched and checked before it is kept. */
export async function subscribe(url) {
  const href = webUrl(url);
  if (!href) throw new Error("a catalog is an http(s) link to a JSON file");
  if (state.catalogs.some((c) => c.url === href)) throw new Error("already subscribed to that catalog");
  const cat = await fetchCatalog(href);
  if (!cat.datasets.length) throw new Error("that file lists no .rete files");
  state.catalogs.push({ url: href, name: cat.name, description: cat.description, datasets: cat.datasets, fetchedAt: Date.now() });
  save(state);
  return state.catalogs.at(-1);
}

export function unsubscribe(url) {
  state.catalogs = state.catalogs.filter((c) => c.url !== url);
  save(state);
}

/** Fetch a subscribed catalog again; on failure the copy kept from last time stays, with the error. */
export async function refresh(url) {
  const sub = state.catalogs.find((c) => c.url === url);
  if (!sub) return null;
  try {
    const cat = await fetchCatalog(url);
    // A catalog that names itself keeps that name; one that does not keeps the one it was given.
    Object.assign(sub, { name: cat.titled ? cat.name : sub.name || cat.name, description: cat.description, datasets: cat.datasets, fetchedAt: Date.now(), error: null });
  } catch (error) {
    sub.error = String(error?.message ?? error);
  }
  save(state);
  return sub;
}

/** A catalog fetched and read, whatever shape it has. */
export async function fetchCatalog(url) {
  const res = await fetch(url, { headers: { Accept: "application/json" } });
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  const text = await res.text();
  if (text.length > 20 << 20) throw new Error("that catalog is over 20 MB");
  let json;
  try {
    json = JSON.parse(text);
  } catch (_) {
    throw new Error("that link is not JSON — a catalog is a JSON list of .rete files");
  }
  return normalizeCatalog(json, url);
}

/**
 * Any of the shapes a catalog comes in, as { name, description, datasets }.
 * Each dataset: { key, name, blurb, url, sizeText, size, triples, tags, license, thumbnail }.
 * An entry with no single .rete file (a sharded dataset) is left out.
 */
export function normalizeCatalog(json, sourceUrl) {
  const list = Array.isArray(json) ? json : Array.isArray(json?.datasets) ? json.datasets : null;
  if (!list) throw new Error("no dataset list in that file — expected { \"datasets\": [...] } or a list");
  const base = String(json?.remoteBase ?? "").replace(/\/$/, "");
  const datasets = [];
  const seen = new Set();
  for (const raw of list) {
    const d = typeof raw === "string" ? { url: raw } : raw;
    if (!d || typeof d !== "object") continue;
    const key = String(d.key ?? d.id ?? "").trim();
    let url = d.url ?? d.rete ?? d.href ?? (key && base && !d.shards ? `${base}/${key}/${key}.rete` : null);
    url = url ? resolve(url, sourceUrl) : null;
    if (!url || seen.has(url)) continue;
    seen.add(url);
    const fileName = decodeURIComponent(url.split(/[?#]/)[0].split("/").pop() || "");
    const { name, blurb } = nameAndBlurb(d, key || fileName.replace(/\.rete$/i, ""));
    const sizeText = typeof d.size === "string" ? d.size : Number.isFinite(d.size) ? bytes(d.size) : null;
    datasets.push({
      key: key || fileName.replace(/\.rete$/i, ""),
      name,
      blurb,
      url,
      sizeText,
      triples: d.triples == null ? null : String(d.triples),
      tags: Array.isArray(d.tags) ? d.tags.slice(0, 6).map(String) : [],
      license: typeof d.license === "string" ? d.license.split(/[(;]/)[0].trim().slice(0, 40) : null,
      icon: typeof d.icon === "string" && d.icon.length <= 4 ? d.icon : null,
      thumbnail: webUrl(d.thumbnail ?? d.image ?? null),
    });
  }
  const titled = String(json?.title ?? json?.name ?? "").trim();
  const description = String(json?.tagline ?? json?.description ?? "").trim() || null;
  return { name: titled || hostOf(sourceUrl), titled: !!titled, description, datasets };
}

/**
 * A short name and a line about a dataset. rete's catalog labels read
 * "boe.rete — Spanish law: 12,330 laws …", so a label in that form gives both.
 */
function nameAndBlurb(d, fallback) {
  const label = String(d.title ?? d.label ?? d.name ?? "").trim();
  let name = label || fallback;
  let blurb = String(d.blurb ?? "").trim();
  const dash = label.indexOf(" — ");
  if (dash > 0) {
    name = label.slice(0, dash).trim();
    blurb ||= label.slice(dash + 3).trim();
  }
  name = name.replace(/\.rete$/i, "");
  if (!blurb && d.description) blurb = plain(d.description);
  blurb = blurb.replace(/\s*\((remote|embedded)[^)]*\)\s*$/i, "");
  return { name, blurb: blurb.length > 220 ? `${blurb.slice(0, 219).replace(/\s+\S*$/, "")}…` : blurb };
}

/** Markdown to one plain line. */
const plain = (s) =>
  String(s)
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[*_`#>]+/g, "")
    .replace(/\s+/g, " ")
    .trim();

function resolve(url, base) {
  try {
    const u = new URL(String(url), base);
    return u.protocol === "http:" || u.protocol === "https:" ? u.href : null;
  } catch (_) {
    return null;
  }
}

export function webUrl(u) {
  if (!u) return null;
  try {
    const url = new URL(String(u));
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch (_) {
    return null;
  }
}

export const hostOf = (u) => {
  try {
    return new URL(u).host;
  } catch (_) {
    return String(u);
  }
};

const bytes = (n) => {
  const u = ["B", "KB", "MB", "GB", "TB"];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) (n /= 1024), i++;
  return `${n < 10 && i ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
};

// ---------------------------------------------------------- cards, for pictures

/**
 * What a thumbnail needs from a file's card, kept small: the classes, the
 * links between them, the vocabularies, a seed. One card read per file, ever.
 */
export function cardSummary(card, header) {
  if (!card) return { none: true, seed: header?.contentHash ?? null };
  const sig = card.signals ?? {};
  return {
    title: typeof card.title === "string" ? card.title.slice(0, 160) : null,
    seed: header?.contentHash ?? card.content_hash ?? null,
    triples: card.triple_count ?? card.quad_count ?? null,
    terms: card.term_count ?? null,
    classes: (card.classes ?? []).slice(0, 24),
    links: [...(card.class_links ?? [])].sort((a, b) => (b.count ?? 0) - (a.count ?? 0)).slice(0, 60),
    vocabularies: (card.vocabularies ?? []).slice(0, 6),
    geo: !!(sig.geo_wkt || sig.geo_latlong),
    temporal: !!(sig.temporal_extent || sig.time_predicates?.length),
    incoherent: card.coherence?.coherent === false,
  };
}

export const cachedCard = (url) => state.cards[url] ?? null;

export function cacheCard(url, summary) {
  state.cards[url] = { ...summary, at: Date.now() };
  const keys = Object.keys(state.cards);
  if (keys.length > MAX_CARDS) {
    keys.sort((a, b) => (state.cards[a].at ?? 0) - (state.cards[b].at ?? 0));
    for (const k of keys.slice(0, keys.length - MAX_CARDS)) delete state.cards[k];
  }
  save(state);
}

/** The thumbnail generator's input (js/procgen.js) from a card summary and a catalog entry. */
export function pictureInfo(summary, entry = {}) {
  const s = summary ?? {};
  return {
    seed: s.seed || entry.url || entry.key || entry.name || "rete",
    mode: "dataset",
    name: entry.key || entry.name || "",
    tags: (entry.tags || []).slice(0, 3),
    triples: s.triples || 1,
    terms: s.terms || 0,
    classes: s.classes || [],
    links: s.links || [],
    predicates: [],
    vocabularies: s.vocabularies || [],
    geo: !!s.geo,
    geoWkt: false,
    bbox: null,
    temporal: !!s.temporal,
    temporalExtent: null,
    incoherent: !!s.incoherent,
  };
}
