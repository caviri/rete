// The davidrumsey spatial roll-up, over the REMOTE collection on live R2.
//
// drm:city / county / stateProvince / region / country / worldArea are all
// rdfs:subPropertyOf dcterms:spatial, and dcterms:spatial is asserted on ZERO
// subjects of the published file — so example ex=6 answers ONLY under OWL 2 QL
// query rewriting. It shipped without `reason: true` and therefore greeted
// every visitor with 0 rows (PR #184 flagged it; no check covered it).
//
// This is that check: the example must auto-enable 🧠 Reason and return rows,
// and turning reasoning off must collapse it to 0 — the contrast IS the demo.
// Usage: node check_davidrumsey_spatial.mjs
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
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=davidrumsey&load=lazy&mode=sparql&ex=6`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000);

  // The example is reason:true → the toggle should already be ON, and the query
  // must still be the single dct:spatial pattern the entailment acts on.
  const setup = await page.evaluate(() => ({
    reasonOn: !!(document.getElementById("owlReason") || {}).checked,
    // PlaygroundEditor exposes getText only once CodeMirror is wired; the
    // textarea it upgrades is the fallback every other check uses.
    q: (window.PlaygroundEditor.getText ? window.PlaygroundEditor.getText("q") : "")
      || (document.getElementById("q") || {}).value || "",
  }));
  // ONE spatial pattern is the whole point: ?place must come from dct:spatial
  // and never be bound off a drm: place predicate directly.
  const singlePattern = /\?m\s+dct:spatial\s+\?place/.test(setup.q)
    && !/drm:(city|county|stateProvince|region|country|worldArea)\s+\?place/.test(setup.q);

  // Run WITH reasoning (as the example loads): expect rows. The six-way rewrite
  // over a 74.8 MB remote file reads ~27 MB, so poll generously.
  const withReason = await runWithRetry(page, { steps: 120, tries: 2 });

  // Now turn reasoning OFF and re-run: expect 0 rows. Only trust a COMPLETED
  // qmeta that differs from the with-reason one — mid-flight the previous
  // result's table and qmeta are still on screen.
  await page.evaluate(() => { const r = document.getElementById("owlReason"); if (r) { r.checked = false; r.dispatchEvent(new Event("change")); } });
  const staleQmeta = await page.evaluate(() => (document.getElementById("qmeta") || {}).textContent || "");
  await page.evaluate(() => document.getElementById("run").click());
  let off = { rows: 1, errBlock: false, qmeta: "" };
  for (let i = 0; i < 60; i++) {
    await page.waitForTimeout(1000);
    off = await page.evaluate(() => {
      const qm = (document.getElementById("qmeta") || {}).textContent || "";
      const m = qm.match(/(\d+)\s+row/);
      return { rows: document.querySelectorAll("#out table tbody tr, #out .cards .card").length || (m ? Number(m[1]) : (/0 row|no result/i.test(qm) ? 0 : 1)), errBlock: !!document.querySelector("#out .error-box"), qmeta: qm };
    });
    if (off.errBlock) break;
    if (off.qmeta && off.qmeta !== staleQmeta && !off.qmeta.startsWith("⏳") && /\d+\s+row|no result/i.test(off.qmeta)) break;
  }

  const pass =
    setup.reasonOn &&
    singlePattern &&
    withReason.rows > 0 && !withReason.errBlock &&
    off.rows === 0 && !off.errBlock &&
    errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    reasonToggleAutoOn: setup.reasonOn,
    oneSpatialPattern: singlePattern,
    rowsWithReason: withReason.rows, rowsWithoutReason: off.rows,
    withReasonMeta: withReason.qmeta, withReasonError: withReason.errText,
    withoutReasonMeta: off.qmeta, forceSync: process.env.RETE_FORCE_SYNC === "1",
    tries: withReason.tries, errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
