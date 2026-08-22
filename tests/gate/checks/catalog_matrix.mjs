import fs from "node:fs";


const ROOT = new URL("../../../", import.meta.url);
const source = fs.readFileSync(new URL("web/playground-src/catalog.js", ROOT), "utf8");
const window = {};
new Function("window", source)(window);
const catalog = window.RETE_PLAYGROUND_CATALOG;

if (!catalog || !Array.isArray(catalog.datasets) || !catalog.examples) {
  throw new Error("playground catalog did not expose datasets and examples");
}

const datasetByKey = new Map(catalog.datasets.map((dataset) => [dataset.key, dataset]));

export function normalizeCatalogScope(scope) {
  const normalized = String(scope || "embedded").toLowerCase();
  if (normalized !== "embedded" && normalized !== "all") {
    throw new Error(`catalog scope must be "embedded" or "all", got ${JSON.stringify(scope)}`);
  }
  return normalized;
}

export function catalogCases(scope = "embedded") {
  const normalized = normalizeCatalogScope(scope);
  const cases = [];
  for (const [datasetKey, examples] of Object.entries(catalog.examples)) {
    const dataset = datasetByKey.get(datasetKey);
    if (!dataset) throw new Error(`catalog examples refer to unknown dataset ${datasetKey}`);
    const remote = dataset.kind === "remote-lazy";
    if (normalized === "embedded" && remote) continue;
    for (const [index, example] of examples.entries()) {
      cases.push({
        id: `${datasetKey}:${index}`,
        dataset: datasetKey,
        index,
        label: example.label || `example ${index}`,
        query: example.q || "",
        view: example.view || "table",
        strategy: example.strategy || "whole",
        reason: example.reason,
        fed: example.fed || [],
        // Opt-out for the rows>0 assertion: a curated example that legitimately
        // returns an empty result (e.g. a SHACL "no violations" check) sets
        // allowEmpty: true in the catalog.
        allowEmpty: !!example.allowEmpty,
        // A justified exclusion from every automated sweep (#212): an example
        // whose cost the harness cannot pay. The reason is the flag's value, and
        // check_catalog_answers.mjs fails if one ever turns out to answer after
        // all, so the exclusion cannot outlive its reason.
        skipCapture: String(example.skipCapture || "").trim(),
        remote,
      });
    }
  }
  return cases;
}

/**
 * Cases grouped per dataset, plus `rendered` — how many example buttons the PAGE
 * will draw for that dataset.
 *
 * The grouping stays a faithful, total projection of the catalog: it drops
 * nothing and reorders nothing (test_catalog_matrix asserts exactly that), so a
 * `skipCapture` example is still in `cases` and a caller that must not RUN it
 * skips it there. Only the runner knows what running costs.
 *
 * `rendered` exists because it is NOT interchangeable with `cases.length` for
 * every caller. scripts/preview/capture.mjs measures a filtered subset on any
 * resume, and waiting for the example library to render exactly as many buttons
 * as that subset is not a slow wait, it is an impossible one — see openDataset()
 * there for the 39 phantom "dataset open failed" records it cost (#212). Here
 * the two happen to be equal; the name says which one you meant.
 */
export function catalogDatasetGroups(scope = "embedded") {
  const groups = [];
  for (const entry of catalogCases(scope)) {
    let group = groups.at(-1);
    if (!group || group.dataset !== entry.dataset) {
      group = { dataset: entry.dataset, remote: entry.remote, rendered: 0, cases: [] };
      groups.push(group);
    }
    group.rendered++;
    group.cases.push(entry);
  }
  return groups;
}
