// End-to-end check of docs/oldmaps.html — the OldMapsOnline-style map-first
// explorer over the remote davidrumsey.rete (R2). Serves /work/docs itself;
// needs network (R2 ranges + unpkg Leaflet + OSM tiles). NOT part of the gate
// matrix — run manually after touching the page:
//
//   docker run --rm -v "$PWD:/work" -w /work/tests/gate \
//     mcr.microsoft.com/playwright:v1.49.0-noble node checks/check_oldmaps.mjs
//
// Optional: OLDMAPS_SHOT=/work/somewhere.png saves a final screenshot.
import { chromium } from "playwright";
import http from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";

const DOCS = path.resolve(process.cwd(), "../../docs");
const PORT = 8412;
const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript",
               ".css": "text/css", ".wasm": "application/wasm" };
const server = http.createServer(async (req, res) => {
  try {
    const p = path.join(DOCS, decodeURIComponent(new URL(req.url, "http://x").pathname));
    const body = await readFile(p);
    res.writeHead(200, { "content-type": MIME[path.extname(p)] || "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404); res.end("not found");
  }
});
await new Promise((ok) => server.listen(PORT, ok));

const SHOT = process.env.OLDMAPS_SHOT || "";
const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1360, height: 900 } });
const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => {
  if (m.type() !== "error") return;
  const url = (m.location() && m.location().url) || "";
  // basemap tile/CDN hiccups are environmental, not page bugs
  if (url.includes("openstreetmap.org") || url.includes("unpkg.com")) return;
  errors.push(`console: ${m.text()}`);
});

const fail = (msg) => { console.error(`FAIL ${msg}`); process.exitCode = 1; };

await page.goto(`http://localhost:${PORT}/oldmaps.html`, { waitUntil: "domcontentloaded" });

// 1. engine boots + the initial Lake-Geneva viewport query completes
await page.waitForFunction(
  () => /maps ·/.test(document.getElementById("status").textContent),
  null, { timeout: 120000 }
).catch(async () => fail(`initial query never completed — status: "${await page.textContent("#status")}"`));
const status1 = await page.textContent("#status");
console.log(`initial query: ${status1}`);

// 2. pins on the map and cards in the rail
const pins = await page.locator(".cnt").count();
const cards = await page.locator("#rail .card").count();
console.log(`pins=${pins} cards=${cards}`);
if (pins < 1) fail("no cluster pins rendered");
if (cards < 1) fail("no result cards rendered");

// 3. the title text filter re-queries and narrows the rail
await page.fill("#q", "geneva");
await page.waitForTimeout(700); // let the 400ms debounce fire so we wait on the NEW query
await page.waitForFunction(
  () => { const s = document.getElementById("status").textContent; return /maps ·/.test(s) && !/querying/.test(s); },
  null, { timeout: 120000 }
);
// "geneva" appears in titles of maps geocoded to the initial viewport (the 1493
// Nuremberg Chronicle views among them) — the filter must keep some cards
const cards2 = await page.locator("#rail .card").count();
console.log(`after "geneva" filter: cards=${cards2}`);
if (cards2 < 1) fail('no cards for the "geneva" title filter');
if (cards2 >= 1) {
  const t1 = await page.locator("#rail .card .t").first().textContent();
  console.log(`first filtered card: ${t1.slice(0, 80)}`);
  if (!/genev|genf|genève/i.test(t1)) fail(`filtered card title doesn't match filter: ${t1.slice(0, 80)}`);
}

// 4. open the viewer from the first card (if any survived the filter)
if (cards2 > 0) {
  await page.locator("#rail .card").first().click();
  const on = await page.locator("#viewer.on").count();
  if (on !== 1) fail("viewer overlay did not open");
  const href = await page.getAttribute("#vRumsey", "href");
  if (!/davidrumsey\.com\/luna\/servlet\/detail\//.test(href || "")) fail(`viewer detail link wrong: ${href}`);
  console.log(`viewer opened, detail: ${href}`);
  await page.click("#vClose");
}

if (SHOT) { await page.screenshot({ path: SHOT }); console.log(`shot -> ${SHOT}`); }
if (errors.length) { console.error("console/page errors:"); errors.forEach((e) => console.error("  " + e)); process.exitCode = 1; }
await browser.close();
server.close();
console.log(process.exitCode ? "check_oldmaps: FAIL" : "check_oldmaps: OK");
