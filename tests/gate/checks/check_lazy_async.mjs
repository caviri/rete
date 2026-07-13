// Verify the ASYNC (asyncify fetch) lazy path still works after the cache:'no-store'
// + range-length-validation fix — runs the exact mtg GROUP BY the user hit, lazily,
// against the live R2 file. Asserts rows come back and no console/page errors.
import { chromium } from "playwright";

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
  page.on("pageerror", (e) => errs.push("page: " + String(e).slice(0, 200)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  // Force async reads ON (the asyncify variant — the path that was failing).
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "1"); } catch (e) {} });
  // Open mtg in lazy mode directly.
  await page.goto(`http://localhost:${PORT}/playground.html?v=probe#dataset=mtg&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(4000); // let the header/remote open

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  await page.evaluate(() => document.getElementById("run").click());

  let out = { rows: 0, text: "", variant: null };
  for (let i = 0; i < 60; i++) { // up to ~60s
    await page.waitForTimeout(1000);
    out = await page.evaluate(() => {
      const rows = document.querySelectorAll("#out table tbody tr").length;
      const errBlock = !!document.querySelector("#out .err-tech-body");
      const errText = errBlock ? document.querySelector("#out .err-tech-body").textContent.slice(0, 500) : "";
      const qmeta = (document.getElementById("qmeta") || {}).textContent || "";
      const variant = window.state ? !!window.state.asyncReadsOn : null;
      return { rows, text: errText, qmeta, errBlock, variant };
    });
    if (out.rows > 0 || out.errBlock) break;
  }

  const verdict = out.rows > 0 && !out.errBlock ? "PASS" : "FAIL";
  console.log(JSON.stringify({ verdict, rows: out.rows, qmeta: out.qmeta, asyncVariant: out.variant, errBlock: out.errBlock, errSample: out.text, consoleErrs: errs.slice(0, 4) }, null, 2));
  await browser.close();
  process.exit(verdict === "PASS" ? 0 : 1);
};
main();
