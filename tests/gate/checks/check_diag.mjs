// Verify the error "Copy full log" diagnostics block renders with the key fields.
// Triggers a non-user engine error (SERVICE ?var, which rete rejects) on the
// embedded default dataset — no network. Usage: node check_diag.mjs
import { chromium } from "playwright";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e).slice(0, 160)));
  const PORT = process.env.PGPORT || "8080";
  await page.goto(`http://localhost:${PORT}/playground.html`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run") && window.state && window.state.bytes, { timeout: 60000 }).catch(() => {});
  // If state isn't global, just wait for the editor + a moment for the embedded load.
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2500);

  await page.evaluate(() => window.PlaygroundEditor.setText("q", "SELECT * WHERE { SERVICE ?e { ?s ?p ?o } } LIMIT 1"));
  await page.evaluate(() => document.getElementById("run").click());

  let found = { block: false, button: false, report: "" };
  for (let i = 0; i < 20; i++) {
    await page.waitForTimeout(500);
    found = await page.evaluate(() => {
      const pre = document.querySelector("#out .err-tech-body");
      const btn = document.querySelector("#out .err-copy");
      return { block: !!pre, button: !!btn, report: pre ? pre.textContent : "" };
    });
    if (found.block) break;
  }

  const R = found.report;
  const has = (k) => R.includes(k);
  const verdict =
    found.block && found.button &&
    has("rete playground — error report") && has("agent:") && has("dataset:") &&
    has("async-reads") && has("device:") && errors.length === 0
      ? "PASS" : "FAIL";
  console.log(JSON.stringify({
    verdict,
    hasBlock: found.block, hasButton: found.button,
    fields: { agent: has("agent:"), dataset: has("dataset:"), asyncReads: has("async-reads"), device: has("device:"), jsHeap: has("jsHeap"), error: has("error:") },
    reportSample: R.slice(0, 400),
    errors: errors.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(verdict === "PASS" ? 0 : 1);
};
main();
