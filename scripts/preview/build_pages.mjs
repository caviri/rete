// Build the shareable landing pages for every catalog example and dataset.
//
// WHY THESE EXIST. The playground keeps its whole state in the URL fragment
// (`playground.html#dataset=hugging-face&load=lazy&mode=sparql&ex=1`). A fragment
// is never sent to the server and no crawler executes the page's JavaScript, so
// every one of those links unfurls as the same generic playground card — the
// question, the dataset and the answer are all invisible to a link preview.
//
// So each example gets its own real URL under docs/q/ (datasets under docs/d/)
// carrying its own Open Graph tags and pre-rendered card image, which then
// forwards a human to the exact playground deep link it describes. The
// playground's Share button hands out these URLs.
//
//   node scripts/preview/build_pages.mjs [--base=https://caviri.github.io/rete/]
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..", "..");
const card = await import(path.join(ROOT, "scripts", "preview", "card.mjs"));
const {
  buildModels, cardInner, socialTitle, socialDescription, ogImagePath, sharePath,
  esc, plain, answerHeadline, answerTiming, answerNetwork, CARD_CSS, T, DEFAULT_BASE,
} = card;

const args = process.argv.slice(2);
const BASE = (args.find((a) => a.startsWith("--base=")) || `--base=${DEFAULT_BASE}`).slice(7).replace(/\/?$/, "/");

// ---------------------------------------------------------------- the page ----

const PAGE_CSS = `
${CARD_CSS}
html{background:${T.paper}}
body{margin:0;font-family:${T.sans};color:${T.ink};background:${T.paper};
  display:flex;align-items:flex-start;justify-content:center;min-height:100vh;padding:32px 24px}
.wrap{width:100%;max-width:1200px}
/* The card is authored at a fixed 1200x630 so the PNG and the page cannot
   diverge; on a narrow screen it is scaled down as a whole rather than reflowed. */
.stage{width:1200px;height:630px;transform-origin:top left;border-radius:18px;overflow:hidden;
  border:1px solid ${T.line};box-shadow:0 24px 60px rgba(32,47,41,.13)}
.actions{display:flex;flex-wrap:wrap;gap:14px;align-items:center;margin-top:26px}
.go{display:inline-block;background:${T.accent};color:${T.onAccent};text-decoration:none;
  font-size:18px;font-weight:600;padding:14px 26px;border-radius:10px}
.go:hover{background:${T.accentDark}}
.alt{color:${T.muted};font-size:15px}
.alt a{color:${T.accent}}
.q{margin-top:22px;background:${T.surface};border:1px solid ${T.line};border-radius:12px;
  padding:16px 18px;font-family:${T.mono};font-size:14px;line-height:1.55;white-space:pre-wrap;
  overflow-x:auto;color:${T.ink}}
.q h2{font-family:${T.sans};font-size:12px;letter-spacing:.12em;text-transform:uppercase;
  color:${T.muted};margin:0 0 10px;font-weight:600}
@media (prefers-color-scheme: dark){
  html,body{background:#0f1512;color:#dde8e2}
  .q{background:#171f1b;border-color:#2a352f;color:#dde8e2}
  .alt{color:#93a69d}
}
`;

// Scale the fixed-size card to whatever viewport the visitor actually has.
const FIT_JS = `
(function(){
  var stage=document.querySelector(".stage"),wrap=document.querySelector(".wrap");
  if(!stage||!wrap) return;
  function fit(){
    var s=Math.min(1,wrap.clientWidth/1200);
    stage.style.transform="scale("+s+")";
    stage.style.marginBottom=(630*s-630)+"px";
  }
  fit(); addEventListener("resize",fit);
})();`;

/**
 * Forward a human to the playground; leave crawlers (and `?stay=1`, which the
 * card renderer and anyone inspecting the preview use) on the page.
 * `replace` keeps the share URL out of the history so Back leaves the site
 * instead of bouncing between the two pages.
 */
const REDIRECT_JS = (target) => `
(function(){
  try{
    if(/[?&]stay=1\\b/.test(location.search)) return;
    location.replace(${JSON.stringify(target)});
  }catch(e){}
})();`;

function jsonLd(model) {
  const url = `${BASE}${sharePath(model)}`;
  if (model.kind === "dataset") {
    const d = model.dataset;
    return {
      "@context": "https://schema.org",
      "@type": "Dataset",
      name: `${d.key} — ${d.name}`,
      description: plain(d.description, 900),
      url,
      license: d.license || undefined,
      creator: { "@type": "Organization", name: "rete" },
      distribution: [{
        "@type": "DataDownload",
        encodingFormat: "application/octet-stream",
        contentUrl: `https://data.graphplaza.com/${d.key}/${d.key}.rete`,
      }],
      isAccessibleForFree: true,
    };
  }
  return {
    "@context": "https://schema.org",
    "@type": "WebPage",
    name: model.title,
    description: socialDescription(model),
    url,
    isPartOf: { "@type": "WebSite", name: "rete", url: BASE },
    about: { "@type": "Dataset", name: model.dataset.key, url: `${BASE}d/${model.dataset.key}.html` },
  };
}

function pageHtml(model) {
  const title = socialTitle(model);
  const description = socialDescription(model);
  const image = `${BASE}${ogImagePath(model)}`;
  const url = `${BASE}${sharePath(model)}`;
  // Share pages sit one directory deep, so the playground link is a level up.
  const target = `../${model.target}`;
  const answer = model.answer;
  const net = answer ? answerNetwork(answer) : null;
  const facts = [
    answer ? answerHeadline(answer) : "",
    answer ? answerTiming(answer) : "",
    net ? `${net.fetched} of ${net.total} read over HTTP range` : "",
  ].filter(Boolean).join(" · ");

  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="color-scheme" content="light dark" />
<title>${esc(title)}</title>
<meta name="description" content="${esc(description)}" />
<link rel="canonical" href="${esc(url)}" />
<meta property="og:type" content="website" />
<meta property="og:site_name" content="rete" />
<meta property="og:title" content="${esc(title)}" />
<meta property="og:description" content="${esc(description)}" />
<meta property="og:url" content="${esc(url)}" />
<meta property="og:image" content="${esc(image)}" />
<meta property="og:image:type" content="image/png" />
<meta property="og:image:width" content="1200" />
<meta property="og:image:height" content="630" />
<meta property="og:image:alt" content="${esc(title)}" />
<meta name="twitter:card" content="summary_large_image" />
<meta name="twitter:title" content="${esc(title)}" />
<meta name="twitter:description" content="${esc(description)}" />
<meta name="twitter:image" content="${esc(image)}" />
<script type="application/ld+json">${JSON.stringify(jsonLd(model))}</script>
<link rel="stylesheet" href="../preview.css" />
<script>${REDIRECT_JS(target)}</script>
</head>
<body>
<div class="wrap">
  <div class="stage">${cardInner(model, { shotSrc: "" })}</div>
  <div class="actions">
    <a class="go" href="${esc(target)}">${model.kind === "dataset" ? "Open this graph in the playground" : "Run this query in the playground"} →</a>
    <span class="alt">${facts ? esc(facts) + " · " : ""}no server, no download — the query runs in your browser.</span>
  </div>
  ${model.kind === "example" && model.query
    ? `<div class="q"><h2>The query</h2>${esc(model.query)}</div>` : ""}
  <p class="alt" style="margin-top:20px">
    <a href="../playground.html">rete playground</a> ·
    <a href="../index.html">documentation</a> ·
    <a href="https://github.com/caviri/rete">github.com/caviri/rete</a>
  </p>
</div>
<script>${FIT_JS}</script>
</body>
</html>
`;
}

// ------------------------------------------------------------------- write ----

function writeIfChanged(file, content) {
  if (fs.existsSync(file) && fs.readFileSync(file, "utf8") === content) return false;
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
  return true;
}

const models = buildModels(ROOT, { base: BASE });
let written = 0;
// One shared stylesheet rather than the same 8 KB inlined into 700+ pages: it
// keeps the generated tree small and lets a visitor's browser cache it once.
// (The card PNG is rendered from a standalone document that still inlines it.)
if (writeIfChanged(path.join(ROOT, "docs", "preview.css"),
  `/* Generated by scripts/preview/build_pages.mjs — edit CARD_CSS/PAGE_CSS there. */\n${PAGE_CSS}`)) written++;
for (const model of models) {
  if (writeIfChanged(path.join(ROOT, "docs", sharePath(model)), pageHtml(model))) written++;
}

// An index of every share page: a human-browsable list, and the thing a sitemap
// or a link checker can walk.
const byDataset = new Map();
for (const model of models) {
  if (!byDataset.has(model.dataset.key)) byDataset.set(model.dataset.key, []);
  byDataset.get(model.dataset.key).push(model);
}
const rows = [...byDataset.entries()].sort((a, b) => a[0].localeCompare(b[0])).map(([key, group]) => {
  const dataset = group.find((m) => m.kind === "dataset");
  const examples = group.filter((m) => m.kind === "example");
  return `<section>
    <h2>${esc(dataset ? dataset.dataset.icon : "◆")} <a href="d/${esc(key)}.html">${esc(key)}</a>
      <small>${examples.length} example${examples.length === 1 ? "" : "s"}</small></h2>
    <ul>${examples.map((m) => `<li><a href="q/${esc(m.slug)}.html">${esc(m.title)}</a>${
      m.answer ? ` <em>${esc(answerHeadline(m.answer))}</em>` : ""}</li>`).join("")}</ul>
  </section>`;
}).join("\n");

const indexHtml = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8" />
<meta name="viewport" content="width=device-width, initial-scale=1" />
<meta name="color-scheme" content="light dark" />
<title>Shareable queries · rete playground</title>
<meta name="description" content="Every example query in the rete playground as its own shareable link, with a preview card showing the real answer." />
<link rel="canonical" href="${BASE}shared.html" />
<meta property="og:type" content="website" />
<meta property="og:site_name" content="rete" />
<meta property="og:title" content="Shareable queries · rete playground" />
<meta property="og:description" content="${models.filter((m) => m.kind === "example").length} example queries over ${byDataset.size} public knowledge graphs — each with its own link preview showing the real answer." />
<meta property="og:url" content="${BASE}shared.html" />
<meta property="og:image" content="${BASE}og/d/${esc([...byDataset.keys()].sort()[0] || "scholar")}.png" />
<meta name="twitter:card" content="summary_large_image" />
<style>
body{font-family:${T.sans};color:${T.ink};background:${T.paper};margin:0;padding:40px 24px;line-height:1.5}
.page{max-width:960px;margin:0 auto}
h1{font-family:${T.serif};font-size:40px;margin:0 0 8px}
p.lead{color:${T.muted};margin:0 0 30px;max-width:62ch}
section{border-top:1px solid ${T.line};padding:18px 0}
h2{font-size:20px;margin:0 0 8px;font-weight:600}
h2 small{font-weight:400;color:${T.muted};font-size:14px;margin-left:8px}
ul{margin:0;padding-left:20px;columns:2;column-gap:34px}
li{margin:3px 0;break-inside:avoid}
a{color:${T.accent}}
em{color:${T.muted};font-style:normal;font-size:13px}
@media (prefers-color-scheme: dark){body{background:#0f1512;color:#dde8e2}section{border-color:#2a352f}}
@media (max-width:700px){ul{columns:1}}
</style>
</head>
<body><div class="page">
<h1>Shareable queries</h1>
<p class="lead">The playground keeps its state in the URL fragment, which link previews cannot see. Each example below has its own page — with a card showing the question, the dataset and the answer that query really returns — that opens the playground on exactly that query.</p>
${rows}
</div></body>
</html>
`;
if (writeIfChanged(path.join(ROOT, "docs", "shared.html"), indexHtml)) written++;

console.log(`pages: ${models.length} share page(s) + index — ${written} written/changed -> docs/q, docs/d, docs/shared.html`);
