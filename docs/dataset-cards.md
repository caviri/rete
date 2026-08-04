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

For the curated fields — including the **identity and provenance** fields and
a list of **example queries**, none of which have flags — supply a small JSON
file with `--card-file`. Explicit flags override whatever the file provides:

```json
{
  "title": "Citation graph 2021",
  "license": "CC0-1.0",
  "source": "https://example.org/citations",
  "description": "Open citations sharded by year",
  "version": "2021.2",
  "creators": [
    { "name": "Ada Lovelace", "orcid": "https://orcid.org/0000-0002-1825-0097" }
  ],
  "publisher": { "name": "EPFL", "ror": "https://ror.org/02s376052" },
  "canonical_url": "https://data.example.org/citations-2021.rete",
  "sparql_endpoint": "https://example.org/sparql",
  "source_date": "2021-12-31",
  "derived_from": ["https://example.org/dumps/citations-2021.nt"],
  "doi": "https://doi.org/10.5281/zenodo.0000000",
  "cite_as": "Lovelace, A. (2021). Citation graph 2021.",
  "keywords": ["citations", "open science", "scholarly communication"],
  "theme": ["http://publications.europa.eu/resource/authority/data-theme/TECH"],
  "example_queries": [
    "SELECT ?citing WHERE { ?citing <http://purl.org/spar/cito/cites> ?cited }"
  ],
  "extra": {
    "atlas:region": "CH",
    "internal_id": "DS-2021-042",
    "review": { "status": "approved", "by": "data-team" }
  }
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
| `version`, `doi`, `cite_as` | **curated** (`--card-file` only) | The publisher's **dataset version** (a date or semver — Croissant requires one; distinct from `format_version` and from the build-info `builder`, see [One card, three versions](#one-card-three-versions)), DOI IRI, preferred citation. |
| `creators`, `publisher` | **curated** (`--card-file` only) | People (`{name, orcid}`) and organisation (`{name, ror}`). ORCID/ROR as **IRIs, not strings** — this project publishes both authority graphs, so "which datasets did this person build?" becomes a federated join, not a text search. |
| `canonical_url`, `sparql_endpoint` | **curated** (`--card-file` only) | Where the authoritative copy of this file lives (`void:dataDump`) and a public endpoint (`void:sparqlEndpoint`). A `.rete` found on a disk can then say where to verify against. |
| `source_date`, `derived_from` | **curated** (`--card-file` only) | The source data's own snapshot date (distinct from `created` and from the build timestamp), and what this file was derived from (`prov:wasDerivedFrom`) — dumps, endpoints, or the shards a `rete merge` folded in. |
| `keywords` | **curated** (`--card-file` only) | Free-text tags (`dcat:keyword` / `schema:keywords` in the projections). Canonicalized at build time — trimmed, sorted, de-duplicated (`dcat:keyword` is an unordered repeated property, so sorting loses nothing and keeps the content hash independent of authoring order); a keyword that is empty after trimming fails the build. |
| `theme` | **curated** (`--card-file` only) | **IRIs into a controlled vocabulary** (`dcat:theme`), e.g. the [EU data themes](http://publications.europa.eu/resource/authority/data-theme). IRIs are required — a free-text theme is a keyword by another name and is rejected with a pointer at `keywords`; the agreed concept scheme is the whole value `dcat:theme` adds. Canonicalized like `keywords`. (There is deliberately **no curated language field**: in RDF the language rides on every literal, so the card *measures* it — see the `languages` row below and [the rule](#first-class-field-or-the-bag-the-rule).) |
| `example_queries` | **curated** (`--card-file` only) | Sample queries a consumer can run. |
| `extra` | **curated** (`--card-file` only) | Publisher-defined **custom fields** — one bounded bag, see [Custom fields](#custom-fields-the-extra-bag). |
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
| `format_version` | derived | The `.rete` **spec version** the card was written against — the format's version, not the dataset's. |

The per-predicate and per-class statistics are computed over the **default
graph** (named-graph contents are summarized only by `quad_count` /
`named_graph_count`), matching `rete stats` and `rete predicates`. Every derived
list is **capped** and **deterministically ordered** (count-descending, ties
broken lexically), so building the same input twice yields a **byte-identical**
card — the card folds into a reproducible content hash. Counts are over the raw
(pre-dedup) multiset, matching `rete progressive`.

### One card, three versions

A `.rete` file carries three different versions, each with its own owner. They
answer different questions and are never merged:

| field | owner | answers | example |
|-------|-------|---------|---------|
| `version` (card, curated) | the **publisher** | which release of the *data* is this? | `2021.2` |
| `format_version` (card, derived) | the **spec** | which `.rete` format does the file conform to? | `5` |
| `builder` (build-info) | the **tool** | which binary wrote the file? | `rete-cli 0.3.2` |

`version` is yours: set it in the card file, bump it when the dataset changes.
`format_version` is stamped by the builder and changes only when the format
does. `builder` lives in the unhashed build-info section because it is a fact
about one build, not about the data.

## Custom fields: the `extra` bag

A publisher can put **their own fields** in a card — internal identifiers,
review status, project tags, anything the official fields don't cover — under
one reserved key, `extra`, in the `--card-file` document:

```json
{
  "title": "Citation graph 2021",
  "extra": {
    "atlas:region": "CH",
    "internal_id": "DS-2021-042",
    "review": { "status": "approved", "by": "data-team" }
  }
}
```

`rete card --json` returns the bag verbatim; the text catalog lists it under
`custom fields`. There is deliberately **no flag** for them: arbitrary
key/value pairs on a command line are a shell-quoting trap with no schema to
validate against — they come from the card file only. (In Python,
`Builder.card()` routes any keyword that is not a rete-defined field into the
bag, and `extra={...}` merges a whole dict — the client writes the card it is
given; the caps below are enforced by `rete build`.)

### First-class field or the bag? The rule

Before putting anything in `extra`, ask two questions:

**1. Does a standard vocabulary already have a term for it?** If not — the
meaning is yours, not agreed — it belongs in `extra`. The bag deliberately
strips meaning: everything in it projects as `rete:extra/<key>`, an opaque
value whose semantics are private to its publisher, and for genuinely
private fields (internal identifiers, review status, pipeline tags) that
opacity is *honest*: the values travel, the meaning stays with you.
Conversely, storing a standard-termed fact in the bag publishes it *as
though it meant nothing* — a consumer that speaks the standard term (a DCAT
harvester, a dataset-search crawler) will never find it. That is strictly
worse than either doing it properly or not at all. If the official field
doesn't exist yet, **ask for it** (file an issue) instead of bagging it for
good.

**2. Can it be derived from the data?** If yes, it is **auto-derived, not
curated** — a curated duplicate of a derivable fact is a second source of
truth that can silently drift from the first, and a reader has no way to
tell which one is right. The worked example is **language**: it looks like
an obvious curated field (DCAT catalogs use `dct:language`) right up until
you notice that in RDF the language already rides on every literal
(`@lang` / `rdf:langString`). So the card *measures* it — the `languages`
histogram and `signals.default_lang`, projected as `schema:inLanguage` —
and there is deliberately **no curated language field** to contradict the
measurement.

`keywords` and `theme` passed both questions — `dcat:keyword` /
`schema:keywords` and `dcat:theme` already said what they mean, and neither
free-text tags nor controlled-vocabulary themes can be computed from the
triples. Each promotion must also state what its field means beyond "a term
exists": `theme` is only worth a field distinct from `keywords` because it
is **required to be an IRI into a controlled vocabulary** — a free-text
theme is a keyword by another name, and the build rejects it as such. A
candidate that cannot say more than "like keywords, but mine" hasn't
passed.

The enforcement mirrors the rule: the top level rejects unknown keys, so a
private field can never sit where official semantics are expected — and an
official field is only ever added deliberately, at the top level, never by a
bag key drifting into common use.

One candidate remains weighed and deliberately in waiting — by this rule a
future official field, not bag material, the day a publisher actually needs
it: curated spatial/temporal coverage (`dct:spatial` / `dct:temporal`). It
passes the second question only in the case its derived counterparts cannot
reach — `spatial_bbox` / `temporal_extent` are computed wherever the data
carries geometry or time, so a curated value earns its keep solely where
coverage is known but *not encoded in the data*. If you need it today, the
honest stopgap is the bag *plus the issue*: the value travels verbatim in
`--json`, and the projection stays truthfully opaque until the field gets
its term.

### Why one bag, not prefixed keys

The failure mode worth designing against is a **future collision**: rete adds
an official card field, and some published card already uses that name for
something else. With prefixed top-level keys (`x-…`) custom and official
fields share a namespace and the guarantee is a naming promise; with a nested
bag it is **structural** — official fields live at the top level, publisher
fields live inside `extra`, and the two can never meet. And it is enforced,
not hoped: the card file's **top level rejects unknown keys loudly** (a stray
key is usually a typo — or a custom field that belongs in the bag), so a
publisher's field cannot even be written where a future official field could
capture it. A consumer that doesn't care skips one key.

One key inside the bag is reserved: `@context`, held back for a future
author-supplied JSON-LD mapping (see the projection below). Everything else in
`extra` is, by construction, not rete's.

### What custom fields actually cost — and the limits

Custom fields never touch the header: the header is a **fixed 1 KiB of
offsets**, and no card content of any size can disturb it. What they grow is
the **metadata section** — and that section is fetched by every CARD-tier
reader on **every open**, across the whole catalog, forever. That is the real
constraint, and it is tighter than "don't disturb the header". So the bag is
bounded, and the bounds are checked at build time:

| limit | value | why |
|-------|-------|-----|
| serialized size (whole bag, compact JSON) | **8,192 B** | Exceeds the smallest whole published card (NKOD, 6,649 B) — generous for *metadata*. The worst realistic case — the largest published card (Hugging Face, 53,580 B) + a maxed bag + its ~1 KB build info ≈ 62.8 KB — still travels in the same single coalesced range. |
| keys | **64** | A key costs ≥ 8 serialized bytes; needing more fields than this means the bag is being used as a data store, and the graph is the data store. |
| key length | **128 bytes** | Keys are identifiers, not values. |
| nesting depth | **2** | An object of objects-of-scalars, no deeper. Deep structures invite storing *records* in the card — Parquet companions exist for records. |

The CARD tier's **request count cannot change**: `rete card-url` still costs
the 1 KiB header plus one coalesced metadata+build-info range, bag or no bag —
pinned by test. The one cost worth knowing: on the smallest cards a maxed-out
bag can push the coalesced range past a conservative TCP initial window
(~14.6 KB), i.e. one extra round trip — never an extra request.

### Overflow: the build fails

A bag over any limit **rejects the build** with the limit and the actual size
in the error — it is never truncated. Truncation would ship a card that says
something different from what its author wrote, with no way to know which
fields vanished; and since the bag folds into the content hash, "what I wrote"
and "what hashed" would diverge invisibly. The derived profile lists *are*
capped (with `truncated` set) because they can be re-derived; curated input
cannot — only its author knows what to cut, and at build time the fix costs a
minute.

### Deterministic, hashed, tamper-evident

The bag is curated input, so it lives **inside** the blake3 content hash like
every other curated field — `rete verify` covers it, and any edit is detected.
That placement is safe because its serialization is deterministic: keys are
sorted at every nesting level when the card is written, so two builds of
identical input remain **byte-identical** and hash equal (pinned by test),
even when the card file's keys are authored in a different order. The
consequence: per-build volatile facts (timestamps, CI run ids) do **not**
belong in `extra` — they would make every build hash differently, and the
unhashed build-info section already records exactly those facts.

### How the projections treat custom fields

A custom field has no vocabulary term, so projecting it as ordinary RDF would
invent semantics. The projections keep the fields without pretending to
understand them:

- **`--format jsonld`** emits each field under `rete:extra/<key>` (the key
  percent-encoded where needed, so the IRI is always valid): a scalar becomes
  a plain literal, an object or array becomes a JSON literal (typed `@json`,
  i.e. `rdf:JSON`) rather than a blank-node structure pretending to be
  modelled data. A consumer of the JSON-LD **gets the values, not their
  meaning**: `rete:extra/review` says "the publisher-defined field named
  `review` of this card" and nothing more — two publishers using the same key
  name share an IRI without sharing semantics. Nothing is dropped (diffing
  the card against its projection finds every field) and nothing is invented.
  If a card's `extra` one day carries an author-supplied `@context`, the
  projection can start honouring it — that key is reserved today so honouring
  it later breaks nothing.
- **`--format croissant`** omits them entirely — Croissant is already the
  honestly-mappable *subset* (no `recordSet`, no partitions), for ML loaders
  that would do nothing with opaque publisher keys.
- **`--json`** always carries the bag verbatim.

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
the generated library, unchanged. Clients surface both: the playground merges
a loaded file's card queries into its examples panel next to any curated
catalog examples (deduplicated against them), so even an off-catalog `.rete`
opened by URL offers its own starter questions.

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
| `keywords` | `schema:keywords` **and** `dcat:keyword` — one list under both standard names, the same dual-vocabulary stance as the `schema:Dataset` + `void:Dataset` typing |
| `theme` | `dcat:theme`, `@id`-typed (the values are controlled-vocabulary IRIs) — exactly one standard term, so no schema.org double |
| `source`, `derived_from` | `prov:wasDerivedFrom` (URLs) / `dct:source` (free text) |
| build info | `prov:wasGeneratedBy` — a `prov:Activity` with `prov:endedAtTime` and the builder as a `schema:SoftwareApplication` agent |
| temporal/spatial signals | `schema:temporalCoverage` / `schema:spatialCoverage` (GeoShape box) |
| `triple_count`, `term_count`, `named_graph_count`, the content hash | a small `rete:` namespace (`https://w3id.org/rete/card#`) — what no standard covers, rather than a bent term |
| `extra` (custom fields) | per-key `rete:extra/<key>` — **opaque values, not vocabulary**: scalars as plain literals, containers as `rdf:JSON` JSON literals (see [Custom fields](#custom-fields-the-extra-bag)) |

The projection is validated JSON-LD: it expands under `pyld` and yields the
intended VoID/schema.org/PROV triples (58 of them on the demo card). It
deliberately projects the interoperable core; the full profile (starter
queries, hubs, datatype histograms, signals) stays in `--json`, where its
private names are honest.

### Croissant — the honest subset (`--format croissant`)

Croissant models **tables**: `recordSet` → `field` → `dataType`. An RDF graph
has no records, and forcing the card into a record-set shape would produce a
document that validates and misleads. So `--format croissant` maps what
genuinely corresponds — the descriptive header, licence, version, keywords,
creators and publisher, provenance, and the `.rete` file itself as a
`cr:FileObject` distribution — and **carries no `recordSet` at all**.
(`theme` is not carried: its only term is DCAT's, and bending it into a
schema.org shape here would be the same dishonesty as a fabricated
`recordSet`.) (Where a dataset ships
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
