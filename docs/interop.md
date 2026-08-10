# Triple-store interop

A `.rete` file is not a silo. **N-Quads is the interchange**: `rete export`
streams a lossless dump that every triple store bulk-loads, and `rete build`
ingests every store's native export. This page gives the verified recipes in
both directions for [Oxigraph](https://github.com/oxigraph/oxigraph),
[GraphDB](https://graphdb.ontotext.com/), and Jena/Fuseki — plus the option
that skips migration entirely.

Fidelity is not taken on faith: the project's regression suite runs
**differentially against Oxigraph** — the same data loaded into both
engines must answer every query identically, on every CI run.

## rete → any triple store

`rete export` streams the dataset with constant memory — it never loads the
graph. The default format is **N-Quads, lossless**: default graph + named
graphs, RDF-star quoted triples included.

```sh
rete export data.rete | gzip > dump.nq.gz
```

The other formats serialize the default graph only (Turtle and JSON-LD have
no named-graph story) — for migration, always N-Quads:

```sh
rete export data.rete --format ttl     > default-graph.ttl
rete export data.rete --format jsonld  > default-graph.jsonld
```

`export` reads a local file. For a remote `.rete`, download it first (it is
one GET) — harvesting through paginated `CONSTRUCT` works but is far
slower than a dump.

### Load into Oxigraph

Oxigraph's bulk loader is parallel and takes gzip directly:

```sh
# CLI (or the same subcommands via the docker image oxigraph/oxigraph):
oxigraph load --location ./store --file dump.nq.gz

# then serve it:
oxigraph serve --location ./store --bind 0.0.0.0:7878
```

Round-trip check (run verbatim in Docker for this page) — count in rete,
count in Oxigraph, same answer, named graph intact:

```sh
$ rete sparql people.rete "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"
?n="6"^^<http://www.w3.org/2001/XMLSchema#integer>

$ curl -s "http://localhost:7878/query" \
    --data-urlencode "query=SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }" \
    -H "Accept: application/sparql-results+json"
{"head":{"vars":["n"]},"results":{"bindings":[{"n":{"type":"literal","value":"6","datatype":"http://www.w3.org/2001/XMLSchema#integer"}}]}}
```

### Load into GraphDB

For big files use the server-side bulk importer (`importrdf`), pointing at
a repository config:

```sh
importrdf preload --force -c repo-config.ttl dump.nq.gz
```

For a running instance, the REST route (also how the workbench imports):

```sh
# create a repository, then:
curl -X POST "http://localhost:7200/repositories/myrepo/statements" \
  -H "Content-Type: application/n-quads" \
  --data-binary @dump.nq
```

### Load into Jena / Fuseki

```sh
tdb2.tdbloader --loc ./tdb dump.nq.gz        # bulk load
fuseki-server --loc ./tdb /ds                # serve
```

## Any triple store → rete

`rete build` ingests N-Triples, N-Quads, Turtle, and RDF/XML — so the
reverse direction is each store's native dump piped into a build:

### From Oxigraph

```sh
oxigraph dump --location ./store --file dump.nq --format nq
rete build dump.nq -o out.rete
```

Verified in Docker for this page: dump the Oxigraph store, rebuild the
`.rete`, and the row-level answers match the original query for query —
including the named graph, which survives the full
rete → Oxigraph → rete cycle.

### From GraphDB

```sh
# The statements endpoint IS the dump (workbench "Export" does the same):
curl -H "Accept: application/n-quads" \
  "http://localhost:7200/repositories/myrepo/statements?infer=false" > dump.nq
rete build dump.nq -o out.rete
```

`infer=false` exports only asserted triples. If you want GraphDB's
materialized inferences frozen into the file, drop it — but consider
shipping the ontology instead and letting rete's
[OWL 2 QL reasoning](reasoning.md) answer entailments at query time.

### From Jena

```sh
tdb2.tdbdump --loc ./tdb > dump.nq
rete build dump.nq -o out.rete
```

### Scale notes

- Hundreds of millions of triples are routine builds (data.bnf.fr: 716 M;
  one 726 M-triple file). If RAM is the constraint, `rete build
  --memory-budget-mb` runs the chunked external build — same byte-identical
  file, bounded memory. It takes the dump **in the form it ships**: N-Triples,
  N-Quads, Turtle or TriG, gzipped or not, decompressed while streaming. So a
  public `dump.ttl.gz` needs no conversion pass and no room for the expanded
  copy, which is usually the larger of the two costs — SemOpenAlex measures
  146.8 N-Quads bytes per triple, so its 8.5 GiB author dump would land as
  ~400 GB of `.nt` before a single triple were indexed.
- If the dump keeps its data in **named graphs** — TriG exports, Wikibase and
  GraphDB dumps — consider `--collapse-graphs`. In SPARQL the default graph is
  not the union of the named ones, so without it `?s ?p ?o` answers nothing and
  the pyramid comes out empty. It is a modelling choice, not a build constraint:
  `--memory-budget-mb` builds named graphs directly.
- Add `--text-index` at build time if you want [full-text search](cli.md)
  over the migrated data, and a [Dataset Card](dataset-cards.md) so the file
  explains itself.

## The no-migration option: federation

If the goal is only that another engine can *query* `.rete` data, skip the
dump entirely — any SPARQL 1.1 engine with `SERVICE` support can federate
against a rete endpoint, live and lazy:

```sparql
SELECT ?law ?title WHERE {
  SERVICE <https://katospiegel-rete.hf.space/sparql/boe> {
    ?law <http://data.europa.eu/eli/ontology#title> ?title .
  }
  # … joined with whatever lives in the local store …
}
LIMIT 10
```

The gateway turns **any** published `.rete` URL into a standard endpoint
(`/sparql/<full-url>` — see [Hosting](hosting.md)), so this works for files
nobody registered anywhere. `rete serve` does the same for a local file.

[Comunica](https://comunica.dev) needs no adapter at all (verified):

```sh
$ npx -y -p @comunica/query-sparql comunica-sparql \
    "sparql@https://katospiegel-rete.hf.space/sparql/boe" \
    "SELECT ?title WHERE { <https://www.boe.es/eli/es/c/1978/12/27/(1)> <http://data.europa.eu/eli/ontology#title> ?title }"
[{"title":"\"Constitución Española.\""}]
```

For native (non-endpoint) integration, the npm client ships an RDF/JS
`ReteSource` — see [the JavaScript client](javascript.md).
Migrate when you need writes, store-specific features (GraphDB's Lucene
connectors, say), or co-location with data already living there;
federate when you just need the answers.
