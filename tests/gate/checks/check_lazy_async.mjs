// Verify the ASYNC (asyncify fetch) lazy path still works after the cache:'no-store'
// + range-length-validation fix — runs the exact mtg GROUP BY the user hit, lazily,
// against the live R2 file. Asserts rows come back and no console/page errors.
import { chromium } from "playwright";
import { runWithRetry } from "./_util.mjs";

const Q = `PREFIX mtg: <https://w3id.org/rete/mtg#>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
SELECT ?type (COUNT(?c) AS ?cards) WHERE {
  ?c a mtg:Card ; a ?t .
  ?t rdfs:subClassOf mtg:Card ; rdfs:label ?type .
} GROUP BY ?type ORDER BY DESC(?cards)`;

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  const asyncWasmReqs = [];
  page.on("pageerror", (e) => errs.push("page: " + String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  page.on("request", (r) => { if (/rete_wasm_async/.test(r.url())) asyncWasmReqs.push(r.url().split("/").pop()); });
  const PORT = process.env.PGPORT || "8090";
  // Force async reads ON (the asyncify variant — the path that was failing).
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "1"); } catch (e) {} });
  // Open mtg in lazy mode directly.
  await page.goto(`http://localhost:${PORT}/playground.html?v=probe#dataset=mtg&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000); // let the header/remote open

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  const out = await runWithRetry(page, { steps: 60 }); // retries a transient R2 blip

  // PASS requires: rows, no error box, NO console/page errors, AND proof the
  // async path actually ran (the async wasm was fetched). Without the last two the
  // check could pass while silently falling back or logging engine errors.
  const asyncRan = asyncWasmReqs.length > 0;
  const pass = out.rows > 0 && !out.errBlock && errs.length === 0 && asyncRan;
  const diagnostic = await page.evaluate(() =>
    (document.querySelector("#out .err-tech-body") || {}).textContent || "",
  );
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", rows: out.rows, qmeta: out.qmeta, tries: out.tries, asyncVariantRan: asyncRan, asyncWasmFetched: asyncWasmReqs, errBlock: out.errBlock, errSample: out.errText.slice(0, 200), diagnostic: diagnostic.slice(0, 1600), consoleErrs: errs.slice(0, 4) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
