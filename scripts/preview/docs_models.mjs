// Social-card models for the documentation and application pages.
//
// The models are read back OUT of the shipped HTML rather than re-derived from
// the sources: docgen (crates/docgen/src/main.rs) decides each rendered page's
// og:title and og:description, and inject_og.mjs decides them for the pre-built
// apps. Reading the <head> that actually ships keeps exactly one source of truth,
// so a card image can never describe something different from the tags beside it.
//
// A page opts into a card simply by pointing og:image at og/doc/<slug>.png.
import fs from "node:fs";
import path from "node:path";

// Vendored / generated trees that are not ours to describe.
const SKIP_DIRS = new Set(["og", "img", "jupyterlite", "demo-iswc2026", "superpowers", "engine", "examples", "q", "d"]);
// Only the <head> is needed, and some of these pages are megabytes of inlined wasm.
const HEAD_BYTES = 96 * 1024;

function readHead(file) {
  const fd = fs.openSync(file, "r");
  try {
    const buf = Buffer.alloc(HEAD_BYTES);
    const read = fs.readSync(fd, buf, 0, HEAD_BYTES, 0);
    return buf.slice(0, read).toString("utf8");
  } finally {
    fs.closeSync(fd);
  }
}

const metaContent = (html, key) => {
  const re = new RegExp(`<meta\\s+(?:property|name)=["']${key}["']\\s+content=["']([^"']*)["']`, "i");
  const m = re.exec(html);
  return m ? m[1] : "";
};

const unescapeAttr = (s) => String(s)
  .replace(/&quot;/g, '"').replace(/&#39;/g, "'")
  .replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");

function walk(dir, base, out) {
  for (const name of fs.readdirSync(dir)) {
    const full = path.join(dir, name);
    const stat = fs.statSync(full);
    if (stat.isDirectory()) {
      if (SKIP_DIRS.has(name)) continue;
      walk(full, base, out);
    } else if (name.endsWith(".html")) {
      out.push(path.relative(base, full).replace(/\\/g, "/"));
    }
  }
}

export function docModels(root) {
  const docsDir = path.join(root, "docs");
  const pages = [];
  walk(docsDir, docsDir, pages);
  const models = [];
  for (const page of pages) {
    const html = readHead(path.join(docsDir, page));
    const image = metaContent(html, "og:image");
    const slug = (/\/og\/doc\/([^"'/]+)\.png$/.exec(image) || [])[1];
    if (!slug) continue;
    models.push({
      kind: "doc",
      slug,
      dir: "doc",
      page,
      title: unescapeAttr(metaContent(html, "og:title")).replace(/\s*·\s*rete( docs)?$/, ""),
      summary: unescapeAttr(metaContent(html, "og:description")),
      section: unescapeAttr(metaContent(html, "rete:section")),
    });
  }
  models.sort((a, b) => a.slug.localeCompare(b.slug));
  return models;
}
