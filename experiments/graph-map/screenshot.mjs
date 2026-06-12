// Scripted screenshots of a graph-map / topic-map viewer for the docs.
// Usage: node screenshot.mjs <url> <out-prefix> <zoom1,zoom2,...>
// Drives window._map (exposed by the viewers) to fixed zoom levels and shoots.
import { chromium } from "playwright";

const URL = process.argv[2];
const PREFIX = process.argv[3] || "shot";
const ZOOMS = (process.argv[4] || "0.5,3,6").split(",").map(Number);

// headless:false under xvfb gives real WebGL (MapLibre won't paint on the
// deprecated headless software-GL path). Run via: xvfb-run -a node screenshot.mjs
const browser = await chromium.launch({
  headless: false,
  args: ["--use-gl=angle", "--use-angle=swiftshader", "--ignore-gpu-blocklist"],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2 });
page.on("console", (m) => { if (m.type() === "error") console.log("PAGE-ERR:", m.text()); });
page.on("response", (r) => { if (r.status() >= 400) console.log("HTTP", r.status(), r.url()); });

await page.goto(URL, { waitUntil: "domcontentloaded" });
await page.waitForFunction(() => window._map && window._map.isStyleLoaded(), { timeout: 60000 });
await page.evaluate(() => { window._errs = []; window._map.on("error", (e) => window._errs.push(String((e.error && e.error.message) || e.type))); });
await page.waitForTimeout(3000);

const diag = await page.evaluate(() => {
  const m = window._map; const sl = m.getStyle().layers.map((l) => l.id);
  const srcLayer = (m.getSource("graph") || m.getSource("t")) ? (m.getStyle().sources.graph ? "graph" : "t") : "?";
  let src = 0; try { src = m.querySourceFeatures(srcLayer, { sourceLayer: srcLayer === "graph" ? "graph" : "topics" }).length; } catch (e) { src = "ERR:" + e.message; }
  return { zoom: m.getZoom(), loaded: m.areTilesLoaded(), rendered: m.queryRenderedFeatures().length,
           sourceFeatures: src, layers: sl, errs: window._errs.slice(0, 6) };
});
console.log("DIAG", JSON.stringify(diag));

for (const z of ZOOMS) {
  await page.evaluate((zz) => new Promise((res) => {
    const m = window._map;
    let done = false;
    const fin = () => { if (!done) { done = true; res(); } };
    m.once("idle", fin);
    m.jumpTo({ center: [0, 0], zoom: zz });
    setTimeout(fin, 5000);
  }), z);
  await page.waitForTimeout(1200);
  const n = await page.evaluate(() => window._map.queryRenderedFeatures().length);
  const file = `${PREFIX}-z${String(z).replace(".", "_")}.png`;
  await page.screenshot({ path: file });
  console.log("wrote", file, "rendered-features", n);
}

await browser.close();
process.exit(0); // headed+xvfb can hang on teardown otherwise
