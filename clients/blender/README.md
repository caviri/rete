# rete for Blender — knowledge graphs as scenes

Query a `.rete` knowledge graph with SPARQL, and turn the answer into a Blender
scene: 3D assets imported, geometry placed, **every RDF property inherited as a
drivable custom property**, relations expressed as hierarchy or as physical
constraints, and time mapped onto the timeline. Scenes go back out as new
`.rete` files.

A [`.rete`](https://github.com/caviri/rete) file is a whole RDF graph in one
immutable, range-queryable file. Point the add-on at a URL and it reads only the
byte ranges each query touches — the human anatomy graph below is 4.9 MB on a
server and answers a query in well under a megabyte, with nothing to download
and no server in between.

## Install

Download `rete-<version>.zip` and use **Edit ▸ Preferences ▸ Add-ons ▸ ⌄ ▸
Install from Disk…**. The engine ships inside the extension, so there is no pip
step and no network needed to install. Blender 4.2 or newer; the panels live in
the 3D viewport sidebar (press <kbd>N</kbd>) under the **rete** tab.

To build the zip yourself:

```sh
docker build -t rete-blender clients/blender
docker run --rm -v "$PWD":/work -w /work rete-blender sh clients/blender/build.sh
```

## Five minutes in

1. **Graph** panel → pick a graph from the preset menu, or paste a URL, and hit
   *Open graph*. The file describes itself: title, licence, counts, and a
   library of example queries that travel inside it.
2. **Query** → *New query*, or load one of the file's own examples. Edit it in
   Blender's Text Editor.
3. *Run query*. The **Columns** panel shows what each column was understood to
   be — an asset, a position, a moment in time — and lets you correct it.
4. *Build scene*.

Try this against the anatomy graph
(`https://data.graphplaza.com/z-anatomy/z-anatomy.rete`), which places 4,884
human structures in a real anatomical frame:

```sparql
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geo3: <https://w3id.org/rete/geo3#>
PREFIX anat: <https://w3id.org/rete/anatomy#>
SELECT ?s ?label ?wkt ?box ?tissue WHERE {
  ?s a anat:AnatomicalStructure ; rdfs:label ?label ;
     geo:hasGeometry ?g ; anat:tissueType ?tissue .
  ?g geo3:asWKT3D ?wkt ; geo3:box ?box .
  FILTER(langMatches(lang(?label), "en"))
} LIMIT 300
```

Set **Placement ▸ Scale** to *Millimetres* and tick **Flip X** (the anatomy
graph's +X is the subject's left; Blender's is the viewer's right). You get 300
structures, each sized by its real bounding box, coloured by tissue, and every
one carrying its full RDF description as custom properties.

## What it does

### 3D assets

A column holding a `.glb`, `.gltf`, `.obj`, `.fbx`, `.stl`, `.ply`, `.usd`,
`.abc`, `.dae` — or a CAD/BIM `.ifc`, `.ifczip`, `.dxf` — URL is recognised as an
asset and imported. Files are cached on disk and imported **once**: repeated
references become linked copies sharing one mesh, so a result whose rows all
point at the same file costs one download.

When a row also names a node inside the file — the anatomy graph's nine
body-system files hold thousands of named structures each — only that node is
instanced, keeping its own place inside the asset. If the graph's node name and
the file's differ, the match degrades gracefully to the structure's constituent
parts rather than failing.

### CAD & BIM (IFC)

A `.rete` graph that describes a building works two ways, and the add-on handles
both.

**As geometry in the graph.** IFC-derived graphs (the FZK-Haus example) carry
each element's `geo3:asWKT3D` point and `geo3:box` bounding box in metres, its
`cad:ifcClass` (`IfcWallStandardCase`, `IfcWindow`, `IfcSpace`, …), and the full
BOT topology. Query the elements and you get a massing model sized by bounding
box, coloured by IFC class, with:

- **BOT topology → structure** — `bot:containsElement`, `cad:inStorey`,
  `bot:hasStorey`/`hasSpace` become parenting or per-storey collections;
- **`cad:adjacentSpace` / `bot:adjacentElement` → physics** — the building's
  own adjacency graph becomes rigid-body constraints, so the spaces of a house
  are literally wired to their neighbours;
- **`cad:elevation`, `cad:netArea`, `cad:grossVolume`** inherited as drivable
  numbers.

**As an IFC file URL.** A column pointing at a raw `.ifc` (via `cad:ifcModel`,
`cad:ifcFile`, or any `.ifc` URL) is imported element by element, at true world
coordinates, each mesh carrying its `ifcGuid` and `ifcClass`. This needs
**ifcopenshell** in Blender's Python — it is *not* bundled (too large):

```sh
<blender-python> -m pip install ifcopenshell
```

or install the **Bonsai** (BlenderBIM) add-on, which the importer will use if
present. Without either, IFC rows degrade with a clear message and everything
else still builds — and most CAD graphs also carry a `cad:glbModel` column,
which imports with no extra install. `.dxf` uses the add-on Blender already
ships; `.step`/`.stp` has no core importer and reports so.

### Inherited properties

This is the point of the whole thing. With **Inherit all properties** on, every
statement about every imported entity is fetched (batched, so a thousand
entities cost a handful of queries) and written onto the object as custom
properties:

- they appear in **Object Properties ▸ Custom Properties**, with the full
  predicate IRI as the tooltip;
- numeric ones are **drivable** — right-click ▸ *Copy as New Driver*, and a
  structure's mass, vertex count or citation count can drive a scale, a shader
  input, a modifier, anything Blender can drive;
- Geometry Nodes can read them;
- and the local-name → predicate-IRI mapping travels with the object, so the
  export round-trips losslessly.

Repeated predicates collect into a JSON array; nothing is dropped.

### Geometry

WKT literals become real geometry, not just positions. A `POINT Z` places an
object, a `LINESTRING` becomes an edge mesh, a `POLYGON` becomes a filled face —
and **Extrude** gives flat footprints height, turning administrative boundaries
into massing models. `BOX3D` bounding boxes size the markers.

Coordinates are handled honestly: millimetre, centimetre, metre and kilometre
presets, a *Fit to size* mode that scales the whole result into a box you
choose, Y-up→Z-up conversion, an X flip, and recentring. Geographic coordinates
are projected to metres — but **only on evidence** that they are degrees, so a
football pitch's local metres are not mistaken for longitude and flung across a
continent.

Datasets that publish positions as separate `x`/`y`/`z` (or `lon`/`lat`)
columns rather than WKT work the same way.

### Time

A date, a timestamp, a year, a duration, or bare decimal seconds becomes a
position on the scene's frame range. Partial ISO dates, BCE years and
`xsd:duration` all parse.

- **Appear** — objects exist only between their start and end, keyed with
  constant interpolation so what you scrub is what you render.
- **Grow in** — objects scale up as their moment arrives.
- **Motion path** — rows sharing an entity become one keyframed trajectory.
  The moving object is identified automatically: a tracking dataset gives every
  sample its own IRI and names the moving thing in another column, and the
  add-on groups by the latter.
- **Retime assets** — assets that carry their own animation are placed at their
  own moment on the timeline.

### Materials

Colour comes from whatever the data offers, best first: an explicit colour
literal, an image URL (downloaded and wired into a textured material — IIIF
endpoints are asked for a bounded size rather than the full scan), a numeric
column mapped through a perceptually uniform ramp, or the entity's class. Any
column can drive it: numeric columns become a ramp, everything else becomes one
stable colour per distinct value. Materials are assigned per object, so linked
copies sharing a mesh still colour independently.

### Relations, and relations as physics

Pick a predicate — `partOf`, a building's topology, a citation, anatomical
adjacency — and read it three ways:

- **Parent objects** — the relation becomes Blender's hierarchy, so the Outliner
  mirrors the partonomy and moving a parent moves its parts. Cycles are refused,
  not resolved.
- **Collections** — the same relation as grouping.
- **Edge geometry** — one line per statement, in a single mesh. Add a Skin or
  Wireframe modifier and the graph becomes solid tubes.

Then the interesting one. Under **Physics**, a predicate can become a network of
**rigid-body constraints**: every statement is a physical link between the two
objects. Mass is read from a numeric property, normalised into a usable band
(raw citation counts and vertex counts span ranges that just explode). Fixed,
point, hinge, slider or spring. The graph stops being a diagram and becomes a
structure that holds itself together — then you can pull it apart and watch what
the topology actually does.

### Very large results

**As point cloud** writes the whole result into one attributed mesh — one vertex
per row, every numeric and colour column as a named attribute, categorical
columns as an integer index plus a lookup table — with a Geometry Nodes
instancer attached. That is how to handle results far past what one-object-per-row
can carry.

### Live values in drivers

`rete()` and `rete_count()` are registered in Blender's driver namespace:

```python
# in any driver expression
rete_count("?s a <https://w3id.org/rete/anatomy#Muscle>")
rete("SELECT (AVG(?m) AS ?avg) WHERE { ?s <https://x.org/mass> ?m }", variable="avg")
```

Results are memoised — a driver is evaluated on every redraw, and without that
one expression would issue thousands of range requests while you scrub.

### Exploring from the viewport

Select an object and use **Expand neighbours** to pull its graph neighbours into
the scene, optionally following one predicate, with the connecting edges drawn.
**Select by query** runs SPARQL and selects the objects it returns. The
**Selected entity** panel shows the active object's IRI, its classes and its
inherited properties.

### Export

Any scene becomes a queryable `.rete` file: objects, transforms, hierarchy,
collections, materials, mesh statistics, animation ranges, and every custom
property — with original predicates restored for anything that came from a graph
and imported entities keeping their IRIs. The vocabulary is
`https://w3id.org/rete/scene#`, described inside the exported file, which also
ships runnable example queries. So Blender is an authoring tool for 3D knowledge
graphs, not only a viewer.

## Tests

Everything runs headless, in the container:

```sh
docker build -t rete-blender clients/blender

# unit and end-to-end, offline: builds its fixture graph in memory and exports
# its own .glb, so the asset path is covered without the network
docker run --rm -v "$PWD":/work -w /work rete-blender \
    blender -b --factory-startup -noaudio --python clients/blender/tests/run_tests.py

# against the published graphs — needs network
docker run --rm -v "$PWD":/work -w /work rete-blender \
    blender -b --factory-startup -noaudio --python clients/blender/tests/test_remote.py

# the packaged extension, in a Blender with no engine installed
docker build --build-arg WITH_ENGINE=0 -t rete-blender-clean clients/blender
docker run --rm -v "$PWD":/work -w /work rete-blender-clean sh -c '
  blender -b -noaudio --command extension install-file -r user_default --enable \
      clients/blender/dist/rete-*.zip &&
  blender -b -noaudio --python clients/blender/tests/test_extension.py'
```

## Graphs to try

| Graph | What is in it |
| --- | --- |
| `z-anatomy` | 4,884 human structures: mesh geometry, 3D adjacency, per-system `.glb` |
| `smithsonian3d` | 2,199 CC0 models — crania, fossils, the Apollo command module |
| `dance` | Salsa duets as animated skeletons: 3D plus time |
| `bioexplora` | Natural-history specimen scans from Barcelona |
| `scrolls` | Herculaneum scroll segment meshes and CT volumes |
| `tracking` | Football player positions over time |
| `geoadmin` | Administrative boundaries — extrude them into terrain |

All at `https://data.graphplaza.com/<key>/<key>.rete`. The full catalogue is in
the [playground](https://caviri.github.io/rete/playground.html).

## Notes and limits

- Queries run on the main thread, so a slow remote query briefly blocks the UI.
- Interior rings (holes) in polygons are dropped.
- Imported assets keep their own scale; the placement transform applies to
  geometry literals. Use either assets or geometry to position a given import,
  not both.
- CAD/BIM assets (IFC, DXF) keep their real-world coordinates and are not
  rescaled — a building imports at true metres regardless of the scale mode.
- IFC import needs ifcopenshell or Bonsai; it is not bundled. STEP has no core
  Blender importer.
- The asset limit (default 400 rows) guards against a query that would import
  thousands of files; rows past it become markers.

Apache-2.0, like the rest of the repository.
