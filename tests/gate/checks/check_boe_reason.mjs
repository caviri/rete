// The BOE ontology-enhancement deliverable: OWL 2 QL reasoning over the REMOTE
// enriched dataset. Example ex=0 ("norms with the force of law") is reason:true,
// so loading it auto-enables 🧠 Reason. With reasoning the query returns the
// force-of-law norms via the subClassOf tiers; with reasoning OFF it returns 0
// (no norm is directly typed the intermediate class). Usage: node check_boe_reason.mjs
import { chromium } from "playwright";
import { runWithRetry } from "./_util.mjs";

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=boe&load=lazy&mode=sparql&ex=0`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000);

  // The example is reason:true → the toggle should be ON, and the query mentions the tier class.
  const setup = await page.evaluate(() => ({
    reasonOn: !!(document.getElementById("owlReason") || {}).checked,
    q: (window.PlaygroundEditor.getText ? window.PlaygroundEditor.getText("q") : "") || "",
  }));

  // Run WITH reasoning (as the example loads): expect rows.
  const withReason = await runWithRetry(page, { steps: 50 });

  // Now turn reasoning OFF and re-run: expect 0 rows (the demo contrast).
  await page.evaluate(() => { const r = document.getElementById("owlReason"); if (r) { r.checked = false; r.dispatchEvent(new Event("change")); } });
  await page.evaluate(() => document.getElementById("run").click());
  let off = { rows: 1 };
  for (let i = 0; i < 40; i++) {
    await page.waitForTimeout(1000);
    off = await page.evaluate(() => {
      const qm = (document.getElementById("qmeta") || {}).textContent || "";
      const m = qm.match(/(\d+)\s+row/);
      return { rows: document.querySelectorAll("#out table tbody tr, #out .cards .card").length || (m ? Number(m[1]) : (/0 row|no result/i.test(qm) ? 0 : 1)), errBlock: !!document.querySelector("#out .error-box"), qmeta: qm };
    });
    if (off.qmeta) break;
  }

  // The meaningful assertions: the example auto-enabled reasoning, reasoning
  // returned the force-of-law norms, and turning it OFF collapses to 0 rows.
  const pass =
    setup.reasonOn &&
    withReason.rows > 0 && !withReason.errBlock &&
    off.rows === 0 && !off.errBlock &&
    errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    reasonToggleAutoOn: setup.reasonOn,
    rowsWithReason: withReason.rows, rowsWithoutReason: off.rows,
    tries: withReason.tries, errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
