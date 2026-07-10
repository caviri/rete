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
| `format_version` | derived | The `.rete` format version the card was written against. |

The per-predicate and per-class statistics are computed over the **default
graph** (named-graph contents are summarized only by `quad_count` /
`named_graph_count`), matching `rete stats` and `rete predicates`. Every derived
list is **capped** and **deterministically ordered** (count-descending, ties
broken lexically), so building the same input twice yields a **byte-identical**
card — the card folds into a reproducible content hash. Counts are over the raw
(pre-dedup) multiset, matching `rete progressive`.

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
requests — header + metadata — and **never touches the index**, so a remote/S3
client gets the whole self-description (counts, vocabulary, class graph, signals,
starter queries) without downloading the file.

## The starter-query library

`queries` is a vetted set of starter SPARQL queries, **auto-instantiated at
build** with the dataset's own vocabulary (`{{TOP_CLASS}}` → the most populous
class, `{{LABEL_PRED}}` → the detected label predicate, and so on) and emitted
**only when the required signal is present** — no geometry query without
geometry, no time query without a time predicate — so the shipped set is
guaranteed to return rows. Each query carries:

- a full **PREFIX block** (the engine injects none, so every query is runnable as-is);
- a `dimension` (overview / identity / labels / types / topology / links / literals / time / space / graphs);
- a `tier` tag (`card` / `summary` / `index`) — the cheapest tier that answers it;
- the `requires` capability keys that gated its emission.

A publisher's own `example_queries` (curated, `--card-file`) are kept alongside
the generated library, unchanged.

## Back-compatibility

All enriched fields are additive with serde defaults, so a card written by an
older `rete` (plain `example_queries`, none of the new fields) still
deserializes. Because the new fields change the card JSON, they change the
`blake3` content hash of every **card-bearing** file — cardless builds are
unaffected and remain byte-identical to a pre-card build.

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
[dictionary]    front-coded terms   (shifts to offset 1024 + L)
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
