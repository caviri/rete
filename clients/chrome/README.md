# Retelens, the Chrome extension

Retelens opens `.rete` graph files where they are hosted, instead of downloading them. It shows the file's Dataset Card, tables from its example queries, results as a table, cards or a graph, and a SPARQL console. It works on Hugging Face (datasets and storage buckets), GitHub and Zenodo inside the page, in each site's own look. Any other `.rete` URL opens in a standalone viewer with a Download button. User guide: [docs/chrome.md](../../docs/chrome.md).

The engine is `rete-graph` (`clients/js`) built from the same commit, so the extension always carries the engine of its release, as the Claude Desktop extension does.

## Install

Every release attaches `retelens-<version>.zip`, and `retelens.zip` under a fixed name: https://github.com/caviri/rete/releases/latest/download/retelens.zip. Unzip it, open `chrome://extensions`, turn on Developer mode, choose **Load unpacked** and pick the unzipped folder.

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

`test/smoke.test.mjs` loads `build/` into Chromium and serves `docs/plaza/data/linked-jazz.rete` from a local Range server. It checks the manifest, the first example query's answer, the engine version in the status line and the Graph view. It needs no network. The host pages themselves (Hugging Face, GitHub, Zenodo) change under us, so they are checked by hand before a release.

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
lib/views.js              Table and Cards views (sort/filter controls in the header)
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
- Graph view is GraphKit (lib/graphkit): at most 1,500 nodes / 3,000 edges per result. graphkit.js (1.4 MB) is injected into the page when a Graph view is first opened (as a manifest content script it made Chrome refuse to load the extension); the bundle must be pure ASCII (a raw U+FFFF in it triggers "isn't UTF-8", so build.mjs uses `charset: "ascii"`).
- Reads are serial synchronous XHR; nothing cancels a running query.
- Search, filters and sort are exact up to 10,000 rows (all rows are fetched in batches); above that they apply to the first 10,000 loaded rows and the bar says so, with "Add as FILTER to the query" writing the rewritten SPARQL into the console (never run for you). State is in the URL (`rq`, `rf`, `rs`).
- A CONSTRUCT/DESCRIBE without a LIMIT over a file larger than 5 MB is not started by itself: a hint shows first, with a Run anyway button.
- The `/viewer` and bucket-studio takeovers edit inside HF's header island and re-apply on mutation; a markup change on HF may break them.
- Private or gated files are not supported (no auth is passed to the engine).
- Zenodo: no Zenodo-side preview or search integration beyond the record page; the Graph/Cards/Table views read the file lazily, so a cold first open of the 1.6 GB file is about 6 requests / 38 MB.
