// Generate a curated Dataset Card input for every EMBEDDED playground dataset,
// from the catalog the playground already shows.
//
// The embedded demo files were all built cardless, so the playground's own
// 🏷 Card button had nothing to show for them while every published dataset on
// the bucket had a card. The curated fields (title, description, licence,
// source, example queries) already existed — in catalog.js — so the card should
// be derived from there rather than written twice and left to drift.
//
// Usage: node scripts/gen_playground_cards.mjs [outDir]
//   outDir defaults to web/playground-cards/
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const OUT = process.argv[2] ? resolve(process.argv[2]) : join(ROOT, "web", "playground-cards");

globalThis.window = {};
createRequire(import.meta.url)(join(ROOT, "web", "playground-src", "catalog.js"));
const C = globalThis.window.RETE_PLAYGROUND_CATALOG;

// The keys build_playground.py embeds. Kept here rather than parsed out of the
// Python so a mismatch shows up as a missing file, not a silently skipped one.
const EMBEDDED = [
  "scholar", "scholar-noisy", "causal", "linked-jazz", "nomisma", "mimotext",
  "openalex-astrocytes", "antarctic-expeditions", "theographic-graph", "monarch",
];

// Labels read "<key>.rete - what it is"; the card has its own title field, so
// the filename prefix is noise there.
const titleOf = (key, label) => {
  const t = String(label || key).replace(/^[\w.-]+\.rete\s*[-–—]\s*/, "").trim();
  return t || key;
};

// A remote/lazy suffix in the label describes how the playground loads it, not
// what the dataset is — it does not belong in the file's own card.
const cleanTitle = (t) => t.replace(/\s*\((?:remote,\s*lazy|lazy|embedded)\)\s*$/i, "").trim();

let wrote = 0;
const missing = [];
mkdirSync(OUT, { recursive: true });

for (const key of EMBEDDED) {
  const entry = (C.datasets || []).find((d) => d.key === key);
  const meta = (C.datasetMeta || {})[key] || {};
  if (!entry) { missing.push(key); continue; }

  // Only the queries the catalog marks for this dataset, in catalog order, and
  // capped: the card is fetched in a couple of range reads, so a curated list
  // that grows without bound would work against the tier it lives in.
  const examples = ((C.examples || {})[key] || [])
    .map((e) => (e && e.q ? String(e.q).trim() : ""))
    .filter(Boolean)
    .slice(0, 8);

  const card = {
    title: cleanTitle(titleOf(key, entry.label)),
    description: String(entry.description || "").trim(),
    ...(meta.license ? { license: String(meta.license) } : {}),
    ...(meta.source ? { source: String(meta.source) } : {}),
    ...(examples.length ? { example_queries: examples } : {}),
  };
  if (!card.description) missing.push(`${key} (no description)`);

  writeFileSync(join(OUT, `${key}.card.json`), JSON.stringify(card, null, 2) + "\n");
  wrote++;
}

console.log(`gen_playground_cards: wrote ${wrote} card(s) to ${OUT}`);
if (missing.length) {
  console.error(`gen_playground_cards: incomplete — ${missing.join(", ")}`);
  process.exit(1);
}
