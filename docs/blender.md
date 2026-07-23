# Blender — knowledge graphs as scenes

The Blender add-on makes a `.rete` file a source of **scene content**. You write
SPARQL; the answer becomes objects — 3D assets imported, geometry placed, every
RDF property inherited onto the object as a drivable custom property, relations
turned into hierarchy or into physical constraints, and time mapped onto the
timeline. Scenes go back out as new `.rete` files, so Blender also becomes an
authoring tool for 3D knowledge graphs.

It bundles the engine (the [Python client](python.md)'s wheel), so remote graphs
are read lazily over HTTP `Range` requests from inside Blender: point it at a
multi-gigabyte graph and a selective query fetches kilobytes.

> Source and build script: [`clients/blender/`](https://github.com/caviri/rete/tree/main/clients/blender).
> Blender 4.2 or newer (tested on 4.5 LTS and 5.1).

## Install

Download `rete-<version>.zip` and use **Edit ▸ Preferences ▸ Add-ons ▸ ⌄ ▸
Install from Disk…**. The engine ships inside the extension — no pip, no
network, no build tools. The panels appear in the 3D viewport sidebar
(<kbd>N</kbd>) under the **rete** tab.

Building it yourself, in the container:

```sh
docker build -t rete-blender clients/blender
docker run --rm -v "$PWD":/work -w /work rete-blender sh clients/blender/build.sh
```

## The loop

1. **Graph** — paste a `.rete` URL (or pick one from the preset menu) and hit
   *Open graph*. The file describes itself: title, licence, counts, and the
   library of [example queries](dataset-cards.md) that travels inside it.
2. **Query** — start from an example or write your own; it lives in a Blender
   text block, editable in the Text Editor.
3. **Run query** — the **Columns** panel reports what each column was understood
   to be, and lets you override it.
4. **Build scene**.

## How columns become scenes

The add-on knows no dataset's vocabulary. Each column earns a role from the
shape of its values, with the variable name and the predicate that bound it as
supporting evidence — so an arbitrary query against an arbitrary graph still
produces a sensible scene.

| Role | Recognised from | Becomes |
|---|---|---|
| 3D asset | a `.glb`/`.gltf`/`.obj`/`.fbx`/`.stl`/`.ply`/`.usd`/`.abc`/`.dae` URL, or a CAD/BIM `.ifc`/`.ifczip`/`.dxf` | an imported, cached, instanced model |
| Mesh node | a node name inside a shared asset | just that node, keeping its place in the file |
| Geometry | WKT (`POINT Z`, `LINESTRING`, `POLYGON`) or `BOX3D` | position, real mesh geometry, and size |
| Time | a date, timestamp, year, duration, or decimal seconds | a position on the frame range |
| Image | an image or IIIF URL | a texture, an upright image plane, or a 360° world |
| Video | an `.mp4`/`.webm`/`.mov`/… URL | a movie-textured plane synced to the timeline |
| Map | a `.pmtiles` URL | vector map meshes (per layer) or raster tile planes |
| Splat | a 3DGS `.ply`/`.splat`/`.ksplat` URL | the 3DGS add-on's splats, or a point-cloud preview |
| Colour | `#rrggbb`, `rgb()`, or a CSS name | the base colour |
| Number | any numeric literal | a drivable property, a colour ramp, a mass |
| Class | `rdf:type` and type-like columns | grouping and a stable per-class colour |

`geo:hasGeometry`, `geo3:asWKT3D`, `geo3:box`, `anat:glbFile`, `anat:meshNode`,
`dance:animation`, `tracking:t`, `subtitles:start` and the other published
vocabularies are pinned explicitly — which matters most for the graphs that
publish **time as bare decimal seconds**, since those are indistinguishable
from any other number by value alone.

## CAD & BIM (IFC)

A building `.rete` works two ways. Its geometry can live **in the graph** — an
IFC-derived graph (the FZK-Haus example, from [`cad-ifc`](geosparql.md)) carries
each element's `geo3:asWKT3D` and `geo3:box` in metres, its `cad:ifcClass`, and
the [BOT](https://w3c-lbd-cg.github.io/bot/) topology. Query the elements and you
get a massing model sized by bounding box, coloured by IFC class, with
`bot:containsElement` / `cad:inStorey` becoming per-storey collections,
`cad:adjacentSpace` becoming rigid-body constraints between the spaces, and
`cad:elevation` / `cad:netArea` / `cad:grossVolume` inherited as drivable
numbers.

Or the graph can point at a **raw `.ifc` file** (via `cad:ifcModel`,
`cad:ifcFile`, or any `.ifc` URL). It is imported element by element at true
world coordinates, each mesh carrying its `ifcGuid` and `ifcClass`. That path
needs **ifcopenshell** in Blender's Python (`<blender-python> -m pip install
ifcopenshell`) or the **Bonsai** add-on; it is not bundled, being far larger
than the engine itself. Without either, IFC rows degrade with a clear message
and everything else still builds — and most CAD graphs also ship a
`cad:glbModel` column that needs no extra install. `.dxf` uses the importer
Blender already ships; `.step` has no core importer.

## Maps, images & video

Beyond 3D models, three URL kinds become scene content.

**PMTiles maps.** A `.pmtiles` URL — a whole tiled map in one immutable,
HTTP-range-readable file, the same idea as `.rete` — is read directly, fetching
only the byte ranges the build touches (a continent's boundaries in a few
hundred KB). Vector tiles (MVT) are decoded into one mesh **per layer**, coloured
per layer, optionally extruded, and projected into the same geographic frame as
any points drawn on top of them; raster tiles become textured planes. The reader
and the MVT decoder are pure Python — no new dependency. Zoom, tile budget and
extrusion are set in the **Media & maps** panel.

**Images.** An image or IIIF URL is a textured material by default; it can
instead become an upright **image plane** at the entity's position (sized to the
picture's aspect), or an equirectangular panorama can become the scene's **360°
world** environment.

**Video.** An `.mp4`/`.webm`/`.mov`/… URL becomes an upright plane whose texture
plays, **synced to the scene's frame range** — a graph of clips laid out in
space, playing as you scrub. It uses Blender's own movie reader; a build without
FFmpeg degrades cleanly.

**Gaussian splats.** A 3DGS splat URL (`.ply` — sniffed apart from a mesh `.ply`
— `.splat`, `.ksplat`, `.spz`) is handled like IFC: if a 3DGS add-on such as KIRI
Engine's *3DGS Render* is installed, it imports and renders the real splats;
otherwise an add-on-free fallback parses the Gaussian centres and colours into an
honest point-cloud preview, with a note about installing the add-on. Splats are
parented to an empty and placed by moving the empty, so their stored attributes
(position, scale, rotation, spherical-harmonic colour, opacity) are never
desynced by an ordinary transform. `.ksplat`/`.spz` are convert-to-`.ply` for the
preview.

## Inherited properties

With **Inherit all properties**, every statement about every imported entity is
fetched — batched, so a thousand entities cost a handful of queries — and
written onto the object as custom properties. They appear in **Object Properties
▸ Custom Properties** with the predicate IRI as the tooltip, numeric ones are
**drivable** (right-click ▸ *Copy as New Driver*), Geometry Nodes can read them,
and the local-name → predicate-IRI map travels with the object so the export
round-trips losslessly.

That is the whole idea: a bone's tissue type, a building element's IFC class, a
paper's citation count stop being metadata in a table and become quantities that
drive geometry, shading and simulation.

## Placement, honestly

Graphs are authored in millimetres (anatomy), metres (buildings), or degrees
(maps). The **Placement** panel folds unit scale, a *fit to size* mode, Y-up→Z-up
conversion, an X flip and recentring into one transform applied to the whole
result.

Geographic coordinates are projected to metres — but only on evidence that they
*are* degrees (a WKT literal, or columns named lon/lat). A football pitch is
105 × 68 metres, comfortably inside the longitude/latitude envelope; projected
as degrees it would scatter across half a continent. See
[GeoSPARQL](geosparql.md) for the geometry vocabulary itself.

## Time

**Appear** keys objects in and out at their moment (with constant interpolation,
so what you scrub is what you render). **Grow in** scales them up. **Motion
path** turns rows sharing an entity into one keyframed trajectory — and works
out which column names the moving thing, since trajectory datasets give every
sample its own IRI. **Retime assets** places each asset's own animation at its
own moment.

## Relations, and relations as physics

One predicate, read three ways: as Blender's **object hierarchy** (a partonomy
becomes the Outliner), as **collections**, or as **edge geometry** — one line
per statement in a single mesh, ready for a Skin or Wireframe modifier.

Then the one worth trying. Under **Physics**, a predicate becomes a network of
**rigid-body constraints**: every statement is a physical link between two
objects, fixed or hinged or springy, with mass read from a numeric property and
normalised into a usable band. Anatomical adjacency, a building's topology, a
citation network — the graph stops being a diagram and becomes a structure that
holds itself together, which you can then pull apart to see what the topology
actually does.

## Scale

**As point cloud** writes the whole result into one attributed mesh — a vertex
per row, numeric and colour columns as named attributes, categorical ones as an
integer index plus a lookup table — with a Geometry Nodes instancer attached.
That carries results far past what one-object-per-row can.

## Live values in drivers

```python
rete_count("?s a <https://w3id.org/rete/anatomy#Muscle>")
rete("SELECT (AVG(?m) AS ?avg) WHERE { ?s <https://x.org/mass> ?m }", variable="avg")
```

Both are registered in Blender's driver namespace and memoised, since a driver
is evaluated on every redraw.

## Exploring, and exporting

Select an object and **Expand neighbours** pulls its graph neighbours into the
scene with the connecting edges drawn; **Select by query** runs SPARQL and
selects what it returns.

**Export** writes any scene as a queryable `.rete`: objects, transforms,
hierarchy, collections, materials, mesh statistics, animation ranges and every
custom property, with original predicates restored and imported entities keeping
their IRIs. The vocabulary is `https://w3id.org/rete/scene#`, described inside
the exported file along with runnable example queries.

## Graphs to try

`z-anatomy` (4,884 human structures with per-system `.glb`), `smithsonian3d`
(2,199 CC0 models), `dance` (salsa duets as animated skeletons), `bioexplora`
(specimen scans), `scrolls` (Herculaneum segment meshes), `tracking` (player
positions over time), `geoadmin` (boundaries to extrude) — all at
`https://data.graphplaza.com/<key>/<key>.rete`, and the rest in the
[playground](playground-guide.md).
