# JS lab — rete × D3, Observable-style

An experiment in the spirit of [Observable](https://observablehq.com) and the
p5.js editor: **an in-browser JavaScript editor next to a live
visualization**. The code on the left queries a remote `.rete` file through
the [`rete-graph` npm client](javascript.md) (loaded straight from the CDN —
the same build any web page can `<script src>`), and draws the result with
[D3](https://d3js.org) on the right. Edit, hit `Ctrl+Enter`, watch it redraw.

**[Launch the lab →](jslab.html)**

The default program renders the **Linked Jazz collaboration network** — who
played, toured, and sat in bands with whom among 20th-century jazz musicians
(a 176 KB `.rete` on plain object storage) — as a draggable, zoomable force
layout, edges colored by relationship. A second preset turns a SPARQL
`GROUP BY` into a bar chart of the best-connected musicians. Both end by
logging `stats()`: how few bytes the queries actually fetched.

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
