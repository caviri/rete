// Successful SPARQL SERVICE federation against a deterministic local endpoint.
// The existing diagnostics check owns the failure/error contract separately.
import { createServer } from "node:http";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const main = async () => {
  let serviceRequests = 0;
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/sparql") { res.writeHead(404); res.end("not found"); return; }
    if (req.method === "OPTIONS") {
      res.writeHead(204, { "Access-Control-Allow-Origin": "*", "Access-Control-Allow-Methods": "GET,POST,OPTIONS", "Access-Control-Allow-Headers": "Content-Type,Accept" });
      res.end(); return;
    }
    serviceRequests++;
    req.resume();
    req.on("end", () => {
      const body = JSON.stringify({
        head: { vars: ["remote"] },
        results: { bindings: [{ remote: { type: "uri", value: "urn:gate:remote" } }] },
      });
      res.writeHead(200, {
        "Content-Type": "application/sparql-results+json",
        "Content-Length": Buffer.byteLength(body),
        "Access-Control-Allow-Origin": "*",
      });
      res.end(body);
    });
  });
  const servicePort = await listen(server);
  const serviceUrl = `http://127.0.0.1:${servicePort}/sparql`;
  const query = `SELECT ?local ?remote WHERE {
    <http://ex/paper/245> ?p ?o .
    BIND(<urn:gate:local> AS ?local)
    SERVICE <${serviceUrl}> { ?remote <urn:gate:predicate> <urn:gate:object> }
  } LIMIT 1`;

  const browser = await launchBrowser();
  const page = await browser.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  page.on("console", (m) => { if (m.type() === "error") errs.push("console: " + m.text().slice(0, 200)); });
  const PORT = process.env.PGPORT || "8090";
  await page.goto(`http://localhost:${PORT}/playground.html#dataset=scholar&mode=sparql`, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => window.PlaygroundEditor && document.getElementById("run"), { timeout: 60000 });
  await page.evaluate((q) => {
    const strategy = document.getElementById("strategy");
    if (strategy) { strategy.value = "whole"; strategy.dispatchEvent(new Event("change")); }
    window.PlaygroundEditor.setText("q", q);
    document.getElementById("run").click();
  }, query);
  await page.waitForFunction(() => document.querySelectorAll("#out table tbody tr").length > 0 || document.querySelector("#out .error-box"), { timeout: 30000 });
  const result = await page.evaluate(() => ({
    rows: document.querySelectorAll("#out table tbody tr").length,
    text: (document.getElementById("out") || {}).textContent || "",
    error: !!document.querySelector("#out .error-box"),
  }));
  const pass = result.rows === 1 && /urn:gate:local/.test(result.text) && /urn:gate:remote/.test(result.text) &&
    serviceRequests > 0 && !result.error && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL", serviceUrl, serviceRequests,
    rows: result.rows, localJoined: /urn:gate:local/.test(result.text),
    remoteJoined: /urn:gate:remote/.test(result.text), error: result.error,
    errs: errs.slice(0, 4),
  }, null, 2));
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
  process.exit(pass ? 0 : 1);
};
main();
