// Query like a child: sketch a SPARQL query by dragging blocks.
//
// The sketch and its SPARQL live in model.js; this file draws the sketch,
// moves things by pointer (mouse, pen or finger), and runs the query on a
// .rete file through worker.js. Without a file the page still composes a
// query: empty slots stand for variables and the text is there to copy.
import {
  NS, CALC_FNS, CMP_OPS, PATHS, RECIPES, TEXT_OPS,
  clone, contains, describe, emptySketch, find, hole, holeName, iri, kindsOf, lit, make,
  nameOf, normVar, prefixer, rename, splitIri, tidy, toSparql, variables, vr, walk,
} from "./model.js";

const R2 = "https://data.graphplaza.com";
const SAMPLES = [
  ["Linked Jazz", `${R2}/linked-jazz/linked-jazz-2026-06-13.rete`],
  ["Hermetic books", `${R2}/bph/bph-2026-07-01.rete`],
  ["Wikidata paintings", `${R2}/wikidata-paintings/wikidata-paintings-2025-11-07.rete`],
];
// What the shelf offers before a file is open.
const COMMON = {
  classes: ["foaf:Person", "schema:Person", "schema:Organization", "schema:Place", "schema:CreativeWork", "schema:Event", "skos:Concept"],
  preds: ["rdfs:label", "schema:name", "dcterms:title", "dcterms:creator", "schema:author", "dcterms:date", "foaf:knows", "dcterms:subject", "owl:sameAs", "rdfs:subClassOf"],
};
const PALETTE = [
  ["Find", "pattern", [["kind", "is a kind of thing"], ["link", "is linked to"], ["name", "has a name"], ["is", "is exactly"]]],
  ["Keep only", "filter", [["text", "text contains"], ["compare", "bigger or smaller"], ["lang", "in a language"], ["calc", "work out a value"]]],
  ["Combine", "group", [["optional", "if there is"], ["without", "but without"], ["either", "either / or"]]],
];
const FAM = {};
for (const [, fam, items] of PALETTE) for (const [t] of items) FAM[t] = fam;
const HOLE_WORDS = { thing: "any thing", class: "any kind", prop: "any property", any: "anything", var: "a value", value: "a value", name: "a name" };
const HUES = [160, 35, 265, 200, 330, 95, 15, 230, 290, 55];
const ROW_CAP = 1000;

// ── small helpers ────────────────────────────────────────────────────────

const $ = (s) => document.querySelector(s);
function h(tag, attrs, ...kids) {
  const e = document.createElement(tag);
  for (const k in attrs || {}) {
    const v = attrs[k];
    if (v == null || v === false) continue;
    if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "class") e.className = v;
    else if (k === "value") e.value = v;
    else if (k in e && k !== "list" && typeof v !== "string") e[k] = v;
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const k of kids.flat()) if (k != null && k !== false) e.append(k);
  return e;
}
/** replaceChildren that skips what is not there (it would print "null"). */
const put = (el, ...kids) => el.replaceChildren(...kids.flat(2).filter((k) => k != null && k !== false));
const hash = (s) => { let x = 0; for (let i = 0; i < s.length; i++) x = (x * 31 + s.charCodeAt(i)) >>> 0; return x; };
const hue = (name) => HUES[hash(name) % HUES.length];
const strip = (t) => (typeof t === "string" && t[0] === "<" && t.endsWith(">") ? t.slice(1, -1) : t);
const fmtN = (n) => Number(n).toLocaleString("en");
function fmtBytes(n) {
  if (!n) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(u.length - 1, Math.floor(Math.log(n) / Math.log(1000)));
  return (n / 1000 ** i).toFixed(i && n / 1000 ** i < 100 ? 1 : 0) + " " + u[i];
}
let toastT = 0;
function toast(text) {
  const t = $("#toast");
  t.textContent = text; t.hidden = false;
  clearTimeout(toastT);
  toastT = setTimeout(() => { t.hidden = true; }, 2200);
}
const expand = (curie) => { const i = curie.indexOf(":"); return NS[curie.slice(0, i)] + curie.slice(i + 1); };

/** What someone typed where an IRI is expected: <iri>, a full IRI or prefix:name. */
function parseIri(s) {
  s = String(s || "").trim();
  if (!s) return null;
  if (s[0] === "<" && s.endsWith(">")) return s.slice(1, -1);
  if (/^[a-z][a-z0-9+.-]*:\/\/\S+$/i.test(s) || /^urn:\S+$/i.test(s)) return s;
  const m = /^([A-Za-z][\w-]*):(\S+)$/.exec(s);
  if (m && NS[m[1]]) return NS[m[1]] + m[2];
  return null;
}

// ── state ────────────────────────────────────────────────────────────────

let sketch = emptySketch();
let ds = null;            // the open file: { url, name, card, classes, preds, links, … }
let dsBusy = null;        // { url } while opening
let dsError = "";
let manual = null;        // SPARQL edited by hand, or null while the blocks rule
let run = { state: "idle" };
let runSeq = 0;
let runTimer = 0;
const past = [];
let snap = JSON.stringify(sketch);
const px = prefixer();    // for showing IRIs short

const ctx = () => ({ labelPredicate: ds && ds.labelPred });
const generated = () => toSparql(sketch, ctx());
const currentQuery = () => (manual != null ? manual : generated().text);

// ── engine ───────────────────────────────────────────────────────────────

const engine = {
  w: null, seq: 0, pending: new Map(), fresh: true,
  spawn() {
    this.w = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
    this.fresh = true;
    this.w.onmessage = (e) => {
      const p = this.pending.get(e.data.id);
      if (!p) return;
      this.pending.delete(e.data.id);
      if (e.data.ok) p.resolve(e.data.value); else p.reject(new Error(e.data.error));
    };
    this.w.onerror = (e) => {
      for (const p of this.pending.values()) p.reject(new Error(e.message || "the engine could not start"));
      this.pending.clear();
    };
  },
  call(type, args) {
    if (!this.w) this.spawn();
    const id = ++this.seq;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.w.postMessage({ id, type, ...args });
    });
  },
  /** Stop whatever runs: a query cannot be interrupted, the worker can. */
  stop() {
    if (this.w) this.w.terminate();
    for (const p of this.pending.values()) p.reject(new Error("stopped"));
    this.pending.clear();
    this.spawn();
  },
};

// A picked file is known to the worker that registered it; a new worker
// needs it again before the first query.
async function ensureOpen() {
  if (ds && ds.file && engine.fresh) await engine.call("local", { url: ds.url, file: ds.file });
  engine.fresh = false;
}

// ── the file ─────────────────────────────────────────────────────────────

function dataset(url, r, file) {
  const card = r.card || {};
  const pairs = (a) => (Array.isArray(a) ? a.filter((x) => Array.isArray(x)).map(([v, n]) => ({ v: strip(v), n })) : []);
  const links = (Array.isArray(card.class_links) ? card.class_links : [])
    .map((l) => ({ s: strip(l.s_class), p: strip(l.predicate), o: strip(l.o_class), n: l.count }))
    .sort((a, b) => b.n - a.n);
  const sig = card.signals || {};
  const classes = pairs(card.classes), preds = pairs(card.predicates);
  const examples = [];
  for (const q of Array.isArray(card.queries) ? card.queries : []) if (q && q.sparql) examples.push({ title: q.title || q.question || q.id, sparql: q.sparql });
  for (const q of Array.isArray(card.example_queries) ? card.example_queries : []) {
    if (typeof q !== "string") continue;
    const m = /^#\s*(.+)/.exec(q);
    examples.push({ title: m ? m[1] : q.slice(0, 50), sparql: q });
  }
  return {
    url, file, card, classes, preds, links, examples,
    hasCard: !!r.card,
    name: card.title || decodeURIComponent(url.split("/").pop() || "graph").replace(/\.rete$/, ""),
    labelPred: strip(sig.label_predicate) || null,
    labelIndex: !!(sig.label_index && sig.label_index.present),
    hubs: pairs(card.in_hubs).concat(pairs(card.top_hubs)),
    quads: card.quad_count || (r.info && r.info.quads) || 0,
    fileLength: r.fileLength || 0,
    topClass: classes[0] ? { v: classes[0].v } : null,
    topLink: links[0] ? { s: { v: links[0].s }, p: { v: links[0].p } } : preds[0] ? { p: { v: preds.find((p) => !/#type$/.test(p.v))?.v || preds[0].v } } : null,
  };
}

let localN = 0;
async function openData(url, file) {
  url = String(url || "").trim();
  if (!url) return;
  if (!file && !/^https?:\/\//i.test(url)) { dsError = "A link to a .rete file starts with http:// or https://."; renderData(); return; }
  dsBusy = { url }; dsError = "";
  renderData();
  try {
    const r = file ? await engine.call("local", { url, file }) : await engine.call("open", { url });
    engine.fresh = false;
    ds = dataset(url, r, file);
    $("#live").checked = ds.fileLength < 2e9;
  } catch (e) {
    ds = null;
    dsError = `Could not open it: ${e.message}`;
  }
  dsBusy = null;
  manual = null;
  renderAll();
  save();
  scheduleRun(0);
}
function closeData() {
  ds = null; dsError = ""; manual = null;
  runSeq++; run = { state: "idle" };
  renderAll(); save();
}

function renderData() {
  const box = $("#data");
  if (ds) {
    const c = ds.card;
    put(box, 
      h("div", { class: "data-open" },
        h("span", { class: "data-name" }, ds.name),
        h("div", { class: "data-facts" },
          ds.quads ? h("span", {}, h("b", {}, fmtN(ds.quads)), " statements") : null,
          ds.classes.length ? h("span", {}, h("b", {}, fmtN(ds.classes.length)), c.truncated && ds.classes.length >= (c.top_n || 1e9) ? "+ kinds" : " kinds") : null,
          ds.preds.length ? h("span", {}, h("b", {}, fmtN(ds.preds.length)), c.truncated && ds.preds.length >= (c.top_n || 1e9) ? "+ properties" : " properties") : null,
          ds.fileLength ? h("span", {}, h("b", {}, fmtBytes(ds.fileLength))) : null,
          c.license ? h("span", {}, String(c.license)) : null,
        ),
        h("button", { class: "btn btn-ghost btn-sm", type: "button", onclick: closeData }, "Close file"),
      ),
      c.description ? h("p", { class: "data-desc" }, String(c.description)) : null,
      !ds.hasCard ? h("p", { class: "data-err" }, "This file has no dataset card, so its kinds and properties are not listed. Open slots and typed IRIs still work.") : null,
    );
    return;
  }
  const input = h("input", { type: "url", id: "url", placeholder: "https://…/graph.rete", value: dsBusy ? dsBusy.url : "", "aria-label": "Link to a .rete file", spellcheck: false });
  const pick = h("input", { type: "file", accept: ".rete", hidden: true, onchange: (e) => { const f = e.target.files[0]; if (f) openData(`rete-local:${++localN}/${f.name}`, f); } });
  put(box, 
    h("form", { class: "data-row", onsubmit: (e) => { e.preventDefault(); openData(input.value); } },
      h("b", {}, "Ask a file"),
      h("div", { class: "data-url" }, input, h("button", { class: "btn btn-primary btn-sm", type: "submit", disabled: !!dsBusy }, dsBusy ? h("span", { class: "spin" }) : "Open")),
      h("div", { class: "samples" }, "or try",
        SAMPLES.map(([n, u]) => h("button", { class: "sample", type: "button", onclick: () => openData(u) }, n)),
        h("button", { class: "sample", type: "button", onclick: () => pick.click() }, "a file from this device…"), pick),
    ),
    dsError ? h("p", { class: "data-err" }, dsError) : null,
    !dsError && !dsBusy ? h("p", { class: "data-desc" }, "No file is needed to sketch: open slots stand for variables and the query can be copied. With a .rete file, the slots offer its own kinds, properties and things, and the result shows on the right.") : null,
  );
}

// A .rete file dropped anywhere on the page opens it.
addEventListener("dragover", (e) => { if (e.dataTransfer && [...e.dataTransfer.types].includes("Files")) { e.preventDefault(); $("#data").classList.add("over"); } });
addEventListener("dragleave", (e) => { if (!e.relatedTarget) $("#data").classList.remove("over"); });
addEventListener("drop", (e) => {
  $("#data").classList.remove("over");
  const f = e.dataTransfer && e.dataTransfer.files[0];
  if (!f) return;
  e.preventDefault();
  if (!/\.rete$/i.test(f.name)) { dsError = `${f.name} is not a .rete file.`; renderData(); return; }
  openData(`rete-local:${++localN}/${f.name}`, f);
});

// ── palette and shelf ────────────────────────────────────────────────────

function renderPalette() {
  put($("#palette"), ...PALETTE.flatMap(([title, fam, items]) => [
    h("h3", {}, title),
    ...items.map(([t, label]) => h("button", {
      class: `pal fam-${fam}`, type: "button", "data-drag": JSON.stringify({ type: "new", t }),
      title: "Drag into the sketch, or press Enter to add it at the end",
      onclick: () => { addBlock(sketch.blocks, sketch.blocks.length, newBlock(t)); },
    }, h("span", { class: "grip", "aria-hidden": "true" }, "⋮⋮"), label)),
  ]), h("h3", {}, "Shapes"),
    h("div", { class: "legend", title: "A piece fits the slots of its own shape. A variable fits anywhere." },
      [["thing", "thing"], ["class", "kind"], ["prop", "property"], ["value", "value"]].map(([s, text]) => h("span", { class: `slot demo is-hole sh-${s}` }, text))));
}

let shelfFilter = "";
function shelfItems() {
  if (ds && (ds.classes.length || ds.preds.length)) return { classes: ds.classes, preds: ds.preds, own: true };
  return { classes: COMMON.classes.map((c) => ({ v: expand(c) })), preds: COMMON.preds.map((c) => ({ v: expand(c) })), own: false };
}
function itemButton(kind, it) {
  const name = nameOf({ v: it.v });
  return h("button", {
    class: `item k-${kind}`, type: "button", title: `${it.v}\nDrag into the sketch or onto a slot`,
    "data-drag": JSON.stringify({ type: "item", kind, iri: it.v }),
    onclick: () => dropItem({ kind, iri: it.v }, sketch.blocks, sketch.blocks.length),
  }, h("span", {}, name), h("small", {}, it.n != null ? fmtN(it.n) : px.show(it.v)));
}
function renderShelf() {
  const { classes, preds, own } = shelfItems();
  const f = shelfFilter.toLowerCase();
  const pass = (it) => !f || it.v.toLowerCase().includes(f) || nameOf({ v: it.v }).toLowerCase().includes(f);
  const list = (kind, items) => h("div", { class: "shelf-list" }, items.filter(pass).slice(0, 200).map((it) => itemButton(kind, it)));
  const filter = h("input", { type: "search", placeholder: "Filter…", value: shelfFilter, "aria-label": "Filter kinds and properties",
    oninput: (e) => { shelfFilter = e.target.value; renderShelf(); $("#shelf input").focus(); } });
  put($("#shelf"), 
    h("h3", {}, own ? "In this file" : "Common vocabulary"),
    h("p", { class: "shelf-note" }, own ? "Drag a kind or a property into the sketch." : "Open a file to see its own kinds and properties here."),
    own && classes.length + preds.length > 12 ? filter : null,
    h("h3", {}, `Kinds${own ? ` · ${classes.length}` : ""}`), list("class", classes),
    h("h3", {}, `Properties${own ? ` · ${preds.length}` : ""}`), list("prop", preds),
  );
}

// ── editing the sketch ───────────────────────────────────────────────────

/** The variable a new block most likely talks about. */
function focusVar(list) {
  for (const src of [list || [], sketch.blocks]) for (let i = src.length - 1; i >= 0; i--) if (src[i].s && src[i].s.k === "var") return src[i].s.v;
  for (const [b] of walk(sketch.blocks)) if (b.s && b.s.k === "var") return b.s.v;
  return "thing";
}
function freshVar(base) {
  base = normVar(base) || "value";
  const taken = new Set(variables(sketch).map((v) => v.name));
  let n = base, i = 2;
  while (taken.has(n)) n = base + i++;
  return n;
}
function newBlock(t, list) {
  const b = make(t, focusVar(list));
  if (t === "text" || t === "lang") {
    // A text filter most often means the name just asked for.
    let last = null;
    for (const [x] of walk(sketch.blocks)) if (x.t === "name" && x.o.k === "var") last = x.o.v;
    if (last) b.v = vr(last);
  }
  if (t === "name") {
    const taken = new Set(variables(sketch).map((v) => v.name));
    if (taken.has(b.o.v)) {
      const named = [...walk(sketch.blocks)].some(([x]) => x.t === "name" && x.s.k === "var" && x.s.v === b.s.v);
      if (named || b.o.v === b.s.v) b.o = vr(freshVar(b.s.v + "Name"));
    }
  }
  return b;
}
function addBlock(list, index, block) {
  list.splice(index, 0, block);
  change();
  const el = document.querySelector(`[data-id="${block.id}"]`);
  if (el) el.scrollIntoView({ block: "nearest", behavior: "smooth" });
}
function dropItem(item, list, index) {
  const focus = focusVar(list);
  let b;
  if (item.kind === "class") b = { ...make("kind", focus), c: iri(item.iri, item.label) };
  else if (item.kind === "prop") b = { ...make("link", focus), p: iri(item.iri), o: vr(freshVar(lowerFirst(splitIri(item.iri)[1]) || "value")) };
  else b = { ...make("is", focus), vals: [iri(item.iri, item.label)] };
  addBlock(list, index, b);
}
const lowerFirst = (s) => (s ? s[0].toLowerCase() + s.slice(1) : s);

function listOf(zoneKey) {
  if (zoneKey === "root") return sketch.blocks;
  const [id, field] = zoneKey.split(":");
  const f = find(sketch, id);
  return f ? f.block[field] : null;
}
function removeBlock(id) {
  const f = find(sketch, id);
  if (f) { f.list.splice(f.index, 1); change(); }
}
function moveBlock(id, zoneKey, index) {
  const f = find(sketch, id);
  if (!f) return;
  const target = listOf(zoneKey);
  if (!target) return;
  if (target === f.list && f.index < index) index--;
  f.list.splice(f.index, 1);
  target.splice(index, 0, f.block);
  change();
}

/** After any edit: tidy, remember for undo, redraw, save, maybe run. */
function change() {
  tidy(sketch);
  const now = JSON.stringify(sketch);
  if (now !== snap) { past.push(snap); if (past.length > 100) past.shift(); snap = now; }
  if (manual != null) manual = null;
  renderSketch();
  save();
  scheduleRun();
}
function undo() {
  if (!past.length) return;
  snap = past.pop();
  sketch = JSON.parse(snap);
  renderSketch(); save(); scheduleRun();
}
function setSketch(s) { sketch = s; change(); }

// ── drawing the sketch ───────────────────────────────────────────────────

let info = new Map();

// The outline of a slot says what fits it: round for a thing, pointed at
// both ends for a kind, an arrow for a property, square for a value, and
// round-then-square where either a thing or a value may go. The shelf's
// kinds and properties carry the same outlines.
const SHAPE = { thing: "thing", class: "class", prop: "prop", any: "any" };
function slot(b, f, role, shape) {
  const t = b[f];
  const el = h("button", { class: "slot sh-" + (shape || SHAPE[role] || "value"), type: "button", "data-b": b.id, "data-f": f, "data-role": role, onclick: () => openPicker(el) });
  if (t.k === "var") {
    el.classList.add("is-var");
    el.style.setProperty("--h", hue(t.v));
    el.textContent = "?" + t.v;
    el.dataset.drag = JSON.stringify({ type: "var", name: t.v });
    el.title = [(info.get(t.v) || []).join("; "), "Click to change, drag to connect"].filter(Boolean).join("\n");
  } else if (t.k === "hole") {
    const n = holeName(sketch, b, f);
    el.classList.add("is-hole");
    el.append(h("span", {}, HOLE_WORDS[role]), h("span", { class: "hv" }, "?" + n));
    el.dataset.drag = JSON.stringify({ type: "var", name: n, from: [b.id, f] });
    el.title = `Open: it stands for the variable ?${n}. Click to fill it in, or drop something on it.`;
  } else if (t.k === "iri") {
    el.classList.add("is-iri");
    const short = px.show(t.v);
    el.append(h("span", {}, nameOf(t)), short !== t.v ? h("small", {}, short) : null);
    el.title = t.v;
  } else {
    el.classList.add("is-lit");
    el.append(t.v === "" ? h("span", { style: "color:var(--muted)" }, "a value…") : h("span", {}, t.dt === "text" ? `“${t.v}”` : String(t.v)));
    el.title = "Click to change";
  }
  return el;
}

function select(options, value, onchange, label) {
  return h("select", { "aria-label": label, onchange: (e) => onchange(e.target.value) },
    options.map(([v, text]) => h("option", { value: v, selected: v === value }, text)));
}
// A text field updates the query as you type and redraws once you leave it.
function field(value, set, attrs) {
  return h("input", { type: "text", value, ...attrs,
    oninput: (e) => { set(e.target.value); renderQuery(); scheduleRun(); },
    onchange: () => change(),
    onkeydown: (e) => { if (e.key === "Enter") e.target.blur(); } });
}

function zone(key, blocks, emptyText) {
  return h("div", { class: "zone", "data-zone": key },
    blocks.length ? blocks.map(renderBlock) : h("div", { class: "zone-empty" }, emptyText));
}

function renderBlock(b) {
  const w = (text) => h("span", { class: "word" }, text);
  let row, kids = [];
  switch (b.t) {
    case "kind": row = [slot(b, "s", "thing"), w("is a"), slot(b, "c", "class")]; break;
    case "link":
      row = [slot(b, "s", "thing"), w("→"), slot(b, "p", "prop"), w("→"), slot(b, "o", "any"),
        b.p.k === "iri" ? select(PATHS, b.path, (v) => { b.path = v; change(); }, "How the link is followed") : null];
      break;
    case "name": row = [slot(b, "s", "thing"), w("is called"), slot(b, "o", "var")]; break;
    case "is":
      row = [slot(b, "v", "var", "thing"), w("is exactly"),
        h("span", { class: "vals", "data-vals": b.id },
          b.vals.map((x, i) => h("span", { class: "val", title: x.v }, nameOf(x),
            h("button", { type: "button", "aria-label": `Remove ${nameOf(x)}`, onclick: () => { b.vals.splice(i, 1); change(); } }, "×"))),
          h("button", { class: "add", type: "button", onclick: (e) => pickThing(e.currentTarget, b) }, b.vals.length ? "+ another" : "+ pick a thing"))];
      break;
    case "text":
      row = [slot(b, "v", "var"), select(Object.entries(TEXT_OPS), b.op, (v) => { b.op = v; change(); }, "How the text is compared"),
        field(b.q, (v) => { b.q = v; }, { placeholder: "a word…", "aria-label": "Text to look for" })];
      break;
    case "compare":
      row = [slot(b, "v", "var"), select(CMP_OPS.map((o) => [o, { ">": "is more than", ">=": "is at least", "<": "is less than", "<=": "is at most", "=": "equals", "!=": "is not" }[o]]), b.op, (v) => { b.op = v; change(); }, "Comparison"),
        slot(b, "val", "value")];
      break;
    case "lang":
      row = [slot(b, "v", "var"), w("is written in"), field(b.lang, (v) => { b.lang = v.trim(); }, { class: "mini", placeholder: "en", "aria-label": "Language code" })];
      break;
    case "calc":
      row = [slot(b, "as", "name"), w("is the"), select(CALC_FNS, b.fn, (v) => { b.fn = v; change(); }, "What to work out"), w("of"), slot(b, "v", "var")];
      break;
    case "optional": row = [w("and, if there is one:")]; kids = [zone(b.id + ":kids", b.kids, "Drop blocks here. Rows are kept even when these find nothing.")]; break;
    case "without": row = [w("but without:")]; kids = [zone(b.id + ":kids", b.kids, "Drop blocks here. Rows that match them are left out.")]; break;
    case "either":
      row = [w("either:")];
      kids = [zone(b.id + ":kids", b.kids, "Drop blocks here…"), h("p", { class: "kids-label" }, "or:"), zone(b.id + ":kids2", b.kids2, "…and here.")];
      break;
  }
  const f = find(sketch, b.id);
  const step = (d) => () => { const j = f.index + d; if (j < 0 || j >= f.list.length) return; f.list.splice(f.index, 1); f.list.splice(j, 0, b); change(); };
  return h("div", { class: `block fam-${FAM[b.t]}`, "data-id": b.id },
    h("div", { class: "row" },
      h("span", { class: "handle", "data-drag": JSON.stringify({ type: "block", id: b.id }), title: "Drag to move", "aria-hidden": "true" }, "⋮⋮"),
      row,
      h("span", { class: "tools" },
        h("button", { class: "tool", type: "button", "aria-label": "Move up", onclick: step(-1) }, "↑"),
        h("button", { class: "tool", type: "button", "aria-label": "Move down", onclick: step(1) }, "↓"),
        h("button", { class: "tool del", type: "button", "aria-label": "Remove this block", onclick: () => removeBlock(b.id) }, "×"))),
    kids.length ? h("div", { class: "kids" }, kids) : null);
}

function recipeCtx() { return ds ? { topClass: ds.topClass, topLink: ds.topLink } : {}; }

function renderCanvas() {
  const root = h("div", { class: "zone root", "data-zone": "root" });
  if (sketch.blocks.length) root.append(...sketch.blocks.map(renderBlock));
  else {
    root.append(h("div", { class: "empty" },
      h("p", { class: "big" }, "Drag a block here to begin"),
      h("p", {}, ds ? `or a kind or property of ${ds.name} from the list on the left.` : "or start from one of these and change it."),
      h("div", { class: "recipes" }, RECIPES.map((r) => h("button", { class: "recipe", type: "button", onclick: () => setSketch(r.build(recipeCtx())) },
        h("b", {}, r.title), h("span", {}, r.hint))))));
  }
  put($("#canvas"), root);
}

function varChip(name, extra) {
  const c = h("span", { class: "var", "data-drag": JSON.stringify({ type: "var", name }), title: (info.get(name) || []).join("; ") }, "?" + name, extra);
  c.style.setProperty("--h", name === "count" ? 15 : hue(name));
  return c;
}

function renderVars() {
  const vs = variables(sketch);
  put($("#vars"), ...(vs.length ? [h("span", {}, "Variables"),
    ...vs.map((v) => { const c = varChip(v.name); c.tabIndex = 0; c.setAttribute("role", "button"); c.onclick = () => openVar(c, v); c.onkeydown = (e) => { if (e.key === "Enter") openVar(c, v); }; if (v.open) c.style.borderStyle = "dashed"; return c; }),
    h("span", {}, "— drag one onto a slot or into the answer")] : []));
}

function renderAnswer() {
  const a = sketch.answer;
  const names = variables(sketch).map((v) => v.name);
  const area = (key, list, emptyText) => h("span", { class: "dropvars", "data-area": key, tabindex: 0, role: "button", "aria-label": `Add a variable: ${emptyText}`,
    onclick: (e) => { if (e.target === e.currentTarget || e.target.tagName === "EM") openAreaPicker(e.currentTarget, key); },
    onkeydown: (e) => { if (e.key === "Enter" && e.target === e.currentTarget) openAreaPicker(e.currentTarget, key); } },
    list.length ? list.map((v) => varChip(v, h("button", { class: "x", type: "button", "aria-label": `Remove ?${v}`, onclick: () => { dropFromArea(key, v); } }, "×"))) : h("em", {}, emptyText));
  const mode = (m, text) => h("button", { type: "button", "aria-pressed": String(a.mode === m), onclick: () => { a.mode = m; if (m === "count" && !a.sort) a.sort = null; change(); } }, text);
  const sortable = a.mode === "count" ? [...a.per, "count"] : names;
  const sortV = a.sort && sortable.includes(a.sort.v) ? a.sort.v : (a.mode === "count" && a.per.length ? "count" : "");
  put($("#answer"), 
    h("div", { class: "ans-row" },
      h("h2", {}, "Answer"),
      h("span", { class: "seg" }, mode("list", "List"), mode("count", "Count")),
      ...(a.mode === "list"
        ? [area("show", a.show, "everything — drop variables here"),
          h("label", {}, h("input", { type: "checkbox", checked: a.distinct, onchange: (e) => { a.distinct = e.target.checked; change(); } }), "no repeats")]
        : [area("of", a.of ? [a.of] : [], "rows"), h("span", { class: "word" }, "per"), area("per", a.per, "drop variables to group by")])),
    h("div", { class: "ans-row" },
      h("label", {}, "Sort by",
        select([["", a.mode === "count" && a.per.length ? "count" : "nothing"], ...sortable.filter((v) => !(a.mode === "count" && v === "count")).map((v) => [v, "?" + v])], sortV === "count" ? "" : sortV,
          (v) => { a.sort = v ? { v, dir: a.sort ? a.sort.dir : "asc" } : (a.mode === "count" ? { v: "count", dir: a.sort ? a.sort.dir : "desc" } : null); change(); }, "Sort by")),
      (a.sort || (a.mode === "count" && a.per.length)) ? select([["asc", "smallest first"], ["desc", "largest first"]], a.sort ? a.sort.dir : "desc",
        (d) => { a.sort = { v: a.sort ? a.sort.v : "count", dir: d }; change(); }, "Direction") : null,
      h("label", {}, "At most", h("input", { type: "number", min: 1, value: a.limit ?? "", placeholder: "all", "aria-label": "Row limit",
        onchange: (e) => { a.limit = e.target.value === "" ? null : Math.max(1, Math.floor(Number(e.target.value) || 1)); change(); } }), "rows")));
}
function dropFromArea(key, v) {
  const a = sketch.answer;
  if (key === "show") a.show = a.show.filter((x) => x !== v);
  else if (key === "per") a.per = a.per.filter((x) => x !== v);
  else a.of = null;
  change();
}
function addToArea(key, v) {
  const a = sketch.answer;
  if (key === "show") { if (!a.show.includes(v)) a.show.push(v); }
  else if (key === "per") { if (!a.per.includes(v)) a.per.push(v); }
  else a.of = v;
  change();
}

function renderSketch() {
  info = describe(sketch);
  renderCanvas(); renderVars(); renderAnswer(); renderQuery();
  $("#undo").disabled = !past.length;
}
function renderAll() {
  renderData(); renderShelf(); renderSketch(); renderRun();
}

// ── popovers ─────────────────────────────────────────────────────────────

function openPop(anchor, kids) {
  const pop = $("#pop");
  put(pop, ...kids.flat().filter(Boolean));
  pop.hidden = false;
  const r = anchor.getBoundingClientRect();
  const width = Math.min(310, innerWidth - 20);
  pop.style.left = Math.max(10, Math.min(r.left + scrollX, scrollX + innerWidth - width - 10)) + "px";
  pop.style.top = r.bottom + scrollY + 6 + "px";
  const first = pop.querySelector("input, button");
  if (first && matchMedia("(pointer:fine)").matches) first.focus({ preventScroll: true });
}
function closePop() { $("#pop").hidden = true; }
addEventListener("pointerdown", (e) => { if (!$("#pop").hidden && !e.target.closest("#pop")) closePop(); }, true);
addEventListener("keydown", (e) => {
  if (e.key === "Escape") closePop();
  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z" && !e.target.closest("input, textarea")) { e.preventDefault(); undo(); }
});

function entryRow(placeholder, button, onUse, extra) {
  const input = h("input", { type: "text", placeholder, spellcheck: false, onkeydown: (e) => { if (e.key === "Enter") { e.preventDefault(); onUse(input.value); } } });
  return h("div", { class: "pop-row" }, input, extra, h("button", { class: "btn btn-ghost btn-sm", type: "button", onclick: () => onUse(input.value) }, button));
}

/** The properties or kinds the file offers for a slot, the fitting ones first. */
function offers(role, b) {
  const { classes, preds } = shelfItems();
  if (role === "class") return classes.map((c) => ({ ...c }));
  const sKinds = b.s && b.s.k === "var" ? kindsOf(sketch, b.s.v) : [];
  const oKinds = b.o && b.o.k === "var" ? kindsOf(sketch, b.o.v) : [];
  const fit = new Set();
  if (ds) for (const l of ds.links) if (sKinds.includes(l.s) || oKinds.includes(l.o)) fit.add(l.p);
  const out = preds.map((p) => ({ ...p, fits: fit.has(p.v) }));
  return out.sort((a, c) => c.fits - a.fits);
}

function openPicker(el) {
  const f0 = find(sketch, el.dataset.b);
  if (!f0) return;
  const b = f0.block, f = el.dataset.f, role = el.dataset.role, t = b[f];
  const set = (term) => { b[f] = term; closePop(); change(); };
  const useIri = (text) => { const v = parseIri(text); if (v) set(iri(v)); else toast("That is not an IRI: use http://… or a known prefix:name"); };
  const kids = [];
  const named = variables(sketch).filter((v) => !v.open && !(t.k === "var" && t.v === v.name));

  if (role === "class" || role === "prop") {
    const items = offers(role, b);
    const listBox = h("div", { class: "pop-list" });
    const draw = (q) => {
      q = q.toLowerCase();
      const shown = items.filter((it) => !q || it.v.toLowerCase().includes(q) || nameOf({ v: it.v }).toLowerCase().includes(q)).slice(0, 80);
      put(listBox, ...(shown.length ? shown.map((it) => h("button", { class: "opt" + (it.fits ? " fits" : ""), type: "button", title: it.v, onclick: () => set(iri(it.v)) },
        h("span", {}, nameOf({ v: it.v }), h("span", { class: "sub" }, px.show(it.v))),
        h("small", {}, it.fits ? "fits · " : "", it.n != null ? fmtN(it.n) : ""))) : [h("p", { class: "hint" }, "Nothing matches.")]));
    };
    draw("");
    kids.push(h("h4", {}, role === "class" ? (ds ? `Kinds in ${ds.name}` : "A kind") : (ds ? `Properties in ${ds.name}` : "A property")),
      items.length > 8 ? h("div", { class: "pop-row" }, h("input", { type: "search", placeholder: "Filter…", oninput: (e) => draw(e.target.value) })) : null,
      listBox);
  }
  if (role === "thing" || role === "any") {
    kids.push(h("h4", {}, "One thing"),
      ds ? h("button", { class: "opt", type: "button", onclick: () => { closePop(); openFinder({ kinds: [], sample: sampleFor(b, f) || anySample(), onPick: (x) => { b[f] = iri(x.v, x.l); change(); } }); } },
        h("span", {}, "Find a thing by name…"), h("small", {}, ds.name)) : null);
  }
  if (role === "any" || role === "value") {
    const type = h("select", { "aria-label": "Type of value" }, [["text", "text"], ["number", "number"], ["date", "date / year"]].map(([v, x]) => h("option", { value: v, selected: t.k === "lit" && t.dt === v }, x)));
    const row = entryRow(t.k === "lit" ? String(t.v) : "a value", "Use", (v) => { if (v !== "") set(lit(v.trim(), type.value)); }, type);
    if (t.k === "lit") row.querySelector("input").value = t.v;
    row.querySelector("input").addEventListener("input", (e) => {
      const v = e.target.value.trim();
      if (/^\d{4}(-\d\d-\d\d)?$/.test(v)) type.value = "date"; else if (/^[+-]?\d*\.?\d+$/.test(v)) type.value = "number"; else if (v) type.value = "text";
    });
    kids.push(h("h4", {}, "A value"), row);
  }
  if (role !== "value" || named.length) {
    kids.push(h("h4", {}, role === "name" ? "Call it" : "A variable"),
      named.length ? h("div", { class: "pop-vars" }, named.map((v) => { const c = varChip(v.name); c.setAttribute("role", "button"); c.tabIndex = 0; c.onclick = () => set(vr(v.name)); c.onkeydown = (e) => { if (e.key === "Enter") set(vr(v.name)); }; return c; })) : null,
      entryRow("a new name", "Use", (v) => { const n = normVar(v); if (n) set(vr(n)); }));
    if (t.k === "var") kids.push(entryRow(`rename ?${t.v} everywhere`, "Rename", (v) => { if (normVar(v)) { rename(sketch, t.v, v); closePop(); change(); } }));
  }
  if (role === "class" || role === "prop" || role === "thing" || role === "any")
    kids.push(h("h4", {}, "An IRI"), entryRow("http://… or prefix:name", "Use", useIri));
  if (t.k !== "hole" && ["s", "c", "p", "o", "v"].includes(f))
    kids.push(h("button", { class: "opt", type: "button", style: "margin-top:8px", onclick: () => set(hole()) }, h("span", {}, "Leave it open"), h("small", {}, HOLE_WORDS[role] || "a variable")));
  openPop(el, kids);
}

function openVar(anchor, v) {
  const lines = info.get(v.name) || [];
  openPop(anchor, [
    h("h4", {}, "?" + v.name),
    lines.length ? h("div", { class: "pop-list" }, lines.map((l) => h("p", { class: "hint", style: "margin:2px" }, l))) : h("p", { class: "hint" }, "Nothing is said about it yet."),
    v.open ? h("p", { class: "hint" }, "It stands for an open slot. Drag it onto another slot to give it a place of its own.") :
      entryRow("a new name", "Rename", (t) => { if (normVar(t)) { rename(sketch, v.name, t); closePop(); change(); } }),
  ]);
}

function openAreaPicker(anchor, key) {
  const a = sketch.answer;
  const have = key === "show" ? a.show : key === "per" ? a.per : [a.of];
  const vs = variables(sketch).filter((v) => !have.includes(v.name));
  openPop(anchor, [h("h4", {}, key === "show" ? "Show" : key === "per" ? "Group by" : "Count different"),
    vs.length ? h("div", { class: "pop-vars" }, vs.map((v) => { const c = varChip(v.name); c.setAttribute("role", "button"); c.tabIndex = 0; const go = () => { closePop(); nameHole(v.name); addToArea(key, v.name); }; c.onclick = go; c.onkeydown = (e) => { if (e.key === "Enter") go(); }; return c; }))
      : h("p", { class: "hint" }, "No more variables: add a block first.")]);
}

/** An open slot named in the answer becomes a variable of that name. */
function nameHole(name) {
  for (const [b] of walk(sketch.blocks)) for (const f of ["s", "c", "p", "o", "v"]) if (b[f] && b[f].k === "hole" && holeName(sketch, b, f) === name) { b[f] = vr(name); return; }
}

// ── finding a thing ──────────────────────────────────────────────────────

function pickThing(anchor, b) {
  const add = (x) => { if (!b.vals.some((y) => y.v === x.v)) b.vals.push(iri(x.v, x.l)); change(); };
  if (ds) { openFinder({ kinds: b.v.k === "var" ? kindsOf(sketch, b.v.v) : [], sample: (b.v.k === "var" && varSample(b.v.v)) || anySample(), onPick: add }); return; }
  openPop(anchor, [h("h4", {}, "An IRI"),
    entryRow("http://… or prefix:name", "Add", (text) => { const v = parseIri(text); if (v) { closePop(); add({ v }); } else toast("That is not an IRI: use http://… or a known prefix:name"); }),
    h("p", { class: "hint" }, "With a file open, this becomes a search by name.")]);
}

// What a slot most likely wants, read off the sketch: things of the kind
// its variable is said to be, or the things a property points to or from.
const kindSample = (c) => ({ title: nameOf({ v: c }), pattern: (pre) => `?s a ${pre.term(c)} .` });
function sampleFor(b, f) {
  if (b.t === "link" && b.p.k === "iri" && (f === "o" || f === "s")) {
    const forward = (f === "o") !== (b.path === "^");
    return forward
      ? { title: `what “${nameOf(b.p)}” points to`, pattern: (pre) => `?x ${pre.term(b.p.v)} ?s . FILTER(isIRI(?s))` }
      : { title: `things with “${nameOf(b.p)}”`, pattern: (pre) => `?s ${pre.term(b.p.v)} ?x .` };
  }
  if (b.t === "kind" && f === "s" && b.c.k === "iri") return kindSample(b.c.v);
  return b[f] && b[f].k === "var" ? varSample(b[f].v) : null;
}
function varSample(name) {
  const kinds = kindsOf(sketch, name);
  if (kinds.length) return kindSample(kinds[0]);
  for (const [x] of walk(sketch.blocks)) {
    if (x.t !== "link" || x.p.k !== "iri") continue;
    if (x.s.k === "var" && x.s.v === name) return sampleFor(x, "s");
    if (x.o.k === "var" && x.o.v === name) return sampleFor(x, "o");
  }
  return null;
}
const anySample = () => (ds && ds.topClass ? kindSample(ds.topClass.v) : null);
const labelPath = (pre) => (ds.labelPred ? pre.term(ds.labelPred) : ["rdfs:label", "schema:name", "foaf:name", "skos:prefLabel", "dcterms:title"].map((c) => pre.term(expand(c))).join("|"));

/** A handful of instances, read with one small query. */
async function quickSample(sample) {
  const pre = prefixer();
  const where = sample.pattern(pre), labels = labelPath(pre);
  const query = `${pre.header()}\nSELECT DISTINCT ?s ?l WHERE {\n  ${where}\n  OPTIONAL { ?s ${labels} ?l }\n} LIMIT 40`;
  await ensureOpen();
  const r = await engine.call("query", { url: ds.url, query });
  const seen = new Set(), rows = [];
  for (const row of r.result.rows || []) {
    const v = strip(row.s);
    if (!v || row.s[0] !== "<" || seen.has(v) || rows.length >= 20) continue;
    seen.add(v);
    rows.push({ v, l: (parseTerm(row.l) || {}).v });
  }
  return { rows, bytes: r.bytes };
}

let finderSeq = 0;
function openFinder({ kinds, sample, onPick }) {
  const dlg = $("#finder");
  const body = h("div", { class: "finder-body" });
  const foot = h("span", {});
  const input = h("input", { type: "search", placeholder: "Start typing a name…", "aria-label": "Name to look for", spellcheck: false });
  const pick = (x) => { dlg.close(); onPick(x); };
  const option = (x, small) => h("button", { class: "opt", type: "button", title: x.v, onclick: () => pick(x) },
    h("span", {}, x.l || nameOf({ v: x.v }), h("span", { class: "sub" }, px.show(x.v))), small ? h("small", {}, small) : null);
  const none = (text, ...more) => put(body, h("p", { class: "none" }, text), ...more);

  const hubs = () => {
    const seen = new Set(), items = [];
    for (const c of ds.classes) seen.add(c.v);
    for (const x of ds.hubs) if (!seen.has(x.v) && items.length < 14) { seen.add(x.v); items.push(x); }
    const hubNodes = items.length ? [h("h4", {}, "Well-connected things"), ...items.map((x) => option(x, fmtN(x.n) + " links"))] : [];
    foot.textContent = "Type the start of a name.";
    if (!sample) { if (hubNodes.length) put(body, hubNodes); else none("Type the start of a name."); return; }
    put(body, h("p", { class: "none" }, h("span", { class: "spin" }), ` Taking a quick sample of ${sample.title}…`), hubNodes);
    const my = ++finderSeq;
    quickSample(sample).then((r) => {
      if (my !== finderSeq || input.value.trim()) return;
      put(body, r.rows.length ? [h("h4", {}, `A sample of ${sample.title}`), ...r.rows.map((x) => option(x))] : h("p", { class: "none" }, `Nothing sampled for ${sample.title}.`), hubNodes);
      foot.textContent = `${r.rows.length} sampled · ${fmtBytes(r.bytes)} read · type to search by name`;
    }).catch(() => { if (my === finderSeq && !input.value.trim()) { if (hubNodes.length) put(body, hubNodes); else none("Type the start of a name."); } });
  };

  const deep = async (q) => {
    const my = ++finderSeq;
    none("Searching inside names…");
    const pre = prefixer();
    const labels = labelPath(pre);
    const kind = kinds.length ? `  ?s a ${pre.term(kinds[0])} .\n` : "";
    const where = `${kind}  ?s ${labels} ?l .\n  FILTER(CONTAINS(LCASE(STR(?l)), "${q.toLowerCase().replace(/[\\"]/g, "\\$&")}"))`;
    const query = `${pre.header()}\nSELECT DISTINCT ?s ?l WHERE {\n${where}\n} LIMIT 40`;
    try {
      await ensureOpen();
      const r = await engine.call("query", { url: ds.url, query });
      if (my !== finderSeq) return;
      const rows = (r.result.rows || []).map((row) => ({ v: strip(row.s), l: (parseTerm(row.l) || {}).v })).filter((x) => x.v && !x.v.startsWith("_:"));
      if (rows.length) put(body, h("h4", {}, `Names containing “${q}”`), ...rows.map((x) => option(x)));
      else none(`No name contains “${q}”.`);
      foot.textContent = `${rows.length} found · ${fmtBytes(r.bytes)} read`;
    } catch (e) {
      if (my === finderSeq) none(`The search failed: ${e.message}`);
    }
  };

  const search = async () => {
    const q = input.value.trim();
    const my = ++finderSeq;
    if (!q) { hubs(); return; }
    const asIri = parseIri(q);
    const deeper = h("button", { class: "linkish", type: "button", onclick: () => deep(q) }, "Search inside names instead (reads more of the file)");
    try {
      await ensureOpen();
      const r = await engine.call("search", { url: ds.url, prefix: q, limit: 40 });
      if (my !== finderSeq) return;
      const hits = (r.hits || []).map((x) => ({ v: strip(x.subject), l: x.label }));
      if (hits.length) ds.labelIndex = true;
      // A file without a label index answers nothing here; a small one can
      // afford the search inside names straight away.
      if (!hits.length && !asIri && !ds.labelIndex && ds.fileLength < 3e8) { deep(q); return; }
      put(body,
        asIri ? option({ v: asIri }, "use this IRI") : null,
        ...(hits.length ? [h("h4", {}, `Names starting with “${q}”`), ...hits.map((x) => option(x))] : [h("p", { class: "none" }, `No name starts with “${q}”.`)]),
        h("p", { class: "none" }, deeper));
      foot.textContent = `${hits.length}${hits.length === 40 ? "+" : ""} found`;
    } catch (e) {
      if (my === finderSeq) none(`The search failed: ${e.message}`, h("p", { class: "none" }, deeper));
    }
  };

  let timer = 0;
  input.oninput = () => { clearTimeout(timer); timer = setTimeout(search, 180); };
  input.onkeydown = (e) => {
    if (e.key === "Enter") { e.preventDefault(); const first = body.querySelector(".opt"); if (first) first.click(); else if (input.value.trim()) deep(input.value.trim()); }
    if (e.key === "ArrowDown") { const first = body.querySelector(".opt"); if (first) { e.preventDefault(); first.focus(); } }
  };
  put(dlg, 
    h("div", { class: "finder-head" }, h("h3", {}, "Find a thing"),
      h("p", {}, `in ${ds.name}` + (kinds.length ? ` · the wider search keeps to ${nameOf({ v: kinds[0] })}` : "")), input),
    body,
    h("div", { class: "finder-foot" }, foot, h("button", { class: "btn btn-ghost btn-sm", type: "button", onclick: () => dlg.close() }, "Close")));
  hubs();
  dlg.showModal();
  input.focus();
}
$("#finder").addEventListener("click", (e) => { if (e.target === e.currentTarget) e.currentTarget.close(); });

// ── drag and drop ────────────────────────────────────────────────────────
//
// Pointer events, so one code path serves mouse, pen and touch. A drag starts
// after a few pixels; a press that never moves stays a click.

let drag = null;
let swallowClick = false;

function accepts(data, el) {
  if (el.classList.contains("dropvars")) return data.type === "var";
  if (el.hasAttribute("data-vals")) return data.type === "item" && data.kind === "entity";
  const role = el.dataset.role;
  if (data.type === "var") return true;
  if (data.type === "item") return (data.kind === "class" && role === "class") || (data.kind === "prop" && role === "prop") || (data.kind === "entity" && (role === "thing" || role === "any"));
  return false;
}

function locate(x, y) {
  const el = document.elementFromPoint(x, y);
  if (!el) return null;
  const d = drag.data;
  const s = el.closest(".slot, .dropvars, [data-vals]");
  if (s && s !== drag.src && accepts(d, s)) return { type: "slot", el: s };
  if (d.type === "var") return null;
  const z = el.closest(".zone");
  if (z) {
    if (d.type === "block") {
      const own = document.querySelector(`.block[data-id="${d.id}"]`);
      if (own && own.contains(z)) return null;
    }
    const blocks = [...z.children].filter((c) => c.classList.contains("block"));
    let index = blocks.length;
    for (let i = 0; i < blocks.length; i++) {
      const r = blocks[i].getBoundingClientRect();
      if (y < r.top + r.height / 2) { index = i; break; }
    }
    return { type: "zone", el: z, index, before: blocks[index] || null };
  }
  if (el.closest("#canvas")) {
    const root = document.querySelector(".zone.root");
    return { type: "zone", el: root, index: sketch.blocks.length, before: null };
  }
  if (d.type === "block" && el.closest("#rail")) return { type: "bin" };
  return null;
}

const ins = h("div", { class: "ins" });
function mark(target) {
  document.querySelectorAll(".over").forEach((e) => e.classList.remove("over"));
  ins.remove();
  if (!target) return;
  if (target.type === "slot") target.el.classList.add("over");
  else if (target.type === "zone") {
    const empty = target.el.querySelector(":scope > .zone-empty, :scope > .empty");
    if (target.before) target.el.insertBefore(ins, target.before); else if (empty) target.el.insertBefore(ins, empty); else target.el.append(ins);
  }
}

function drop(target) {
  const d = drag.data;
  if (!target) return;
  if (target.type === "bin") { removeBlock(d.id); return; }
  if (target.type === "zone") {
    const key = target.el.dataset.zone, list = listOf(key);
    if (!list) return;
    if (d.type === "new") addBlock(list, target.index, newBlock(d.t, list));
    else if (d.type === "block") moveBlock(d.id, key, target.index);
    else if (d.type === "item") dropItem(d, list, target.index);
    return;
  }
  const el = target.el;
  if (el.classList.contains("dropvars")) {
    if (d.from) { const src = find(sketch, d.from[0]); if (src) src.block[d.from[1]] = vr(d.name); } else nameHole(d.name);
    addToArea(el.dataset.area, d.name);
    return;
  }
  if (el.hasAttribute("data-vals")) {
    const f = find(sketch, el.dataset.vals);
    if (f && !f.block.vals.some((x) => x.v === d.iri)) { f.block.vals.push(iri(d.iri, d.label)); change(); }
    return;
  }
  const f = find(sketch, el.dataset.b);
  if (!f) return;
  if (d.type === "var") {
    // A variable dragged out of an open slot names both ends.
    if (d.from) { const src = find(sketch, d.from[0]); if (src) src.block[d.from[1]] = vr(d.name); } else nameHole(d.name);
    f.block[el.dataset.f] = vr(d.name);
  } else f.block[el.dataset.f] = iri(d.iri, d.label);
  change();
}

addEventListener("pointerdown", (e) => {
  if (e.button !== 0 || !e.isPrimary) return;
  const src = e.target.closest("[data-drag]");
  if (!src || e.target.closest("input, select, textarea, .x")) return;
  drag = { src, data: JSON.parse(src.dataset.drag), x: e.clientX, y: e.clientY, started: false, ghost: null, target: null };
});
addEventListener("pointermove", (e) => {
  if (!drag) return;
  if (!drag.started) {
    if (Math.hypot(e.clientX - drag.x, e.clientY - drag.y) < 5) return;
    drag.started = true;
    closePop();
    const shown = drag.data.type === "block" ? drag.src.closest(".block") : drag.src;
    const r = shown.getBoundingClientRect();
    drag.ghost = shown.cloneNode(true);
    drag.ghost.classList.add("ghost-drag");
    drag.ghost.style.width = Math.min(r.width, 360) + "px";
    drag.dx = Math.min(drag.x - r.left, 340); drag.dy = drag.y - r.top;
    document.body.append(drag.ghost);
    document.body.classList.add("dragging");
    if (drag.data.type === "var" || drag.data.type === "item") {
      document.body.classList.add("matching");
      document.querySelectorAll("#canvas .slot, .dropvars, [data-vals]").forEach((s) => { if (s !== drag.src && accepts(drag.data, s)) s.classList.add("can"); });
    }
    if (drag.data.type === "block") shown.classList.add("drag-src");
  }
  e.preventDefault();
  drag.ghost.style.left = e.clientX - drag.dx + "px";
  drag.ghost.style.top = e.clientY - drag.dy + "px";
  drag.target = locate(e.clientX, e.clientY);
  mark(drag.target);
  // Near the edge of the window, scroll so far-away zones can be reached.
  const board = $(".board"), br = board.getBoundingClientRect();
  const dir = e.clientY < Math.max(60, br.top + 30) ? -14 : e.clientY > Math.min(innerHeight - 60, br.bottom - 30) ? 14 : 0;
  if (dir) { scrollBy(0, dir); if (e.clientX > br.left && e.clientX < br.right) board.scrollBy(0, dir); }
});
function endDrag(e, cancelled) {
  if (!drag) return;
  const d = drag;
  if (d.started) {
    d.ghost.remove();
    document.body.classList.remove("dragging", "matching");
    document.querySelectorAll(".can").forEach((x) => x.classList.remove("can"));
    document.querySelectorAll(".drag-src").forEach((x) => x.classList.remove("drag-src"));
    const target = cancelled ? null : locate(e.clientX, e.clientY);
    mark(null);
    drop(target);
    swallowClick = true;
    setTimeout(() => { swallowClick = false; }, 0);
  }
  drag = null;
}
addEventListener("pointerup", (e) => endDrag(e, false));
addEventListener("pointercancel", (e) => endDrag(e, true));
addEventListener("click", (e) => { if (swallowClick) { e.stopPropagation(); e.preventDefault(); } }, true);
// A link in a result cell is draggable; keep the browser's own drag out of it.
addEventListener("dragstart", (e) => { if (e.target.closest && e.target.closest("[data-drag]")) e.preventDefault(); });

// ── the query text ───────────────────────────────────────────────────────

const KEYWORDS = /^(PREFIX|SELECT|DISTINCT|WHERE|FILTER|OPTIONAL|UNION|NOT|EXISTS|VALUES|BIND|AS|GROUP|ORDER|BY|LIMIT|OFFSET|DESC|ASC|COUNT|SUM|AVG|MIN|MAX|CONTAINS|LCASE|UCASE|STR|STRSTARTS|STRLEN|REGEX|LANG|DATATYPE|YEAR|MONTH|MINUS|ASK|CONSTRUCT|DESCRIBE|HAVING|SERVICE|GRAPH)$/i;
function highlight(text) {
  const out = [];
  const re = /(<[^>\s]*>)|(#[^\n]*)|("(?:[^"\\\n]|\\.)*")|([?$][\w]+)|\b([A-Za-z_]+)\b(?![:\w])|(\b\d+(?:\.\d+)?\b)/g;
  let last = 0, m;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    const cls = m[2] ? "co" : m[3] ? "st" : m[4] ? "va" : m[5] && KEYWORDS.test(m[5]) ? "kw" : m[6] ? "nu" : null;
    out.push(cls ? h("span", { class: cls }, m[0]) : m[0]);
    last = m.index + m[0].length;
  }
  out.push(text.slice(last));
  return out;
}

function renderQuery() {
  const g = generated();
  const text = manual != null ? manual : g.text;
  const body = manual != null
    ? h("textarea", { spellcheck: false, "aria-label": "SPARQL query", value: text, oninput: (e) => { manual = e.target.value; scheduleRun(900); } })
    : h("pre", { "aria-label": "SPARQL query" }, h("code", {}, highlight(text)));
  const examples = ds && ds.examples.length ? h("select", { "aria-label": "Queries from this file's card", onchange: (e) => {
    const x = ds.examples[Number(e.target.value)];
    if (x) { manual = x.sparql; renderQuery(); scheduleRun(0); }
  } }, h("option", { value: "" }, "From this file's card…"), ds.examples.map((x, i) => h("option", { value: i }, x.title))) : null;
  put($("#query"), body,
    h("div", { class: "query-bar" },
      manual != null ? h("span", { class: "tag" }, "Written by hand: the blocks are not in step") : null,
      examples,
      manual != null
        ? h("button", { class: "linkish", type: "button", onclick: () => { manual = null; renderQuery(); scheduleRun(0); } }, "Back to the blocks")
        : h("button", { class: "linkish", type: "button", onclick: () => { manual = g.text; renderQuery(); $("#query textarea").focus(); } }, "Edit as text"),
      h("button", { class: "linkish", type: "button", onclick: () => copy(currentQuery(), "Query copied") }, "Copy")));
  put($("#notes"), ...(manual == null ? g.problems.map((p) => h("p", { class: "note" }, p)) : []));
}
async function copy(text, done) {
  try { await navigator.clipboard.writeText(text); toast(done); }
  catch { toast("Copying is blocked here: select the text instead"); }
}

// ── running ──────────────────────────────────────────────────────────────

const NUMERIC = /#(integer|decimal|double|float|int|long|short|byte|nonNegativeInteger|positiveInteger|unsignedInt|unsignedLong)$/;
function unesc(s) {
  return s.replace(/\\(u[0-9a-fA-F]{4}|U[0-9a-fA-F]{8}|.)/g, (m, c) => (c[0] === "u" || c[0] === "U" ? String.fromCodePoint(parseInt(c.slice(1), 16)) : { n: "\n", t: "\t", r: "\r" }[c] ?? c));
}
function parseTerm(s) {
  if (s == null) return null;
  if (typeof s !== "string") return { t: "lit", v: String(s) };
  if (s.startsWith("<<")) return { t: "triple", v: s };
  if (s[0] === "<" && s.endsWith(">")) return { t: "iri", v: s.slice(1, -1) };
  if (s.startsWith("_:")) return { t: "bnode", v: s };
  if (s[0] === '"') {
    const m = /^"([\s\S]*)"(?:@([A-Za-z0-9-]+)|\^\^<([^>]*)>)?$/.exec(s);
    if (m) return { t: "lit", v: unesc(m[1]), lang: m[2], dt: m[3], num: m[3] && NUMERIC.test(m[3]) };
  }
  return { t: "lit", v: s };
}

let waiting = false;      // a run is scheduled or in flight
function scheduleRun(delay = 450) {
  clearTimeout(runTimer);
  waiting = false;
  if (!ds || !$("#live").checked) return;
  waiting = true;
  runTimer = setTimeout(runNow, delay);
}
async function runNow() {
  clearTimeout(runTimer);
  if (!ds) { renderRun(); return; }
  const text = currentQuery();
  const my = ++runSeq;
  waiting = true;
  run = { state: "running", since: performance.now() };
  renderRun();
  try {
    await ensureOpen();
    const r = await engine.call("query", { url: ds.url, query: text });
    if (my !== runSeq) return;
    run = { state: "done", ...r };
  } catch (e) {
    if (my !== runSeq) return;
    run = e.message === "stopped" ? { state: "idle", note: "Stopped." } : { state: "error", error: e.message };
  }
  waiting = false;
  renderRun();
}

function cell(tok) {
  const t = parseTerm(tok);
  if (!t) return h("td", {});
  if (t.t === "iri") {
    const isImg = /\.(jpe?g|png|gif|webp|svg)(\?.*)?$/i.test(t.v) || /Special:FilePath\//.test(t.v);
    if (isImg) return h("td", {}, h("a", { href: t.v, target: "_blank", rel: "noopener" }, h("img", { src: t.v, alt: "", loading: "lazy", referrerpolicy: "no-referrer" })));
    const short = px.show(t.v);
    return h("td", {}, h("a", { class: "cell-iri", href: t.v, target: "_blank", rel: "noopener", title: `${t.v}\nDrag into the sketch to ask about exactly this`,
      "data-drag": JSON.stringify({ type: "item", kind: "entity", iri: t.v }) }, short !== t.v ? short : nameOf({ v: t.v })),
      short === t.v ? h("small", {}, " " + (splitIri(t.v)[0].replace(/^https?:\/\//, "").split("/")[0])) : null);
  }
  if (t.t === "lit") return h("td", { class: t.num ? "num" : null }, t.num && isFinite(Number(t.v)) ? fmtN(Number(t.v)) : t.v, t.lang ? h("small", {}, " @" + t.lang) : null);
  return h("td", {}, t.v);
}

function renderRun() {
  const status = $("#status"), out = $("#out");
  status.className = "status";
  $("#run").disabled = !ds;
  if (!ds) {
    put(status, );
    put(out, h("div", { class: "idle" }, h("b", {}, "Nothing to run it on yet"),
      "The query above is complete and ready to copy. To see rows, open a .rete file by link, or try ",
      h("button", { class: "linkish", type: "button", onclick: () => openData(SAMPLES[0][1]) }, SAMPLES[0][0]), "."));
    return;
  }
  if (run.state === "idle") {
    put(status, run.note || "");
    put(out, h("div", { class: "idle" }, h("b", {}, `Ready to ask ${ds.name}`), "Press Run, or turn Live on to see rows as you sketch."));
    return;
  }
  if (run.state === "running") {
    put(status, h("span", { class: "spin" }), " Asking… ", h("button", { class: "linkish", type: "button", onclick: () => { runSeq++; engine.stop(); run = { state: "idle", note: "Stopped." }; renderRun(); } }, "Stop"));
    out.style.opacity = ".5";
    return;
  }
  out.style.opacity = "";
  if (run.state === "error") {
    status.className = "status err";
    status.textContent = run.error;
    put(out, );
    return;
  }
  const r = run.result;
  const cost = [`${run.ms < 10 ? run.ms.toFixed(1) : Math.round(run.ms)} ms`, `${fmtBytes(run.bytes)} read`, run.savedBytes ? `${fmtBytes(run.savedBytes)} from this browser's cache` : null].filter(Boolean).join(" · ");
  if (r.kind === "ask") {
    put(status, cost);
    put(out, h("div", { class: "ask" }, r.boolean ? "Yes" : "No"));
    return;
  }
  if (r.kind === "construct") {
    const triples = r.triples || [];
    put(status, h("b", {}, `${fmtN(triples.length)} statements`), " · ", cost);
    put(out, h("table", {}, h("tbody", {}, triples.slice(0, ROW_CAP).map((t) => h("tr", {}, t.map(cell))))));
    return;
  }
  const rows = r.rows || [], vars = r.vars || [];
  const limit = manual == null ? Number(sketch.answer.limit) : NaN;
  put(status, h("b", {}, rows.length === 1 ? "1 row" : `${fmtN(rows.length)} rows`), " · ", cost,
    rows.length && rows.length === limit ? " · the limit was reached, there may be more" : "",
    rows.length > ROW_CAP ? ` · showing the first ${fmtN(ROW_CAP)}` : "");
  if (!rows.length) {
    put(out, h("div", { class: "idle" }, h("b", {}, "No rows"), "Nothing in the file matches the whole sketch. Open a slot, or move a block into “if there is”."));
    return;
  }
  put(out, h("table", {},
    h("thead", {}, h("tr", {}, vars.map((v) => { const s = h("span", {}, "?" + v); s.style.setProperty("--h", v === "count" ? 15 : hue(v)); return h("th", {}, s); }))),
    h("tbody", {}, rows.slice(0, ROW_CAP).map((row) => h("tr", {}, vars.map((v) => cell(row[v])))))));
}

// ── saving and sharing ───────────────────────────────────────────────────

const b64 = {
  enc: (s) => btoa(String.fromCharCode(...new TextEncoder().encode(s))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""),
  dec: (s) => new TextDecoder().decode(Uint8Array.from(atob(s.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0))),
};
function save() {
  const state = { s: sketch };
  if (ds && !ds.file) state.u = ds.url;
  const empty = !sketch.blocks.length && !state.u;
  try { history.replaceState(null, "", empty ? location.pathname + location.search : "#s=" + b64.enc(JSON.stringify(state))); } catch { /* a sandboxed page */ }
}
function restore() {
  let url = new URLSearchParams(location.search).get("rete");
  const m = /^#s=(.+)$/.exec(location.hash);
  if (m) {
    try {
      const st = JSON.parse(b64.dec(m[1]));
      if (st.s && Array.isArray(st.s.blocks) && st.s.answer) { sketch = { ...emptySketch(), ...st.s, answer: { ...emptySketch().answer, ...st.s.answer } }; snap = JSON.stringify(sketch); }
      if (st.u) url = st.u;
    } catch { /* an old or cut link: start empty */ }
  }
  return url;
}

// ── start ────────────────────────────────────────────────────────────────

for (const r of RECIPES) $("#recipe").append(h("option", { value: r.id }, r.title));
$("#recipe").onchange = (e) => { const r = RECIPES.find((x) => x.id === e.target.value); e.target.value = ""; if (r) setSketch(r.build(recipeCtx())); };
$("#clear").onclick = () => setSketch(emptySketch());
$("#undo").onclick = undo;
$("#share").onclick = () => { save(); copy(location.href, ds && ds.file ? "Link copied (the sketch only: a file from this device cannot be linked)" : "Link copied"); };
$("#run").onclick = runNow;
$("#live").onchange = () => scheduleRun(0);

const startUrl = restore();
renderPalette();
renderAll();
if (startUrl) openData(startUrl);

// For the browser test.
window.__sketch = { get: () => clone(sketch), set: setSketch, query: currentQuery, busy: () => waiting || !!dsBusy };
