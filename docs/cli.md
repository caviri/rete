# CLI reference

The `rete` binary (crate `rete-cli`). Run `rete <command> --help` for the
authoritative flags. Terms are written as canonical N-Triples tokens —
`<http://ex/Alice>` for an IRI, `"30"` or `"30"^^<…#integer>` for a literal.

Rete-specific `--json` responses carry `"schemaVersion": 1`. This covers
Dataset Cards, provenance, search, communities, cost, progressive metadata, and
SHACL reports. Standard SPARQL Results JSON and JSON-LD keep their standard
shapes and do not add that field.

Release archives also contain Bash, Zsh, Fish, and PowerShell completions plus
the `rete(1)` man page. Maintainers generate these from the exact binary with the
hidden `rete generate --output <directory>` packaging command; they are never
maintained as hand-written copies of the CLI definition.

## Building

### `rete build <inputs…> -o <out.rete> [--format nt|nq|ttl|rdfxml]`
Build a file from one or more RDF inputs, merged under one shared dictionary.
Format is detected by extension (`.nt` / `.nq` / `.ttl`, plus `.rdf` / `.owl` /
`.rdfxml` for RDF/XML — how most OWL ontologies ship); `-` reads stdin and
defaults to N-Triples; `--format` forces a format for all inputs (use `--format
rdfxml` for an RDF/XML file with a non-standard extension). N-Quads inputs produce
a dataset with named graphs.

```sh
rete build a.nt b.nt -o merged.rete
curl -s https://host/data.nt | rete build - -o data.rete
```

`--materialize` bakes RDFS/OWL-RL entailments into the file at build time (see
[Reasoning](reasoning.md)). `--reason` runs the same reasoner but instead of
adding triples it stamps the **coherence verdict** into the Dataset Card (implies
`--card`) — so a remote reader learns whether the graph is logically coherent from
the index-free card with no compute; unlike `--materialize` it records
`coherent: false` honestly rather than aborting (verify later with `rete reason
--verify-card`, combine with `--materialize` to also bake the inferred triples).
`--card` (and `--card-file` / `--title` / `--license` / `--source` /
`--description` / `--created`) embeds a [Dataset Card](dataset-cards.md) —
data-catalog metadata plus an auto-derived profile; the card file's `extra`
object carries bounded publisher-defined custom fields (no flag — see
[Custom fields](dataset-cards.md)); a card build also writes a
**build-info section** (build timestamp, builder version, parameters, measured
starter-query costs — kept *outside* the content hash so identical data still
hashes identically). That same run is what proves each starter query answers:
one measured at zero rows (or returning a row that binds nothing) is **dropped
from the card**, loudly and with the reason recorded — see
[dataset-cards.md](dataset-cards.md). `--no-card-costs` skips the run, and so
skips that check as well. `--text-index` adds a
full-text (word/CONTAINS) index over the literals for `rete search --contains`
(see below). `--type-predicate <IRI>` overrides the predicate that types subjects
with classes for the schema pyramid (default `rdf:type`, else auto-detected) —
e.g. Wikidata's `--type-predicate http://www.wikidata.org/prop/direct/P31`.
`--no-pyramid` skips the community pyramid entirely (no pyramid section): SPARQL /
SHACL / triple-pattern / reachability queries don't use it, so the file stays
fully queryable and is markedly smaller — only community / summary / progressive
queries need the pyramid. `--pyramid-algo louvain|types` picks the community
algorithm: `louvain` (default) detects topological communities; `types` partitions
by `rdf:type` instead — deterministic, self-naming communities that stay feasible
on graphs too large for the single-threaded Louvain build (it falls back to
Louvain when the graph is untyped). All of these are opt-in; without them the
output is byte-identical to a plain build.

`--permutations 3|6` (default **6**) chooses how many index permutations to
store. `6` writes SPO, POS, OSP, SOP, PSO and OPS; `3` writes only SPO, POS and
OSP — the orders that decide *routing*. Those three tie the longest bound prefix
on all eight triple-pattern shapes, so a 3-permutation file answers **every query
with the same rows, from the same tiles**; what it gives up is the sort-merge
join, on the three (bound-set, join-column) shapes SOP/PSO/OPS existed for —
most visibly `?s <p1> ?o1 . ?s <p2> ?o2`, two bound predicates sharing a subject.
The three orders are 36.8%–50.5% of a built file
([BENCHMARK.md](BENCHMARK.md#the-merge-join-permutations-cost-vs-benefit)), and
the file records its own set in the header, so `rete info` / `rete stats` and the
card's `signals.permutations` all report it.

**A 3-permutation file is not readable by a Rete older than this one** — it
refuses loudly (`malformed container: expected 6 permutation sections`, exit 1)
rather than answering wrongly, but it does refuse. Keep the default for anything
published; see [compatibility.md](compatibility.md).

The flag exists on `build` only. The two commands that re-assemble an existing
file do not re-decide how it was built: [`repyramid`](#rete-repyramid-file--o-outrete---type-predicate-iri---pyramid-algo-louvaintypes---text-index---card-)
**preserves its input's set** (and says so on stderr when that set is not the
default six), and [`merge`](#rete-merge-inputs--o-outrete---memory-budget-mb-n---tmp-dir-dir---card-)
writes the **union** of its inputs' sets — one full shard is enough to keep the
merge-join orders its queries relied on. To change a file's set, rebuild it from
its RDF source.

### `rete repyramid <file> -o <out.rete> [--type-predicate <IRI>] [--pyramid-algo louvain|types] [--text-index] [--card …]`
Rebuild a file's pyramid **in place**, reading the triples straight from the
existing `.rete` — no `export | build` N-Quads round-trip. Use it to add a schema
pyramid (or a `--text-index` / a Dataset Card) to a file built before those
existed, or to re-derive the schema pyramid under a different `--type-predicate`
or `--pyramid-algo` (same semantics as `rete build`). The card flags match
`rete build` (`--card-file` / `--title` / `--license` / …) — and take the
curated half **only** from those flags, so a bare `--card` drops the
publisher's `title`/`license`/`source`/`description`. `--pyramid-algo` likewise
defaults to `louvain` rather than to whatever the file was built with.

It loads and materializes every quad, so its RAM tracks the **statement count**
(~350–700 B each), not the file size — roughly **80 M statements** on a 48 GB
machine, past which the staged `export --format nq | build` route is the one
that fits. See
[Maintaining a published card](dataset-cards.md#maintaining-a-published-card).

```sh
rete repyramid old.rete -o new.rete --type-predicate http://www.wikidata.org/prop/direct/P31
rete repyramid old.rete -o new.rete --text-index    # add full-text search to an existing file

# re-card a published file, carrying its curated fields across
rete card old.rete --json > old-card.json
python3 scripts/recard/card_tools.py curated old-card.json -o curated.json
rete repyramid old.rete -o new.rete --card --card-file curated.json --pyramid-algo types
```

`repyramid` **preserves its input's permutation set**: a 3-permutation file stays
3-permutation, a 6 stays 6, and it prints the preserved set on stderr when it is
not the default six. It re-assembles an existing file rather than re-deciding how
it was built, so it has no `--permutations` of its own — rebuild from source to
change the set.

### `rete merge <inputs…> -o <out.rete> [--memory-budget-mb N] [--tmp-dir dir] [--card …]`
Fold several `.rete` files into one without going back through text — the way to
consolidate a sharded dataset when the original RDF is gone or expensive to
re-emit. It reads the **shards** (dictionary-encoded and compressed, roughly a
quarter of the RDF bytes), so it skips the conversion and the parse.

It does **not** skip the sorting. The dictionary is HDT-style (`shared` /
subject-only / object-only) and IDs are assigned per section, so a term that is
subject-only in one shard and object-only in another becomes *shared* in the
merge and changes ID section: the shards' orderings do not survive the remap and
every permutation is rebuilt. Memory is bounded at both ends: each input is
opened lazily and streamed rather than loaded, and the quads feed the same
memory-bounded external builder `rete build --memory-budget-mb` uses, which
chunks and spills under this command's own `--memory-budget-mb` (default 4096)
into `--tmp-dir`. Shards carrying **named graphs** fold like any other: the
builder puts the graph in its sort key, so each graph comes back out of the
merge as one contiguous run. Card flags match `rete build`.

The merged file carries the **union** of its inputs' permutation sets — an
all-lean merge stays lean, and one six-permutation input is enough to keep the
merge-join orders. Like `repyramid`, it has no `--permutations` flag: it
consolidates shards, it does not re-decide how they were built.

```sh
rete merge shard-*.rete -o all.rete --memory-budget-mb 8192 --tmp-dir /spill \
  --card --card-file curated.json
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
Print the decoded 1 KB header (the section directory, codecs, counts, content hash) — plus
the [Dataset Card](dataset-cards.md) catalog when the file carries one. Reads only
the header and card byte ranges (the same two small reads `card-url` does over
HTTP), so a 50 GB file answers in about a second.

### `rete stats <file>`
Human-friendly overview: file size, default-graph triple count, distinct terms,
named-graph count, pyramid levels, and top predicates. Notes the label-index and
**text-index** sizes when the file carries them.

### `rete verify <file>`
Recompute the blake3 content hash and compare to the header. Exits non-zero with
`FAILED — content hash mismatch` on corruption/truncation.

### `rete search <file> [<prefix>] [--contains <word>…] [--contains-prefix P] [--limit N] [--json]`
Two modes.

**Label prefix** (default) — the subjects whose label starts with `prefix`
(case-insensitive), printed as `label<TAB><iri>` (or
`{"schemaVersion":1,"matches":[{"label":…,"subject":…}]}` with `--json`).
An empty prefix returns the first `--limit` labels (default 20).
Answered from a bounded, label-sorted block in the pyramid-meta by binary search
— **no literal scan** — so it is the fast path for autocomplete (~22× a
`FILTER(STRSTARTS(LCASE(?l), …))` scan at 6k labels; the gap widens with size).
Labels come from `rdfs:label`, `skos:prefLabel`/`altLabel`, `foaf:name`,
`dc(terms):title`, and `schema:name`; the block keeps the top 8,192 most-connected
labeled subjects. Files built before this feature carry no label index (the block
is additive — rebuild to add it).

**Full-text** (`--contains <word>…`) — the subjects whose literals contain
**every** given word (whole-word, case-insensitive — AND), printed one IRI per
line (or the same versioned `matches` envelope with `{subject}` entries under
`--json`). `--contains-prefix einst` additionally
requires a literal word starting with `einst`. Answered from the opt-in
**TEXT_INDEX** section (`rete build --text-index`); on a remote file only the
queried words' posting lists are fetched, not the whole index. A file built
without `--text-index` reports that it has no text index. To search a file you
have **not** downloaded, use `rete search-url` (below).

```sh
rete search data.rete gluc                       # label prefix (autocomplete)
rete search data.rete --contains glucose         # literals containing "glucose"
rete search data.rete --contains glucose phosphate  # both words (AND)
rete search data.rete --contains-prefix einst    # a word starting with "einst…"
```

### `rete card <file> [--json] [--format jsonld|croissant] [--sha256 <hex>]`
Print the embedded [Dataset Card](dataset-cards.md) — curated metadata
(title/license/source/creators/publisher/…) plus the derived profile (counts, top
predicates and classes, vocabularies), the content-hash checksum, and (when the
file carries one) the **build record**: when it was built, by which `rete`, with
which flags, and the starter queries' measured costs. Costs two small range
reads (header + one coalesced card/build-info range), never the dictionary or
index — instant on any size. `--json` emits the card object plus
`schemaVersion: 1` and a `build` block. `--format jsonld` projects the card to
JSON-LD (VoID + schema.org + PROV-O — already RDF when lifted out);
`--format croissant` emits the honestly-mappable Croissant subset (no
`recordSet`: a graph is not a table). `--sha256` supplies the whole-file
sha256 the Croissant projection's `FileObject` requires — a file cannot carry
its own, so compute it outside (`sha256sum file.rete`); with it the document
is validator-clean. For a file built without one it prints
`(no dataset card — …)`, still naming what the header alone decides: whether
the file carries a full-text index (`signals.text_index`, measured from the
section directory rather than read out of the card — see
[Dataset Cards](dataset-cards.md#the-full-text-signal-measured-not-stored)).
`rete info` shows the same catalog beneath the header when a card is present.

### `rete graphs <file>`
List the named-graph IRIs in a dataset (the default graph is unnamed).

### `rete export <file> [--format nq|ttl|jsonld] [--graph G] [--subject S] [--predicate P] [--object O]`
Serialize the dataset, or a slice of it. `nq` (the default) dumps every
triple/quad as N-Quads (default graph + named graphs) — a lossless round-trip.
`ttl` emits Turtle and `jsonld` emits expanded JSON-LD; both serialize a
**single graph** (the default graph unless `--graph` names one), because
Turtle/JSON-LD carry no default-vs-named distinction — use `nq` to export the
whole dataset.

```sh
rete export data.rete                 # N-Quads (default)
rete export data.rete --format ttl    # Turtle
rete export data.rete --format jsonld # expanded JSON-LD
```

**Filters prune the file; they are not `| grep`.** `--graph` /`--subject` /
`--predicate` / `--object` become a triple pattern the engine routes: it picks
the index permutation that sorts on the bound components, binary-searches its
tile directory down to the tiles that can match, and drops the rest by their
recorded synopsis *without fetching them*. On a lazily-opened file that is the
difference between exporting a slice and reading the graph.

```sh
# one predicate of one named graph: 16 MB read instead of 376 MB, 0.4 s
# instead of 12.8 s, 183 MB peak RSS instead of 2.1 GB (cordis.rete, 801 MB)
rete export cordis.rete --format nq \
    --graph http://data.europa.eu/s66/graph/results \
    --predicate http://data.europa.eu/s66#doi

rete export data.rete --format nq --graph ''            # the default graph alone
rete export data.rete --format nq --subject http://ex/a # everything about one subject
rete export data.rete --format nq --object '"text"@en'  # an exact literal
```

Terms are bare IRIs or N-Triples tokens (`<iri>`, `"lit"@en`, `"lit"^^<dt>`,
`_:b`) — quote literals for your shell. A term the file's dictionary does not
contain matches nothing, which is an empty export, not an error. `--graph ''`
selects the default graph alone; omitting `--graph` keeps the current behaviour
(default graph, then every named graph).

Two caveats worth knowing before you measure it:

- **A filtered dump prunes the index, not the dictionary.** Resolving the rows
  it keeps still faults the dictionary chunks their terms live in, so on a graph
  whose payload *is* long literals the saving is much smaller: on the same file,
  a predicate whose objects are abstracts went 261 MB → 213 MB, not 23×.
- **Rows arrive in the routed permutation's order.** Unfiltered (and
  subject-bound) that is `(s, p, o)` as before; a bound predicate streams
  `(p, o, s)`, a bound object `(o, s, p)`. The *set* is identical; N-Quads does
  not care, but `diff` does — sort both sides.

Preview any of this before running it with [`rete cost --dump`](#rete-cost-file-or-url---dump---graph-g---subject-s---predicate-p---object-o---json).

## Querying

### `rete query <file> [--subject S] [--predicate P] [--object O]`
Match a single triple pattern; omitted positions are wildcards.

```sh
rete query data.rete --predicate '<http://ex/knows>'
```

### `rete why <file> [--subject S] [--predicate P] [--object O] [--json]`
Explain the provenance of each triple-pattern result. The command reports the
matched terms, dictionary IDs, graph scope, the chosen permutation (one of the
six), and the file byte ranges for the dictionary, full index container, selected
permutation payload, and pyramid metadata. With `--json`, the same data is
emitted as stable machine-readable JSON with `schemaVersion: 1`.

```sh
rete why data.rete --predicate '<http://ex/knows>'
rete why data.rete --subject '<http://ex/Alice>' --json
```

### `rete why-url <url> [--subject S] [--predicate P] [--object O] [--json]`
The remote counterpart of `rete why`: explain a triple-pattern result over a
`.rete` served on HTTP, **range-fetching only the routed tiles** — the same
provenance (permutation, section, byte ranges, the physical tile) plus the bytes
fetched and range-request count. The CLI version of the browser's `why_url`.

```sh
rete why-url https://host/data.rete --predicate '<http://ex/knows>'
# … provenance …
# (fetched 4096 bytes in 1 range request(s); file is 1048576 bytes)
```

Provenance is honest about the physical layout: it identifies the index
container, the selected permutation payload, and — for tiled files —
the physical tile holding each match (`PERM/index`) with its compressed byte
range.

### `rete bgp <file> "<pattern> . <pattern> …"`
Evaluate a Basic Graph Pattern. Patterns are separated by ` . `, terms by spaces;
`?name` is a variable (terms may not contain spaces).

```sh
rete bgp data.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"
```

### `rete sparql <file> "<query>" [--json] [--entail]`
Run SPARQL: `SELECT` / `ASK` / `CONSTRUCT` / `DESCRIBE`. With `--json`, emit
standard SPARQL Results JSON (for `SELECT`/`ASK`). See [SPARQL support](sparql.md).

`--entail` turns on **OWL 2 QL reasoning**: the query is rewritten so the answer
includes ontology-entailed solutions (`rdfs:subClassOf` / `subPropertyOf` /
`domain` / `range` / `owl:inverseOf` / `someValuesFrom`), computed over the raw
data with no materialization — off by default, so a plain query is unchanged. Same
flag on `rete sparql-url` reasons over a remote file, fetching only what the
rewritten query touches. See [Reasoning by query rewriting](reasoning.md#reasoning-by-query-rewriting-owl-2-ql).

There is **no union-default-graph flag** here: the opt-in ⛁ All graphs mode —
a pattern outside `GRAPH` matching the merge of the default graph and every
named graph, for files whose data lives entirely in named graphs — exists
today in the playground and the wasm `query_opts` API only, not on the CLI and
not in `rete serve`. On the CLI, scope the query with `GRAPH ?g { … }`
instead. See [Union default graph](sparql.md#union-default-graph).

```sh
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?p (COUNT(?f) AS ?n) WHERE { ?p e:knows ?f } GROUP BY ?p"
rete sparql data.rete "SELECT ?o WHERE { ?o a <…/Aves> }" --entail    # ontology-aware
```

**Memory & I/O.** Local files above 1 GiB open through the same lazy range
reader the `-url` commands use (threshold: `RETE_LOCAL_LAZY_ABOVE_MB`, block
size: `RETE_BLOCK_KB`, cache capped at 256 MiB), and aggregation streams
through per-group accumulators — a `COUNT` over a 9.83 B-triple file runs in a
2 GiB container ([benchmark](BENCHMARK.md)). Preview a query's byte cost with
`rete cost`, and see the exact ranges a query read with `rete why`.

### `rete serve <file> [--bind addr] [--token t] [--journal path]`
Serve one `.rete` — or a [manifest](manifest.md) of segments (`.json`), whose
visible fold becomes the served state — as a live **SPARQL 1.1 Protocol
endpoint — queries and SPARQL Update**. The base file is **never mutated**:
updates append to a plain-text journal next to it (`<file>.changes`, one
`+`/`-`-prefixed N-Quads line per change) and the merged state answers the
very next query; a restart replays the journal. `GET /snapshot.rete` downloads the current state as a
fresh `.rete` — the update cycle's publishable artifact (upload the snapshot,
delete the journal). Any rete client — including the browser playground — can
federate against the endpoint with `SERVICE <http://host:port/sparql>`.

- `GET/POST /sparql` with `query=` → results (`application/sparql-results+json`;
  CONSTRUCT as N-Triples). Queries may themselves contain `SERVICE` blocks.
- `POST /sparql` (or `/update`) with `update=` or an
  `application/sparql-update` body → `INSERT DATA` / `DELETE DATA` /
  `DELETE/INSERT … WHERE` / `CLEAR` / `DROP` (`LOAD` and `USING` are rejected).
- Binds loopback by default. Expose deliberately (`--bind 0.0.0.0:7878`) and
  set `--token`: updates then require `Authorization: Bearer <t>` (reads stay
  open). CORS is enabled, so browsers can query it directly.
- Scale: the state lives in memory and rebuilds after writes — built for
  living small/medium datasets (annotation, curation; up to a few million
  triples), not the multi-GB catalog files.

```sh
rete serve notes.rete
curl -s --data-urlencode 'query=SELECT * WHERE { ?s ?p ?o } LIMIT 5' http://127.0.0.1:7878/sparql
curl -s --data-urlencode 'update=INSERT DATA { <http://ex/n1> <http://ex/note> "hello" }' http://127.0.0.1:7878/sparql
curl -sO http://127.0.0.1:7878/snapshot.rete   # the updated companion .rete
```

### `rete cost <file-or-url> "<query>" [--json] [--explain]`
Preview the byte/range-request cost of a SPARQL query without evaluating it.
The report parses the query, lists the concrete predicates that can drive
summary-based routing, and compares three access paths:

- **summary overview** — header + dictionary + pyramid summary, skipping the
  triple index.
- **routed pattern open** — for a single default-graph triple pattern, header +
  dictionary + the one selected permutation payload (the best of the six).
- **full query open** — the current SPARQL engine path, which opens dictionary +
  index (+ pyramid/named-graph metadata when present) before evaluation.

```sh
rete cost data.rete "PREFIX e: <http://ex/> SELECT ?y WHERE { e:Alice e:knows ?y }"
rete cost https://host/data.rete "ASK { ?s <http://ex/knows> ?o }" --json
```

#### `rete cost <file-or-url> --dump [--graph G] [--subject S] [--predicate P] [--object O] [--json]`

The same preview for a **dump** — what `rete export` (or a client's streaming
dump) will fetch for that filter, before it starts. Same report shape as above.

The index figure is *computed*, not sampled: the tile directories say which
tiles the filter's routed scan can touch and how big each is, so no tile is
fetched to produce it.

```
$ rete cost cordis.rete --dump \
      --graph http://data.europa.eu/s66/graph/results \
      --predicate http://data.europa.eu/s66#doi
  file bytes: 801016143
  lazy dump open: 43156179 bytes in 66 range request(s) · reads index
  graphs selected: 1
    <…/graph/results>: POS · 31 of 528 tile(s) admitted · 1217273 bytes (section 21227100)
  index tiles: 31 of 528 admitted · 1217273 bytes of 21227100 · computed from the
               tile directories, no tile fetched
  dictionary ceiling: 417246556 bytes
  estimated dump cost: 44373452 – 461620008 bytes
```

Read the range honestly: the **floor** (open + admitted tiles) is exact; the
**ceiling** adds the whole dictionary, which only a dump touching every term
pays. Term resolution faults chunks the tile directories cannot name, so the
true cost sits between — near the floor when the slice is small and its terms
are short, near the ceiling on a literal-heavy file however well the index
prunes. The dump previewed above actually read 59,672,911 bytes.

For the exact summary-only shapes `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }`,
`SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`,
`SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`,
`SELECT DISTINCT ?p WHERE { ?s ?p ?o }`,
`SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`, `ASK { ?s <p> ?o }`,
and `ASK { ?s ?p ?o }`, the JSON output includes `summary_answer` with the
exact count/boolean value read from the pyramid summary. Predicate-specific
shapes also include the predicate; predicate totals return all predicate/count
pairs, predicate lists return all predicates present in the summary, and
predicate distinct counts return the number of predicates. More complex shapes
are marked `requires_index`.

Add `--explain` to include a planner explanation. In JSON, this adds an
`explain` object with the classified `query_shape`, whether the answer is
`summary_exact`, the planned access path (`summary-only`, `routed-pattern`, or
`full-index`), and whether the current engine path still reads the index.

For HTTP(S), the host must honor `Range` requests, just like `query-url` and
`sparql-url`. Treat this as a deployment/debugging preview: it reports the
current SPARQL engine's range budget, the summary budget, and the exact routed
single-pattern budget when the query shape allows it.

### `rete progressive <file-or-url> "<query>" [--json]`
Run the first summary-only progressive query path. This command answers only
the exact shapes that can be proven from the pyramid summary without opening
the triple index:

- `SELECT (COUNT(*) AS ?n) WHERE { ?s <p> ?o }`
- `SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }`
- `SELECT ?p (COUNT(*) AS ?n) WHERE { ?s ?p ?o } GROUP BY ?p`
- `SELECT DISTINCT ?p WHERE { ?s ?p ?o }`
- `SELECT (COUNT(DISTINCT ?p) AS ?n) WHERE { ?s ?p ?o }`
- `ASK { ?s ?p ?o }`
- `ASK { ?s <p> ?o }`

```sh
rete progressive data.rete "PREFIX e: <http://ex/> SELECT (COUNT(*) AS ?n) WHERE { ?s e:knows ?o }" --json
```

The JSON output is SPARQL Results JSON with `schemaVersion: 1` and an added `progressive` object
describing the summary stage, exactness, predicate when one is fixed, bytes
fetched, request count, and `reads_index: false`. Other query shapes fail
clearly; use `rete sparql` for full-index evaluation or `rete cost --explain` to
inspect why a query is not summary-answerable yet.

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

### `rete reason [<file>] [--url <url>] [--materialize] [--check] [--verify-card] [--format nq|ttl]`
Run the prototype **OWL RL / RDFS reasoner**: materialize RDFS/OWL entailments to
a fixpoint and report any logical **inconsistencies** ("incoherent points", e.g. a
disjoint-class violation). Prints the count of newly entailed triples and each
inconsistency (`kind` + detail). **Exits non-zero if any inconsistency is found**
(zero if coherent), so it works as a coherence gate in CI. With `--materialize`,
also serialize the base + inferred graph (`nq` default, or `ttl`). This is a
documented subset, not full OWL DL — see [Reasoning & coherence](reasoning.md).

- `--url <url>` reads a **remote** `.rete` over HTTP range requests instead of a
  local file (omit the positional `<file>`).
- `--check` is **coherence-gate mode**: print one verdict line and exit non-zero
  on any incoherent point (suppresses `--materialize` output) — the minimal CI gate.
- `--verify-card` checks the file's **baked coherence card** (from `rete build
  --reason`) against a fresh reasoning run, guarding against drift or a stale
  ruleset; it exits non-zero if the stored verdict disagrees with recomputation.

```sh
rete reason data.rete
rete reason data.rete --materialize --format ttl
rete reason data.rete --check                  # one-line CI verdict
rete reason --url https://host/data.rete       # reason over a remote file
rete reason data.rete --verify-card            # baked card vs fresh run
```

## Shape validation

### `rete shacl <file> --shapes <shapes.ttl> [--graph <iri>] [--format text|json|ttl]`
Validate a `.rete` graph against SHACL Core shapes read from Turtle. The default
graph is validated unless `--graph` names one dataset graph. The command exits
zero when the report conforms and non-zero when it finds validation results, so
it can be used as a CI data-quality gate. See [SHACL validation](shacl.md) for
the supported components and current limits.

```sh
rete shacl data.rete --shapes shapes.ttl
rete shacl data.rete --shapes shapes.ttl --format json
rete shacl data.rete --shapes shapes.ttl --graph '<http://ex/snapshot>'
```

### `rete shacl-url <url> --shapes <shapes.ttl> [--format text|json|ttl]`
Validate a **remote** `.rete` over HTTP, **range-reading only what the shapes
target**. The file is opened lazily and each focus node's values are fetched as
routed range reads, so a targeted shape (`sh:targetClass` / `targetNode` /
`targetSubjectsOf` / `targetObjectsOf`) never downloads the whole graph — it
faults only the tiles holding the target nodes and their property values. Reports
the bytes fetched and the range-request count. Validates the default graph.

```sh
rete shacl-url https://host/data.rete --shapes shapes.ttl
# (fetched 38912 bytes in 7 range request(s); file is 1048576 bytes)
```

## Coarse graphs (no index read)

### `rete summary <file> [--level k]`
Print the **structural** coarse graph: the Louvain community quotient graph
(community → community relations with counts), plus — for v2 files — the **schema
pyramid**: a leveled `rdf:type` histogram where abstract classes describe coarse
levels and leaf classes resolve as you zoom in (e.g. `Agent → Person → Scientist
→ Astronomer`). `--level k` prints just level `k`'s type histogram (`0` =
coarsest / most abstract). Everything here reads **index-free** from the
pyramid-meta — `summary-url` shows the same over HTTP without fetching the index.

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
`--json` emits `{schemaVersion:1, communities:[{community, size,
members:[<iri>…], text:[lexical…]}]}` (plus a `profile` object on each record
when `--profile` is set); `--round N` cuts the dendrogram at a
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

### `rete card-url <url> [--json] [--format jsonld|croissant] [--sha256 <hex>]`
Fetch only the embedded [Dataset Card](dataset-cards.md) — the header and one
coalesced metadata + build-info range, in **two small range requests**. The
dictionary, index, and pyramid are never fetched: this is the index-free **CARD
tier**, the cold-start self-description (counts, vocabulary, class graph,
signals, starter queries, build record) a client reads before it knows what to
query. Reports bytes fetched + range count. The `--format` projections (and
`--sha256` for Croissant) match `rete card`.

```sh
rete card-url https://host/data.rete --json
```

### `rete card-audit <path|url> [--json] [--measure] [--only IDS] [--max-mb N] [--write-costs] [--allow-empty]`
Do the starter queries a card **already ships** still answer on the file that
carries them? A published `.rete` cannot be re-carded for free, so this decides
each query's fate from the card's own profile — the CARD tier again, so a
multi-GB file costs tens of KB to check.

Each query gets one verdict, and the two that matter are kept apart: `empty` is
the card *refuting* a shipped query (a property path through a predicate the
file does not have, a `VALUES` list disjoint from the dataset's link predicates,
a class joined to a label predicate the class-link quotient accounts for and
never pairs with it), while `undecidable` and `suspect` are the honest middle —
run the query to settle those. `answers` is the card proving a row comes back,
and `revision` says what a re-card would do to the body (`current`,
`superseded`, `dropped`).

The judgement is the query generator's own (`Cap::joint_with`, `NonEmpty`,
`provably_empty`), not a second copy of it. Input is a `.rete` (local path or
URL) or a card JSON document from `rete card --json` / `rete card-url --json`.

```sh
rete card-audit https://host/data.rete
rete card-audit card.json --json | jq '.findings[] | select(.verdict=="empty")'
```

#### `--measure`: run them instead of reasoning about them

The static pass has a hard ceiling. Some templates are undecidable **by
construction** — nothing in a card ties a subject to a predicate, so `top-reach`
cannot be settled from one, and a card does not record which objects are also
subjects, so `top-dangling` cannot either. No better card-only reasoning closes
that; a run does, and records what the answer cost.

`--measure` runs each starter query **cold** — a fresh lazy open, logical range
reads, no block cache — and reports rows, bytes, requests and a reference
timing. It is the same `measure_query` a `rete build` uses to fill in its
`query_costs`, so when the file already carries a build record the command
checks itself against it and says whether the two agree (`= build record` /
`!= build record`). On `switzerland-fedlex.rete` — the one published file that
has a record — all ten queries reproduce it exactly.

Each row shows the card's verdict and the run's outcome side by side; they are
never merged, because one is what a card can prove and the other is what the
file did.

```
card says    run says   query             rows               bytes      req           ms
answers      answers    ov-triples           1 row(s)       2800822 B     74 req      294 ms
suspect      empty      lb-labels            0 row(s)        228126 B     68 req       12 ms
undecidable  answers    top-dangling       100 row(s)       4468038 B    118 req      592 ms
```

**Local or remote.** Point it at a path or at an `http(s)://` URL. `bytes` and
`requests` are the same quantity either way — no block cache is in the stack, so
the range sequence is a function of layout and query, not of transport — but
only the remote run actually pays for them. The output always names which it
was, in the text header and in the JSON `measurement.transport`, because a cost
figure without its transport is not a cost figure.

**It is a download.** `--only ov-triples,lb-labels` measures a subset;
`--max-mb 8` abandons a query once it has asked for more than that, and reports
the abandonment with the bytes it spent — "costs more than 8 MB" is itself an
answer. Both matter: eight of `switzerland-fedlex`'s ten starter queries read
~1.02 GB each, so measuring that card remotely without a leash is an 8 GB
download.

#### `--write-costs`: make the measurement durable

Records the run in the file's build-info section, so the next reader gets the
figures from the CARD tier (two range requests) instead of re-measuring. Local
files only.

The section sits **outside** the blake3 content hash — that is why two builds of
identical data hash equal — so the file keeps its identity: same checksum, same
`rete verify`, byte-identical N-Quads. It sits **near the front**, though, right
after the card, so making room for it shifts everything behind it and the file
is rewritten end to end. The rewrite streams through a 4 MiB buffer and commits
by rename, but it is still one full pass of I/O and — for a published file — a
full re-upload.

Where that is worth it: when the alternative is a re-card. A re-card rewrites
the file too, and additionally costs 17–35× the file in RAM (`repyramid`) or
9–15× in staged N-Quads on disk (`--mode stream`, see `scripts/recard`), which
puts anything past ~150 M statements out of reach. Attaching costs is one pass
with no staging.

**The RAM goes into the measurement, not the rewrite**, and it is not free
either: the engine evaluates eagerly, so a starter query with a big result
materializes it. `switzerland-fedlex` (1.04 GB, 56.3 M quads) took 381 s and
peaked at **14.2 GiB** for `--measure --write-costs` — `ng-list` alone, which
returns 497,905 rows, accounts for 3.2 GiB of that; the rewrite of the 1.04 GB
file that followed is a bounded-buffer copy. Against `repyramid`'s ≈36 GiB
prediction and the staged path's ≤19.1 GiB for the same file, it is the cheaper
route, but budget for the queries, not for the file.

So: if the audit says the queries are stale or broken, re-card — same rewrite,
more value. If the queries answer and the only gap is the missing record, write
the costs. The command enforces that split: it refuses when a query measured
zero rows (`--allow-empty` overrides, for `top-dangling` on a fully-described
graph), when a run did not finish, and when `--only` measured a subset that
would be stored as if it were the whole card.

```sh
rete card-audit data.rete --measure --json | jq '.findings[] | select(.observed.outcome=="empty")'
rete card-audit https://host/data.rete --measure --only ov-triples --max-mb 8
rete card-audit data.rete --measure --write-costs
```

### `rete search-url <url> [<prefix>] [--contains <word>…] [--contains-prefix P] [--limit N] [--json]`
`rete search` over HTTP — the same two modes and the same output, without
downloading the file.

The open is deliberately narrower than any other remote command's. It reads the
header and the **subject** halves of the dictionary (the shared and subject-only
sections), and stops: no permutation tile directories, no pyramid, no index.
`--contains` then faults the TEXT_INDEX token table once and one range request
per posting list; the bare prefix mode faults the pyramid instead, where the
label index lives.

That narrowness is the point, because on a literal-heavy graph the dictionary is
most of the file, and on a file built before 2026-08 its **object-only chunk
directory** — which carried every chunk's first term verbatim — is most of what a
normal open costs. (Newer builds key that directory by the shortest *separator*
instead, a few bytes per chunk: on `epfl-infoscience` that is 234 MB of routing
keys against ~600 KB. It is a write-side change, so a published file keeps its
large directory until it is rebuilt — the figures below are the published one.)
Searching `epfl-infoscience.rete` (1.64 GB; a 1.35 GB dictionary, a 186 MB text
index over titles and 132k abstracts) for `photosynthesis`:

| | bytes fetched | requests | time |
|---|---|---|---|
| `sparql-url` with `FILTER(CONTAINS(…))` | 334 MB | 83 | 28.6 s |
| `search-url --contains` | **29.5 MB** | **5** | **4.8 s** |

Same five hits. The 29.5 MB is the token table itself; label-prefix mode over the
same file costs 1.4 MB in 4 requests. A file built without `--text-index` says so
instead of silently scanning.

```sh
rete search-url https://host/data.rete --contains glucose phosphate
rete search-url https://host/data.rete --contains graphs --contains-prefix existen --json
rete search-url https://host/data.rete "Photosynth" --limit 3   # label prefix
```

### `rete summary-url <url>`
Fetch only the header + dictionary + pyramid summary and print the coarse graph.
The (large) index is never fetched — the "overview first" path.

### `rete query-url <url> [--subject S] [--predicate P] [--object O]`
Match a triple pattern over HTTP, fetching only the byte ranges the query needs
and reporting how many ranges/bytes were pulled. Bound terms are resolved from
the dictionary first; if they exist, the reader fetches only the selected
permutation payload rather than the whole index container. If a
bound term is unknown, the index is skipped entirely and the result is empty.

```sh
rete query-url https://host/data.rete --object '<http://ex/Dave>'
```

### `rete sparql-url <url> "<query>" [--json]`
Run a full SPARQL query over HTTP with **lazy tile faulting** (tiled
files): the open fetches the header, dictionary, pyramid, and the index's
small tile directories; index tiles are then range-fetched only when the
query's scans and probes touch them, so a selective query reads O(touched
tiles) rather than the whole index. A range failure mid-query is reported as
an error, never as silently fewer rows.

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

### `rete manifest <init|add|status|query|seal|compact>`
Manage a **writable logical graph** as an ordered log of immutable `.rete`
segments plus tombstone (deletion) segments, described by a small JSON
manifest — grow a dataset across sessions, delete/update quads, query it all
as **one** graph (joins across segments resolve, unlike `federate`'s UNION),
and fold it back into a single `.rete`. `rete serve` accepts a manifest, and
`manifest seal` checkpoints the server's journal into fresh segments. See
[Writable graphs — manifest & WAL](manifest.md).

```sh
rete manifest init   g.rete-manifest.json base.rete
rete manifest add    g.rete-manifest.json --adds delta.rete [--dels tomb.rete]
rete manifest query  g.rete-manifest.json "SELECT … WHERE { … }"
rete manifest status g.rete-manifest.json --count
rete manifest seal   g.rete-manifest.json     # journal → segments (stop serve first)
rete manifest compact g.rete-manifest.json    # the whole log → ONE fresh .rete
```

## Exit codes

- `0` — success.
- `1` — runtime, data, or network failure: malformed RDF, a missing/corrupt or
  unsupported `.rete`, HTTP failure, ignored `Range`, or unsupported query input.
- `2` — command-line usage error reported by Clap.
- `3` — the command completed its requested check, but the graph failed it:
  SHACL non-conformance or reasoning incoherence.
