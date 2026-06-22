// ontology.js — a proper page for one ontology/vocabulary. Hybrid model:
//   • metadata (name, description, homepage) + the terms seen across the plaza
//     and the datasets that use it, all EXTRACTED from the .rete cards;
//   • if a dataset PROVIDES this ontology as a full .rete (manifest `provides`),
//     its real schema is rendered (the UML diagram) and linked.
import { readReteCard, liteCardFromHeader } from "./rete-card.js";
import { usedOntologies, ontologyTerms, ontologyMeta } from "./vocabs.js";
import { mountSchemaUML } from "./schema-uml.js";

const root = document.getElementById("root");
const esc = (s) =>
  String(s == null ? "" : s).replace(/[&<>"]/g, (c) =>
    ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c])
  );
const httpsify = (u) => {
  const s = String(u || "");
  return /^http:\/\/(localhost|127\.0\.0\.1)/i.test(s) ? s : s.replace(/^http:\/\//i, "https://");
};
const enc = encodeURIComponent;

main();

async function main() {
  const id = new URLSearchParams(location.search).get("id");
  const manifest = await fetch("plaza.json").then((r) => r.json());
  if (!id) {
    root.innerHTML = `<div class="warnbox">No ontology specified. <a href="index.html">Back to the plaza.</a></div>`;
    return;
  }
  document.title = `rete plaza — ${id}`;

  // Read every dataset's card so we can find which use this ontology.
  const recs = await Promise.all(
    manifest.datasets.map(async (entry) => {
      try {
        const { header, card } = await readReteCard(entry.rete);
        return { entry, card: card || liteCardFromHeader(header, entry), header };
      } catch (_) {
        return { entry, card: { _unreachable: true }, header: null };
      }
    })
  );

  const using = recs.filter((r) => usedOntologies(r.card, r.entry).some((o) => o.name === id));

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

  // All terms of this ontology seen anywhere in the corpus.
  const termSet = new Set();
  for (const r of using) for (const t of ontologyTerms(id, r.card)) termSet.add(t);
  const termsAll = [...termSet].sort();

  root.innerHTML = `
    <div class="detail-hero" style="grid-template-columns:1fr">
      <div>
        <div class="modal-kicker">Ontology / vocabulary</div>
        <h1>${esc(id)}</h1>
        ${meta.desc ? `<div class="desc">${esc(meta.desc)}</div>` : ""}
        <div class="facts" style="margin-top:12px">
          <span class="pill">${using.length} dataset${using.length === 1 ? "" : "s"}</span>
          ${termsAll.length ? `<span class="pill">${termsAll.length} terms used</span>` : ""}
          ${backing ? `<span class="pill ok">full .rete available</span>` : ""}
        </div>
        ${meta.url ? `<div style="margin-top:10px"><a href="${esc(httpsify(meta.url))}" target="_blank" rel="noopener">${esc(meta.url)} ↗</a></div>` : ""}
        ${backing ? `<div style="margin-top:14px"><a class="run" style="text-decoration:none;display:inline-block" href="dataset.html?key=${enc(backing.key)}">Open the full ontology dataset — ${esc(backing.title || backing.key)} →</a></div>` : ""}
      </div>
    </div>

    ${backingRec ? `<div class="section"><h2>Schema</h2>
      <div class="schema-wrap">
        <div id="schemaGraph" class="schema-graph"></div>
        <div id="schemaInfo" class="schema-info"></div>
      </div>
      <div class="notice">The schema of the full <code>.rete</code> that provides this ontology (${esc(backing.title || backing.key)}). Boxes are classes, arrows are object properties — click a class to expand.</div>
    </div>` : ""}

    <div class="section"><h2>Terms used across the plaza</h2>
      ${termsAll.length
        ? `<div class="conns">${termsAll.map((t) => `<span class="conn">${esc(t)}</span>`).join("")}</div>`
        : `<div class="notice">No specific classes/properties of this ontology were detected in the dataset cards${backing ? " — see the full schema above" : ""}.</div>`}
    </div>

    <div class="section"><h2>Used in the following datasets</h2>
      ${using.length
        ? `<ul class="modal-used">${using.map((r) => {
            const terms = ontologyTerms(id, r.card);
            return `<li><a href="dataset.html?key=${enc(r.entry.key)}">${esc(r.entry.title || r.entry.key)}</a>${
              terms.length ? `<div class="modal-terms">${terms.map((t) => `<span>${esc(t)}</span>`).join("")}</div>` : ""
            }</li>`;
          }).join("")}</ul>`
        : `<div class="notice">No datasets in the catalogue use this ontology yet.</div>`}
    </div>`;

  if (backingRec) {
    const g = document.getElementById("schemaGraph"), i = document.getElementById("schemaInfo");
    if (g && i) { try { mountSchemaUML(backingRec.card, g, i).catch(() => {}); } catch (_) {} }
  }
}
