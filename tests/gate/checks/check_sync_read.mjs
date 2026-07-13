// Verify the SYNC read path (asyncReadsOn=0 → sync wasm + sync-XHR in the worker)
// runs a remote query correctly — the path we want iOS to use. Usage: node check_sync_read.mjs
import { chromium } from "playwright";

const Q = `PREFIX wc: <https://w3id.org/rete/worldcup#>
PREFIX sc: <http://schema.org/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX xsd: <http://www.w3.org/2001/XMLSchema#>
SELECT ?num ?pos ?player ?club ?dob WHERE {
  <https://w3id.org/rete/worldcup/2026/team/Argentina> wc:squadPlayer ?p .
  ?p sc:name ?player .
  OPTIONAL { ?p wc:shirtNumber ?num }
  OPTIONAL { ?p wc:position ?pos }
  OPTIONAL { ?p sc:birthDate ?dob }
  OPTIONAL { ?p wc:clubAtTournament ?c . ?c rdfs:label ?club }
} ORDER BY xsd:integer(?num)`;

const main = async () => {
  const browser = await chromium.launch();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  const PORT = process.env.PGPORT || "8090";
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "0"); } catch (e) {} });
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(3500);

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  await page.evaluate(() => document.getElementById("run").click());

  let out = { rows: 0, errBlock: false, qmeta: "" };
  for (let i = 0; i < 45; i++) {
    await page.waitForTimeout(1000);
    out = await page.evaluate(() => ({
      rows: document.querySelectorAll("#out table tbody tr").length,
      errBlock: !!document.querySelector("#out .error-box"),
      qmeta: (document.getElementById("qmeta") || {}).textContent || "",
    }));
    if (out.rows > 0 || out.errBlock) break;
  }
  const pass = out.rows > 0 && !out.errBlock && errs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", ...out, errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
