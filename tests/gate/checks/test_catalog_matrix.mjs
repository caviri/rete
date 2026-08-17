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

// 637 -> 644 -> 651 -> 656 -> 664 -> 669 -> 673: this literal has to be raised by hand every time the
// catalog grows, and it silently went stale because the `browser` job was
// SKIPPED on the PRs that grew it (#113 datacite/epfl-graph/opencitations,
// #122 mirbase). Keeping it is deliberate — it is the tripwire that makes a
// catalog change visible — but the number is only ever as fresh as the last
// run of this gate.
assert.equal(all.length, 673, "every catalog query must be in the exhaustive matrix");
assert.equal(embedded.length, 69, "the deterministic tier must cover every embedded query");
assert.equal(new Set(all.map((entry) => entry.id)).size, all.length, "case ids must be unique");
assert.ok(all.every((entry) => entry.query.trim()), "every case must carry executable SPARQL");
assert.ok(all.every((entry) => Number.isInteger(entry.index)), "every case needs its catalog index");

const allGroups = catalogDatasetGroups("all");
const embeddedGroups = catalogDatasetGroups("embedded");
assert.equal(allGroups.length, 99, "the exhaustive tier must cover every dataset");
assert.equal(embeddedGroups.length, 10, "the deterministic tier must cover every embedded dataset");
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
