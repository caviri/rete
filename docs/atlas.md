# Historical Atlas — SPARQL + GIS over time

**[▸ Launch the atlas](atlas-app.html)** — a single, fully static HTML page. No
server, no database, no map tiles, no build step: the whole thing runs in your
browser over a `.rete` file through the WebAssembly engine — either the copy
**embedded in the page** (offline) or the same file **streamed from remote
storage** by HTTP range.

It is **half SPARQL, half GIS**: a query editor on the left, a world map in the
centre, and a **temporal timeline** along the bottom. Pick one of the bundled
example queries, drag the timeline and the borders of the world **cross-fade** to
that era, hit **▶ Play** to sweep through history, or click anywhere on the map
and it names the territory under your cursor. The map is a dependency-free canvas
— a curated palette, an ocean gradient, decluttered on-map labels, hover/selection
highlighting, **zoom-to-fit** (and keyboard timeline nav) — drawn entirely from
query results.

<figure class="fig-center">
  <img src="img/atlas-1914.png" alt="The Historical Atlas at 1914 CE: a SPARQL example-query picker and editor on the left over a world map of the empires of 1914, with a temporal timeline at the bottom whose markers include an 'Outbreak of World War I · 1914 CE' event tooltip.">
  <figcaption>The atlas at <b>1914 CE</b> — every border polygon comes from a GeoSPARQL query over the <code>history.rete</code>, drawn on a dependency-free canvas. Left: the example picker, the live query, the data-source selector, and the territory legend. Bottom: the era timeline, with historical-event markers (hover for a tooltip).</figcaption>
</figure>

## What you're looking at

The dataset is `history.rete` — world territorial borders at seven snapshots
from **323 BCE to 1994 CE** ([aourednik/historical-basemaps](https://github.com/aourednik/historical-basemaps),
GPL-3.0). Each border is stored as a GeoSPARQL `geo:wktLiteral` polygon with an
integer `ex:year`, so a snapshot is just `FILTER(?year = …)` and the geometry is
whatever your query binds to `?wkt`.

## How it works

1. **The map is driven by SPARQL.** The editor holds a query; *every row it
   returns that binds `?wkt`* is parsed (a tiny in-page WKT reader) and drawn —
   so you can swap in any query: a bbox `geof:sfIntersects`, a `geof:distance`
   ranking, a `geof:envelope`, a `FILTER` on the label, anything. The
   **example picker** ships six ready-made queries (era borders, nearest
   territories to London, what touches Europe, empires by name, bounding boxes,
   a count); each carries a `{YEAR}` placeholder the timeline fills in.
2. **The timeline is the temporal control — and it's continuous.** It runs
   year-by-year from −323 to 1994; the discovered `ex:year` snapshots and a row
   of **historical-event markers** (death of Alexander, fall of Constantinople,
   1789, 1914, the fall of the USSR…) sit on the track — hover a marker for its
   name, click it to jump there (arrow keys nudge ±1/±10 years; Space toggles
   play). Scrub to any year and the map **cross-fades** to the nearest snapshot
   ("borders of 1815 CE"); **▶ Play** animates through history at a **speed** you
   choose (slow → very fast).
3. **Two views of every result.** **Map** draws the polygons; **Table** shows the
   raw result rows — so a `geof:distance` ranking reads as an ordered list of
   territories and kilometres, not just shapes.
4. **Click to identify.** A click runs
   [`geof:sfContains`](geosparql.html) for the current era against every border
   polygon and reports the territory the point fell inside — e.g. clicking North
   Africa in 1000 CE returns *Fatimid Caliphate*, Paris in 1914 returns *France*.
5. **Pick where the data lives.** The data-source selector runs the *same*
   queries three ways: **embedded** (the `.rete` baked into the page, fully
   offline), **remote · lazy** (the file stays on remote storage and each query
   faults in only the byte ranges it touches, over a Web Worker), or
   **remote · cached** (download the file once, then query locally). It's the
   "simple file, remote; logic in the browser" story made switchable.

<figure class="fig-center">
  <img src="img/atlas-table.png" alt="The Historical Atlas in Table view: the 'nearest territories to London' GeoSPARQL distance query at 1000 CE, listing England at 0 km, Kingdom of France at 113 km, Holy Roman Empire at 240 km, and so on down the result rows.">
  <figcaption>The same query as a <b>table</b>: <code>geof:distance(?w, "POINT(0 51)", uom:metre)</code> ranks the territories of 1000 CE by distance to London — England 0 km, France 113 km, the Holy Roman Empire 240 km — straight from the embedded <code>.rete</code>.</figcaption>
</figure>

Everything is computed by the same Rust engine that powers the CLI, compiled to
WebAssembly — see [GeoSPARQL](geosparql.html) for the spatial functions and
[Browser / WASM](browser.html) for how the engine runs client-side (including
the lazy HTTP-range reader).

## Build it yourself

The page is assembled by inlining the no-modules WASM engine and the embedded
`.rete` into a template (the same offline-only pattern as the playground); the
remote `lazy`/`cached` modes point at the same file served by HTTP range:

```sh
# 1. the embedded dataset (simplified for an in-browser-sized file)
python3 scripts/geo_to_rete.py basemaps \
  --years bc323,1000,1492,1815,1914,1945,1994 --prec 2 --min-bbox 0.3 \
  --max-per-year 90 -o dev/geo/history.nt
rete build dev/geo/history.nt -o web/history.rete

# 2. the browser engine, then the page
wasm-pack build crates/rete-wasm --target no-modules --out-dir ../../web/pkg-nomodules
python3 scripts/build_atlas.py            # → docs/atlas-app.html
```
