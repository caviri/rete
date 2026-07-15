// End-to-end check of docs/yasgui.html — the single-file Yasgui-style SPARQL
// IDE (built by scripts/build_yasgui.py from web/yasgui.template.html +
// web/yasgui-src/). Serves /work/docs itself; needs network for the R2-hosted
// catalog datasets. NOT part of the gate matrix (run.mjs lists its checks
// explicitly) — run it manually after touching the yasgui sources:
//
//   docker run --rm --network host -v "$PWD:/work" -w /work/tests/gate \
//     mcr.microsoft.com/playwright:v1.49.0-jammy node checks/check_yasgui.mjs
//
// Optional: YASGUI_SHOT=/work/somewhere.png saves a final screenshot.
import { chromium } from "playwright";
import http from "node:http";
import { readFile } from "node:fs/promises";
import path from "node:path";

const DOCS = path.resolve(process.cwd(), "../../docs");
const PORT = 8399;
const MIME = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css" };
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

const URL_ = `http://localhost:${PORT}/yasgui.html`;
const SHOT = process.env.YASGUI_SHOT || "";

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
const errors = [];
page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
page.on("console", (m) => { if (m.type() === "error") errors.push(`console: ${m.text()}`); });

await page.goto(URL_, { waitUntil: "load" });

// 1. editor mounted, default endpoint prefilled
await page.waitForSelector(".cm-content", { timeout: 15000 });
const defaultEp = await page.inputValue("#endpoint");
console.log("endpoint prefilled:", defaultEp);
if (!defaultEp.includes("getty-ulan")) throw new Error("default endpoint is not getty-ulan");

// 2. run the default SELECT * LIMIT 10 against remote getty-ulan
await page.click("#runBtn");
await page.waitForSelector("table.rs tbody tr, .errbox", { timeout: 90000 });
if (await page.$(".errbox")) {
  throw new Error("query errored: " + (await page.textContent(".errbox")));
}
const rows1 = await page.$$eval("table.rs tbody tr", (r) => r.length);
const stats1 = await page.textContent("#resultStats");
console.log(`default query: ${rows1} rows — stats: ${stats1.trim()}`);
if (rows1 < 1) throw new Error("no rows from default query");
if (!/range request/.test(stats1)) throw new Error("stats line missing lazy-fetch traffic");

// 3. crafted Rembrandt query via catalog pick (replaces default query)
await page.click("#catalogBtn");
await page.waitForSelector(".catitem");
await page.click(".catitem"); // first = getty-ulan
const doc2 = await page.evaluate(() => document.querySelector(".cm-content").textContent);
if (!doc2.includes("teacherOf")) throw new Error("catalog pick did not install starter query");
await page.click("#runBtn");
await page.waitForFunction(() => {
  const s = document.getElementById("resultStats").textContent;
  return /results in|Took/.test(s) || document.querySelector(".errbox");
}, { timeout: 90000 });
if (await page.$(".errbox")) throw new Error("rembrandt query errored: " + (await page.textContent(".errbox")));
const rows2 = await page.$$eval("table.rs tbody tr", (r) => r.length).catch(() => 0);
const stats2 = await page.textContent("#resultStats");
console.log(`rembrandt query: ${rows2} rows — stats: ${stats2.trim()}`);
if (rows2 < 5) throw new Error("suspiciously few pupils of Rembrandt: " + rows2);
const firstRow = await page.$eval("table.rs tbody tr", (tr) => tr.textContent);
console.log("first row:", firstRow.trim().slice(0, 120));

// 4. Response view is JSON
await page.click('.rtab[data-view="response"]');
const raw = await page.textContent("pre.rawjson");
JSON.parse(raw);
console.log("response view: valid JSON,", raw.length, "chars");
await page.click('.rtab[data-view="table"]');

// 5. ASK query
await page.click(".cm-content");
await page.keyboard.press("Control+a");
await page.keyboard.type("ASK { ?s ?p ?o }");
await page.click("#runBtn");
await page.waitForSelector(".askbox", { timeout: 60000 });
const ask = await page.textContent(".askbox");
console.log("ask result:", ask.trim());
if (ask.trim() !== "true") throw new Error("ASK should be true");

// 6. new tab + tab switching keeps per-tab editor state
await page.click("#addTab");
const tabCount = await page.$$eval("#tabs .tab", (t) => t.length);
if (tabCount !== 2) throw new Error("expected 2 tabs, got " + tabCount);
await page.click("#tabs .tab"); // back to first
const backDoc = await page.evaluate(() => document.querySelector(".cm-content").textContent);
if (!backDoc.includes("ASK")) throw new Error("tab 1 lost its query on switch");
console.log("tabs: create + switch keep per-tab editor state ✓");

// 7. share link round-trip (hash → new tab), against a second dataset
await page.evaluate(() => {
  location.hash = "query=" + encodeURIComponent("SELECT ?s WHERE { ?s ?p ?o } LIMIT 3") +
    "&endpoint=" + encodeURIComponent("https://data.graphplaza.com/nidm/nidm.rete");
});
await page.reload({ waitUntil: "load" });
await page.waitForSelector(".cm-content", { timeout: 15000 });
const sharedEp = await page.inputValue("#endpoint");
if (!sharedEp.includes("nidm")) throw new Error("shared endpoint not restored: " + sharedEp);
await page.click("#runBtn");
await page.waitForSelector("table.rs tbody tr, .errbox", { timeout: 90000 });
if (await page.$(".errbox")) throw new Error("shared query errored: " + (await page.textContent(".errbox")));
const rows3 = await page.$$eval("table.rs tbody tr", (r) => r.length).catch(() => 0);
console.log("shared-link tab against nidm:", rows3, "rows");
if (rows3 !== 3) throw new Error("expected 3 rows, got " + rows3);

// 8. local-file mode: build a tiny .rete IN THE PAGE with the same wasm, then
// query it through the upload path (files Map → Graph over bytes).
const localOk = await page.evaluate(async () => {
  await wasm_bindgen({ module_or_path: b64ToBytes(RETE_WASM_B64).buffer });
  const bytes = wasm_bindgen.build(
    "<http://ex.org/a> <http://ex.org/p> \"hello local\" .\n" +
    "<http://ex.org/b> <http://ex.org/p> \"second\" .\n", "nt");
  const f = new File([bytes], "tiny.rete", { lastModified: 1 });
  // reuse the app's attach path
  const dt = new DataTransfer();
  dt.items.add(f);
  window.dispatchEvent(new DragEvent("drop", { dataTransfer: dt }));
  return true;
});
if (!localOk) throw new Error("local build/attach failed");
await page.waitForFunction(() => document.getElementById("endpoint").value.startsWith("file:"), { timeout: 10000 });
await page.click(".cm-content");
await page.keyboard.press("Control+a");
await page.keyboard.type("SELECT ?s ?o WHERE { ?s ?p ?o } ORDER BY ?o");
await page.click("#runBtn");
await page.waitForFunction(() => {
  const s = document.getElementById("resultStats").textContent;
  return /in-memory/.test(s) || document.querySelector(".errbox");
}, { timeout: 60000 });
if (await page.$(".errbox")) throw new Error("local-file query errored: " + (await page.textContent(".errbox")));
const localRows = await page.$$eval("table.rs tbody tr", (r) => r.length);
const localStats = await page.textContent("#resultStats");
console.log(`local uploaded file: ${localRows} rows — stats: ${localStats.trim()}`);
if (localRows !== 2) throw new Error("expected 2 rows from local file");

if (SHOT) {
  // leave a pretty state for the screenshot: back to the Rembrandt tab
  await page.evaluate(() => { location.hash = ""; });
  await page.reload({ waitUntil: "load" });
  await page.waitForSelector(".cm-content");
  await page.click("#tabs .tab");
  await page.click("#runBtn");
  await page.waitForSelector("table.rs tbody tr, .askbox, .errbox", { timeout: 90000 });
  await page.screenshot({ path: SHOT, fullPage: false });
  console.log("screenshot:", SHOT);
}

if (errors.length) {
  console.log("--- page errors ---");
  for (const e of errors) console.log(e);
  throw new Error(`${errors.length} page error(s)`);
}
await browser.close();
server.close();
console.log("ALL CHECKS PASSED");
