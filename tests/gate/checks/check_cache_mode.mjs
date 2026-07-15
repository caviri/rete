// Whole-file cache mode must download once, persist across a page reload, and
// answer the second query without touching the local range-capable host.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { launchBrowser } from "./_browser.mjs";

const listen = (server) => new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server.address().port)));

const embeddedFixture = async (name) => {
  const html = await readFile("/work/docs/playground.html", "utf8");
  const marker = "const RETE_DATASETS_B64 = ";
  const start = html.indexOf(marker);
  if (start < 0) throw new Error("tracked playground has no embedded dataset map");
  const jsonStart = start + marker.length;
  const lineEnd = html.indexOf("\n", jsonStart);
  const expression = html.slice(jsonStart, lineEnd).trim().replace(/;$/, "");
  const encoded = JSON.parse(expression)[name];
  if (!encoded) throw new Error(`tracked playground has no embedded ${name} dataset`);
  return Buffer.from(encoded, "base64");
};

const main = async () => {
  // Standalone web/*.rete files are deliberately ignored. Decode the exact
  // tracked causal fixture from the generated playground so this check is
  // self-contained on a clean CI checkout.
  const fixture = await embeddedFixture("causal");
  const traffic = { full: 0, range: 0 };
  const server = createServer((req, res) => {
    if (req.url?.split("?")[0] !== "/causal.rete") { res.writeHead(404); res.end("not found"); return; }
    const range = req.headers.range && /bytes=(\d+)-(\d*)/.exec(req.headers.range);
    const common = { "Access-Control-Allow-Origin": "*", "Access-Control-Expose-Headers": "Content-Range,Content-Length,Accept-Ranges", "Accept-Ranges": "bytes" };
    if (range) {
      traffic.range++;
      const start = Number(range[1]);
      const end = range[2] ? Math.min(Number(range[2]), fixture.length - 1) : fixture.length - 1;
      const body = fixture.subarray(start, end + 1);
      res.writeHead(206, { ...common, "Content-Type": "application/octet-stream", "Content-Range": `bytes ${start}-${end}/${fixture.length}`, "Content-Length": body.length });
      res.end(body); return;
    }
    traffic.full++;
    res.writeHead(200, { ...common, "Content-Type": "application/octet-stream", "Content-Length": fixture.length });
    res.end(fixture);
  });
  const fixturePort = await listen(server);
  const fixtureUrl = `http://127.0.0.1:${fixturePort}/causal.rete`;

  const browser = await launchBrowser();
  const context = await browser.newContext();
  const page = await context.newPage();
  const errs = [];
  page.on("pageerror", (e) => errs.push(String(e).slice(0, 240)));
  await page.addInitScript((url) => {
    Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", {
      configurable: true,
      set(value) {
        const row = (value.datasets || []).find((d) => d.key === "causal");
        if (row) row.url = url;
        Object.defineProperty(window, "RETE_PLAYGROUND_CATALOG", { configurable: true, writable: true, value });
      },
    });
  }, fixtureUrl);
  const PORT = process.env.PGPORT || "8090";
  const url = `http://localhost:${PORT}/playground.html#dataset=causal&load=cache&mode=sparql&ex=0`;
  await page.goto(url, { waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => /remote \(cached\)/i.test((document.getElementById("sourcePill") || {}).textContent || "") && document.getElementById("cacheModal")?.classList.contains("hidden"), { timeout: 60000 });
  await page.click("#run");
  await page.waitForFunction(() => document.querySelectorAll("#out table tbody tr, #out .mapview").length > 0 || document.querySelector("#out .error-box"), { timeout: 30000 });
  const firstTraffic = { ...traffic };

  await page.reload({ waitUntil: "domcontentloaded" });
  await page.waitForFunction(() => /remote \(cached\)/i.test((document.getElementById("sourcePill") || {}).textContent || "") && document.getElementById("cacheModal")?.classList.contains("hidden"), { timeout: 60000 });
  const beforeSecondQuery = { ...traffic };
  await page.click("#run");
  await page.waitForFunction(() => document.querySelectorAll("#out table tbody tr, #out .mapview").length > 0 || document.querySelector("#out .error-box"), { timeout: 30000 });
  const secondTraffic = { full: traffic.full - beforeSecondQuery.full, range: traffic.range - beforeSecondQuery.range };
  const reloadTraffic = { full: beforeSecondQuery.full - firstTraffic.full, range: beforeSecondQuery.range - firstTraffic.range };
  const result = await page.evaluate(() => ({ error: !!document.querySelector("#out .error-box"), qmeta: (document.getElementById("qmeta") || {}).textContent || "" }));

  const pass = firstTraffic.full === 1 && firstTraffic.range === 0 && reloadTraffic.full === 0 && reloadTraffic.range === 0 &&
    secondTraffic.full === 0 && secondTraffic.range === 0 && !result.error && errs.length === 0;
  console.log(JSON.stringify({
    verdict: pass ? "PASS" : "FAIL", fixtureUrl, firstTraffic, reloadTraffic, secondTraffic,
    qmeta: result.qmeta, error: result.error, errs: errs.slice(0, 4),
  }, null, 2));
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
  process.exit(pass ? 0 : 1);
};
main();
