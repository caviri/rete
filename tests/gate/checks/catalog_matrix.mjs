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
        remote,
      });
    }
  }
  return cases;
}

export function catalogDatasetGroups(scope = "embedded") {
  const groups = [];
  for (const entry of catalogCases(scope)) {
    let group = groups.at(-1);
    if (!group || group.dataset !== entry.dataset) {
      group = { dataset: entry.dataset, remote: entry.remote, cases: [] };
      groups.push(group);
    }
    group.cases.push(entry);
  }
  return groups;
}
