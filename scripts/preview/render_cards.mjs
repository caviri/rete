// Render every social card to a 1200x630 PNG under docs/og/.
//
// The card markup comes from card.mjs — the same module the share pages use — so
// the image a crawler unfurls and the page a visitor lands on cannot drift apart.
// Screenshots are taken from a data: URL (no server needed); the captured result
// thumbnails for graph/map/timeline views are inlined as base64 so the render is
// self-contained.
//
//   node scripts/preview/render_cards.mjs [--only=<substr>] [--force] [--docs]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { execFileSync } from "node:child_process";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const { chromium } = createRequire(path.join(ROOT, "tests", "gate", "package.json"))("playwright");
const { buildModels, ogHtml, ogImagePath, OG_W, OG_H } = await import(path.join(ROOT, "scripts", "preview", "card.mjs"));
const { docModels } = await import(path.join(ROOT, "scripts", "preview", "docs_models.mjs"));

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const hit = args.find((a) => a.startsWith(`--${name}=`));
  return hit === undefined ? fallback : hit.slice(name.length + 3);
};
const ONLY = flag("only", "");
const FORCE = args.includes("--force");
const CONCURRENCY = Math.max(1, Number(flag("concurrency", 4)));

// pngquant turns these flat, few-colour cards from ~90 KB into ~25 KB with no
// visible loss — worth it when ~800 of them ship in the repo. Optional: without
// it the pipeline still produces correct (larger) PNGs.
let PNGQUANT = null;
for (const candidate of ["pngquant", "/usr/bin/pngquant"]) {
  try { execFileSync(candidate, ["--version"], { stdio: "pipe" }); PNGQUANT = candidate; break; } catch { /* not installed */ }
}
if (!PNGQUANT) console.log("note: pngquant not found — writing unquantized PNGs");

function optimize(file) {
  if (!PNGQUANT) return;
  try {
    execFileSync(PNGQUANT, ["--force", "--skip-if-larger", "--quality=65-92", "--speed", "1",
      "--strip", "--output", file, "--", file], { stdio: "pipe" });
  } catch { /* --skip-if-larger exits non-zero when it declines; keep the original */ }
}

/** Inline a captured result thumbnail so the card renders with no network. */
function shotDataUri(model) {
  if (!model.answer || !model.answer.shot) return "";
  const file = path.join(ROOT, model.answer.shot);
  if (!fs.existsSync(file)) return "";
  return `data:image/png;base64,${fs.readFileSync(file).toString("base64")}`;
}

// One manifest of markup hashes instead of a sidecar file per card: it is what
// makes a re-render incremental, and 800 eight-byte files next to 800 PNGs would
// be worse than the problem.
const MANIFEST = path.join(ROOT, "docs", "og", "cards.json");
function readManifest() {
  if (!fs.existsSync(MANIFEST)) return {};
  try { return JSON.parse(fs.readFileSync(MANIFEST, "utf8")).cards || {}; } catch { return {}; }
}

async function main() {
  const models = [...buildModels(ROOT), ...docModels(ROOT)]
    .filter((m) => !ONLY || m.slug.includes(ONLY) || m.dir.includes(ONLY));
  console.log(`render: ${models.length} card(s)`);

  const manifest = readManifest();
  const browser = await chromium.launch();
  const queue = [...models];
  let done = 0, written = 0, skipped = 0;
  const workers = Array.from({ length: Math.min(CONCURRENCY, queue.length) }, async () => {
    const page = await browser.newPage({
      viewport: { width: OG_W, height: OG_H },
      deviceScaleFactor: 1,
    });
    for (;;) {
      const model = queue.shift();
      if (!model) break;
      const rel = ogImagePath(model);
      const out = path.join(ROOT, "docs", rel);
      done++;
      const html = ogHtml(model, { shotSrc: shotDataUri(model) });
      const stamp = hash(html);
      // The card is a pure function of its markup, so an unchanged hash means an
      // unchanged image — no need to re-render (or re-churn) it.
      if (!FORCE && fs.existsSync(out) && manifest[rel] === stamp) { skipped++; continue; }
      fs.mkdirSync(path.dirname(out), { recursive: true });
      await page.setContent(html, { waitUntil: "load" });
      // Give the emoji font and layout one frame to settle.
      await page.waitForTimeout(60);
      await page.screenshot({ path: out, clip: { x: 0, y: 0, width: OG_W, height: OG_H } });
      optimize(out);
      manifest[rel] = stamp;
      written++;
      if (done % 50 === 0) console.log(`  ${done}/${models.length} (${written} written, ${skipped} unchanged)`);
    }
    await page.close();
  });
  await Promise.all(workers);
  await browser.close();
  const ordered = Object.keys(manifest).sort().reduce((acc, k) => (acc[k] = manifest[k], acc), {});
  fs.writeFileSync(MANIFEST, JSON.stringify({ generator: "scripts/preview/render_cards.mjs", cards: ordered }, null, 1) + "\n");
  console.log(`render: ${written} written, ${skipped} unchanged -> docs/og/`);
}

function hash(text) {
  // Small, dependency-free FNV-1a; only needs to detect change, not resist attack.
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) { h ^= text.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return h.toString(16);
}

main().catch((e) => { console.error(e); process.exit(1); });
