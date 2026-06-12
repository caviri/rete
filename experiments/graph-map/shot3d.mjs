// Screenshots of the deck.gl 3D viewer: top view, side (rotate 90), side+depth.
// Usage: node shot3d.mjs <url> <out-prefix>
import { chromium } from "playwright";

const URL = process.argv[2];
const PREFIX = process.argv[3] || "out/shot3d";

const browser = await chromium.launch({
  headless: false,
  args: ["--use-gl=angle", "--use-angle=swiftshader", "--ignore-gpu-blocklist"],
});
const page = await browser.newPage({ viewport: { width: 1280, height: 860 }, deviceScaleFactor: 2 });
page.on("console", (m) => { if (m.type() === "error") console.log("PAGE-ERR:", m.text()); });
page.on("pageerror", (e) => console.log("PAGEERROR:", e.message));

await page.goto(URL, { waitUntil: "domcontentloaded" });
await page.waitForSelector("canvas", { timeout: 60000 });
await page.waitForTimeout(5000); // data fetch + first render

await page.screenshot({ path: `${PREFIX}-top.png` });
console.log("wrote top");

await page.click("#rot");          // rotate to orthographic side view
await page.waitForTimeout(2800);
await page.screenshot({ path: `${PREFIX}-side.png` });
console.log("wrote side");

await page.$eval("#depth", (el) => { el.value = 45; el.dispatchEvent(new Event("input")); });
await page.waitForTimeout(1600);
await page.screenshot({ path: `${PREFIX}-side-depth.png` });
console.log("wrote side-depth");

await browser.close();
process.exit(0);
