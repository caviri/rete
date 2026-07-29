// Static check: every shareable URL has a link preview, and every preview points
// at a file that exists.
//
// The failure this guards against is silent. A new catalog example, a renamed
// dataset or a re-run of docgen produces a page whose og:image 404s — nothing
// breaks, no test goes red, and the link just unfurls blank in every chat client
// for as long as nobody notices. So the contract is checked here:
//
//   1. every catalog example/dataset has a share page under docs/q | docs/d,
//   2. every page that declares an og:image under og/ has that PNG on disk,
//   3. every og:image / og:url is absolute (a relative one is dropped silently),
//   4. every docgen-rendered page and pre-built app page carries the tags.
//
// Run standalone:  node tests/gate/checks/check_social_previews.mjs
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..");
const DOCS = path.join(ROOT, "docs");
const { buildModels, sharePath, ogImagePath } = await import(path.join(ROOT, "scripts", "preview", "card.mjs"));
const { APP_PAGES } = await import(path.join(ROOT, "scripts", "preview", "inject_og.mjs"));

const failures = [];
const fail = (what) => failures.push(what);

const HEAD_BYTES = 96 * 1024;
function head(file) {
  const fd = fs.openSync(file, "r");
  try {
    const buf = Buffer.alloc(HEAD_BYTES);
    const read = fs.readSync(fd, buf, 0, HEAD_BYTES, 0);
    return buf.slice(0, read).toString("utf8");
  } finally { fs.closeSync(fd); }
}
const meta = (html, key) => {
  const m = new RegExp(`<meta\\s+(?:property|name)=["']${key}["']\\s+content=["']([^"']*)["']`, "i").exec(html);
  return m ? m[1] : "";
};

// ---- 1. the playground's share pages + cards ----
const models = buildModels(ROOT);
let missingPages = 0, missingImages = 0;
for (const model of models) {
  if (!fs.existsSync(path.join(DOCS, sharePath(model)))) {
    if (missingPages++ < 5) fail(`share page missing: docs/${sharePath(model)} (run scripts/preview/run.sh pages)`);
  }
  if (!fs.existsSync(path.join(DOCS, ogImagePath(model)))) {
    if (missingImages++ < 5) fail(`card image missing: docs/${ogImagePath(model)} (run scripts/preview/run.sh cards)`);
  }
}
if (missingPages > 5) fail(`… and ${missingPages - 5} more missing share pages`);
if (missingImages > 5) fail(`… and ${missingImages - 5} more missing card images`);

// ---- 2/3. every declared preview resolves, and is absolute ----
const SKIP_DIRS = new Set(["og", "img", "jupyterlite", "demo-iswc2026", "superpowers", "engine", "examples"]);
const pages = [];
(function walk(dir) {
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    if (fs.statSync(full).isDirectory()) { if (!SKIP_DIRS.has(name)) walk(full); }
    else if (name.endsWith(".html")) pages.push(path.relative(DOCS, full).replace(/\\/g, "/"));
  }
})(DOCS);

const BASE = "https://caviri.github.io/rete/";
let checkedImages = 0;
for (const page of pages) {
  const html = head(path.join(DOCS, page));
  const image = meta(html, "og:image");
  if (!image) continue;
  if (!/^https?:\/\//.test(image)) { fail(`${page}: og:image is relative (${image}) — unfurlers drop it`); continue; }
  const url = meta(html, "og:url");
  if (url && !/^https?:\/\//.test(url)) fail(`${page}: og:url is relative (${url})`);
  if (!image.startsWith(BASE)) { fail(`${page}: og:image is off-site (${image})`); continue; }
  const local = path.join(DOCS, image.slice(BASE.length));
  if (!fs.existsSync(local)) fail(`${page}: og:image 404s — docs/${image.slice(BASE.length)} does not exist`);
  else checkedImages++;
  if (!meta(html, "og:title")) fail(`${page}: og:image without og:title`);
  if (!meta(html, "og:description")) fail(`${page}: og:image without og:description`);
  if (meta(html, "twitter:card") !== "summary_large_image") fail(`${page}: twitter:card is not summary_large_image`);
}

// ---- 4. coverage: the rendered docs and the pre-built apps ----
for (const md of fs.readdirSync(DOCS).filter((f) => f.endsWith(".md"))) {
  const page = md.replace(/\.md$/, ".html");
  const file = path.join(DOCS, page);
  if (!fs.existsSync(file)) continue;             // a source without a rendered page
  const html = head(file);
  // Only pages docgen actually renders carry the section marker; others (hand
  // written or app pages) are covered by the APP_PAGES sweep below.
  if (!meta(html, "rete:section")) continue;
  if (!meta(html, "og:image")) fail(`${page}: rendered docs page has no og:image (re-run docgen)`);
}
for (const entry of APP_PAGES) {
  const file = path.join(DOCS, entry.page);
  if (!fs.existsSync(file)) continue;
  const html = head(file);
  if (!meta(html, "og:image")) {
    fail(`${entry.page}: app page lost its social tags (run scripts/preview/run.sh inject)`);
  }
}

const verdict = failures.length ? "FAIL" : "PASS";
console.log(JSON.stringify({
  verdict,
  sharePages: models.length,
  pagesWithPreview: checkedImages,
  failures: failures.slice(0, 20),
}, null, 2));
process.exit(failures.length ? 1 : 0);
