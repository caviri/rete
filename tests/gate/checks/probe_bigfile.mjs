// Manual triage probe (not part of the gate): open DEPLOYED playground
// datasets in the selected browser (RETE_BROWSER=chromium|firefox), run their
// example — or an arbitrary query — and report rows/errors. This is the tool
// that pinned the 2026-07-17 big-remote-file regressions: it exercises the
// exact shipped page + wasm against live R2, which no local test does.
//
//   docker run --rm --network host -v "$PWD:/work" -w /work/tests/gate/checks \
//     -e PROBE_TARGETS="wikidata-ontology:0,databnf:0,boe:0" \
//     mcr.microsoft.com/playwright:v1.49.0-jammy node probe_bigfile.mjs
//
// Env: PROBE_TARGETS  dataset:exIndex[:sync] list (":sync" forces the sync reader)
//      PROBE_QUERY    replace the example's query text before running
//      PROBE_STEPS    seconds to poll for rows (default 180)
import { launchBrowser, selectedBrowserName } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

// target syntax: dataset:exIndex[:sync]  — ":sync" forces asyncReadsOn=0
const targets = (process.env.PROBE_TARGETS || "wikidata-ontology:0,boe:0").split(",");
const base = "https://caviri.github.io/rete/playground.html";

const main = async () => {
  const browser = await launchBrowser();
  const results = [];
  for (const t of targets) {
    const [ds, ex, mode] = t.split(":");
    const page = await browser.newPage();
    if (mode === "sync") {
      await page.addInitScript(() => localStorage.setItem("asyncReadsOn", "0"));
    }
    const errs = [];
    page.on("pageerror", (e) => errs.push(`page: ${String(e).slice(0, 250)}`));
    page.on("console", (m) => {
      if (m.type() === "error") errs.push(`console: ${m.text().slice(0, 250)}`);
    });
    try {
      await page.goto(`${base}#dataset=${ds}&load=lazy&mode=sparql&ex=${ex}`, {
        waitUntil: "domcontentloaded",
      });
      await page.waitForFunction(
        () => window.PlaygroundEditor && document.getElementById("run"),
        { timeout: 60000 },
      );
      await page.waitForFunction(
        () => {
          const run = document.getElementById("run");
          const q = window.PlaygroundEditor?.getText
            ? window.PlaygroundEditor.getText("q")
            : (document.getElementById("q") || {}).value;
          return run && !run.disabled && q && q.trim().length > 0;
        },
        { timeout: 60000 },
      );
      await page.waitForTimeout(4000);
      if (process.env.PROBE_QUERY) {
        await page.evaluate((q) => {
          if (window.PlaygroundEditor?.setText) window.PlaygroundEditor.setText("q", q);
          else document.getElementById("q").value = q;
        }, process.env.PROBE_QUERY);
      }
      const out = await runWithRetry(page, {
        tries: 1,
        steps: Number(process.env.PROBE_STEPS || 180),
        stepMs: 1000,
      });
      results.push({
        ds,
        mode: mode || "async",
        browser: selectedBrowserName(),
        rows: out.rows,
        errBlock: out.errBlock,
        errText: String(out.errText || "").slice(0, 1400),
        errs: errs.slice(0, 3),
      });
    } catch (e) {
      results.push({
        ds,
        browser: selectedBrowserName(),
        fatal: String(e).slice(0, 250),
        errs: errs.slice(0, 3),
      });
    }
    await page.close();
  }
  console.log(JSON.stringify(results, null, 2));
  await browser.close();
};

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
