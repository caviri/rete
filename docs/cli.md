# CLI reference

The `rete` binary (crate `rete-cli`). Run `rete <command> --help` for the
authoritative flags. Terms are written as canonical N-Triples tokens —
`<http://ex/Alice>` for an IRI, `"30"` or `"30"^^<…#integer>` for a literal.

## Building

### `rete build <inputs…> -o <out.rete> [--format nt|nq|ttl]`
Build a file from one or more RDF inputs, merged under one shared dictionary.
Format is detected by extension (`.nt` / `.nq` / `.ttl`); `-` reads stdin and
defaults to N-Triples; `--format` forces a format for all inputs. N-Quads inputs
produce a dataset with named graphs.

```sh
rete build a.nt b.nt -o merged.rete
curl -s https://host/data.nt | rete build - -o data.rete
```

## Validating

### `rete validate <inputs…> [--format nt|nq|ttl]`
Parse RDF input(s) without building, to check they are well-formed
N-Triples/N-Quads/Turtle. Reports statement and named-graph counts, or exits
non-zero with a precise parse error (file, line, column).

```sh
rete validate data.ttl
curl -s https://host/data.nt | rete validate - --format nt
```

## Inspecting

### `rete info <file>`
Print the decoded 128-byte header (offsets, codecs, counts, content hash).

### `rete stats <file>`
Human-friendly overview: file size, default-graph triple count, distinct terms,
named-graph count, pyramid levels, and top predicates.

### `rete verify <file>`
Recompute the blake3 content hash and compare to the header. Exits non-zero with
`FAILED — content hash mismatch` on corruption/truncation.

### `rete graphs <file>`
List the named-graph IRIs in a dataset (the default graph is unnamed).

### `rete export <file> [--format nq|ttl|jsonld]`
Serialize the dataset. `nq` (the default) dumps every triple/quad as N-Quads
(default graph + named graphs) — a lossless round-trip. `ttl` emits Turtle and
`jsonld` emits expanded JSON-LD; both serialize the **default graph only**
(Turtle/JSON-LD here carry no default-vs-named distinction, so named graphs are
skipped — use `nq` to export those).

```sh
rete export data.rete                 # N-Quads (default)
rete export data.rete --format ttl    # Turtle
rete export data.rete --format jsonld # expanded JSON-LD
```

## Querying

### `rete query <file> [--subject S] [--predicate P] [--object O]`
Match a single triple pattern; omitted positions are wildcards.

```sh
rete query data.rete --predicate '<http://ex/knows>'
```

### `rete bgp <file> "<pattern> . <pattern> …"`
Evaluate a Basic Graph Pattern. Patterns are separated by ` . `, terms by spaces;
`?name` is a variable (terms may not contain spaces).

```sh
rete bgp data.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"
```

### `rete sparql <file> "<query>" [--json]`
Run SPARQL: `SELECT` / `ASK` / `CONSTRUCT` / `DESCRIBE`. With `--json`, emit
standard SPARQL Results JSON (for `SELECT`/`ASK`). See [SPARQL support](sparql.md).

```sh
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p e:knows ?f } GROUP BY ?p"
```

### `rete cypher <file> "<query>" [--base <iri>] [--json]`
Run a read-only **Cypher subset** (a prototype). The query is translated to an
equivalent SPARQL `SELECT` and evaluated by the same engine — no second query
engine. Supports `MATCH … [WHERE …] RETURN … [LIMIT n]`: node patterns
(`(a)`, `(a:Label)`), forward/reverse relationships (`(a)-[:REL]->(b)`,
`(a)<-[:REL]-(b)`), variable-length relationships (`-[:REL*]->` → the SPARQL
property path `REL+`, one-or-more), simple `WHERE` comparisons on a property
(`a.age > 30`) or identity (`a = <iri>`) joined by `AND`/`OR`, and `RETURN` of
variables and/or properties. Writes, `OPTIONAL MATCH`, `WITH`, aggregations,
relationship variables/properties, and multiple labels are rejected with a clear
error. See [compatibility](compatibility.md) for the full subset and the
name→IRI convention. With `--json`, emit standard SPARQL Results JSON.

A bare label/relationship/property name `X` maps to `<BASE + X>`, where `BASE`
defaults to `http://ex/` and is overridable with `--base`.

```sh
rete cypher deps.rete "MATCH (a:Application) RETURN a"
rete cypher deps.rete "MATCH (a)-[:dependsOn*]->(b) WHERE b = <http://ex/log4x> RETURN a"
```

## Reasoning

### `rete reason <file> [--materialize] [--format nq|ttl]`
Run the prototype **OWL RL / RDFS reasoner**: materialize RDFS/OWL entailments to
a fixpoint and report any logical **inconsistencies** ("incoherent points", e.g. a
disjoint-class violation). Prints the count of newly entailed triples and each
inconsistency (`kind` + detail). **Exits non-zero if any inconsistency is found**
(zero if coherent), so it works as a coherence gate in CI. With `--materialize`,
also serialize the base + inferred graph (`nq` default, or `ttl`). This is a
documented subset, not full OWL DL — see [Reasoning & coherence](reasoning.md).

```sh
rete reason data.rete
rete reason data.rete --materialize --format ttl
```

## Coarse graphs (no index read)

### `rete summary <file>`
Print the **structural** coarse graph: the Louvain community quotient graph
(community → community relations with counts).

### `rete schema <file>`
Print the **semantic** coarse graph: classes (by `rdf:type`) with instance
counts, and the class→predicate→class relations between them — the dataset's
effective schema.

### `rete communities <file> [--json] [--profile] [--predicate IRI] [--round N] [--min-size N]`
Recompute the Louvain communities and expose, per community, its **member
subjects** and the **literal text** of its triples — the per-community text
corpus for downstream topic modeling. `--profile` adds a no-ML "topic" profile
per community (top literal words, `rdf:type` classes, and predicates).
`--predicate <iri>` detects communities using **only** that relation's edges — a
criterion-specific partition (see [multi-criteria splitting](multi-criteria.md)).
`--json` emits `[{community, size, members:[<iri>…], text:[lexical…]}]` (plus a
`profile` object when `--profile` is set); `--round N` cuts the dendrogram at a
specific round (default: the round chosen for the tile budget); `--min-size N`
drops communities with fewer than `N` members. See the
[topic modeling tutorial](topic-modeling.md).

```sh
rete communities papers.rete --profile               # no-ML topic labels
rete communities researchers.rete --predicate '<http://ex/cites>'  # one criterion
rete communities papers.rete --json | python3 scripts/lda_topics.py --topics 3
```

### `rete predicates <file>`
Exact per-predicate triple totals, summed from the pyramid summary alone — the
triple index is never read.

## Graph traversal

### `rete reach <file> --predicate <iri> --seed <iri>… [--seeds-file F] [--reverse] [--parallel] [--count]`
Multi-source transitive reachability over one relation: for each seed, the set of
nodes it transitively reaches. `--reverse` answers *"who reaches the seed?"*
(impact analysis); `--seeds-file` reads one seed IRI per line; `--count` prints
only sizes; **`--parallel`** runs one [rayon](BENCHMARK.md#parallelism) task per
seed (the batch-reachability workload that benchmarks ~14–15× on many cores).

```sh
# Who (transitively) depends on the vulnerable package?  (impact analysis)
rete reach deps.rete --predicate '<http://ex/dependsOn>' --seed '<http://ex/log4x>' --reverse

# Many seeds at once, in parallel:
rete reach g.rete --predicate '<http://ex/knows>' --seeds-file seeds.txt --parallel --count
```

## Over HTTP

Both URL commands work against `http://` and `https://` hosts that honor `Range`
requests (S3, GitHub, any CDN).

### `rete summary-url <url>`
Fetch only the header + dictionary + pyramid summary and print the coarse graph.
The (large) index is never fetched — the "overview first" path.

### `rete query-url <url> [--subject S] [--predicate P] [--object O]`
Match a triple pattern over HTTP, fetching only the byte ranges the query needs
and reporting how many ranges/bytes were pulled.

```sh
rete query-url https://host/data.rete --object '<http://ex/Dave>'
```

### `rete sparql-url <url> "<query>" [--json]`
Run a full SPARQL query over HTTP, range-fetching the file (header, dictionary,
index, pyramid) rather than downloading it whole.

```sh
rete sparql-url https://host/data.rete \
  "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x }"
```

## Federation

### `rete federate <sources…> --query "<SPARQL>" [--json] [--no-route]`
Run one SPARQL query across several `.rete` **sources** — local file paths and/or
`http(s)://` URLs, mixed — and merge the results at the **term (string) level**:
`SELECT` rows are unioned + deduped, `ASK` is OR'd across sources, `CONSTRUCT`
triples are unioned + deduped. **Routing** (default on) reads each source's
predicate set from its summary (index never touched) and **prunes** sources whose
predicates are disjoint from the query's; `--no-route` queries every source.
Per-source diagnostics and the queried/pruned tally go to stderr.

This is **union** federation — correct for sharded data where each file
independently yields complete rows. It does **not** do cross-file joins, and
aggregates (`COUNT`/`GROUP BY`) and `LIMIT` are evaluated **per source** then
unioned (a federated `COUNT(*)` returns per-source counts, not a global sum). See
[Federated queries](federation.md) for the full model, limitations, and a real
OpenCitations multi-shard example.

```sh
rete federate data/opencitations/cites-2021.rete data/opencitations/cites-2024.rete \
  --query 'SELECT ?citing WHERE { ?citing <http://purl.org/spar/cito/cites>
                                          <https://doi.org/10.1038/s41586-021-03819-2> } LIMIT 5'
```

## Exit codes

`0` on success; non-zero with a message on error (bad input, missing file,
corrupt file, a host that ignores `Range`, or an unsupported query construct).
