// Give the pre-built application pages the same social tags docgen writes.
//
// docs/playground.html, docs/yasgui.html, docs/atlas-app.html … are not rendered
// from Markdown: each is produced by its own build script from a template in
// web/. So the tags are injected into a marked block, and — where a template
// exists — into the TEMPLATE as well, so the next rebuild of that page keeps
// them instead of silently dropping the preview.
//
// Descriptions are inherited from each app's guide page wherever there is one
// (docgen already derived that text from the guide's Markdown), so there is one
// source of truth for what a page says about itself.
//
//   node scripts/preview/inject_og.mjs [--check]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const { DEFAULT_BASE } = await import(path.join(ROOT, "scripts", "preview", "card.mjs"));
const BASE = DEFAULT_BASE;
const CHECK = process.argv.includes("--check");

const START = "<!-- og:start (scripts/preview/inject_og.mjs) -->";
const END = "<!-- og:end -->";

/**
 * The interactive pages, in nav order. `guide` names the rendered guide whose
 * description this page inherits; `description` is only spelled out for the few
 * apps that have no guide page of their own.
 */
export const APP_PAGES = [
  { page: "playground.html", template: "web/playground.template.html", section: "Explore in the browser",
    title: "rete playground — query knowledge graphs in your browser",
    guide: "playground-guide.html" },
  { page: "yasgui.html", template: "web/yasgui.template.html", section: "Explore in the browser",
    title: "yasgui·wasm — a SPARQL IDE with no server",
    guide: "yasgui-guide.html" },
  { page: "explorer.html", template: "web/explorer.template.html", section: "Explore in the browser",
    title: "rete explorer — walk a graph node by node",
    description: "A single-file network explorer: open any .rete file over HTTP range and walk the graph node by node, following edges outward. No server, no index build, no download." },
  { page: "atlas-app.html", template: "web/atlas.template.html", section: "Explore in the browser",
    title: "Historical atlas — SPARQL over 84 map layers",
    guide: "atlas.html" },
  { page: "ask-browser.html", section: "Explore in the browser",
    title: "Ask the graph — knowledge-graph search in your browser",
    guide: "ask-the-graph.html" },
  { page: "wcfinal.html", section: "Explore in the browser",
    title: "Football — replay a match from a graph",
    guide: "football.html" },
  { page: "subtitles.html", section: "Explore in the browser",
    title: "Subtitle timeline — 20 languages, one graph",
    guide: "subtitles-guide.html" },
  { page: "anatomy.html", section: "Explore in the browser",
    title: "Z-Anatomy — the human body as a 3D graph",
    guide: "anatomy-guide.html" },
  // building-guide.md and bim-pair-guide.md are not in docgen's nav, so their
  // rendered pages carry no tags to inherit — these two spell it out.
  { page: "building.html", section: "Explore in the browser",
    title: "FZK-Haus — a building as a 3D knowledge graph",
    description: "A three.js explorer over the fzk-haus knowledge graph: pick any wall, door, window, slab or room and see where it sits — its floor, the rooms it encloses, the rooms next to it, and everything within reach in 3D — with real SPARQL and geo3 (GeoSPARQL in 3D) running in the browser." },
  { page: "bim-pair.html", section: "Explore in the browser",
    title: "Architecture vs Structure — one building, two BIM models",
    description: "The same building modelled twice by one team of the TUM BIM Project course: an architectural model (walls, curtain walls, doors, windows) and a structural model (beams, columns, slabs). Toggle between them, or overlay the skeleton inside a translucent envelope — all queried live from a .rete graph." },
  { page: "lombardi.html", template: "web/lombardi.template.html", section: "Explore in the browser",
    title: "Mark Lombardi — network drawings in ink",
    guide: "lombardi-guide.html" },
  { page: "neuro-showcase.html", section: "Explore in the browser",
    title: "Neuromorphology — 3D neurons and astrocytes",
    guide: "neuro-showcase-guide.html" },
  { page: "jslab.html", section: "Explore in the browser",
    title: "JS lab — rete × D3 in the page",
    guide: "jslab-guide.html" },
  { page: "webgpu.html", section: "Explore in the browser",
    title: "WebGPU coherence — reasoning on the GPU",
    guide: "webgpu-guide.html" },
  { page: "pitch.html", template: "web/pitch.template.html", section: "Start here",
    title: "rete — cloud-native, range-queryable RDF graph files",
    description: "One immutable file on any static host, queried by HTTP range reads: publish a knowledge graph once and let anyone query it from a browser, with no server, no database and no download." },
  { page: "plaza/index.html", section: "Explore in the browser",
    title: "Plaza — the rete dataset gallery",
    guide: "plaza-guide.html" },
  { page: "graph-map/viewer.html", section: "Explore in the browser",
    title: "Graph-map — a knowledge graph as a slippy map",
    guide: "graph-map.html" },
  { page: "graph-map/viewer-topics.html", section: "Explore in the browser",
    title: "Topic map — LDA topics as a slippy map",
    guide: "graph-map.html" },
  { page: "graph-map/viewer-3d.html", section: "Explore in the browser",
    title: "Graph-map in 3D — deck.gl",
    guide: "graph-map.html" },
  { page: "graph-map/viewer-3d-three.html", section: "Explore in the browser",
    title: "Graph-map in 3D — three.js",
    guide: "graph-map.html" },
];

/** The card image slug for a page — also how docs_models.mjs finds these pages. */
export const appSlug = (page) => page.replace(/\.html$/, "").replace(/\//g, "-");

const attr = (text) => String(text || "")
  .replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

function metaContent(html, key) {
  const re = new RegExp(`<meta\\s+(?:property|name)=["']${key}["']\\s+content=["']([^"']*)["']`, "i");
  const m = re.exec(html);
  return m ? m[1] : "";
}

function block(entry) {
  const slug = appSlug(entry.page);
  let description = entry.description || "";
  if (!description && entry.guide) {
    const guide = path.join(ROOT, "docs", entry.guide);
    if (fs.existsSync(guide)) {
      description = metaContent(fs.readFileSync(guide, "utf8").slice(0, 200000), "og:description")
        .replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, "&");
    }
  }
  if (!description) throw new Error(`${entry.page}: no description (guide ${entry.guide} missing?)`);
  const url = `${BASE}${entry.page}`;
  const image = `${BASE}og/doc/${slug}.png`;
  return [
    START,
    `<meta name="description" content="${attr(description)}" />`,
    `<link rel="canonical" href="${url}" />`,
    `<meta property="og:type" content="website" />`,
    `<meta property="og:site_name" content="rete" />`,
    `<meta property="og:title" content="${attr(entry.title)}" />`,
    `<meta property="og:description" content="${attr(description)}" />`,
    `<meta property="og:url" content="${url}" />`,
    `<meta property="og:image" content="${image}" />`,
    `<meta property="og:image:width" content="1200" />`,
    `<meta property="og:image:height" content="630" />`,
    `<meta property="og:image:alt" content="${attr(entry.title)}" />`,
    `<meta name="twitter:card" content="summary_large_image" />`,
    `<meta name="twitter:title" content="${attr(entry.title)}" />`,
    `<meta name="twitter:description" content="${attr(description)}" />`,
    `<meta name="twitter:image" content="${image}" />`,
    `<meta name="rete:section" content="${attr(entry.section)}" />`,
    END,
  ].map((line, i) => (i === 0 ? `  ${line}` : `  ${line}`)).join("\n");
}

/** Idempotent: replace the marked block, or insert one right after <head>. */
function patch(html, marked) {
  const startAt = html.indexOf(START);
  if (startAt !== -1) {
    const endAt = html.indexOf(END, startAt);
    if (endAt === -1) throw new Error("found an og:start marker with no og:end");
    const from = html.lastIndexOf("\n", startAt) + 1;
    return html.slice(0, from) + marked + html.slice(endAt + END.length);
  }
  const head = /<head[^>]*>/i.exec(html);
  if (!head) throw new Error("no <head> to inject into");
  const at = head.index + head[0].length;
  return `${html.slice(0, at)}\n${marked}${html.slice(at)}`;
}

let changed = 0, missing = [], stale = [];
for (const entry of APP_PAGES) {
  const marked = block(entry);
  for (const rel of [path.join("docs", entry.page), entry.template].filter(Boolean)) {
    const file = path.join(ROOT, rel);
    if (!fs.existsSync(file)) { missing.push(rel); continue; }
    const html = fs.readFileSync(file, "utf8");
    const next = patch(html, marked);
    if (next === html) continue;
    if (CHECK) { stale.push(rel); continue; }
    fs.writeFileSync(file, next);
    changed++;
    console.log(`  patched ${rel}`);
  }
}
if (missing.length) console.log(`note: not found (skipped): ${missing.join(", ")}`);
if (CHECK) {
  if (stale.length) {
    console.error(`inject_og --check: ${stale.length} file(s) missing or out-of-date social tags:\n  ${stale.join("\n  ")}`);
    process.exit(1);
  }
  console.log(`inject_og --check: all ${APP_PAGES.length} app page(s) carry current social tags`);
} else {
  console.log(`inject_og: ${changed} file(s) updated across ${APP_PAGES.length} app page(s)`);
}
