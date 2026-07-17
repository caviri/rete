# JS lab — rete × D3, Observable-style

An experiment in the spirit of [Observable](https://observablehq.com) and the
p5.js editor: **an in-browser JavaScript editor next to a live
visualization**. The code on the left queries a remote `.rete` file through
the [`rete-graph` npm client](javascript.md) (loaded straight from the CDN —
the same build any web page can `<script src>`), and draws the result with
[D3](https://d3js.org) on the right. Edit, hit `Ctrl+Enter`, watch it redraw.

**[Launch the lab →](jslab.html)**

Five presets, over two datasets, each ending with `stats()` — how few bytes
the queries actually fetched:

- **network** — the **Linked Jazz collaboration graph** (who played, toured,
  and sat in bands with whom, a 176 KB `.rete`) as a draggable, zoomable
  force layout; hover a musician to isolate their circle.
- **chord** — the same collaborations as a circos-style chord diagram of the
  top 16 musicians.
- **stacked** — each musician's connections broken down by relationship type
  (a validated 4-color categorical stack).
- **top 20** — a SPARQL `GROUP BY` as a clean ranked bar chart.
- **timeline ▶** — a *different* dataset with real temporal data:
  **Heroic-Age Antarctic expeditions** (Wikidata, CC0). Time runs down the
  y-axis; a year-cursor sweeps 1896→1918, expeditions grow as it passes,
  crews pop out of them, and the four people who sailed more than once
  become orange threads connecting expeditions. Run again to replay.

Two more buttons in the header: **⿻ hide code** collapses the editor for a
presentation view, and **⬇ standalone** downloads the *current program* as a
single self-contained HTML file — D3 and the rete-graph engine embedded, no
CDN, no editor — that runs anywhere and credits its sources (only the data's
own network is needed). That exported file is yours to host or share.

## How it works

- The rete engine's remote reader uses **synchronous XHR range requests**,
  which browsers only allow in workers — so the page boots a tiny **Web
  Worker** that `importScripts` the CDN bundle and holds the opened graphs.
  Your code gets a promise-based façade: `openGraph(url)` →
  `{ query, prefixSearch, textSearch, stats, contentHash }`.
- Graphs stay **resident in the worker** between runs, so a re-run reuses the
  block cache and decoded dictionary — the second query is nearly free.
- Query rows arrive as `{variable: term}` objects; each term carries `.kind`
  (`iri`/`literal`/`bnode`) and `.value` (plus `.datatype`/`.lang`). Numbers
  from aggregates are literal terms — coerce with `+row.n.value`.
- Your program runs with four bindings in scope: **`openGraph`**, **`d3`**,
  **`viz`** (the output container element), and **`log(...)`** (the console
  strip).

Point `openGraph()` at any `.rete` URL whose host serves `Range` with CORS
(see [Hosting](hosting.md)) — every dataset in the
[playground](playground-guide.md) qualifies; find their URLs in its catalog.

## Credits & license

Data: the [Linked Jazz Project](https://linkedjazz.org/) (CC BY-SA) — thank
you for a wonderful graph. Libraries: [D3](https://d3js.org) (ISC),
[CodeMirror](https://codemirror.net) (MIT), served by
[jsDelivr](https://www.jsdelivr.com/).

Source & issues: <https://github.com/caviri/rete> · © 2026 Carlos Vivar Ríos,
released under the
[Apache License 2.0](https://github.com/caviri/rete/blob/main/LICENSE).
