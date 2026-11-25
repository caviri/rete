// The graph view: an MCP App the host renders in a sandboxed iframe. It holds
// no data of its own: it asks the server (through the host) for its view's
// elements, applies the numbered patches the server records — its own
// expansions and searches, and the changes Claude makes with
// graph_view_update — and reports what the user selected and set, which
// Claude reads with graph_view_state. It also takes a file from the user — the
// way a chat attachment, which Claude keeps in its own sandbox, reaches the
// extension: chosen or dropped here, sent to the server in parts.
import { App } from "@modelcontextprotocol/ext-apps/app-with-deps";
import cytoscape from "cytoscape";

import * as Cards from "./cards.js";
import { hivePositions, layeredPositions } from "./layout.js";

// Retelens (clients/chrome/lib), bundled as they are — one copy for the browser extension, this view in Claude and
// the file explorer: result cells (pictures, IIIF manifests, geometries as small maps, Markdown, links), column
// statistics, query hints (# view:, # column ?v: Label — note) and maps (an open OpenStreetMap basemap read as
// PMTiles, .pmtiles columns as layers). Each sets a window.Retelens* object; the order is their dependency order.
import "../../../chrome/lib/octicons.js";
import "../../../chrome/lib/format.js";
import "../../../chrome/lib/results.js";
import "../../../chrome/lib/cells.js";
import "../../../chrome/lib/stats.js";
import "../../../chrome/lib/hints.js";
import "../../../chrome/lib/maps.js";

const PALETTE = ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#2f4b7c", "#a05195", "#5fa2ce"];
const OTHER = "#a8a8a8";
const MAX_PROPS = 40; // shown in the selection panel
const MAX_LITERAL_NODES = 12; // per node, with literals as nodes
const MAX_TABLE = 1000; // rows drawn at once
const FIELD_LABELS = Object.fromEntries(Cards.FIELD_TYPES);
const PART = 1024 * 1024; // bytes of a file per graph_view_upload call
// Version, build, credits and links: stamped in by build.mjs.
const ABOUT = typeof __RETE_ABOUT__ === "object" ? __RETE_ABOUT__ : null;
const FONT = `system-ui, -apple-system, "Segoe UI", Roboto, Helvetica, Arial, sans-serif`;
// A box: its name on a band, its type under it, then a line per property (px).
const BOX = { pad: 8, title: 12, small: 10, head: 24, sub: 14, row: 15, rows: 8, keyMax: 120, valueMax: 190, min: 110, max: 330 };

const S = {
  app: null,
  cy: null,
  host: {},
  viewId: null,
  dataset: "",
  title: "",
  version: 0,
  nodes: new Map(),
  edges: new Map(),
  table: null,
  options: {
    diagram: "graph",
    labels: true,
    edge_labels: false,
    properties: true,
    node_shape: "circle",
    edge_style: "curved",
    group_by: "type",
    group_boxes: false,
    layout: "cose",
    hive_axes: "auto", // the hive layout: what puts a node on an axis,
    hive_order: "degree", // and what orders it along it
    card_fields: {}, // the cards view: how each field is drawn, by column
    view: "graph",
    literals_as_nodes: false,
    theme: "auto", // or "light", "dark"; auto follows Claude's
  },
  groups: new Map(), // key -> { color, count }
  classBoxes: new Map(), // box id -> { id, iri, label, members, props }: the classes diagram's boxes
  aggregates: new Map(), // id -> { id, label, members }: nodes drawn as one (kept by the server)
  placeAt: new Map(), // node id -> where to put it when it is drawn again (out of an aggregate)
  selecting: false, // the "select several" mode: a click adds or removes, a drag draws a box
  menu: [], // the open right-click menu's items
  hiddenGroups: new Set(),
  hiddenNodes: new Set(),
  highlight: new Set(),
  selection: [],
  search: "",
  status: "",
  awaiting: null, // { file }: the chat attachment this view asks the user to open
  uploading: false,
  labelled: new Set(), // node ids whose labels were asked for
  sort: { col: null, desc: false },
  rich: { key: null }, // the rows as Retelens terms: { key, data, model, short, colInfo } (richModel)
  map: null, // the Map tab's map, while it shows
  hive: null, // the hive layout's last plan: { pos, axes, bow, by }
  cards: { viewerTimeout: 45000, shown: Cards.CARDS_FIRST, io: null, live: [], fieldsKey: "", blocked: new Set() },
  lastActivity: Date.now(),
  stateDirty: true,
  syncing: false,
  timer: null,
};
window.__reteView = S; // tests read the view through this

const $ = (id) => document.getElementById(id);

// What the Retelens libraries ask of the page that hosts them: start the map library this page carries as text (as
// the 3D viewer is), show a .pmtiles column's layer (on the Map tab), and the theme (this page's, not the system's).
window.RetelensHost = {
  loadMapKit() {
    if (window.RetelensMapKit) return Promise.resolve();
    const source = document.getElementById("mapkit-src")?.textContent;
    if (!source) return Promise.reject(new Error("the map library is not in this page"));
    try {
      const script = document.createElement("script");
      script.textContent = source;
      document.head.append(script);
    } catch {
      /* a host that forbids inline scripts leaves it undefined */
    }
    return window.RetelensMapKit ? Promise.resolve() : Promise.reject(new Error("the map library could not start in this host"));
  },
  openLayer: () => showTab("map"),
};
window.RetelensTheme = {
  tokens: () => ({
    dark: document.documentElement.dataset.theme === "dark",
    accent: getComputedStyle(document.documentElement).getPropertyValue("--accent").trim() || "#d97757",
  }),
};
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
/** A web address to open, or null: only http(s) leaves the view, whatever scheme an IRI has. */
const webUrl = (u) => {
  try {
    const url = new URL(String(u ?? ""));
    return url.protocol === "http:" || url.protocol === "https:" ? url.href : null;
  } catch {
    return null;
  }
};
const openUrl = (u) => {
  const url = webUrl(u);
  return url ? S.app.openLink({ url }) : Promise.reject(new Error("only http(s) links open"));
};
/** An IRI in the details panel: a link when it is a web address, text otherwise. */
const iriLine = (iri) =>
  webUrl(iri) ? `<a href="#" class="iri" data-open="${esc(iri)}">${esc(iri)}</a>` : `<span class="iri">${esc(iri)}</span>`;
const local = (iri) => {
  const s = String(iri ?? "");
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  const tail = cut >= 0 && cut < s.length - 1 ? s.slice(cut + 1) : s;
  // Percent-encoded names (…/%C3%81lvaro_Morata) read as what they encode.
  try {
    return decodeURIComponent(tail);
  } catch {
    return tail;
  }
};
const namespace = (iri) => {
  const s = String(iri ?? "");
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return cut > 0 ? s.slice(0, cut + 1) : s;
};
const trunc = (s, n) => (String(s).length > n ? `${String(s).slice(0, n - 1)}…` : String(s));
const nodeLabel = (n) => trunc(n.label || (n.iri ? local(n.iri) : n.id), 48);
const shortName = (p) => (String(p).startsWith("?") ? p : local(p)); // a predicate, or a SELECT column
const diagramOf = () => S.options.diagram || "graph";
/** Whether the host takes a message for the conversation (Claude does; the file explorer has no chat). */
const chatty = () => !!S.app?.getHostCapabilities()?.message;
/** A diagram's own look, as the server gives it: boxes and classes in layers with square lines. */
const diagramLook = (diagram) =>
  diagram === "graph" ? { diagram, layout: "cose", edge_style: "curved" } : { diagram, layout: "layered", edge_style: "square" };
const debounce = (fn, ms) => {
  let t;
  return (...a) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...a), ms);
  };
};

// --- talking to the server ----------------------------------------------------

async function call(name, args) {
  const result = await S.app.callServerTool({ name, arguments: args });
  const text = result?.content?.find((c) => c.type === "text")?.text ?? "";
  if (result?.isError) throw new Error(text || `${name} failed`);
  return text ? JSON.parse(text) : {};
}

async function sync(force = false) {
  if (!S.viewId || S.syncing) return schedule();
  S.syncing = true;
  try {
    const args = { view_id: S.viewId, since: S.version };
    if (S.stateDirty || force) {
      args.state = reportState();
      S.stateDirty = false;
    }
    apply(await call("graph_view_sync", args));
  } catch (error) {
    setNote(`sync: ${error.message}`);
  } finally {
    S.syncing = false;
    schedule();
  }
}

function schedule() {
  clearTimeout(S.timer);
  if (!S.viewId) return;
  const idle = Date.now() - S.lastActivity > 60_000;
  S.timer = setTimeout(() => sync(false), document.hidden ? 5000 : idle ? 4000 : 1200);
}

const markState = debounce(() => {
  S.stateDirty = true;
  S.lastActivity = Date.now();
  sync(false);
}, 400);

/** What the user did, for graph_view_state. */
function reportState() {
  const visible = S.cy ? S.cy.nodes().filter((n) => !n.hasClass("hidden") && !n.hasClass("box") && !n.hasClass("lit")).length : 0;
  return {
    selection: S.selection.slice(0, 50),
    selection_count: S.selection.length,
    search: S.search,
    hidden_layers: [...S.hiddenGroups],
    hidden_nodes: S.hiddenNodes.size,
    visible_nodes: visible,
    options: { ...S.options },
    layers: [...S.groups].slice(0, 20).map(([key, g]) => ({ layer: key || "(all)", nodes: g.count })),
  };
}

// --- applying the server's changes ----------------------------------------------

function apply(response) {
  if (response.snapshot) loadSnapshot(response.snapshot);
  // Two answers can carry the same patches (a sync and a search, say): each applies once.
  for (const patch of response.patches ?? []) if (!(patch.version <= S.version)) applyPatch(patch);
  if (typeof response.version === "number") S.version = response.snapshot ? response.version : Math.max(S.version, response.version);
  renderStatus();
}

function merge(nodes = [], edges = []) {
  for (const n of nodes) {
    const had = S.nodes.get(n.id);
    S.nodes.set(n.id, had ? { ...had, ...n, label: had.label || n.label } : n);
  }
  for (const e of edges) S.edges.set(e.id, e);
}

function loadSnapshot(snap) {
  S.nodes = new Map();
  S.edges = new Map();
  merge(snap.nodes, snap.edges);
  S.table = snap.table ?? null;
  S.title = snap.title ?? S.title;
  S.dataset = snap.dataset ?? S.dataset;
  Object.assign(S.options, snap.options ?? {});
  S.status = snap.status ?? "";
  S.awaiting = snap.awaiting ?? null;
  S.aggregates = new Map((snap.aggregates ?? []).map((a) => [a.id, a]));
  renderAwaiting();
  applyTheme(false); // the rebuild below draws the boxes
  syncControls();
  rebuild(true);
  if (S.options.view !== "graph") showTab(S.options.view); // opened on the table or the cards
  requestLabels();
}

function applyPatch(p) {
  S.lastActivity = Date.now();
  switch (p.op) {
    case "replace":
      S.aggregates = new Map();
      S.nodes = new Map();
      S.edges = new Map();
      S.table = p.table ?? null;
      merge(p.nodes, p.edges);
      rebuild(true);
      setNote("the view was replaced");
      break;
    case "add":
      merge(p.nodes, p.edges);
      if (p.table) S.table = p.table;
      grow(p.nodes ?? [], p.edges ?? []);
      if (p.highlight) setHighlight(p.highlight);
      if (p.focus) focusNode(p.focus);
      break;
    case "labels":
      for (const [id, l] of Object.entries(p.labels)) {
        const n = S.nodes.get(id);
        if (!n) continue;
        n.label ||= l.label;
        if (l.types?.length) n.types = l.types;
      }
      refresh();
      break;
    case "options": {
      const before = { ...S.options };
      Object.assign(S.options, p.options ?? {});
      if (p.title) S.title = p.title;
      syncControls();
      const structural = ["group_by", "group_boxes", "literals_as_nodes", "diagram", "properties"].some((k) => before[k] !== S.options[k]);
      if (before.theme !== S.options.theme) applyTheme();
      if (structural) rebuild(true);
      else {
        applyStyle();
        if (before.layout !== S.options.layout) runLayout(true);
      }
      showTab(S.options.view);
      setNote(chatty() ? "Claude changed the view" : "the view changed");
      break;
    }
    case "highlight":
      setHighlight(p.ids);
      break;
    case "focus":
      focusNode(p.id);
      break;
    case "remove": {
      const gone = new Set(p.ids);
      for (const id of gone) S.nodes.delete(id);
      for (const [k, e] of S.edges) if (gone.has(e.source) || gone.has(e.target)) S.edges.delete(k);
      // As the server does: a node leaves its aggregate, an emptied one goes.
      for (const a of [...S.aggregates.values()]) {
        a.members = a.members.filter((m) => !gone.has(m));
        if (!a.members.length) S.aggregates.delete(a.id);
      }
      rebuild(false);
      break;
    }
    case "aggregate":
      S.aggregates.set(p.aggregate.id, p.aggregate);
      rebuild(true);
      break;
    case "ungroup":
      for (const id of p.ids) {
        // Its members come back where it was.
        const el = S.cy?.getElementById(id);
        if (el?.nonempty()) for (const m of S.aggregates.get(id)?.members ?? []) S.placeAt.set(m, { ...el.position() });
        S.aggregates.delete(id);
      }
      rebuild(true);
      break;
    case "status":
      S.status = p.status ?? "";
      break;
    case "dataset":
      // A file the user opened: the view starts over on it.
      S.dataset = p.dataset;
      if (p.title) S.title = p.title;
      S.awaiting = null;
      S.aggregates = new Map();
      S.nodes = new Map();
      S.edges = new Map();
      S.table = null;
      S.hiddenGroups.clear();
      S.hiddenNodes.clear();
      renderAwaiting();
      rebuild(true);
      break;
  }
  requestLabels();
  renderRows();
}

// --- the graph ------------------------------------------------------------------------

function groupKey(n) {
  const by = S.options.group_by || "type";
  if (by === "none") return "";
  if (by === "type") return n.types?.length ? local(n.types[0]) : n.kind === "iri" ? "untyped" : n.kind;
  if (by === "namespace") return n.iri ? namespace(n.iri) : n.kind;
  const value = n.props?.find(([p]) => p === by)?.[1];
  return value === undefined ? "(none)" : trunc(value, 40);
}

function computeGroups() {
  const counts = new Map();
  for (const n of S.nodes.values()) {
    const k = groupKey(n);
    counts.set(k, (counts.get(k) ?? 0) + 1);
  }
  const ordered = [...counts].sort((a, b) => b[1] - a[1]);
  S.groups = new Map(ordered.map(([k, count], i) => [k, { count, color: k === "" ? PALETTE[0] : PALETTE[i] ?? OTHER }]));
}

function elements() {
  computeGroups();
  S.classBoxes = new Map();
  const diagram = diagramOf();
  if (diagram === "graph") return dotElements();
  const theme = themeColors();
  // A schema view's nodes are classes already: its class diagram is its boxes.
  const schema = S.nodes.size > 0 && [...S.nodes.values()].every((n) => n.kind === "class");
  return diagram === "classes" && !schema ? classElements(theme) : boxElements(theme);
}

/**
 * What is drawn: the nodes outside any aggregate, a node per aggregate, and
 * the edges re-pointed at the aggregates — merged and counted where several
 * fall on the same pair, and not drawn at all inside one aggregate.
 */
function drawn() {
  const owner = new Map();
  for (const a of S.aggregates.values()) for (const m of a.members) if (S.nodes.has(m)) owner.set(m, a.id);
  const nodes = [...S.nodes.values()].filter((n) => !owner.has(n.id));
  const aggregates = [...S.aggregates.values()]
    .map((a) => ({ ...a, members: a.members.filter((m) => owner.get(m) === a.id).map((m) => S.nodes.get(m)) }))
    .filter((a) => a.members.length);
  const edges = [];
  const merged = new Map();
  for (const e of S.edges.values()) {
    const source = owner.get(e.source) ?? e.source;
    const target = owner.get(e.target) ?? e.target;
    if (source === e.source && target === e.target) edges.push(e);
    else if (source !== target) {
      const k = `${source}\u0001${e.p ?? e.label}\u0001${target}`;
      const m = merged.get(k) ?? { id: `ae:${k}`, source, target, label: e.label, n: 0 };
      m.n += 1;
      merged.set(k, m);
    }
  }
  for (const m of merged.values()) edges.push({ ...m, label: m.n > 1 ? `${m.label} ×${m.n}` : m.label });
  return { nodes, aggregates, edges };
}

/** The graph diagram: a dot per entity, in layer colours, maybe framed by layer. */
function dotElements() {
  const boxes = S.options.group_boxes && S.options.group_by !== "none";
  const els = [];
  if (boxes) {
    for (const [k, g] of S.groups) els.push({ group: "nodes", data: { id: `group:${k}`, label: k || "—", color: g.color, gk: k }, classes: "box" });
  }
  const { nodes, aggregates, edges } = drawn();
  for (const a of aggregates) {
    const k = groupKey(a.members[0]);
    const size = Math.round(Math.min(64, 20 + 5 * Math.sqrt(a.members.length)));
    els.push({
      group: "nodes",
      data: { id: a.id, label: `${a.label} (${a.members.length})`, color: S.groups.get(k)?.color ?? OTHER, gk: k, size, ...(boxes ? { parent: `group:${k}` } : {}) },
      classes: "agg",
    });
  }
  for (const n of nodes) {
    const k = groupKey(n);
    els.push({
      group: "nodes",
      data: { id: n.id, label: nodeLabel(n), color: S.groups.get(k)?.color ?? OTHER, gk: k, ...(boxes ? { parent: `group:${k}` } : {}) },
      classes: n.kind === "iri" ? "" : n.kind,
    });
    if (S.options.literals_as_nodes) {
      n.props.slice(0, MAX_LITERAL_NODES).forEach(([p, v], i) => {
        const id = `lit:${n.id}:${i}`;
        els.push({ group: "nodes", data: { id, label: trunc(v, 40), color: OTHER, gk: k }, classes: "lit" });
        els.push({ group: "edges", data: { id: `le:${id}`, source: n.id, target: id, label: local(p) }, classes: "litedge" });
      });
    }
  }
  for (const e of edges) els.push({ group: "edges", data: { id: e.id, source: e.source, target: e.target, label: e.label } });
  return els;
}

// --- boxes: an entity or a class as a box, its properties listed under its name ---------

let measure = null;
function textWidth(text, size, weight = 400) {
  measure ??= document.createElement("canvas").getContext("2d");
  measure.font = `${weight} ${size}px ${FONT}`;
  return measure.measureText(text).width;
}

/** `text` cut to `max` pixels, with an ellipsis. */
function fit(text, size, max, weight = 400) {
  const s = String(text ?? "");
  if (textWidth(s, size, weight) <= max) return s;
  let lo = 0;
  let hi = s.length;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (textWidth(`${s.slice(0, mid)}…`, size, weight) <= max) lo = mid;
    else hi = mid - 1;
  }
  return `${s.slice(0, lo)}…`;
}

function themeColors() {
  const css = getComputedStyle(document.documentElement);
  const v = (name) => css.getPropertyValue(name).trim();
  return { bg: v("--bg"), fg: v("--fg"), muted: v("--muted") };
}

const boxCache = new Map(); // what a box shows -> its size and picture

/**
 * A box as a picture, the node's background: its name on a band in its
 * layer's colour, its type under it, then its properties a line each. Drawn
 * at twice its size, so it stays sharp when zoomed in. `numeric` right-aligns
 * the values (counts).
 */
function boxImage({ title, subtitle, rows, color, numeric }, theme) {
  const showRows = S.options.properties !== false;
  const key = JSON.stringify([title, subtitle, rows, color, numeric, theme, showRows]);
  const hit = boxCache.get(key);
  if (hit) return hit;
  const { pad, small, row } = BOX;
  const name = fit(title, BOX.title, BOX.max - 2 * pad - 4, 600);
  const sub = subtitle ? fit(subtitle, small, BOX.max - 2 * pad - 4) : "";
  const list = showRows ? rows.slice(0, BOX.rows) : [];
  const more = showRows ? rows.length - list.length : 0;
  const keys = list.map(([k]) => fit(k, small, BOX.keyMax));
  const values = list.map(([, v]) => fit(v, small, BOX.valueMax));
  const keyW = Math.max(0, ...keys.map((k) => textWidth(k, small)));
  const valueW = Math.max(0, ...values.map((v) => textWidth(v, small)));
  const w = Math.round(
    Math.min(
      BOX.max,
      Math.max(BOX.min, textWidth(name, BOX.title, 600) + 2 * pad + 4, sub ? textWidth(sub, small) + 2 * pad + 4 : 0, list.length ? keyW + 14 + valueW + 2 * pad : 0),
    ),
  );
  const headH = BOX.head + (sub ? BOX.sub : 0);
  const lines = list.length + (more ? 1 : 0);
  const h = headH + (lines ? 6 + lines * row + 4 : 0);
  const text = (x, y, s, size, fill, attrs = "") => `<text x="${x}" y="${y}" font-size="${size}" fill="${fill}"${attrs}>${esc(s)}</text>`;
  let body = "";
  list.forEach((_, i) => {
    const y = headH + 6 + i * row + 10;
    body += text(pad, y, keys[i], small, theme.muted);
    body += numeric ? text(w - pad, y, values[i], small, theme.fg, ' text-anchor="end"') : text(pad + keyW + 14, y, values[i], small, theme.fg);
  });
  if (more) body += text(pad, headH + 6 + list.length * row + 10, `… ${more} more`, small, theme.muted, ' font-style="italic"');
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${w * 2}" height="${h * 2}" viewBox="0 0 ${w} ${h}" font-family='${FONT}'>` +
    `<rect width="${w}" height="${h}" fill="${theme.bg}"/>` +
    `<rect width="${w}" height="${headH}" fill="${color}" fill-opacity="0.18"/>` +
    `<rect width="4" height="${headH}" fill="${color}"/>` +
    text(pad + 4, 16, name, BOX.title, theme.fg, ' font-weight="600"') +
    (sub ? text(pad + 4, 31, sub, small, theme.muted, ' font-style="italic"') : "") +
    (lines ? `<line x1="0" x2="${w}" y1="${headH}" y2="${headH}" stroke="${color}" stroke-opacity="0.55"/>` : "") +
    body +
    `</svg>`;
  const out = { w, h, svg: `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}` };
  if (boxCache.size > 4000) boxCache.clear();
  boxCache.set(key, out);
  return out;
}

/** What an entity's box shows: its name, its types, its properties. A schema's class shows its count. */
function entityBox(n, color) {
  const subtitle =
    n.kind === "class"
      ? n.count == null
        ? "class"
        : `${n.count.toLocaleString("en-US")} instance${n.count === 1 ? "" : "s"}`
      : n.types?.length
        ? n.types.map(local).join(", ")
        : { bnode: "blank node", literal: "literal" }[n.kind] ?? "";
  const title = n.label || (n.iri ? local(n.iri) : n.id);
  const rows = n.props.filter(([, v]) => v !== n.label).map(([p, v]) => [shortName(p), v]);
  return { title, subtitle, rows, color, numeric: n.kind === "class" };
}

/**
 * An edge between boxes. One from a box back to itself loops round the box's
 * top-right corner: cytoscape puts a loop's control points 1.4 × its step from
 * the node's centre, so the step grows with the box, or the loop is lost inside it.
 */
function boxEdge(id, source, target, label, sizes) {
  const box = source === target ? sizes.get(source) : null;
  if (!box) return { group: "edges", data: { id, source, target, label } };
  return { group: "edges", data: { id, source, target, label, loop: Math.round((Math.max(box.w, box.h) / 2 + 36) / 1.4) }, classes: "self" };
}

/** The boxes diagram: a box per entity, its properties inside. */
function boxElements(theme) {
  const els = [];
  const sizes = new Map();
  const { nodes, aggregates, edges } = drawn();
  for (const n of nodes) {
    const k = groupKey(n);
    const color = S.groups.get(k)?.color ?? OTHER;
    const box = boxImage(entityBox(n, color), theme);
    sizes.set(n.id, box);
    els.push({ group: "nodes", data: { id: n.id, label: nodeLabel(n), color, gk: k, w: box.w, h: box.h, svg: box.svg }, classes: "card" });
  }
  // An aggregate's box lists its members.
  for (const a of aggregates) {
    const k = groupKey(a.members[0]);
    const color = S.groups.get(k)?.color ?? OTHER;
    const rows = a.members.map((m) => [m.types?.[0] ? local(m.types[0]) : "", nodeLabel(m)]);
    const box = boxImage({ title: a.label, subtitle: `${a.members.length} aggregated`, rows, color }, theme);
    sizes.set(a.id, box);
    els.push({ group: "nodes", data: { id: a.id, label: a.label, color, gk: k, w: box.w, h: box.h, svg: box.svg }, classes: "card agg" });
  }
  for (const e of edges) els.push(boxEdge(e.id, e.source, e.target, e.label, sizes));
  return els;
}

/**
 * The classes diagram: the view's entities folded into their (first)
 * rdf:type — a box per class listing the properties its members carry and
 * how many do, a line per predicate between two classes.
 */
function classElements(theme) {
  const classes = new Map(); // class IRI (or "(untyped)") -> its box
  const classOf = new Map(); // entity id -> class key
  for (const n of S.nodes.values()) {
    if (n.kind === "literal") continue;
    const type = n.types?.[0] ?? null;
    const key = type ?? "(untyped)";
    let c = classes.get(key);
    if (!c) classes.set(key, (c = { id: `class:${key}`, iri: type, label: type ? local(type) : "(untyped)", members: [], props: new Map() }));
    c.members.push(n);
    for (const [p] of n.props) c.props.set(p, (c.props.get(p) ?? 0) + 1);
    classOf.set(n.id, key);
  }
  const links = new Map();
  for (const e of S.edges.values()) {
    const a = classOf.get(e.source);
    const b = classOf.get(e.target);
    if (a === undefined || b === undefined) continue;
    const p = e.p ?? e.label;
    const k = `${a}\u0001${p}\u0001${b}`;
    const link = links.get(k) ?? { id: `cl:${k}`, source: classes.get(a).id, target: classes.get(b).id, p, n: 0 };
    link.n += 1;
    links.set(k, link);
  }
  const els = [];
  const sizes = new Map();
  for (const c of classes.values()) {
    const k = groupKey(c.members[0]);
    const color = S.groups.get(k)?.color ?? OTHER;
    const rows = [...c.props].sort((x, y) => y[1] - x[1]).map(([p, n]) => [shortName(p), String(n)]);
    const box = boxImage({ title: c.label, subtitle: `${c.members.length} in this view`, rows, color, numeric: true }, theme);
    els.push({ group: "nodes", data: { id: c.id, label: c.label, color, gk: k, w: box.w, h: box.h, svg: box.svg }, classes: "card" });
    sizes.set(c.id, box);
    S.classBoxes.set(c.id, c);
  }
  for (const l of links.values()) els.push(boxEdge(l.id, l.source, l.target, `${shortName(l.p)}${l.n > 1 ? ` ×${l.n}` : ""}`, sizes));
  return els;
}

function styleSheet() {
  const css = getComputedStyle(document.documentElement);
  const v = (name) => css.getPropertyValue(name).trim();
  const square = S.options.edge_style === "square";
  // Square lines leave and enter boxes vertically when the layout runs top-down.
  const lines = square
    ? { "curve-style": "round-taxi", "taxi-direction": S.options.layout === "layered" ? "vertical" : "auto", "taxi-turn": "50%", "taxi-turn-min-distance": 12, "taxi-radius": 6 }
    : { "curve-style": S.options.edge_style === "straight" ? "straight" : "bezier" };
  return [
    {
      selector: "node",
      style: {
        "background-color": "data(color)",
        label: S.options.labels ? "data(label)" : "",
        shape: S.options.node_shape === "square" ? "round-rectangle" : "ellipse",
        width: 16,
        height: 16,
        "font-size": 9,
        color: v("--fg"),
        "text-valign": "bottom",
        "text-margin-y": 3,
        "text-outline-width": 2,
        "text-outline-color": v("--bg"),
        "text-max-width": 140,
        "text-wrap": "ellipsis",
        "min-zoomed-font-size": 6,
      },
    },
    {
      selector: "node.box",
      style: {
        "background-opacity": 0.07,
        "background-color": "data(color)",
        "border-width": 1,
        "border-color": "data(color)",
        shape: "round-rectangle",
        label: "data(label)",
        "text-valign": "top",
        "font-size": 11,
        "font-weight": 600,
        padding: 14,
      },
    },
    { selector: "node.literal, node.lit", style: { shape: "round-rectangle", width: 10, height: 10, "font-size": 8 } },
    { selector: "node.bnode", style: { shape: "diamond" } },
    {
      selector: "node.agg",
      style: {
        shape: "round-rectangle",
        width: "data(size)",
        height: "data(size)",
        "border-width": 4,
        "border-style": "double",
        "border-color": "data(color)",
        "background-opacity": 0.85,
        "font-weight": 600,
      },
    },
    {
      selector: "node.card",
      style: {
        shape: "round-rectangle",
        width: "data(w)",
        height: "data(h)",
        "background-color": v("--bg"),
        "background-image": "data(svg)",
        "background-width": "100%",
        "background-height": "100%",
        "background-clip": "node",
        "border-width": 1,
        "border-color": "data(color)",
        label: "",
      },
    },
    { selector: "node.card.agg", style: { "border-width": 4, "border-style": "double" } },
    {
      selector: "edge",
      style: {
        width: 1,
        "line-color": v("--edge"),
        "target-arrow-shape": "triangle",
        "target-arrow-color": v("--edge"),
        "arrow-scale": 0.7,
        ...lines,
        label: S.options.edge_labels ? "data(label)" : "",
        "font-size": 7,
        color: v("--muted"),
        "text-rotation": square ? "none" : "autorotate",
        "text-outline-width": 2,
        "text-outline-color": v("--bg"),
        "min-zoomed-font-size": 6,
      },
    },
    { selector: "edge.litedge", style: { "line-style": "dashed", "target-arrow-shape": "none" } },
    {
      selector: "edge.self",
      style: { "curve-style": "bezier", "loop-direction": "45deg", "loop-sweep": "-60deg", "control-point-step-size": "data(loop)", "text-rotation": "none" },
    },
    { selector: "node:selected", style: { "border-width": 3, "border-color": v("--accent") } },
    { selector: ".hl", style: { "border-width": 3, "border-color": v("--hl"), "z-index": 10 } },
    { selector: ".faded", style: { opacity: 0.15 } },
    { selector: ".hidden", style: { display: "none" } },
  ];
}

function applyStyle() {
  S.cy?.style(styleSheet());
  styleHive();
}

function rebuild(layout) {
  const cy = S.cy;
  if (!cy) return;
  const positions = new Map(cy.nodes().map((n) => [n.id(), { ...n.position() }]));
  cy.elements().remove();
  cy.add(elements());
  for (const [id, pos] of positions) cy.getElementById(id).position(pos);
  // A new aggregate starts amid its members; members out of one, where it was.
  for (const a of S.aggregates.values()) {
    const at = a.members.map((m) => positions.get(m)).filter(Boolean);
    if (!positions.has(a.id) && at.length) {
      cy.getElementById(a.id).position({ x: at.reduce((s, p) => s + p.x, 0) / at.length, y: at.reduce((s, p) => s + p.y, 0) / at.length });
    }
  }
  for (const [id, pos] of S.placeAt) {
    if (!positions.has(id)) cy.getElementById(id).position({ x: pos.x + (Math.random() - 0.5) * 60, y: pos.y + (Math.random() - 0.5) * 60 });
  }
  S.placeAt.clear();
  applyStyle();
  applyHidden();
  applyHighlight();
  applySearch();
  if (layout) runLayout(positions.size === 0);
  renderLayers();
  renderDetails();
  renderRows();
  styleHive();
}

/** New elements into the drawing, keeping what is there in place. */
function grow(nodes, edges) {
  const cy = S.cy;
  if (!cy) return;
  if (S.options.group_boxes || S.options.literals_as_nodes || diagramOf() !== "graph" || S.aggregates.size) return rebuild(true);
  const fresh = [];
  for (const n of nodes) {
    const el = cy.getElementById(n.id);
    if (el.nonempty()) el.data("label", nodeLabel(S.nodes.get(n.id)));
    else fresh.push({ group: "nodes", data: { id: n.id, label: nodeLabel(S.nodes.get(n.id)), color: OTHER, gk: "" }, classes: n.kind === "iri" ? "" : n.kind });
  }
  for (const e of edges) {
    if (cy.getElementById(e.id).empty()) fresh.push({ group: "edges", data: { id: e.id, source: e.source, target: e.target, label: e.label } });
  }
  if (fresh.length) {
    // Born next to the node they hang off, so the layout has less to do.
    const added = cy.add(fresh);
    added.nodes().forEach((n) => {
      const neighbour = n.neighborhood("node").filter((m) => !added.contains(m))[0];
      if (neighbour) n.position({ x: neighbour.position("x") + (Math.random() - 0.5) * 80, y: neighbour.position("y") + (Math.random() - 0.5) * 80 });
    });
  }
  refresh();
  if (fresh.length) runLayout(false);
}

/** Labels, colours and layers again, after names, types or grouping change. */
function refresh() {
  if (!S.cy) return;
  // A box grows with its name and types, and a class with its members: draw them again.
  if (diagramOf() !== "graph") return rebuild(true);
  if (S.options.group_boxes || S.aggregates.size) return rebuild(false);
  computeGroups();
  S.cy.nodes().not(".box, .lit").forEach((el) => {
    const n = S.nodes.get(el.id());
    if (!n) return;
    const k = groupKey(n);
    el.data({ label: nodeLabel(n), gk: k, color: S.groups.get(k)?.color ?? OTHER });
  });
  applyHidden();
  renderLayers();
  renderDetails();
}

function runLayout(randomize) {
  const cy = S.cy;
  if (!cy || cy.nodes().empty()) return;
  const name = S.options.layout || "cose";
  const many = cy.nodes().length > 400;
  if (name !== "hive") clearHive();
  if (name === "hive") return hive(cy.elements(":visible"), !many);
  if (name === "layered") return layered(cy.elements(":visible"), !many);
  const opts = { name, fit: true, padding: 30, animate: !many, animationDuration: 350, nodeDimensionsIncludeLabels: true };
  if (name === "cose") Object.assign(opts, { randomize, idealEdgeLength: 70, nodeRepulsion: 9000, gravity: 0.25, numIter: many ? 600 : 1200, componentSpacing: 60 });
  if (name === "concentric") Object.assign(opts, { minNodeSpacing: 18, concentric: (n) => n.degree(), levelWidth: () => 2 });
  if (name === "breadthfirst") Object.assign(opts, { directed: true, spacingFactor: 1.1 });
  cy.elements(":visible").layout(opts).run();
}

/**
 * Layers, top down, for what is linked (dagre, sized to each box); what is
 * not linked is packed in rows underneath rather than strung along one rank.
 */
function layered(eles, animate) {
  const nodes = eles
    .nodes()
    .filter((n) => !n.isParent())
    .map((n) => ({ id: n.id(), ...n.layoutDimensions({ nodeDimensionsIncludeLabels: true }) }));
  const edges = eles.edges().map((e) => ({ id: e.id(), source: e.source().id(), target: e.target().id() }));
  const { pos, error } = layeredPositions(nodes, edges);
  if (error) setNote(`layers: ${error.message} — packed in rows instead`);
  eles.layout({ name: "preset", positions: (n) => pos[n.id()] ?? n.position(), fit: true, padding: 30, animate, animationDuration: 350 }).run();
}

/**
 * The hive plot: nodes on radial axes, ordered along each by a measure, edges
 * bent between the axes (hive.js has the arithmetic). cytoscape only places the
 * nodes and bends the edges, so selecting, hovering, panning and zooming are
 * what they are in every other layout.
 */
function hive(eles, animate) {
  const nodes = eles
    .nodes()
    .filter((n) => !n.isParent())
    .map((n) => {
      const m = S.nodes.get(n.id());
      return { id: n.id(), kind: m?.kind ?? "other", type: m?.types?.[0] ? local(m.types[0]) : null, label: m?.label ?? n.data("label") };
    });
  const edges = eles.edges().map((e) => ({ id: e.id(), source: e.source().id(), target: e.target().id() }));
  S.hive = hivePositions(nodes, edges, { by: S.options.hive_axes || "auto", order: S.options.hive_order || "degree" });
  styleHive();
  eles.layout({ name: "preset", positions: (n) => S.hive.pos[n.id()] ?? n.position(), fit: true, padding: 60, animate, animationDuration: 350, stop: drawAxes }).run();
  setNote(`hive: axes by ${S.hive.by}, ${S.options.hive_order === "name" ? "ordered by name" : `ordered by ${S.options.hive_order || "degree"}`}`);
}

/** Each edge bent toward where its axes meet; the other layouts' lines come back when the hive goes. */
function styleHive() {
  const cy = S.cy;
  if (!cy || S.options.layout !== "hive" || !S.hive) return;
  cy.edges().forEach((e) => {
    const d = S.hive.bow[e.id()];
    if (d !== undefined) e.style({ "curve-style": "unbundled-bezier", "control-point-distances": [d], "control-point-weights": [0.5] });
  });
}

function clearHive() {
  if (!S.hive) return;
  S.hive = null;
  S.cy?.edges().removeStyle("curve-style control-point-distances control-point-weights");
  drawAxes();
}

/** The axes and their names, over the drawing, following pan and zoom. */
function drawAxes() {
  const svg = $("hive-axes");
  const cy = S.cy;
  if (!S.hive || !cy || S.options.view !== "graph" || S.options.layout !== "hive") {
    svg.toggleAttribute("hidden", true); // an SVG element has no .hidden
    return;
  }
  svg.toggleAttribute("hidden", false);
  const w = cy.width();
  const h = cy.height();
  svg.setAttribute("width", w);
  svg.setAttribute("height", h);
  const { x: px, y: py } = cy.pan();
  const z = cy.zoom();
  const at = (r, a) => [r * Math.cos(a) * z + px, r * Math.sin(a) * z + py];
  const ns = "http://www.w3.org/2000/svg";
  svg.replaceChildren(
    ...S.hive.axes.flatMap((a) => {
      const [x1, y1] = at(a.from, a.angle);
      const [x2, y2] = at(a.to, a.angle);
      const [tx, ty] = at(a.to + 26 / z, a.angle);
      const line = document.createElementNS(ns, "line");
      for (const [k, v] of Object.entries({ x1, y1, x2, y2 })) line.setAttribute(k, v);
      const text = document.createElementNS(ns, "text");
      text.setAttribute("x", tx);
      text.setAttribute("y", ty);
      text.textContent = `${a.key} (${a.count})`;
      return [line, text];
    }),
  );
}

function applyHidden() {
  const cy = S.cy;
  if (!cy) return;
  cy.nodes().forEach((n) => {
    const hidden = S.hiddenGroups.has(n.data("gk")) || S.hiddenNodes.has(n.id());
    n.toggleClass("hidden", hidden);
  });
  const p = $("hidden-nodes");
  p.hidden = !S.hiddenNodes.size;
  p.innerHTML = `${S.hiddenNodes.size} node(s) hidden · <a href="#" id="unhide">show again</a>`;
  $("unhide")?.addEventListener("click", (e) => {
    e.preventDefault();
    unhideAll();
  });
}

function setHighlight(ids) {
  S.highlight = new Set(ids);
  applyHighlight();
  const first = S.cy?.getElementById(ids[0]);
  if (first?.nonempty()) S.cy.animate({ center: { eles: first }, duration: 300 });
}

function applyHighlight() {
  S.cy?.nodes().forEach((n) => n.toggleClass("hl", S.highlight.has(n.id())));
}

function focusNode(id) {
  const el = S.cy?.getElementById(id);
  if (!el || el.empty()) return;
  S.cy.$(":selected").unselect();
  el.select();
  S.cy.animate({ center: { eles: el }, zoom: Math.max(S.cy.zoom(), 1.2), duration: 350 });
}

function applySearch() {
  const q = S.search.trim().toLowerCase();
  const cy = S.cy;
  if (!cy) return 0;
  if (!q) {
    cy.elements().removeClass("faded");
    return 0;
  }
  let hits = 0;
  cy.nodes().not(".box").forEach((el) => {
    const n = S.nodes.get(el.id());
    const text = `${el.data("label")} ${n?.iri ?? ""}`.toLowerCase();
    const hit = text.includes(q);
    if (hit) hits += 1;
    el.toggleClass("faded", !hit);
  });
  cy.edges().addClass("faded");
  return hits;
}

// --- side panel -----------------------------------------------------------------------

function renderLayers() {
  const ul = document.querySelector("#layers ul");
  if (S.options.group_by === "none") {
    ul.innerHTML = `<li><span class="name">No layers — choose “by type” or another grouping</span></li>`;
    return;
  }
  ul.innerHTML = [...S.groups]
    .map(
      ([k, g]) =>
        `<li title="${esc(k)}"><input type="checkbox" data-layer="${esc(k)}" ${S.hiddenGroups.has(k) ? "" : "checked"}>` +
        `<span class="sw" style="background:${g.color}"></span><span class="name">${esc(k || "—")}</span><span class="n">${g.count}</span></li>`,
    )
    .join("");
}

function renderDetails() {
  const body = document.querySelector("#details .body");
  const picked = S.selection;
  if (!picked.length) {
    body.innerHTML = `<p class="hint">Click a node to see it. Double-click to expand it. Shift-drag selects several.</p>`;
    return;
  }
  if (picked.length > 1) {
    body.innerHTML =
      `<p class="title">${picked.length} nodes selected</p>` +
      `<div class="actions"><button type="button" data-act="aggregate">Aggregate</button>${chatty() ? `<button type="button" data-act="send">Send to chat</button>` : ""}` +
      `<button type="button" data-act="hide">Hide</button><button type="button" data-act="remove">Remove</button>` +
      `${chatty() ? `<button type="button" data-act="ask">Ask Claude</button>` : ""}</div>` +
      `<p class="hint">${picked.slice(0, 12).map((s) => esc(s.label)).join(" · ")}${picked.length > 12 ? " …" : ""}</p>`;
    return;
  }
  const c = S.classBoxes.get(picked[0].id);
  if (c) {
    body.innerHTML = classDetails(c);
    return;
  }
  const a = S.aggregates.get(picked[0].id);
  if (a) {
    body.innerHTML =
      `<p class="title">${esc(a.label)}</p><div class="types">an aggregate of ${a.members.length} nodes</div>` +
      `<div class="actions"><button type="button" data-act="ungroup">Ungroup</button>${chatty() ? `<button type="button" data-act="send">Send to chat</button>` : ""}` +
      `<button type="button" data-act="remove">Remove</button></div>` +
      `<p class="hint">${a.members.slice(0, 20).map((m) => esc(nameOf(m))).join(" · ")}${a.members.length > 20 ? " …" : ""}</p>`;
    return;
  }
  const n = S.nodes.get(picked[0].id);
  if (!n) return;
  const degree = S.cy?.getElementById(n.id).degree() ?? 0;
  const props = n.props.slice(0, MAX_PROPS).map(([p, v]) => `<dt title="${esc(p)}">${esc(local(p))}</dt><dd>${esc(trunc(v, 300))}</dd>`).join("");
  body.innerHTML =
    `<p class="title">${esc(n.label || local(n.iri) || n.id)}</p>` +
    (n.iri ? iriLine(n.iri) : "") +
    (n.types.length ? `<div class="types">${n.types.map((t) => esc(local(t))).join(", ")}</div>` : "") +
    (n.kind === "class" && n.count != null ? `<div class="types">${n.count.toLocaleString("en-US")} instance${n.count === 1 ? "" : "s"}</div>` : "") +
    `<div class="actions">` +
    (n.kind === "iri" ? `<button type="button" data-act="expand">Expand</button>` : "") +
    `<button type="button" data-act="hide">Hide</button>${chatty() ? `<button type="button" data-act="ask">Ask Claude</button>` : ""}</div>` +
    `<p class="hint">${degree} link(s) in the view${n.props.length > MAX_PROPS ? ` · ${n.props.length} properties, first ${MAX_PROPS} shown` : ""}</p>` +
    (props ? `<dl>${props}</dl>` : "");
}

/** A class box of the classes diagram: its members and the properties they carry. */
function classDetails(c) {
  const props = [...c.props]
    .sort((x, y) => y[1] - x[1])
    .slice(0, MAX_PROPS)
    .map(([p, n]) => `<dt title="${esc(p)}">${esc(shortName(p))}</dt><dd>${n} of ${c.members.length}</dd>`)
    .join("");
  return (
    `<p class="title">${esc(c.label)}</p>` +
    (c.iri ? iriLine(c.iri) : "") +
    `<div class="actions"><button type="button" data-act="members">Show members</button>${chatty() ? `<button type="button" data-act="ask">Ask Claude</button>` : ""}</div>` +
    `<p class="hint">${c.members.length} in the view: ${c.members.slice(0, 12).map((m) => esc(nodeLabel(m))).join(" · ")}${c.members.length > 12 ? " …" : ""}</p>` +
    (props ? `<dl>${props}</dl>` : "")
  );
}

/** From a class to its members: the boxes diagram, with them highlighted. */
function showMembers(c) {
  const ids = c.members.map((m) => m.id);
  setDiagram("boxes");
  setHighlight(ids);
  setTimeout(() => {
    const eles = S.cy.collection(ids.map((id) => S.cy.getElementById(id)).filter((el) => el.nonempty()));
    if (eles.nonempty()) S.cy.animate({ fit: { eles, padding: 40 }, duration: 300 });
  }, 450);
}

// --- the right-click menu, and what it does ------------------------------------------------

function nameOf(id) {
  const a = S.aggregates.get(id);
  if (a) return a.label;
  const c = S.classBoxes.get(id);
  if (c) return c.label;
  const n = S.nodes.get(id);
  return n ? nodeLabel(n) : id;
}

/** The nodes a menu acts on: the selection, when the node is part of it; else the node alone. */
function targetsOf(el) {
  const picked = S.cy.$("node:selected").not(".box, .lit");
  return picked.length > 1 && picked.contains(el) ? picked.map((n) => n.id()) : [el.id()];
}

/** Separators only between items: none first, last, or twice. */
function tidy(items) {
  const out = [];
  for (const it of items) {
    if (!it || (it === "-" && (!out.length || out.at(-1) === "-"))) continue;
    out.push(it);
  }
  while (out.at(-1) === "-") out.pop();
  return out;
}

function nodeMenu(el) {
  const ids = targetsOf(el);
  const many = ids.length > 1;
  const count = many ? ` ${ids.length}` : "";
  const one = many ? null : ids[0];
  const c = one && S.classBoxes.get(one);
  if (c) {
    return tidy([
      { label: "Show members", hint: `${c.members.length}`, run: () => showMembers(c) },
      chatty() && { label: "Send to chat", run: () => sendToChat(ids) },
      "-",
      { label: "Hide", run: () => hide(ids) },
    ]);
  }
  const a = one && S.aggregates.get(one);
  const n = one && S.nodes.get(one);
  const expandable = ids.filter((id) => S.nodes.get(id)?.kind === "iri");
  // A node's layer, aggregated: every node of its type (or namespace, or value).
  const layer = n && S.options.group_by !== "none" ? groupKey(n) : null;
  const grouped = new Set([...S.aggregates.values()].flatMap((g) => g.members));
  const sameLayer = layer === null ? [] : [...S.nodes.values()].filter((m) => groupKey(m) === layer && !grouped.has(m.id)).map((m) => m.id);
  return tidy([
    expandable.length && { label: `Expand${many ? ` ${expandable.length}` : ""}`, hint: "add its links", run: () => expandAll(expandable) },
    { label: `Compress${count}`, hint: "drop what hangs only off it", run: () => compress(ids) },
    { label: many ? "Select their neighbours" : "Select neighbours", run: () => selectNeighbours(ids) },
    n && (n.label || n.iri) && { label: "Search its name", hint: "in the dataset", run: () => searchDataset(n.label || local(n.iri)) },
    chatty() && { label: `Send${count} to chat`, run: () => sendToChat(ids) },
    { label: "Select several…", hint: "click to add", run: () => selectMode(true, ids) },
    "-",
    a
      ? { label: "Ungroup", hint: `${a.members.length} nodes`, run: () => edit({ ungroup: a.id }) }
      : many
        ? { label: `Aggregate${count}`, hint: "draw as one", run: () => aggregate(ids) }
        : sameLayer.length > 1 && { label: `Aggregate all ${layer || "nodes"}`, hint: `${sameLayer.length} nodes`, run: () => aggregate(sameLayer, layer) },
    "-",
    { label: `Hide${count}`, hint: "until shown again", run: () => hide(ids) },
    { label: `Remove${count}`, hint: "from the view", danger: true, run: () => removeNodes(ids) },
    webUrl(n?.iri) && { label: "Open in browser", run: () => openUrl(n.iri).catch(() => {}) },
  ]);
}

function backgroundMenu() {
  return tidy([
    ABOUT && { label: "About rete…", hint: ABOUT.version, run: () => ($("about").hidden = false) },
    "-",
    { label: S.selecting ? "Stop selecting" : "Select several…", hint: "click to add", run: () => selectMode(!S.selecting) },
    { label: "Select all", run: () => S.cy.nodes(":visible").not(".box, .lit").select() },
    S.hiddenNodes.size && { label: `Show ${S.hiddenNodes.size} hidden`, run: unhideAll },
    S.aggregates.size && { label: "Ungroup all", run: () => edit({ ungroup: "all" }) },
    "-",
    { label: "Fit to view", run: () => S.cy.fit(S.cy.elements(":visible"), 30) },
    { label: "Lay out again", run: () => runLayout(true) },
  ]);
}

function openMenu(title, items, x, y) {
  const menu = $("ctx");
  S.menu = items;
  menu.innerHTML =
    (title ? `<div class="ctx-title">${esc(title)}</div>` : "") +
    items
      .map((it, i) =>
        it === "-"
          ? "<hr>"
          : `<button type="button" role="menuitem" data-i="${i}"${it.danger ? ' class="danger"' : ""}>` +
            `<span>${esc(it.label)}</span>${it.hint ? `<span class="hint">${esc(it.hint)}</span>` : ""}</button>`,
      )
      .join("");
  menu.hidden = false;
  // Inside the frame, however near its edge the click was.
  const { width, height } = menu.getBoundingClientRect();
  menu.style.left = `${Math.max(4, Math.min(x, window.innerWidth - width - 4))}px`;
  menu.style.top = `${Math.max(4, Math.min(y, window.innerHeight - height - 4))}px`;
}

/** The About panel: version and build, author, links, credits. */
function renderAbout() {
  if (!ABOUT) return;
  const { version, build, author, license, links, cite, credits, data } = ABOUT;
  const link = (url, text) => (webUrl(url) ? `<a href="#" data-open="${esc(url)}">${esc(text)}</a>` : esc(text));
  $("about-open").textContent = `rete ${version}`;
  $("about").innerHTML =
    `<p class="title">rete graph view <span class="v">${esc(version)}</span></p>` +
    `<p class="muted">${esc([build.commit ? `build ${build.commit}` : build.label ? null : "build of unknown commit", build.date, build.label].filter(Boolean).join(" · "))}</p>` +
    `<p>By ${link(author.orcid, author.name)} · ${esc(license)}</p>` +
    `<p class="links">${link(links.documentation, "Documentation")} · ${link(links.playground, "Playground")} · ` +
    `${link(links.repository, "Source")} · ${link(links.issues, "Report an issue")} · ${link(links.releases, "Releases")}` +
    `${cite?.url ? ` · ${link(cite.url, "Cite")}` : ""}</p>` +
    `<p class="muted">Built with ${credits.map((c) => `${link(c.url, `${c.name}${c.version ? ` ${c.version}` : ""}`)} (${esc(c.license)})`).join(", ")}.</p>` +
    `<p class="muted">${esc(data)}</p>`;
}

function closeMenu() {
  $("ctx").hidden = true;
  S.menu = [];
}

async function edit(change) {
  try {
    apply(await call("graph_view_edit", { view_id: S.viewId, since: S.version, ...change }));
  } catch (error) {
    setNote(`edit: ${error.message}`);
  }
}

async function expandAll(ids) {
  for (const id of ids.slice(0, 10)) await expand(id);
  if (ids.length > 10) setNote(`expanded the first 10 of ${ids.length}`);
}

/** What hangs only off these nodes: linked to them, and to nothing else in the view. */
function leavesOf(ids) {
  const set = new Set(ids.flatMap((id) => S.aggregates.get(id)?.members ?? [id]));
  const links = new Map();
  const link = (a, b) => (links.get(a) ?? links.set(a, new Set()).get(a)).add(b);
  for (const e of S.edges.values()) {
    link(e.source, e.target);
    link(e.target, e.source);
  }
  const out = new Set();
  for (const id of set) {
    for (const m of links.get(id) ?? []) if (!set.has(m) && [...links.get(m)].every((x) => set.has(x))) out.add(m);
  }
  return [...out];
}

/** Compress: drop what hangs only off the nodes; Expand brings it back. */
function compress(ids) {
  const leaves = leavesOf(ids);
  if (!leaves.length) return setNote(`nothing hangs only off ${ids.length > 1 ? `these ${ids.length}` : nameOf(ids[0])}`);
  setNote(`compressed: ${leaves.length} node(s) taken out — Expand brings them back`);
  return edit({ remove: leaves });
}

function selectNeighbours(ids) {
  const els = S.cy.collection(ids.map((id) => S.cy.getElementById(id)));
  els.union(els.neighborhood("node")).not(".box, .lit").select();
}

function hide(ids) {
  for (const id of ids) S.hiddenNodes.add(id);
  S.cy.$(":selected").unselect();
  applyHidden();
  markState();
}

function unhideAll() {
  S.hiddenNodes.clear();
  applyHidden();
  markState();
}

/** Out of the view, for good (Expand or a query brings them back); an aggregate takes its members. */
function removeNodes(ids) {
  const all = ids.flatMap((id) => S.aggregates.get(id)?.members ?? (S.nodes.has(id) ? [id] : []));
  return all.length ? edit({ remove: all }) : undefined;
}

/** Draw nodes as one, named after the type they share, if they do. */
function aggregate(ids, label) {
  const types = ids.map((id) => S.nodes.get(id)?.types?.[0]);
  const shared = types[0] && types.every((t) => t === types[0]) ? local(types[0]) : null;
  const name = label || shared;
  return edit({ aggregate: { ids, ...(name ? { label: name } : {}) } });
}

/** The nodes as text for the conversation: name, IRI, types, a few properties; an aggregate, its members. */
function describe(ids) {
  const line = (id) => {
    const a = S.aggregates.get(id);
    if (a) return `- ${a.label}: an aggregate of ${a.members.length} — ${a.members.slice(0, 10).map(nameOf).join(", ")}${a.members.length > 10 ? ", …" : ""}`;
    const c = S.classBoxes.get(id);
    if (c) return `- the class ${c.label}${c.iri ? ` <${c.iri}>` : ""}, ${c.members.length} in the view`;
    const n = S.nodes.get(id);
    if (!n) return `- ${id}`;
    const props = n.props
      .filter(([, v]) => v !== n.label)
      .slice(0, 6)
      .map(([p, v]) => `${shortName(p)}: ${trunc(v, 80)}`)
      .join("; ");
    return `- ${nodeLabel(n)}${n.iri ? ` <${n.iri}>` : ""}${n.types.length ? ` (${n.types.map(local).join(", ")})` : ""}${props ? ` — ${props}` : ""}`;
  };
  const head = `From graph view ${S.viewId} on ${S.dataset}${ids.length > 1 ? `, ${ids.length} nodes` : ""}:`;
  return [head, ...ids.slice(0, 25).map(line), ...(ids.length > 25 ? [`… and ${ids.length - 25} more`] : [])].join("\n");
}

function sendToChat(ids) {
  S.app
    .sendMessage({ role: "user", content: [{ type: "text", text: describe(ids) }] })
    .catch((error) => setNote(`send: ${error.message}`));
}

/**
 * Select several: a click adds a node or takes it out again, and a drag on
 * the background draws a box over many (panning waits until the mode ends).
 */
function selectMode(on, ids = []) {
  S.selecting = on;
  S.cy.selectionType(on ? "additive" : "single");
  S.cy.userPanningEnabled(!on);
  $("selecting").hidden = !on;
  for (const id of ids) S.cy.getElementById(id).select();
  $("selecting-count").textContent = S.selection.length;
}

/** Find entities by name in the dataset, as Enter in the search box does. */
async function searchDataset(text) {
  if (!text?.trim() || !S.viewId) return;
  $("search").value = text;
  setNote(`finding “${text.trim()}” in ${S.dataset}…`);
  try {
    const r = await call("graph_view_search", { view_id: S.viewId, text: text.trim(), since: S.version });
    apply(r);
    setNote(r.note ?? `${r.found ?? 0} found in the graph`);
  } catch (error) {
    setNote(`search: ${error.message}`);
  }
}

/** Draw the graph another way; each diagram comes with its own layout and lines. */
function setDiagram(diagram) {
  if (diagramOf() === diagram) return;
  Object.assign(S.options, diagramLook(diagram));
  S.cy?.$(":selected").unselect();
  syncControls();
  rebuild(true);
  markState();
}

const tellClaude = debounce(() => {
  if (!S.app?.getHostCapabilities()?.updateModelContext) return;
  const picked = S.selection;
  const text = picked.length
    ? `In graph view ${S.viewId} (${S.dataset}) the user has selected ${picked.length} node(s): ` +
      picked.slice(0, 20).map((s) => `${s.label}${s.iri ? ` <${s.iri}>` : ""}`).join("; ")
    : `In graph view ${S.viewId} (${S.dataset}) nothing is selected.`;
  S.app.updateModelContext({ content: [{ type: "text", text }], structuredContent: { view_id: S.viewId, selection: picked.slice(0, 50) } }).catch(() => {});
}, 700);

function onSelectionChanged() {
  if (!S.cy) return;
  S.selection = S.cy
    .$("node:selected")
    .not(".box, .lit")
    .map((el) => {
      const c = S.classBoxes.get(el.id());
      if (c) return { id: el.id(), iri: c.iri, label: c.label, class_members: c.members.length };
      const a = S.aggregates.get(el.id());
      if (a) return { id: el.id(), iri: null, label: a.label, aggregate_of: a.members.length };
      const n = S.nodes.get(el.id());
      return { id: el.id(), iri: n?.iri ?? null, label: n ? nodeLabel(n) : el.id() };
    });
  renderDetails();
  $("selecting-count").textContent = S.selection.length;
  markState();
  tellClaude();
}

async function expand(id) {
  const n = S.nodes.get(id);
  if (!n?.iri) return;
  setNote(`expanding ${nodeLabel(n)}…`);
  try {
    apply(await call("graph_view_expand", { view_id: S.viewId, iri: n.iri, since: S.version }));
    setNote("");
  } catch (error) {
    setNote(`expand: ${error.message}`);
  }
}

/** Labels and types for the nodes that have none yet, a batch at a time. */
const requestLabels = debounce(async () => {
  if (!S.viewId) return;
  const ids = [...S.nodes.values()].filter((n) => n.kind === "iri" && !n.label && !S.labelled.has(n.id)).map((n) => n.id);
  for (let i = 0; i < ids.length; i += 150) {
    const batch = ids.slice(i, i + 150);
    for (const id of batch) S.labelled.add(id);
    try {
      apply(await call("graph_view_labels", { view_id: S.viewId, ids: batch, since: S.version }));
    } catch (error) {
      setNote(`labels: ${error.message}`);
      return;
    }
  }
}, 300);

// --- table -------------------------------------------------------------------------------

/** A cell given only as text (the graph's own edges, an older server): an IRI when it looks like one. */
const textTerm = (s) => {
  const v = String(s);
  return /^(https?|urn|mailto|ftp):\S+$/i.test(v) ? { kind: "iri", value: v } : { kind: "literal", value: v };
};
const termsOf = (vars, rows) => rows.map((r) => Object.fromEntries(vars.filter((v) => r[v] !== "" && r[v] != null).map((v) => [v, textTerm(r[v])])));

/** The rows the table, the cards and the map show: { vars, rows (text), terms (typed), query? }. */
function tableData() {
  if (S.table?.vars?.length) {
    S.table.terms ??= termsOf(S.table.vars, S.table.rows);
    return S.table;
  }
  const vars = ["subject", "predicate", "object"];
  const rows = [...S.edges.values()].map((e) => ({
    subject: nodeLabel(S.nodes.get(e.source) ?? { id: e.source }),
    predicate: e.p ?? e.label,
    object: nodeLabel(S.nodes.get(e.target) ?? { id: e.target }),
  }));
  return { vars, rows, terms: termsOf(vars, rows) };
}

/**
 * The rows as Retelens holds a result (lib/results.js ResultModel: text search, column filters, sort), the
 * column names and notes the query's hints give (lib/hints.js), and how IRIs are shortened (the query's prefixes).
 * Made again only when the rows change.
 */
function richModel() {
  const key = S.table?.vars?.length ? S.table : `edges:${S.edges.size}:${S.nodes.size}`;
  const r = S.rich;
  if (r.key === key) return r;
  const data = tableData();
  const prefixes = new window.RetelensFmt.Prefixes();
  if (data.query) prefixes.learn(data.query);
  r.key = key;
  r.data = data;
  r.short = (iri) => prefixes.shorten(iri);
  r.model = new window.RetelensResults.ResultModel(data.vars, r.short);
  r.model.setRows(data.terms);
  r.colInfo = data.query ? window.RetelensHints.parse(data.query).columns : {};
  return r;
}

/** A column's name: the query's hint (# column ?v: Label — note), else the variable. */
const columnLabel = (v) => S.rich.colInfo?.[v]?.label || v;

function renderTable() {
  const r = richModel();
  const { vars } = r.data;
  const m = r.model;
  const q = $("table-filter").value.trim().toLowerCase();
  m.q = q;
  m.sort = S.sort.col ? { v: S.sort.col, dir: S.sort.desc ? "desc" : "asc" } : null;
  m.changed();
  const shown = m.view();
  const filtered = m.activeCols();
  $("table-count").textContent = `${shown.length} of ${m.rows.length} rows${shown.length > MAX_TABLE ? ` (first ${MAX_TABLE} drawn)` : ""}${filtered.length ? ` · filtered by ${filtered.map(columnLabel).join(", ")}` : ""}`;
  $("table-clear").hidden = !filtered.length;
  const ctx = { short: r.short };
  // the header: the column's name (sorts on click), then a chart of its values (a click on a bar filters the rows)
  const head = el("tr");
  for (const v of vars) {
    const info = r.colInfo[v];
    const th = el("th");
    th.dataset.col = v;
    th.title = `?${v}${info?.note ? ` — ${info.note}` : ""}`;
    const name = el("div", "th-name");
    name.append(el("span", "", columnLabel(v)), el("span", "dir", S.sort.col === v ? (S.sort.desc ? " ▼" : " ▲") : ""));
    th.append(name);
    const stats = window.RetelensStats;
    th.append(stats.chart(stats.summarize(m.rows, v, r.short), { current: m.filters[v], onPick: (f) => (m.setFilter(v, f), renderTable(), renderMapIfShown()) }));
    head.append(th);
  }
  const body = document.createElement("tbody");
  for (const i of shown.slice(0, MAX_TABLE)) {
    const tr = el("tr");
    tr.dataset.first = r.data.rows[i]?.[vars[0]] ?? "";
    for (const v of vars) {
      const td = el("td");
      const t = m.rows[i][v];
      if (t) td.append(window.RetelensCells.render(t, "auto", ctx, { hl: q }));
      tr.append(td);
    }
    body.append(tr);
  }
  const thead = document.createElement("thead");
  thead.append(head);
  $("rows").replaceChildren(thead, body);
  $("csv").hidden = !S.app?.getHostCapabilities()?.downloadFile;
}

function csvText() {
  const { vars, rows } = tableData();
  const cell = (s) => (/[",\r\n]/.test(s) ? `"${String(s).replaceAll('"', '""')}"` : String(s));
  return [vars.map(cell).join(","), ...rows.map((r) => vars.map((v) => cell(r[v] ?? "")).join(","))].join("\r\n") + "\r\n";
}

function showTab(tab) {
  S.options.view = ["table", "cards", "map"].includes(tab) ? tab : "graph";
  syncMapTab();
  for (const b of document.querySelectorAll(".tabs button")) {
    b.classList.toggle("on", b.dataset.tab === S.options.view);
    b.setAttribute("aria-selected", String(b.dataset.tab === S.options.view));
  }
  $("graph").hidden = S.options.view !== "graph";
  $("table").hidden = S.options.view !== "table";
  $("cards").hidden = S.options.view !== "cards";
  $("map").hidden = S.options.view !== "map";
  if (S.options.view === "graph") {
    S.cy?.resize();
    if (S.options.layout === "hive") runLayout(false);
    else S.cy?.fit(S.cy.elements(":visible"), 30);
  } else drawAxes();
  if (S.options.view === "cards") renderCards(true);
  else stopCards();
  if (S.options.view === "table") renderTable();
  if (S.options.view === "map") renderMap();
  else stopMap();
}

/** The rows' tab that is showing, drawn again after the data changed. */
function renderRows() {
  syncMapTab();
  if (S.options.view === "table") renderTable();
  else if (S.options.view === "cards") renderCards(false);
  else if (S.options.view === "map") renderMap();
}

// --- map ------------------------------------------------------------------------------------
// The rows on a map (Retelens lib/maps.js MapView): a WKT column, or latitude and longitude columns, drawn over an
// open OpenStreetMap basemap read as PMTiles; a column of .pmtiles URLs (a raster such as an elevation model, or
// vector tiles) drawn under them as layers. It shows the rows the table's search and column filters let through.
// The basemap and the layers are fetched by HTTP Range from the sites this view declares (src/views.mjs).

/** Whether the rows have something to map: a geometry, a position, or a layer. */
function hasMap() {
  const r = richModel();
  const M = window.RetelensMaps;
  return !!(M.geoColumns(r.data.vars, r.model.rows) || M.layerUrls(r.model.rows, r.data.vars).length);
}

/** The Map tab is offered when the rows have something to map (or Claude asked for it). */
function syncMapTab() {
  const b = document.querySelector('.tabs button[data-tab="map"]');
  if (b) b.hidden = S.options.view !== "map" && !hasMap();
}

function stopMap() {
  S.map?.destroy();
  S.map = null;
}

function renderMap() {
  stopMap();
  const r = richModel();
  const m = r.model;
  m.q = $("table-filter").value.trim().toLowerCase();
  m.changed();
  S.map = window.RetelensMaps.MapView(m.viewRows(), r.data.vars, { short: r.short });
  $("map").replaceChildren(S.map.el);
}

const renderMapIfShown = () => S.options.view === "map" && renderMap();

// --- cards --------------------------------------------------------------------------------
// One card per row of the table, as the playground's Cards output does: the fields
// stacked, media full width, a masonry on a wide page. Everything is built as DOM
// nodes from text, never as markup, and an address is loaded only when safeUrl
// (cards.js) accepts it; images, audio, video and 3D load when their card nears the
// screen, and the number of cards drawn is bounded (CARDS_FIRST, then CARDS_STEP at a time).

function stopCards() {
  const c = S.cards;
  c.io?.disconnect();
  c.io = null;
  c.live = [];
}

/** The card view's fields: the table's columns, and how each is drawn. */
function renderCards(reset) {
  richModel(); // the columns' names from the query's hints
  const { vars, rows, terms } = tableData();
  const q = $("cards-filter").value.trim().toLowerCase();
  const found = rows.map((r, i) => [r, terms?.[i]]).filter(([r]) => !q || vars.some((v) => String(r[v] ?? "").toLowerCase().includes(q)));
  const c = S.cards;
  if (reset) c.shown = Cards.CARDS_FIRST;
  const n = Cards.cardsShown(found.length, c.shown);
  stopCards();
  const types = S.options.card_fields ?? {};
  const grid = $("cards-grid");
  grid.replaceChildren(...found.slice(0, n).map(([row, term]) => cardElement(vars, row, types, term)));
  grid.hidden = !found.length;
  $("cards-count").textContent = `${found.length} of ${rows.length} cards${found.length > Cards.CARDS_MAX ? ` (first ${Cards.CARDS_MAX} drawn)` : ""}`;
  const more = $("cards-more");
  more.textContent = Cards.moreLabel(found.length, n);
  more.hidden = !more.textContent;
  renderFieldsMenu(vars);
  $("cards-empty").hidden = !!found.length;
  $("cards-grid").dataset.drawn = String(n);
  $("cards").scrollTop = 0;
}

const el = (tag, className, text) => {
  const e = document.createElement(tag);
  if (className) e.className = className;
  if (text !== undefined) e.textContent = text;
  return e;
};

const prettyName = (v) => String(v).replace(/([a-z0-9])([A-Z])/g, "$1 $2").replace(/[_-]+/g, " ").replace(/^./, (ch) => ch.toUpperCase());

function cardElement(vars, row, types, terms) {
  const card = el("article", "rcard");
  card.tabIndex = 0;
  card.dataset.first = row[vars[0]] ?? "";
  const fields = Cards.cardFields(vars, row, types);
  for (const f of fields) card.append(fieldElement(f, terms?.[f.name]));
  if (!fields.length) card.append(el("div", "cf-v", "(empty row)"));
  card.setAttribute("aria-label", trunc(String(row[vars.find((v) => row[v]) ?? ""] ?? "card"), 80));
  return card;
}

function fieldElement(f, term) {
  const wrap = el("div", "cf");
  const info = S.rich.colInfo?.[f.name];
  const key = el("div", "cf-k", info?.label || prettyName(f.name));
  key.title = (f.name.startsWith("?") ? f.name : `?${f.name}`) + (info?.note ? ` — ${info.note}` : "");
  const value = el("div", "cf-v");
  wrap.append(key, value);
  if (Cards.isRich(f.type)) {
    // a IIIF manifest's pictures, a geometry's map, Markdown, a map layer: the Retelens cells
    value.classList.add("cf-rich");
    value.append(window.RetelensCells.render(term ?? textTerm(f.value), f.type, { short: S.rich.short ?? ((x) => x) }, { card: true }));
  } else if (Cards.isMedia(f.type)) {
    value.classList.add("cf-media");
    const slot = el("div", "media-slot", f.type === "model3d" ? "3D" : f.type);
    slot.dataset.kind = f.type;
    slot._field = f;
    value.append(slot, openLink(f, Cards.OPEN_LABEL[f.type], "media-footer"));
    watch(slot);
  } else if (f.type === "number") {
    value.classList.add("num");
    value.textContent = f.value;
    value.title = f.value;
  } else if (f.link && (f.type === "link" || f.type === "button" || f.type === "pdf")) {
    const label = f.type === "button" ? "Open ↗" : f.type === "pdf" ? "Open PDF ↗" : trunc(f.value, 120);
    const a = el("a", f.type === "button" ? "cell-btn" : "", label);
    a.href = f.link;
    a.dataset.open = f.link;
    a.title = f.value;
    a.rel = "noopener noreferrer";
    value.append(a);
  } else {
    value.textContent = trunc(f.value, 600);
    value.title = f.value.length > 120 ? trunc(f.value, 600) : "";
  }
  return wrap;
}

/** A link that opens through the host, with the label the playground gives it. */
function openLink(f, label, className) {
  const wrap = el("div", className);
  const a = el("a", "media-source", label);
  a.href = f.link ?? "#";
  a.dataset.open = f.link ?? "";
  a.rel = "noopener noreferrer";
  a.title = f.value;
  wrap.append(a);
  return wrap;
}

/** Media loads when its card is near the screen, not before. */
function watch(slot) {
  const c = S.cards;
  if (typeof IntersectionObserver === "undefined") return queueMicrotask(() => hydrate(slot));
  c.io ??= new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        c.io?.unobserve(e.target);
        hydrate(e.target);
      }
    },
    { root: document.querySelector(".cards-wrap"), rootMargin: "300px 0px" },
  );
  c.io.observe(slot);
}

/** What the media could not do, said where the picture would be, with a way to the file. */
function fallback(slot, why) {
  const f = slot._field;
  const box = el("div", "media-fallback");
  box.append(el("span", "why", why));
  const a = el("a", "", Cards.OPEN_LABEL[f.type]);
  a.href = f.link ?? "#";
  a.dataset.open = f.link ?? "";
  a.rel = "noopener noreferrer";
  box.append(a);
  slot.classList.remove("done");
  slot.replaceChildren(box);
}

function hydrate(slot) {
  const f = slot._field;
  const csp = S.app?.getHostCapabilities?.()?.sandbox?.csp;
  if (Cards.hostAllows(csp, f.src, f.type) === false) {
    return fallback(slot, `${new URL(f.src).host} is not on this host's list of allowed sites`);
  }
  const done = () => slot.classList.add("done");
  const failed = () => fallback(slot, `this ${f.type === "model3d" ? "3D model" : f.type} did not load here (the host may block ${new URL(f.src).host})`);
  if (f.type === "image") {
    const img = new Image();
    img.alt = prettyName(f.name);
    img.decoding = "async";
    img.referrerPolicy = "no-referrer";
    img.onload = done;
    img.onerror = failed;
    img.src = Cards.thumbnail(f.src);
    img.addEventListener("click", (e) => (e.stopPropagation(), f.link && openUrl(f.link).catch((error) => setNote(error.message))));
    slot.replaceChildren(img);
  } else if (f.type === "video" || f.type === "audio") {
    const m = document.createElement(f.type);
    m.controls = true;
    m.preload = "metadata";
    m.referrerPolicy = "no-referrer";
    if (f.type === "video") m.playsInline = true;
    m.addEventListener("loadedmetadata", done);
    m.addEventListener("error", failed);
    m.src = f.src;
    m.setAttribute("aria-label", prettyName(f.name));
    slot.replaceChildren(m);
  } else if (f.type === "model3d") {
    loadModelViewer().then((ok) => {
      if (!slot.isConnected) return;
      if (!ok) return fallback(slot, "the 3D viewer could not start in this host");
      const mv = document.createElement("model-viewer");
      for (const [k, v] of Object.entries({ src: f.src, alt: prettyName(f.name), "camera-controls": "", "auto-rotate": "", "interaction-prompt": "none", "shadow-intensity": "0.6", "environment-image": "neutral", "touch-action": "pan-y", reveal: "auto" })) mv.setAttribute(k, v);
      mv.addEventListener("load", done);
      mv.addEventListener("error", failed);
      slot.replaceChildren(mv);
      // A viewer that fails without an error (a host that forbids WebAssembly, say) must not sit there for ever:
      // after viewerTimeout of being on screen, unloaded, the model becomes a link. (model-viewer itself
      // waits for the model to be on screen before it fetches it, so time off screen does not count.)
      let waited = 0;
      const wait = setInterval(() => {
        const open = slot.contains(mv) && !slot.classList.contains("done");
        const r = mv.getBoundingClientRect();
        if (open && r.bottom > 0 && r.top < window.innerHeight) waited += 500;
        if (open && waited > S.cards.viewerTimeout) {
          fallback(slot, `this 3D model did not finish loading here (the host may block the 3D viewer or ${new URL(f.src).host})`);
        }
        if (!slot.contains(mv) || slot.classList.contains("done") || slot.querySelector(".media-fallback")) clearInterval(wait);
      }, 500);
      // Every viewer holds a WebGL context, and a browser keeps only a few: the oldest go back to their place.
      const live = S.cards.live;
      live.push(slot);
      while (live.length > Cards.LIVE_3D) {
        const old = live.shift();
        old.classList.remove("done");
        old.replaceChildren(document.createTextNode("3D"));
        watch(old);
      }
    });
  }
}

let viewerLoading = null;
/** The 3D viewer (model-viewer, built into this page) starts the first time a model needs it. */
function loadModelViewer() {
  viewerLoading ??= new Promise((resolve) => {
    if (customElements.get("model-viewer")) return resolve(true);
    const source = document.getElementById("model-viewer-src")?.textContent;
    if (!source) return resolve(false);
    try {
      const script = document.createElement("script");
      script.textContent = source;
      document.head.append(script);
    } catch {
      /* a host that forbids inline scripts leaves the element undefined */
    }
    resolve(!!customElements.get("model-viewer"));
  });
  return viewerLoading;
}

/** The ⚙ Fields menu: a select per column, the playground's field types. */
function renderFieldsMenu(vars) {
  const key = vars.join("\u0000");
  const types = S.options.card_fields ?? {};
  const box = $("cards-fields");
  if (S.cards.fieldsKey !== key) {
    S.cards.fieldsKey = key;
    box.replaceChildren(
      ...vars.map((v) => {
        const row = el("label", "field-row");
        const name = el("span", "", prettyName(v));
        name.title = v;
        const select = el("select");
        select.dataset.field = v;
        select.setAttribute("aria-label", `How to draw ${v}`);
        for (const [type, label] of Cards.FIELD_TYPES) select.append(Object.assign(el("option", "", label), { value: type }));
        row.append(name, select);
        return row;
      }),
    );
  }
  for (const select of box.querySelectorAll("select")) select.value = types[select.dataset.field] ?? "auto";
}

// --- status, controls, host ----------------------------------------------------------------

function setNote(text) {
  $("status-note").textContent = text;
  if (text) S.lastActivity = Date.now();
}

function renderStatus() {
  if (S.awaiting) {
    $("status-main").textContent = `waiting for ${S.awaiting.file}${S.viewId ? ` · view ${S.viewId}` : ""}`;
    return;
  }
  const parts = [S.title, S.dataset, `${S.nodes.size} nodes`, `${S.edges.size} edges`].filter(Boolean);
  $("status-main").textContent = `${parts.join(" · ")}${S.viewId ? ` · view ${S.viewId}` : ""}${S.status ? ` — ${S.status}` : ""}`;
}

function renderAwaiting() {
  $("awaiting").hidden = !S.awaiting;
  if (S.awaiting) $("awaiting-file").textContent = S.awaiting.file;
}

// --- a file from the user ------------------------------------------------------------------

/** base64 of bytes, a slice at a time: String.fromCharCode takes no megabyte at once. */
function base64(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** Send a file the user chose or dropped to the server, in parts; the view then shows it. */
async function openFile(file) {
  if (!file || !S.viewId || S.uploading) return;
  const awaited = S.awaiting;
  const say = (text) => {
    setNote(text);
    $("awaiting-progress").textContent = text;
  };
  if (!file.size) return say(`${file.name} is empty`);
  S.uploading = true;
  try {
    let r = {};
    for (let offset = 0; offset < file.size; ) {
      const part = new Uint8Array(await file.slice(offset, offset + PART).arrayBuffer());
      r = await call("graph_view_upload", { view_id: S.viewId, name: file.name, size: file.size, offset, data: base64(part), since: S.version });
      offset += part.length;
      say(`opening ${file.name}… ${Math.round((100 * offset) / file.size)}%`);
    }
    apply(r);
    say(`${file.name}: ${r.statements} statements`);
    // Claude asked for this file: tell it, so it carries on. Otherwise let it know quietly.
    const text = awaited
      ? `I opened ${file.name} in graph view ${S.viewId}: it is dataset "${r.dataset}" (${r.statements} statements).`
      : `In graph view ${S.viewId} the user opened ${file.name}: the view now shows dataset "${r.dataset}" (${r.statements} statements).`;
    if (awaited) await S.app.sendMessage({ role: "user", content: [{ type: "text", text }] }).catch(() => {});
    else if (S.app.getHostCapabilities()?.updateModelContext) await S.app.updateModelContext({ content: [{ type: "text", text }] }).catch(() => {});
  } catch (error) {
    say(`open: ${error.message}`);
  } finally {
    S.uploading = false;
    $("file-input").value = "";
  }
}

function syncControls() {
  const dots = diagramOf() === "graph";
  $("opt-diagram").value = diagramOf();
  $("opt-edges").value = S.options.edge_style || "curved";
  $("opt-shape").checked = S.options.node_shape === "square";
  $("opt-props").checked = S.options.properties !== false;
  // Each setting where it means something: dots have labels and frames, boxes properties.
  for (const id of ["opt-labels", "opt-literals", "opt-boxes", "opt-shape"]) $(id).disabled = !dots;
  $("opt-props").disabled = dots;
  $("opt-labels").checked = !!S.options.labels;
  $("opt-edge-labels").checked = !!S.options.edge_labels;
  $("opt-literals").checked = !!S.options.literals_as_nodes;
  $("opt-boxes").checked = !!S.options.group_boxes;
  $("opt-layout").value = S.options.layout || "cose";
  $("opt-hive-axes").value = S.options.hive_axes || "auto";
  $("opt-hive-order").value = S.options.hive_order || "degree";
  for (const e of document.querySelectorAll(".hive-opt")) e.hidden = S.options.layout !== "hive";
  // Grouping by a property: every predicate or column the nodes carry.
  const select = $("opt-group");
  const preds = new Set();
  for (const n of S.nodes.values()) for (const [p] of n.props) if (preds.size < 30) preds.add(p);
  if (S.options.group_by && !["type", "namespace", "none"].includes(S.options.group_by)) preds.add(S.options.group_by);
  select.innerHTML =
    `<option value="type">by type</option><option value="namespace">by namespace</option><option value="none">none</option>` +
    [...preds].map((p) => `<option value="${esc(p)}">by ${esc(local(p))}</option>`).join("");
  select.value = S.options.group_by || "type";
  for (const b of document.querySelectorAll(".tabs button")) b.classList.toggle("on", b.dataset.tab === S.options.view);
}

function applyHost(ctx = {}) {
  S.host = { ...S.host, ...ctx };
  const root = document.documentElement;
  for (const [k, v] of Object.entries(S.host.styles?.variables ?? {})) if (v) root.style.setProperty(k, v);
  $("fullscreen").hidden = !(S.host.availableDisplayModes ?? []).includes("fullscreen");
  document.body.classList.toggle("full", S.host.displayMode === "fullscreen");
  applyTheme();
  S.cy?.resize();
}

const THEMES = { auto: ["◐", "Theme: follows the app"], light: ["☀", "Theme: light"], dark: ["☾", "Theme: dark"] };

/** Light or dark: the user's choice, or else Claude's theme, or else the system's. */
function applyTheme(redraw = true) {
  const chosen = THEMES[S.options.theme] ? S.options.theme : "auto";
  const system = window.matchMedia?.("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  document.documentElement.dataset.theme = chosen === "auto" ? (S.host.theme ?? system) : chosen;
  document.documentElement.classList.toggle("dark", document.documentElement.dataset.theme === "dark"); // Retelens' styles
  if (S.options.view === "map") renderMap(); // the basemap has a light and a dark style
  const [icon, title] = THEMES[chosen];
  $("theme").textContent = icon;
  $("theme").title = `${title} — click to change`;
  applyStyle();
  if (redraw && diagramOf() !== "graph") rebuild(false); // the boxes are pictures, in the theme's colours
}

function wire() {
  const option = (key, value) => {
    S.options[key] = value;
    markState();
  };
  $("opt-labels").addEventListener("change", (e) => (option("labels", e.target.checked), applyStyle()));
  $("opt-edge-labels").addEventListener("change", (e) => (option("edge_labels", e.target.checked), applyStyle()));
  $("opt-literals").addEventListener("change", (e) => (option("literals_as_nodes", e.target.checked), rebuild(true)));
  $("opt-boxes").addEventListener("change", (e) => (option("group_boxes", e.target.checked), rebuild(true)));
  $("opt-group").addEventListener("change", (e) => {
    option("group_by", e.target.value);
    S.hiddenGroups.clear();
    rebuild(S.options.group_boxes);
  });
  $("opt-layout").addEventListener("change", (e) => {
    option("layout", e.target.value);
    for (const h of document.querySelectorAll(".hive-opt")) h.hidden = e.target.value !== "hive";
    applyStyle();
    runLayout(true);
  });
  $("opt-hive-axes").addEventListener("change", (e) => (option("hive_axes", e.target.value), runLayout(true)));
  $("opt-hive-order").addEventListener("change", (e) => (option("hive_order", e.target.value), runLayout(true)));
  $("opt-diagram").addEventListener("change", (e) => setDiagram(e.target.value));
  $("opt-edges").addEventListener("change", (e) => (option("edge_style", e.target.value), applyStyle()));
  $("opt-shape").addEventListener("change", (e) => (option("node_shape", e.target.checked ? "square" : "circle"), applyStyle()));
  $("opt-props").addEventListener("change", (e) => (option("properties", e.target.checked), rebuild(true)));
  // The Show menu closes on a click anywhere else.
  document.addEventListener("click", (e) => {
    const menu = $("show");
    if (menu.open && !menu.contains(e.target)) menu.open = false;
  });
  $("zoom-in").addEventListener("click", () => zoomBy(1.25));
  $("zoom-out").addEventListener("click", () => zoomBy(0.8));
  $("fit").addEventListener("click", () => S.cy?.fit(S.cy.elements(":visible"), 30));
  $("relayout").addEventListener("click", () => runLayout(true));
  $("theme").addEventListener("click", () => {
    const order = Object.keys(THEMES);
    S.options.theme = order[(order.indexOf(S.options.theme) + 1) % order.length];
    applyTheme();
    markState();
  });
  $("fullscreen").addEventListener("click", async () => {
    const want = S.host.displayMode === "fullscreen" ? "inline" : "fullscreen";
    try {
      const { mode } = await S.app.requestDisplayMode({ mode: want });
      applyHost({ displayMode: mode });
      setTimeout(() => S.cy?.fit(S.cy.elements(":visible"), 30), 200);
    } catch (error) {
      setNote(`fullscreen: ${error.message}`);
    }
  });
  for (const b of document.querySelectorAll(".tabs button")) {
    b.addEventListener("click", () => {
      showTab(b.dataset.tab);
      markState();
    });
  }

  const search = $("search");
  search.addEventListener(
    "input",
    debounce(() => {
      S.search = search.value;
      const hits = applySearch();
      setNote(S.search ? `${hits} match(es) in the view` : "");
      markState();
    }, 200),
  );
  search.addEventListener("keydown", (e) => e.key === "Enter" && searchDataset(search.value));

  document.querySelector("#layers ul").addEventListener("change", (e) => {
    const key = e.target.dataset.layer;
    if (key === undefined) return;
    if (e.target.checked) S.hiddenGroups.delete(key);
    else S.hiddenGroups.add(key);
    applyHidden();
    markState();
  });
  const allLayers = (show) => (e) => {
    e.preventDefault();
    S.hiddenGroups = show ? new Set() : new Set(S.groups.keys());
    applyHidden();
    renderLayers();
    markState();
  };
  $("layers-all").addEventListener("click", allLayers(true));
  $("layers-none").addEventListener("click", allLayers(false));

  document.querySelector("#details").addEventListener("click", async (e) => {
    const open = e.target.closest("[data-open]");
    if (open) {
      e.preventDefault();
      openUrl(open.dataset.open).catch(() => {});
      return;
    }
    const act = e.target.closest("[data-act]")?.dataset.act;
    if (!act) return;
    if (act === "expand") await expand(S.selection[0]?.id);
    if (act === "members") showMembers(S.classBoxes.get(S.selection[0]?.id));
    const ids = S.selection.map((s) => s.id);
    if (act === "hide") hide(ids);
    if (act === "remove") removeNodes(ids);
    if (act === "aggregate") aggregate(ids);
    if (act === "ungroup") edit({ ungroup: ids[0] });
    if (act === "send") sendToChat(ids);
    if (act === "ask") {
      const about = S.selection.slice(0, 10).map((s) => `${s.label}${s.iri ? ` (<${s.iri}>)` : ""}`).join(", ");
      S.app
        .sendMessage({ role: "user", content: [{ type: "text", text: `In graph view ${S.viewId} on ${S.dataset}: tell me about ${about}.` }] })
        .catch((error) => setNote(`ask: ${error.message}`));
    }
  });

  const choose = () => $("file-input").click();
  $("open-file").addEventListener("click", choose);
  $("awaiting-choose").addEventListener("click", choose);
  $("file-input").addEventListener("change", (e) => openFile(e.target.files?.[0]));
  // A file dropped anywhere on the view opens in it.
  let dragging = 0;
  const carriesFiles = (e) => [...(e.dataTransfer?.types ?? [])].includes("Files");
  document.addEventListener("dragenter", (e) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    dragging += 1;
    $("dropzone").hidden = false;
  });
  document.addEventListener("dragover", (e) => carriesFiles(e) && e.preventDefault());
  document.addEventListener("dragleave", () => {
    dragging = Math.max(0, dragging - 1);
    if (!dragging) $("dropzone").hidden = true;
  });
  document.addEventListener("drop", (e) => {
    if (!carriesFiles(e)) return;
    e.preventDefault();
    dragging = 0;
    $("dropzone").hidden = true;
    openFile(e.dataTransfer.files?.[0]);
  });

  // The right-click menu: on a node, on the selection it is part of, or on the background.
  S.cy.on("cxttap", (e) => {
    const el = e.target === S.cy ? null : e.target;
    if (el && (el.isEdge() || el.hasClass("lit"))) return closeMenu();
    const node = el && !el.hasClass("box") ? el : null;
    const r = $("graph").getBoundingClientRect();
    const x = e.originalEvent?.clientX ?? r.left + (e.renderedPosition?.x ?? 0);
    const y = e.originalEvent?.clientY ?? r.top + (e.renderedPosition?.y ?? 0);
    const ids = node ? targetsOf(node) : [];
    openMenu(node ? (ids.length > 1 ? `${ids.length} nodes` : nameOf(node.id())) : "", node ? nodeMenu(node) : backgroundMenu(), x, y);
  });
  $("graph").addEventListener("contextmenu", (e) => e.preventDefault());
  $("ctx").addEventListener("click", async (e) => {
    const item = S.menu[e.target.closest("[data-i]")?.dataset.i];
    if (!item) return;
    closeMenu();
    try {
      await item.run();
    } catch (error) {
      setNote(error.message);
    }
  });
  document.addEventListener("pointerdown", (e) => !$("ctx").contains(e.target) && closeMenu(), true);
  // Closed by the user's own moves only: a layout's animated fit pans and zooms too.
  $("graph").addEventListener("wheel", closeMenu, { passive: true });
  $("selecting-done").addEventListener("click", () => selectMode(false));
  $("about-open").addEventListener("click", () => ($("about").hidden = !$("about").hidden));
  $("about").addEventListener("click", (e) => {
    const open = e.target.closest("[data-open]");
    if (!open) return;
    e.preventDefault();
    openUrl(open.dataset.open).catch((error) => setNote(`link: ${error.message}`));
  });
  document.addEventListener("pointerdown", (e) => {
    if (!$("about").hidden && !$("about").contains(e.target) && e.target !== $("about-open")) $("about").hidden = true;
  });
  // A click on the background unselects everything (cytoscape does, even when
  // selection is additive) — not while selecting several: the selection is put back.
  let keep = null;
  S.cy.on("tapstart", (e) => (keep = S.selecting && e.target === S.cy ? S.cy.$(":selected") : null));
  S.cy.on("tapend", (e) => {
    const kept = e.target === S.cy ? keep : null;
    keep = null;
    if (kept?.nonempty()) setTimeout(() => kept.select(), 0);
  });
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      closeMenu();
      $("about").hidden = true;
      if (S.selecting) selectMode(false);
    }
    if (e.key === "Delete" && !/^(INPUT|SELECT|TEXTAREA)$/.test(e.target.tagName) && S.selection.length) removeNodes(S.selection.map((s) => s.id));
  });

  $("table-filter").addEventListener("input", debounce(renderTable, 150));
  $("table-clear").addEventListener("click", () => {
    const m = richModel().model;
    for (const v of m.activeCols()) m.setFilter(v, null);
    renderTable();
  });
  // A web link drawn by the Retelens cells (in the table, the cards, a map popup or a picture's dialog) opens
  // through the host, as every link here does: the sandbox lets nothing navigate on its own.
  document.addEventListener(
    "click",
    (e) => {
      const a = e.target.closest?.("a[href]");
      if (!a || a.hasAttribute("data-open") || !webUrl(a.getAttribute("href"))) return;
      e.preventDefault();
      e.stopPropagation();
      openUrl(a.getAttribute("href")).catch((error) => setNote(error.message));
    },
    true,
  );
  $("rows").addEventListener("click", (e) => {
    const th = e.target.closest("th");
    if (th) {
      const col = th.dataset.col;
      S.sort = { col, desc: S.sort.col === col ? !S.sort.desc : false };
      return renderTable();
    }
    const tr = e.target.closest("tbody tr");
    if (!tr) return;
    for (const r of document.querySelectorAll("#rows tr.on")) r.classList.remove("on");
    tr.classList.add("on");
    const first = tr.dataset.first;
    const id = S.nodes.has(`<${first}>`) ? `<${first}>` : [...S.nodes.values()].find((n) => nodeLabel(n) === first)?.id;
    if (id) focusNode(id);
  });
  $("csv").addEventListener("click", () => {
    S.app
      .downloadFile({ contents: [{ type: "resource", resource: { uri: `file:///${(S.title || "graph").replace(/\W+/g, "-")}.csv`, mimeType: "text/csv", text: csvText() } }] })
      .catch((error) => setNote(`CSV: ${error.message}`));
  });
  // Cards.
  $("cards-filter").addEventListener("input", debounce(() => renderCards(true), 150));
  $("cards-more").addEventListener("click", () => {
    S.cards.shown = Cards.cardsShown(Infinity, S.cards.shown, true);
    renderCards(false);
  });
  $("cards-fields").addEventListener("change", (e) => {
    const select = e.target.closest("select[data-field]");
    if (!select) return;
    const fields = { ...(S.options.card_fields ?? {}) };
    if (select.value === "auto") delete fields[select.dataset.field];
    else fields[select.dataset.field] = select.value;
    S.options.card_fields = fields;
    markState();
    renderCards(false);
  });
  const pick = (card) => {
    for (const r of document.querySelectorAll("#cards-grid .rcard.on")) r.classList.remove("on");
    card.classList.add("on");
    const first = card.dataset.first;
    const id = S.nodes.has(`<${first}>`) ? `<${first}>` : [...S.nodes.values()].find((n) => nodeLabel(n) === first)?.id;
    if (id) focusNode(id);
  };
  $("cards-grid").addEventListener("click", (e) => {
    const a = e.target.closest("a[data-open]");
    if (a) {
      e.preventDefault();
      e.stopPropagation();
      if (a.dataset.open) openUrl(a.dataset.open).catch((error) => setNote(error.message));
      return;
    }
    const card = e.target.closest(".rcard");
    if (card && !e.target.closest("video, audio, model-viewer, img")) pick(card);
  });
  $("cards-grid").addEventListener("keydown", (e) => {
    const card = e.target.closest?.(".rcard");
    if (card && e.target === card && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      pick(card);
    }
  });
  $("cards-fields-menu").addEventListener("click", (e) => e.stopPropagation());
  document.addEventListener("click", (e) => {
    const menu = $("cards-fields-menu");
    if (menu.open && !menu.contains(e.target)) menu.open = false;
  });
  // What the host's content policy refuses is said once, by site, where it was refused.
  document.addEventListener("securitypolicyviolation", (e) => {
    if (!/^https?:/.test(e.blockedURI)) return;
    S.cards.blocked.add(new URL(e.blockedURI).host);
    setNote(`the host blocked ${[...S.cards.blocked].slice(0, 3).join(", ")}${S.cards.blocked.size > 3 ? "…" : ""} (content policy)`);
  });
  document.addEventListener("visibilitychange", () => !document.hidden && sync(false));
}

function zoomBy(factor) {
  const cy = S.cy;
  if (!cy) return;
  cy.zoom({ level: cy.zoom() * factor, renderedPosition: { x: cy.width() / 2, y: cy.height() / 2 } });
}

// --- start --------------------------------------------------------------------------------

function start() {
  S.cy = cytoscape({ container: $("graph"), elements: [], style: styleSheet(), wheelSensitivity: 0.25, minZoom: 0.05, maxZoom: 6, boxSelectionEnabled: true });
  S.cy.on("select unselect", "node", debounce(onSelectionChanged, 60));
  S.cy.on("viewport resize", drawAxes);
  S.cy.on("dbltap", "node", (e) => {
    const el = e.target;
    const c = S.classBoxes.get(el.id());
    if (c) return showMembers(c);
    if (!el.hasClass("box") && !el.hasClass("lit") && S.nodes.get(el.id())?.kind !== "class") expand(el.id());
  });
  wire();
  renderAbout();
  syncControls();
  renderLayers();

  const app = new App({ name: "rete graph view", version: ABOUT?.version ?? "0.0.0-dev" }, { availableDisplayModes: ["inline", "fullscreen"] }, { autoResize: true });
  S.app = app;
  app.ontoolinput = ({ arguments: args = {} }) => {
    if (args.options) Object.assign(S.options, args.options);
    S.dataset = args.dataset ?? S.dataset;
    S.title = args.title ?? S.title;
    syncControls();
    $("status-main").textContent = `${S.dataset ? `${S.dataset} · ` : ""}running the query…`;
  };
  app.ontoolresult = (result) => {
    const text = result?.content?.find((c) => c.type === "text")?.text ?? "";
    if (result?.isError) {
      $("status-main").textContent = text || "the view could not be opened";
      return;
    }
    const id = result?.structuredContent?.view_id ?? text.match(/Graph view (v\d+)/)?.[1];
    if (!id) return;
    S.viewId = id;
    S.dataset = result?.structuredContent?.dataset ?? S.dataset;
    sync(true);
  };
  app.onhostcontextchanged = (ctx) => applyHost(ctx);
  app.onteardown = async () => {
    clearTimeout(S.timer);
    S.viewId = null;
    return {};
  };
  app
    .connect()
    .then(() => applyHost(app.getHostContext() ?? {}))
    .catch((error) => {
      $("status-main").textContent = `could not connect to the host: ${error.message}`;
    });
}

start();
