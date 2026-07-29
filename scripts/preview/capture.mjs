// Run every playground catalog example in a real browser and record its ANSWER.
//
// This is the data-gathering half of the social-preview pipeline: the cards that
// appear when someone shares a playground link show the real result — the same
// numbers a visitor gets when they press Run — so they have to be measured, not
// invented. The sweep reuses the regression gate's approach (open the built
// docs/playground.html, click the catalog's own example button, press Run) and
// additionally scrapes the rendered result table plus the `#qmeta` line.
//
// Output is append-only JSONL so a multi-hour sweep over live HTTP-range
// datasets can be resumed, re-run per dataset, and topped up incrementally:
//   web/preview/.cache/answers.jsonl   (cache, gitignored)
//   web/preview/answers.json           (consolidated, committed — see --finalize)
//
// Runs inside the Playwright image; see scripts/preview/run.sh.
//   node scripts/preview/capture.mjs [--scope=all|embedded] [--dataset=<substr>]
//                                    [--concurrency=4] [--timeout=90000] [--force]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { spawn } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
// ESM ignores NODE_PATH and resolves from the importing file, so reuse the one
// playwright install the repo already has (tests/gate) instead of a second tree.
const { chromium } = createRequire(path.join(ROOT, "tests", "gate", "package.json"))("playwright");
const CACHE_DIR = path.join(ROOT, "web", "preview", ".cache");
const JSONL = path.join(CACHE_DIR, "answers.jsonl");
// Committed, unlike the JSONL cache: the graph/map/timeline thumbnails are an
// INPUT to the card render, so a clean checkout has to be able to reproduce the
// PNGs byte-for-byte without re-running a multi-hour live sweep.
const SHOTS_DIR = path.join(ROOT, "web", "preview", "shots");

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit === undefined ? fallback : hit.slice(name.length + 3);
};
const SCOPE = flag("scope", "all");
const DATASET = flag("dataset", "");
const CONCURRENCY = Math.max(1, Number(flag("concurrency", 4)));
const TIMEOUT = Number(flag("timeout", 90000));
// Opening a multi-GB remote graph faults its dictionary directory over HTTP
// range before the first query can run — minutes on a cold cache, and it is
// paid once per dataset, not per example.
const OPEN_TIMEOUT = Number(flag("open-timeout", 180000));
const FORCE = args.includes("--force");
const FINALIZE_ONLY = args.includes("--finalize");
// Re-run only the drawing views, to refresh their result thumbnails without
// paying for the whole sweep again.
const SHOTS_ONLY = args.includes("--shots-only");
const PORT = Number(flag("port", 8099));
const READER = flag("reader", "default");
// Non-table views (graph / map / timeline / tiles) have no rows to redraw in the
// card, so the answer preview is a screenshot of the output panel instead.
const SHOT_VIEWS = new Set(["graph", "map", "time", "tiles", "cards"]);

const catalogSrc = fs.readFileSync(path.join(ROOT, "web", "playground-src", "catalog.js"), "utf8");
const catalogWindow = {};
new Function("window", catalogSrc)(catalogWindow);
const CATALOG = catalogWindow.RETE_PLAYGROUND_CATALOG;
const datasetByKey = new Map(CATALOG.datasets.map((d) => [d.key, d]));

/** Every catalog example as a flat work item, grouped per dataset below. */
function allCases() {
  const cases = [];
  for (const [key, examples] of Object.entries(CATALOG.examples)) {
    const dataset = datasetByKey.get(key);
    if (!dataset) throw new Error(`catalog examples refer to unknown dataset ${key}`);
    const remote = dataset.kind === "remote-lazy";
    if (SCOPE === "embedded" && remote) continue;
    if (DATASET && !key.includes(DATASET)) continue;
    for (const [index, example] of examples.entries()) {
      cases.push({
        id: `${key}:${index}`,
        dataset: key,
        index,
        remote,
        label: example.label || `example ${index}`,
        family: example.family || "",
        view: example.view || "table",
        tip: example.tip || "",
        q: example.q || "",
        // The catalog's own opt-out for examples whose point IS an empty answer
        // (a SHACL "no violations" check); those are captured, not retried.
        allowEmpty: !!example.allowEmpty,
      });
    }
  }
  return cases;
}

function groupByDataset(cases) {
  const groups = new Map();
  for (const entry of cases) {
    if (!groups.has(entry.dataset)) {
      groups.set(entry.dataset, { dataset: entry.dataset, remote: entry.remote, cases: [] });
    }
    groups.get(entry.dataset).cases.push(entry);
  }
  return [...groups.values()];
}

function readCache() {
  if (!fs.existsSync(JSONL)) return new Map();
  const seen = new Map();
  for (const line of fs.readFileSync(JSONL, "utf8").split("\n")) {
    if (!line.trim()) continue;
    try {
      const record = JSON.parse(line);
      // Later lines win — EXCEPT that a failure never supersedes an answer that
      // already worked. A re-run for another reason (refreshing the thumbnails,
      // say) hits live multi-gigabyte sources and can time out on a query that
      // succeeded an hour ago; without this, one flaky pass would silently strip
      // real answers off the cards.
      const previous = seen.get(record.id);
      if (!previous || record.ok || !previous.ok) seen.set(record.id, record);
    } catch { /* a truncated last line from an interrupted run */ }
  }
  return seen;
}

/** Consolidate the append-only cache into the committed, deterministic JSON. */
function finalize() {
  const seen = readCache();
  const answers = {};
  for (const entry of allCases()) {
    const record = seen.get(entry.id);
    if (record) answers[entry.id] = record;
  }
  const ordered = Object.keys(answers).sort().reduce((acc, k) => (acc[k] = answers[k], acc), {});
  const out = path.join(ROOT, "web", "preview", "answers.json");
  fs.mkdirSync(path.dirname(out), { recursive: true });
  fs.writeFileSync(out, JSON.stringify({
    // No timestamp: this file is committed, and a rebuild that changes nothing
    // should produce no diff.
    generator: "scripts/preview/capture.mjs",
    answers: ordered,
  }, null, 1) + "\n");
  const ok = Object.values(ordered).filter((a) => a.ok).length;
  console.log(`finalize: ${Object.keys(ordered).length} answers (${ok} with data) -> web/preview/answers.json`);
}

if (FINALIZE_ONLY) { finalize(); process.exit(0); }

fs.mkdirSync(CACHE_DIR, { recursive: true });
fs.mkdirSync(SHOTS_DIR, { recursive: true });
const cached = FORCE ? new Map() : readCache();
const jsonl = fs.createWriteStream(JSONL, { flags: "a" });
const write = (record) => new Promise((resolve) => jsonl.write(JSON.stringify(record) + "\n", resolve));

// ---------- the docs/ static server (Range-capable, same one the gate uses) ----------
async function serveDocs() {
  const child = spawn("node", [path.join(ROOT, "tests", "gate", "serve.mjs"), path.join(ROOT, "docs"), String(PORT)], {
    stdio: ["ignore", "pipe", "inherit"],
  });
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("docs server did not start")), 15000);
    child.stdout.on("data", (chunk) => {
      if (String(chunk).includes("serving")) { clearTimeout(timer); resolve(); }
    });
  });
  return child;
}

let documentSequence = 0;
function playgroundUrl(group) {
  const params = new URLSearchParams({ dataset: group.dataset, mode: "sparql" });
  if (group.remote) params.set("load", "lazy");
  // A fragment-only change does not reload the document and the playground has
  // no hashchange router, so vary the query string to force a clean document.
  return `http://127.0.0.1:${PORT}/playground.html?preview-capture=${group.dataset}-${++documentSequence}#${params}`;
}

async function openDataset(page, group) {
  await page.goto(playgroundUrl(group), { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    (count) => window.PlaygroundEditor
      && document.getElementById("run")
      && document.querySelectorAll("#examples [data-example]").length === count,
    group.cases.length,
    { timeout: OPEN_TIMEOUT },
  );
  await page.waitForTimeout(group.remote ? 300 : 1000);
}

async function waitForResult(page, timeout) {
  await page.waitForFunction(() => {
    const qmeta = (document.getElementById("qmeta") || {}).textContent || "";
    const run = (document.getElementById("run") || {}).textContent || "";
    const error = document.querySelector("#out .error-box");
    const busy = /querying/i.test(qmeta) || run.trim() === "Cancel";
    return !!error || (!!qmeta.trim() && !busy);
  }, undefined, { timeout });
}

/** Scrape the rendered answer: the meta line plus the head of the result table. */
async function scrape(page) {
  return page.evaluate(() => {
    const text = (el) => (el && el.textContent ? el.textContent.replace(/\s+/g, " ").trim() : "");
    const table = document.querySelector("#out table");
    let columns = [], rows = [];
    if (table) {
      columns = [...table.querySelectorAll("thead th")].map((th) => {
        const name = th.querySelector(".th-name");
        const bound = name && name.getAttribute("title");
        return {
          label: text(name) || text(th),
          var: bound && bound.startsWith("?") ? bound.slice(1) : "",
        };
      });
      rows = [...table.querySelectorAll("tbody tr")].slice(0, 8).map((tr) =>
        [...tr.querySelectorAll("td")].map((td) => ({
          text: text(td).slice(0, 160),
          media: !!td.querySelector("img, video, audio, model-viewer, iframe"),
        })),
      );
    }
    return {
      qmeta: text(document.getElementById("qmeta")),
      error: !!document.querySelector("#out .error-box"),
      errorText: text(document.querySelector("#out .err-advice, #out .err-tech-body, #out .error-box")).slice(0, 400),
      banner: text(document.querySelector("#out .banner")).slice(0, 300),
      columns,
      rows,
    };
  });
}

/**
 * The row count the card headlines. `#qmeta` is authoritative when it states one
 * ("1,204 rows in 3.2 s"); otherwise fall back to what the table actually shows.
 */
function parseCount(qmeta, rows) {
  const m = /^([\d,]+)\s+(row|triple|solution|card)/i.exec(qmeta || "");
  if (m) return { count: Number(m[1].replace(/,/g, "")), unit: m[2].toLowerCase() };
  if (rows && rows.length) return { count: rows.length, unit: "row" };
  return { count: null, unit: "" };
}

async function captureGroup(browser, group, progress) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  await context.addInitScript((reader) => {
    try {
      // No basemap tiles: the card must not depend on a third-party tile server,
      // and the gate pins the same so map results render deterministically.
      localStorage.setItem("mapBasemap", "none");
      // `--reader=sync` pins the reliable reader the gate uses. The default is
      // the browser's own (Asyncify on Chromium) — what a visitor actually gets,
      // and what the captured timings should therefore describe.
      if (reader === "sync") localStorage.setItem("asyncReadsOn", "0");
    } catch { /* private mode */ }
  }, READER);
  const page = await context.newPage();
  let pageErrors = [];
  page.on("pageerror", (e) => pageErrors.push(String(e).slice(0, 200)));

  try {
    await openDataset(page, group);
    for (const entry of group.cases) {
      pageErrors = [];
      const started = Date.now();
      let record;
      try {
        const selected = await page.evaluate((index) => {
          const button = document.querySelector(`#examples [data-example="${index}"]`);
          if (!button) return false;
          button.click();
          return true;
        }, entry.index);
        if (!selected) throw new Error("example button missing from the rendered library");
        await page.evaluate(() => document.getElementById("run").click());
        await waitForResult(page, TIMEOUT);
        const scraped = await scrape(page);
        const { count, unit } = parseCount(scraped.qmeta, scraped.rows);
        const ok = !scraped.error && !(count === 0 && !entry.allowEmpty) && !!scraped.qmeta;
        record = {
          id: entry.id, dataset: entry.dataset, index: entry.index, view: entry.view,
          ok,
          qmeta: scraped.qmeta,
          count, unit,
          columns: scraped.columns,
          rows: scraped.rows,
          banner: scraped.banner,
          error: scraped.error ? scraped.errorText : "",
          elapsedMs: Date.now() - started,
        };
        // A visual answer for the views that draw rather than tabulate. Shoot the
        // drawing itself, not the whole output panel: the panel's caption line
        // ("16 nodes | 30 edges") repeats what the card's rail already says, and
        // including it shrinks the picture that carries the meaning.
        if (ok && SHOT_VIEWS.has(entry.view)) {
          const shot = path.join(SHOTS_DIR, `${entry.id.replace(":", "-")}.png`);
          await page.waitForTimeout(entry.view === "graph" ? 2500 : 1200);
          const target = await page.$(
            "#out .graphwrap, #out .mapwrap, #out svg, #out .tgrid, #out .cards-grid, #out .tbl, #out",
          );
          if (target) {
            await target.screenshot({ path: shot }).then(
              () => { record.shot = path.relative(ROOT, shot).replace(/\\/g, "/"); },
              () => { /* an output panel scrolled out of view — card falls back to text */ },
            );
          }
        }
      } catch (e) {
        record = {
          id: entry.id, dataset: entry.dataset, index: entry.index, view: entry.view,
          ok: false, qmeta: "", count: null, unit: "", columns: [], rows: [],
          error: String((e && e.message) || e).slice(0, 300),
          elapsedMs: Date.now() - started,
        };
        // A timed-out remote query leaves a worker running; start the next case
        // from a fresh document so its Run cannot cancel the stale one.
        await openDataset(page, group).catch(() => {});
      }
      if (pageErrors.length && record.ok) record.pageErrors = pageErrors.slice(0, 3);
      await write(record);
      progress.done++;
      const mark = record.ok ? "ok  " : "MISS";
      console.log(`[${progress.done}/${progress.total}] ${mark} ${entry.id} — ${record.qmeta || record.error}`);
    }
  } finally {
    await context.close().catch(() => {});
  }
}

async function main() {
  const cases = allCases().filter((entry) => (SHOTS_ONLY
    ? SHOT_VIEWS.has(entry.view)
    : !cached.has(entry.id) || !cached.get(entry.id).ok));
  const groups = groupByDataset(cases);
  const total = cases.length;
  console.log(`capture: ${total} example(s) across ${groups.length} dataset(s) `
    + `(scope=${SCOPE}${DATASET ? `, dataset~${DATASET}` : ""}, concurrency=${CONCURRENCY})`);
  if (!total) { finalize(); return; }

  const server = await serveDocs();
  const browser = await chromium.launch();
  const progress = { done: 0, total };
  const queue = [...groups];
  try {
    const workers = Array.from({ length: Math.min(CONCURRENCY, queue.length) }, async () => {
      for (;;) {
        const group = queue.shift();
        if (!group) return;
        try {
          await captureGroup(browser, group, progress);
        } catch (e) {
          console.log(`  dataset ${group.dataset} aborted: ${String((e && e.message) || e).slice(0, 200)}`);
          for (const entry of group.cases) {
            await write({
              id: entry.id, dataset: entry.dataset, index: entry.index, view: entry.view,
              ok: false, qmeta: "", count: null, unit: "", columns: [], rows: [],
              error: `dataset open failed: ${String((e && e.message) || e).slice(0, 200)}`,
            });
            progress.done++;
          }
        }
      }
    });
    await Promise.all(workers);
  } finally {
    await browser.close().catch(() => {});
    server.kill();
    await new Promise((resolve) => jsonl.end(resolve));
  }
  finalize();
}

main().catch((e) => { console.error(e); process.exit(1); });
