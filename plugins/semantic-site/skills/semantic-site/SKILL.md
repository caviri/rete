---
name: semantic-site
description: Generate a static website from data (CSV, JSON, JSON-LD, N-Triples, N-Quads, or anything converted to them) where every page is HTML content, schema.org JSON-LD, an entry in a static JSON API, and a set of WebMCP tools for in-browser agents. Use when someone wants a website, portal, catalogue, explorer or "semantic app" for a dataset, spreadsheet or knowledge graph, hosted as plain files (GitHub Pages, Cloudflare, Netlify, Hugging Face, any bucket) with no server and no framework.
---

# Build a semantic static site

One command turns a `site.json` and its data files into a folder of plain files:

| Layer | What every site gets |
|---|---|
| **Content** | A home page with search, one page per collection (filters, sort), one page per item (fields, links, back-links), content pages from Markdown, an Agents & API page, a 404 page. Every list is in the HTML too, so pages work without JavaScript. |
| **Linked data** | schema.org JSON-LD in every page; the whole site as `api/dataset.jsonld`. RDF sources keep their predicate IRIs. |
| **Static API** | `api/index.json` (the site described), `api/<collection>/index.json`, `api/<collection>/<id>.json`, `api/search.json`, `api/openapi.json`, `api/tools.json`, `llms.txt`, `sitemap.xml`. CORS headers for hosts that read `_headers`. |
| **WebMCP** | Every page registers tools with `document.modelContext.registerTool` (WebMCP draft of 24 November 2026): `get_site_info`, `search`, `list_items`, `get_item`, `get_facets`, `get_current_page`, `open_page` everywhere, `show_items` on collection pages, `show_search` on the home page, `run_sparql` and `get_example_queries` when a SPARQL endpoint is configured. |

WebMCP is not optional: **every page this skill produces registers tools**. A
page added by hand must load `assets/boot.js` (see "Custom pages" below), and
`site-check` fails a page that registers none.

Requires Node 20 or newer. No packages to install. Scripts live in this skill's
directory, `scripts/`.

## 1. Look at the data

```sh
node <skill-dir>/scripts/inspect.mjs data/people.csv data/graph.nt
```

It prints each source's records, RDF classes with counts, and per field: type,
coverage, distinct values, samples and predicate IRI. Decide from it:

- **Collections**: one per kind of thing a visitor browses (a CSV file, or one RDF class).
- **Key**: the field that names an item in its URL (`prefix`, `doi`, `id`); default the IRI's last segment, then the row number.
- **Label**: what the item is called; default `title`, `name`, `label`, `prefLabel`…
- **Links**: fields whose values name items of another collection.
- **Facets**: fields with a handful of repeated values (status, year, type); auto-picked when omitted.

Inputs the reader takes: `.csv`, `.tsv`, `.json` (array, `{items|data|records}`, or
JSON-LD), `.jsonld` (compacted; prefixes, terms and `@vocab` are expanded, remote
contexts are not fetched), `.ndjson`/`.jsonl`, `.nt`, `.nq`. Convert anything else
first: Turtle or RDF/XML with any RDF tool (`riot --output=nt`, `rdflib`, `rete
export`); a SPARQL endpoint with a `CONSTRUCT` saved as N-Triples; a spreadsheet as CSV.

## 2. Write site.json

`node <skill-dir>/scripts/inspect.mjs <sources> --draft > site.json` gives a start.
A complete small example is the plugin's `example/` folder (a CSV of vocabularies
linked to an N-Triples graph of publishers). The full reference is
[reference/config.md](reference/config.md). The essentials:

```json
{
  "title": "Semantic Web vocabularies",
  "description": "Twelve vocabularies, who publishes them, where their specs live.",
  "baseUrl": "https://example.org/vocabularies/",
  "collections": [
    { "id": "vocabularies", "source": "data/vocabularies.csv", "key": "prefix", "label": "name",
      "type": "DefinedTermSet", "fields": { "publisher": { "link": "publishers" } } },
    { "id": "publishers", "source": "data/publishers.nt",
      "rdfType": ["https://schema.org/Organization"], "key": "name", "type": "Organization" }
  ],
  "pages": [{ "path": "about", "title": "About", "source": "content/about.md" }]
}
```

Set `baseUrl` once the address is known: JSON-LD identifiers, `sitemap.xml`,
canonical links and `llms.txt` become absolute. Without it everything is relative
and still works from any folder.

## 3. Build, preview, check

```sh
node <skill-dir>/scripts/build.mjs site.json --out dist
node <skill-dir>/scripts/serve.mjs dist            # http://127.0.0.1:8080/
node <plugin-dir>/skills/site-check/scripts/check.mjs dist
```

The build prints each collection's facets and columns, the page and file counts,
the tools per page kind and any warnings; read them and adjust `site.json`. It
refuses to overwrite a non-empty folder it did not write (`--force` overrides).

`serve.mjs` is enough for WebMCP: `http://localhost` is a secure context. In the
browser console, `pageTools.tools` lists the page's tools and
`await pageTools.call('search', { query: 'skos' })` runs one, with or without
WebMCP in the browser. To see the tools as an agent does, use Chrome with
`chrome://flags/#enable-webmcp-testing` and the Model Context Tool Inspector
extension.

`check.mjs` must pass before the site is published. It checks every page
(title, description, JSON-LD, local links, WebMCP), the API (every list, item
and page file, counts, search rows), and runs every read-only tool of every page
kind against the built files.

## 4. Publish

The output folder is the whole site. Hosting notes for GitHub Pages, Cloudflare,
Netlify, Hugging Face Spaces and buckets are in [reference/hosting.md](reference/hosting.md).
Commit `site.json`, the data and the content; rebuild `dist/` in CI rather than
committing it.

## Customising

- **Look**: `theme.accent`, `theme.accent2` (and `…Dark`) in `site.json`; light and dark follow the system and a toggle.
- **Text**: `kicker`, `intro` (Markdown on the home page), `pages` (Markdown or HTML fragments), `links` (footer).
- **Static files**: `"static": "public"` copies a folder into the site root (images, a `CNAME`, extra scripts).
- **More tools**: list ES modules in `webmcp.scripts`; each default-exports `(env) => [tools]` and is loaded on every page. How to write good tools: the `webmcp-tools` skill.
- **Live queries**: `"sparql": { "endpoint": "…", "examples": [{ "title": "…", "query": "…" }] }` adds `run_sparql` and `get_example_queries`. Any SPARQL 1.1 endpoint that allows CORS works (a rete gateway, Fuseki, QLever, Wikidata…).
- **Untrusted data**: tools that return site data carry `untrustedContentHint`, so an agent treats it as data, not instructions. Set `"webmcp": { "untrustedContent": false }` only when every word on the site is the owner's own.

## Custom pages

A page written by hand inside the site keeps the contract when its head has:

```html
<meta name="site-root" content="../">            <!-- path to the site root -->
<script type="application/json" id="page-context">{"kind":"page","title":"My page"}</script>
<script type="module" src="../assets/boot.js"></script>
```

## Rules

- Never hand-edit the output folder; change `site.json`, the data or the content and rebuild.
- Keep one source of truth: the pages, the API, the JSON-LD and the tools all come from the same model, so they cannot disagree.
- Large collections: each item is two files. Past about 20,000 items per collection, add a SPARQL endpoint and keep the static pages for the parts people browse.
- Report what `check.mjs` says as it says it; do not call a site done with errors.
