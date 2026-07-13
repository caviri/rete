// C2 regression: if the async reader assets (~8 MB) fail to load (404 / missing
// build / network), the remote query must DEGRADE to the always-present sync wasm
// and still return rows — not hard-fail. We 404 the async wasm with asyncReadsOn
// forced on, then assert rows + no error. Usage: node check_async_fallback.mjs
import { chromium } from "playwright";
import { runWithRetry } from "./_util.mjs";

const Q = `PREFIX wc: <https://w3id.org/rete/worldcup#>
PREFIX sc: <http://schema.org/>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?num ?player WHERE {
  <https://w3id.org/rete/worldcup/2026/team/Argentina> wc:squadPlayer ?p .
  ?p sc:name ?player .
  OPTIONAL { ?p wc:shirtNumber ?num }
} ORDER BY xsd:integer(?num)`;

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  // Make the async assets unavailable → loadAsyncAssets rejects → must fall back.
  await page.route(/rete_wasm_async\.(js|wasm)/, (route) => route.fulfill({ status: 404, body: "nope" }));
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "1"); } catch (e) {} }); // force async intent
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(3000);

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  const out = await runWithRetry(page);

  // Fell back to sync and worked: rows, no error box, no page errors.
  const pass = out.rows > 0 && !out.errBlock && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", rows: out.rows, qmeta: out.qmeta, tries: out.tries, errBlock: out.errBlock, errText: out.errText.slice(0, 160), errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
