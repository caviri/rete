// Build a static page and a JSON API from a .rete file, at build time.
//
//   npm install rete-graph@0.7.8
//   node build-static.mjs papers.rete dist https://example.org/papers/
//
// Writes dist/index.html (content + Open Graph tags + JSON-LD, readable with no
// JavaScript), dist/api/papers.json and dist/api/papers/<id>.json. Link previews
// and most crawlers read what is in the HTML, so this is where the metadata goes.
// The source is a local file (read whole) or an https URL that serves Range (read lazily).

import { mkdir, readFile, writeFile } from "node:fs/promises";
import { open } from "rete-graph";

const [source = "papers.rete", out = "dist", base = "https://example.org/papers/"] = process.argv.slice(2);

const g = await open(/^https?:/.test(source) ? source : new Uint8Array(await readFile(source)));
const card = g.card() || {};

// Change this query to your data. Ask only for what the page shows.
const rows = g.query(`
  PREFIX ex: <https://example.org/>
  PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
  SELECT ?paper ?title ?year WHERE {
    ?paper a ex:Paper ; rdfs:label ?title ; ex:year ?year .
  } ORDER BY ?year LIMIT 1000`);

const papers = rows.map((r) => ({
  id: r.paper.value.split("/").pop(),
  iri: r.paper.value,
  title: r.title.value,
  year: Number(r.year.value),
}));

const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
// JSON inside <script> must not be able to close the tag.
const jsonForScript = (v) => JSON.stringify(v).replace(/</g, "\\u003c");

const title = card.title || "Papers";
const description = card.description || `${papers.length} papers`;

const jsonld = {
  "@context": "https://schema.org",
  "@type": "Dataset",
  name: title,
  description,
  license: card.license,
  url: base,
  isBasedOn: card.source,
  creator: (card.creators || []).map((c) => ({ "@type": "Person", name: c.name, ...(c.orcid ? { sameAs: c.orcid } : {}) })),
  distribution: { "@type": "DataDownload", encodingFormat: "application/vnd.rete", contentUrl: new URL("papers.rete", base).href },
  hasPart: papers.map((p) => ({ "@type": "ScholarlyArticle", "@id": p.iri, name: p.title, datePublished: String(p.year), url: `${base}api/papers/${p.id}.json` })),
};

const html = `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${esc(title)}</title>
<meta name="description" content="${esc(description)}">
<link rel="canonical" href="${esc(base)}">
<meta property="og:type" content="website">
<meta property="og:title" content="${esc(title)}">
<meta property="og:description" content="${esc(description)}">
<meta property="og:url" content="${esc(base)}">
<meta property="og:image" content="${esc(new URL("og.png", base).href)}">
<meta name="twitter:card" content="summary_large_image">
<script type="application/ld+json">${jsonForScript(jsonld)}</script>
</head>
<body>
<main>
<h1>${esc(title)}</h1>
<p>${esc(description)}</p>
<ul>
${papers.map((p) => `<li><a href="api/papers/${esc(p.id)}.json">${esc(p.title)}</a> (${p.year})</li>`).join("\n")}
</ul>
</main>
</body>
</html>
`;

await mkdir(`${out}/api/papers`, { recursive: true });
await writeFile(`${out}/index.html`, html);
await writeFile(`${out}/api/papers.json`, JSON.stringify({ count: papers.length, items: papers }, null, 1));
for (const p of papers) await writeFile(`${out}/api/papers/${p.id}.json`, JSON.stringify(p, null, 1));
console.log(`${papers.length} papers -> ${out}/`);
