// Execute the catalog's actual example-library flow in a real browser. This is
// deliberately an optional, slow tier: the complete matrix is 435 queries over
// 60 datasets, many of them live HTTP-range sources.
import fs from "node:fs";
import { launchBrowser, selectedBrowserName } from "./_browser.mjs";
import {
  catalogDatasetGroups,
  normalizeCatalogScope,
} from "./catalog_matrix.mjs";


const PORT = process.env.PGPORT || "8090";
const SCOPE = normalizeCatalogScope(process.env.RETE_CATALOG_SCOPE || "embedded");
const DATASET_FILTER = process.env.RETE_CATALOG_DATASET || "";
// The playground itself has NO query timeout, so the remote tier must tolerate
// slow-but-correct examples (an ORDER BY on a billion-triple file enumerates
// every match before the LIMIT — minutes, not seconds, over live HTTP range).
// Embedded stays tight: a local wasm query that needs 30 s IS a regression.
const QUERY_TIMEOUT_MS = Number(
  process.env.RETE_CATALOG_QUERY_TIMEOUT_MS || (SCOPE === "all" ? 300000 : 30000),
);
const REMOTE_TRIES = Number(process.env.RETE_CATALOG_RETRIES || 2);
// Which wasm reader the page runs under. "sync" (default) pins the reliable
// reader so an example failure is attributable to the example; "async" leaves
// the browser's real default (asyncify on Chromium) — the configuration users
// actually hit, where the orcid 0-rows regression lived invisible to the gate.
const READER = process.env.RETE_CATALOG_READER || "sync";
const browserName = selectedBrowserName();

let groups = catalogDatasetGroups(SCOPE);
if (DATASET_FILTER) {
  groups = groups.filter((group) => group.dataset.includes(DATASET_FILTER));
}
if (!groups.length) {
  throw new Error(`no ${SCOPE} catalog datasets match ${JSON.stringify(DATASET_FILTER)}`);
}

const failureDir = "/work/tests/gate/.cache/catalog-failures";
fs.mkdirSync(failureDir, { recursive: true });
let documentSequence = 0;

function failureScreenshot(entry) {
  const safe = entry.id.replace(/[^A-Za-z0-9_.-]/g, "_");
  return `${failureDir}/${browserName}-${safe}.png`;
}

for (const group of groups) {
  for (const entry of group.cases) fs.rmSync(failureScreenshot(entry), { force: true });
}

function playgroundUrl(group) {
  const params = new URLSearchParams({ dataset: group.dataset, mode: "sparql" });
  if (group.remote) params.set("load", "lazy");
  // A fragment-only change does not reload the document, and the playground
  // intentionally has no hashchange router. Vary the harmless query string so
  // each dataset starts with clean application/worker state.
  const loadId = `${group.dataset}-${++documentSequence}`;
  return `http://127.0.0.1:${PORT}/playground.html?catalog-gate=${encodeURIComponent(loadId)}`
    + `#${params}`;
}

async function waitForResult(page) {
  await page.waitForFunction(() => {
    const qmeta = (document.getElementById("qmeta") || {}).textContent || "";
    const run = (document.getElementById("run") || {}).textContent || "";
    const error = document.querySelector("#out .error-box");
    const stillQuerying = /querying/i.test(qmeta) || run.trim() === "Cancel";
    return !!error || (!!qmeta.trim() && !stillQuerying);
  }, undefined, { timeout: QUERY_TIMEOUT_MS });
  return page.evaluate(() => ({
    qmeta: ((document.getElementById("qmeta") || {}).textContent || "").trim(),
    error: !!document.querySelector("#out .error-box"),
    errorText: (
      document.querySelector("#out .err-advice, #out .err-tech-body, #out .error-box") || {}
    ).textContent || "",
    runText: ((document.getElementById("run") || {}).textContent || "").trim(),
  }));
}

async function openDataset(page, group) {
  await page.goto(playgroundUrl(group), { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    (count) => window.PlaygroundEditor
      && document.getElementById("run")
      && document.querySelectorAll("#examples [data-example]").length === count,
    // Every example the catalog defines, not the subset being run: the page
    // draws a button for a `skipCapture` example like any other. See
    // catalogDatasetGroups().
    group.rendered,
    // Remote-lazy opens of multi-GB files fetch dictionary directories over
    // live HTTP range — allow the slow-network case instead of flaking.
    { timeout: 120000 },
  );
  // The embedded graph initialization is asynchronous. Remote datasets defer
  // opening until Run, but this small settle also lets their library render.
  await page.waitForTimeout(group.remote ? 250 : 1000);
}

async function selectAndRun(page, entry) {
  const selected = await page.evaluate(({ index, query }) => {
    const button = document.querySelector(`#examples [data-example="${index}"]`);
    if (!button) return { found: false, queryMatches: false };
    button.click();
    const text = window.PlaygroundEditor.getText
      ? window.PlaygroundEditor.getText("q")
      : (document.getElementById("q") || {}).value || "";
    return { found: true, queryMatches: text === query };
  }, entry);
  if (!selected.found) throw new Error("example button is missing from the rendered library");
  if (!selected.queryMatches) throw new Error("rendered editor text differs from the catalog query");
  await page.evaluate(() => document.getElementById("run").click());
  return waitForResult(page);
}

async function main() {
  const browser = await launchBrowser();
  const context = await browser.newContext();
  // The normal gate separately tests Asyncify. The default sweep pins the
  // reliable reader so a reader fallback cannot hide which example failed;
  // RETE_CATALOG_READER=async runs the browser's real defaults instead.
  await context.addInitScript((reader) => {
    try {
      if (reader === "sync") localStorage.setItem("asyncReadsOn", "0");
      localStorage.setItem("mapBasemap", "none");
    } catch (_error) { /* private mode */ }
  }, READER);
  const page = await context.newPage();
  const failures = [];
  let passed = 0;
  let skipped = 0;
  let pageErrors = [];
  page.on("pageerror", (error) => pageErrors.push(String(error).slice(0, 240)));

  try {
    for (const group of groups) {
      try {
        await openDataset(page, group);
      } catch (error) {
        throw new Error(`opening dataset ${group.dataset}: ${error && error.message || error}`);
      }
      console.log(`\n[${browserName}] ${group.dataset} (${group.cases.length} queries)`);
      for (const entry of group.cases) {
        // A justified exclusion, with the reason in the flag (#212). Running it
        // does not measure the example, it measures how long we were willing to
        // wait — and check_catalog_answers.mjs already holds these to the other
        // half of the bargain: one that turns out to answer fails as stale.
        if (entry.skipCapture) {
          skipped++;
          console.log(`  SKIP ${entry.id} ${entry.label} — ${entry.skipCapture.slice(0, 100)}`);
          continue;
        }
        let lastFailure = null;
        const tries = group.remote ? REMOTE_TRIES : 1;
        for (let attempt = 1; attempt <= tries; attempt++) {
          if (attempt > 1) await openDataset(page, group);
          pageErrors = [];
          try {
            const result = await selectAndRun(page, entry);
            const limit = /cancelled|switched readers|browser's limit|browser limit/i.test(result.qmeta);
            // Catalog examples are CURATED — every one must produce data. A clean
            // "0 row(s)" is a failure (the orcid async regression returned empty
            // results with no error box and sailed through this gate as green).
            // Only qmeta strings that state a count are judged; other views
            // (e.g. serializations) keep the error-box-only contract.
            const counted = result.qmeta.match(/^(\d[\d,]*)\s+(row|triple|solution)/i);
            const empty = counted && Number(counted[1].replace(/,/g, "")) === 0 && !entry.allowEmpty;
            if (result.error || limit || empty || pageErrors.length) {
              lastFailure = {
                ...entry,
                attempt,
                qmeta: result.qmeta,
                error: (empty ? "example returned 0 rows (curated examples must produce data)"
                              : result.errorText.trim()).slice(0, 500),
                pageErrors: [...pageErrors],
              };
            } else {
              lastFailure = null;
              passed++;
              console.log(`  PASS ${entry.id} ${entry.label} — ${result.qmeta}`);
              break;
            }
          } catch (error) {
            lastFailure = {
              ...entry,
              attempt,
              error: String(error && error.message || error).slice(0, 500),
              pageErrors: [...pageErrors],
            };
          }
          console.log(`  RETRY ${entry.id} attempt ${attempt}/${tries}: ${lastFailure.error || lastFailure.qmeta}`);
        }
        if (lastFailure) {
          failures.push(lastFailure);
          await page.screenshot({ path: failureScreenshot(entry), fullPage: true })
            .catch(() => {});
          console.log(`  FAIL ${entry.id} ${entry.label}: ${lastFailure.error || lastFailure.qmeta}`);
          // A timeout can leave the remote worker running. Start the next case
          // from a fresh document so its Run click cannot cancel the old query
          // and create a cascade of misleading "cancelled" failures.
          await openDataset(page, group);
        }
      }
    }
  } finally {
    await browser.close();
  }

  const total = groups.reduce((sum, group) => sum + group.cases.length, 0);
  const verdict = failures.length ? "FAIL" : "PASS";
  const report = {
    verdict,
    scope: SCOPE,
    browser: browserName,
    datasets: groups.length,
    queries: total,
    passed,
    // Excluded by a `skipCapture` reason in the catalog, not by this run.
    skipped,
    failures,
  };
  const reportFilter = DATASET_FILTER
    ? `-${DATASET_FILTER.replace(/[^A-Za-z0-9_.-]/g, "_")}`
    : "";
  const reportPath = `/work/tests/gate/.cache/catalog-report-${browserName}-${SCOPE}${reportFilter}.json`;
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify({ ...report, reportPath }, null, 2));
  process.exit(failures.length ? 1 : 0);
}

main().catch((error) => {
  const report = {
    verdict: "FAIL",
    scope: SCOPE,
    browser: browserName,
    fatal: String(error && error.stack || error),
  };
  const reportFilter = DATASET_FILTER
    ? `-${DATASET_FILTER.replace(/[^A-Za-z0-9_.-]/g, "_")}`
    : "";
  const reportPath = `/work/tests/gate/.cache/catalog-report-${browserName}-${SCOPE}${reportFilter}.json`;
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2) + "\n");
  console.log(JSON.stringify({ ...report, reportPath }, null, 2));
  process.exit(1);
});
