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
// SAFETY CONTRACT. answers.json is committed and expensive (hours of live
// queries); the cache it is consolidated from is gitignored, so on a clean
// checkout it does not exist. Finalize is therefore ADDITIVE, never a
// replacement: it seeds from the committed answers.json, merges the cache over
// it, keeps the whole catalog in view regardless of --scope/--dataset, and
// refuses to write a file with fewer answers than the one already committed.
// See finalize() for the three guards and the flags that lift them.
//
// Runs inside the Playwright image; see scripts/preview/run.sh.
//   node scripts/preview/capture.mjs [--scope=all|embedded] [--dataset=<substr>]
//                                    [--concurrency=4] [--timeout=90000] [--force]
//   node scripts/preview/capture.mjs --finalize [--rebuild] [--allow-shrink]
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
const ANSWERS = path.join(ROOT, "web", "preview", "answers.json");

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit === undefined ? fallback : hit.slice(name.length + 3);
};
const SCOPE = flag("scope", "all");
const DATASET = flag("dataset", "");
const CONCURRENCY = Math.max(1, Number(flag("concurrency", 4)));
// How long ONE example may take, split by what it is querying — because a single
// number cannot be right for both halves of this catalog (#212).
//
// The playground itself imposes NO query timeout: a visitor who presses Run on
// `databnf:3` (predicate totals over 673.5M triples) or `gbif-birds:7` (4,000
// GeoSPARQL points out of 1.43 GB) waits, and gets an answer. Every timeout here
// is therefore a statement about the HARNESS's patience, never about whether the
// example works, and recording one as a failure is recording our own budget.
//
// 90 s was the single budget for both tiers, and against the remote tier it was
// simply wrong — `check_catalog_examples.mjs`, which runs the same examples in
// the same browser, has always allowed 300 s for exactly this reason ("an ORDER
// BY on a billion-triple file enumerates every match before the LIMIT — minutes,
// not seconds, over live HTTP range").
//
// 600 s, not 300 s, and the extra is a measurement rather than a hunch. Of the
// examples that answer, these are the four slowest, all of them correct:
//
//     crossref:4              295.9 s   50 rows, 706 range req, 474 MB of 56.09 GB
//     boe:6                   256.4 s   87 triples (CONSTRUCT, reasoning on)
//     ror:4                   193.7 s   50 rows, cross-source join into 17.5 GB of ORCID
//     gbif-birds:7            131.5 s   4,000 GeoSPARQL points out of 1.43 GB
//
// A budget of 300 s clears the slowest of those by four seconds, which is not a
// budget, it is a coin toss — and losing the toss does not read as "slow", it
// reads as a dead example in answers.json. Doubling it costs nothing on a green
// run (a passing example returns when it returns) and buys the sweep a 2× margin
// over the worst answer anyone has measured.
//
// Embedded stays at 90 s deliberately: those graphs are in memory, and a local
// wasm query that needs a minute and a half IS a regression worth failing on.
const TIMEOUT = Number(flag("timeout", 90000));
const REMOTE_TIMEOUT = Number(flag("remote-timeout", 600000));
// Opening a multi-GB remote graph faults its dictionary directory over HTTP
// range before the first query can run — minutes on a cold cache, and it is
// paid once per dataset, not per example.
const OPEN_TIMEOUT = Number(flag("open-timeout", 180000));
const FORCE = args.includes("--force");
const FINALIZE_ONLY = args.includes("--finalize");
// Deliberately discard the committed answers and consolidate the cache alone —
// the only way to record that an example that used to work no longer does. It
// still refuses to shrink the file without --allow-shrink, so a --rebuild run
// against a half-populated cache stops instead of deleting the other answers.
const REBUILD = args.includes("--rebuild");
// Permit an output with fewer answers than the committed file: needed when a
// catalog example is genuinely deleted, and paired with --rebuild to re-record
// regressions. Never a default.
const ALLOW_SHRINK = args.includes("--allow-shrink");
// Re-run only the drawing views, to refresh their result thumbnails without
// paying for the whole sweep again.
const SHOTS_ONLY = args.includes("--shots-only");
const PORT = Number(flag("port", 8099));
const READER = flag("reader", "default");
// Non-table views (graph / map / timeline / tiles) have no rows to redraw in the
// card, so the answer preview is a screenshot of the output panel instead.
const SHOT_VIEWS = new Set(["graph", "map", "time", "tiles", "cards"]);

// The playground states in `#qmeta` when it stopped rather than answered — the
// tab hit its WebAssembly memory/stack ceiling, the worker was cancelled, or the
// reader was swapped mid-flight. That is NOT an answer, and recording it as one
// had two consequences worth naming: `capture` skipped the example on the next
// resume (it only re-runs entries whose cached record is not ok), and finalize
// let it OVERWRITE a good committed answer, since a failure may only supersede
// another failure. `check_catalog_examples.mjs` has always judged these the same
// way; this is the same list.
const GAVE_UP = /cancelled|switched readers|browser'?s limit|browser limit/i;

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
      // A deliberate, justified exclusion from the sweep (#212). The flag's
      // VALUE is the justification — an example whose cost the harness cannot
      // pay, like a FILTER over 223,082 inline WebP literals on a 25.4 GB graph,
      // which does not time out so much as decline to finish. Running it anyway
      // does not measure anything; it just spends the budget and records the
      // budget back. check_catalog_answers.mjs accepts the missing entry, and
      // fails if one of these ever turns up with a good answer after all.
      if (String(example.skipCapture || "").trim()) continue;
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

/**
 * Every example id in the catalog, ignoring --scope and --dataset.
 *
 * finalize() must use THIS and not allCases(): allCases() honours the filters,
 * so consolidating at the end of `capture --dataset=x` used to emit a file
 * containing x and nothing else — deleting every other dataset's answer even
 * when the cache still held them.
 */
function catalogIds() {
  const ids = [];
  for (const [key, examples] of Object.entries(CATALOG.examples)) {
    if (!datasetByKey.has(key)) throw new Error(`catalog examples refer to unknown dataset ${key}`);
    for (let index = 0; index < examples.length; index++) ids.push(`${key}:${index}`);
  }
  return ids;
}

/** The committed answers — the base every finalize builds on, and the floor it may not go under. */
function readCommitted() {
  if (!fs.existsSync(ANSWERS)) return {};
  // Deliberately not tolerant: silently treating an unreadable committed file as
  // empty would disable the shrink guard exactly when it matters most.
  return JSON.parse(fs.readFileSync(ANSWERS, "utf8")).answers || {};
}

function groupByDataset(cases) {
  const groups = new Map();
  for (const entry of cases) {
    if (!groups.has(entry.dataset)) {
      groups.set(entry.dataset, {
        dataset: entry.dataset,
        remote: entry.remote,
        // How many example buttons the PAGE will render for this dataset — the
        // whole catalog list, which is NOT `cases.length` once anything has been
        // filtered out. See openDataset() for what conflating the two cost.
        rendered: CATALOG.examples[entry.dataset].length,
        cases: [],
      });
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

/**
 * Merge the append-only cache into the committed, deterministic JSON.
 *
 * Additive by construction. The committed answers.json is the base; cache
 * records are laid over it; the key set is the WHOLE catalog. So a partial
 * capture — one dataset, one scope, a clean checkout with no cache at all — can
 * only add or update entries, never remove them.
 *
 * Three guards, in order of how much they cost to get wrong:
 *   1. base = committed answers (skip with --rebuild),
 *   2. a cached FAILURE never supersedes an answer that already worked — the
 *      same rule readCache() applies inside the cache, extended to the file,
 *   3. an output smaller than the committed file aborts unless --allow-shrink.
 */
function finalize() {
  const committed = readCommitted();
  const seen = readCache();
  const base = REBUILD ? {} : committed;
  const answers = {};
  for (const id of catalogIds()) {
    const previous = base[id];
    const fresh = seen.get(id);
    // Guard 2: prefer what was just measured, unless it is a failure standing
    // against an answer that worked (a flaky remote sweep must not strip data).
    const pick = fresh && (fresh.ok || !previous || !previous.ok) ? fresh : previous;
    if (pick) answers[id] = pick;
  }
  const ordered = Object.keys(answers).sort().reduce((acc, k) => (acc[k] = answers[k], acc), {});

  // Guard 3. Entries can still legitimately disappear — an example deleted from
  // the catalog is no longer addressable — but never by accident and never
  // silently, because re-measuring one costs a live query against a multi-GB
  // remote graph.
  const before = Object.keys(committed).length;
  const after = Object.keys(ordered).length;
  const okBefore = Object.values(committed).filter((a) => a.ok).length;
  const okAfter = Object.values(ordered).filter((a) => a.ok).length;
  if (!ALLOW_SHRINK && (after < before || okAfter < okBefore)) {
    const gone = Object.keys(committed).filter((id) => !(id in ordered));
    const inCatalog = new Set(catalogIds());
    const droppedButStillInCatalog = gone.filter((id) => inCatalog.has(id));
    console.error(
      `finalize refused: this would shrink web/preview/answers.json.\n`
      + `  committed: ${before} answers (${okBefore} with data)\n`
      + `  would write: ${after} answers (${okAfter} with data)  `
      + `— dropping ${before - after} entr${before - after === 1 ? "y" : "ies"}`
      + `${okBefore - okAfter > 0 ? `, ${okBefore - okAfter} of them measured answers` : ""}\n`
      + (gone.length ? `  first dropped: ${gone.slice(0, 8).join(", ")}${gone.length > 8 ? ` … (+${gone.length - 8})` : ""}\n` : "")
      + (droppedButStillInCatalog.length
        ? `  ${droppedButStillInCatalog.length} of those are STILL in the catalog — this is data loss, not pruning.\n`
        : `  (all dropped ids have left the catalog)\n`)
      + `  Nothing was written. If the loss is intended, re-run with --allow-shrink:\n`
      + `    scripts/preview/run.sh finalize --allow-shrink\n`
      + `  To reconsolidate from the cache alone, add --rebuild.`,
    );
    process.exit(1);
  }

  fs.mkdirSync(path.dirname(ANSWERS), { recursive: true });
  fs.writeFileSync(ANSWERS, JSON.stringify({
    // No timestamp: this file is committed, and a rebuild that changes nothing
    // should produce no diff.
    generator: "scripts/preview/capture.mjs",
    answers: ordered,
  }, null, 1) + "\n");
  const delta = after - before;
  console.log(`finalize: ${after} answers (${okAfter} with data) -> web/preview/answers.json`
    + ` [was ${before} (${okBefore}); ${delta === 0 ? "no net change" : `${delta > 0 ? "+" : ""}${delta}`}`
    + `${REBUILD ? ", --rebuild" : ""}${ALLOW_SHRINK ? ", --allow-shrink" : ""}]`);
}

if (FINALIZE_ONLY) { finalize(); process.exit(0); }

/**
 * Give a clean checkout the cache it would have had.
 *
 * The cache is gitignored, so a fresh clone starts with none — and every record
 * in the committed answers.json came out of exactly this cache, in exactly this
 * shape. Seeding it back makes the first run on a clone incremental, like a run
 * on the machine that did the original sweep: only the missing and failed
 * examples are measured. `--force` still re-measures everything.
 */
function seedCacheFromCommitted() {
  if (fs.existsSync(JSONL)) return;
  const committed = readCommitted();
  const ids = Object.keys(committed).sort();
  if (!ids.length) return;
  fs.writeFileSync(JSONL, ids.map((id) => JSON.stringify({ ...committed[id], id })).join("\n") + "\n");
  console.log(`seed: no cache on disk — seeded ${ids.length} answer(s) from the committed `
    + `web/preview/answers.json, so this run tops them up instead of replacing them `
    + `(--force to re-measure everything).`);
}

fs.mkdirSync(CACHE_DIR, { recursive: true });
fs.mkdirSync(SHOTS_DIR, { recursive: true });
seedCacheFromCommitted();
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

/**
 * Wait until the playground has rendered this dataset's whole example library.
 *
 * The count to wait for is `group.rendered` — every example the CATALOG defines
 * for the dataset — and never `group.cases.length`, which is the subset this run
 * happens to be measuring.
 *
 * Getting that wrong is not a slow path, it is an unsatisfiable one, and it cost
 * 39 of the 52 dead entries in the committed answers.json (#212). A resume only
 * re-runs examples that are missing or failed, so `cases.length` is smaller than
 * the rendered count for any dataset that is not being measured whole: the page
 * renders gbif-birds' 21 buttons, the predicate demands exactly 9, and it waits
 * for the full budget and throws. The failure then arrives at the group-level
 * catch, which records `dataset open failed: page.waitForFunction: Timeout …`
 * for every case in the group — overwriting whatever the real, per-example error
 * had been, because a failure may supersede a failure. So the harness both
 * invented a dataset-wide open timeout and destroyed the evidence of what had
 * actually gone wrong, and the number in the message was only ever the budget
 * someone last passed on the command line (240 s, 300 s, 420 s), never a
 * measurement of anything.
 */
async function openDataset(page, group) {
  await page.goto(playgroundUrl(group), { waitUntil: "domcontentloaded" });
  await page.waitForFunction(
    (count) => window.PlaygroundEditor
      && document.getElementById("run")
      && document.querySelectorAll("#examples [data-example]").length === count,
    group.rendered,
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
        await waitForResult(page, group.remote ? REMOTE_TIMEOUT : TIMEOUT);
        const scraped = await scrape(page);
        const { count, unit } = parseCount(scraped.qmeta, scraped.rows);
        const ok = !scraped.error && !(count === 0 && !entry.allowEmpty) && !!scraped.qmeta
          && !GAVE_UP.test(scraped.qmeta);
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
        // Say WHOSE limit was hit. "Timeout 90000ms exceeded" reads as a property
        // of the query; it is a property of this sweep, and the distinction is
        // the whole of #212.
        const raw = String((e && e.message) || e);
        const budget = group.remote ? REMOTE_TIMEOUT : TIMEOUT;
        const message = /waitForFunction: Timeout/.test(raw)
          ? `the capture gave up after ${Math.round(budget / 1000)} s — this is the harness budget `
            + `(--${group.remote ? "remote-" : ""}timeout), not a playground limit; the query may still be correct: ${raw}`
          : raw;
        record = {
          id: entry.id, dataset: entry.dataset, index: entry.index, view: entry.view,
          ok: false, qmeta: "", count: null, unit: "", columns: [], rows: [],
          error: message.slice(0, 300),
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
