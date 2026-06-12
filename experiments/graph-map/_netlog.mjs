import { chromium } from "playwright";
const URL = process.argv[2] || "http://localhost:8090/viewer.html";
const b = await chromium.launch();
const p = await b.newPage();
p.on("request", (r) => { const u = r.url(); if (u.includes("pmtiles") || u.endsWith(".json")) console.log("REQ", r.method(), (r.headers().range || "-"), u); });
p.on("response", (r) => { const u = r.url(); if (u.includes("pmtiles") || u.endsWith(".json")) console.log("RES", r.status(), u); });
p.on("console", (m) => console.log("CONSOLE", m.type(), m.text()));
p.on("pageerror", (e) => console.log("PAGEERROR", e.message));
await p.goto(URL, { waitUntil: "networkidle" });
await p.waitForTimeout(4000);
const info = await p.evaluate(() => {
  const m = window._map;
  return { srcLoaded: !!m.getSource("graph"), tiles: m.areTilesLoaded(), zoom: m.getZoom() };
});
console.log("INFO", JSON.stringify(info));
await b.close();
