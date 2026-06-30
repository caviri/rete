# Source-type playbook: getting RDF out of any graph source

Each entry: when it applies, the recipe, and a **real script in this repo** to copy.
All converters emit **streaming N-Triples** (one `<s> <p> <o> .` per line). Object
values are canonical N-Triples tokens: `<iri>`, `"literal"`, `"literal"@en`,
`"5"^^<http://www.w3.org/2001/XMLSchema#integer>`.

A minimal emitter (reuse in every converter):

```python
import sys
def iri(s):  return "<" + s + ">"
def esc(s):  return str(s).replace("\\","\\\\").replace('"','\\"').replace("\n"," ").replace("\r"," ").replace("\t"," ")
def lit(s):  return '"' + esc(s) + '"'
def t(s,p,o): sys.stdout.write(iri(s)+" "+iri(p)+" "+o+" .\n")   # o is already a token
def tl(s,p,o): t(s,p,lit(o))                                      # literal object
```

---

## 1. RDF dump — `.nt` / `.nq` / `.ttl` / `.rdf` / `.owl` / `.rdfxml`

**Build it directly. No conversion.** `rete build` parses all of these (RDF/XML via
oxrdfxml — most OWL ontologies ship as RDF/XML and "just build"). Multiple inputs
merge into one file.

```bash
rete build ontology.owl extra.ttl -o out.rete --pyramid-algo types --card
```

If a dump is large or messy, `nt_clean.py` first (see below), then build.

## 2. OWL/XML (the OTHER OWL syntax)

Some ontologies ship as **OWL/XML** (not RDF/XML) — `rete build` (and rdflib/rapper)
can't parse it. Convert with **owlready2**:

```bash
pip install --break-system-packages owlready2
python skills/rete-from-graph/scripts/owl_to_nt.py input.owl > data/foo/foo.nt
```

Model script: `scripts/causalgraph_to_nt.py` notes (causalgraph dataset), the
`rdfxml-owl-ingest` workflow.

## 3. SPARQL endpoint

CONSTRUCT (or paginated SELECT `?s ?p ?o`) the slice you want, serialize to NT.
Use the generic harvester:

```bash
# CONSTRUCT, asking the endpoint for N-Triples directly:
python skills/rete-from-graph/scripts/sparql_to_nt.py \
  --endpoint https://vocab.getty.edu/sparql \
  --construct "CONSTRUCT { ?s ?p ?o } WHERE { ?s a skos:Concept ; ?p ?o }" \
  --page 50000 > data/getty/getty.nt

# or paginated SELECT ?s ?p ?o (when CONSTRUCT isn't allowed):
python skills/rete-from-graph/scripts/sparql_to_nt.py \
  --endpoint https://query.wikidata.org/sparql \
  --select "SELECT ?s ?p ?o WHERE { ?s wdt:P31 wd:Q5 ; ?p ?o }" --page 10000 > out.nt
```

Model scripts: `scripts/fetch_playground_kgs.sh` (getty-ulan CONSTRUCT pattern).
Respect endpoint rate limits; page in chunks; resume on failure.

## 4. Wikibase / Wikidata

Either the SPARQL endpoint (§3) or the **truthy dump → Parquet → NT** path
(`scripts/wikidata_parquet_to_nt.py` recovers datatypes from the columnar dump for
a sliced graph). **Crucial build flag:** Wikidata types via `wdt:P31`, not
`rdf:type` → `--type-predicate http://www.wikidata.org/prop/direct/P31`.
A custom Wikibase harvest goes per-entity when there's no dump (model:
`scripts/biblissima_*`, sharded because monolithic OOMs).

## 5. GeoJSON / shapefiles → GeoSPARQL

Emit each feature's geometry as a `geo:asWKT` `wktLiteral` (POINT/POLYGON/
MULTIPOLYGON), plus typed entities + names + codes. **Simplify detailed polygons**
(Douglas–Peucker, ~1 km) or the browser hangs parsing megabyte WKT. For zoom/LOD,
add a finer `g:geomFine` and/or a paired PMTiles basemap (see the geoadmin work).

Model script: **`scripts/geoboundaries_to_nt.py`** (GeoJSON→WKT by hand, no GDAL;
multi-LOD; ISO alpha-3→alpha-2 mapping for federation joins).

## 6. Tabular / CSV / Parquet

One subject per row (a stable IRI from a key column), one triple per non-empty
cell, with a small per-column ontology (datatype properties for scalars, object
properties for foreign keys → IRIs). Add `rdf:type`. Model: `scripts/jonas_to_nt.py`
(built from a DuckDB export, not a scrape), `scripts/memoria_*` (regional open data).

## 7. GBIF Darwin-Core archive (DwC-A)

Unzip the DwC-A; stream the `occurrence.txt` (tab-separated); map Darwin-Core terms
to a small ontology (Specimen/Taxon/Agent/Place classes, `dwc:` properties), link
the graph (parentTaxon / collectedBy / foundIn), attach media (IIIF, audio, 3D).
Model script: **`scripts/bioexplora_to_nt.py`** (207k specimens, connected graph
layer, media columns) — harvested KEYLESS from the GBIF DwC-A.

## 8. TEI / TEITOK corpus

Parse the TEI-P5 XML; build a correspondence/inscription network (people, places,
documents) + the text. Model: `scripts/postscriptum_to_nt.py` (TEITOK letters),
`scripts/lineara_to_nt.js` (inscription↔sign/word network). Check the corpus
license case-by-case.

## 9. JSON API

Page the API, map each entity to an IRI + typed triples. Prefer an official bulk
export over hammering an API; cache pages so a re-run resumes. Model: the keyless
harvests (`scripts/smithsonian3d_to_nt.py` from the public S3 bucket, `scripts/
albala` from an anonymous Solr route).

---

## Modeling tips that pay off downstream

- **Type your subjects** (`rdf:type` to real classes) — the schema pyramid, the
  Explore tab, and `--pyramid-algo types` all key off it.
- **Add `rdfs:label`** — the playground decodes IRIs to labels everywhere.
- **Make a CONNECTED graph**, not just flat rows — emit the object-property edges
  (taxon trees, collectedBy, partOf) so path/reachability queries and the graph
  view have something to traverse.
- **Media columns** render inline in the playground: image/IIIF URLs, `geo:asWKT`,
  `.glb` meshes, audio/video, turntable spins. Emit the URLs as object literals/IRIs.
- **Federation**: expose a join key in a standard vocab (e.g. `dwc:countryCode`,
  `owl:sameAs`, a VIAF/Wikidata id) so the dataset can be joined to others.
