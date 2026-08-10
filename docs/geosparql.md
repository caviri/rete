# GeoSPARQL (Geometry & Time)

Rete has built-in support for spatial geometry and time. It stores geometries as standard [GeoSPARQL](https://www.ogc.org/standard/geosparql/) `geo:wktLiteral` values. 

This means you can write SPARQL queries to ask spatial questions like:
- *"Is this point inside this polygon?"*
- *"Do these two regions intersect?"*
- *"How far is city A from territory B?"*

Crucially, **this all runs completely in the browser** via WASM. The geometry engine is written in pure Rust with no external GIS C libraries, meaning you can query massive geospatial datasets directly from a `.rete` file offline.

> [!TIP]
> Try it out in the **[Playground](playground.html)**! Load the **history.rete** dataset and try the **Geo** examples.

## How the Data is Modeled

Rete follows standard GeoSPARQL conventions. A feature (like a country) is linked to a geometry node containing a Well-Known Text (WKT) literal. Time can be easily modeled as a standard integer or string.

```turtle
<http://ex/terr/France_1914>
  rdfs:label "France" ;
  ex:year 1914 ;                           # A simple integer for temporal filtering
  geo:hasGeometry [ 
    geo:asWKT "POLYGON((-4.79 48.32, 2.55 51.09, 7.59 47.59, ...))"^^geo:wktLiteral 
  ] .
```

### Important Data Rules:
- **Namespaces:** Use `geo:` for `http://www.opengis.net/ont/geosparql#`.
- **Finding Geometries:** WKT values are typically reached using the path `?f geo:hasGeometry/geo:asWKT ?wkt`.
- **Axis Order:** **Always use CRS84 (Longitude/Latitude).** Do NOT provide Lat/Lon (EPSG:4326) data, as Rete will not swap the axes for you.
- **Time:** Time is handled using standard literals (e.g., `ex:year`, `ex:startDate`), meaning you can slice time using a simple `FILTER(?year = 1914)`.

## Supported Spatial Functions

You can use the following `geof:` functions (Prefix: `http://www.opengis.net/def/function/geosparql/`) to filter and compute spatial data:

| Function | Returns | What it does |
|---|---|---|
| `geof:sfContains(g1, g2)` | Boolean | True if `g2` is completely inside or on the border of `g1`. (Most common use: Polygon contains Point). |
| `geof:sfWithin(g1, g2)` | Boolean | The reverse of `sfContains`. True if `g1` is within `g2`. |
| `geof:sfIntersects(g1, g2)` | Boolean | True if `g1` and `g2` share *any* point (even just a border). |
| `geof:sfDisjoint(g1, g2)` | Boolean | True if they do not intersect at all. |
| `geof:sfEquals(g1, g2)` | Boolean | True if they are structurally identical. |
| `geof:distance(g1, g2, unit)`| Double | Calculates minimum distance. Units: `uom:metre` (curved earth/haversine) or `uom:degree` (flat map). |
| `geof:envelope(g)` | WKT | Returns the bounding box (envelope) of the geometry as a `POLYGON`. |

*(Note: `uom:` maps to `http://www.opengis.net/def/uom/OGC/1.0/`)*

Rete can parse `POINT`, `MULTIPOINT`, `LINESTRING`, `POLYGON` (including holes), `MULTIPOLYGON`, `GEOMETRYCOLLECTION`, and `EMPTY`. If your data contains `Z` or `M` dimensions, they will be safely ignored.

## Examples

### 1. Point in Time and Space
*"Which historical territory contained this exact GPS coordinate in the year 1914?"*

```sparql
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?territory WHERE {
  # 1. Filter by year
  ?t ex:year 1914 ; 
     rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
     
  # 2. Filter by spatial containment (Longitude 2.35, Latitude 48.85 is Paris)
  FILTER(geof:sfContains(?w, "POINT(2.35 48.85)"^^geo:wktLiteral))
}
# Result -> France 
```

### 2. Distance Ranking
*"What were the nearest territories to a point over London in 1914, sorted by distance in kilometers?"*

```sparql
SELECT ?territory ?km WHERE {
  ?t ex:year 1914 ; 
     rdfs:label ?territory ; 
     geo:hasGeometry/geo:asWKT ?w .
     
  # Calculate distance in metres, convert to km, and bind it
  BIND(geof:distance(?w, "POINT(0 51)"^^geo:wktLiteral,
       <http://www.opengis.net/def/uom/OGC/1.0/metre>) / 1000 AS ?km)
} ORDER BY ?km LIMIT 8
```

## The Example Dataset (`history.rete`)

To see this in action, we embedded the `history.rete` dataset in the playground. It is derived from [aourednik/historical-basemaps](https://github.com/aourednik/historical-basemaps), providing world territorial borders from 323 BCE to 1994 CE.

If you want to build this yourself from scratch, run:
```sh
# Fetch and convert the data to N-Triples
python3 scripts/geo_to_rete.py basemaps \
  --years bc323,1000,1492,1815,1914,1945,1994 --prec 2 --min-bbox 0.3 \
  --max-per-year 90 -o history.nt

# Build the .rete file
rete build history.nt -o web/history.rete
```

## Limitations to Keep in Mind

To keep the engine blazing fast and browser-compatible, Rete focuses heavily on the most common spatial operations:

- **No Reprojection:** Computations assume a planar Cartesian coordinate system on Lon/Lat. We do not reproject CRS on the fly.
- **Specific Topological Coverage:** Point-in-polygon and intersections are exact and fully supported. Advanced constructs like `sfTouches`, `buffer`, `union`, or `geof:area` are **not** implemented.
- **No Spatial Index:** There is no dedicated spatial index (like an R-Tree). Spatial filters scan candidate rows directly. For best performance, use standard SPARQL constraints (like the `ex:year 1914` example) to narrow down the rows *before* applying the spatial filter.
