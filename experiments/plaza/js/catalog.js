// catalog.js — the plaza front page.
//
// Loads the manifest, reads each dataset's embedded Card live (two HTTP range
// requests, never the graph), and paints a gallery whose artwork is a
// deterministic fingerprint of that card.
//
// The browsing model is faceted, in the way a model hub is: filters are
// CUMULATIVE — values inside one group widen (OR), groups narrow each other
// (AND) — and each value carries the count it would yield, computed against the
// OTHER groups' selections so a number is never a lie about what clicking does.
import { readReteCard, liteCardFromHeader, fmtBytes } from "./rete-card.js";
import { imageInfoFromCard } from "./procgen.js";
import { renderFingerprint } from "./procgen-p5.js";
import { derivedFacets, FILTERABLE } from "./facets.js";
import { detectProviders } from "./providers.js";
import { usedOntologies } from "./vocabs.js";
import { mountSearch } from "./search.js";

const ART = { w: 520, h: 325 };
const SPOT_MS = 7000;

const $ = (sel, el = document) => el.querySelector(sel);
const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const themeNow = () => (document.documentElement.dataset.theme === "light" ? "light" : "dark");
const escapeHtml = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));

const cats = $("#cats");
const empty = $("#empty");
const countEl = $("#count");
const facetsEl = $("#facets");
const activeEl = $("#active");
const clearBtn = $("#clearAll");
const sortEl = $("#sort");

let entries = [];              // { entry, card, header, img, facets, ontologies, providers, search }
let query = "";
let sortBy = "relevance";
const selected = new Map();    // groupId -> Set(value)

const SIDEBAR_ROWS = 6;        // values shown in the sidebar; the rest via the picker
let pickerGid = null;          // group whose full list the modal is showing
let pickerQuery = "";

// ── facet model ────────────────────────────────────────────────────────────
// Each group knows how to read its values off a record. Everything else — the
// sidebar, the counts, the tokens, the search index — is derived from this.
const SIZE_BUCKETS = [
  { label: "under 1 MB", test: (n) => n != null && n < 1e6 },
  { label: "1 – 50 MB", test: (n) => n != null && n >= 1e6 && n < 5e7 },
  { label: "50 – 500 MB", test: (n) => n != null && n >= 5e7 && n < 5e8 },
  { label: "over 500 MB", test: (n) => n != null && n >= 5e8 },
];

// "GeoSPARQL" is both a derived FEATURE (the file carries geometry) and a
// VOCABULARY (it imports the ontology) — the same word for two different facts.
// The underlying value stays as-is (dataset.html shows the same chips); only the
// sidebar wording is disambiguated.
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
  // Named to match the hero rail — same facet, two places to reach it.
  { id: "vocab", label: "Built with", open: true, of: (r) => (r.ontologies || []).map((o) => o.name) },
  { id: "topic", label: "Topic", open: true, of: (r) => r.entry.tags || [] },
  { id: "licence", label: "Licence", open: false, of: (r) => { const l = (r.card && r.card.license) || r.entry.license; return l ? [l] : []; } },
  { id: "size", label: "File size", open: false, of: (r) => { const b = SIZE_BUCKETS.find((s) => s.test(r.size)); return b ? [b.label] : []; } },
  { id: "provider", label: "Connected to", open: false, of: (r) => (r.providers || []).map((p) => p.name) },
];
const GROUP_BY_ID = Object.fromEntries(GROUPS.map((g) => [g.id, g]));

const valuesOf = (rec, gid) => {
  try { return GROUP_BY_ID[gid].of(rec) || []; } catch { return []; }
};

/** Does `rec` satisfy every selected group except `skip`? */
function passesFacets(rec, skip) {
  for (const [gid, set] of selected) {
    if (!set.size || gid === skip) continue;
    const vals = valuesOf(rec, gid);
    if (!vals.some((v) => set.has(v))) return false;
  }
  return true;
}
const passesQuery = (rec) =>
  !query || (rec.search || (rec.entry.title + " " + rec.entry.key).toLowerCase()).includes(query);

const matches = (rec) => passesFacets(rec, null) && passesQuery(rec);

function toggle(gid, value) {
  const set = selected.get(gid) || new Set();
  set.has(value) ? set.delete(value) : set.add(value);
  set.size ? selected.set(gid, set) : selected.delete(gid);
  render();
}

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

// ── boot ───────────────────────────────────────────────────────────────────
(async function main() {
  const manifest = await fetch("plaza.json").then((r) => r.json());
  entries = manifest.datasets.map((entry) => ({ entry, card: null, header: null, img: null }));
  render();

  await Promise.all(
    entries.map(async (rec) => {
      try {
        const { header, card, size } = await readReteCard(rec.entry.rete);
        rec.header = header;
        rec.card = card || liteCardFromHeader(header, rec.entry);
        rec.size = size;
      } catch (err) {
        // CORS / offline / not-a-rete — fall back to manifest-only.
        rec.card = { _unreachable: true, title: rec.entry.title, description: rec.entry.blurb };
        rec.error = String(err);
      }
      rec.facets = derivedFacets(rec.card, rec.entry);
      rec.providers = detectProviders(rec.card, rec.entry);
      rec.ontologies = usedOntologies(rec.card, rec.entry);
      const mode = categoryOf(rec) === "ontology" ? "ontology" : "dataset";
      rec.img = await renderFingerprint(imageInfoFromCard(rec.card, rec.entry, rec.header, mode), { theme: themeNow(), ...ART });
      rec.search = [
        rec.entry.key, rec.entry.title, rec.entry.blurb,
        (rec.entry.tags || []).join(" "),
        (rec.facets || []).join(" "),
        (rec.providers || []).map((p) => p.name).join(" "),
        (rec.ontologies || []).map((o) => o.name).join(" "),
        (rec.card.vocabularies || []).join(" "),
        rec.card.license || "",
      ].join(" ").toLowerCase();
      render();
    })
  );
  await buildOntologyThumbs();
  render();
  startSpotlight();
})();

// ── search ─────────────────────────────────────────────────────────────────
mountSearch({
  input: $("#q"),
  panel: $("#ac"),
  onQuery: (q) => { query = q; render(); },
  getIndex: () => {
    const idx = [];
    for (const rec of entries) {
      idx.push({
        type: "dataset",
        label: rec.entry.title || rec.entry.key,
        meta: rec.card && rec.card.triple_count != null ? `${fmt(rec.card.triple_count)} triples` : "open",
        pick: () => { location.href = `dataset.html?key=${encodeURIComponent(rec.entry.key)}`; },
      });
    }
    for (const o of aggregateOntologies(true)) {
      idx.push({
        type: "ontology",
        label: o.name,
        meta: `${o.datasets.length} dataset${o.datasets.length > 1 ? "s" : ""}`,
        pick: () => { location.href = `ontology.html?id=${encodeURIComponent(o.name)}`; },
      });
    }
    for (const gid of ["topic", "licence", "provider", "feature"]) {
      const type = { topic: "tag", licence: "licence", provider: "provider", feature: "tag" }[gid];
      const g = GROUP_BY_ID[gid];
      for (const [value, n] of countsFor(gid, true)) {
        if (!n) continue;
        idx.push({
          type,
          label: g.display ? g.display(value) : value,
          meta: `${n} dataset${n > 1 ? "s" : ""}`,
          pick: () => toggle(gid, value),
        });
      }
    }
    return idx;
  },
});

// ── facet counts ───────────────────────────────────────────────────────────
/** [value, count] for one group, counted against the OTHER groups + the query.
 *
 *  Seeded with every value the whole corpus has, so an option that the current
 *  filters reduce to zero is shown greyed at 0 rather than DISAPPEARING — a
 *  vanishing checkbox reads as a bug, and it hides the fact that the
 *  combination is empty. */
function countsFor(gid, ignoreQuery = false) {
  const tally = new Map();
  for (const rec of entries) for (const v of valuesOf(rec, gid)) tally.set(v, 0);
  for (const rec of entries) {
    if (!passesFacets(rec, gid)) continue;
    if (!ignoreQuery && !passesQuery(rec)) continue;
    for (const v of valuesOf(rec, gid)) tally.set(v, (tally.get(v) || 0) + 1);
  }
  return [...tally.entries()].sort((a, b) => b[1] - a[1] || String(a[0]).localeCompare(String(b[0])));
}

// ── sidebar ────────────────────────────────────────────────────────────────
function renderFacets() {
  facetsEl.innerHTML = "";
  for (const g of GROUPS) {
    const counts = countsFor(g.id);
    if (!counts.length) continue;
    const sel = selected.get(g.id) || new Set();
    const isOpen = g.open || sel.size > 0;

    const det = document.createElement("details");
    det.className = "pz-group";
    det.open = isOpen;
    det.innerHTML = `<summary>${escapeHtml(g.label)}${sel.size ? ` <span class="n">${sel.size}</span>` : ""}</summary>`;

    const box = document.createElement("div");
    box.className = "pz-opts";
    // Only the head of the list lives in the sidebar. Thirty vocabularies in a
    // column is a scroll, not a filter — the rest go behind the picker.
    const show = counts.slice(0, SIDEBAR_ROWS);
    for (const [value, n] of show) {
      const on = sel.has(value);
      const b = document.createElement("button");
      b.type = "button";
      b.className = "pz-opt" + (on ? " on" : "");
      if (!n && !on) b.disabled = true;
      const shown = g.display ? g.display(value) : value;
      b.title = shown === value ? value : `${shown} (${value})`;
      b.innerHTML = `<span class="box">${on ? "✓" : ""}</span><span class="lab">${escapeHtml(shown)}</span><span class="n">${n}</span>`;
      b.onclick = () => toggle(g.id, value);
      box.appendChild(b);
    }
    if (counts.length > SIDEBAR_ROWS) {
      const more = document.createElement("button");
      more.type = "button";
      more.className = "pz-more";
      more.innerHTML = `Show all ${counts.length} <span aria-hidden="true">›</span>`;
      more.onclick = () => openPicker(g.id);
      box.appendChild(more);
    }
    det.appendChild(box);
    facetsEl.appendChild(det);
  }
}

// ── facet picker ───────────────────────────────────────────────────────────
// The full value list of one group: searchable, multi-select, applied live.
// Selected values are pinned to the top, because a filter you cannot find is a
// filter you cannot remove — the whole reason the sidebar keeps them visible.
const picker = $("#facetModal");

function openPicker(gid) {
  pickerGid = gid;
  pickerQuery = "";
  $("#facetSearch").value = "";
  $("#facetTitle").textContent = GROUP_BY_ID[gid].label;
  picker.hidden = false;
  renderPicker();
  setTimeout(() => $("#facetSearch").focus(), 30);
}
function closePicker() {
  picker.hidden = true;
  pickerGid = null;
}

function renderPicker() {
  if (!pickerGid) return;
  const g = GROUP_BY_ID[pickerGid];
  const sel = selected.get(pickerGid) || new Set();
  const counts = countsFor(pickerGid);
  const label = (v) => (g.display ? g.display(v) : v);

  const hits = counts.filter(([v]) => !pickerQuery || label(v).toLowerCase().includes(pickerQuery));
  hits.sort((a, b) => Number(sel.has(b[0])) - Number(sel.has(a[0])) || b[1] - a[1] || String(a[0]).localeCompare(String(b[0])));

  const list = $("#facetList");
  list.innerHTML = "";
  if (!hits.length) {
    list.innerHTML = `<div class="pz-picker-empty">Nothing matches “${escapeHtml(pickerQuery)}”.</div>`;
  }
  for (const [value, n] of hits) {
    const on = sel.has(value);
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pz-pick" + (on ? " on" : "");
    if (!n && !on) b.disabled = true;
    b.innerHTML = `<span class="box">${on ? "✓" : ""}</span><span class="lab">${escapeHtml(label(value))}</span><span class="n">${n}</span>`;
    b.onclick = () => toggle(pickerGid, value); // render() repaints this list
    list.appendChild(b);
  }

  const shown = entries.filter(matches).length;
  $("#facetSummary").textContent =
    `${sel.size} selected · ${shown} of ${entries.length} dataset${entries.length === 1 ? "" : "s"}`;
  $("#facetClear").disabled = !sel.size;
}

$("#facetSearch")?.addEventListener("input", (e) => {
  pickerQuery = e.target.value.trim().toLowerCase();
  renderPicker();
});
$("#facetClear")?.addEventListener("click", () => {
  if (pickerGid) selected.delete(pickerGid);
  render();
});
$("#facetDone")?.addEventListener("click", closePicker);
$("#facetX")?.addEventListener("click", closePicker);
picker?.addEventListener("click", (e) => { if (e.target === picker) closePicker(); });
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && pickerGid) closePicker();
});

function renderActive() {
  activeEl.innerHTML = "";
  let any = false;
  for (const [gid, set] of selected) {
    for (const value of set) {
      any = true;
      const g = GROUP_BY_ID[gid];
      const tok = document.createElement("span");
      tok.className = "pz-tok";
      tok.innerHTML = `<em>${escapeHtml(g.label)}</em>${escapeHtml(g.display ? g.display(value) : value)}<button aria-label="Remove filter">×</button>`;
      tok.querySelector("button").onclick = () => toggle(gid, value);
      activeEl.appendChild(tok);
    }
  }
  if (query) {
    any = true;
    const tok = document.createElement("span");
    tok.className = "pz-tok";
    tok.innerHTML = `<em>Search</em>${escapeHtml(query)}<button aria-label="Clear search">×</button>`;
    tok.querySelector("button").onclick = () => { query = ""; $("#q").value = ""; render(); };
    activeEl.appendChild(tok);
  }
  clearBtn.hidden = !any;
}

clearBtn.onclick = () => { selected.clear(); query = ""; $("#q").value = ""; render(); };
sortEl.onchange = () => { sortBy = sortEl.value; render(); };

// The sidebar collapses on narrow screens (see the media query); this is its
// disclosure. It carries the active-filter count so a collapsed panel can never
// hide the fact that filters are on.
const side = $("#side");
const sideToggle = $("#sideToggle");
sideToggle?.addEventListener("click", () => {
  const open = side.classList.toggle("open");
  sideToggle.setAttribute("aria-expanded", String(open));
});
function renderSideToggle() {
  const n = [...selected.values()].reduce((a, s) => a + s.size, 0);
  const badge = $("#sideToggleN");
  if (!badge) return;
  badge.hidden = !n;
  badge.textContent = String(n);
  // A filter applied from the rail or the search box should be visible even if
  // the visitor never opened the panel.
  if (n && window.matchMedia("(max-width: 900px)").matches) side.classList.add("open");
}

// ── hero ───────────────────────────────────────────────────────────────────
function renderHeroStats() {
  const withCard = entries.filter((r) => r.card && !r.card._unreachable);
  const triples = withCard.reduce((a, r) => a + (r.card.triple_count ?? r.card.quad_count ?? 0), 0);
  const bytes = entries.reduce((a, r) => a + (r.size || 0), 0);
  const vocabs = new Set(entries.flatMap((r) => (r.ontologies || []).map((o) => o.name)));
  const el = $("#heroStats");
  if (!el) return;
  el.innerHTML = [
    [entries.length, "datasets"],
    [triples ? fmt(triples) : "…", "triples"],
    [bytes ? fmtBytes(bytes) : "…", "of graph"],
    [vocabs.size || "…", "vocabularies"],
  ].map(([b, s]) => `<div class="pz-stat"><b>${escapeHtml(String(b))}</b><span>${s}</span></div>`).join("");
}

let spotIdx = 0;
let spotTimer = null;
const spotPool = () => entries.filter((r) => r.img && r.card && !r.card._unreachable).slice(0, 6);

function renderSpot() {
  const pool = spotPool();
  if (!pool.length) return;
  spotIdx %= pool.length;
  const rec = pool[spotIdx];
  const t = splitTitle(rec.entry.title || rec.entry.key);
  const triples = rec.card.triple_count ?? rec.card.quad_count;

  $("#spotArt").innerHTML = `<img src="${rec.img}" alt="">`;
  $("#spotKicker").textContent = categoryOf(rec) === "ontology" ? "ontology in the plaza" : "in the plaza";
  const a = $("#spotTitle");
  a.textContent = t.name;
  a.href = `dataset.html?key=${encodeURIComponent(rec.entry.key)}`;
  $("#spotSub").textContent = t.sub || rec.entry.blurb || "";
  $("#spotStats").innerHTML = [
    rec.size ? `<span><b>${fmtBytes(rec.size)}</b></span>` : "",
    triples != null ? `<span><b>${fmt(triples)}</b> triples</span>` : "",
    (rec.ontologies || []).length ? `<span>${escapeHtml(rec.ontologies.slice(0, 3).map((o) => o.name).join(" · "))}</span>` : "",
  ].join("");

  const dots = $("#spotDots");
  dots.innerHTML = "";
  pool.forEach((_, i) => {
    const d = document.createElement("button");
    d.className = "pz-spot-dot" + (i === spotIdx ? " on" : "");
    d.type = "button";
    d.setAttribute("aria-label", `Spotlight ${i + 1}`);
    d.onclick = () => { spotIdx = i; renderSpot(); restartSpotlight(); };
    dots.appendChild(d);
  });
}

function startSpotlight() {
  renderSpot();
  restartSpotlight();
  const spot = $("#spot");
  spot?.addEventListener("mouseenter", () => clearInterval(spotTimer));
  spot?.addEventListener("mouseleave", restartSpotlight);
}
function restartSpotlight() {
  clearInterval(spotTimer);
  spotTimer = setInterval(() => { spotIdx++; renderSpot(); }, SPOT_MS);
}

function renderRail() {
  const rail = $("#ontRail");
  if (!rail) return;
  const sel = selected.get("vocab") || new Set();
  const onts = aggregateOntologies(true).slice(0, 14);
  rail.innerHTML = "";
  for (const o of onts) {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "pz-onto" + (sel.has(o.name) ? " on" : "");
    b.innerHTML = `<i>${escapeHtml(o.name.slice(0, 1).toUpperCase())}</i>${escapeHtml(o.name)}<s>${o.datasets.length}</s>`;
    b.title = o.desc || `Filter by ${o.name}`;
    b.onclick = () => toggle("vocab", o.name);
    rail.appendChild(b);
  }
}

// ── ontologies ─────────────────────────────────────────────────────────────
let ontImg = {};
async function buildOntologyThumbs() {
  const onts = aggregateOntologies(true);
  await Promise.all(
    onts.map(async (o) => {
      const backing = entries.find((r) => (r.entry.provides || []).includes(o.name));
      const info = backing
        ? imageInfoFromCard(backing.card, backing.entry, backing.header, "ontology")
        : { seed: "onto:" + o.name, mode: "ontology", name: o.name, tags: [], triples: Math.max(20, o.datasets.length * 60), classes: [], links: [], vocabularies: [], geo: false, temporal: false, incoherent: false };
      ontImg[o.name] = await renderFingerprint(info, { theme: themeNow(), w: 360, h: 225, labels: false });
    })
  );
}

/** name -> {url, desc, datasets[]}, across every record (or only the shown ones). */
function aggregateOntologies(all = false) {
  const pool = all ? entries : entries.filter(matches);
  const omap = new Map();
  for (const rec of pool)
    for (const o of rec.ontologies || []) {
      const e = omap.get(o.name) || { name: o.name, url: o.url, desc: o.desc, datasets: [] };
      if (!e.desc && o.desc) e.desc = o.desc;
      if (!e.url && o.url) e.url = o.url;
      if (!e.datasets.some((d) => d.key === rec.entry.key))
        e.datasets.push({ key: rec.entry.key, title: rec.entry.title || rec.entry.key, card: rec.card });
      omap.set(o.name, e);
    }
  return [...omap.values()].sort((a, b) => b.datasets.length - a.datasets.length || a.name.localeCompare(b.name));
}

// ── grid ───────────────────────────────────────────────────────────────────
const SORTERS = {
  relevance: (a, b) => Number(Boolean(b.card)) - Number(Boolean(a.card)) || tri(b) - tri(a),
  name: (a, b) => String(a.entry.title || a.entry.key).localeCompare(String(b.entry.title || b.entry.key)),
  triples: (a, b) => tri(b) - tri(a),
  size: (a, b) => (b.size || 0) - (a.size || 0),
  vocabs: (a, b) => (b.ontologies || []).length - (a.ontologies || []).length,
};
const tri = (r) => (r.card && (r.card.triple_count ?? r.card.quad_count)) || 0;

function render() {
  renderFacets();
  renderActive();
  renderSideToggle();
  renderPicker();
  renderHeroStats();
  renderRail();

  const shown = entries.filter(matches).sort(SORTERS[sortBy] || SORTERS.relevance);
  countEl.textContent = `${shown.length} of ${entries.length}`;
  cats.innerHTML = "";

  const graphs = shown.filter((r) => categoryOf(r) === "graph");
  if (graphs.length) cats.appendChild(tileSection("Datasets", graphs));

  const ontTiles = shown.filter((r) => categoryOf(r) === "ontology");
  const onts = aggregateOntologies();
  if (ontTiles.length || onts.length) {
    const sec = document.createElement("section");
    sec.className = "cat-sec";
    sec.innerHTML = `<h2 class="cat">Ontologies<span class="cat-n">${ontTiles.length + onts.length}</span></h2>`;
    if (ontTiles.length) {
      const g = document.createElement("div");
      g.className = "grid";
      for (const rec of ontTiles) g.appendChild(tile(rec));
      sec.appendChild(g);
    }
    if (onts.length) {
      const sub = document.createElement("div");
      sub.innerHTML = `<div class="notice" style="margin:14px 0 8px">vocabularies &amp; external ontologies these datasets are built with</div>`;
      const og = document.createElement("div");
      og.className = "ogrid";
      for (const o of onts) {
        const a = document.createElement("a");
        a.className = "ocard";
        a.href = `ontology.html?id=${encodeURIComponent(o.name)}`;
        a.innerHTML = `<div class="art">${ontImg[o.name] ? `<img class="art-img" src="${ontImg[o.name]}" alt="">` : ""}<span class="kind">ontology</span></div>
          <div class="ocard-body"><div class="ocard-name">${escapeHtml(o.name)}</div><div class="ocard-meta">${o.datasets.length} dataset${o.datasets.length > 1 ? "s" : ""}</div></div>`;
        og.appendChild(a);
      }
      sub.appendChild(og);
      sec.appendChild(sub);
    }
    cats.appendChild(sec);
  }
  empty.hidden = shown.length > 0 || onts.length > 0;
}

function tileSection(label, recs) {
  const sec = document.createElement("section");
  sec.className = "cat-sec";
  sec.innerHTML = `<h2 class="cat">${label}<span class="cat-n">${recs.length}</span></h2><div class="grid"></div>`;
  const grid = sec.querySelector(".grid");
  for (const rec of recs) grid.appendChild(tile(rec));
  return sec;
}

function tile(rec) {
  const { entry, card, img } = rec;
  const el = document.createElement("a");
  el.className = "card" + (card ? "" : " skeleton");
  el.href = `dataset.html?key=${encodeURIComponent(entry.key)}`;

  const triples = card && (card.triple_count ?? card.quad_count);
  const terms = card && card.term_count;
  const license = (card && card.license) || entry.license;
  const kindLabel = categoryOf(rec) === "ontology" ? "ontology" : "dataset";
  const size = fmtBytes(rec.size);
  const nameOf = splitTitle(entry.title || entry.key);

  el.innerHTML = `
    <div class="art">
      ${img ? `<img class="art-img" src="${img}" alt="">` : ""}
      <span class="kind">${kindLabel}</span>
    </div>
    <div class="body">
      <div class="title">${escapeHtml(nameOf.name)}</div>
      ${nameOf.sub ? `<div class="subtitle">${escapeHtml(nameOf.sub)}</div>` : ""}
      <div class="blurb">${escapeHtml((card && card.description) || entry.blurb || "")}</div>
      <div class="stats">
        ${size ? `<span class="size"><b>${size}</b></span>` : ""}
        ${triples != null ? `<span><b>${fmt(triples)}</b> triples</span>` : ""}
        ${terms != null ? `<span><b>${fmt(terms)}</b> terms</span>` : ""}
        ${license ? `<span>${escapeHtml(license)}</span>` : ""}
        ${rec.error ? `<span title="${escapeHtml(rec.error)}">card unreachable</span>` : ""}
        ${card && card._lite ? `<span title="file has no embedded card">header-only</span>` : ""}
      </div>
      ${rec.ontologies && rec.ontologies.length ? `<div class="conns-mini" title="built with these ontologies / vocabularies">⚙ ${rec.ontologies.slice(0, 4).map((o) => escapeHtml(o.name)).join(" · ")}</div>` : ""}
      ${rec.providers && rec.providers.length ? `<div class="conns-mini" title="connected to external databases">↔ ${rec.providers.slice(0, 4).map((pv) => escapeHtml(pv.name)).join(" · ")}</div>` : ""}
      <div class="tags">
        ${(rec.facets || []).map((t) => `<span class="facet">${escapeHtml(t)}</span>`).join("")}
        ${(entry.tags || []).slice(0, 3).map((t) => `<span>${escapeHtml(t)}</span>`).join("")}
      </div>
    </div>`;
  return el;
}

/** Split a title at the first " — "/" – "/" - " into a name + secondary subtitle. */
function splitTitle(t) {
  const s = String(t || "");
  const m = s.match(/^(.*?)\s+[—–-]\s+(.*)$/);
  return m ? { name: m[1].trim(), sub: m[2].trim() } : { name: s.trim(), sub: "" };
}

// Re-skin the procedural images when the theme toggles.
window.addEventListener("plaza-theme", async () => {
  await Promise.all(
    entries.map(async (rec) => {
      if (!rec.card) return;
      const mode = categoryOf(rec) === "ontology" ? "ontology" : "dataset";
      rec.img = await renderFingerprint(imageInfoFromCard(rec.card, rec.entry, rec.header, mode), { theme: themeNow(), ...ART });
    })
  );
  await buildOntologyThumbs();
  render();
  renderSpot();
});
