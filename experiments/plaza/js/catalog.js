// catalog.js — the plaza grid. Loads the manifest, then for each dataset reads
// its embedded Dataset Card live (two HTTP range requests) and paints a tile
// whose image is a deterministic fingerprint of that card.
import { readReteCard, liteCardFromHeader, fmtBytes } from "./rete-card.js";
import { imageInfoFromCard } from "./procgen.js";
import { renderFingerprint } from "./procgen-p5.js";
import { derivedFacets, FILTERABLE } from "./facets.js";
import { detectProviders } from "./providers.js";
import { usedOntologies } from "./vocabs.js";

const ART = { w: 520, h: 325 };

const $ = (sel, el = document) => el.querySelector(sel);
const fmt = (n) => (n == null ? "—" : Intl.NumberFormat().format(n));
const themeNow = () => (document.documentElement.dataset.theme === "light" ? "light" : "dark");

const cats = $("#cats");
const empty = $("#empty");

// A dataset's category: its manifest `category`, else derived (ontology tags, or
// an owl-class-dominated schema). The catalog has two groups: Datasets ("graph")
// and Ontologies ("ontology" .rete files + the reference vocabularies).
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
const qInput = $("#q");
const chipsEl = $("#chips");
const countEl = $("#count");

let entries = []; // { entry, card, header, img, search } enriched as cards resolve
let activeTags = new Set();
let query = "";

(async function main() {
  const manifest = await fetch("plaza.json").then((r) => r.json());
  $("#tagline").textContent = manifest.tagline || "";
  $("#hdrmeta").textContent = `${manifest.datasets.length} datasets · cards read live from each .rete file`;

  // Seed the records + skeleton tiles immediately, fill them as cards arrive.
  entries = manifest.datasets.map((entry) => ({ entry, card: null, header: null, img: null }));
  renderChips();
  render();

  await Promise.all(
    entries.map(async (rec) => {
      try {
        const { header, card, size } = await readReteCard(rec.entry.rete);
        rec.header = header;
        rec.card = card || liteCardFromHeader(header, rec.entry);
        rec.size = size;
      } catch (err) {
        // CORS / offline / not-a-rete — fall back to manifest-only, image from key.
        rec.card = { _unreachable: true, title: rec.entry.title, description: rec.entry.blurb };
        rec.error = String(err);
      }
      rec.facets = derivedFacets(rec.card, rec.entry);
      rec.providers = detectProviders(rec.card, rec.entry);
      rec.ontologies = usedOntologies(rec.card, rec.entry);
      const mode = categoryOf(rec) === "ontology" ? "ontology" : "dataset";
      rec.img = await renderFingerprint(imageInfoFromCard(rec.card, rec.entry, rec.header, mode), { theme: themeNow(), ...ART });
      rec.search = [
        rec.entry.key,
        rec.entry.title,
        rec.entry.blurb,
        (rec.entry.tags || []).join(" "),
        rec.facets.join(" "),
        rec.providers.map((p) => p.name).join(" "),
        rec.ontologies.map((o) => o.name).join(" "),
        (rec.card.vocabularies || []).join(" "),
        rec.card.license || "",
      ]
        .join(" ")
        .toLowerCase();
      renderChips();
      render();
    })
  );
  await buildOntologyThumbs();
  render();
})();

// Procedural thumbnails for the reference-ontology cards (square nodes, sized by
// properties). Uses the backing .rete card when one provides the ontology, else
// an abstract plate seeded by the ontology name.
let ontImg = {};
async function buildOntologyThumbs() {
  const onts = aggregateOntologies();
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

function renderChips() {
  const tags = new Set();
  for (const rec of entries) {
    (rec.entry.tags || []).forEach((t) => tags.add(t));
    (rec.facets || []).forEach((t) => { if (FILTERABLE.has(t)) tags.add(t); });
  }
  chipsEl.innerHTML = "";
  [...tags].sort().forEach((t) => {
    const c = document.createElement("span");
    c.className = "chip" + (activeTags.has(t) ? " on" : "");
    c.textContent = t;
    c.onclick = () => {
      activeTags.has(t) ? activeTags.delete(t) : activeTags.add(t);
      renderChips();
      render();
    };
    chipsEl.appendChild(c);
  });
}

function matches(rec) {
  const { entry } = rec;
  const tags = [...(entry.tags || []), ...(rec.facets || [])];
  if (activeTags.size && ![...activeTags].some((t) => tags.includes(t))) return false;
  if (!query) return true;
  return (rec.search || (entry.title + " " + entry.key).toLowerCase()).includes(query);
}

function render() {
  const shown = entries.filter(matches);
  countEl.textContent = `${shown.length} / ${entries.length}`;
  cats.innerHTML = "";

  // Datasets (knowledge graphs).
  const graphs = shown.filter((r) => categoryOf(r) === "graph");
  if (graphs.length) cats.appendChild(tileSection("Datasets", graphs));

  // Ontologies = the ontology .rete files + the reference vocabularies used across the plaza.
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

// Aggregate the ontologies used across all datasets → name → {url, desc, datasets[]}.
function aggregateOntologies() {
  const omap = new Map();
  for (const rec of entries)
    for (const o of (rec.ontologies || [])) {
      const e = omap.get(o.name) || { name: o.name, url: o.url, desc: o.desc, datasets: [] };
      if (!e.desc && o.desc) e.desc = o.desc;
      if (!e.url && o.url) e.url = o.url;
      if (!e.datasets.some((d) => d.key === rec.entry.key))
        e.datasets.push({ key: rec.entry.key, title: rec.entry.title || rec.entry.key, card: rec.card });
      omap.set(o.name, e);
    }
  let onts = [...omap.values()].sort((a, b) => b.datasets.length - a.datasets.length || a.name.localeCompare(b.name));
  if (query) onts = onts.filter((o) => o.name.toLowerCase().includes(query));
  return onts;
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

function escapeHtml(s) {
  return String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
}

// Split a title at the first " — "/" – "/" - " into a name + secondary subtitle.
function splitTitle(t) {
  const s = String(t || "");
  const m = s.match(/^(.*?)\s+[—–-]\s+(.*)$/);
  return m ? { name: m[1].trim(), sub: m[2].trim() } : { name: s.trim(), sub: "" };
}

qInput.addEventListener("input", () => {
  query = qInput.value.trim().toLowerCase();
  render();
});

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
});
