# graphkit

A self-contained graph view for retelens: force, hierarchical, radial, circle, grid, tree, hive plot,
circos and a chord diagram, with legend, minimap, filters, search, shortest path and exports. One
file, `graphkit.js`, exposes `window.GraphKit`. No remote code, no ES modules: it works as a plain
content script.

```
lib/graphkit/graphkit.js      built bundle (IIFE, ~1.4 MB, worker source included)   <- load this
lib/graphkit/graphkit-worker.js  the same worker as a stand-alone file (optional, for `workerUrl`)
lib/graphkit/build.sh         rebuild (Docker node:22, --memory 4g)
lib/graphkit/src/             sources (esbuild entry: src/index.js, CSS is bundled into the JS)
vendor/graphkit/*.LICENSE     licence of every bundled library (MIT / ISC only)
test/graphkit/                demo page, fixtures, Playwright run, screenshots
```

Bundled: cytoscape, cytoscape-fcose (+ cose-base, layout-base), cytoscape-dagre (+ dagre, graphlib, lodash),
d3-chord + d3-shape (+ d3-path), graphology + graphology-communities-louvain (+ mnemonist, pandemonium, ...).
`cytoscape-svg` was **not** used: it is GPL-3.0. SVG export is our own (`src/svgexport.js`).

## Load it

```json
"content_scripts": [{ "js": ["lib/graphkit/graphkit.js", "content/main.js"] }]
```

`graphkit.js` must come before the script that calls `GraphKit.mount`. It injects its CSS once per
document through a constructable stylesheet (so a page CSP that forbids inline `<style>` does not matter),
falling back to a `<style>` element. All rules are scoped under `.gk-root`. It also works inside a
shadow root if the container lives in one.

## API

```js
const kit = GraphKit.mount(containerEl, {
  skin,      // "hf" | "github" | "zenodo" | "standalone", or { base: "hf", classes: {...} }
  classes,   // optional class map overrides, merged over the skin
  tokens,    // { surface, surface2, text, textMuted, border, accent, fontSans, fontMono, radius }
  dark,      // boolean
  onExpand,  // async (nodeId, "out" | "in" | "both") => ({ nodes, edges })
  onOpen,    // (iri) => void   called only for http(s) IRIs
  labelFor,  // optional (id) => string; wins over node.label when it returns a non-empty string
});
```

`containerEl` needs a height (the root is `height:100%`, min 380 px). `tokens` must be resolved
CSS values (`#fff`, `rgb(...)`, font stacks), not `var(--x)`: cytoscape and the canvas minimap cannot
resolve custom properties. Missing tokens fall back to a light or dark default.

Data:

```js
kit.setData({
  nodes: [{ id, kind: "iri" | "bnode" | "literal", label, types: [iri], props: { k: v },
            // optional, for literals: value, datatype, lang. Optional for IRIs: iri (defaults to id)
          }],
  edges: [{ id, source, target, predicate /* full IRI */, label }],
});
kit.addData({ nodes, edges }, { near: nodeId, layout: true });   // merge; new nodes appear around `near`
```

Merging is by node id and by (source, predicate, target). An edge to an unknown id creates a stub node
(kind guessed from the id: `_:` is a blank node). `props` values are strings or arrays of strings.

Other methods:

| call | what |
| --- | --- |
| `kit.setTheme({ tokens, dark })` | re-skin live (colours, fonts, legend, minimap, hive/circos decoration) |
| `kit.fit()`, `kit.resize()` | fit to view; call `resize()` if the host changes the container size without the window resizing |
| `kit.runLayout(name, { incremental, noFit })` | `fcose dagre concentric circle grid tree hive circos chord`; resolves with milliseconds |
| `kit.set({ layout, groupBy, colorBy, sizeBy, labels, edgeColor, boxes, minimap, minDeg, hideLiterals, hideBnodes })` | same as the UI controls |
| `kit.getState()` | current options, hidden groups, zoom |
| `kit.search(q)` | highlights matches, returns ids |
| `kit.selectNodes(ids)`, `kit.shortestPath(a, b)`, `kit.clearPath()` | |
| `kit.expand(id, dir)` | what the panel buttons call |
| `kit.exportData(kind)` | `png png-full svg json nt` -> `Promise<Blob>` |
| `kit.stats()` | `{ nodes, edges, visibleNodes, visibleEdges }` |
| `kit.on(name, fn)` -> unsubscribe | `select unselect hover open expand layout layoutstop path filter data groupby options export pin` |
| `kit.destroy()` | removes DOM and listeners |

`GraphKit.skins` holds the built-in class maps, `GraphKit.palette`, `GraphKit.sequential`, `GraphKit.shapes` the
fixed encodings.

## Skins

A skin is a class map. Every key is optional; a missing key falls back to the standalone look
(`gk-v-*` classes, themed through the tokens). A host class replaces the standalone look completely.

| key | element |
| --- | --- |
| `button`, `buttonPrimary` | toolbar and panel buttons |
| `select`, `input` | layout / group-by / colour selects, search box |
| `panel` | details panel and popovers |
| `segmented`, `segmentedItem`, `segmentedItemActive` | Auto/All/None, Down/Up/Right |
| `badge` | kind / degree / type chips |

The class maps for `hf` (Tailwind), `github` (Primer) and `zenodo` (Semantic UI) live in `src/skins.js`.
**Tailwind purges unused classes**, see below; adding the `hf` classes to a safelist helps but is no longer required.
**You cannot rely on the host having the classes** (a purged Tailwind build only contains what the host page uses).
So graphkit checks at mount: for each component whose class list has a class that no readable stylesheet in
`document.styleSheets` (or `adoptedStyleSheets`) has a rule for, that component silently falls back to graphkit's own
styling, which is driven by the `tokens` you pass and so still matches the host colours. Components are
`button`, `buttonPrimary`, `select`, `input`, `panel`, `badge` and the `segmented` trio (all three keys count as one).
A single `console.debug("[graphkit] skin ...")` names what fell back; `kit.fellBack` has the same as data.
Turn it off with `mount(el, { skinFallback: false })`.

```js
const missing = GraphKit.checkSkin("hf");   // classes of the hf map that match no rule in the page's styles
missing.unknown;                            // not found, but some stylesheet is cross-origin and unreadable
missing.byKey;                              // { button: ["inline-flex", ...], ... }
```

`checkSkin(skin, overrides?)` accepts the same `skin` value as `mount`. Cross-origin sheets count as "unknown", never "missing",
so a skin is never dropped only because a sheet could not be read.
Tailwind build (it is a plain string list; `GraphKit.skins.hf` has it at run time).
The demo (`test/graphkit/demo.html`, `host.css`) emulates the three host stylesheets with a hand-written
subset, only to exercise the class maps.

## Behaviour notes

* **Page scroll**: the wheel zooms only after a click inside the graph, with Ctrl/Cmd held, on a trackpad
  pinch (ctrlKey wheel), or in fullscreen. Otherwise the event goes to the page and a brief "Use Ctrl + scroll
  to zoom" overlay shows. Clicking outside the graph deactivates it again.
* **Keys** (canvas focused): `+` `-` zoom, `0` 100 %, `F` fit, arrows pan, `Esc` clears selection, path and search.
* **Selection**: click one node; shift-click adds; shift-drag draws a box. Two selected nodes enable "Shortest path"
  (BFS on the visible graph).
* **Colour** is per entity: group -> slot is fixed at load by frequency (never cycled, 9th group on goes to "Other"),
  and does not change when filtering. Each slot has a fixed node shape as secondary encoding. Edges are neutral
  unless "colour edges by predicate" (same slots, plus a dash pattern, own legend).
* **Security**: node/edge text, props and types go through `textContent` only. No HTML labels, no `innerHTML`.
  Only `http(s)` IRIs are linkified or passed to `onOpen`.
* **Performance**: smooth to about 2000 nodes / 4000 edges. Above that a warning bar shows; haystack edges are used
  above 800 edges, labels are hidden while moving above 600 nodes. Layout animation only up to 300 nodes.
  Force and hierarchical layouts of 120+ nodes run in a **Web Worker** (a Blob worker built from the bundled worker source,
  or `mount(el, { workerUrl })` with an extension-origin file such as `lib/graphkit/graphkit-worker.js`; list it in
  `web_accessible_resources` for that). The page keeps painting and Stop terminates the worker. If the page CSP blocks
  workers (`worker-src`), graphkit logs one `console.debug`, remembers it (`kit.workerOk === false`) and uses the synchronous
  path, where Stop can only interrupt animations. `kit.lastLayoutVia` is `"worker"` or `"sync"`. `mount(el, { worker: false })` disables it.
* **Fit** pads by the overlays (details panel, minimap, legend, zoom buttons) so nodes never sit under them; the minimap
  moves left of the details panel when it opens.
* **Wide hierarchies**: when the widest dagre rank has more than 60 nodes the finished layout is folded into 2 to 5 stacked
  bands (View > "Wrap wide hierarchies", on by default; `kit.set({ wrap: false })`). On the 286-node ChEBI sample this fits at about
  2x the zoom of the plain layout; Right (LR) is worse (see `test/graphkit/screens/dagre-wide-*.png`).
* **Tree from selection** above 150 nodes is a radial tree whose ring radii grow with the leaf counts, so subtrees do not collide.
* **Exports**: PNG (view or whole graph), SVG (own writer: nodes, edges, labels, hive axes / circos arcs; chord view
  exports its own SVG), JSON (visible nodes/edges), N-Triples (visible edges; literal nodes only as objects), copy image.

## Building

`sh lib/graphkit/build.sh` (needs Docker). It installs the pinned packages from `src/package.json` in a throwaway
`node:22` container (`--memory 4g`), bundles with esbuild and refreshes `vendor/graphkit/*.LICENSE`.

## Tests

`sh test/graphkit/run-docker.sh run.mjs` runs Playwright (`mcr.microsoft.com/playwright:v1.49.0-noble`, 4 GB) against
`test/graphkit/demo.html` served from a tiny static server, writes `test/graphkit/screens/*.png` and
`test/graphkit/results.json`. `ONLY=layouts,perf` selects groups. Fixtures: `sh`-free, see
`test/graphkit/tools/make-fixtures.mjs` (Docker `node:22`, `rete-graph@0.7.8`, SELECT with LIMIT only).
