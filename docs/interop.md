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
rete export data.rete --compress zstd > dump.nq.zst
```

`--compress` compresses the dump as it is written, streaming, so the
memory bound is unaffected. zstd is the default recommendation over gzip on
measured grounds: about 21% smaller output than gzip at the same nominal level,
compressed 2.4x faster and decompressed 1.5x faster (numbers and method in the
[CLI reference](cli.md)). `--compress gzip` is there for consumers that only
take gzip; piping through an external `gzip` still works too.

**TriG is the compact lossless alternative.** It is Turtle syntax wrapped in
`GRAPH <g> { … }` blocks, so it keeps every named graph exactly as N-Quads does,
while writing each subject and each namespace once instead of once per
statement:

```sh
rete export data.rete --format trig > dump.trig
```

Both `nq` and `trig` stream, so peak memory follows `--memory-budget-mb` rather
than the size of the graph, and both are byte-identical at any budget. TriG is
the smaller of the two — substantially so on data with many statements per
subject — and every store on this page loads it. Use N-Quads when the consumer
is a line-oriented pipeline (`split`, `grep`, a Spark reader); use TriG when the
consumer is an RDF parser and the file has to travel.

### The single-graph formats

Turtle and JSON-LD carry no graph term, so they serialize **one** graph. Which
one follows a fixed ladder, and the choice is always reported on stderr:

| situation | what is exported |
| --- | --- |
| `--graph <iri>` | that graph; an error naming the real graphs if it does not exist |
| `--graph ''` | the default graph, explicitly |
| no `--graph`, default graph has content | the default graph, with a note that named graphs were left out |
| no `--graph`, default graph empty, one named graph | that graph |
| no `--graph`, default graph empty, several named graphs | an error listing them |

Graphs are never silently merged and a non-empty selection is never silently
dropped — a quads file that cannot be written as Turtle says so instead of
producing a plausible-looking partial dump.

```sh
rete export data.rete --format ttl                        > default-graph.ttl
rete export data.rete --format ttl --graph http://g/1     > one-graph.ttl
rete export data.rete --format jsonld                     > default-graph.jsonld
```

`ttl` streams like `nq` and `trig`. `jsonld` does not — expanded JSON-LD is a
single JSON array, so it is built in memory and is not the format for a file
that does not fit in RAM.

### Prefix compression

`ttl` and `trig` abbreviate IRIs to QNames, which is where most of their size
advantage over N-Quads comes from: the namespace is the repeated part of an IRI,
and writing it once in an `@prefix` line removes it from every term that uses it.

The bindings come from two places. Well-known vocabularies (`rdf`, `rdfs`, `owl`,
`xsd`, `dct`, `skos`, `foaf`, `prov`, `schema`, `sh`, `void`, the SPAR family,
Wikibase, and the common scholarly identifier namespaces) keep their conventional
names. On top of that, the exporter reads a bounded sample of the data — a
hundred thousand statements, whatever the file's size — and gives a name to the
namespaces that are actually frequent in *this* file, derived from their own last
path segment. That second group is usually where the bytes are: a dataset's own
entity namespace typically outnumbers every standard vocabulary in it by an order
of magnitude.

An IRI is only abbreviated when the part after the namespace is a legal Turtle
`PN_LOCAL` without backslash escaping; anything else is written in full. The
result is a file that is smaller but never a file that will not parse.

`--no-prefixes` turns all of this off and writes every IRI in full — for a
consumer that cannot resolve QNames, or to diff two dumps term by term.

### Compression and the two wins together

Prefix compression and a general-purpose codec attack different redundancy, and
they compose — but not additively, which is worth knowing before choosing.
Turtle/TriG remove *structural* repetition (a subject and its namespaces written
once); zstd removes *byte* repetition. On a 4M-quad dump:

| | raw | zstd -3 |
| --- | --- | --- |
| nq | 626,336,881 | 32,787,635 |
| trig | 259,715,103 | 23,308,451 |

TriG is 41% of N-Quads raw, but 71% of it after compression — the codec had
already found most of what TriG removes structurally. TriG still wins on both,
and it wins *before* decompression too, which is the case that matters when
something has to parse the file rather than just store it.

(That table is at `zstd -3`, so the two formats are compared without codec time
dominating. `--compress zstd` defaults to level 6, which is smaller than both
columns shown.)

So: `--format trig --compress zstd` is the smallest lossless option, and
`--format nq` is the one every line-oriented tool can `split` and `grep`.

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

### When the dump does not load: invalid IRIs

rete's N-Triples reader is deliberately tolerant — it stores whatever sits
between `<` and the next `>`. Oxigraph's is not, and this is where the two
disagree. Measured against **Oxigraph 0.5.9 in Docker**, on a 9-quad graph of
which 5 statements carry an IRI outside the N-Triples `IRIREF` grammar:

```
$ oxigraph load --location ./store --file dump.nq
Some files like Wikidata dumps contain invalid IRIs or language tags.
If you want to load them anyway use the `--lenient` option.
Error while loading file dump.nq: Parser error at line 1 between columns 1 and 42:
  Invalid IRI percent encoding '%x-'

$ oxigraph dump --location ./store --format nq | wc -l
0
```

Two things to take from that.

**A load is atomic — one bad IRI costs the whole file.** Nine quads went in and
zero came out, including the four that were perfectly good. This is why the same
defect cost `openaire-2021-datasource` an entire ~102,000-line chunk to a single
IRI with no scheme: the loader rejects the *chunk*, not the line.

**`oxigraph load` exits 0 even when it rejects the file.** The message above goes
to stderr and the process returns success, so a bulk-load script that only checks
`$?` will report a clean import of an empty store. Check the quad count, not the
exit code. (`tests/interop/oxigraph.sh` does exactly that.)

`rete build` now **counts** such IRIs as it ingests them, so the problem is
visible where it enters rather than at the far end of someone else's loader:

```
warning: 5 statement(s) carry an invalid IRI (5 IRI occurrence(s)).
               2  '[' or ']' outside an IP-literal host
               1  more than one '#'
               1  '%' not followed by two hex digits
               1  a character the IRIREF grammar excludes …
```

For the dump itself, `--sanitize-iris` percent-encodes the repairable classes:

```sh
rete export data.rete --format nq --sanitize-iris > dump.nq   # now loads
```

The same 9-quad graph then loads and Oxigraph holds **9 quads** — the count the
dump carried. But this is not free, and it is opt-in for that reason:

- the escaped IRIs are **different IRIs**, so the dump no longer joins against
  the graph it came from, and rete → Oxigraph → rete is no longer the identity;
- an IRI with **no scheme cannot be repaired at all**. Escaping does nothing for
  it — resolving it needs a base IRI the `.rete` never recorded — so it is
  counted, named on stderr, and written verbatim. The dump is then *still*
  rejected (`No scheme found in an absolute IRI`, again with an empty store).
  Fix those at the source; nothing downstream can.

`rete build --strict` refuses such input outright, if you would rather find out
at build time. Full rules and the five classes: [CLI → Invalid
IRIs](cli.md#invalid-iris).

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

Verified in Docker for this page, and re-verified against Oxigraph 0.5.9 by
`tests/interop/oxigraph.sh`: dump the Oxigraph store, rebuild the `.rete`, and
the row-level answers match the original query for query — including the named
graph, which survives the full rete → Oxigraph → rete cycle. On a 6-quad
dataset (3 default + 3 in one named graph) the cycle came back **quad for quad
identical**.

That is the result **on clean data**, which is what this page was originally
written from. It is not the result on data with an invalid IRI — see below.

One spelling does change even on clean data: an N-Triples `UCHAR` escape.
`<http://example.org/uchar/caf\u00E9>` goes in and `<http://example.org/uchar/café>`
comes back, because Oxigraph resolves the escape while rete stores the token
verbatim. Same IRI, different dictionary key — worth knowing if you compare
dumps byte for byte, or if a graph contains both spellings (they are one term
after the round-trip and two before it).

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
