// ontology.js — a page for one ontology or vocabulary. Hybrid model:
//   • its description and homepage, and the terms seen across the plaza, are
//     extracted from the datasets' .rete cards;
//   • if a dataset PROVIDES this ontology as a full .rete (manifest `provides`),
//     its real schema is drawn (UML diagram) and linked.
// Every card is read with a bounded pool (a few range requests each, never the
// graphs). Cards the home page already read are reused from its sessionStorage
// cache.
import { readReteCard, liteCardFromHeader } from "./rete-card.js";
import { usedOntologies, ontologyTerms, ontologyMeta } from "./vocabs.js";
import { mountSchemaUML } from "./schema-uml.js";

const root = document.getElementById("root");
const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const httpsify = (u) => {
  const s = String(u || "");
  return /^http:\/\/(localhost|127\.0\.0\.1)/i.test(s) ? s : s.replace(/^http:\/\//i, "https://");
};
const enc = encodeURIComponent;
const abs = (u) => { try { return new URL(u, location.href).href; } catch (_) { return String(u); } };

const CACHE_PREFIX = "plaza:card:v1:"; // shared with catalog.js
const POOL = 8;
const READ_TIMEOUT_MS = 20000;

const cacheGet = (url) => { try { const s = sessionStorage.getItem(CACHE_PREFIX + url); return s ? JSON.parse(s) : null; } catch (_) { return null; } };
const cachePut = (url, v) => { try { sessionStorage.setItem(CACHE_PREFIX + url, JSON.stringify(v)); } catch (_) { /* full or blocked */ } };

main().catch((e) => {
  console.error(e);
  root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-err" role="alert"><strong>This page failed to load.</strong> ${esc((e && e.message) || e)}</div></div>`;
});

async function readOne(entry) {
  const url = abs(entry.rete);
  const hit = cacheGet(url);
  if (hit && hit.header) return { entry, card: hit.card || liteCardFromHeader(hit.header, entry), header: hit.header };
  try {
    const r = await Promise.race([
      readReteCard(url),
      new Promise((_, rej) => setTimeout(() => rej(new Error("timeout")), READ_TIMEOUT_MS)),
    ]);
    cachePut(url, { header: r.header, card: r.card, size: r.size });
    return { entry, card: r.card || liteCardFromHeader(r.header, entry), header: r.header };
  } catch (_) {
    return { entry, card: { _unreachable: true }, header: null };
  }
}

async function main() {
  const id = new URLSearchParams(location.search).get("id");
  if (!id) {
    root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-warn"><div><strong>No ontology specified.</strong> <a href="index.html">Back to all datasets.</a></div></div></div>`;
    return;
  }
  document.title = `${id} · rete plaza`;
  let manifest;
  try {
    manifest = await fetch("plaza.json").then((r) => { if (!r.ok) throw new Error("HTTP " + r.status); return r.json(); });
  } catch (e) {
    root.innerHTML = `<div class="wrap"><div class="dz-banner dz-banner-err" role="alert"><div><strong>The plaza listing could not be loaded.</strong> ${esc(e.message || e)}</div></div></div>`;
    return;
  }

  // Read every dataset's card so we can find which ones use this ontology.
  const list = manifest.datasets;
  const recs = new Array(list.length);
  let next = 0, done = 0;
  const note = root.querySelector(".dz-loading");
  const worker = async () => {
    while (next < list.length) {
      const i = next++;
      recs[i] = await readOne(list[i]);
      done++;
      if (note) note.textContent = `Reading dataset cards: ${done} of ${list.length}`;
    }
  };
  await Promise.all(Array.from({ length: Math.min(POOL, list.length) }, worker));

  const using = recs.filter((r) => usedOntologies(r.card, r.entry).some((o) => o.name === id));
  const unreachable = recs.filter((r) => r.card._unreachable).length;

  // Metadata: prefer what a using dataset reports, else the registry.
  let meta = { url: null, desc: null };
  for (const r of using) {
    const e = usedOntologies(r.card, r.entry).find((o) => o.name === id);
    if (e) { meta = { url: e.url, desc: e.desc }; break; }
  }
  if (!meta.url && !meta.desc) meta = ontologyMeta(id);

  // A dataset that provides this ontology as a full .rete (for the real schema).
  const backing = manifest.datasets.find((d) => (d.provides || []).includes(id));
  const backingRec = backing ? recs.find((r) => r.entry.key === backing.key) : null;
  const hasSchema = backingRec && ((backingRec.card.classes || []).length || (backingRec.card.class_links || []).length);

  // All terms of this ontology seen anywhere in the corpus.
  const termSet = new Set();
  for (const r of using) for (const t of ontologyTerms(id, r.card)) termSet.add(t);
  const termsAll = [...termSet].sort();

  const unknown = !using.length && !meta.url && !meta.desc && !backing;

  root.innerHTML = `
    <section class="ont-hero" aria-labelledby="h1">
      <div class="wrap">
        <p class="pz-kicker">Ontology or vocabulary</p>
        <h1 id="h1">${esc(id)}</h1>
        ${meta.desc ? `<p class="dz-desc">${esc(meta.desc)}</p>` : ""}
        <div class="dz-chips">
          <span class="pz-chip">${using.length} dataset${using.length === 1 ? "" : "s"}</span>
          ${termsAll.length ? `<span class="pz-chip">${termsAll.length} terms seen</span>` : ""}
          ${backing ? `<span class="pz-chip pz-chip-accent">full .rete available</span>` : ""}
        </div>
        <div class="dz-actions-top">
          ${meta.url ? `<a class="pz-btn" href="${esc(httpsify(meta.url))}" target="_blank" rel="noopener">Specification ↗</a>` : ""}
          ${backing ? `<a class="pz-btn pz-btn-primary" href="dataset.html?key=${enc(backing.key)}">Open ${esc(backing.title ? backing.title.split(/\s+[—–-]\s+/)[0] : backing.key)}</a>` : ""}
        </div>
      </div>
    </section>

    <div class="wrap ont-body">
      ${unknown ? `<div class="dz-banner dz-banner-warn"><div><strong>Nothing is known about “${esc(id)}”.</strong> No dataset in the plaza lists it. <a href="index.html">Browse the datasets</a>.</div></div>` : ""}
      ${unreachable ? `<div class="dz-banner dz-banner-info"><div>${unreachable} of ${list.length} dataset files could not be reached, so the lists below may be incomplete.</div></div>` : ""}

      ${hasSchema ? `<section class="dz-sec" aria-labelledby="h-schema">
        <div class="dz-sec-head"><h2 id="h-schema">Schema</h2></div>
        <div class="schema-wrap">
          <div id="schemaGraph" class="schema-graph"></div>
          <div id="schemaInfo" class="schema-info"></div>
        </div>
        <p class="dz-fine">The schema of the full <code>.rete</code> that provides this ontology (${esc(backing.title || backing.key)}). Boxes are classes and arrows are object properties. Click a class to expand it.</p>
      </section>` : ""}

      <section class="dz-sec" aria-labelledby="h-terms">
        <div class="dz-sec-head"><h2 id="h-terms">Terms used across the plaza</h2></div>
        ${termsAll.length
          ? `<p class="dz-chips" style="margin-top:0">${termsAll.map((t) => `<span class="pz-chip">${esc(t)}</span>`).join("")}</p>`
          : `<p class="dz-empty">No specific classes or properties of this ontology were found in the dataset cards${hasSchema ? ". See the full schema above" : ""}.</p>`}
      </section>

      <section class="dz-sec" aria-labelledby="h-used">
        <div class="dz-sec-head"><h2 id="h-used">Datasets that use it</h2></div>
        ${using.length
          ? `<ul class="ont-used">${using.map((r) => {
              const terms = ontologyTerms(id, r.card);
              return `<li><a class="t" href="dataset.html?key=${enc(r.entry.key)}">${esc(r.entry.title || r.entry.key)}</a>${
                terms.length ? `<div class="ont-terms">${terms.map((t) => `<span>${esc(t)}</span>`).join("")}</div>` : ""
              }</li>`;
            }).join("")}</ul>`
          : `<p class="dz-empty">No dataset in the plaza uses this ontology yet.</p>`}
      </section>
    </div>`;

  if (hasSchema) {
    const g = document.getElementById("schemaGraph"), i = document.getElementById("schemaInfo");
    if (g && i) { try { mountSchemaUML(backingRec.card, g, i).catch(() => {}); } catch (_) {} }
  }
}
