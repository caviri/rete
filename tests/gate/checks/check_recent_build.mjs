// A RECENTLY-BUILT dataset, read in the browser, with a bound predicate AND a
// bound object.
//
// Why this check exists: the playground shipped a 2026-07-19 wasm for six weeks
// while the engine moved to u64 section/restart offsets (#70, 1bd8902c,
// 10e5749d). Files written by the newer CLI then broke its index path — a scan
// still worked, but any query binding predicate + object returned **0 rows with
// no error**. mirbase, ethz-research-collection and hugging-face were all
// affected in production.
//
// The gate did not catch it because every live-R2 browser check ran a dataset
// built BEFORE that engine change (boe, enac-it4research), which the old wasm
// could still read. So the coverage gap was not "no browser test" — it was "no
// browser test against a file the current CLI produced".
//
// HONEST SCOPE: this does NOT reproduce that specific failure. Swapping the
// 07-19 wasm back in — with the default reader and again with RETE_FORCE_SYNC=1
// — still passes here, so whatever emptied those results needs a condition this
// example does not hit. What the check DOES buy is the coverage that was
// missing: every other live-R2 browser check runs a graph built before the
// engine changed, so none of them would notice the shipped browser engine
// drifting away from the shipped builder at all.
//
// hugging-face is pinned in web/datasets.lock.json (stable bytes, format 5) and
// its example 3 is `?person hf:memberOf <…/mistralai>` — a bound predicate and
// a bound object, the shape that was reported as silently empty.
//
// Usage: node check_recent_build.mjs
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

const main = async () => {
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  if (process.env.RETE_FORCE_SYNC === "1") {
    await page.addInitScript(() => localStorage.setItem("asyncReadsOn", "0"));
  }
  const PORT = process.env.PGPORT || "8090";
  // ex=3 — "An organization's people, ranked by reach": bound predicate
  // (hf:memberOf) AND bound object (the mistralai org IRI).
  await page.goto(
    `http://localhost:${PORT}/playground.html#dataset=hugging-face&load=lazy&mode=sparql&ex=3`,
    { waitUntil: "domcontentloaded" },
  );
  await page.waitForFunction(
    () => window.PlaygroundEditor && document.getElementById("run"),
    { timeout: 60000 },
  );
  await page.waitForTimeout(4000);

  const res = await runWithRetry(page, { steps: 60 });

  // Shape check: the rows must carry huggingface.co person IRIs and a follower
  // count, so a truncated or mis-decoded result cannot pass as success.
  const body = await page.evaluate(
    () => (document.querySelector("#out table") || {}).textContent || "",
  );
  const hasPersonIri = /huggingface\.co\//.test(body);
  const hasFollowers = /\d/.test(body);

  const pass =
    res.rows > 0 && !res.errBlock && hasPersonIri && hasFollowers && errs.length === 0;
  console.log(
    JSON.stringify(
      {
        verdict: pass ? "PASS" : "FAIL",
        note: "bound predicate + bound object on a post-u64-offsets file",
        rows: res.rows,
        qmeta: res.qmeta,
        errText: res.errText,
        hasPersonIri,
        hasFollowers,
        forceSync: process.env.RETE_FORCE_SYNC === "1",
        tries: res.tries,
        errs: errs.slice(0, 3),
      },
      null,
      2,
    ),
  );
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
