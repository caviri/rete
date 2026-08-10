# JS Lab: Rete × D3 (Observable-Style)

Welcome to the **JS Lab**, an experimental environment inspired by [Observable](https://observablehq.com) and the p5.js editor. It features an **in-browser JavaScript editor paired with a live visualization canvas**. 

Here is how it works: the code on the left queries a remote `.rete` file using the [`rete-graph` npm client](javascript.md) (loaded directly from a CDN, just like you would with a `<script>` tag in any web page). The results are then visualized on the right using [D3.js](https://d3js.org). 

**How to use it:** Simply edit your code, press `Ctrl+Enter`, and watch the visualization update instantly!

**[▶ Launch the JS Lab](jslab.html)**

## Included Presets

To get you started, we've included five presets spanning two different datasets. Each preset demonstrates how few bytes are actually fetched, using the `stats()` function:

*   **Network:** Explore the **Linked Jazz collaboration graph** (a 176 KB `.rete` file showing who played or toured together). It uses a draggable, zoomable force-directed layout. **Tip:** Hover over a musician to isolate their network.
*   **Chord:** View a circos-style diagram of the top 16 musicians and their complex relationships (e.g., mentorship, played with). Ribbons are colored by the dominant relationship type. Hover over a ribbon to see the exact breakdown.
*   **Stacked:** See a validated, 4-color stacked bar chart detailing each musician's connections, broken down by relationship type.
*   **Top 20:** A clean, ranked bar chart powered by a SPARQL `GROUP BY` query.
*   **Timeline ▶:** Dive into a different dataset featuring temporal data: **Heroic-Age Antarctic expeditions** (Wikidata, CC0). Watch a cursor sweep through the years (1896–1918) as expeditions grow and crews appear. 

## Interface Features

Look for these handy tools in the header:

*   **⿻ Hide Code:** Collapses the editor, giving you a clean, presentation-ready view of the visualization.
*   **⬇ Standalone:** Downloads your *current program* as a single, self-contained HTML file. It embeds D3 and the rete-graph engine without needing a CDN or the editor. It runs anywhere—just host or share it!
*   **◐ Theme:** Toggles between light and dark modes. Both color palettes are colorblind-safe and automatically update your charts. (The standalone export will bake in whichever theme is currently active.)

## Under the Hood

Curious how the magic happens?

*   **Web Worker:** The Rete engine's remote reader uses **synchronous XHR range requests**, which browsers only allow inside Web Workers. We boot a tiny worker that loads the CDN bundle and holds the opened graphs. You interact with a friendly Promise-based API: `openGraph(url)` → `{ query, prefixSearch, textSearch, stats, contentHash }`.
*   **Caching:** Graphs stay **resident in the worker** between runs. This means re-runs reuse the block cache and the decoded dictionary, making subsequent queries nearly instantaneous.
*   **Data Types:** Query rows arrive as `{variable: term}` objects. Each term has a `.kind` (e.g., `iri`, `literal`, `bnode`) and a `.value` (plus optional `.datatype` or `.lang`). Note that numbers from aggregates are literal terms; coerce them in JS using `+row.n.value`.
*   **Environment Variables:** Your program runs with five helpful bindings already in scope: 
    *   `openGraph`
    *   `d3`
    *   `viz` (the output container element)
    *   `log(...)` (the console strip)
    *   `theme` (the active color palette)

You can point `openGraph()` at **any** `.rete` URL, provided the host supports `Range` requests with CORS enabled (see [Hosting](hosting.md)). All datasets in the [Playground](playground-guide.md) catalog meet these requirements!

## Credits & License

*   **Data:** The [Linked Jazz Project](https://linkedjazz.org/) (CC BY-SA) — Thanks for a wonderful graph!
*   **Libraries:** [D3](https://d3js.org) (ISC), [CodeMirror](https://codemirror.net) (MIT), served by [jsDelivr](https://www.jsdelivr.com/).
*   **Source & Issues:** [caviri/rete on GitHub](https://github.com/caviri/rete)
*   **License:** © 2026 Carlos Vivar Ríos, released under the [Apache License 2.0](https://github.com/caviri/rete/blob/main/LICENSE).
