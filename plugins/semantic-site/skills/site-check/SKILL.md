---
name: site-check
description: Check a built static site before publishing — every page has a title, description, valid JSON-LD, working local links and WebMCP tools; the static JSON API (api/index.json, lists, items, search, OpenAPI) is complete and consistent; and every read-only WebMCP tool runs. Use after building a site with semantic-site, before deploying any static site that should be WebMCP ready, or when asked to verify, audit or debug such a site.
---

# Check a semantic static site

```sh
node <skill-dir>/scripts/check.mjs <site-dir>          # human-readable
node <skill-dir>/scripts/check.mjs <site-dir> --json   # { ok, notes, issues[] }
```

Node 20 or newer, no dependencies. Exit code 0 when there are no errors, 1 otherwise.

## What it checks

**Every HTML page**
- `<title>` (error), meta description, viewport and `lang` (warnings).
- Every JSON-LD block parses and has `@context`.
- Every local `href`/`src` resolves to a file (directories resolve to `index.html`).
- **WebMCP**: the page loads a script that registers tools (it follows local
  `<script src>` and their relative imports, looking for `document.modelContext` /
  `registerTool`). A page without tools is an error; only `404.html` is exempt.

**The static API** (when `api/index.json` exists; a site without it gets a warning)
- Every `.json` / `.jsonld` under `api/` parses.
- `api/index.json` has the site, its collections and the endpoints it declares, and those files exist.
- For each collection: the list exists, its `count` matches its items and the index, every item has its JSON file and its page, facets have values.
- Every row of the search index names an existing item; `openapi.json` is OpenAPI 3; `llms.txt` exists.

**The tools, run** (when `assets/` holds the semantic-site runtime)
- For one page of each kind (home, each collection's list and item pages, content pages, agents, 404) it registers the page's tools against a stand-in `document.modelContext`, exactly as the page would.
- Each tool needs a valid name, a description an agent can choose by, and an object `inputSchema`; registration must pass an `AbortSignal`.
- Every read-only tool runs with sample input against the built files; results must serialise and stay within the 18,000-character budget. `open_page`, `show_items` and `show_search` must reach the page; `get_item` with an unknown id must fail with a message that says what to call instead.

`run_sparql` is not called (it needs the live endpoint).

## Reading the result

```
  ok  23 pages: 23 register WebMCP tools, 22 JSON-LD blocks, 437 local links checked
  ok  API: 2 collections, 17 items, every list, item and page file present
  ok  WebMCP tools registered and run on each page kind: home 8, collection 8, item:vocabularies 7, …
site-check: passed
```

Fix errors at their source (`site.json`, the data, or the page), rebuild, and
check again. Report the outcome as printed, warnings included.
