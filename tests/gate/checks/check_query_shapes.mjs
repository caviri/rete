// Coverage for query surfaces the gate didn't touch: property paths, CONSTRUCT →
// graph view, and the reasoning (query_reasoned) path. All on the EMBEDDED scholar
// dataset, so it's deterministic (no live R2). Usage: node check_query_shapes.mjs
import { chromium } from "playwright";

const PATH_Q = `PREFIX cito: <http://purl.org/spar/cito/>
SELECT DISTINCT ?reached WHERE { <http://ex/paper/245> cito:cites+ ?reached }`;

const CONSTRUCT_Q = `PREFIX ex: <http://ex/>
CONSTRUCT { ?a ex:coauthor ?b } WHERE {
  { <http://ex/author/105> ex:coauthor ?b BIND(<http://ex/author/105> AS ?a) }
  UNION
  { <http://ex/author/105> ex:coauthor ?a . ?a ex:coauthor ?b }
}`;

const poll = async (page, fn, steps = 20) => {
  let r;
  for (let i = 0; i < steps; i++) { await page.waitForTimeout(500); r = await page.evaluate(fn); if (r.done) break; }
  return r;
};

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 160)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(2500);
  // Force the WHOLE-index strategy: the default progressive strategy answers from
  // the pyramid summary and shows a "not summary-answerable" note for value-
  // returning queries (paths / CONSTRUCT), which would hide the real result.
  await page.evaluate(() => { const s = document.getElementById("strategy"); if (s) { s.value = "whole"; s.dispatchEvent(new Event("change")); } });

  // 1) Property path: cito:cites+ transitive closure → table rows.
  await page.evaluate(() => { const f = document.getElementById("fmt"); if (f) { f.value = "table"; f.dispatchEvent(new Event("change")); } });
  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), PATH_Q);
  await page.evaluate(() => document.getElementById("run").click());
  const pathRes = await poll(page, () => {
    const n = document.querySelectorAll("#out table tbody tr").length;
    const err = !!document.querySelector("#out .error-box");
    return { done: n > 0 || err, rows: n, err };
  });

  // 2) CONSTRUCT → graph view → an <svg> with node circles renders.
  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), CONSTRUCT_Q);
  await page.evaluate(() => { const f = document.getElementById("fmt"); if (f) { f.value = "graph"; f.dispatchEvent(new Event("change")); } });
  await page.evaluate(() => document.getElementById("run").click());
  const graphRes = await poll(page, () => {
    const svg = document.querySelector("#out svg");
    const nodes = document.querySelectorAll("#out svg circle").length;
    const err = !!document.querySelector("#out .error-box");
    return { done: (svg && nodes > 0) || err, hasSvg: !!svg, nodes, err };
  });

  // 3) Reasoning path: enable the OWL-QL toggle, re-run a query — query_reasoned
  //    must still return rows (scholar has no TBox, so no NEW rows, but the reasoned
  //    code path must work end-to-end, not crash).
  let reasonRes = { done: true, rows: 0, skipped: true };
  const hasReason = await page.evaluate(() => !!document.getElementById("owlReason"));
  if (hasReason) {
    await page.evaluate(() => { const f = document.getElementById("fmt"); if (f) { f.value = "table"; f.dispatchEvent(new Event("change")); } });
    await page.evaluate(() => { const r = document.getElementById("owlReason"); if (r && !r.checked) { r.checked = true; r.dispatchEvent(new Event("change")); } });
    await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), PATH_Q);
    await page.evaluate(() => document.getElementById("run").click());
    reasonRes = await poll(page, () => {
      const n = document.querySelectorAll("#out table tbody tr").length;
      const err = !!document.querySelector("#out .error-box");
      return { done: n > 0 || err, rows: n, err, skipped: false };
    });
  }

  // 4) The "?" help next to 🧠 Reason opens the reasoning help modal (matching
  //    the Output / Strategy help pattern).
  const help = await page.evaluate(() => {
    const btn = document.getElementById("reasonHelp");
    if (!btn) return { has: false };
    btn.click();
    const modal = document.getElementById("reasonModal");
    const shown = modal && !modal.classList.contains("hidden");
    const body = (modal && modal.textContent) || "";
    return { has: true, shown, mentionsQL: /OWL 2 QL/i.test(body), mentionsCost: /costs|no-op|nothing/i.test(body) };
  });

  const pass =
    pathRes.rows > 0 && !pathRes.err &&
    graphRes.hasSvg && graphRes.nodes > 0 && !graphRes.err &&
    (reasonRes.skipped || (reasonRes.rows > 0 && !reasonRes.err)) &&
    help.has && help.shown && help.mentionsQL &&
    errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    propertyPathRows: pathRes.rows,
    constructGraphSvg: graphRes.hasSvg, graphNodes: graphRes.nodes,
    reasoningRows: reasonRes.rows, reasoningSkipped: reasonRes.skipped,
    reasonHelpOpens: help.shown, reasonHelpHasContent: help.mentionsQL,
    errs: errs.slice(0, 3),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
