# graph-map — a `.rete` community pyramid as a zoomable map (side experiment)

> **Experimental plug-in, not part of the core tool.** It only *reads* a
> `.rete` (via `rete summary`) plus the community pyramid already inside it, and
> emits a standalone `.pmtiles` + a viewer. The format and CLI are untouched.

## The idea

A `.rete` file already carries a **Louvain community pyramid** — a multi-level
clustering of the graph. This turns that hierarchy into a **slippy map**:

- **zoomed out** → a handful of coarse *super-communities* (the "continents"),
  the highest-connectivity hubs,
- **zoom in** → finer rete pyramid communities resolve, level by level.

Each granularity level becomes a map **zoom band**, exactly like map tiles go
from countries → cities → streets. Node size ∝ connectivity (summed superedge
weight), so the most-connected hubs dominate at low zoom. The 2D positions are a
force-directed **layout**, not a measurement — proximity ≈ relatedness, axes
mean nothing.

[PMTiles](https://github.com/protomaps/PMTiles) is the natural delivery format:
a single file, HTTP **range**-read, no server — the same publish-and-query-by-
byte-range story as `.rete` itself.

## How it works

```
rete summary <file>            -> weighted community graph (pyramid round 0)
igraph multilevel Louvain      -> coarser super-communities (extra zoom levels)
igraph DRL layout              -> 2D coords for the base communities
size-weighted centroids upward -> coords for every coarser level
GeoJSON (per-feature minzoom)  -> tippecanoe -> graphmap.pmtiles
```

The base layer is rete's own pyramid round-0 communities; coarser zoom levels
are produced by recursively coarsening that graph with multilevel Louvain — the
same algorithm family the pyramid is built with — so "each community level = one
zoom level" holds end to end.

## Run it

Build the (standalone) image once, then point it at a `.rete`. It calls the
rete binary already built at `target/release/rete`, so build that first if you
haven't (`docker run … rete-dev cargo build --release -p rete-cli`).

```sh
docker build -t rete-graphmap -f experiments/graph-map/Dockerfile experiments/graph-map

# from the repo root (mounts the repo so it can read the .rete + the binary):
docker run --rm -v "${PWD}:/work" -w /work rete-graphmap \
  data/wikidata-100MB/wikidata.rete -o experiments/graph-map/out
```

Outputs in `out/`:

| file | what |
|------|------|
| `graphmap.pmtiles`  | the zoomable vector-tile map (range-readable) |
| `graphmap.geojson`  | the intermediate features (one per (super)community + coarse edges) |
| `graphmap.json`     | metadata: per-level counts, maxzoom |

### View in the browser

`viewer.html` is a static MapLibre GL page using the `pmtiles://` protocol — no
build step. Serve the folder and open it:

```sh
python -m http.server -d experiments/graph-map 8000
# http://localhost:8000/viewer.html         (reads out/graphmap.pmtiles)
# http://localhost:8000/viewer.html?pmtiles=https://…/graphmap.pmtiles   (remote, range-read)
```

## Options

| flag | default | meaning |
|------|---------|---------|
| `-o/--output`  | `experiments/graph-map/out` | output dir |
| `--name`       | `graphmap` | output basename |
| `--max-base`   | `60000` | cap base communities laid out (keep top-N by connectivity) |
| `--zoom-pad`   | `2` | extra tile zoom levels past the finest community level (zoom headroom) |
| `--footprint`  | `55` | half-width (deg) of the centered layout box; smaller ⇒ more zoom-out room |
| `--rete-bin`   | `./target/release/rete` | rete binary to call |

## What the viewer shows

- **Boundary polygons** — each super-community is a translucent convex hull; its
  finer child communities render inside it as you zoom in.
- **Labels** — super-community size; hover any node for level / size /
  sub-community count / id.
- **Click to trace connections** — click a node to highlight all its links;
  click a 2nd/3rd to show only the links *among* the selected set; click empty
  space (or *clear*) to reset. Links exist at the super levels (the base level
  is nodes-only — see caveats).
- **Zoom out** to see the whole graph as a small island; zoom in to resolve it
  level by level.

## Caveats (it's an experiment)

- **Layout is the cost, not the tiling.** DRL handles ~100k base communities;
  past that, `--max-base` keeps the most-connected ones. Full per-triple layout
  is out of scope here.
- **Edges** are drawn only at the coarse super levels (few, meaningful);
  base-community edges are omitted to keep tiles light.
- **Coordinates are arbitrary** (a layout). Don't read geographic meaning into
  them.
