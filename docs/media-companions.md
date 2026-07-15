# Media & SQL companions

A `.rete` file holds the graph; a *published dataset* is usually a little more
than that. This page covers the two kinds of companion that make a dataset shine
in the [playground](playground-guide.md): **media** — images, IIIF pages, 3D
models, audio, video and maps rendered inline in query results — and **SQL
companions** — the same triples as Parquet/DuckDB/SQLite files for the Explore
tab. The first half is for anyone publishing a dataset; the pipeline recipes and
gotchas at the end are development notes.

## How result cells render

A SELECT (or triples) result cell whose value is an IRI/URL is rendered
**inline** when its URL matches a media heuristic — an image becomes a
thumbnail, a `.glb` becomes a rotatable 3D model, a `.mp3` an audio player, a
WKT literal a mini-map, and so on. The relevant code is the cell renderers in
`web/playground-src/app.js` (`autoCell`, `prettyCell`, and the `looks*Url` /
`*Cell` pairs).

`autoCell` is the default per-value heuristic. For an IRI it tries each detector
in order and falls through to a plain link / text; for a literal it checks for a
WKT geometry. A column header dropdown (`COL_TYPES`) can **force** a render type,
overriding the heuristic — useful for an image/3D column whose URLs don't end in
a known extension (a CDN/API URL or a bare entity IRI), or to stop a long IRI
column from rendering.

URL pattern → renderer (the detector functions, in `autoCell`'s order):

| value looks like | detector | renders as |
| --- | --- | --- |
| Wikimedia Commons `Special:FilePath/…`, a `coeli` portraitMedia/IIIF-Image path, or `.jpg .jpeg .png .gif .svg .webp` | `looksImageUrl` → `imageCell` | clickable thumbnail (Commons gets `?width=200`) |
| `…/manifest(.json)` or `…/iiif/…` | `looksIiifUrl` → `iiifCell` | placeholder, then async-fetch the manifest and swap in a paged thumbnail; click → lightbox (enlarged page + paging + manifest metadata). Supports IIIF Presentation v2 and v3 |
| `.glb .gltf .ply .splat .ksplat` | `looksMeshUrl` → `mesh3dCell` | inline `<model-viewer>` (drag-rotate, auto-spin, ⛶ lightbox with lighting/exposure/shadow controls and a real-world scale bar). The web component lazy-loads from the jsDelivr CDN on first appearance; each viewer lazy-loads its own mesh (`loading="lazy"`) so a 60-row table doesn't fetch 60 meshes at once |
| INSCRIBE (`inscribercproject.com`), PAITO (`paitoproject.it`), `sketchfab.com/(3d-models|models)/…`, `.nxz` (Nexus), or `/3dhop` | `looks3dViewerUrl` → `viewer3dCell` | a `🧊 3D ↗` launch link (these are HTML viewer pages / usually all-rights-reserved, not embeddable meshes) |
| `.mp3 .wav .ogg .oga .flac .m4a .aac .opus` | `looksAudioUrl` → `audioCell` | inline `<audio controls preload=metadata>` |
| a bucket `…-spin/<id>.webm` (or `.mp4`) | `looksSpinUrl` → `spinCell` | autoplaying, muted, looping clip (a pre-rendered turntable preview — no WebGL) |
| `.mp4 .webm .ogv .m4v .mov` | `looksVideoUrl` → `videoCell` | inline `<video controls preload=metadata>` |
| `.pdf` | `looksPdfUrl` → `pdfCell` | a PDF launch button in Auto; force **PDF viewer** for an inline page canvas with paging and an enlarged modal |
| a WKT literal (`POINT`/`POLYGON`/`LINESTRING`/… — `looksWktGeo`) | `geoCell` | a small locator mini-map (a point sits on a cached world basemap tile; a shape fits its own bbox). With many geo rows, **Output → Map** plots the whole result set |

Notes:

- Each media cell gets a **caption** (`hydrateMediaMeta`): `format ·
  dimensions/duration/real-size · file size`. The file size comes from a
  best-effort `HEAD` request; dimensions/duration/3D real-size come from the
  loaded element (`updateScaleBar` drives the lightbox scale bar from the live
  camera).
- The column dropdown options (`COL_TYPES`) are **Auto / Text / Link / Button /
  Image / IIIF / PDF viewer / Page preview / Markdown / Map / 3D / Audio /
  Video / Spin / Number**. **Page preview** is opt-in and lazily places a
  sandboxed, scaled desktop iframe in the cell; **Markdown** is opt-in for RDF
  literals and escapes raw HTML while allowing only `http:`, `https:`, and
  `mailto:` links.
- Every rich-media cell retains a contextual **Open … ↗** source link below its
  preview, including images, PDF, IIIF, audio/video/spin, 3D, and Page preview.
- **PDF page modal.** For a forced PDF viewer, clicking the inline canvas opens
  the current page in a larger paged modal. It reuses the same PDF.js document,
  so the modal does not start a second file request. Linearized PDFs give the
  quickest first page when the server supports byte ranges; non-linear PDFs are
  still valid, but PDF.js may request the tail first or download the whole file.
- **Image hover-zoom.** Hovering an image thumbnail (`imageCell`/IIIF) pops a larger
  preview on fine-pointer desktops, anchored beside the cell and clamped to the
  viewport (never clipped by the table's scroll). It is disabled inside focused
  cards and lightboxes. A Commons `?width=200` thumb is re-requested at
  `width=900` so the zoom is sharp. Click still opens the full image / lightbox.
- **Focused cards.** The desktop dialog is wide enough to show neighboring cards
  and exposes its horizontal scrollbar. Prev/Next, Left/Right, horizontal
  trackpad input, Shift+wheel, and mouse drag from a non-interactive card area
  all move the native scroll-snap carousel; touch swipe remains unchanged.
- **Geo cell → full map, multi-LOD.** Clicking a mini-map opens a pannable/zoomable
  Leaflet map. For a dataset that ships a finer level of detail (e.g. `geoadmin`'s
  `g:geomFine` alongside the coarse `geo:asWKT`), the modal range-fetches **just that
  feature's** fine geometry on demand and swaps it in — the remote-lazy LOD payoff
  (zoom in → fetch only what you inspect). GeoSPARQL has no native tiling, so this is
  modelled with a second geometry property.
- **Tiles output (vector basemap).** A dataset can carry a PMTiles archive
  (`CATALOG.pmtiles[key]`) — built with tippecanoe for true per-zoom LOD — rendered in
  the **Output → Tiles** view by protomaps-leaflet (Canvas, no WebGL). The tiles draw
  all the geometry; the SPARQL result features are highlighted on top (joined by name).
  The PMTiles can sit next to the `.rete` (a separate file) or **inside** it as a
  section (`embedded: true`; the reader parses the header for the section offset and
  range-reads tiles from the same URL) — one file = graph + map tiles.
- The page is served over HTTPS, so an `http://` media URL is upgraded to
  `https://` for the fetch (`httpsUpgrade`); the original IRI is still shown.
- **CORS is a hard requirement for media fetched by JavaScript.** A PDF/IIIF/3D
  URL used inline must send `access-control-allow-origin`; range-streamed PDFs
  must also support byte ranges and expose the headers PDF.js reads. Native
  image/audio/video elements have their own browser rules. Page-preview iframes
  do not use CORS, but the remote site's `X-Frame-Options` or CSP
  `frame-ancestors` can refuse embedding. Every failure retains its direct source
  link (a blocked IIIF manifest also degrades to `⚠ IIIF blocked`). The project's
  own storage is CORS-open; so are Wikimedia Commons, most IIIF servers, and
  `iiif.coeli.cat`.

## Preparing media for a dataset

To make a cell render inline, emit a triple whose object is a CORS-open URL (or,
for geo, a WKT literal) on a property the renderers recognise. `prop:` below is
each dataset's own property namespace; the renderers key off the **value**, not
the predicate name, so any predicate works — these are just the conventions used
by the existing datasets.

| media | emit | object |
| --- | --- | --- |
| image | `prop:image` | a CORS-open image URL or a IIIF manifest |
| geo | `geo:asWKT` (`http://www.opengis.net/ont/geosparql#asWKT`, a `geo:wktLiteral`) | a WKT geometry literal |
| audio / video | `prop:audio` / `prop:video` | a CORS-open media file |
| streamable 3D | `prop:mesh` | a hosted `.glb` URL |
| turntable spin | `prop:spinVideo` / `prop:spinGif` | a hosted `…-spin/<id>.{webm,gif}` URL |

`scripts/bioexplora_to_nt.py` and `scripts/smithsonian3d_to_nt.py` are the worked
examples (specimens → `prop:image` + `geo:asWKT`; 3D models → `prop:mesh` +
`prop:spinVideo`/`prop:spinGif`).

## SQL companions (the Explore tab)

The playground's **Explore** tab queries the *same data* through DuckDB-WASM and
SQLite-WASM, side by side with the rete engine — both fetch lazily over HTTP
ranges (`httpfs` / VFS), so the comparison is transport-fair.
`skills/rete-publish/scripts/make_companions.py` emits the flat, lossless
`triples` table from the same N-Triples the `.rete` was built from:

```sh
python skills/rete-publish/scripts/make_companions.py foo.nt -o foo --parquet --duckdb --sqlite
#  → foo.parquet, foo.duckdb, foo.sqlite
#    columns: subject, predicate, object (raw token), otype, value, datatype, lang
```

Upload them next to the `.rete` and register them under `CATALOG.companions[key]`.
For a richer one-wide-table-per-class layout, use
`scripts/rdf_to_entity_tables.py` instead — see
[Tables, VKG & big builds](data-engineering.md).

## Pipeline recipes (development)

### Streamable 3D (.glb)

Source meshes are usually too big to stream (20–50 MB). Compress with
`gltf-transform optimize` (Draco geometry + WebP textures, ~40× smaller, e.g.
20–50 MB → ~0.5–1 MB), run inside the `rete-gltf` node image:

```bash
docker build -f scripts/Dockerfile.gltf -t rete-gltf .
docker run --rm -v "$PWD":/work -w /work rete-gltf \
  gltf-transform optimize in.glb out.glb --compress draco --texture-compress webp
```

Upload the compressed `.glb` (below) and emit `prop:mesh` with its URL.
`scripts/bioexplora_sketchfab.sh` is the end-to-end worked example:
Sketchfab Data API → download each downloadable `.glb` → Draco+WebP compress →
upload → write a `uid → mesh-url` TSV that `bioexplora_to_nt.py` reads.

### Turntable spin previews

A pre-rendered rotating clip is a lightweight preview that plays like a GIF and
needs no WebGL on the client. Render it with headless Blender (Cycles on
OptiX/CUDA GPU) in the `rete-blender` image (the official Blender 3.6 LTS tarball
— it ships the glTF/Draco importer and bundled numpy):

```bash
docker build -f scripts/Dockerfile.blender -t rete-blender .
# batch over a "id<TAB>glb_url" TSV; GPU + a mounted OptiX kernel cache:
docker run --rm --gpus all -e NVIDIA_DRIVER_CAPABILITIES=all -e BLENDER_GPU=1 \
  -v "$PWD":/work -v "$PWD/data/.blendercache":/root/.cache -w /work rete-blender \
  sh scripts/render_turntables.sh data/<ds>/mesh_list.tsv data/<ds>/turntables
```

`render_turntables.sh` loops the TSV → `glb_to_spin.sh` (per model) →
`blender_turntable.py` (the Blender script: import, centre, frame a camera, spin
0→360°, write PNG frames), then ffmpeg makes a tiny VP9 `.webm` plus a
palette-optimized `.gif` fallback. Upload under a `…-spin/<id>.{webm,gif}` prefix
and emit `prop:spinVideo` / `prop:spinGif`.

### Uploading to storage

The playground's datasets and media are served from Cloudflare R2 at
`https://data.graphplaza.com/<key>` — CORS-open, direct `206` range responses,
no token. Upload with the S3 helper (credentials from env, never committed):

```bash
# env: S3_API_ENDPOINT, ACCESS_KEY_ID, SECRET_ACCESS_KEY
python3 dev/r2_s3.py put <bucket> local/file.glb datasets/<ds>/file.glb
#  → served at https://data.graphplaza.com/datasets/<ds>/file.glb
```

Any HTTP host that serves ranges + CORS works just as well — see
[Hosting your .rete](hosting.md) for the requirements and alternatives.
(Source-API secrets such as `SKETCHFAB_TOKEN` come from `.env` and must never
be committed.)

### Gotchas

- **CORS is mandatory** — see above; the single most common reason a media cell
  shows nothing is a host that doesn't send `access-control-allow-origin`.
- **Windows Python writes CRLF.** `print()` on Windows emits `\r\n`, so a
  uid/URL list piped through `python` carries a trailing `\r` that corrupts the
  next URL/header. Strip it (`tr -d '\r'`, or `uid=${uid%$'\r'}` after `read`),
  or generate the list with Linux python (e.g. inside the dev container).
- **`while read … done < file` eats the loop's stdin.** Any inner command that
  reads stdin (ffmpeg, sometimes curl/wget, a nested script) consumes the loop's
  TSV → truncated/garbled iterations. Pass `-nostdin` to ffmpeg and `</dev/null`
  to curl/wget and nested script calls inside such loops.
- **Source-API rate limits.** The Sketchfab `/download` endpoint throttles
  bursts; add a small delay between requests and retry with exponential backoff
  (see `glb_url()` in `bioexplora_sketchfab.sh`: 2,4,8,16 s).
- **Atomic writes.** Download/compress to a `.part` temp then `mv` into place, so
  an interrupted run never leaves a truncated file that a `[ -s file ] &&
  continue` resume-guard would wrongly accept as complete.
- **Blender headless.** Use the official Blender tarball (the distro `apt`
  package can lack `libextern_draco.so` and bundled numpy). Cycles renders
  headless on GPU (OptiX); on a new GPU arch the first run pays a one-time
  ~200 s OptiX kernel JIT that caches under `/root/.nv` — mount it to keep warm.
- **Image split.** `gltf-transform` runs in the `rete-gltf` node image; Blender
  in the `rete-blender` image.
