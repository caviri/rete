# GeoSPARQL (geometry + time)

rete stores geometry as standard [GeoSPARQL](https://www.ogc.org/standard/geosparql/)
`geo:wktLiteral` values and answers a focused set of **GeoSPARQL filter
functions** — so you can ask spatial questions (*point-in-polygon*,
*intersection*, *distance*) and combine them with ordinary SPARQL, including
temporal filters. The whole thing runs in the **browser/WASM** engine too: the
geometry code is pure Rust (`core` + `f64`, no GIS C libraries), so a `.rete`
file of polygons-with-time is queryable in place, offline.

Try it in the **[playground](playground.html)** — pick the *history.rete*
dataset and the **Geo** examples.

## Data model

Geometry follows the GeoSPARQL convention — a feature links to a geometry node
that carries a WKT literal:

```turtle
<http://ex/terr/France_1914>
  rdfs:label "France" ;
  ex:year 1914 ;                                   # plain xsd:integer — filter on it
  geo:hasGeometry [ geo:asWKT
    "POLYGON((-4.79 48.32, 2.55 51.09, 7.59 47.59, ...))"^^geo:wktLiteral ] .
```

- `geo:` = `http://www.opengis.net/ont/geosparql#`
- The functions operate on a `geo:wktLiteral` term, normally reached with the
  property path `?f geo:hasGeometry/geo:asWKT ?wkt`. A plain or `xsd:string`
  literal whose value happens to be WKT is also accepted.
- **Axis order is CRS84 lon/lat, always.** A CRS-URI prefix on a literal
  (`<http://…/CRS84> POINT(…)`) is accepted but ignored — rete never swaps
  axes, so do **not** feed it lat/lon EPSG:4326 data.
- Time is just an ordinary literal (`ex:year 1914`, or `ex:startYear`/
  `ex:endYear`), so a temporal slice is a plain `FILTER(?year = 1914)`.

## Functions

| Function | Returns | Notes |
|---|---|---|
| `geof:sfContains(g1, g2)` | `xsd:boolean` | `g2` inside/on `g1` (POLYGON ⊇ POINT is the common case) |
| `geof:sfWithin(g1, g2)` | `xsd:boolean` | `sfContains(g2, g1)` |
| `geof:sfIntersects(g1, g2)` | `xsd:boolean` | share any point (boundary contact counts) |
| `geof:sfDisjoint(g1, g2)` | `xsd:boolean` | `!sfIntersects` |
| `geof:sfEquals(g1, g2)` | `xsd:boolean` | structural equality within ε |
| `geof:distance(g1, g2, unit)` | `xsd:double` | min distance; `unit` = `uom:metre` (haversine) or `uom:degree` (planar) |
| `geof:envelope(g)` | `geo:wktLiteral` | axis-aligned bounding box as a `POLYGON` |

`geof:` = `http://www.opengis.net/def/function/geosparql/`,
`uom:` = `http://www.opengis.net/def/uom/OGC/1.0/`.

WKT parsing covers `POINT`, `MULTIPOINT`, `LINESTRING`, `POLYGON` (with holes),
`MULTIPOLYGON`, `GEOMETRYCOLLECTION`, the `EMPTY` keyword, and `Z`/`M`
ordinates (dropped). A malformed or non-geometry argument is a type error: the
row drops out of a `FILTER` and the variable is left unbound in a `BIND` — never
an engine error.

## Example — "which territory contained this point in year Y?"

The headline query combines a temporal filter and a spatial predicate in one
`FILTER`:

```sparql
PREFIX geo:  <http://www.opengis.net/ont/geosparql#>
PREFIX geof: <http://www.opengis.net/def/function/geosparql/>
PREFIX ex:   <http://ex/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

SELECT ?territory WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ;
     geo:hasGeometry/geo:asWKT ?w .
  FILTER(geof:sfContains(?w, "POINT(2.35 48.85)"^^geo:wktLiteral))
}
# → France   (the same point resolves to "Kingdom of France" in the year-1000 snapshot,
#    and "Liao" / "Qing" / … for Beijing across the era snapshots)
```

Distance ranking (metres → km), bounding boxes, and bbox intersection work the
same way:

```sparql
# nearest territories to a point over London, 1914
SELECT ?territory ?km WHERE {
  ?t ex:year 1914 ; rdfs:label ?territory ; geo:hasGeometry/geo:asWKT ?w .
  BIND(geof:distance(?w, "POINT(0 51)"^^geo:wktLiteral,
       <http://www.opengis.net/def/uom/OGC/1.0/metre>) / 1000 AS ?km)
} ORDER BY ?km LIMIT 8
```

## The example dataset

`history.rete` (embedded in the playground) is built from
[**aourednik/historical-basemaps**](https://github.com/aourednik/historical-basemaps)
(GPL-3.0) — world territorial borders at snapshots from 323 BCE to 1994 CE. Each
border polygon becomes a `geo:wktLiteral`, the snapshot year an `xsd:integer`,
geometry simplified to ~1 km for an in-browser-sized file. Build it with:

```sh
python3 scripts/geo_to_rete.py basemaps \
  --years bc323,1000,1492,1815,1914,1945,1994 --prec 2 --min-bbox 0.3 \
  --max-per-year 90 -o history.nt
rete build history.nt -o web/history.rete
```

The same script also converts an
[**OpenHistoricalMap**](https://www.openhistoricalmap.org/) extract — real dated
administrative boundaries pulled from its public
[QLever endpoint](https://qlever.dev/api/ohm-planet) (`geo:hasGeometry/geo:asWKT`
+ `start_date`/`end_date`) — via the `ohm` subcommand.

## Scope & limitations

v1 is deliberately tight and exact for the common cases rather than broad:

- **Planar** Cartesian computation on lon/lat (haversine only for the metre
  distance). No CRS reprojection — `geof:transform` and non-CRS84 axis handling
  are out of scope.
- The five topological relations are computed directly (point-in-polygon +
  segment crossing), not via a full DE-9IM matrix. Exact for the dominant
  polygon ⊇ point / nested-territory cases; `sfTouches`/`sfCrosses`/`sfOverlaps`,
  `geof:relate`, constructive geometry (`buffer`/`union`/`boundary`/…),
  `geof:area`/`length`, and `geof:getSRID` are **not** implemented — an
  unsupported `geof:` IRI is rejected at parse time.
- No spatial index: filters scan the candidate rows (fine for the curated demo
  data and for queries already narrowed by a non-spatial pattern).

Performance of the non-spatial engine is covered in [Benchmarks](BENCHMARK.html);
overall SPARQL coverage in the [conformance scorecard](conformance.html).
