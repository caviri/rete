// Render the landing page's link preview: scripts/preview/landing_card.html
// -> site/landing/og.png (1200 x 630). site/index.html points og:image at it.
//
//   scripts/preview/run.sh landing
//   node scripts/preview/landing_card.mjs [--out=<png>] [--seed=<n>]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const { chromium } = createRequire(path.join(ROOT, "tests", "gate", "package.json"))("playwright");

const arg = (name, fallback) => {
  const hit = process.argv.slice(2).find((a) => a.startsWith(`--${name}=`));
  return hit === undefined ? fallback : hit.slice(name.length + 3);
};
const OUT = path.resolve(arg("out", path.join(ROOT, "site", "landing", "og.png")));
const SEED = arg("seed", "");

const page = pathToFileURL(path.join(ROOT, "scripts", "preview", "landing_card.html")).href + (SEED ? `?seed=${SEED}` : "");
const browser = await chromium.launch();
try {
  const tab = await browser.newPage({ viewport: { width: 1200, height: 630 }, deviceScaleFactor: 1 });
  await tab.goto(page, { waitUntil: "load" });
  await tab.waitForSelector("body[data-ready]");
  const clashes = await tab.evaluate(() => document.body.dataset.clashes);
  if (clashes) console.log(`warning: labels overlap (${clashes}); try another --seed`);
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  await tab.screenshot({ path: OUT });
} finally {
  await browser.close();
}

// Same quantizing as the other cards, when pngquant is there.
try {
  execFileSync("pngquant", ["--force", "--skip-if-larger", "--quality=70-95", "--speed", "1",
    "--strip", "--output", OUT, "--", OUT], { stdio: "pipe" });
} catch { /* not installed, or it declined: keep the full PNG */ }
console.log(`${path.relative(ROOT, OUT)}  ${fs.statSync(OUT).size} bytes`);
