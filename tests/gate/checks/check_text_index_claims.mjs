// G0 — the playground may not advertise a full-text index a published file does
// not have (and may not hide one it does).
//
// A `.rete` TEXT_INDEX (section kind 6) is OPT-IN at build time (`rete build
// --text-index`). Nothing used to tie the catalog's prose to the bytes on the
// wire, so two datasets (`boe`, `memoria`) advertised an index their published
// files never carried — `FILTER(CONTAINS(…))` still answered, by full scan, so
// the drift was invisible — while the catalog's largest index (causenet, 1.88 GB)
// went unmentioned.
//
// The fix is a machine-readable declaration: `textIndex: true` on the dataset
// entry. This check enforces the OFFLINE half of the contract — the flag and the
// prose must agree, in both directions, on every surface a reader sees:
//
//   flag === true   ⇒ at least one surface states it, and none denies it
//   flag !== true   ⇒ no surface states it (a denial is fine, and encouraged)
//
// The NETWORK half — flag vs the TEXT_INDEX section actually present in the
// published object — is `scripts/check_dataset_catalog.py`, which already
// range-reads all 1024 header bytes of every catalog target and runs weekly in
// `.github/workflows/catalog.yml`. Deliberately NOT here: the gate must not need
// ~100 network round-trips to go green.
//
// Assertions go through `_expect.mjs` (#186): the runner reads the last JSON
// verdict, never the exit code, so a check that throws reports a stack trace
// instead of naming the dataset that drifted — which is the one fact this check
// exists to produce.
import fs from "node:fs";
import { expect } from "./_expect.mjs";

const t = expect("check_text_index_claims");

const ROOT = new URL("../../../", import.meta.url);
let catalog = null;
try {
  const source = fs.readFileSync(new URL("web/playground-src/catalog.js", ROOT), "utf8");
  const window = {};
  new Function("window", source)(window);
  catalog = window.RETE_PLAYGROUND_CATALOG;
  if (!catalog || !Array.isArray(catalog.datasets)) {
    throw new Error("catalog.js did not expose RETE_PLAYGROUND_CATALOG.datasets");
  }
} catch (error) {
  t.threw("load catalog.js", error);
  t.finish({ datasets: 0, declaringTextIndex: 0, surfacesScanned: 0 });
  process.exit(1);
}

// A CLAIM, not a passing mention: "its full-text URL" and "infs:fullText" are
// about the source PDFs, not about an index, and must not match.
const CLAIM =
  /(TEXT_INDEX|--text-index|\btext[- ]index\b|(?:full[- ]?text|word)[- ]?(?:index|indexes|indexed|indexing|search|searchable|searching))/i;
// …and a sentence that DENIES one ("carries no TEXT_INDEX", "built without
// --text-index") is a disclaimer, not a claim. Scoped to the same sentence and
// to the neighbourhood of the match so an unrelated "no" elsewhere is ignored.
const NEGATION = /\b(no|not|never|without|none|lacks|lacking)\b/i;
const NEG_WINDOW = 90;

function classify(text) {
  const claims = [];
  const disclaimers = [];
  for (const sentence of String(text || "").split(/(?<=[.!?])[\s]+/)) {
    const m = CLAIM.exec(sentence);
    if (!m) continue;
    const near = sentence.slice(Math.max(0, m.index - NEG_WINDOW), m.index + m[0].length + NEG_WINDOW);
    (NEGATION.test(near) ? disclaimers : claims).push(sentence.trim());
  }
  return { claims, disclaimers };
}

const meta = catalog.datasetMeta || {};
const extra = catalog.datasetExtra || {};
let flagged = 0;
let surfaces = 0;

for (const dataset of catalog.datasets) {
  const key = dataset.key;
  const flag = dataset.textIndex === true;
  if (flag) flagged++;
  const read = {
    label: dataset.label,
    description: dataset.description,
    provenance: (meta[key] || {}).provenance,
    tags: ((extra[key] || {}).tags || []).join(" · "),
  };
  const claims = [];
  const disclaimers = [];
  for (const [name, text] of Object.entries(read)) {
    if (!text) continue;
    surfaces++;
    const c = classify(text);
    claims.push(...c.claims.map((s) => `${name}: ${s}`));
    disclaimers.push(...c.disclaimers.map((s) => `${name}: ${s}`));
  }
  // The failure line has to name the DATASET and the DIRECTION of the drift —
  // "boe claims one, flag says no" — because that is the whole repair
  // instruction. `check` is the dataset key, so it leads every rendering.
  const state = claims.length ? "claims an index" : disclaimers.length ? "denies an index" : "silent";
  const observed = `${state} (textIndex: ${flag})`;
  if (flag && claims.length === 0 && disclaimers.length === 0) {
    t.equal(key, observed, `claims an index (textIndex: ${flag})`,
      "the published file HAS a full-text index and nothing tells the reader — say so in the description");
  } else if (flag && disclaimers.length > 0) {
    t.equal(key, observed, `claims an index (textIndex: ${flag})`,
      `textIndex: true but a surface denies it — ${disclaimers[0].slice(0, 160)}`);
  } else if (!flag && claims.length > 0) {
    t.equal(key, observed, `silent (textIndex: ${flag})`,
      "advertises a full-text index the entry does not declare — either the published file has one"
        + ` (add textIndex: true) or the sentence is false (drop it) — ${claims[0].slice(0, 160)}`);
  }
}

t.finish({
  datasets: catalog.datasets.length,
  declaringTextIndex: flagged,
  surfacesScanned: surfaces,
});
