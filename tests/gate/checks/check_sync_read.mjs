// Verify the SYNC read path (asyncReadsOn=0 → sync wasm + sync-XHR in the worker)
// runs a remote query correctly — the path we want iOS to use. Usage: node check_sync_read.mjs
import { launchBrowser } from "./_browser.mjs";
import { runWithRetry } from "./_util.mjs";

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
  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  const asyncWasmReqs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 200)));
  page.on("request", (r) => { if (/rete_wasm_async/.test(r.url())) asyncWasmReqs.push(r.url().split("/").pop()); });
  const PORT = process.env.PGPORT || "8090";
  await page.addInitScript(() => { try { localStorage.setItem("asyncReadsOn", "0"); } catch (e) {} });
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=worldcup2026&load=lazy&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.waitForTimeout(3500);

  await page.evaluate((q) => window.PlaygroundEditor.setText("q", q), Q);
  const out = await runWithRetry(page); // retries a transient R2 blip

  // Prove the SYNC path actually ran: the async wasm must NOT have been fetched
  // (else the flag was ignored and this proves nothing).
  const pass = out.rows > 0 && !out.errBlock && errs.length === 0 && asyncWasmReqs.length === 0;
  console.log(JSON.stringify({ verdict: pass ? "PASS" : "FAIL", rows: out.rows, errBlock: out.errBlock, qmeta: out.qmeta, tries: out.tries, asyncWasmFetched: asyncWasmReqs, errs: errs.slice(0, 3) }, null, 2));
  await browser.close();
  process.exit(pass ? 0 : 1);
};
main();
