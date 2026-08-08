// Every assertion here is COLLECTED, not thrown (see _expect.mjs): the gate
// runner reads the last JSON object this prints, so a check that dies on the
// first bad assertion tells CI nothing but a stack-trace tail. The counts below
// go stale routinely — the log has to name the number and show the new one.
import { expect } from "./_expect.mjs";

const t = expect("test_catalog_matrix");

let all = [], embedded = [], allGroups = [], embeddedGroups = [];
try {
  const { catalogCases, catalogDatasetGroups, normalizeCatalogScope } =
    await import("./catalog_matrix.mjs");

  t.equal("normalizeCatalogScope('embedded')", normalizeCatalogScope("embedded"), "embedded");
  t.equal("normalizeCatalogScope('all')", normalizeCatalogScope("all"), "all");
  t.throws("normalizeCatalogScope('remote')", () => normalizeCatalogScope("remote"), /embedded.*all/);

  all = catalogCases("all");
  embedded = catalogCases("embedded");

  // 637 -> 644 -> 651 -> 656 -> 664 -> 669 -> 676 -> 680: this literal has to be raised by hand every time
  // the catalog grows, and it silently went stale because the `browser` job was
  // SKIPPED on the PRs that grew it (#113 datacite/epfl-graph/opencitations,
  // #122 mirbase) and did not run on the pushes that grew it last (davidrumsey,
  // +7 queries and +1 dataset, landed while the `wasm` job was red so `browser`
  // never started). Keeping it is deliberate — it is the tripwire that makes a
  // catalog change visible — but the number is only ever as fresh as the last
  // run of this gate.
  t.equal("allQueries", all.length, 680, "every catalog query must be in the exhaustive matrix");
  t.equal("embeddedQueries", embedded.length, 69, "the deterministic tier must cover every embedded query");
  t.equal("uniqueCaseIds", new Set(all.map((entry) => entry.id)).size, all.length, "case ids must be unique");
  t.ok("everyCaseHasSparql", all.every((entry) => entry.query.trim()), "every case must carry executable SPARQL");
  t.ok("everyCaseHasIndex", all.every((entry) => Number.isInteger(entry.index)), "every case needs its catalog index");

  allGroups = catalogDatasetGroups("all");
  embeddedGroups = catalogDatasetGroups("embedded");
  t.equal("allDatasets", allGroups.length, 100, "the exhaustive tier must cover every dataset");
  t.equal("embeddedDatasets", embeddedGroups.length, 10, "the deterministic tier must cover every embedded dataset");
  t.deepEqual(
    "groupingOrder",
    allGroups.flatMap((group) => group.cases.map((entry) => entry.id)),
    all.map((entry) => entry.id),
    "grouping must neither drop nor reorder queries",
  );
} catch (error) {
  t.threw("catalog matrix", error);
}

t.finish({
  allQueries: all.length,
  allDatasets: allGroups.length,
  embeddedQueries: embedded.length,
  embeddedDatasets: embeddedGroups.length,
});
