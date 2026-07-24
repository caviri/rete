import assert from "node:assert/strict";
import {
  catalogCases,
  catalogDatasetGroups,
  normalizeCatalogScope,
} from "./catalog_matrix.mjs";

assert.equal(normalizeCatalogScope("embedded"), "embedded");
assert.equal(normalizeCatalogScope("all"), "all");
assert.throws(() => normalizeCatalogScope("remote"), /embedded.*all/);

const all = catalogCases("all");
const embedded = catalogCases("embedded");

assert.equal(all.length, 558, "every catalog query must be in the exhaustive matrix");
assert.equal(embedded.length, 73, "the deterministic tier must cover every embedded query");
assert.equal(new Set(all.map((entry) => entry.id)).size, all.length, "case ids must be unique");
assert.ok(all.every((entry) => entry.query.trim()), "every case must carry executable SPARQL");
assert.ok(all.every((entry) => Number.isInteger(entry.index)), "every case needs its catalog index");

const allGroups = catalogDatasetGroups("all");
const embeddedGroups = catalogDatasetGroups("embedded");
assert.equal(allGroups.length, 79, "the exhaustive tier must cover every dataset");
assert.equal(embeddedGroups.length, 11, "the deterministic tier must cover every embedded dataset");
assert.deepEqual(
  allGroups.flatMap((group) => group.cases.map((entry) => entry.id)),
  all.map((entry) => entry.id),
  "grouping must neither drop nor reorder queries",
);

console.log(JSON.stringify({
  verdict: "PASS",
  allQueries: all.length,
  allDatasets: allGroups.length,
  embeddedQueries: embedded.length,
  embeddedDatasets: embeddedGroups.length,
}));
