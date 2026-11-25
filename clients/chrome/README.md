# Retelens, the browser extension

Retelens is a Chromium extension (tested in Chrome and Brave) that opens `.rete` graph files where they are hosted, instead of downloading them. It shows the file's Dataset Card, tables from its example queries, results as a table, cards or a graph, and a SPARQL console. It works on Hugging Face (datasets and storage buckets), GitHub and Zenodo inside the page, in each site's own look. Any other `.rete` URL opens in a standalone viewer with a Download button. User guide: [docs/chrome.md](../../docs/chrome.md).

The engine is `rete-graph` (`clients/js`) built from the same commit, so the extension always carries the engine of its release, as the Claude Desktop extension does.

## Install

Every release attaches `retelens-<version>.zip`, and `retelens.zip` under a fixed name: https://github.com/caviri/rete/releases/latest/download/retelens.zip. Unzip it, open `chrome://extensions` (`brave://extensions` in Brave), turn on Developer mode, choose **Load unpacked** and pick the unzipped folder.

## Build

In Docker, from the repository root (the dev image builds the wasm engine; the Playwright image has Node and Chromium):

```sh
docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) dev bash clients/js/build-wasm.sh
docker run --rm --ipc=host -v "$PWD:/work" -w /work mcr.microsoft.com/playwright:v1.49.0-noble sh -c '
  npm --prefix clients/js ci && node clients/js/build.mjs &&
  npm --prefix clients/chrome ci && node clients/chrome/build.mjs --zip dist/retelens.zip &&
  xvfb-run -a node --test clients/chrome/test/*.test.mjs'
```

`build.mjs` writes `build/` (load that folder unpacked while developing):

- copies the sources listed in `RUNTIME`;
- stamps the workspace version from `Cargo.toml` into the manifest and the status line;
- bundles the graph view from `lib/graphkit/src` with esbuild, and writes the licence of every package in the bundle to `vendor/graphkit/`;
- copies the engine, `clients/js/dist/rete-graph.min.js`.

`--zip <file>` packs `build/` with sorted entries and a fixed date, so the same tree gives the same bytes.

`test/smoke.test.mjs` loads `build/` into Chromium and serves `docs/plaza/data/linked-jazz.rete` from a local Range server. It checks the manifest, the first example query's answer, the engine version in the status line and the Graph view. It needs no network.

`test/rich.test.mjs` builds its fixtures in memory (`test/fixtures.mjs`: PNGs, a one-tile PMTiles archive, a IIIF v3 manifest, a page with JSON-LD, microdata and Open Graph) and checks the cells (images, IIIF, locator maps, Markdown from the column menu), the column statistics (a click on a bar filters), the cards and their focus view, the Map view over a basemap set as the popup does (its own archive, so no network), the PMTiles viewer, and *Read this page's data* end to end (reader, graph, download). Set `RETELENS_SHOTS=<dir>` to keep screenshots. Tiles embedded in a `.rete` (0x05 only) and the host pages are checked by hand: `geoadmin-tiles.rete` and `geoadmin.pmtiles` on data.graphplaza.com. The host pages themselves (Hugging Face, GitHub, Zenodo) change under us, so they are checked by hand before a release.

## Cells, cards and maps (from the playground)

`lib/cells.js` draws every value of a result, in the Table and the Cards alike, the way the playground learnt to:
images as thumbnails (Commons `Special:FilePath` and IIIF Image API URLs scaled by the server; a larger copy on hover
on fine pointers; a lightbox on click; an *Open image* link under each), IIIF manifests (Presentation v2 and v3) as a
paged thumbnail that opens a paged viewer with the manifest's metadata, WKT geometries as an offline locator map,
audio and video players, PDF and web links, opt-in Markdown and page previews (a sandboxed frame), numbers. The
column menu's *Show as* and the cards' *Fields* dialog force a type per variable (kept across queries by name).
Manifests and page previews load when their cell comes near the viewport. On a host page whose CSP refuses outside
images, an image is fetched from the content script and shown from a `blob:` URL; when that fails too (no CORS),
the explorer says so and offers the file in the extension's viewer.

`lib/views.js` builds the cards: the first image / IIIF / video field fills the top, a title from the first name-like
literal, then the fields label over value; a click opens the focus view (native horizontal scroll with snap; arrow
keys, Shift+wheel, drag).

`lib/maps.js` adds the Map view (a WKT column, or latitude + longitude columns; 2,000 rows), the map a geometry cell
opens, and PMTiles. Leaflet, protomaps-leaflet and pmtiles are bundled by `build.mjs` from `lib/mapkit/src` into
`lib/mapkit/mapkit.js` (300 kB) and injected like the graph view when a map is first opened. The basemap is an open
PMTiles archive (`basemapUrl` in `chrome.storage.local`, set in the popup). The default is OpenStreetMap as the
Protomaps basemap: `pmtiles extract https://build.protomaps.com/20261124.pmtiles … --maxzoom=12` (18 GB; z11 is
8.1 GB, z13 36 GB), at `https://data.graphplaza.com/osm-basemap/osm-basemap-z12-2026-11-24.pmtiles` and `rete/osm-basemap/` in the rete-public bucket, drawn with protomaps-leaflet's
light or dark flavour. Another archive in the Protomaps layout gets the same; any other vector archive a quiet style
derived from its tilestats (the coarsest polygon layer as land, finer ones as borders by zoom, named points
labelled), e.g. `data.graphplaza.com/geoadmin/geoadmin.pmtiles`. No raster service: from an extension page
Carto answers "API key required" and tile.openstreetmap.org "Access blocked" (measured 2026-11-24).

`lib/stats.js` puts column statistics under each table header, as Hugging Face's viewer does: a histogram (numbers,
dates, string lengths; at most 12 bins with nice steps, min and max under it), one bar per value for columns with at
most 12 distinct values, namespaces for IRI columns, % empty. Hover a bar for its range and count; a click sets the
column's filter to exactly the rows of that bar (the bin's smallest and largest value, the value, or the namespace)
and a second click clears it. Computed on the model's rows (every row up to 10,000) or the page before they load.
A `.pmtiles` URL or file opens the PMTiles viewer (header, metadata, layers with their fields and colour, features
under a click). A PMTiles archive embedded in a `.rete` (`scripts/embed_tiles.py`, format 0x05 only) is found by
the worker (`tiles` op: kind 7 or an unknown kind, at least 16 KiB, PMTiles magic) and read through the engine
(`readRange` op, so the chunk cache and 429 handling apply); the Map view draws it under the result and draws the
features whose properties match a value of the result on top.

## Shared with the Claude extension and the file explorer

`lib/octicons.js`, `format.js`, `results.js`, `cells.js`, `stats.js`, `hints.js` and `maps.js` (with
`lib/mapkit/src`, `rl-common.css`, `rl-rich.css`) are also bundled into the graph view of the Claude Desktop extension
by `clients/mcpb/view-build.mjs`, and through it into the file explorer and the desktop app. They stay free of
`chrome.*` except where a host can answer first: `window.RetelensHost.loadMapKit()` starts the map library a page
carries, `RetelensHost.openLayer(url, name)` is what a *Map layer* button does, `window.RetelensTheme.tokens()` says
light or dark. `hints.js` sets `globalThis.RetelensHints`, so the extension's server reads hints in Node. A change here
runs the extension's tests too (`.github/workflows/mcpb-test.yml`).

## Query hints, layers, the example

`lib/hints.js` reads a query's comments: the title (first line), a description (more plain lines), `# view:` (table,
cards, graph, map) and `# column ?var: Label — note`. Without a view line the view is guessed (WKT -> map,
CONSTRUCT/DESCRIBE -> graph, picture predicates -> cards, a starter query's `dimension`). The examples panel and the
query popover show the view and the explained columns as chips; selecting an example (or running a console query with a
view line) opens it in that view; the table, cards and map popups use the column labels and notes. The convention is
documented for card authors in docs/dataset-card-reference.md.

A column of `.pmtiles` URLs is drawn by the Map view as layers (raster via `pmtiles.leafletRasterLayer`, vector with
the generic style) between the basemap and the result, each with a switch; a cell shows a *Map layer* button that opens
the PMTiles viewer.

The default example is `https://huggingface.co/datasets/katospiegel/rete-graphs/resolve/main/antequera/antequera-2026-11-24.rete` (viewer start page, popup). On Hugging Face, `content/adapters/hf.js` puts first
the file named by the README's `configs` entry with `default: true` (the API's `cardData`). The example is built by
`data/antequera/build2.mjs` (outside this repository, like the catalog's pipelines): REDIAM WFS (RENPA, Red Natura
2000, humedales, senderos, equipamientos), Overpass, Wikidata, Copernicus GLO-30 with GDAL (relief tiles and zonal
elevation).

## Reading a page's data

On request only (context menu *Read this page's data*, the popup, the `read-page` command; `activeTab`), the service
worker injects `content/extract.js` into the tab. It returns the text of every JSON-LD block, the microdata items as
JSON-LD with absolute IRIs (WHATWG microdata values), the Open Graph / description meta tags and the canonical link.
The result goes to `chrome.storage.session` (the last 8 pages) and `viewer.html?page=<id>` opens next to the tab.

`lib/page-data.js` prepares the JSON-LD for `rete.build(..., "jsonld")` (the engine loads no remote context): the
schema.org context becomes its inline equivalent (the official context is `@vocab` `http://schema.org/` with no
coercions), `@base` is the page URL, blocks are parsed leniently (raw newlines, trailing commas), `{@context, @graph}`
wrappers are opened, other remote contexts are read as their namespace, and every repair is listed. The worker builds
one `.rete` from all of it (`buildJsonld`), or, when the engine refuses that, from the documents it accepts one by
one, and opens it from bytes (`openBytes`). The reader renders the JSON objects, not the graph (RDF keeps no order):
the main things first (named as `mainEntity`, else typed nodes nobody points to that are not site furniture, richest
first), ISO 8601 durations and dates, ratings, prices, quantities, addresses as one line, lists of strings and of
steps in order, long lists folded. Starter queries: things on the page, every statement, pictures, links.

## What it does on Hugging Face

The UI is built as DOM in the HF page with HF's own Tailwind classes, so fonts, colours and dark mode are HF's.

- **Dataset home** (repo has `.rete` files): "Dataset Viewer" box in place of "not available", a Data Studio tab, chips (Formats, Size, License, Tags), a card block in the right column, a README-style summary of the embedded card.
- **`/datasets/<id>/viewer`**: full-width Data Studio under HF's header, with a back button, Graph/Query pills (`?file=&q=&view=`), search, Table | Cards | Graph, pager, side panel (Examples, Console, Schema, Card).
- **Tree / blob**: "Open in viewer" links; blob pages get the viewer box.
- **Buckets**: file pages get the box and a Data Studio switch (`?studio=1`, same URL, back/forward work); folder pages get "Open in viewer" links, and a folder holding exactly one `.rete` gets the card summary.
- Datasets without `.rete` files, and datasets where HF's own viewer works, are left alone.

`viewer.html?src=<url>` is the same studio as a normal extension page (popup, other hosts), styled by `shim.css`.

## What it does on Zenodo

InvenioRDM on Semantic UI, so the markup is Zenodo's own (`ui compact mini button`, `ui accordion panel`, `ui definition table`). Light only.

- **Record page with `.rete` files:** a Preview button (eye icon) before Download on each `.rete` row; an accordion panel right after the "Files" heading, open by default, with the shared viewer in it (`?preview_file=<name>` is kept like Zenodo does). Its Data Studio button goes full width under Zenodo's header (`?studio=1`, back control, breadcrumb).
- **Sidebar "Graph card"** after Rights, and a header label `rete 0x07 · 119.9 M statements`. Attribution merges the file's card with the record metadata in `#recordVersions[data-record]` (creators with ORCID, DOI, concept DOI, version, date, licence; Zenodo ids and SPDX ids are normalised). Where the two disagree both are shown, labelled "from the file's card" / "from Zenodo".
- Files are read through `/records/<id>/files/<name>?download=1`, which needs the `zenodo.org` host permission (the API URL does not expose `Content-Range`). Zenodo's CSP does not block the hidden engine iframe.
- The concept record (`/records/<concept>`) redirects to the latest version on Zenodo's side.

## Read chunks and the cache

Zenodo limits requests (about 133 per window, Range reads count). The engine worker installs a shim over `XMLHttpRequest` (`rete-xhr-shim.js`): for hosts with a chunk policy it answers the engine's synchronous Range reads from large aligned chunks (a RAM LRU of 256 MiB, one real request per run of missing chunks) and passes everything else through. A 429 is waited out (`Retry-After`, up to 3 retries) with a note in the status line. Chunks are also kept in IndexedDB (`rete-cache.js`, 1 GiB cap, LRU, keyed by URL + size + ETag + chunk; validated against the first response), so a second visit costs one request. The popup's Advanced section sets the chunk size per host (Auto / 1 / 4 / 8 / 16 MiB) and clears the cache. Default: 1 MiB on zenodo.org, Auto (the engine's own 128/256/512 KiB blocks) elsewhere. On `wikidata.rete` (1.56 GB) a cold open with 1 MiB chunks took 8 requests, a second visit 1.

## Turning the viewer off

Right-click a page on huggingface.co, github.com or zenodo.org: "rete viewer" has "Turn off on this page", "... this dataset / bucket / repository / record" and "... this site", and "Turn on again" for the ones that are off. The popup shows the same three switches and lists what is off. It is live: everything the adapters changed is reverted (`content/main.js` registry: `ctx.track/hide/replace/on/observe/every`), the engine iframe is removed, turning it on again mounts afresh. Rules are plain strings in `chrome.storage.local` (`offRules`). "Preview with rete" on links keeps working.

## The .rete link scanner

Besides each adapter's precise hooks, `content/scanner.js` finds `.rete` links and filenames in text (README tables, file lists) and adds a small native-looking Preview next to them. On websites the user granted ("every website" or a host in the viewer's permission prompt) the same scanner is registered as a dynamic content script (`content/scan-any.js`) with a neutral badge in a closed shadow root.

## Architecture

```
manifest.json             MV3; wasm allowed in extension pages; "scripting" to inject graphkit on demand
engine.html, engine-frame.js   hidden extension iframe, ONE per host page: owns the Worker, takes the first MessagePort
rete-worker.js            classic Worker: owns the rete-graph Graph (sync XHR range reads only work in a worker)
lib/engine.js             promise client over a Worker or a MessagePort
lib/format.js             DOM builder (text nodes only), http(s)-only URLs, static icons, IRI shortening
lib/cells.js              one value as DOM: images, IIIF, WKT locator, media, links, Markdown, page preview; render types, hover zoom, dialogs
lib/views.js              Table and Cards views (sort/filter controls in the header), the card focus view and Fields dialog
lib/maps.js               Map view, geometry map, PMTiles viewer and layers (loads lib/mapkit/mapkit.js on first use)
lib/page-data.js          a page's structured data: JSON-LD preparation, the reader, starter queries (viewer only)
lib/mapkit/src/           Leaflet + protomaps-leaflet + pmtiles; build.mjs bundles it to lib/mapkit/mapkit.js
lib/rl-rich.css, rl-reader.css   styles of the cells, cards, dialogs and maps; of the reader (hand-written)
content/extract.js        injected on request: a page's JSON-LD, microdata and meta tags
lib/results.js, results-ui.js  search / per-column filters / sort model, filter bar, column menu, Cards facets, Add-as-FILTER
lib/graph-view.js         Graph view: rows -> GraphKit nodes/edges, expand through the shared engine, host tokens + skin
lib/theme.js              host colour/font tokens, theme-change watcher, class vocabulary of the page
lib/rl-common.css         thin themed scrollbars, search-hit marks
lib/card-sidebar.js       Dataset Card in HF's sidebar vocabulary
lib/explorer.js           Session (engine + files) and Explorer (pills, search, view switch, pager, side panel)
content/main.js           picks the adapter, creates the engine iframe once
content/adapters/hf.js    all HF page logic (github.js, zenodo.js the same for their hosts)
content/scanner.js        .rete link / filename scanner shared by the adapters; scan-any.js = the same on granted sites
rete-xhr-shim.js, rete-cache.js   engine worker: chunked Range reads, 429 backoff, IndexedDB chunk cache
content/retelens.css      GENERATED: only the Tailwind classes HF's stylesheet lacks, scoped under [data-retelens]
shim.css                  GENERATED: the class vocabulary for the standalone viewer, with bundled fonts
tools/                    Tailwind config + CSS build scripts (Docker); scope.mjs prefixes selectors; icon.svg
vendor/                   Octicons licence; the build adds rete-graph.min.js and vendor/graphkit/*.LICENSE
lib/graphkit/src/         the graph view (cytoscape + layouts); build.mjs bundles it to lib/graphkit/graphkit.js
store/                    Chrome Web Store listing text and promo tile source
fonts/                    Source Sans 3 and IBM Plex Mono (OFL, licenses included)
```

The engine: rete-graph's lazy remote open uses synchronous XHR, allowed only in a Worker. The content script cannot start an extension Worker from the HF origin, so it adds a hidden `engine.html` iframe; the iframe starts the Worker and bridges it to the content script over one `MessageChannel` port (it accepts only the first port). All components on a page share that one engine. The HF `resolve/` URL answers 302 to a signed CDN URL; the worker resolves it once and re-resolves on 403.

Security rules: card fields, IRIs, literals and query results are untrusted and HF pages have no CSP. They only enter the DOM through text nodes; links and thumbnails must be `http(s)` (`rel="noopener noreferrer"`, thumbnails `loading="lazy" referrerpolicy="no-referrer"`); cytoscape labels are plain text; `innerHTML` is used only for static icon templates. IRIs are validated before they are put into an Expand query.

Regenerate CSS: `docker run --rm --memory 4g -v "$PWD/clients/chrome:/ext" node:22 sh /ext/tools/build-css.sh` (and `build-zenodo-css.sh`). The generated CSS is committed.

## Known limits

- Reads the formats its engine reads (0x05 to 0x08). Older files show an error naming the format.
- Graph view is GraphKit (lib/graphkit): at most 1,500 nodes / 3,000 edges per result. graphkit.js (1.4 MB) is injected into the page when a Graph view is first opened (as a manifest content script it made Chrome refuse to load the extension); Chrome refuses an injected script that holds a Unicode noncharacter (a raw U+FFFF gives "It isn't UTF-8 encoded"). esbuild's `charset: "ascii"` escapes strings but not regex literals, so build.mjs checks every built script and fails on one.
- Reads are serial synchronous XHR; nothing cancels a running query.
- Search, filters and sort are exact up to 10,000 rows (all rows are fetched in batches); above that they apply to the first 10,000 loaded rows and the bar says so, with "Add as FILTER to the query" writing the rewritten SPARQL into the console (never run for you). State is in the URL (`rq`, `rf`, `rs`).
- A CONSTRUCT/DESCRIBE without a LIMIT over a file larger than 5 MB is not started by itself: a hint shows first, with a Run anyway button.
- The `/viewer` and bucket-studio takeovers edit inside HF's header island and re-apply on mutation; a markup change on HF may break them.
- Private or gated files are not supported (no auth is passed to the engine).
- Zenodo: no Zenodo-side preview or search integration beyond the record page; the Graph/Cards/Table views read the file lazily, so a cold first open of the 1.6 GB file is about 6 requests / 38 MB.
