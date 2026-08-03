# Dataset Cards

A **Dataset Card** is a small, embeddable data-catalog record carried *inside* a
`.rete` file. It turns an opaque graph blob into a **self-describing dataset**:
who made it, under what license, where it came from — plus an auto-computed
profile of what's in it (counts, the predicates and classes actually used, the
vocabularies they belong to). One `rete card` (or `rete info`) reads it back, so a
`.rete` file doubles as its own mini data catalog.

The card lives in the file's **metadata section** — a slot the format reserves
right after the 1 KB header. Adding a card is **not a format change**: a file
without one is byte-for-byte identical to a pre-card build, and an older reader
simply ignores the section. The card is folded into the file's `blake3` content
hash, so `rete verify` covers it and it is **tamper-evident**.

## Building a card

Cards are **opt-in**: a plain `rete build` writes no card and is unchanged. Pass
any card flag to embed one. The statistics are derived from the data; the curated
fields (title, license, …) are yours to supply.

```sh
# Auto-derived stats only (no curated fields):
rete build data.nt -o data.rete --card

# With curated catalog metadata:
rete build data.nt -o data.rete \
  --title "Citation graph 2021" \
  --license "CC0-1.0" \
  --source "https://example.org/citations" \
  --description "Open citations sharded by year" \
  --created 2026-06-08
```

For the curated fields — including a list of **example queries**, which has no
flag — supply a small JSON file with `--card-file`. Explicit flags override
whatever the file provides:

```json
{
  "title": "Citation graph 2021",
  "license": "CC0-1.0",
  "source": "https://example.org/citations",
  "description": "Open citations sharded by year",
  "example_queries": [
    "SELECT ?citing WHERE { ?citing <http://purl.org/spar/cito/cites> ?cited }"
  ]
}
```

```sh
rete build data.nt -o data.rete --card-file card.json --title "Override title"
```

Any of `--card`, `--card-file`, `--title`, `--license`, `--source`,
`--description`, or `--created` opts the build into writing a card.

## What's in a card

| Field | Source | Meaning |
|-------|--------|---------|
| `title`, `description`, `license`, `source`, `created` | **curated** (flags / `--card-file`) | Free-text catalog metadata. Omitted fields are absent from the JSON. |
| `version`, `doi`, `cite_as` | **curated** (`--card-file` only) | Dataset version (Croissant requires one), DOI IRI, preferred citation. |
| `creators`, `publisher` | **curated** (`--card-file` only) | People (`{name, orcid}`) and organisation (`{name, ror}`). ORCID/ROR as **IRIs, not strings** — this project publishes both authority graphs, so "which datasets did this person build?" becomes a federated join, not a text search. |
| `canonical_url`, `sparql_endpoint` | **curated** (`--card-file` only) | Where the authoritative copy of this file lives (`void:dataDump`) and a public endpoint (`void:sparqlEndpoint`). A `.rete` found on a disk can then say where to verify against. |
| `source_date`, `derived_from` | **curated** (`--card-file` only) | The source data's own snapshot date (distinct from `created` and from the build timestamp), and what this file was derived from (`prov:wasDerivedFrom`) — dumps, endpoints, or the shards a `rete merge` folded in. |
| `example_queries` | **curated** (`--card-file` only) | Sample queries a consumer can run. |
| `triple_count` | derived | Triples in the **default graph**. |
| `quad_count`, `named_graph_count` | derived | Total statements and number of named graphs. |
| `term_count` | derived | Distinct dictionary terms. |
| `predicates` | derived | Each predicate IRI with its default-graph triple count, **descending**. |
| `classes` | derived | Each `rdf:type` object (class) with its instance count, descending. |
| `vocabularies` | derived | Distinct **namespaces** of the predicate and class IRIs (the prefix up to the last `#` or `/`). |
| `datatypes` | derived | `DATATYPE(o)` histogram over literal objects (bracketed datatype IRIs; `…#langString` for language-tagged), descending. |
| `languages` | derived | `LANG(o)` histogram over literal objects (`""` = untagged/typed), descending. |
| `class_links` | derived | The **effective schema**: `(s_class, predicate, o_class, count)` rows — the class-to-class quotient (same as `rete schema` / `schema_summary`, with `(literal)`/`(untyped)` sentinels). |
| `top_hubs`, `in_hubs` | derived | Top subjects by out-degree and top non-literal objects by in-degree. |
| `signals` | derived | Detected **affordances**: `label_predicate`, `base_iri`, `default_lang`, ranked `time_predicates` / `numeric_predicates`, present `link_predicates`, `geo_wkt` / `geo_latlong`, `temporal_extent`, `spatial_bbox` (CRS84 lon/lat). |
| `queries` | derived | The auto-generated, **tiered starter-query library** (see below). |
| `truncated` | derived | `true` iff any capped list was actually cut (the profile is partial). |
| `top_n` | derived | The cap the profile lists were derived under — the number `truncated` was hinting at without stating. |
| `format_version` | derived | The `.rete` format version the card was written against. |

The per-predicate and per-class statistics are computed over the **default
graph** (named-graph contents are summarized only by `quad_count` /
`named_graph_count`), matching `rete stats` and `rete predicates`. Every derived
list is **capped** and **deterministically ordered** (count-descending, ties
broken lexically), so building the same input twice yields a **byte-identical**
card — the card folds into a reproducible content hash. Counts are over the raw
(pre-dedup) multiset, matching `rete progressive`.

## Build conditions: the build-info section

The card answers *what the data is*; a second, adjacent record answers **how
this particular file came to be** — the questions you ask when a file behaves
oddly and there was previously no way to answer them from the file ("which
`rete` even wrote this?"). Every card build also writes a **build-info
section** (format section kind `7`, laid out immediately after the card)
carrying:

- `built_at` — when the file was written (RFC 3339 UTC; `SOURCE_DATE_EPOCH` is
  honored for reproducible pipelines). Distinct from the curated `created` and
  `source_date`, which describe the *data*.
- `builder` — the binary that wrote the file (`rete-cli 0.3.2`).
- `params` — the flags that shaped the result: `--no-pyramid`, the pyramid
  algorithm, `--text-index`, `--materialize`/`--reason`,
  `--memory-budget-mb`, the section codec, and the card's `top_n` cap.
- `query_costs` — measured cost figures for every starter query (below).

### Why it sits outside the content hash

The card folds into the file's reproducible blake3 content hash, and **two
builds of identical data must keep hashing identically** — that property is
load-bearing (release parity checks, dedup, cache validators). A timestamp and
a timing are exactly the facts that differ between two such builds. So the
build-info section is **deliberately excluded from the hash**: `rete verify`
does not cover it (old readers see an unknown kind-7 entry and ignore it; new
readers skip it knowingly), stripping it from two builds of the same input
yields byte-identical images, and its contents are advisory provenance, not
integrity-protected data. Cardless builds carry no build-info at all and remain
byte-identical to pre-build-info output.

### The starter-query cost figures

Each starter query is run once, **cold** (a fresh lazy open per query, what a
stateless remote client pays), against the finished image. Two kinds of number
are recorded together but labelled apart:

- **`bytes` and `requests` are portable.** They are a property of the file's
  layout and the query — the same query against the same file pulls the same
  bytes from R2, GitHub Pages, or disk. They state the CARD/summary/index-tier
  claims in checkable numbers instead of prose. (Counting is of *logical* range
  reads, no block cache; a block-caching client coalesces requests and rounds
  bytes up to blocks.)
- **`debug_ms` is not.** It is one machine's wall clock at build time — a
  debug reference, never a guarantee — and is stored **with its context**
  (`engine`, `transport`, and a note saying exactly this) so it cannot be
  quoted bare. The pairing is what makes it interpretable: "12 ms for 24 KB in
  9 requests" survives being read elsewhere; "12 ms" does not.

`--no-card-costs` skips the measurement. The memory-bounded external build and
`rete merge` record timestamp/builder/params but no costs (their cards carry no
derived starter queries to measure).

### The one-row smoke query

Every generated library now begins with `ov-one-row`: *return exactly one
statement*. It is the unambiguous "did this file open and answer?" probe — the
previous nearest thing was a `COUNT`, which on a named-graph-only file honestly
answers `0` and reads as failure. The body is graph-scope aware like every
other starter, so it returns exactly one row on any non-empty file.

## The three-tier exploration model

The enriched card exists to fix a graph's **cold-start problem**: a newcomer who
opens a `.rete` has no reference for what to ask. Every exploration question is
answerable from one of three tiers, in increasing cost — and the card makes that
explicit so a client knows what is free before it runs anything:

| Tier | Source | Cost |
|------|--------|------|
| 🟢 **Card** | this metadata section, read once on open | index-free, instant |
| 🔵 **Summary** | the pyramid superedge totals **and the [schema pyramid](semantic-zoom.md)** — a leveled `rdf:type` histogram (`rete summary [--level k]` / `summary-url`) | index-free |
| 🟠 **Index** | the triple index (range-fetched tiles) | O(touched tiles) |

`rete card-url <url>` reads the **Card tier** over HTTP in two small range
requests — the header, then **one coalesced range covering the card and the
adjacent build-info** — and **never touches the index**, so a remote/S3 client
gets the whole self-description (counts, vocabulary, class graph, signals,
starter queries, build record) without downloading the file. Measured on the
published catalog: 7,670 of 71,237,191 bytes for NKOD and 54,601 of
1,161,874,550 bytes for Hugging Face, two range requests each.

## The starter-query library

`queries` is a vetted set of starter SPARQL queries, **auto-instantiated at
build** with the dataset's own vocabulary (`{{TOP_CLASS}}` → the most populous
class, `{{LABEL_PRED}}` → the detected label predicate, and so on) and emitted
**only when the required signal is present** — no geometry query without
geometry, no time query without a time predicate — so the shipped set is
guaranteed to return rows. The bodies are also **scoped to where the
statements live** (see [Named-graph datasets](#named-graph-datasets) below).
Each query carries:

- a full **PREFIX block** (the engine injects none, so every query is runnable as-is);
- a `dimension` (overview / identity / labels / types / topology / links / literals / time / space / graphs);
- a `tier` tag (`card` / `summary` / `index`) — the cheapest tier that answers it;
- the `requires` capability keys that gated its emission.

A publisher's own `example_queries` (curated, `--card-file`) are kept alongside
the generated library, unchanged.

## Named-graph datasets

Some datasets (DCAT catalogs like NKOD, provenance stores, anything built from
N-Quads) keep **every statement in named graphs** — the default graph is empty,
and the card records exactly that (`triple_count: 0`, `named_graph_count > 0`).
On such a file a default-graph starter like
`SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }` can only ever answer `0`.

The generator therefore reads the card's own counts and picks each query's
body for **where the data actually lives**:

| Shape | Detected by | Bodies |
|-------|-------------|--------|
| default-graph only | `named_graph_count == 0` | the classic bodies, byte-identical to earlier builds |
| named-graph only | `triple_count == 0` and `named_graph_count > 0` | `GRAPH ?g { … }`-scoped; a template with no meaningful graph-scoped form is dropped rather than shipped as a guaranteed-zero-rows query |
| mixed (both hold data) | both counts non-zero | overview and hub queries scan `{ … } UNION { GRAPH ?g { … } }` so neither half is silently hidden; profile-driven queries keep addressing the default graph their vocabulary came from, and the `graphs` family surfaces the named side |

A named-graph file also gets the `graphs` dimension: which graphs exist
(`ng-list`), which are biggest (`ng-sizes`), and a sample of quads with `?g`
projected so each row says where it lives (`ng-sample`). Graph-scoped bodies
need the triple index, so they are tagged `index` — the `summary` fast path
covers the default graph only.

### Fixing the card on an existing named-graph `.rete`

The library is generated **at build time and baked into the file**, so a
`.rete` published with an older generator keeps its default-graph-scoped
starter queries until its publisher re-embeds a card. That does **not** require
going back to the source RDF — `rete repyramid` reads every statement (default
graph and named graphs) straight out of the existing file and re-assembles it,
deriving a fresh card on the way:

```sh
# One command, no source RDF needed — data is carried over losslessly:
rete repyramid catalog.rete -o catalog-fixed.rete \
  --card --title "National Open Data Catalog" --license "CC0-1.0"

rete card catalog-fixed.rete --json   # the queries are now GRAPH-scoped
```

The equivalent round-trip through text works too (`nq` export is lossless,
named graphs included), and is the place to add curated fields or your own
`example_queries` via `--card-file`:

```sh
rete export catalog.rete --format nq > catalog.nq
rete build catalog.nq -o catalog-fixed.rete --card-file card.json
```

Either way the **file is rewritten** — the card is folded into the `blake3`
content hash, so there is deliberately no in-place patch — and the fixed file
replaces the published one. On a named-graph-only input, every query in the
new card is `GRAPH`-scoped and returns rows; for example `ov-triples` becomes:

```sparql
SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }
```

## Interoperability: JSON at rest, RDF on demand

A card lifted out of a `.rete` should already be RDF — a dataset describing
itself in standard vocabularies. The first design decision was **JSON-LD at
rest versus a projection**, and it was decided by measurement, not preference.
The stored card is not valid JSON-LD as-is: its partition lists are arrays of
2-tuples (`["<iri>", 123]`), which JSON-LD cannot lift to meaningful RDF, so an
at-rest card needs one object per row (`{"void:property": …, "void:triples":
…}`) plus a context. Converting the two reference cards to that *minimal
faithful* at-rest form costs:

| card | stored JSON | at-rest JSON-LD | delta |
|---|---|---|---|
| NKOD (named-graph catalog) | 6,649 B | 7,484 B | **+12.6%** |
| Hugging Face Hub (53 KB, truncated profile) | 53,580 B | 61,899 B | **+15.5%** |

Every reader pays the metadata section on every open, across the whole catalog
— a permanent double-digit tax to serve occasional RDF consumers. So the card
**stays plain JSON at rest**, and the RDF view is a **pure projection**:
`rete card --format jsonld` (and `card-url`) reshapes bytes already fetched —
no network, no index read — and there is no second artefact to drift, because
the projection is derived from the stored card on every call.

### The JSON-LD projection (`--format jsonld`)

One document, typed both `schema:Dataset` **and** `void:Dataset` — one dataset
described in two vocabularies, not two objects. The mapping reuses standard
terms before inventing any:

| card | projected as |
|---|---|
| `quad_count` | `void:triples` (the whole dataset) |
| `predicates` | `void:propertyPartition` (`void:property` + `void:triples`) |
| `classes` | `void:classPartition` (`void:class` + `void:entities`) |
| `vocabularies` | `void:vocabulary` |
| `sparql_endpoint`, `canonical_url` | `void:sparqlEndpoint`, `void:dataDump` (+ a `schema:DataDownload` distribution) |
| `title`/`description`/`license`/`version`/`created`/`cite_as`/`doi` | `schema:name`/`description`/`license`/`version`/`dateCreated`/`citation`/`identifier` |
| `creators`, `publisher` | `schema:Person` (ORCID as `@id`) / `schema:Organization` (ROR as `@id`) |
| `source`, `derived_from` | `prov:wasDerivedFrom` (URLs) / `dct:source` (free text) |
| build info | `prov:wasGeneratedBy` — a `prov:Activity` with `prov:endedAtTime` and the builder as a `schema:SoftwareApplication` agent |
| temporal/spatial signals | `schema:temporalCoverage` / `schema:spatialCoverage` (GeoShape box) |
| `triple_count`, `term_count`, `named_graph_count`, the content hash | a small `rete:` namespace (`https://w3id.org/rete/card#`) — what no standard covers, rather than a bent term |

The projection is validated JSON-LD: it expands under `pyld` and yields the
intended VoID/schema.org/PROV triples (58 of them on the demo card). It
deliberately projects the interoperable core; the full profile (starter
queries, hubs, datatype histograms, signals) stays in `--json`, where its
private names are honest.

### Croissant — the honest subset (`--format croissant`)

Croissant models **tables**: `recordSet` → `field` → `dataType`. An RDF graph
has no records, and forcing the card into a record-set shape would produce a
document that validates and misleads. So `--format croissant` maps what
genuinely corresponds — the descriptive header, licence, version, creators and
publisher, provenance, and the `.rete` file itself as a `cr:FileObject`
distribution — and **carries no `recordSet` at all**. (Where a dataset ships
tabular Parquet companions, *those* are the honest record-set material, with
their own Croissant documents beside the bucket.)

One Croissant requirement is structurally unsatisfiable from inside the file:
every `FileObject` must carry an `md5`/`sha256`, and **a file cannot contain
its own whole-file sha256** (the hash would change the bytes being hashed).
The format's own integrity hash (blake3-16 over the payload sections) is
published as `rete:contentHash`; the sha256 — knowable only outside the file —
can be supplied by the publisher with `--sha256 <hex>`, which makes the
document fully validator-clean: `mlcroissant` reports **zero errors** with it,
and exactly that one missing-property error without it.

```sh
rete card data.rete --format jsonld                      # VoID + schema.org + PROV
rete card data.rete --format croissant --sha256 $(sha256sum data.rete | cut -d' ' -f1)
```

## Back-compatibility

All enriched fields are additive with serde defaults, so a card written by an
older `rete` (plain `example_queries`, none of the new fields) still
deserializes. Because the new fields change the card JSON, they change the
`blake3` content hash of every **card-bearing** file — cardless builds are
unaffected and remain byte-identical to a pre-card build.

Compatibility holds in both directions, verified against real artifacts: the
new reader parses the published NKOD and Hugging Face cards unchanged (2 range
requests each), and a pre-build-info binary run against a new card-bearing
file **verifies it** (the hash never covered the new section), renders its
card (unknown JSON fields are ignored by serde), and queries it (the unknown
kind-7 directory entry is preserved and skipped by the section-directory
contract the header has always had).

## Reading a card

`rete card` prints a human-readable catalog; `--json` emits the raw card:

```sh
rete card data.rete
#  Dataset Card
#    title        : Citation graph 2021
#    license      : CC0-1.0
#    source       : https://example.org/citations
#    triples      : 12048
#    terms        : 9571
#    checksum     : 8f5b97374ac5f5e324b5cc53f592e96c  (blake3-16 content hash)
#    vocabularies : 2
#        http://purl.org/spar/cito/
#        http://purl.org/dc/terms/
#    predicates (2):
#           11020  <http://purl.org/spar/cito/cites>
#            1028  <http://purl.org/dc/terms/date>
#    ...

rete card data.rete --json     # the embedded JSON, pretty-printed
```

`rete info` prints the file header and, when a card is present, appends the same
catalog — so `info` is a one-shot overview of *what the file is* as well as *how
it's laid out*. A file with no card just shows the header.

The `checksum` line is the file's `blake3` content hash (the same value
`rete verify` checks) — it identifies the exact bytes, card included.

## How it sits in the file

The card is stored as compact **JSON** in the metadata section, which sits
between the header and the dictionary:

```text
[0..1024)     header              (metadata_offset = 1024, metadata_len = card bytes)
[1024 .. 1024+L) Dataset Card JSON  (L = metadata_len; absent when no card)
[.. +B]         build-info JSON     (B = build_info_len; adjacent, OUTSIDE the hash)
[dictionary]    front-coded terms   (shifts to offset 1024 + L + B)
[index]         permutation blocks
[pyramid-meta]  community summary
[named graphs]  (if any)
[footer]        trailing magic
```

A few properties worth knowing:

- **No format break.** Without a card, `metadata_len` is `0` and the dictionary
  starts right after the 1 KB header exactly as before — the output is
  byte-identical to a cardless build, and existing files keep verifying unchanged.
- **Integrity-covered.** The card bytes are part of the `blake3` content hash, so
  `rete verify` validates the card too and any edit to it is detected.
- **Off the query path.** Range-reading opens (`rete query-url`, `sparql-url`,
  `summary-url`, federation routing) fetch sections by offset and **never read
  the card**, so embedding one adds nothing to query-time bytes-on-the-wire. To
  read the card remotely without downloading the file, `rete card-url` fetches
  just the header + metadata range (two ranges, index untouched).
- **Opaque to the engine.** `rete-core` treats the section as raw bytes; the card
  schema lives entirely in the CLI. The metadata section is a general extension
  point — a card is just its first use.

See [the format specification](SPEC.md) for the header layout and
[the CLI reference](cli.md) for every flag.
