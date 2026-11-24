import cytoscape from "cytoscape";
import fcose from "cytoscape-fcose";
import dagre from "cytoscape-dagre";
import css from "./graphkit.css";
import { Model, ntTerm, ntIri } from "./model.js";
import { resolveSkin } from "./skins.js";
import { Minimap } from "./minimap.js";
import { buildSvg } from "./svgexport.js";
import { applyFallback } from "./skincheck.js";
import workerSrc from "./worker.built.txt";
import { renderChord } from "./chord.js";
import { makeAxes, hive, circos, drawDeco } from "./layouts.js";
import { PALETTE, SEQ, SHAPES, OTHER, DASHES, TOKENS_LIGHT, TOKENS_DARK, GLYPHS, isHttp, localName, shortIri, trunc, el, svg, clear, fmt } from "./util.js";

cytoscape.use(fcose);
cytoscape.use(dagre);

export const VERSION = "1.0.0";
const LAYOUTS = [
  ["fcose", "Force"], ["dagre", "Hierarchical"], ["concentric", "Radial (by degree)"], ["circle", "Circle"], ["grid", "Grid"],
  ["tree", "Tree from selection"], ["hive", "Hive plot"], ["circos", "Circos"], ["chord", "Chord (groups)"],
];
const GROUPS = [["type", "Class (rdf:type)"], ["namespace", "Namespace"], ["community", "Community"], ["component", "Component"], ["none", "None"]];
const GROUP_WORD = { type: "class", namespace: "namespace", community: "community", component: "component", none: "none" };
const SIZEBY = [["degree", "Degree"], ["in", "In-degree"], ["out", "Out-degree"], ["constant", "Constant"]];
const COLORBY = [["group", "Group"], ["degree", "Degree (blue ramp)"], ["none", "None"]];
const CURVE_SIGN = -1; // verified against cytoscape's midpoint in test/graphkit/tests
const MAX_NODES_SOFT = 2000, MAX_EDGES_SOFT = 4000;

const styled = new WeakSet();
function injectCss(host) {
  const rootNode = host.getRootNode ? host.getRootNode() : document;
  const target = rootNode.nodeType === 11 ? rootNode : document;
  if (styled.has(target)) return;
  styled.add(target);
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(css);
    target.adoptedStyleSheets = [...target.adoptedStyleSheets, sheet];
  } catch (e) {
    const st = document.createElement("style");
    st.textContent = css;
    (target === document ? document.head : target).appendChild(st);
  }
}

let _cctx;
function normColor(c) {
  try { _cctx = _cctx || document.createElement("canvas").getContext("2d"); _cctx.fillStyle = "#000000"; _cctx.fillStyle = c; return _cctx.fillStyle; } catch (e) { return c; }
}
const frames = (n) => new Promise((res) => { const f = () => (--n <= 0 ? res() : requestAnimationFrame(f)); requestAnimationFrame(f); });
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

export function mount(container, opts) { return new Kit(container, opts || {}); }

class Kit {
  constructor(container, o) {
    this.o = o;
    this.container = container;
    injectCss(container);
    this.sk = resolveSkin(o.skin, o.classes);
    this.fellBack = o.skinFallback === false ? [] : applyFallback(this.sk);
    if (this.fellBack.length) console.debug("[graphkit] skin \"" + this.sk.name + "\": classes not found in the page styles, using graphkit styling for: " + this.fellBack.map((f) => f.component + " (missing " + f.missing.slice(0, 4).join(" ") + (f.missing.length > 4 ? " ..." : "") + ")").join("; "));
    this.dark = !!o.dark;
    this.tokens = Object.assign({}, this.dark ? TOKENS_DARK : TOKENS_LIGHT, o.tokens || {});
    this.model = new Model();
    this.st = { layout: "fcose", dir: "TB", groupBy: "type", colorBy: "group", sizeBy: "degree", labels: "auto", edgeColor: false, boxes: false, minimap: true, wrap: true, minDeg: 0, hideLiterals: false, hideBnodes: false };
    this.hiddenSlots = new Set(); this.hiddenPreds = new Set(); this.hiddenNodes = new Set(); this.pinned = new Set();
    this.focus = null; this.expanded = new Set();
    this.slotMap = new Map(); this.predMap = new Map(); this.groups = []; this.preds = [];
    this.listeners = {}; this.cleanups = [];
    this.active = false; this.curveSign = CURVE_SIGN;
    this.matches = []; this.matchIdx = -1;
    this._lt = 0;
    this.build();
    this.initCy();
    this.bindEvents();
    this.setVars();
    this.refreshUi();
  }

  /* ---------------------------------------------------------------- helpers */
  cls(key, fallback) { return this.sk.classes[key] || fallback; }
  pal() { return this.dark ? PALETTE.dark : PALETTE.light; }
  slotColor(slot) { return slot >= 0 && slot < OTHER ? this.pal()[slot] : normColor(this.tokens.textMuted); }
  on(name, fn) { (this.listeners[name] = this.listeners[name] || []).push(fn); return () => this.off(name, fn); }
  off(name, fn) { this.listeners[name] = (this.listeners[name] || []).filter((f) => f !== fn); }
  emit(name, payload) { for (const f of this.listeners[name] || []) { try { f(payload); } catch (e) { console.error("graphkit listener", e); } } }
  isFs() { return document.fullscreenElement === this.root; }
  N() { return this.cy.nodes(".gk-n"); }
  nodeCol(n) { return n.data("col"); }
  vis(n) { return !n.hasClass("gk-hide"); }

  btn(label, onClick, o = {}) {
    const b = el("button", this.cls(o.primary ? "buttonPrimary" : "button", o.primary ? "gk-v-btnp" : "gk-v-btn"), label);
    b.type = "button";
    if (o.title) { b.title = o.title; b.setAttribute("aria-label", o.aria || o.title); }
    if (o.cls) b.classList.add(o.cls);
    b.addEventListener("click", onClick);
    return b;
  }
  select(items, value, onChange, aria) {
    const s = el("select", this.cls("select", "gk-v-sel"));
    s.setAttribute("aria-label", aria);
    for (const [v, l] of items) { const op = el("option", null, l); op.value = v; s.appendChild(op); }
    s.value = value;
    s.addEventListener("change", () => onChange(s.value));
    return s;
  }
  segmented(items, value, onChange, aria) {
    const w = el("span", this.cls("segmented", "gk-v-seg") + " gk-seg");
    w.setAttribute("role", "group"); w.setAttribute("aria-label", aria);
    const bs = [];
    const set = (v) => bs.forEach(([b, bv]) => { const on = bv === v; b.className = on ? this.cls("segmentedItemActive", "gk-on") : this.cls("segmentedItem", ""); if (on) b.classList.add("gk-on"); b.setAttribute("aria-pressed", String(on)); });
    for (const [v, l] of items) {
      const b = el("button", ""); b.type = "button"; b.textContent = l;
      b.addEventListener("click", () => { set(v); onChange(v); });
      w.appendChild(b); bs.push([b, v]);
    }
    set(value);
    w.set = set;
    return w;
  }
  checkbox(label, checked, onChange) {
    const l = el("label", "gk-chk");
    const i = el("input"); i.type = "checkbox"; i.checked = checked;
    i.addEventListener("change", () => onChange(i.checked));
    l.append(i, el("span", null, label));
    l.input = i;
    return l;
  }
  field(label, ctl) { const f = el("label", "gk-field"); f.append(el("span", null, label), ctl); return f; }

  /* ---------------------------------------------------------------- DOM */
  build() {
    const c = this.container;
    const root = (this.root = el("div", "gk-root"));
    root.setAttribute("data-gk-skin", this.sk.name);
    root.setAttribute("role", "region");
    root.setAttribute("aria-label", "Graph view");
    this.banner = el("div", "gk-banner"); this.banner.hidden = true;

    // toolbar
    const tb = (this.toolbar = el("div", "gk-toolbar"));
    const sw = el("span", "gk-search");
    this.searchIn = el("input", this.cls("input", "gk-v-in"));
    this.searchIn.type = "search"; this.searchIn.placeholder = "Find node…"; this.searchIn.setAttribute("aria-label", "Find node by label or IRI");
    this.searchCount = el("span", "gk-count");
    this.searchPrev = this.btn("↑", () => this.stepMatch(-1), { title: "Previous match" });
    this.searchNext = this.btn("↓", () => this.stepMatch(1), { title: "Next match" });
    sw.append(this.searchIn, this.searchCount, this.searchPrev, this.searchNext);
    this.layoutSel = this.select(LAYOUTS, this.st.layout, (v) => this.runLayout(v), "Layout");
    this.runBtn = this.btn("Run", () => this.runLayout(), { title: "Run the layout again", aria: "Run layout again" });
    this.dirSeg = this.segmented([["TB", "Down"], ["BT", "Up"], ["LR", "Right"]], this.st.dir, (v) => { this.st.dir = v; this.runLayout("dagre"); }, "Hierarchy direction");
    this.groupSel = this.select(GROUPS, this.st.groupBy, (v) => this.setGroupBy(v), "Group by");
    this.viewBtn = this.btn("View ▾", () => this.togglePop("view"), { title: "Colour, size, labels" });
    this.filtBtn = this.btn("Filters ▾", () => this.togglePop("filters"), { title: "Degree, node types, predicates" });
    this.expBtn = this.btn("Export ▾", () => this.togglePop("export"), { title: "Export the visible graph" });
    this.fsBtn = this.btn("Fullscreen", () => this.toggleFullscreen(), { title: "Toggle fullscreen" });
    this.chips = el("span", "gk-field");
    this.statusEl = el("span", "gk-status"); this.statusEl.setAttribute("aria-live", "polite");
    tb.append(sw, this.field("Layout", this.layoutSel), this.runBtn, this.dirSeg, this.field("Group by", this.groupSel), this.viewBtn, this.filtBtn, this.expBtn, el("span", "gk-spacer"), this.chips, this.statusEl, this.fsBtn);

    // stage
    const main = (this.main = el("div", "gk-main"));
    const stage = (this.stage = el("div", "gk-stage"));
    stage.tabIndex = 0;
    stage.setAttribute("aria-label", "Graph canvas. Plus and minus zoom, arrow keys pan, 0 resets zoom, F fits.");
    this.decoSvg = svg("svg", { class: "gk-deco", "aria-hidden": "true" });
    this.world = svg("g", {});
    this.decoSvg.appendChild(this.world);
    this.cyEl = el("div", "gk-cy");
    this.chordEl = el("div", "gk-chord"); this.chordEl.hidden = true;
    const zoom = (this.zoomBox = el("div", "gk-zoom"));
    this.pct = el("span", "gk-pct", "100%");
    zoom.append(this.btn("+", () => this.zoomBy(1.4), { title: "Zoom in" }), this.pct, this.btn("−", () => this.zoomBy(1 / 1.4), { title: "Zoom out" }),
      this.btn("Fit", () => this.fit(), { title: "Fit the graph in view" }), this.btn("1:1", () => this.resetZoom(), { title: "Reset zoom to 100%" }));
    this.legends = el("div", "gk-legends");
    this.miniBox = el("div", "gk-mini");
    const mx = el("button", "gk-x", "×"); mx.type = "button"; mx.title = "Hide minimap"; mx.setAttribute("aria-label", "Hide minimap");
    mx.addEventListener("click", () => this.set({ minimap: false }));
    this.miniBox.appendChild(mx);
    this.hint = el("div", "gk-hint", (/Mac|iPhone|iPad/.test(navigator.platform || "") ? "Use ⌘ + scroll to zoom" : "Use Ctrl + scroll to zoom"));
    this.hint.setAttribute("aria-hidden", "true");
    this.busy = el("div", "gk-busy"); this.busy.hidden = true;
    this.busyText = el("span", null, "Layout running…");
    this.busy.append(el("i"), this.busyText, this.btn("Stop", () => this.stopLayout(), { cls: "gk-stop" }));
    this.toast = el("div", "gk-toast"); this.toast.setAttribute("role", "status");
    this.tipEl = el("div", "gk-tip"); this.tipEl.hidden = true;
    stage.append(this.decoSvg, this.cyEl, this.chordEl, zoom, this.legends, this.miniBox, this.hint, this.busy, this.toast, this.tipEl);

    this.side = el("aside", this.cls("panel", "gk-v-panel") + " gk-side"); this.side.hidden = true;
    this.side.setAttribute("aria-label", "Node details");
    main.append(stage, this.side);
    root.append(this.banner, tb, main);
    c.appendChild(root);
    this.scrollables = [];
  }

  setVars() {
    const t = this.tokens, s = this.root.style;
    s.setProperty("--gk-surface", t.surface); s.setProperty("--gk-surface2", t.surface2); s.setProperty("--gk-text", t.text);
    s.setProperty("--gk-text-muted", t.textMuted); s.setProperty("--gk-border", t.border); s.setProperty("--gk-accent", t.accent);
    s.setProperty("--gk-font-sans", t.fontSans); s.setProperty("--gk-font-mono", t.fontMono); s.setProperty("--gk-radius", t.radius);
    this.root.classList.toggle("gk-dark", this.dark);
    this.root.style.colorScheme = this.dark ? "dark" : "light";
  }
  th() {
    const t = this.tokens;
    return { surface: normColor(t.surface), surface2: normColor(t.surface2), text: normColor(t.text), muted: normColor(t.textMuted), border: normColor(t.border), accent: normColor(t.accent), font: t.fontSans, accentRaw: normColor(t.accent) };
  }

  /* ---------------------------------------------------------------- cytoscape */
  styleSheet() {
    const t = this.th();
    const lblFont = String(this.tokens.fontSans).replace(/'/g, '"').replace(/[^A-Za-z0-9_ ",-]/g, "");
    return [
      { selector: "node", style: { "background-color": "data(col)", shape: "data(shp)", width: "data(size)", height: "data(size)", label: "data(lbl)", color: t.text, "font-size": 11, "font-family": lblFont, "text-valign": "bottom", "text-margin-y": 3, "text-outline-color": t.surface, "text-outline-width": 2.5, "text-outline-opacity": 1, "min-zoomed-font-size": 6, "text-wrap": "ellipsis", "text-max-width": 110, "border-width": 1.5, "border-color": "data(bc)", "overlay-opacity": 0, "z-index": 10 } },
      { selector: 'node[kind="literal"]', style: { "border-style": "dashed", "border-color": t.text, "border-width": 1.4 } },
      { selector: 'node[kind="bnode"]', style: { "border-style": "dotted", "border-color": t.text, "border-width": 2 } },
      { selector: "node.gk-nolabel", style: { label: "" } },
      { selector: "node.gk-ring", style: { "border-width": 1 } },
      { selector: "node.gk-pinned", style: { "border-width": 3, "border-color": t.text } },
      { selector: "node.gk-match", style: { "border-width": 3.5, "border-color": t.accent, "underlay-color": t.accent, "underlay-opacity": 0.2, "underlay-padding": 5 } },
      { selector: "node.gk-cur", style: { "border-width": 4.5, "border-color": t.accent, "underlay-color": t.accent, "underlay-opacity": 0.35, "underlay-padding": 8, label: "data(lbl)" } },
      { selector: "node:selected", style: { "border-width": 3.5, "border-color": t.accent, "underlay-color": t.accent, "underlay-opacity": 0.28, "underlay-padding": 6, label: "data(lbl)", "z-index": 40 } },
      { selector: "node.gk-hl", style: { label: "data(lbl)", "z-index": 30 } },
      { selector: "node.gk-box", style: { shape: "round-rectangle", "background-color": "data(col)", "background-opacity": 0.07, "border-width": 1.5, "border-color": "data(col)", "border-style": "dashed", "border-opacity": 0.8, label: "data(lbl)", "text-valign": "top", "text-halign": "center", "font-size": 13, "font-weight": 600, padding: 16, events: "no", "z-index": 1, "text-margin-y": -4, "min-zoomed-font-size": 0 } },
      { selector: "edge", style: { "curve-style": "bezier", width: "data(w)", "line-color": "data(col)", "line-style": "data(ls)", "target-arrow-shape": "triangle", "target-arrow-color": "data(col)", "arrow-scale": 0.7, opacity: 0.55, "z-index": 5, "font-size": 9.5, "font-family": lblFont, color: t.text, "text-outline-color": t.surface, "text-outline-width": 2.5, "text-outline-opacity": 1, "text-rotation": "autorotate", "min-zoomed-font-size": 6, "loop-direction": "-45deg", "loop-sweep": "40deg", "overlay-opacity": 0 } },
      { selector: "edge.gk-hay", style: { "curve-style": "haystack", "haystack-radius": 0, "target-arrow-shape": "none" } },
      { selector: "edge.gk-curved", style: { "target-arrow-shape": "none", opacity: 0.4 } },
      { selector: "edge.gk-elbl", style: { label: "data(lbl)" } },
      { selector: "edge:selected", style: { label: "data(lbl)", width: 3, opacity: 1, "line-color": t.accent, "target-arrow-color": t.accent, "z-index": 50 } },
      { selector: "edge.gk-hl", style: { opacity: 1, width: 2, "z-index": 30, label: "data(lbl)" } },
      { selector: ".gk-dim", style: { opacity: 0.1, "text-opacity": 0 } },
      { selector: ".gk-faded", style: { opacity: 0.18, "text-opacity": 0.4 } },
      { selector: "node.gk-path", style: { "border-width": 4, "border-color": t.accent, "underlay-color": t.accent, "underlay-opacity": 0.3, "underlay-padding": 6, label: "data(lbl)", opacity: 1, "text-opacity": 1, "z-index": 60 } },
      { selector: "edge.gk-path", style: { "line-color": t.accent, "target-arrow-color": t.accent, width: 4, opacity: 1, label: "data(lbl)", "z-index": 55 } },
      { selector: ".gk-hide", style: { display: "none" } },
    ];
  }

  initCy() {
    this.cy = cytoscape({
      container: this.cyEl, elements: [], style: this.styleSheet(), minZoom: 0.02, maxZoom: 6,
      boxSelectionEnabled: true, selectionType: "single", autounselectify: false, textureOnViewport: false, motionBlur: false,
    });
    this.cy.userZoomingEnabled(true); // pinch on touch; wheel is intercepted before it reaches cytoscape
    this.mini = new Minimap(this.miniBox, this.cy, (n) => this.nodeCol(n), () => this.th());
  }

  /* ---------------------------------------------------------------- events */
  bindEvents() {
    const cy = this.cy, stage = this.stage;
    const onWheel = (ev) => {
      if (this.chordMode) return;
      const ok = ev.ctrlKey || ev.metaKey || this.active || this.isFs();
      if (!ok) { ev.stopPropagation(); this.showHint(); return; } // the page scrolls
      ev.preventDefault(); ev.stopPropagation();
      let dy = ev.deltaMode === 1 ? ev.deltaY * 16 : ev.deltaY;
      dy = Math.max(-80, Math.min(80, dy));
      const k = ev.ctrlKey ? (Math.abs(dy) < 30 ? 0.012 : 0.0045) : 0.0018;
      const r = this.cyEl.getBoundingClientRect();
      this.zoomBy(Math.exp(-dy * k), { x: ev.clientX - r.left, y: ev.clientY - r.top }, true);
    };
    stage.addEventListener("wheel", onWheel, { capture: true, passive: false });
    const setActive = (v) => { this.active = v; stage.classList.toggle("gk-active", v && !this.isFs()); };
    stage.addEventListener("pointerdown", () => { setActive(true); if (document.activeElement !== stage && !this.root.querySelector(":focus-within input")) stage.focus({ preventScroll: true }); }, true);
    const docDown = (ev) => {
      if (!this.root.contains(ev.target) && !(ev.composedPath && ev.composedPath().includes(this.root))) { setActive(false); this.closePop(); }
      else if (this.pop && !this.pop.contains(ev.target) && !ev.target.closest(".gk-toolbar")) this.closePop();
    };
    document.addEventListener("pointerdown", docDown, true);
    this.cleanups.push(() => document.removeEventListener("pointerdown", docDown, true));
    const fsch = () => { setActive(this.active || this.isFs()); this.fsBtn.textContent = this.isFs() ? "Exit fullscreen" : "Fullscreen"; this.root.classList.toggle("gk-fs", this.isFs()); this.onResize(); };
    document.addEventListener("fullscreenchange", fsch);
    this.cleanups.push(() => document.removeEventListener("fullscreenchange", fsch));
    this.ro = new ResizeObserver(() => this.onResize());
    this.ro.observe(this.stage);

    stage.addEventListener("keydown", (ev) => {
      if (ev.target !== stage || ev.altKey || ev.ctrlKey || ev.metaKey) return;
      const k = ev.key;
      if (k === "+" || k === "=") this.zoomBy(1.3);
      else if (k === "-" || k === "_") this.zoomBy(1 / 1.3);
      else if (k === "0") this.resetZoom();
      else if (k === "f" || k === "F") this.fit();
      else if (k === "ArrowLeft") cy.panBy({ x: 60, y: 0 });
      else if (k === "ArrowRight") cy.panBy({ x: -60, y: 0 });
      else if (k === "ArrowUp") cy.panBy({ x: 0, y: 60 });
      else if (k === "ArrowDown") cy.panBy({ x: 0, y: -60 });
      else if (k === "Escape") { cy.elements().unselect(); this.clearPath(); this.clearSearch(); }
      else return;
      ev.preventDefault();
    });

    cy.on("viewport", () => { this.syncDeco(); this.pct.textContent = Math.round(cy.zoom() * 100) + "%"; this.labelTimer(); this.hideTip(); });
    let selT = 0;
    cy.on("select unselect", () => { clearTimeout(selT); selT = setTimeout(() => this.onSelection(), 0); });
    cy.on("mouseover", "node.gk-n", (e) => { if (this.dragging) return; this.hoverNode(e.target); this.tipTimer(e); });
    cy.on("mouseout", "node.gk-n", () => { this.clearHl(); this.hideTip(); });
    cy.on("mouseover", "edge", (e) => { this.tipTimer(e); });
    cy.on("mouseout", "edge", () => this.hideTip());
    cy.on("grab", () => { this.dragging = true; this.clearHl(); this.hideTip(); });
    cy.on("free", () => { this.dragging = false; });
    cy.on("tap", (e) => { if (e.target === cy) { /* background click keeps the page scrollable but clears selection */ } });
    cy.on("dbltap", "node.gk-n", (e) => this.open(e.target.id()));

    this.searchIn.addEventListener("input", () => this.search(this.searchIn.value));
    this.searchIn.addEventListener("keydown", (ev) => {
      if (ev.key === "Enter") { ev.preventDefault(); this.stepMatch(ev.shiftKey ? -1 : 1, true); }
      else if (ev.key === "Escape") { this.clearSearch(); this.searchIn.blur(); }
    });
    this.container.addEventListener("keydown", (ev) => { if (ev.key === "Escape" && this.pop) { this.closePop(); } });
  }

  onResize() {
    if (!this.cy) return;
    this.cy.resize();
    if (this.chordMode) this.drawChord();
    this.mini && this.mini.schedule();
    if (this._needFit && this.cyEl.clientWidth > 40) { this._needFit = false; this.fit(false); }
  }

  showHint() {
    this.hint.classList.add("gk-show");
    clearTimeout(this._hintT);
    this._hintT = setTimeout(() => this.hint.classList.remove("gk-show"), 1300);
  }
  say(msg, ms = 2400) {
    this.toast.textContent = msg; this.toast.classList.add("gk-show");
    clearTimeout(this._toastT);
    this._toastT = setTimeout(() => this.toast.classList.remove("gk-show"), ms);
  }

  /* ---------------------------------------------------------------- tooltip */
  tipTimer(e) {
    clearTimeout(this._tipT);
    const ele = e.target, rp = e.renderedPosition || { x: 0, y: 0 };
    this._tipT = setTimeout(() => this.showElTip(ele, rp.x, rp.y), 220);
  }
  showElTip(ele, x, y) {
    if (!ele || ele.removed() || this.dragging) return;
    const t = this.tipEl; clear(t);
    if (ele.isNode()) {
      const n = this.model.nodes.get(ele.id());
      if (!n) return;
      t.appendChild(el("b", null, trunc(this.model.displayLabel(n, this.o.labelFor), 120)));
      if (n.kind === "iri") t.appendChild(el("div", "gk-m", shortIri(n.iri, 70)));
      else t.appendChild(el("div", null, n.kind === "literal" ? "Literal" + (n.lang ? " @" + n.lang : n.datatype ? " ^^" + localName(n.datatype) : "") : "Blank node"));
      if (n.types.length) t.appendChild(el("div", null, "Types: " + n.types.slice(0, 4).map(localName).join(", ") + (n.types.length > 4 ? " +" + (n.types.length - 4) : "")));
      t.appendChild(el("div", null, "Degree " + n.deg + " (" + n.indeg + " in, " + n.outdeg + " out)"));
    } else {
      const e = this.model.edges.get(ele.data("mid"));
      if (!e) return;
      t.appendChild(el("b", null, e.label || localName(e.predicate)));
      if (e.predicate) t.appendChild(el("div", "gk-m", shortIri(e.predicate, 70)));
    }
    this.placeTip(x, y);
  }
  placeTip(x, y) {
    const t = this.tipEl; t.hidden = false;
    const W = this.stage.clientWidth, H = this.stage.clientHeight;
    let l = x + 14, tp = y + 14;
    if (l + t.offsetWidth > W - 4) l = Math.max(4, x - t.offsetWidth - 10);
    if (tp + t.offsetHeight > H - 4) tp = Math.max(4, y - t.offsetHeight - 10);
    t.style.left = l + "px"; t.style.top = tp + "px";
  }
  hideTip() { clearTimeout(this._tipT); this.tipEl.hidden = true; }
  chordTip() {
    const rel = (ev) => { const r = this.stage.getBoundingClientRect(); return { x: ev.clientX - r.left, y: ev.clientY - r.top }; };
    return {
      show: (ev, lines) => { clear(this.tipEl); lines.forEach((l, i) => this.tipEl.appendChild(i ? el("div", null, l) : el("b", null, l))); const p = rel(ev); this.placeTip(p.x, p.y); },
      move: (ev) => { const p = rel(ev); this.placeTip(p.x, p.y); },
      hide: () => this.hideTip(),
    };
  }

  /* ---------------------------------------------------------------- data */
  setData(data) {
    this.cy.elements().remove();
    this.model.clear();
    this.hiddenSlots.clear(); this.hiddenPreds.clear(); this.hiddenNodes.clear(); this.pinned.clear(); this.expanded.clear(); this.focus = null;
    this.slotMap = new Map(); this.predMap = new Map();
    this.clearPath(true); this.clearSearch(true);
    this.st.minDeg = 0;
    const added = this.model.add(data || {});
    this.computeGroups(true);
    this.addElements(added, null);
    this.perfMode();
    this.applyEncoding();
    this.applyFilters(true);
    this.refreshUi();
    this._needFit = true;
    const p = this.runLayout(this.st.layout, { noAnimate: true });
    this.emit("data", this.stats());
    return p;
  }

  addData(data, o = {}) {
    const added = this.model.add(data || {});
    if (!added.nodes.length && !added.edges.length) return Promise.resolve(added);
    this.computeGroups(false);
    this.addElements(added, o.near || null);
    this.perfMode();
    this.applyEncoding();
    this.applyFilters(true);
    this.refreshUi();
    this.emit("data", this.stats());
    if (o.layout === false) return Promise.resolve(added);
    return this.runLayout(this.st.layout, { incremental: true, noFit: !!o.near }).then(() => added);
  }

  stats() { return { nodes: this.model.nodes.size, edges: this.model.edges.size, visibleNodes: this.N().not(".gk-hide").length, visibleEdges: this.cy.edges().not(".gk-hide").length }; }

  addElements(added, near) {
    const els = [];
    const base = near && this.cy.getElementById(near).nonempty() ? this.cy.getElementById(near).position() : { x: 0, y: 0 };
    const spread = near ? 70 : Math.sqrt(Math.max(1, added.nodes.length)) * 22;
    let i = 0; const m = added.nodes.length;
    for (const n of added.nodes) {
      const a = (i++ / Math.max(m, 1)) * Math.PI * 2, r = near ? spread : spread * Math.sqrt(i / Math.max(m, 1));
      els.push({ group: "nodes", data: { id: n.id, kind: n.kind, lbl: "", deg: 0, size: 20, col: "#888", shp: "ellipse", slot: 0 }, position: { x: base.x + Math.cos(a) * r, y: base.y + Math.sin(a) * r }, classes: "gk-n" });
    }
    const big = this.model.edges.size > 800;
    for (const e of added.edges) els.push({ group: "edges", data: { id: "e|" + e.id, mid: e.id, source: e.source, target: e.target, lbl: trunc(e.label, 40), col: "#888", ls: "solid", pred: e.predicate }, classes: big ? "gk-hay" : "" });
    this.cy.add(els);
    if (big) this.cy.edges().not(".gk-hay").addClass("gk-hay");
    else this.cy.edges(".gk-hay").removeClass("gk-hay");
  }

  perfMode() {
    const nN = this.model.nodes.size, nE = this.model.edges.size;
    const big = nN > 800 || nE > 1500;
    try {
      const r = this.cy.renderer();
      r.hideLabelsOnViewport = nN > 600;
      r.hideEdgesOnViewport = nE > 2500;
    } catch (e) { /* internals differ: ignore */ }
    if (nN > MAX_NODES_SOFT || nE > MAX_EDGES_SOFT) {
      clear(this.banner);
      const s = el("span", null, "Large graph (" + fmt(nN) + " nodes, " + fmt(nE) + " edges): labels and arrows are simplified and layouts take a while. Filters, Focus and Group by help.");
      const x = el("button", null, "×"); x.type = "button"; x.setAttribute("aria-label", "Dismiss");
      x.addEventListener("click", () => { this.banner.hidden = true; this.onResize(); });
      this.banner.append(s, x); this.banner.hidden = false;
    } else this.banner.hidden = true;
    this.big = big;
  }

  /* ---------------------------------------------------------------- groups and encoding */
  computeGroups(reset) {
    const mode = this.st.groupBy;
    if (reset) this.slotMap = new Map();
    const counts = new Map(), names = new Map();
    for (const n of this.model.nodes.values()) {
      const g = this.model.groupOf(n, mode);
      n._gk = g.key; counts.set(g.key, (counts.get(g.key) || 0) + 1); names.set(g.key, g.name);
    }
    const used = new Set(this.slotMap.values());
    const fresh = [...counts.keys()].filter((k) => !this.slotMap.has(k)).sort((a, b) => counts.get(b) - counts.get(a) || (a < b ? -1 : 1));
    // fixed order, never cycled: the 9th group and everything after folds into Other
    let next = 0;
    for (const k of fresh) {
      while (used.has(next) && next < OTHER) next++;
      if (next < OTHER) { this.slotMap.set(k, next); used.add(next); } else this.slotMap.set(k, OTHER);
    }
    const bySlot = new Map();
    for (const [k, c] of counts) {
      const s = this.slotMap.get(k);
      const g = bySlot.get(s) || { slot: s, name: "", count: 0, top: 0 };
      g.count += c;
      if (s === OTHER) g.name = "Other"; else if (c >= g.top) { g.top = c; g.name = names.get(k); }
      bySlot.set(s, g);
    }
    this.groups = [...bySlot.values()].sort((a, b) => a.slot - b.slot);
    // predicates
    const pc = new Map();
    for (const e of this.model.edges.values()) pc.set(e.predicate, (pc.get(e.predicate) || 0) + 1);
    const pu = new Set(this.predMap.values());
    const pf = [...pc.keys()].filter((k) => !this.predMap.has(k)).sort((a, b) => pc.get(b) - pc.get(a) || (a < b ? -1 : 1));
    let pn = 0;
    for (const k of pf) { while (pu.has(pn) && pn < OTHER) pn++; if (pn < OTHER) { this.predMap.set(k, pn); pu.add(pn); } else this.predMap.set(k, OTHER); }
    this.preds = [...pc.entries()].map(([p, c]) => ({ pred: p, count: c, slot: this.predMap.get(p), name: p ? localName(p) : "(no predicate)" })).sort((a, b) => b.count - a.count);
  }
  slotOfNode(n) { const s = this.slotMap.get(n._gk); return s == null ? OTHER : s; }

  applyEncoding() {
    const cy = this.cy, st = this.st, th = this.th();
    const nodes = this.model.nodes;
    const nN = nodes.size;
    const f = nN > 1500 ? 0.62 : nN > 600 ? 0.8 : 1;
    let maxM = 1, maxDeg = 1;
    const metric = (n) => (st.sizeBy === "in" ? n.indeg : st.sizeBy === "out" ? n.outdeg : n.deg);
    for (const n of nodes.values()) { maxM = Math.max(maxM, metric(n)); maxDeg = Math.max(maxDeg, n.deg); }
    this.maxDeg = maxDeg;
    const one = this.groups.length < 2;
    cy.batch(() => {
      cy.nodes(".gk-n").forEach((cn) => {
        const n = nodes.get(cn.id());
        if (!n) return;
        const slot = this.slotOfNode(n);
        let col;
        if (st.colorBy === "group") col = one ? normColor(this.pal()[0]) : this.slotColor(slot);
        else if (st.colorBy === "degree") col = SEQ[Math.min(SEQ.length - 1, Math.floor(Math.sqrt(n.deg / maxDeg) * SEQ.length))];
        else col = th.muted;
        const size = (st.sizeBy === "constant" ? 20 : 10 + 30 * Math.sqrt(metric(n) / maxM)) * f * (n.kind === "literal" ? 0.8 : 1);
        cn.data({ col, bc: st.colorBy === "degree" ? th.muted : th.surface, bsize: size, size, shp: one ? "ellipse" : SHAPES[slot], slot, deg: n.deg, lbl: trunc(this.model.displayLabel(n, this.o.labelFor), 46) });
      });
      cy.edges().forEach((ce) => {
        const slot = this.predMap.get(ce.data("pred"));
        ce.data({ w: 1.1, col: st.edgeColor ? this.slotColor(slot == null ? OTHER : slot) : th.muted, ls: st.edgeColor ? DASHES[slot == null ? OTHER : slot] : "solid" });
      });
      cy.nodes(".gk-box").forEach((b) => b.data("col", this.slotColor(b.data("slot"))));
      this.updateLod(true);
    });
    this.mini && this.mini.schedule();
  }

  // Level of detail: when zoomed far out, nodes and lines grow in model units so they stay visible.
  updateLod(force, neutral) {
    const z = this.cy.zoom();
    const fn = neutral ? 1 : z < 0.4 ? Math.min(3.2, Math.pow(0.4 / z, 0.65)) : 1;
    const fe = neutral ? 1 : z < 0.6 ? Math.min(9, 0.6 / z) : 1;
    const old = this.lod || { n: 1, e: 1 };
    if (!force && Math.abs(fn / old.n - 1) < 0.12 && Math.abs(fe / old.e - 1) < 0.12) return;
    this.lod = { n: fn, e: fe };
    this.cy.batch(() => {
      this.cy.nodes(".gk-n").forEach((n) => n.data("size", (n.hasClass("gk-ring") ? 9 : n.data("bsize") || 20) * fn));
      this.cy.edges().forEach((e) => e.data("w", (e.hasClass("gk-curved") ? 0.9 : 1.1) * fe));
    });
  }

  setGroupBy(v) {
    this.st.groupBy = v; this.groupSel.value = v;
    this.hiddenSlots.clear();
    this.computeGroups(true);
    this.applyEncoding(); this.applyFilters(true); this.refreshUi();
    if (this.chordMode) this.drawChord();
    else if (this.st.layout === "hive" || this.st.layout === "circos" || this.st.boxes) this.runLayout();
    this.emit("groupby", v);
  }

  /* ---------------------------------------------------------------- filters */
  applyFilters(skipLabels) {
    const st = this.st, hide = new Set();
    for (const n of this.model.nodes.values()) {
      const slot = this.slotOfNode(n);
      if (this.hiddenSlots.has(slot) || n.deg < st.minDeg || (st.hideLiterals && n.kind === "literal") || (st.hideBnodes && n.kind === "bnode") || this.hiddenNodes.has(n.id) || (this.focus && !this.focus.has(n.id))) hide.add(n.id);
    }
    const hp = this.hiddenPreds;
    this.cy.batch(() => {
      this.cy.nodes(".gk-n").forEach((cn) => cn.toggleClass("gk-hide", hide.has(cn.id())));
      this.cy.edges().forEach((ce) => ce.toggleClass("gk-hide", hide.has(ce.data("source")) || hide.has(ce.data("target")) || hp.has(ce.data("pred"))));
      this.cy.nodes(".gk-box").forEach((b) => b.toggleClass("gk-hide", b.children().not(".gk-hide").empty()));
    });
    if (!skipLabels) this.updateLabels();
    this.updateStatus();
    this.mini && this.mini.schedule();
    if (this.chordMode) this.drawChord();
    if (this.pathActive) this.clearPath(true);
    this.emit("filter", this.stats());
  }

  updateStatus() {
    const s = this.stats();
    this.statusEl.textContent = (s.visibleNodes === s.nodes ? fmt(s.nodes) : fmt(s.visibleNodes) + " of " + fmt(s.nodes)) + " nodes · " + (s.visibleEdges === s.edges ? fmt(s.edges) : fmt(s.visibleEdges) + " of " + fmt(s.edges)) + " edges";
    clear(this.chips);
    if (this.hiddenNodes.size) this.chips.appendChild(this.btn("Show " + this.hiddenNodes.size + " hidden", () => { this.hiddenNodes.clear(); this.applyFilters(); }, { title: "Show nodes you hid" }));
    if (this.focus) this.chips.appendChild(this.btn("Exit focus", () => this.setFocus(null), { title: "Show the whole graph again" }));
    if (this.pathActive) this.chips.appendChild(this.btn("Clear path", () => this.clearPath(), { title: "Clear the highlighted path" }));
  }

  /* labels: Auto shows the top-degree nodes always and the rest when zoomed in */
  labelTimer() { clearTimeout(this._lblT); this._lblT = setTimeout(() => { this.updateLabels(); this.updateLod(); }, 90); }
  updateLabels() {
    const mode = this.st.labels, cy = this.cy;
    const vis = this.N().not(".gk-hide");
    const n = vis.length;
    cy.batch(() => {
      if (mode === "all") { vis.removeClass("gk-nolabel"); return; }
      if (mode === "none") { vis.addClass("gk-nolabel"); return; }
      const z = cy.zoom();
      const zt = n <= 120 ? 0 : n <= 400 ? 0.7 : n <= 1000 ? 1.2 : 1.8;
      if (z >= zt && n <= 400) { vis.removeClass("gk-nolabel"); return; }
      const k = n <= 120 ? n : Math.max(8, Math.round(n * (z >= zt ? 0.25 : 0.03)));
      const sorted = vis.toArray().sort((a, b) => b.data("deg") - a.data("deg"));
      const top = new Set(sorted.slice(0, k).map((x) => x.id()));
      const ext = cy.extent();
      vis.forEach((x) => {
        let show = top.has(x.id());
        if (!show && z >= zt) { const p = x.position(); show = p.x > ext.x1 && p.x < ext.x2 && p.y > ext.y1 && p.y < ext.y2; }
        x.toggleClass("gk-nolabel", !show);
      });
    });
  }

  /* ---------------------------------------------------------------- highlight */
  hoverNode(n) {
    this.clearHl();
    const nb = n.closedNeighborhood().not(".gk-hide");
    this.cy.batch(() => {
      this.cy.elements().not(nb).not(".gk-hide").addClass("gk-dim");
      nb.addClass("gk-hl");
    });
    this.emit("hover", { id: n.id() });
  }
  clearHl() { this.cy.elements(".gk-dim, .gk-hl").removeClass("gk-dim gk-hl"); }
  hlSlot(slot) {
    this.clearHl();
    this.cy.batch(() => {
      const inn = this.N().filter((n) => n.data("slot") === slot);
      const ed = inn.connectedEdges().filter((e) => e.source().data("slot") === slot && e.target().data("slot") === slot);
      this.cy.elements().not(inn).not(ed).not(".gk-hide").addClass("gk-dim");
      inn.addClass("gk-hl"); ed.addClass("gk-hl");
    });
  }

  /* ---------------------------------------------------------------- selection and panel */
  onSelection() {
    const sel = this.cy.elements(":selected");
    const nodes = sel.nodes().not(".gk-box");
    this.cy.edges(".gk-elbl").removeClass("gk-elbl");
    const ne = nodes.length === 1 ? nodes.connectedEdges().not(".gk-hide") : null;
    if (ne && ne.length <= 60) ne.addClass("gk-elbl");
    this.renderPanel();
    this.emit(nodes.length ? "select" : "unselect", { ids: nodes.map((n) => n.id()), nodes: nodes.map((n) => this.model.nodes.get(n.id())) });
  }
  selectedIds() { return this.cy.nodes(":selected").not(".gk-box").map((n) => n.id()); }
  selectNodes(ids) { this.cy.elements().unselect(); for (const id of ids) this.cy.getElementById(id).select(); }

  renderPanel() {
    const side = this.side;
    const nodes = this.cy.nodes(":selected").not(".gk-box");
    const edges = this.cy.edges(":selected");
    clear(side);
    if (!nodes.length && !edges.length) { side.hidden = true; this.main.classList.remove("gk-panelopen"); this.miniBox.style.right = ""; this.onResize(); return; }
    side.hidden = false; this.main.classList.add("gk-panelopen");
    this.miniBox.style.right = (side.offsetWidth + 16) + "px";
    const head = el("header");
    const title = el("h3");
    const close = el("button", "gk-xbtn", "×"); close.type = "button"; close.setAttribute("aria-label", "Close details");
    close.addEventListener("click", () => this.cy.elements().unselect());
    const body = el("div", "gk-body gk-scroll");
    if (nodes.length === 1) {
      const id = nodes[0].id(), n = this.model.nodes.get(id);
      title.textContent = trunc(this.model.displayLabel(n, this.o.labelFor), 200);
      head.append(title, close); side.append(head, body);
      const meta = el("div", "gk-row");
      meta.append(el("span", this.cls("badge", "gk-v-badge"), n.kind === "iri" ? "IRI" : n.kind === "bnode" ? "Blank node" : "Literal"), el("span", this.cls("badge", "gk-v-badge"), "degree " + n.deg + " (" + n.indeg + " in, " + n.outdeg + " out)"));
      body.appendChild(meta);
      if (n.kind === "iri") {
        if (isHttp(n.iri)) { const b = el("button", "gk-link gk-iri", n.iri); b.type = "button"; b.title = "Open"; b.addEventListener("click", () => this.open(id)); body.appendChild(b); }
        else body.appendChild(el("div", "gk-iri", n.iri));
      } else if (n.kind === "literal") {
        body.appendChild(el("div", "gk-iri", trunc((n.value != null ? n.value : n.label), 600) + (n.lang ? " @" + n.lang : n.datatype ? " ^^" + localName(n.datatype) : "")));
      }
      if (n.types.length) {
        body.appendChild(el("h5", null, "Types"));
        const row = el("div", "gk-row");
        for (const t of n.types.slice(0, 12)) { const b = el("span", this.cls("badge", "gk-v-badge"), localName(t)); b.title = t; row.appendChild(b); }
        body.appendChild(row);
      }
      const keys = Object.keys(n.props);
      if (keys.length) {
        body.appendChild(el("h5", null, "Properties"));
        const tbl = el("table"); const tb = el("tbody");
        for (const k of keys.slice(0, 80)) {
          const tr = el("tr"), td = el("td", null, trunc(k, 60));
          td.title = k;
          const v = n.props[k], vals = Array.isArray(v) ? v : [v];
          const vd = el("td");
          vals.slice(0, 8).forEach((x, i) => {
            const s = String(x == null ? "" : x);
            if (i) vd.appendChild(document.createElement("br"));
            if (isHttp(s)) { const b = el("button", "gk-link", trunc(shortIri(s, 90), 90)); b.type = "button"; b.title = s; b.addEventListener("click", () => this.open(s)); vd.appendChild(b); }
            else vd.appendChild(document.createTextNode(trunc(s, 400)));
          });
          if (vals.length > 8) vd.appendChild(el("div", "gk-note", "+" + (vals.length - 8) + " more"));
          tr.append(td, vd); tb.appendChild(tr);
        }
        tbl.appendChild(tb); body.appendChild(tbl);
      }
      const g = el("div", "gk-btns");
      const canExpand = typeof this.o.onExpand === "function" && n.kind !== "literal";
      const ex = (d, l) => { const b = this.btn(l, () => this.expand(id, d)); b.disabled = !canExpand; return b; };
      g.append(ex("out", "Expand out"), ex("in", "Expand in"));
      const both = ex("both", "Expand both"); both.classList.add("gk-wide"); g.appendChild(both);
      g.append(this.btn("Focus 1-hop", () => this.focusHops(id, 1)), this.btn("Focus 2-hop", () => this.focusHops(id, 2)));
      g.append(this.btn("Hide", () => { this.hiddenNodes.add(id); this.cy.elements().unselect(); this.applyFilters(); }), this.btn(this.pinned.has(id) ? "Unpin" : "Pin", () => { this.togglePin(id); this.renderPanel(); }));
      const op = this.btn("Open", () => this.open(id), { primary: true }); op.classList.add("gk-wide"); op.disabled = !(n.kind === "iri" && isHttp(n.iri)); g.appendChild(op);
      body.appendChild(g);
      if (!canExpand && n.kind !== "literal") body.appendChild(el("div", "gk-note", "Expanding is not available in this view."));
    } else if (nodes.length > 1) {
      title.textContent = nodes.length + " nodes selected";
      head.append(title, close); side.append(head, body);
      const g = el("div", "gk-btns");
      if (nodes.length === 2) { const p = this.btn("Shortest path", () => this.shortestPath(nodes[0].id(), nodes[1].id()), { primary: true }); p.classList.add("gk-wide"); g.appendChild(p); }
      g.append(this.btn("Hide selected", () => { nodes.forEach((x) => this.hiddenNodes.add(x.id())); this.cy.elements().unselect(); this.applyFilters(); }), this.btn("Pin selected", () => { nodes.forEach((x) => this.togglePin(x.id(), true)); this.renderPanel(); }));
      body.appendChild(g);
      body.appendChild(el("div", "gk-note", nodes.length === 2 ? "Select two nodes (shift-click to add) and find the shortest path between them." : "Shift-click adds to the selection; shift-drag draws a box."));
      const ul = el("div", "gk-col");
      for (const x of nodes.slice(0, 30)) { const b = el("button", "gk-link", trunc(x.data("lbl") || x.id(), 40)); b.type = "button"; b.addEventListener("click", () => this.selectNodes([x.id()])); ul.append(b); }
      body.appendChild(ul);
    } else {
      const e = this.model.edges.get(edges[0].data("mid"));
      title.textContent = (e && e.label) || "Edge";
      head.append(title, close); side.append(head, body);
      if (e) {
        body.appendChild(el("div", "gk-iri", e.predicate));
        const s = this.model.nodes.get(e.source), t = this.model.nodes.get(e.target);
        body.appendChild(el("h5", null, "From"));
        const a = el("button", "gk-link", trunc(this.model.displayLabel(s, this.o.labelFor), 80)); a.type = "button"; a.addEventListener("click", () => this.selectNodes([s.id]));
        body.appendChild(a);
        body.appendChild(el("h5", null, "To"));
        const b = el("button", "gk-link", trunc(this.model.displayLabel(t, this.o.labelFor), 80)); b.type = "button"; b.addEventListener("click", () => this.selectNodes([t.id]));
        body.appendChild(b);
      }
    }
    this.onResize();
  }

  open(idOrIri) {
    const n = this.model.nodes.get(idOrIri);
    const iri = n ? (n.kind === "iri" ? n.iri : "") : idOrIri;
    if (!isHttp(iri)) { this.say("Only http(s) links can be opened."); return; }
    if (typeof this.o.onOpen === "function") { try { this.o.onOpen(iri); } catch (e) { console.error(e); } }
    this.emit("open", { iri });
  }

  togglePin(id, force) {
    const n = this.cy.getElementById(id);
    const on = force != null ? force : !this.pinned.has(id);
    if (on) { this.pinned.add(id); n.lock().addClass("gk-pinned"); } else { this.pinned.delete(id); n.unlock().removeClass("gk-pinned"); }
    this.emit("pin", { id, pinned: on });
  }

  async expand(id, dir) {
    if (typeof this.o.onExpand !== "function") return null;
    this.busyOn("Expanding…", false);
    try {
      const res = await this.o.onExpand(id, dir);
      const before = this.stats();
      const added = await this.addData(res || { nodes: [], edges: [] }, { near: id });
      this.expanded.add(id);
      const dn = added.nodes ? added.nodes.length : 0, de = added.edges ? added.edges.length : 0;
      this.say(dn || de ? "+" + dn + " nodes, +" + de + " edges" : "Nothing new to add");
      this.emit("expand", { id, direction: dir, nodes: dn, edges: de, before });
      return { nodes: dn, edges: de };
    } catch (e) {
      this.say("Expand failed: " + trunc(e && e.message ? e.message : String(e), 120), 4000);
      return null;
    } finally { this.busyOff(); }
  }

  focusHops(id, k) {
    const adj = new Map();
    for (const e of this.model.edges.values()) {
      (adj.get(e.source) || adj.set(e.source, []).get(e.source)).push(e.target);
      (adj.get(e.target) || adj.set(e.target, []).get(e.target)).push(e.source);
    }
    const seen = new Set([id]); let frontier = [id];
    for (let i = 0; i < k; i++) { const nx = []; for (const u of frontier) for (const v of adj.get(u) || []) if (!seen.has(v)) { seen.add(v); nx.push(v); } frontier = nx; }
    this.setFocus(seen, id);
  }
  setFocus(set, keep) {
    this.focus = set;
    this.applyFilters(true);
    this.updateLabels();
    this.runLayout(this.st.layout).then(() => { if (keep) this.selectNodes([keep]); });
  }

  /* ---------------------------------------------------------------- path */
  shortestPath(a, b) {
    this.clearPath(true);
    const cy = this.cy;
    const adj = new Map();
    cy.edges().not(".gk-hide").forEach((e) => {
      const s = e.data("source"), t = e.data("target");
      (adj.get(s) || adj.set(s, []).get(s)).push([t, e.id()]);
      (adj.get(t) || adj.set(t, []).get(t)).push([s, e.id()]);
    });
    const prev = new Map([[a, null]]); const q = [a];
    for (let i = 0; i < q.length && !prev.has(b); i++) for (const [v, eid] of adj.get(q[i]) || []) if (!prev.has(v)) { prev.set(v, [q[i], eid]); q.push(v); }
    if (!prev.has(b)) { this.say("No path between these nodes in the visible graph.", 3200); this.emit("path", { found: false }); return null; }
    const nodes = [], edges = [];
    for (let cur = b; cur != null;) { nodes.push(cur); const p = prev.get(cur); if (p) { edges.push(p[1]); cur = p[0]; } else cur = null; }
    nodes.reverse(); edges.reverse();
    const pathEles = cy.collection();
    cy.batch(() => {
      cy.elements().addClass("gk-faded");
      for (const id of nodes) { const x = cy.getElementById(id); x.removeClass("gk-faded").addClass("gk-path"); }
      for (const id of edges) { const x = cy.getElementById(id); x.removeClass("gk-faded").addClass("gk-path"); }
    });
    this.pathActive = true; this.updateStatus();
    this.say("Shortest path: " + edges.length + " hop" + (edges.length === 1 ? "" : "s"), 3000);
    this.fitEles(cy.elements(".gk-path"), 70, true, 1.2);
    this.emit("path", { found: true, nodes, edges: edges.map((e) => this.cy.getElementById(e).data("mid")) });
    return { nodes, hops: edges.length };
  }
  clearPath(silent) {
    if (!this.cy) return;
    this.cy.elements(".gk-path, .gk-faded").removeClass("gk-path gk-faded");
    this.pathActive = false;
    if (!silent) this.updateStatus();
  }

  /* ---------------------------------------------------------------- search */
  search(q) {
    q = (q || "").trim().toLowerCase();
    this.cy.nodes(".gk-match, .gk-cur").removeClass("gk-match gk-cur");
    this.matches = []; this.matchIdx = -1;
    if (q) {
      this.N().not(".gk-hide").forEach((cn) => {
        const n = this.model.nodes.get(cn.id());
        if (n && ((cn.data("lbl") || "").toLowerCase().includes(q) || this.model.displayLabel(n, this.o.labelFor).toLowerCase().includes(q) || n.iri.toLowerCase().includes(q))) this.matches.push(cn.id());
      });
      this.cy.batch(() => this.matches.slice(0, 400).forEach((id) => this.cy.getElementById(id).addClass("gk-match")));
      if (this.matches.length) this.stepMatch(1, false);
    }
    this.searchCount.textContent = q ? (this.matches.length ? (this.matchIdx + 1) + "/" + this.matches.length : "0") : "";
    this.searchPrev.disabled = this.searchNext.disabled = this.matches.length < 2;
    return this.matches.slice();
  }
  stepMatch(d, select) {
    if (!this.matches.length) return;
    this.cy.nodes(".gk-cur").removeClass("gk-cur");
    this.matchIdx = (this.matchIdx + d + this.matches.length) % this.matches.length;
    const id = this.matches[this.matchIdx];
    const n = this.cy.getElementById(id);
    n.addClass("gk-cur");
    this.focusNode(id);
    if (select) this.selectNodes([id]);
    this.searchCount.textContent = (this.matchIdx + 1) + "/" + this.matches.length;
  }
  clearSearch(silent) {
    if (!this.cy) return;
    this.searchIn.value = ""; this.cy.nodes(".gk-match, .gk-cur").removeClass("gk-match gk-cur");
    this.matches = []; this.matchIdx = -1; this.searchCount.textContent = ""; this.searchPrev.disabled = this.searchNext.disabled = true;
  }
  focusNode(id) {
    const n = this.cy.getElementById(id);
    if (n.empty()) return;
    const z = Math.min(Math.max(this.cy.zoom(), 1.1), 2.2);
    const p = n.position(), ins = this.insets(), W = this.cyEl.clientWidth, H = this.cyEl.clientHeight;
    const pan = { x: ins.l + (W - ins.l - ins.r) / 2 - p.x * z, y: ins.t + (H - ins.t - ins.b) / 2 - p.y * z };
    this.cy.animate({ zoom: z, pan }, { duration: 260 });
  }

  /* ---------------------------------------------------------------- zoom */
  zoomBy(f, pos, instant) {
    const cy = this.cy;
    const z = Math.max(cy.minZoom(), Math.min(cy.maxZoom(), cy.zoom() * f));
    const rp = pos || { x: cy.width() / 2, y: cy.height() / 2 };
    if (instant) cy.zoom({ level: z, renderedPosition: rp });
    else cy.animate({ zoom: { level: z, renderedPosition: rp } }, { duration: 140 });
  }
  resetZoom() { this.cy.animate({ zoom: { level: 1, renderedPosition: { x: this.cy.width() / 2, y: this.cy.height() / 2 } } }, { duration: 160 }); }
  // Space taken by the overlays: the details panel on the right, the legend / minimap band at the bottom,
  // the zoom buttons on top. Fitting pads by these so nodes never end up under them.
  insets() {
    const ins = { l: 0, r: 0, t: 44, b: 0 };
    const W = this.cyEl.clientWidth || 1, H = this.cyEl.clientHeight || 1;
    if (!this.side.hidden) ins.r = this.side.offsetWidth + 16;
    if (!this.miniBox.hidden) ins.b = this.miniBox.offsetHeight + 8;
    const lw = this.legends.offsetWidth, lh = this.legends.offsetHeight;
    if (lh) { // the legend sits bottom-left: reserve whichever side costs less view area
      if ((lw + 16) / W < (lh + 8) / H) ins.l = lw + 16; else ins.b = Math.max(ins.b, lh + 8);
    }
    ins.b = Math.min(ins.b, H * 0.28); ins.l = Math.min(ins.l, W * 0.25);
    return ins;
  }
  fitEles(eles, pad = 30, animate = true, maxZoom = 1.6) {
    const cy = this.cy;
    if (!eles.length) return;
    const bb = eles.boundingBox();
    const W = this.cyEl.clientWidth, H = this.cyEl.clientHeight;
    const ins = this.insets();
    const aw = Math.max(W - ins.l - ins.r - 2 * pad, 80), ah = Math.max(H - ins.t - ins.b - 2 * pad, 80);
    const z = Math.max(cy.minZoom(), Math.min(maxZoom, aw / Math.max(bb.w, 1), ah / Math.max(bb.h, 1)));
    const pan = { x: ins.l + pad + aw / 2 - (bb.x1 + bb.w / 2) * z, y: ins.t + pad + ah / 2 - (bb.y1 + bb.h / 2) * z };
    if (animate) cy.animate({ zoom: z, pan }, { duration: 280 });
    else { cy.zoom(z); cy.pan(pan); }
  }
  fit(animate = true) {
    const eles = this.cy.elements().not(".gk-hide");
    if (!eles.length || this.chordMode) return;
    this.fitEles(eles, 30, animate && this.N().length <= 300);
  }
  resize() { this.onResize(); }
  toggleFullscreen() {
    if (this.isFs()) document.exitFullscreen && document.exitFullscreen();
    else if (this.root.requestFullscreen) this.root.requestFullscreen().catch(() => this.say("Fullscreen is not available here."));
    else this.say("Fullscreen is not available here.");
  }

  /* ---------------------------------------------------------------- layouts */
  busyOn(text, stoppable) { this.busyText.textContent = text; this.busy.querySelector(".gk-stop").hidden = !stoppable; this.busy.hidden = false; }
  busyOff() { this.busy.hidden = true; }
  stopLayout() { this._stopped = true; if (this._workerReject) this._workerReject("stopped"); try { this._layoutObj && this._layoutObj.stop(); } catch (e) { /* done already */ } if (this._stopRes) this._stopRes(); }

  clearCurves() {
    const c = this.cy.edges(".gk-curved");
    if (c.length) this.cy.batch(() => { c.removeStyle().removeClass("gk-curved"); });
  }
  setDeco(deco) {
    this.deco = deco;
    const th = this.th();
    drawDeco(this.world, deco, { color: (s) => this.slotColor(s), text: th.text, muted: th.muted, fontSans: this.tokens.fontSans, border: th.border });
    this.syncDeco();
  }
  syncDeco() {
    const p = this.cy.pan(), z = this.cy.zoom();
    this.world.setAttribute("transform", "translate(" + p.x + " " + p.y + ") scale(" + z + ")");
  }

  syncBoxes(name) {
    const cy = this.cy;
    const old = cy.nodes(".gk-box");
    if (old.length) { cy.batch(() => { old.children().move({ parent: null }); old.remove(); }); }
    this.cy.nodes(".gk-ring").removeClass("gk-ring");
    if (!this.st.boxes || (name !== "fcose" && name !== "dagre") || this.groups.length < 2) return;
    cy.batch(() => {
      const bySlot = new Map();
      this.N().not(".gk-hide").forEach((n) => { const s = n.data("slot"); (bySlot.get(s) || bySlot.set(s, []).get(s)).push(n); });
      for (const [slot, ns] of bySlot) {
        const g = this.groups.find((x) => x.slot === slot);
        const id = "box|" + slot;
        cy.add({ group: "nodes", data: { id, lbl: g ? g.name : "Other", col: this.slotColor(slot), slot, kind: "box" }, classes: "gk-box" });
        ns.forEach((n) => n.move({ parent: id }));
      }
    });
  }

  axesFor(nodes) {
    const keyOf = (n) => { const slot = n.data("slot"); const g = this.groups.find((x) => x.slot === slot); return { key: slot, name: g ? g.name : "Other", slot }; };
    let axes = this.groups.length >= 3 ? makeAxes(nodes, keyOf) : null;
    if (!axes) {
      const comm = this.model.communities();
      axes = makeAxes(nodes, (n) => { const name = comm.get(n.id()); return { key: name, name, slot: 9 }; });
      axes.forEach((a, i) => { a.slot = i < OTHER ? i : OTHER; });
      this._axisNote = "communities";
    } else this._axisNote = "";
    return axes;
  }

  // Spanning forest (BFS from the best-connected nodes) plus all nodes: dagre's ordering is far too slow on thousands of edges.
  forestEles(eles) {
    const nodes = eles.nodes().not(".gk-box");
    const adj = new Map();
    eles.edges().forEach((e) => {
      const s = e.data("source"), t = e.data("target");
      if (s === t) return;
      (adj.get(s) || adj.set(s, []).get(s)).push([t, e]);
      (adj.get(t) || adj.set(t, []).get(t)).push([s, e]);
    });
    const seen = new Set(), keep = [];
    const order = nodes.toArray().sort((a, b) => b.data("deg") - a.data("deg"));
    for (const r of order) {
      if (seen.has(r.id())) continue;
      seen.add(r.id());
      const q = [r.id()];
      for (let i = 0; i < q.length; i++) for (const [v, e] of adj.get(q[i]) || []) if (!seen.has(v)) { seen.add(v); keep.push(e); q.push(v); }
    }
    return eles.nodes().union(this.cy.collection(keep));
  }

  // fCoSE in "draft" quality (used above 700 nodes) packs nodes tightly. Spread the layout so the nodes
  // take about 2.2 times their own area, then push overlapping pairs apart on a spatial hash.
  declutter(nodes) {
    const arr = nodes.toArray().filter((n) => !n.locked());
    if (arr.length < 2) return;
    const P = arr.map((n) => { const p = n.position(); return { x: p.x, y: p.y, r: (n.data("bsize") || 20) / 2 + 3 }; });
    let x1 = Infinity, y1 = Infinity, x2 = -Infinity, y2 = -Infinity, area = 0, maxR = 0;
    for (const p of P) { x1 = Math.min(x1, p.x); y1 = Math.min(y1, p.y); x2 = Math.max(x2, p.x); y2 = Math.max(y2, p.y); area += Math.PI * p.r * p.r; maxR = Math.max(maxR, p.r); }
    const cur = Math.max((x2 - x1) * (y2 - y1), 1), want = area * 2.2;
    const s = cur < want ? Math.sqrt(want / cur) : 1;
    const cx = (x1 + x2) / 2, cy0 = (y1 + y2) / 2;
    for (const p of P) { p.x = cx + (p.x - cx) * s; p.y = cy0 + (p.y - cy0) * s; }
    const cell = maxR * 2.2;
    for (let it = 0; it < 40; it++) {
      const grid = new Map();
      P.forEach((p, i) => { const k = Math.floor(p.x / cell) * 73856093 ^ Math.floor(p.y / cell) * 19349663; (grid.get(k) || grid.set(k, []).get(k)).push(i); });
      let moved = 0;
      P.forEach((p, i) => {
        const gx = Math.floor(p.x / cell), gy = Math.floor(p.y / cell);
        for (let dx = -1; dx <= 1; dx++) for (let dy = -1; dy <= 1; dy++) {
          const l = grid.get((gx + dx) * 73856093 ^ (gy + dy) * 19349663);
          if (!l) continue;
          for (const j of l) {
            if (j <= i) continue;
            const q = P[j]; let ddx = q.x - p.x, ddy = q.y - p.y, d = Math.hypot(ddx, ddy);
            const min = p.r + q.r;
            if (d >= min) continue;
            if (d < 1e-6) { ddx = Math.cos(i); ddy = Math.sin(i); d = 1; }
            const push = (min - d) / 2 + 0.2;
            const ux = ddx / d, uy = ddy / d;
            p.x -= ux * push; p.y -= uy * push; q.x += ux * push; q.y += uy * push; moved++;
          }
        }
      });
      if (!moved) break;
    }
    this.cy.batch(() => arr.forEach((n, i) => n.position({ x: P[i].x, y: P[i].y })));
  }

  // Radial by degree: rings by log2(degree), highest degree in the middle; inside a ring nodes are
  // ordered by group so each group forms an arc. Ring radius grows with the ring's own node sizes.
  ringPositions(nodes) {
    const buckets = new Map();
    nodes.forEach((n) => { const b = Math.floor(Math.log2(n.data("deg") + 1)); (buckets.get(b) || buckets.set(b, []).get(b)).push(n); });
    const levels = [...buckets.keys()].sort((a, b) => b - a);
    const pos = new Map();
    let prevR = 0, prevMax = 0, first = true;
    for (const b of levels) {
      const list = buckets.get(b).sort((p, q) => p.data("slot") - q.data("slot") || q.data("deg") - p.data("deg"));
      const sizes = list.map((n) => n.data("bsize") || 20);
      const maxS = Math.max(...sizes), sum = sizes.reduce((a, c) => a + c + 5, 0);
      let r = sum / (2 * Math.PI);
      if (first) r = list.length === 1 ? 0 : Math.max(r, maxS);
      else r = Math.max(r, prevR + prevMax / 2 + maxS / 2 + 16);
      const off = (levels.indexOf(b) % 2) * (Math.PI / list.length);
      list.forEach((n, i) => { const th = off + (i / list.length) * 2 * Math.PI; pos.set(n.id(), { x: r * Math.sin(th), y: -r * Math.cos(th) }); });
      prevR = r; prevMax = maxS; first = false;
    }
    return pos;
  }

  /* ---- layout worker: a Blob worker from the bundled worker source, or o.workerUrl (an extension-origin
     file, which is the way to go on pages whose CSP forbids blob workers). Any failure -> synchronous layout. */
  workerUrl() {
    if (this.o.workerUrl) return this.o.workerUrl;
    if (!Kit._blobUrl) Kit._blobUrl = URL.createObjectURL(new Blob([workerSrc], { type: "text/javascript" }));
    return Kit._blobUrl;
  }
  runInWorker(name, eles, params) {
    return new Promise((resolve, reject) => {
      let w;
      const fail = (code, msg) => { try { w && w.terminate(); } catch (e) { /* gone */ } this._worker = null; this._workerReject = null; reject(Object.assign(new Error(msg), { code })); };
      try { w = new Worker(this.workerUrl()); } catch (e) { return fail("blocked", e && e.message); }
      this._worker = w;
      this._workerReject = (code) => fail(code, code);
      w.onmessage = (ev) => {
        const d = ev.data;
        try { w.terminate(); } catch (e) { /* gone */ }
        this._worker = null; this._workerReject = null;
        if (d.error) reject(Object.assign(new Error(d.error), { code: "failed" })); else resolve(d.positions);
      };
      w.onerror = (e) => fail("blocked", (e && e.message) || "worker error");
      const nodes = eles.nodes().map((n) => {
        const par = n.parent();
        const o = { id: n.id(), pos: n.position() };
        if (!n.hasClass("gk-box")) { o.w = n.data("size") || 20; o.h = o.w; }
        if (par.nonempty()) o.parent = par.id();
        return o;
      });
      const edges = eles.edges().map((e) => ({ id: e.id(), s: e.data("source"), t: e.data("target") }));
      try { w.postMessage({ name, nodes, edges, params }); } catch (e) { fail("failed", e.message); }
    });
  }

  // Dagre puts every node of a rank on one line, so a tree with a 150-node rank is thousands of units wide.
  // When the widest rank has more than 60 nodes, fold the finished layout into 2-5 horizontal bands stacked
  // under each other (like folding a banner), choosing the fold count that gives the largest zoom for the
  // current view. Structure inside a band is untouched; only edges crossing a fold get long.
  wrapRanks(nodes) {
    const byY = new Map();
    nodes.forEach((n) => { const y = Math.round(n.position("y")); byY.set(y, (byY.get(y) || 0) + 1); });
    if (Math.max(...byY.values()) <= 60) return false;
    const bb = nodes.boundingBox();
    const W = Math.max(bb.w, 1), H = Math.max(bb.h, 1);
    const ins = this.insets();
    const vw = Math.max(this.cyEl.clientWidth - ins.l - ins.r - 60, 300), vh = Math.max(this.cyEl.clientHeight - ins.t - ins.b - 60, 200);
    const gap = 90;
    let best = 1, bestZ = Math.min(vw / W, vh / H);
    for (let s = 2; s <= 5; s++) {
      const z = Math.min(vw / (W / s + 60), vh / (H * s + gap * (s - 1)));
      if (z > bestZ * 1.25) { best = s; bestZ = z; }
    }
    if (best === 1) return false;
    const bw = W / best;
    this.cy.batch(() => nodes.forEach((n) => {
      const p = n.position();
      const i = Math.min(best - 1, Math.floor((p.x - bb.x1) / bw));
      n.position({ x: p.x - i * bw, y: p.y + i * (H + gap) });
    }));
    this._folds = best;
    return true;
  }

  // Radial tree: BFS tree from `root`, other components hang under the root at depth 1. A node's angular
  // span is proportional to its leaf count; each ring is at least wide enough for its own node count.
  radialTree(nodes, eles, root) {
    const adj = new Map();
    nodes.forEach((n) => adj.set(n.id(), []));
    eles.edges().forEach((e) => { const s = e.data("source"), t = e.data("target"); if (s !== t && adj.has(s) && adj.has(t)) { adj.get(s).push(t); adj.get(t).push(s); } });
    const kids = new Map(), depth = new Map(), order = [];
    const rid = root.id();
    const bfs = (start, d0, parent) => {
      const q = [start];
      depth.set(start, d0); kids.set(start, kids.get(start) || []);
      if (parent) kids.get(parent).push(start);
      for (let i = 0; i < q.length; i++) { const u = q[i]; order.push(u); for (const v of adj.get(u)) if (!depth.has(v)) { depth.set(v, depth.get(u) + 1); kids.set(v, []); kids.get(u).push(v); q.push(v); } }
    };
    bfs(rid, 0, null);
    const rest = nodes.toArray().sort((a, b) => b.data("deg") - a.data("deg"));
    for (const n of rest) if (!depth.has(n.id())) bfs(n.id(), 1, rid);
    const leaves = new Map();
    for (let i = order.length - 1; i >= 0; i--) { const u = order[i], k = kids.get(u); leaves.set(u, k.length ? k.reduce((a, c) => a + leaves.get(c), 0) : 1); }
    const span = new Map([[rid, [0, 2 * Math.PI]]]);
    for (const u of order) {
      const [a0, a1] = span.get(u); let a = a0;
      for (const c of kids.get(u)) { const w = ((a1 - a0) * leaves.get(c)) / leaves.get(u); span.set(c, [a, a + w]); a += w; }
    }
    const byDepth = new Map();
    for (const u of order) { const d = depth.get(u); (byDepth.get(d) || byDepth.set(d, []).get(d)).push(u); }
    const size = new Map(); nodes.forEach((n) => size.set(n.id(), n.data("bsize") || 20));
    const radius = [0]; let prevMax = size.get(rid);
    const ds = [...byDepth.keys()].sort((a, b) => a - b);
    for (const d of ds) {
      if (d === 0) continue;
      const list = byDepth.get(d), mx = Math.max(...list.map((u) => size.get(u))), sum = list.reduce((a, u) => a + size.get(u) + 7, 0);
      radius[d] = Math.max(radius[d - 1] + prevMax / 2 + mx / 2 + 34, sum / (2 * Math.PI));
      // a parent's children must fit in the parent's own angular span
      let need = 0;
      for (const u of byDepth.get(d - 1)) { const k = kids.get(u); if (!k.length) continue; const sp = span.get(u)[1] - span.get(u)[0]; need = Math.max(need, k.reduce((a, c) => a + size.get(c) + 7, 0) / Math.max(sp, 1e-3)); }
      radius[d] = Math.max(radius[d], need);
      prevMax = mx;
    }
    const pos = new Map();
    for (const u of order) { const d = depth.get(u), [a0, a1] = span.get(u), th = (a0 + a1) / 2, r = radius[d]; pos.set(u, { x: r * Math.sin(th), y: -r * Math.cos(th) }); }
    return pos;
  }

  async runLayout(name, o = {}) {
    name = name || this.st.layout;
    if (this.st.layout !== name) { this.st.layout = name; }
    this.layoutSel.value = name;
    this.dirSeg.hidden = name !== "dagre";
    this.emit("layout", name);
    if (this._layoutObj) { try { this._layoutObj.stop(); } catch (e) { /* ok */ } }
    const token = ++this._lt;
    const cy = this.cy;
    if (name === "chord") { this.enterChord(); return 0; }
    this.leaveChord();
    this.clearCurves(); this.setDeco(null);
    cy.nodes(".gk-ring").removeClass("gk-ring");
    this.updateLod(true, true);
    this.syncBoxes(name);
    const eles = cy.elements().not(".gk-hide");
    const nodes = eles.nodes().not(".gk-box");
    const n = nodes.length;
    if (!n) { this.updateStatus(); return 0; }
    const animate = n <= 300 && !o.noAnimate && !(name === "dagre" && this.st.wrap && n > 120);
    this._stopped = false;
    const big = n > 300;
    this.busyOn(big ? "Layout running…" : "Layout…", true);
    if (big) await frames(2); else await frames(1);
    if (token !== this._lt) return 0;
    const t0 = performance.now();
    let opts, pre = null, lEles = null, wp = null;
    const dur = 300;
    const pinned = [...this.pinned].filter((id) => cy.getElementById(id).nonempty());
    switch (name) {
      case "fcose":
        wp = { quality: n > 700 ? "draft" : "default", randomize: !o.incremental, padding: 30, packComponents: true, nodeRepulsion: 7000, idealEdgeLength: 75, edgeElasticity: 0.45, nodeSeparation: 90, gravity: 0.3, tile: true, numIter: n > 1000 ? 1200 : 2500 };
        opts = Object.assign({ name: "fcose", animate, animationDuration: dur, fit: false, uniformNodeDimensions: false }, wp, { nodeRepulsion: () => wp.nodeRepulsion, idealEdgeLength: () => wp.idealEdgeLength, edgeElasticity: () => wp.edgeElasticity });
        if (this.layoutOpts && this.layoutOpts.fcose) Object.assign(opts, this.layoutOpts.fcose);
        if (pinned.length) { opts.fixedNodeConstraint = pinned.map((id) => ({ nodeId: id, position: cy.getElementById(id).position() })); wp.fixed = opts.fixedNodeConstraint; }
        break;
      case "dagre":
        if (eles.edges().length > 2000) { lEles = this.forestEles(eles); this.say("Hierarchical layout of " + fmt(eles.edges().length) + " edges: laying out a spanning forest to stay fast.", 4000); }
        wp = { rankDir: this.st.dir, nodeSep: 22, rankSep: 70, edgeSep: 10, ranker: n > 1500 || eles.edges().length > 2500 ? "longest-path" : n > 800 ? "tight-tree" : "network-simplex", padding: 30 };
        opts = Object.assign({ name: "dagre", animate, animationDuration: dur, fit: false, nodeDimensionsIncludeLabels: n < 400 }, wp);
        break;
      case "concentric": {
        const pos = this.ringPositions(nodes);
        opts = { name: "preset", positions: (x) => pos.get(x.id()) || x.position(), animate, animationDuration: dur, fit: false };
        break;
      }
      case "circle":
        opts = { name: "circle", avoidOverlap: true, sort: (a, b) => a.data("slot") - b.data("slot") || b.data("deg") - a.data("deg"), animate, animationDuration: dur, fit: false, spacingFactor: n > 600 ? 0.9 : 1 };
        break;
      case "grid":
        opts = { name: "grid", avoidOverlap: true, sort: (a, b) => a.data("slot") - b.data("slot") || b.data("deg") - a.data("deg"), animate, animationDuration: dur, fit: false, spacingFactor: 1.1, condense: false };
        break;
      case "tree": {
        let root = cy.nodes(":selected").not(".gk-box")[0];
        if (!root || root.hasClass("gk-hide")) root = nodes.toArray().sort((a, b) => b.data("deg") - a.data("deg"))[0];
        if (n > 150) {
          const pos = this.radialTree(nodes, eles, root);
          opts = { name: "preset", positions: (x) => pos.get(x.id()) || x.position(), animate, animationDuration: dur, fit: false };
        } else opts = { name: "breadthfirst", roots: cy.collection([root]), directed: false, spacingFactor: 1.05, avoidOverlap: true, animate, animationDuration: dur, fit: false, padding: 30 };
        break;
      }
      case "hive":
      case "circos": {
        const axes = this.axesFor(nodes.toArray());
        const ed = eles.edges().map((e) => ({ id: e.id(), source: e.data("source"), target: e.data("target") }));
        const nd = axes.map((a) => ({ ...a, nodes: a.nodes.map((x) => ({ id: x.id(), deg: x.data("deg") })) }));
        pre = name === "hive" ? hive(nd, ed) : circos(nd, ed);
        opts = { name: "preset", positions: (x) => pre.pos.get(x.id()) || x.position(), animate, animationDuration: dur, fit: false };
        break;
      }
      default: opts = { name: "grid" };
    }
    if (pre) {
      cy.batch(() => {
        const sgn = this.curveSign;
        eles.edges().forEach((e) => {
          const c = pre.curves.get(e.id());
          if (!c) return;
          e.style({ "curve-style": "unbundled-bezier", "control-point-distances": [c.d * sgn], "control-point-weights": [c.w], "edge-distances": "node-position" });
          e.addClass("gk-curved");
        });
        if (name === "circos") nodes.addClass("gk-ring");
      });
      this.setDeco(pre.deco);
    }
    const target = lEles || eles;
    let viaWorker = false, wStopped = false;
    const useWorker = (name === "fcose" || name === "dagre") && wp && !o.incremental && n >= 120 && this.o.worker !== false && this.workerOk !== false && typeof Worker !== "undefined";
    if (useWorker) {
      try {
        const pos = await this.runInWorker(name, target, wp);
        if (token !== this._lt) return 0;
        const pl = target.layout({ name: "preset", positions: (x) => pos[x.id()] || x.position(), animate: false, fit: false });
        this._layoutObj = pl;
        const d2 = new Promise((res) => pl.one("layoutstop", res));
        pl.run(); await d2;
        viaWorker = true;
        this.lastLayoutVia = "worker";
      } catch (e) {
        if (e && e.code === "stopped") wStopped = true;
        else { this.workerOk = false; console.debug("[graphkit] layout worker unavailable (" + (e && e.message) + "); using the synchronous layout"); }
      }
    }
    if (wStopped) this._stopped = true;
    if (!viaWorker && !wStopped) {
      this.lastLayoutVia = "sync";
      const layout = target.layout(opts);
      this._layoutObj = layout;
      const done = new Promise((res) => layout.one("layoutstop", res));
      const stopped = new Promise((res) => { this._stopRes = res; });
      try { layout.run(); } catch (e) { console.error("layout failed", e); this.busyOff(); this.say("Layout failed: " + trunc(e.message, 80)); return 0; }
      await Promise.race([done, stopped]);
    }
    if (token !== this._lt) return 0;
    this._layoutObj = null; this._stopRes = null;
    if (name === "dagre" && this.st.wrap && !this._stopped && this.st.dir !== "LR") { if (this.wrapRanks(nodes)) this.say("Wide hierarchy folded into " + this._folds + " bands (View > Wrap wide hierarchies)", 3500); }
    if (name === "fcose" && n > 700 && !this._stopped) this.declutter(nodes);
    const ms = performance.now() - t0;
    this.busyOff();
    this.lastLayoutMs = ms;
    this.updateLod(true);
    if (!o.noFit && !o.incremental) this.fit(animate);
    this.updateLabels();
    this.mini && this.mini.schedule();
    this.syncDeco();
    if (this._stopped) this.say("Layout stopped.");
    this.emit("layoutstop", { name, ms });
    return ms;
  }

  /* ---------------------------------------------------------------- chord view */
  enterChord() {
    this.chordMode = true;
    this.stage.classList.add("gk-chordmode");
    this.chordEl.hidden = false;
    this.busyOff(); this.hideTip();
    this.drawChord();
  }
  leaveChord() {
    if (!this.chordMode) return;
    this.chordMode = false;
    this.stage.classList.remove("gk-chordmode");
    this.chordEl.hidden = true;
    this.cy.resize();
  }
  drawChord() {
    const groups = this.groups.filter((g) => !this.hiddenSlots.has(g.slot));
    const idx = new Map(groups.map((g, i) => [g.slot, i]));
    const m = groups.map(() => groups.map(() => 0));
    this.cy.edges().not(".gk-hide").forEach((e) => {
      const a = idx.get(e.source().data("slot")), b = idx.get(e.target().data("slot"));
      if (a != null && b != null) m[a][b]++;
    });
    const th = this.th();
    const one = groups.length < 2 && this.groups.length < 2;
    renderChord(this.chordEl, groups.map((g) => ({ name: g.name, color: one ? normColor(this.pal()[0]) : this.slotColor(g.slot), count: g.count })), m, { surface: th.surface, text: th.text, muted: th.muted, font: this.tokens.fontSans }, this.chordTip());
  }

  /* ---------------------------------------------------------------- theme and options */
  setTheme(t = {}) {
    if (t.dark != null) this.dark = !!t.dark;
    this.tokens = Object.assign({}, this.dark ? TOKENS_DARK : TOKENS_LIGHT, t.tokens || {});
    this.setVars();
    this.cy.style(this.styleSheet());
    this.applyEncoding();
    this.renderLegend();
    if (this.deco) this.setDeco(this.deco);
    if (this.chordMode) this.drawChord();
    this.mini.schedule();
  }

  set(p) {
    const st = this.st;
    let relayout = false, enc = false, lbl = false;
    for (const k of Object.keys(p)) {
      if (!(k in st) && k !== "layout") continue;
      if (k === "groupBy") { this.setGroupBy(p[k]); continue; }
      if (k === "layout") { this.runLayout(p[k]); continue; }
      st[k] = p[k];
      if (k === "colorBy" || k === "sizeBy" || k === "edgeColor") enc = true;
      if (k === "labels") lbl = true;
      if (k === "boxes" || k === "dir" || k === "wrap") relayout = true;
      if (k === "minDeg" || k === "hideLiterals" || k === "hideBnodes") this.applyFilters();
    }
    if (enc) this.applyEncoding();
    if (lbl) this.updateLabels();
    if (relayout && (st.layout === "fcose" || st.layout === "dagre")) this.runLayout();
    this.refreshUi();
    this.emit("options", Object.assign({}, st));
  }
  getState() { return Object.assign({}, this.st, { hiddenSlots: [...this.hiddenSlots], hiddenPreds: [...this.hiddenPreds], zoom: this.cy.zoom() }); }

  /* ---------------------------------------------------------------- UI refresh */
  refreshUi() {
    this.layoutSel.value = this.st.layout;
    this.dirSeg.hidden = this.st.layout !== "dagre";
    this.groupSel.value = this.st.groupBy;
    this.miniBox.hidden = !this.st.minimap;
    if (this.st.minimap) this.mini.schedule();
    this.renderLegend();
    this.updateStatus();
    this.searchPrev.disabled = this.searchNext.disabled = this.matches.length < 2;
    if (this.popName) this.buildPop(this.popName);
  }

  glyph(slot, color, hollow) {
    const shape = this.groups.length < 2 ? "ellipse" : SHAPES[Math.min(slot, OTHER)];
    const s = svg("svg", { width: 15, height: 15, viewBox: "0 0 10 10", "aria-hidden": "true" });
    s.appendChild(svg("path", { d: GLYPHS[shape], fill: hollow ? "none" : color, stroke: hollow ? color : this.th().surface, "stroke-width": hollow ? 1.3 : 0.6 }));
    return s;
  }
  lineGlyph(slot, color) {
    const s = svg("svg", { width: 22, height: 10, viewBox: "0 0 22 10", "aria-hidden": "true" });
    const d = DASHES[Math.min(slot, OTHER)];
    s.appendChild(svg("line", { x1: 1, y1: 5, x2: 21, y2: 5, stroke: color, "stroke-width": 2.2, "stroke-dasharray": d === "dashed" ? "5 3" : d === "dotted" ? "1.5 3" : "none", "stroke-linecap": "round" }));
    return s;
  }

  renderLegend() {
    const L = this.legends; clear(L);
    const th = this.th();
    const mk = (title, collapsedKey) => {
      const box = el("div", "gk-legend");
      const h = el("h4"); h.appendChild(el("span", null, title));
      const t = el("button", null, this[collapsedKey] ? "▸" : "▾"); t.type = "button"; t.setAttribute("aria-label", "Collapse or expand legend"); t.setAttribute("aria-expanded", String(!this[collapsedKey]));
      t.addEventListener("click", () => { this[collapsedKey] = !this[collapsedKey]; this.renderLegend(); });
      h.appendChild(t); box.appendChild(h);
      return box;
    };
    if (this.groups.length >= 2 || this.st.colorBy === "degree") {
      const box = mk(this.st.colorBy === "degree" && this.groups.length < 2 ? "Degree" : "Groups by " + GROUP_WORD[this.st.groupBy], "_lgCollapsed");
      if (!this._lgCollapsed) {
        if (this.st.colorBy === "degree") {
          const r = el("div", "gk-ramp"); SEQ.forEach((c) => { const i = el("i"); i.style.background = c; r.appendChild(i); });
          r.setAttribute("role", "img"); r.setAttribute("aria-label", "Colour by degree, light to dark");
          const lab = el("div", "gk-ramplab"); lab.append(el("span", null, "0"), el("span", null, "degree"), el("span", null, String(this.maxDeg || 0)));
          box.append(r, lab);
        }
        if (this.groups.length >= 2) {
          const ul = el("ul", "gk-scroll"); ul.setAttribute("role", "list"); ul.style.maxHeight = "260px";
          for (const g of this.groups) {
            const li = el("li"); const b = el("button"); b.type = "button";
            const on = !this.hiddenSlots.has(g.slot);
            b.setAttribute("aria-pressed", String(on));
            b.title = (on ? "Hide " : "Show ") + g.name + " (" + g.count + " nodes)";
            const col = this.st.colorBy === "group" ? this.slotColor(g.slot) : th.muted;
            b.append(this.glyph(g.slot, col, !on), el("span", "gk-nm", g.name), el("span", "gk-n", fmt(g.count)));
            b.addEventListener("click", () => { if (on) this.hiddenSlots.add(g.slot); else this.hiddenSlots.delete(g.slot); this.clearHl(); this.applyFilters(); this.renderLegend(); });
            b.addEventListener("mouseenter", () => this.hlSlot(g.slot)); b.addEventListener("mouseleave", () => this.clearHl());
            b.addEventListener("focus", () => this.hlSlot(g.slot)); b.addEventListener("blur", () => this.clearHl());
            li.appendChild(b); ul.appendChild(li);
          }
          box.appendChild(ul);
        }
      }
      L.appendChild(box);
    }
    if (this.st.edgeColor && this.preds.length) {
      const box = mk("Edges by predicate", "_pgCollapsed");
      if (!this._pgCollapsed) {
        const ul = el("ul", "gk-scroll"); ul.setAttribute("role", "list"); ul.style.maxHeight = "190px";
        const shown = new Map();
        for (const p of this.preds) { const s = p.slot; const e = shown.get(s) || { slot: s, name: s === OTHER ? "Other predicates" : p.name, count: 0, preds: [] }; e.count += p.count; e.preds.push(p.pred); shown.set(s, e); }
        for (const e of [...shown.values()].sort((a, b) => a.slot - b.slot)) {
          const li = el("li"); const b = el("button"); b.type = "button";
          const on = e.preds.some((p) => !this.hiddenPreds.has(p));
          b.setAttribute("aria-pressed", String(on)); b.title = (on ? "Hide " : "Show ") + e.name;
          b.append(this.lineGlyph(e.slot, this.slotColor(e.slot)), el("span", "gk-nm", e.name), el("span", "gk-n", fmt(e.count)));
          b.addEventListener("click", () => { for (const p of e.preds) { if (on) this.hiddenPreds.add(p); else this.hiddenPreds.delete(p); } this.applyFilters(); this.renderLegend(); });
          li.appendChild(b); ul.appendChild(li);
        }
        box.appendChild(ul);
      }
      L.appendChild(box);
    }
  }

  /* ---------------------------------------------------------------- popovers */
  togglePop(name) {
    if (this.popName === name) { this.closePop(); return; }
    this.buildPop(name);
  }
  closePop() {
    if (this.pop) { this.pop.remove(); this.pop = null; }
    this.popName = null;
    [this.viewBtn, this.filtBtn, this.expBtn].forEach((b) => b.setAttribute("aria-expanded", "false"));
  }
  buildPop(name) {
    if (this.pop) this.pop.remove();
    const pop = el("div", this.cls("panel", "gk-v-panel") + " gk-pop gk-scroll");
    pop.setAttribute("role", "dialog");
    this.pop = pop; this.popName = name;
    const btn = name === "view" ? this.viewBtn : name === "filters" ? this.filtBtn : this.expBtn;
    [this.viewBtn, this.filtBtn, this.expBtn].forEach((b) => b.setAttribute("aria-expanded", String(b === btn)));
    if (name === "view") {
      const g = el("div", "gk-grid");
      g.append(el("span", null, "Colour"), this.select(COLORBY, this.st.colorBy, (v) => this.set({ colorBy: v }), "Colour nodes by"));
      g.append(el("span", null, "Size"), this.select(SIZEBY, this.st.sizeBy, (v) => this.set({ sizeBy: v }), "Size nodes by"));
      g.append(el("span", null, "Labels"), this.segmented([["auto", "Auto"], ["all", "All"], ["none", "None"]], this.st.labels, (v) => this.set({ labels: v }), "Labels"));
      pop.appendChild(g);
      pop.append(
        this.checkbox("Colour edges by predicate", this.st.edgeColor, (v) => this.set({ edgeColor: v })),
        this.checkbox("Show groups as boxes (Force, Hierarchical)", this.st.boxes, (v) => this.set({ boxes: v })),
        this.checkbox("Wrap wide hierarchies", this.st.wrap, (v) => this.set({ wrap: v })),
        this.checkbox("Minimap", this.st.minimap, (v) => this.set({ minimap: v })),
      );
    } else if (name === "filters") {
      const maxD = Math.max(1, this.maxDeg || 1);
      const lab = el("div", "gk-sec", "Minimum degree: " + this.st.minDeg);
      const r = el("input"); r.type = "range"; r.min = "0"; r.max = String(Math.min(maxD, 60)); r.value = String(this.st.minDeg); r.setAttribute("aria-label", "Minimum degree");
      r.addEventListener("input", () => { lab.textContent = "Minimum degree: " + r.value; this.st.minDeg = +r.value; this.applyFilters(); });
      pop.append(lab, r,
        this.checkbox("Hide literals", this.st.hideLiterals, (v) => this.set({ hideLiterals: v })),
        this.checkbox("Hide blank nodes", this.st.hideBnodes, (v) => this.set({ hideBnodes: v })));
      pop.appendChild(el("div", "gk-sec", "Predicates"));
      const row = el("div", "gk-row"); row.style.display = "flex"; row.style.gap = "4px";
      row.append(this.btn("All", () => { this.hiddenPreds.clear(); this.applyFilters(); this.buildPop("filters"); this.renderLegend(); }), this.btn("None", () => { this.preds.forEach((p) => this.hiddenPreds.add(p.pred)); this.applyFilters(); this.buildPop("filters"); this.renderLegend(); }));
      pop.appendChild(row);
      const list = el("div", "gk-list gk-scroll");
      for (const p of this.preds.slice(0, 200)) {
        const l = el("label"); const i = el("input"); i.type = "checkbox"; i.checked = !this.hiddenPreds.has(p.pred);
        i.addEventListener("change", () => { if (i.checked) this.hiddenPreds.delete(p.pred); else this.hiddenPreds.add(p.pred); this.applyFilters(); this.renderLegend(); });
        l.title = p.pred;
        l.append(i);
        if (this.st.edgeColor) l.append(this.lineGlyph(p.slot, this.slotColor(p.slot)));
        l.append(el("span", "gk-nm", p.name), el("span", "gk-n", fmt(p.count)));
        list.appendChild(l);
      }
      pop.appendChild(list);
      const clr = this.btn("Clear filters", () => { this.st.minDeg = 0; this.st.hideLiterals = this.st.hideBnodes = false; this.hiddenSlots.clear(); this.hiddenPreds.clear(); this.hiddenNodes.clear(); this.focus = null; this.applyFilters(); this.refreshUi(); });
      pop.appendChild(clr);
    } else {
      pop.classList.add("gk-menu");
      const items = [["png", "PNG image", "what you see"], ["png-full", "PNG, whole graph", "all visible nodes"], ["svg", "SVG", "vector, whole graph"], ["json", "JSON", "visible nodes and edges"], ["nt", "N-Triples", "visible edges"], ["copy", "Copy image", "to the clipboard"]];
      for (const [k, l, s] of items) {
        const b = el("button", null, l); b.type = "button"; b.appendChild(el("small", null, s));
        if (k === "copy" && !(navigator.clipboard && window.ClipboardItem)) { b.disabled = true; b.style.opacity = ".5"; }
        b.addEventListener("click", () => { this.closePop(); this.doExport(k); });
        pop.appendChild(b);
      }
    }
    this.root.appendChild(pop);
    // place under the button, inside the root
    const rr = this.root.getBoundingClientRect(), br = btn.getBoundingClientRect();
    let left = br.left - rr.left;
    left = Math.max(6, Math.min(left, rr.width - pop.offsetWidth - 6));
    pop.style.left = left + "px"; pop.style.top = (br.bottom - rr.top + 4) + "px";
  }

  /* ---------------------------------------------------------------- export */
  visibleModel() {
    const nodes = [], edges = [];
    const hid = new Set();
    this.cy.nodes(".gk-n").forEach((n) => { if (n.hasClass("gk-hide")) hid.add(n.id()); else nodes.push(this.model.nodes.get(n.id())); });
    this.cy.edges().forEach((e) => { if (!e.hasClass("gk-hide")) edges.push(this.model.edges.get(e.data("mid"))); });
    return { nodes, edges };
  }
  async exportData(kind, o = {}) {
    const bg = this.th().surface;
    if (kind === "json") {
      const v = this.visibleModel();
      const plain = (n) => { const x = { id: n.id, kind: n.kind, label: n.label || this.model.displayLabel(n, this.o.labelFor), types: n.types, props: n.props }; if (n.datatype) x.datatype = n.datatype; if (n.lang) x.lang = n.lang; if (n.value != null) x.value = n.value; return x; };
      return new Blob([JSON.stringify({ nodes: v.nodes.map(plain), edges: v.edges.map((e) => ({ id: e.id, source: e.source, target: e.target, predicate: e.predicate, label: e.label })) }, null, 2)], { type: "application/json" });
    }
    if (kind === "nt") {
      const v = this.visibleModel(); const lines = [];
      for (const e of v.edges) {
        if (!e.predicate) continue;
        const s = this.model.nodes.get(e.source), t = this.model.nodes.get(e.target);
        if (s.kind === "literal") continue;
        lines.push(ntTerm(s) + " " + ntIri(e.predicate) + " " + ntTerm(t) + " .");
      }
      return new Blob([lines.join("\n") + (lines.length ? "\n" : "")], { type: "application/n-triples" });
    }
    if (this.chordMode) {
      const s = this.chordSvgString();
      if (kind === "svg") return new Blob([s], { type: "image/svg+xml" });
      return this.svgToPng(s, this.chordEl.clientWidth, this.chordEl.clientHeight);
    }
    if (kind === "svg") {
      const s = buildSvg(this);
      return new Blob([s], { type: "image/svg+xml" });
    }
    const full = kind === "png-full" || kind === "copy-full" || o.full;
    return this.cy.png({ output: "blob", full, scale: full ? 2 : (window.devicePixelRatio > 1 ? 2 : 1.5), bg, maxWidth: 8000, maxHeight: 8000 });
  }
  chordSvgString() { const s = this.chordEl.querySelector("svg"); return s ? new XMLSerializer().serializeToString(s) : ""; }
  svgToPng(str, w, h) {
    return new Promise((res, rej) => {
      const img = new Image();
      img.onload = () => { const c = document.createElement("canvas"); c.width = w * 2; c.height = h * 2; const x = c.getContext("2d"); x.scale(2, 2); x.drawImage(img, 0, 0, w, h); c.toBlob((b) => (b ? res(b) : rej(new Error("toBlob failed"))), "image/png"); };
      img.onerror = () => rej(new Error("SVG render failed"));
      img.src = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(str);
    });
  }
  async doExport(k) {
    try {
      if (k === "copy") {
        const blob = await this.exportData("png");
        await navigator.clipboard.write([new window.ClipboardItem({ "image/png": blob })]);
        this.say("Image copied to the clipboard"); return;
      }
      const blob = await this.exportData(k);
      const name = "graph" + ({ png: ".png", "png-full": "-full.png", svg: ".svg", json: ".json", nt: ".nt" }[k] || "");
      const a = el("a"); a.href = URL.createObjectURL(blob); a.download = name; a.rel = "noopener";
      this.root.appendChild(a); a.click(); a.remove();
      setTimeout(() => URL.revokeObjectURL(a.href), 4000);
      this.say("Saved " + name);
      this.emit("export", { kind: k, size: blob.size });
    } catch (e) { this.say("Export failed: " + trunc(e && e.message ? e.message : String(e), 100), 4000); }
  }

  destroy() {
    clearTimeout(this._tipT); clearTimeout(this._lblT); clearTimeout(this._hintT); clearTimeout(this._toastT);
    if (this._layoutObj) { try { this._layoutObj.stop(); } catch (e) { /* ok */ } }
    this._lt++;
    this.cleanups.forEach((f) => f());
    this.ro && this.ro.disconnect();
    this.mini && this.mini.destroy();
    this.cy && this.cy.destroy();
    this.root.remove();
    this.listeners = {};
  }
}
