---
name: web-pages
description: Make web pages, sites and small apps from a .rete knowledge graph. Use when the user wants a page, site, dashboard, explorer or widget that shows data from a graph, wants the page to query the file live in the browser over HTTP range, wants static pages with JSON-LD and a JSON API, wants a page that an AI agent in the browser can use (WebMCP tools), or wants link previews. Includes a working single-file page and a build script.
---

# Web pages from a .rete file

A `.rete` file is a static file you can host anywhere that serves HTTP `Range`
requests. A page can query it directly, in the visitor's browser, with no
server and no database: the engine is WebAssembly and it fetches only the byte
ranges a query needs. You have three ways to build with it, and they combine.

| Approach | Use when | Cost |
|---|---|---|
| **Live page**: the browser queries the file | data changes, search boxes, many possible queries | first visit loads the engine (about 3 MB compressed, cached after) and a few KB to MB of the file |
| **Static build**: run the queries once, write HTML, JSON-LD and JSON files | content pages, SEO, link previews, small data, no JS needed | nothing at visit time; rebuild when the data changes |
| **Both**: static pages for content, live queries for search and filters | most real sites | |

Start by reading the graph with the `query` skill's order (`dataset_card`,
`dataset_schema`, `example_queries`) so you know the classes and predicates the
page will use. Take the file's `url` from `list_datasets`.

## 1. Hosting: what the file's host must do

- Answer `Range: bytes=a-b` with `206 Partial Content`. A host that ignores `Range` and sends the whole file works for small files and is slow for big ones.
- For a page on another origin, send CORS headers: `Access-Control-Allow-Origin` and `Access-Control-Allow-Headers: Range`.

Check with curl before you debug anything else:

```sh
curl -s -o /dev/null -D - -r 0-1023 https://host/papers.rete | grep -iE "^HTTP|content-range"
# HTTP/2 206   and   content-range: bytes 0-1023/27798
curl -sI -r 0-1023 -H "Origin: https://example.org" https://host/papers.rete | grep -i access-control
```

Cloudflare R2 and S3 serve `Range`. `python -m http.server` does not, so for
local testing use a server that does (nginx and Caddy do).

## 2. A live page (the template)

[references/page.html](references/page.html) is a complete page: it opens a
file, shows its card, searches titles, shows how many bytes it read, writes
JSON-LD, and registers WebMCP tools. It was run in Chromium against a local
file. Copy it, then change three things: `RETE_URL`, the `PREFIXES` and the
`LIST` query.

How it works, because the constraints are the point:

- **The engine must run in a Web Worker.** Browsers allow the synchronous range reads it uses only in workers. On the main thread `rete.open(url)` throws. The page makes a worker from a Blob, `importScripts` the engine from a CDN (`rete-graph@0.7.8/dist/rete-graph.min.js`, pin the version) and talks to it with `postMessage`.
- **Send plain values out of the worker.** Rows hold term objects. Map them to strings (`t.value`) before `postMessage`.
- **Open once.** The worker keeps the graph open, so the block cache and decoded dictionary make the second query cheaper than the first.
- **Card first.** `graph.card()` reads two small ranges and gives you the title, description, license and counts for the page header, before any query.
- **Show `graph.stats()`** (`bytes`, `fileLength`, `requests`). It proves the page is not downloading the file and it makes slowness visible. On a tiny file `bytes` can exceed the file size because of block alignment.

Smaller engine calls you will use in the worker: `graph.query(sparql)`
(SELECT gives rows of terms, ASK a boolean, CONSTRUCT triples),
`graph.prefixSearch("Berl")` (label autocomplete, no scan, needs a label
index), `graph.textSearch("volcano")` (needs a text index), `graph.schema()`,
`graph.examples()`, `graph.queryRows(sparql)` for paging a big result
(`rows.length`, `rows.page(0, 25)`, `rows.free()`).

A file that fits in memory can also be fetched whole and opened from bytes on
the main thread (`rete.open(new Uint8Array(await (await fetch(url)).arrayBuffer()))`).
Use that below a few MB only.

## 3. Make it fast

- Ask small, selective questions. A page that shows 25 rows runs `LIMIT 25`, not a query for everything that it filters in JavaScript.
- Debounce the search box (250 ms in the template) and drop stale answers.
- Prefer `prefixSearch` over `FILTER(CONTAINS(...))` for a name box, when the file has a label index. `CONTAINS` scans every literal unless the file has a text index; `find_entities` on the server tells you whether a text index exists.
- Query only the columns you render; fetch details when the user opens an item, with `VALUES` or a bound IRI.
- Aggregates over one predicate (counts per year, per type) are cheap. Compute them once and cache.
- Cache answers in memory by query string. Repeat visits benefit from the browser's HTTP cache for the file's ranges when the host sends `ETag` and `Cache-Control`; files are immutable, so a versioned file name (`papers-2026-11-28.rete`) can be cached forever.
- For a heavy page, build the first screen statically (section 4) and let live queries take over.
- Prefer one worker with several queries over several workers.

## 4. Static pages: content, JSON-LD, JSON API, link previews

Run the queries at build time and write plain files. Crawlers and link
previewers (Slack, X, LinkedIn, Mastodon) read the HTML as served and do not
run scripts, so the title, description and `og:` tags must be in the HTML. That
is the reason to build statically even when the live page exists.

[references/build-static.mjs](references/build-static.mjs) is a Node script:

```sh
npm install rete-graph@0.7.8
node build-static.mjs papers.rete dist https://example.org/papers/
```

It writes, from the file's card and one query:

- `dist/index.html`: the list, `<title>`, description, canonical, Open Graph and Twitter tags, and a schema.org `Dataset` as JSON-LD in `<script type="application/ld+json">`. JSON in a script tag is escaped so data cannot close the tag. Page text is escaped too.
- `dist/api/papers.json` and `dist/api/papers/<id>.json`: a static JSON API, one file per item. Any client can use it without SPARQL.

Open Graph needs an `og:image` (1200 by 630 is the usual size) at an absolute
URL; make one image per site or per item at build time. Without it most
previewers show a bare link.

JSON-LD: use schema.org types that match the data (`Dataset`, `Person`,
`ScholarlyArticle`, `Place`, `CreativeWork`), `@id` with the entity's IRI, and
`sameAs` for ORCID, ROR or Wikidata IRIs the graph already holds. Take the
dataset-level fields (name, license, creator, source) from the card and do not
type them again by hand. Validate with the schema.org validator before you ship.

For a whole site (collections, item pages, search, sitemap, `llms.txt`) from CSV,
JSON or N-Triples, the `semantic-site` skill in the rete plugins does the same
job with a generator; export your SELECT as CSV or N-Triples and feed it in, or
use it as the model for the files above.

## 5. WebMCP: let an agent use the page

Every page you make should register tools, so an agent in the browser calls
functions instead of reading pixels. The template does it with feature detection:
`document.modelContext.registerTool(tool)` (the spec) or
`navigator.modelContext` (older Chrome previews), and without either the tools
still work as `window.pageTools.call(name, input)`.

A tool is `{ name, description, inputSchema, execute(input), annotations }`.
Rules that matter, with a longer list in
[references/webmcp.md](references/webmcp.md):

- Describe when to use the tool and what comes back, in one or two sentences.
- Give read tools `annotations: { readOnlyHint: true }`, and `untrustedContentHint: true` when the result is data someone else wrote.
- Return small JSON (a few KB). Cut lists and say `truncated: true`.
- Validate input; refuse anything that is not SELECT or ASK in a `run_sparql` tool, and cap rows.
- Offer a few high-level tools (`search_papers`, `get_paper`) before a raw `run_sparql`.

## 6. Pictures and embeds

- **Playground link.** Anyone can explore the same file in the rete playground: `https://caviri.github.io/rete/docs/playground.html#url=<encoded https URL of your .rete>&mode=sparql`. A good "open in SPARQL" link next to your page; the host must serve Range with CORS.
- **Graph on your page.** The server's `graph_view` is an MCP App for chat clients, not a widget for a web page. On a page, run a SELECT for the edges (`?s ?p ?o`, a few hundred rows) and draw it with cytoscape.js or d3. Send it as plain rows from the worker.
- **Charts.** An aggregate query (`GROUP BY` year or type) is the data for a bar or line chart.

## 7. Check before you ship

- Open the page with the network tab: one `206` per range, no `200` for the whole file.
- Look at `stats` for the three queries a visitor makes most; each should read a small part of the file.
- Turn JavaScript off: the static content and the `og:` tags are still there.
- View source and paste the URL into a link-preview tool.
- Test the tools with `window.pageTools.call("search_papers", { text: "engine" })`.
- Data is untrusted: set `textContent`, never `innerHTML`, for anything from the graph.
