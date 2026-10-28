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

const PALETTE = ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#2f4b7c", "#a05195", "#5fa2ce"];
const OTHER = "#a8a8a8";
const MAX_PROPS = 40; // shown in the selection panel
const MAX_LITERAL_NODES = 12; // per node, with literals as nodes
const MAX_TABLE = 1000; // rows drawn at once
const PART = 1024 * 1024; // bytes of a file per graph_view_upload call

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
  options: { labels: true, edge_labels: false, group_by: "type", group_boxes: false, layout: "cose", view: "graph", literals_as_nodes: false },
  groups: new Map(), // key -> { color, count }
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
  lastActivity: Date.now(),
  stateDirty: true,
  syncing: false,
  timer: null,
};
window.__reteView = S; // tests read the view through this

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? "").replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
const local = (iri) => {
  const s = String(iri ?? "");
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return cut >= 0 && cut < s.length - 1 ? s.slice(cut + 1) : s;
};
const namespace = (iri) => {
  const s = String(iri ?? "");
  const cut = Math.max(s.lastIndexOf("#"), s.lastIndexOf("/"));
  return cut > 0 ? s.slice(0, cut + 1) : s;
};
const trunc = (s, n) => (String(s).length > n ? `${String(s).slice(0, n - 1)}…` : String(s));
const nodeLabel = (n) => trunc(n.label || (n.iri ? local(n.iri) : n.id), 48);
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
  renderAwaiting();
  syncControls();
  rebuild(true);
  requestLabels();
}

function applyPatch(p) {
  S.lastActivity = Date.now();
  switch (p.op) {
    case "replace":
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
      const structural = ["group_by", "group_boxes", "literals_as_nodes"].some((k) => before[k] !== S.options[k]);
      if (structural) rebuild(true);
      else {
        applyStyle();
        if (before.layout !== S.options.layout) runLayout(true);
      }
      showTab(S.options.view);
      setNote("Claude changed the view");
      break;
    }
    case "highlight":
      setHighlight(p.ids);
      break;
    case "focus":
      focusNode(p.id);
      break;
    case "remove":
      for (const id of p.ids) {
        S.nodes.delete(id);
        for (const [k, e] of S.edges) if (e.source === id || e.target === id) S.edges.delete(k);
      }
      rebuild(false);
      break;
    case "status":
      S.status = p.status ?? "";
      break;
    case "dataset":
      // A file the user opened: the view starts over on it.
      S.dataset = p.dataset;
      if (p.title) S.title = p.title;
      S.awaiting = null;
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
  if (S.options.view === "table") renderTable();
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
  const boxes = S.options.group_boxes && S.options.group_by !== "none";
  const els = [];
  if (boxes) {
    for (const [k, g] of S.groups) els.push({ group: "nodes", data: { id: `group:${k}`, label: k || "—", color: g.color, gk: k }, classes: "box" });
  }
  for (const n of S.nodes.values()) {
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
  for (const e of S.edges.values()) els.push({ group: "edges", data: { id: e.id, source: e.source, target: e.target, label: e.label } });
  return els;
}

function styleSheet() {
  const css = getComputedStyle(document.documentElement);
  const v = (name) => css.getPropertyValue(name).trim();
  return [
    {
      selector: "node",
      style: {
        "background-color": "data(color)",
        label: S.options.labels ? "data(label)" : "",
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
      selector: "edge",
      style: {
        width: 1,
        "line-color": v("--edge"),
        "target-arrow-shape": "triangle",
        "target-arrow-color": v("--edge"),
        "arrow-scale": 0.7,
        "curve-style": "bezier",
        label: S.options.edge_labels ? "data(label)" : "",
        "font-size": 7,
        color: v("--muted"),
        "text-rotation": "autorotate",
        "text-outline-width": 2,
        "text-outline-color": v("--bg"),
        "min-zoomed-font-size": 6,
      },
    },
    { selector: "edge.litedge", style: { "line-style": "dashed", "target-arrow-shape": "none" } },
    { selector: "node:selected", style: { "border-width": 3, "border-color": v("--accent") } },
    { selector: ".hl", style: { "border-width": 3, "border-color": v("--hl"), "z-index": 10 } },
    { selector: ".faded", style: { opacity: 0.15 } },
    { selector: ".hidden", style: { display: "none" } },
  ];
}

function applyStyle() {
  S.cy?.style(styleSheet());
}

function rebuild(layout) {
  const cy = S.cy;
  if (!cy) return;
  const positions = new Map(cy.nodes().map((n) => [n.id(), { ...n.position() }]));
  cy.elements().remove();
  cy.add(elements());
  for (const [id, pos] of positions) cy.getElementById(id).position(pos);
  applyStyle();
  applyHidden();
  applyHighlight();
  applySearch();
  if (layout) runLayout(positions.size === 0);
  renderLayers();
  renderDetails();
  if (S.options.view === "table") renderTable();
}

/** New elements into the drawing, keeping what is there in place. */
function grow(nodes, edges) {
  const cy = S.cy;
  if (!cy) return;
  if (S.options.group_boxes || S.options.literals_as_nodes) return rebuild(true);
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
  if (S.options.group_boxes) return rebuild(false);
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
  const opts = { name, fit: true, padding: 30, animate: !many, animationDuration: 350, nodeDimensionsIncludeLabels: true };
  if (name === "cose") Object.assign(opts, { randomize, idealEdgeLength: 70, nodeRepulsion: 9000, gravity: 0.25, numIter: many ? 600 : 1200, componentSpacing: 60 });
  if (name === "concentric") Object.assign(opts, { minNodeSpacing: 18, concentric: (n) => n.degree(), levelWidth: () => 2 });
  if (name === "breadthfirst") Object.assign(opts, { directed: true, spacingFactor: 1.1 });
  cy.elements(":visible").layout(opts).run();
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
    S.hiddenNodes.clear();
    applyHidden();
    markState();
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
      `<div class="actions"><button type="button" data-act="hide">Hide</button><button type="button" data-act="ask">Ask Claude</button></div>` +
      `<p class="hint">${picked.slice(0, 12).map((s) => esc(s.label)).join(" · ")}${picked.length > 12 ? " …" : ""}</p>`;
    return;
  }
  const n = S.nodes.get(picked[0].id);
  if (!n) return;
  const degree = S.cy?.getElementById(n.id).degree() ?? 0;
  const props = n.props.slice(0, MAX_PROPS).map(([p, v]) => `<dt title="${esc(p)}">${esc(local(p))}</dt><dd>${esc(trunc(v, 300))}</dd>`).join("");
  body.innerHTML =
    `<p class="title">${esc(n.label || local(n.iri) || n.id)}</p>` +
    (n.iri ? `<a href="#" class="iri" data-open="${esc(n.iri)}">${esc(n.iri)}</a>` : "") +
    (n.types.length ? `<div class="types">${n.types.map((t) => esc(local(t))).join(", ")}</div>` : "") +
    `<div class="actions">` +
    (n.kind === "iri" ? `<button type="button" data-act="expand">Expand</button>` : "") +
    `<button type="button" data-act="hide">Hide</button><button type="button" data-act="ask">Ask Claude</button></div>` +
    `<p class="hint">${degree} link(s) in the view${n.props.length > MAX_PROPS ? ` · ${n.props.length} properties, first ${MAX_PROPS} shown` : ""}</p>` +
    (props ? `<dl>${props}</dl>` : "");
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
      const n = S.nodes.get(el.id());
      return { id: el.id(), iri: n?.iri ?? null, label: n ? nodeLabel(n) : el.id() };
    });
  renderDetails();
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

function tableData() {
  if (S.table?.vars?.length) return S.table;
  const rows = [...S.edges.values()].map((e) => ({
    subject: nodeLabel(S.nodes.get(e.source) ?? { id: e.source }),
    predicate: e.p ?? e.label,
    object: nodeLabel(S.nodes.get(e.target) ?? { id: e.target }),
  }));
  return { vars: ["subject", "predicate", "object"], rows };
}

function renderTable() {
  const { vars, rows } = tableData();
  const q = $("table-filter").value.trim().toLowerCase();
  let shown = q ? rows.filter((r) => vars.some((v) => String(r[v] ?? "").toLowerCase().includes(q))) : rows.slice();
  if (S.sort.col) {
    const c = S.sort.col;
    shown.sort((a, b) => String(a[c] ?? "").localeCompare(String(b[c] ?? ""), undefined, { numeric: true }) * (S.sort.desc ? -1 : 1));
  }
  $("table-count").textContent = `${shown.length} of ${rows.length} rows${shown.length > MAX_TABLE ? ` (first ${MAX_TABLE} drawn)` : ""}`;
  const head = `<thead><tr>${vars.map((v) => `<th data-col="${esc(v)}">${esc(v)} <span class="dir">${S.sort.col === v ? (S.sort.desc ? "▼" : "▲") : ""}</span></th>`).join("")}</tr></thead>`;
  const body = shown
    .slice(0, MAX_TABLE)
    .map((r) => `<tr data-first="${esc(r[vars[0]] ?? "")}">${vars.map((v) => `<td>${esc(trunc(r[v] ?? "", 400))}</td>`).join("")}</tr>`)
    .join("");
  $("rows").innerHTML = `${head}<tbody>${body}</tbody>`;
  $("csv").hidden = !S.app?.getHostCapabilities()?.downloadFile;
}

function csvText() {
  const { vars, rows } = tableData();
  const cell = (s) => (/[",\r\n]/.test(s) ? `"${String(s).replaceAll('"', '""')}"` : String(s));
  return [vars.map(cell).join(","), ...rows.map((r) => vars.map((v) => cell(r[v] ?? "")).join(","))].join("\r\n") + "\r\n";
}

function showTab(tab) {
  S.options.view = tab === "table" ? "table" : "graph";
  for (const b of document.querySelectorAll(".tabs button")) b.classList.toggle("on", b.dataset.tab === S.options.view);
  $("graph").hidden = S.options.view !== "graph";
  $("table").hidden = S.options.view !== "table";
  if (S.options.view === "table") renderTable();
  else {
    S.cy?.resize();
    S.cy?.fit(S.cy.elements(":visible"), 30);
  }
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
  $("opt-labels").checked = !!S.options.labels;
  $("opt-edge-labels").checked = !!S.options.edge_labels;
  $("opt-literals").checked = !!S.options.literals_as_nodes;
  $("opt-boxes").checked = !!S.options.group_boxes;
  $("opt-layout").value = S.options.layout || "cose";
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
  if (S.host.theme) root.dataset.theme = S.host.theme;
  for (const [k, v] of Object.entries(S.host.styles?.variables ?? {})) if (v) root.style.setProperty(k, v);
  $("fullscreen").hidden = !(S.host.availableDisplayModes ?? []).includes("fullscreen");
  document.body.classList.toggle("full", S.host.displayMode === "fullscreen");
  applyStyle();
  S.cy?.resize();
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
  $("opt-layout").addEventListener("change", (e) => (option("layout", e.target.value), runLayout(true)));
  $("zoom-in").addEventListener("click", () => zoomBy(1.25));
  $("zoom-out").addEventListener("click", () => zoomBy(0.8));
  $("fit").addEventListener("click", () => S.cy?.fit(S.cy.elements(":visible"), 30));
  $("relayout").addEventListener("click", () => runLayout(true));
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
  search.addEventListener("keydown", async (e) => {
    if (e.key !== "Enter" || !search.value.trim() || !S.viewId) return;
    setNote(`finding “${search.value.trim()}” in ${S.dataset}…`);
    try {
      const r = await call("graph_view_search", { view_id: S.viewId, text: search.value.trim(), since: S.version });
      apply(r);
      setNote(r.note ?? `${r.found ?? 0} found in the graph`);
    } catch (error) {
      setNote(`search: ${error.message}`);
    }
  });

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
      S.app.openLink({ url: open.dataset.open }).catch(() => {});
      return;
    }
    const act = e.target.closest("[data-act]")?.dataset.act;
    if (!act) return;
    if (act === "expand") await expand(S.selection[0]?.id);
    if (act === "hide") {
      for (const s of S.selection) S.hiddenNodes.add(s.id);
      S.cy.$(":selected").unselect();
      applyHidden();
      markState();
    }
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

  $("table-filter").addEventListener("input", debounce(renderTable, 150));
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
  S.cy.on("dbltap", "node", (e) => {
    const el = e.target;
    if (!el.hasClass("box") && !el.hasClass("lit")) expand(el.id());
  });
  wire();
  syncControls();
  renderLayers();

  const app = new App({ name: "rete graph view", version: "1.0.0" }, { availableDisplayModes: ["inline", "fullscreen"] }, { autoResize: true });
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
