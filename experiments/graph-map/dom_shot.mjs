// Generic DOM page shot: wait for #status "ready", run a query, screenshot.
// node dom_shot.mjs <url> <out.png> <query>
import { chromium } from "playwright";
const URL = process.argv[2], OUT = process.argv[3] || "out/ask.png", Q = process.argv[4] || "neural networks for images";
const b = await chromium.launch();
const p = await b.newPage({ viewport: { width: 900, height: 820 }, deviceScaleFactor: 2 });
p.on("console", (m) => console.log("C", m.type(), m.text().slice(0, 160)));
p.on("pageerror", (e) => console.log("PAGEERR", e.message));
p.on("response", (r) => { if (r.status() >= 400) console.log("HTTP", r.status(), r.url().slice(0, 130)); });
p.on("requestfailed", (r) => console.log("REQFAIL", r.url().slice(0, 130), r.failure()?.errorText));
await p.goto(URL, { waitUntil: "domcontentloaded" });
await p.waitForFunction(() => /ready/.test(document.getElementById("status")?.textContent || ""), { timeout: 180000 });
console.log("READY");
await p.fill("#q", Q);
await p.click("#go");
await p.waitForFunction(() => (document.getElementById("out")?.children.length || 0) > 0, { timeout: 60000 });
await p.waitForTimeout(800);
await p.screenshot({ path: OUT, fullPage: true });
console.log("SHOT", OUT);
const node = await p.$("[data-node]");           // explore the first ranked result
if (node) { await node.click(); await p.waitForTimeout(700); await p.screenshot({ path: OUT.replace(/\.png$/, "-explore.png"), fullPage: true }); console.log("SHOT explore"); }
await b.close();
process.exit(0);
