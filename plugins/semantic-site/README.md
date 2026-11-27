# semantic-site

A Claude Code plugin for static websites built from data. Every page it makes is
four things at once:

- **content**: HTML pages for a home page with search, collections with
  filters, items with their links and back-links, and Markdown pages;
- **linked data**: schema.org JSON-LD in every page, and the whole site in `api/dataset.jsonld`;
- **a static API**: `api/index.json` describes the site, and every list and item is a JSON file (with OpenAPI, `llms.txt` and a sitemap);
- **WebMCP tools**: every page registers tools with `document.modelContext`, so
  an agent in the browser can search, read and filter what the user sees.

It does not need rete. The input is CSV, JSON, JSON-LD, NDJSON, N-Triples or
N-Quads, and the output is plain files for GitHub Pages, Cloudflare, Netlify, a
Hugging Face Space or a bucket. Node 20 or newer, no packages.

## Install

```
/plugin marketplace add caviri/rete
/plugin install semantic-site@rete
```

## Skills

| Skill | Use it when |
|---|---|
| `semantic-site` | "make a website for this dataset / spreadsheet / graph" — inspect the data, write `site.json`, build, preview, check, publish |
| `webmcp-tools` | "make this page agent ready" — add WebMCP tools to any page or app, with the kit in `runtime/webmcp-kit.js` and the design rules |
| `site-check` | "is this site ready to publish?" — pages, JSON-LD, links, the API, and a run of every read-only tool |

## Try it

```sh
node skills/semantic-site/scripts/build.mjs example/site.json --out example/dist
node skills/semantic-site/scripts/serve.mjs example/dist
node skills/site-check/scripts/check.mjs example/dist
```

The example is twelve vocabularies in a CSV, linked to an N-Triples graph of
their publishers. Open <http://127.0.0.1:8080/>, then in the console
`await pageTools.call('search', { query: 'skos' })`. In Chrome with
`chrome://flags/#enable-webmcp-testing`, the Model Context Tool Inspector
extension lists the same tools.

## Layout

```
.claude-plugin/plugin.json
runtime/                 browser files copied into every site (ES modules)
  webmcp-kit.js          registration, fallbacks, validation, budgets — usable on any page
  site-tools.js          the tools of a generated site
  site.js, boot.js       page behaviour and the entry point
  query.js, site.css
skills/semantic-site/    SKILL.md, reference/, scripts/ (build, inspect, serve)
skills/webmcp-tools/     SKILL.md, reference/spec.md
skills/site-check/       SKILL.md, scripts/check.mjs
example/                 site.json + data + content
test/                    node --test test/semantic-site.test.mjs
```

## WebMCP

The tools follow the WebMCP Community Group draft of 24 November 2026
(`document.modelContext.registerTool(tool, { signal })`) and fall back to the
`navigator.modelContext` and `provideContext` shapes of older Chrome previews.
WebMCP is a draft; when it changes, the kit is the one file to update.

## Development

```sh
node --test test/semantic-site.test.mjs
claude plugin validate .
```

Bump `version` in `.claude-plugin/plugin.json` with each change users should
receive; it is also the generator version written into every site.
