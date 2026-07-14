// Build the immutable v1 N-Quads compatibility fixture in-browser, open the
// produced bytes, and query the named graph for Alice. No network is involved.
import { chromium } from "playwright";

const FIXTURE = `<http://example.test/alice> <http://example.test/knows> <http://example.test/bob> .
<http://example.test/bob> <http://example.test/name> "Bob"@en .
<http://example.test/alice> <http://example.test/name> "Alice"@en <http://example.test/people> .`;

const QUERY = `SELECT ?person ?name WHERE {
  GRAPH <http://example.test/people> {
    ?person <http://example.test/name> ?name
  }
}`;

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("buildBtn"), { timeout: 60000 });

  await page.click("#buildBtn");
  await page.selectOption("#buildFormat", "nq");
  await page.evaluate((text) => window.PlaygroundEditor.setText("buildText", text), FIXTURE);
  await page.fill("#cardTitle", "Release compatibility fixture");
  await page.fill("#cardKey", "release-compat-fixture");
  await page.click("#buildRun");
  await page.waitForFunction(() => /Saved|Built/.test((document.getElementById("buildOut") || {}).textContent || ""), { timeout: 30000 });

  const build = await page.evaluate(() => ({
    meta: (document.getElementById("buildMeta") || {}).textContent || "",
    out: (document.getElementById("buildOut") || {}).textContent || "",
    canOpen: !(document.getElementById("buildOpen") || {}).disabled,
  }));
  await page.click("#buildOpen");
  await page.waitForFunction(() => /release compatibility fixture/i.test((document.getElementById("dsName") || {}).textContent || ""), { timeout: 15000 });
  await page.evaluate((q) => {
    const strategy = document.getElementById("strategy");
    if (strategy) { strategy.value = "whole"; strategy.dispatchEvent(new Event("change")); }
    window.PlaygroundEditor.setText("q", q);
    document.getElementById("run").click();
  }, QUERY);
  await page.waitForFunction(() => document.querySelectorAll("#out table tbody tr").length > 0 || document.querySelector("#out .error-box"), { timeout: 30000 });
  const query = await page.evaluate(() => ({
    rows: document.querySelectorAll("#out table tbody tr").length,
    text: (document.getElementById("out") || {}).textContent || "",
    error: !!document.querySelector("#out .error-box"),
  }));

  const pass = build.canOpen && /3 triples/.test(build.meta) && query.rows === 1 &&
    /example\.test\/alice/i.test(query.text) && /Alice/i.test(query.text) && !query.error && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL",
    buildMeta: build.meta, canOpen: build.canOpen,
    rows: query.rows, hasAlice: /Alice/i.test(query.text), error: query.error,
    errs: errs.slice(0, 4),
  }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
