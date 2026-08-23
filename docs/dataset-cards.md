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

`description` is **Markdown** and may run to several paragraphs with headings
and lists — see [The description](#the-description-markdown-not-html) for what
is supported, why raw HTML is not, and how to write one without hand-escaping
newlines.

## What's in a card

| Field | Source | Meaning |
|-------|--------|---------|
| `title`, `license`, `source`, `created` | **curated** (flags / `--card-file`) | Free-text catalog metadata. Omitted fields are absent from the JSON. |
| `description` | **curated** (flags / `--card-file`) | The dataset's abstract, read as **Markdown** — headings, lists, links, code. Raw HTML is escaped, never rendered. May be written as an array of lines. Capped at 8 KiB. See [The description](#the-description-markdown-not-html). |
| `version`, `doi`, `cite_as` | **curated** (`--card-file` only) | The publisher's **dataset version** (a date or semver — Croissant requires one; distinct from `format_version` and from the build-info `builder`, see [One card, three versions](#one-card-three-versions)), DOI IRI, preferred citation. |
| `creators`, `publisher` | **curated** (`--card-file` only) | People (`{name, orcid}`) and organisation (`{name, ror}`). ORCID/ROR as **IRIs, not strings** — this project publishes both authority graphs, so "which datasets did this person build?" becomes a federated join, not a text search. |
| `canonical_url`, `sparql_endpoint` | **curated** (`--card-file` only) | Where the authoritative copy of this file lives (`void:dataDump`) and a public endpoint (`void:sparqlEndpoint`). A `.rete` found on a disk can then say where to verify against. |
| `source_date`, `derived_from` | **curated** (`--card-file` only) | The source data's own snapshot date (distinct from `created` and from the build timestamp), and what this file was derived from (`prov:wasDerivedFrom`) — dumps, endpoints, or the shards a `rete merge` folded in. |
| `keywords` | **curated** (`--card-file` only) | Free-text tags (`dcat:keyword` / `schema:keywords` in the projections). Canonicalized at build time — trimmed, sorted, de-duplicated (`dcat:keyword` is an unordered repeated property, so sorting loses nothing and keeps the content hash independent of authoring order); a keyword that is empty after trimming fails the build. |
| `theme` | **curated** (`--card-file` only) | **IRIs into a controlled vocabulary** (`dcat:theme`), e.g. the [EU data themes](http://publications.europa.eu/resource/authority/data-theme). IRIs are required — a free-text theme is a keyword by another name and is rejected with a pointer at `keywords`; the agreed concept scheme is the whole value `dcat:theme` adds. [Where to get one](#where-to-get-theme-iris). Canonicalized like `keywords`. (There is deliberately **no curated language field**: in RDF the language rides on every literal, so the card *measures* it — see the `languages` row below and [the rule](#first-class-field-or-the-bag-the-rule).) |
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
| `signals.text_index` | **measured** | Whether the file carries a **full-text (TEXT_INDEX) section** — `{present, bytes, token_table_bytes}`. Derived from the file's *sections* rather than its triples, and never written into the file. See [The full-text signal](#the-full-text-signal-measured-not-stored). |
| `signals.permutations` | **measured** | Which **index permutations** the file stores — `{count, names, merge_join}`. Also derived, from the header's permutation mask, and also never written. See [The permutation signal](#the-permutation-signal-measured-not-stored). |
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

### The full-text signal: measured, not stored

A `.rete` carries an optional **TEXT_INDEX section** (kind `6`, [SPEC §6.3](SPEC.md#63-full-text-index-text_index-section-optional)),
opt-in via `rete build --text-index` / `rete repyramid --text-index`. A file
that has one answers `FILTER(CONTAINS(…))` by word lookup; a file that does not
answers **the same query with the same rows** by full scan. The capability is
therefore invisible from the results — which is exactly how the playground
catalog came to advertise an index for two published datasets whose files never
carried one, and to ship the catalog's largest index (1.88 GB, 29% of
`causenet-full-typed.rete`) without telling anyone.

So the card states it, in both directions:

```json
"signals": { "text_index": { "present": true, "bytes": 1879287762, "token_table_bytes": 193295361 } }
"signals": { "text_index": { "present": false } }
```

(Those are the real figures for the published `causenet-full-typed.rete`, read
by `rete card-url` in **43,671 bytes across 3 range requests** — the 1 KiB
header, the 42,637-byte card, and a 10-byte probe — against a 6.39 GB file.)

| value | means |
|---|---|
| `{"present": true, …}` | measured — the file has a kind-6 section |
| `{"present": false}` | measured — it has none |
| **field absent** | **unknown** — nobody measured (a card read out of a saved JSON document, with no file behind it). Never read this as "no index". |

**It is measured by the reader, not written by the builder.** Every other
derived field is computed once at build time and stored; this one is not stored
at all. Three reasons:

1. **The ground truth is already in the bytes every card read fetches.** The
   section directory lives in the 1 KiB header, and `rete card` parses it for
   the content hash anyway. A stored copy would be a second source of truth for
   a question the first source answers for free — the same reasoning that keeps
   a curated `language` field out of the card (see
   [the rule](#first-class-field-or-the-bag-the-rule)).
2. **A stored flag can outlive the section it describes.** `rete repyramid
   --text-index` rewrites a file's sections; `rete repyramid` without `--card`
   drops the card entirely. A measured signal simply becomes true, and is right
   for a file with no card at all — where `rete card` prints
   `(no dataset card — TEXT_INDEX present — …)` rather than nothing.
3. **No re-card.** Every already-published `.rete` reports it today. A stamped
   field would have reached the catalog only after each file was rebuilt.

**Why byte counts and not a token count.** `bytes` is the whole section — worth
stating because it can dominate a download (1.88 GB, 29% of
`causenet-full-typed.rete`). `token_table_bytes` is its leading token table,
which is what a *first search* actually fetches; the postings blob behind it is
read one posting at a time and never whole. On that same file the two differ by
**9.7×** (1.88 GB against 193 MB), so quoting `bytes` alone would badly
overstate the price of pressing Search — which is why the playground's full-text
panel already shows both. The **number of indexed tokens** is deliberately
absent: it is the first varint of the *decompressed* token table, so reporting
it would mean fetching and inflating all 193 MB — not a card-tier fact.
`token_table_bytes` answers the same question for one ≤10-byte range read.

`rete card-audit` reports the measured signal for any `.rete`, local or remote,
and flags a card whose own bytes disagree with the file's sections. Given a card
*document* it reports `unknown` — there is nothing to measure.

### The permutation signal: measured, not stored

A `.rete` stores its triples in six orders of `(s, p, o)` by default —
`SPO, POS, OSP, SOP, PSO, OPS`. `rete build --permutations 3` keeps only the
first three. The first three decide **routing**: they tie the longest bound
prefix on all eight triple-pattern shapes, so a three-permutation file answers
every query with the same rows, fetched from the same tiles. The other three
exist to hand a **sort-merge join** two streams already sorted on the join key;
without them the planner declines the merge seed and hash-joins instead.

So, exactly like a missing full-text index, the difference **cannot be seen in
any result** — and it is not small: measured on two datasets built both ways,
`SOP + PSO + OPS` are **36.8%** of `davidrumsey` (58.7 MB → 37.1 MB) and
**50.5%** of `tree-city-inventory` (19.4 MB → 9.6 MB).

```json
"signals": { "permutations": { "count": 6, "names": ["SPO","POS","OSP","SOP","PSO","OPS"], "merge_join": true } }
"signals": { "permutations": { "count": 3, "names": ["SPO","POS","OSP"], "merge_join": false } }
```

| value | means |
|---|---|
| `{"count": 6, "merge_join": true, …}` | measured — the file carries the merge-join orders |
| `{"count": 3, "merge_join": false, …}` | measured — it carries only the routing three |
| **field absent** | **unknown** — nobody measured (a card read out of a saved JSON document). Never read this as "six". |

**Measured, never stored**, for the same three reasons as `text_index` — and one
that is sharper here. Which permutations a file carries is a fact about *its own
bytes*, sitting in the 1 KiB header (byte 50; `0` means all six, which is why
every file written before the mask existed reports six today, with no re-card).
A stored copy would be an **authored claim about the file's own layout** — the
single class of statement a file can always check for itself, for free. The
measurement costs **no range read at all**: the header is already in hand.

`names` is written out rather than implied by `count` because the mask is a set,
and a future build may keep a different three.

Beyond the card, `rete info` and `rete stats` report it directly, so a file with
no card still answers the question.

### Where to get `theme` IRIs

`theme` rejects free text, which is only helpful if you know where an IRI
comes from. Pick a **published concept scheme** and copy the concept's IRI —
every example below resolves as printed:

| Vocabulary | Example IRI | Good for |
|---|---|---|
| **EU Data Themes** | `http://publications.europa.eu/resource/authority/data-theme/GOVE` (*Government and public sector*) | The DCAT-AP controlled list — **the default answer** for government and open-data portals. 13 themes, labelled in 27 languages. |
| **EuroVoc** | `http://eurovoc.europa.eu/1460` (*EU financial instrument*) | EU policy and legal subject headings, when the data-theme list is too coarse. |
| **Wikidata** | `https://www.wikidata.org/entity/Q413` (*physics*) | **Anything with no domain vocabulary.** Stable IRIs that are never reassigned, labels in hundreds of languages (282 on that one concept), and cross-links into most of the vocabularies below — so a Wikidata theme stays joinable even when a consumer speaks a different scheme. |
| **LCSH** | `https://id.loc.gov/authorities/subjects/sh85101653` (*Physics*) | Library-style subject headings; what bibliographic consumers already index. |
| **UNESCO Thesaurus** | `http://vocabularies.unesco.org/thesaurus/concept197` (*Environmental management*) | Education, science, culture, communication. |
| **AGROVOC** | `http://aims.fao.org/aos/agrovoc/c_12332` (*maize*) | Agriculture, fisheries, food, environment. |
| **MeSH** | `http://id.nlm.nih.gov/mesh/D009369` (*Neoplasms*) | Biomedical and clinical topics. |
| **OBO Foundry** | `http://purl.obolibrary.org/obo/GO_0008150` (*biological_process*) | Life sciences. The playground catalog already carries graphs built on OBO IRIs (`chebi-full` is the complete ChEBI ontology; `chemotion` merges CHMO and RXNO), so an OBO theme is joinable against a graph you can actually query. |
| **GeoNames** | `https://sws.geonames.org/3077311/` (*Czechia*) | When the dataset's subject is really a **place**. |

**How to choose:**

1. **Prefer the vocabulary your consumers already harvest.** DCAT-AP requires a
   theme from the EU Data Themes list, so a concept from a different scheme —
   however precise — does not satisfy a portal that checks for one. Matching
   your audience beats picking the most exact concept. (A national open-data
   catalog is the clean case: `…/data-theme/GOVE`, plus anything else you like
   alongside it.)
2. **Then the obvious domain vocabulary**, if your dataset has one (AGROVOC for
   a crop census, MeSH for a clinical corpus).
3. **Otherwise Wikidata**, which has a concept for essentially any topic. It is
   the honest fallback, not a defeat.
4. **Use several** when the dataset genuinely spans several — `theme` is a
   **list**, and mixing schemes is fine (one EU data theme *and* a Wikidata
   concept is a common, useful pairing). Entries are sorted and de-duplicated
   at build time.

`theme` is for concepts a scheme has agreed on; everything else you would like
a reader to search by belongs in `keywords`, which takes free text precisely so
you are never tempted to invent an IRI. **Never mint a theme IRI yourself** — a
plausible-looking IRI in a scheme's namespace that resolves to nothing is worse
than no theme at all.

### One card, three versions

A `.rete` file carries three different versions, each with its own owner. They
answer different questions and are never merged:

| field | owner | answers | example |
|-------|-------|---------|---------|
| `version` (card, curated) | the **publisher** | which release of the *data* is this? | `2021.2` |
| `format_version` (card, derived) | the **spec** | which `.rete` format does the file conform to? | `6` (ordinary paired build), `5` (external transition) |
| `builder` (build-info) | the **tool** | which binary wrote the file? | `rete-cli 0.3.2` |

`version` is yours: set it in the card file, bump it when the dataset changes.
`format_version` is stamped from the physical generation actually written; it
is `6` for an ordinary paired build and remains `5` for the external and
three-permutation paths during the transition. `builder` lives in the unhashed build-info section because it is a fact
about one build, not about the data.

## The description: Markdown, not HTML

`description` is the one curated field long enough to want structure, so it is
read as **Markdown**. The card viewer renders headings, lists, links and code;
every other surface reduces the same text to one readable line.

```json
{
  "description": [
    "The complete Wikidata class ontology — no instances.",
    "",
    "## What's inside",
    "",
    "- **4,420,121 classes** — every `wdt:P279` subject or object",
    "- the **5.1M-edge** subclass hierarchy",
    "- per-class direct-instance counts",
    "",
    "See [the build notes](https://example.org/notes) for how it was derived."
  ]
}
```

### Supported

| Construct | Written as |
|---|---|
| Headings | `# One` … `###### Six` — rendered **shifted down**, so a card never emits an `<h1>`; the viewer's own title owns that level |
| Bulleted lists | `- item`, `* item`, `+ item` — **nested by indentation** |
| Numbered lists | `1. item`, `1) item` — nested the same way |
| Block quotes | `> quoted` (every line carries the `>`; a quote's body is Markdown too) |
| Horizontal rules | `---`, `***`, `___` |
| Fenced code | ```` ``` ```` … ```` ``` ```` |
| Links | `[text](https://example.org)` — `http(s)` and `mailto:` only |
| Emphasis / code | `**bold**`, `*italic*`, `` `code` `` |

Deliberately **not** supported: tables (a card's tabular data belongs in the
graph — and the viewer already draws its own tables from the derived profile),
reference-style links, images, and footnotes. Every construct is more parser
for less description.

### Raw HTML is not supported — and that is the point

HTML in a description is **escaped, not rendered**: `<b>x</b>` shows up as the
five characters `<b>x</b>`, and a `<script>` shows up as text.

This is a deliberate refusal, not a missing feature. A card is **third-party
data** — it arrives inside a file that someone else published, on a page that
also holds the reader's own local files. Honouring raw HTML would mean any
publisher could put a `<script>` (or an `onerror=` on an `<img>`) into a
description and have it execute in every reader's browser, on every open,
simply because the reader clicked 🏷 Card. That is not a formatting feature; it
is remote code execution with extra steps.

Markdown gives the headings, bullets and links with none of that: the renderer
escapes every character of the source, and the only tags in the output are the
ones **it** chose. `javascript:` links degrade to visible text. The playground's
gate pins both halves — that the formatting appears, and that a description
carrying a `<script>`, an `<img onerror=>` and a `javascript:` link creates no
element and executes nothing.

The same reasoning already governs the card's JSON view, which tokenizes the
raw bytes and escapes every token precisely so a `<` inside a description
cannot become an element.

### Writing a multi-line description

A JSON string can only carry line breaks as `\n` escapes, which is miserable to
write by hand and worse to review. Three ways out, in the order they are worth
reaching for:

1. **An array of lines in `--card-file`** (shown above). `rete build` joins them
   with newlines. It is input sugar only — the card always stores one string, so
   `rete card --json` output feeds straight back into `--card-file`.
2. **The shell, for `--description`**: `--description "$(cat description.md)"`
   keeps the Markdown in its own file, where an editor can help.
3. **The playground's Build panel**: the *Description* box is a textarea, so
   Markdown typed into it works as typed — the JSON mirror beside it inserts the
   `\n` escapes for you. The JSON editor accepts the array shape too.

A literal `"description": "line one\nline two"` is of course still valid, and
still what the file stores.

### How long may it be?

Up to **8 KiB** of UTF-8 — the same budget as the [`extra` bag](#custom-fields-the-extra-bag),
for the same reason: both ride in the metadata section that every CARD-tier
reader fetches on every open. For scale, the longest description on the
published catalog is 813 bytes; 8 KiB is roughly 1,300 words of Markdown.

Over the cap, the **build fails loudly** rather than truncating — the text is
authored, it folds into the content hash, and a silent cut would ship a card
that no longer says what the publisher wrote. A description is the dataset's
**abstract**, not its documentation: link the long form from `source` /
`canonical_url`, or put it in the graph.

Readers never validate any of this. A card written oversized by some other tool
still opens — the cap is a write-time rule, like every other card rule.

### Where a description is shown

Only the **🏷 Card viewer** renders blocks; it is a scrollable panel with room
for them. Every other surface has room for one line, so it shows the same text
with its block markers removed (`## Heading` becomes `Heading`, `- item`
becomes `• item`) rather than leaking raw markers into a paragraph:

| Surface | Shows |
|---|---|
| 🏷 Card modal | the full Markdown, rendered |
| Dataset sidebar / picker blurb | one flattened stream, inline markup kept |
| Dataset header tagline | the first sentence, plain text |
| Plaza gallery tile and hero | flattened to plain text |
| `rete card` (terminal) | the source as written, indented to the value column |
| `--format jsonld` / `croissant` | the source as written (both formats take Markdown in `description`) |

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
- `dropped_queries` — starter queries this build generated, **ran**, and then
  refused to ship (below). Absent on a healthy build.

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

The memory-bounded external build and `rete merge` record
timestamp/builder/params but no costs (their cards carry no derived starter
queries to measure).

### A build does not ship a query it just measured at zero rows

The run above is not only a costing. It is the moment a carded build stops
*reasoning* about whether its starter queries answer and simply **observes** it
— and where an observation exists it is ground truth, so it wins over every
static rule that guessed. A query whose run comes back worthless is therefore
removed from the card before the file is written, and the fact is recorded in
`dropped_queries` (`id`, `why`, and a `contradicts_claim` flag). Two shapes are
caught:

- **zero rows** (or a false `ASK`, or nothing constructed) — the failure the
  query library exists to prevent: *a starter query that answers nothing is
  worse than no starter query, because the reader concludes the file is
  broken*;
- **a row that binds no variable at all** — the un-grouped aggregate over an
  empty solution sequence. SPARQL returns exactly one row there no matter what,
  so no row count can catch it, and the row carries no information: `sp-bbox`
  on a file where no single subject holds both `wgs:lat` and `wgs:long` returns
  one row of four unbound variables. That template's own note says the card
  "cannot do better" than shipping it. The measurement can.

**Dropped, not fatal.** Refusing to build is the right answer for *authored*
content — an oversized `extra` bag is the publisher's text, and quietly
rewriting it destroys an intent only they can restore. A generated starter
query has no author: the generator wrote it moments earlier out of this
dataset's own profile, and the build is the only party able to act. Failing
would make a file unbuildable for a reason its publisher cannot fix, at the end
of a build that may have taken hours, over a metadata nicety — and would push
them onto `--no-card-costs`, which switches off the measurement rather than the
problem. The generator already *drops* (rather than fails) when its static
`provably_empty` hook fires; measurement is a better oracle for the same
question and earns the same consequence. Loudness is bought without fatality:
every drop is printed with its reason, and the build record keeps it after the
terminal is gone.

**What this does to the hash.** The card is inside the content hash — dropping
a query is a change to what the file says about itself, so it is a new content
hash, correctly. Two builds of the same input still hash identically (the
measurement is deterministic), and a build with nothing to drop is byte-identical
to one that never measured at all.

**Measurement versus the static machinery.** They are not redundant. The
generator's `NonEmpty` claims and `provably_empty` hooks act at *generation*
time, before any measurement exists, and they carry every card built without a
build record (external builds, `merge`, and every file published before build
records). Where both speak, the measurement wins. Where they *disagree* — a
template that declares its query cannot come back empty, and then it does —
that is a defect in the rule, not a fact about the dataset: the build says so
in as many words and sets `contradicts_claim` on the record so it is findable
in a published file years later. A template that admitted up front it could not
decide (`NonEmpty::Undecidable`, e.g. `top-dangling`, `sp-within`) sets no such
flag: a measured zero there is news, not a broken promise.

**What measurement cannot catch.** A vacuous `COUNT` binds — to `0`.
`cmp-coverage` returning `total = 76990, have = 0` is one row of two bound
variables: useless, and invisible to any rows-based gate. That class is closed
by *derivation*, not measurement — the query is instantiated with
`{{LABELED_CLASS}}`, the most populous class a `class_links` row proves carries
the label predicate, so `have > 0` holds by construction. Deciding which
binding of a query is the payload is a per-template judgement a build cannot
make; picking the terms so the payload cannot be zero is a judgement the
generator can.

**`--no-card-costs` opts out of this too.** The flag exists to skip running the
starter queries on a graph where that is expensive — and the run is what proves
they answer, so skipping it leaves the card with whatever static reasoning
produced, unchecked. The build says so on stderr. Note the consequence for
reproducibility: the flag used to be hash-neutral, and on a dataset that *has* a
useless starter query it no longer is (the measured build ships a smaller card).
On every dataset whose queries all answer, it remains hash-neutral.

### Measuring a file that already exists

Almost nothing published carries this record. A survey of 110 cards found
**one** file with a build record — every other `query_costs` is missing, so the
field that would say "this starter query returns N rows and costs M bytes in K
range requests" is absent exactly where a reader would want it.

`rete card-audit <path|url> --measure` fills that gap without rebuilding
anything: it runs the starter queries the card already ships and reports the
same three numbers, through the **same** `measure_query` the build uses. That
sharing is the point — a second measurement loop would drift, and then a
re-measured figure could not be compared against a recorded one, which is the
only thing anyone wants to do with it. Where a file *does* carry a record, the
command checks itself against it and says whether they agree.

Two consequences of the numbers being portable:

- a **local** measurement and a **remote** one produce the same `bytes` and
  `requests` (no block cache is in the stack, so the range sequence depends on
  layout and query, not on transport) — only the remote one actually pays. The
  output names which it was regardless;
- the measurement can be **written back** (`--write-costs`). Build info is
  outside the content hash, so the file keeps its identity — same checksum,
  same `rete verify`, byte-identical N-Quads — but the section is near the
  front, so the file is rewritten end to end to make room. The rewrite itself
  is a bounded-buffer copy; the RAM goes into *running the queries* (eager
  evaluation, so a 497,905-row starter query materializes 497,905 rows).
  `switzerland-fedlex` measured and rewrote in 381 s at a 14.2 GiB peak,
  against ≈36 GiB for a `repyramid` re-card of the same file. See
  `rete card-audit` in the [CLI reference](cli.md).

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

### Presence is not co-occurrence

"The required signal is present" is not enough on its own, and getting that
wrong is how a starter query comes back empty. `{{TOP_CLASS}}` is the class with
the most instances; `{{LABEL_PRED}}` is the most-used labelling predicate. Both
are certainly *in* the graph — and instances of that class need never carry that
predicate. On `mtg` the top class is `mtg:Ruling` (76,990 instances, no
`schema:name`); on `hugging-face` it is `hf:Model` while `rdfs:label` appears
only on the embedded ontology terms. Joining the two maxima produced a query
that could not match a single statement, on plain default-graph files.

So a body may conjoin substituted vocabulary **only where the card proves the
pieces meet**, and `class_links` — the `(s_class, predicate, o_class, count)`
quotient — is the proof. The capabilities that carry a witness are:

| key | what it resolves to | witness |
|---|---|---|
| `LABELED_CLASS` | the most populous class that carries `LABEL_PRED` | a `class_links` row for that class *and* predicate |
| `OBJECT_PRED` | the most frequent relation whose object is not a literal — one a path query can walk | a `class_links` row whose `o_class` is not `(literal)` |
| `WKT_PATH` | how this dataset hangs a geometry off a subject (`geo:asWKT`, or `geo:hasGeometry?/geo:asWKT`) | the predicates the card recorded |
| `EXTERNAL_IRI` | that some recorded IRI lies outside the base IRI | `classes` / `in_hubs` |

`LABELED_CLASS` equals `TOP_CLASS` on every dataset whose top class *is*
labelled, so the common case is unchanged. Where the card cannot prove a
witness — `class_links` is capped at `top_n` rows, and labels may sit on untyped
subjects — the label query falls back to the class-free
`SELECT ?s ?label WHERE { ?s <label> ?label }` rather than disappearing.

Three templates are honest about not being decidable from the card, and may
return zero rows as a fact about the data rather than a defect:
`top-dangling` (a fully-described graph has no dangling IRI), `sp-within` (the
box comes from `wgs84` literals, not from the WKT geometries the query reads)
and `top-in-hubs` under `GRAPH` scope (the card profiles the default graph). The
reason is recorded in the source beside each body.

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
starter queries until its publisher re-embeds a card. (That is the named-graph
instance of a general problem — [Maintaining a published card](#maintaining-a-published-card)
below is how you find every affected file and pick a path that fits its size.)
Fixing it does **not** require going back to the source RDF — `rete repyramid`
reads every statement (default graph and named graphs) straight out of the
existing file and re-assembles it, deriving a fresh card on the way:

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

## Maintaining a published card

A card is written **once, at build time**, and never again. Everything in this
section follows from that one fact, and most confusion about cards dissolves
once it is internalised: **fixing the card generator today does not fix a
published file.** It fixes the *next* build. A `.rete` already sitting in a
bucket keeps the card its builder could write on the day it was built —
starter queries, profile caps, missing build record and all — until somebody
deliberately re-cards it. And because the card is inside the `blake3` content
hash there is no in-place patch: either the file is rewritten, or nothing
changes.

So the work splits in two: **find out which files need it** (cheap, and there
is a tool for exactly that), then **pick a command that can actually do it at
that file's size** (not cheap, and the ceiling is real).

### How a card goes stale

| symptom | what a reader loses | how bad |
|---|---|---|
| **no build record** | `built_at`, `builder`, `params`, and the per-query `rows`/`bytes`/`requests` a client would use to budget a query before running it | invisible until someone asks |
| **a dated profile** | a `top_n` cap that predates the field, no `ov-one-row` smoke query, derived lists missing later additions | cosmetic |
| **starter queries that return zero rows** | the file's entire first impression | reads as a broken file |

Only the third is a defect, and it has known causes — each already corrected in
the generator, each still baked into every file built before the correction:
default-graph bodies on a [named-graph dataset](#named-graph-datasets), and
[a join the card never proved co-occurs](#presence-is-not-co-occurrence). A
carded build also measures every query it generates, so a current `rete` will
not ship one it just watched return nothing; older files had no such check.

### Survey first — the survey is free, the re-card is not

Re-carding is a bandwidth cost, not a CPU one: the file is rewritten, so a
remote dataset has to come down in full and go back up in full. The published
catalog is **248 GB across 98 files** (22 of them ≥ 1 GB), which makes
"re-card everything" 248 GB each way. Finding out which files actually need it
costs **0.6 MB** — the same two range requests `rete card-url` makes, per file,
and never the index:

```sh
bash scripts/recard/survey.sh                                    # whole catalog
bash scripts/recard/survey.sh --keys "lombardi orcid switzerland-fedlex"
```

It writes a TSV, a JSON, and a `todo.txt` of every key that is not `CURRENT`,
worst first. The verdicts run `CARDLESS` → `ZERO-ROWS` → `EMPTY-QUERY` →
`MIXED-HIDDEN` → `SUSPECT-QUERY` → `DATED` → `CURRENT`; the full table and the
scripts are in [`scripts/recard/`](https://github.com/caviri/rete/tree/main/scripts/recard).
The first full run (2026-08-05, 98 datasets) returned 14 `CARDLESS`, **one**
`ZERO-ROWS`, 83 `DATED` and zero `CURRENT` — so the alarming case is rare and
specific, and almost everything else is a metadata refresh rather than a fire.

Two things the survey will not do for you. It re-decides for free from cards
already on disk (`--cards <dir>`), so iterate there rather than re-fetching.
And one file is not cheap: `geoadmin-tiles` carries an embedded PMTiles section
*ahead* of its metadata, so its coalesced card range is 117 MB rather than
6 KB — survey it last, or not at all.

### Deciding one file: `rete card-audit`

Where the survey classifies a catalog, [`rete card-audit`](cli.md) explains one
file — one row per starter query the card already ships, decided from the same
card bytes, using the query generator's own judgement rather than a second copy
of it:

```sh
rete card-audit https://data.graphplaza.com/hugging-face/hugging-face.rete
```

The verdict worth acting on is `empty`: the card *refuting* a query it ships.
`suspect` and `undecidable` are the honest middle.

**The static pass has a ceiling, and it is structural.** Nothing in a card ties
a specific subject to a specific predicate, and nothing records which objects
are also subjects — so `top-reach` and `top-dangling` cannot be decided from a
card at all, no matter how good the reasoning gets. In the 2026-08-05 audit
(110 files, 96 of them carrying starter queries) that left `top-reach`
undecided on **79** files and `top-dangling` on **80**. The way to settle one
is to run it, which is what `rete card-audit --measure` does — reporting rows,
bytes and range requests beside the card's verdict, never merged with it. On
the published `lombardi.rete`, both undecidables come back `answers` (100 rows
and 1 row); the whole 22-query audit reads 3.58 MB in 1,447 range requests,
cold. `--write-costs` then records the run in the file's build record, so the
next reader gets the numbers from the CARD tier instead of re-measuring.

### Which rebuild path — the ceiling is statements, not bytes

There are three tools, and choosing between them by file size is the mistake.
`repyramid` materializes **every quad as owned strings** before re-assembling,
so its RAM tracks the **statement count**: ~350–700 bytes per statement on
ordinary graphs. As a ratio of file size that lands anywhere between **17× and
35×**, which is why bytes are the wrong planning variable.

| the file's problem | command | ceiling on a 48 GB machine |
|---|---|---|
| only the build record is missing; the queries answer | `rete card-audit <file> --measure --write-costs` | none from the rewrite — but the *queries* need the RAM (below) |
| the card is stale or its queries are broken | `rete repyramid <in> -o <out> --card --card-file <curated>` | **~80 M statements** (~70–100 M; ≈ 2 GB of a typically dense `.rete`) |
| same, but past that | `rete export <in> --format nq` → `rete build … --card-file <curated>` | **~150 M statements**, at ~2.5× the wall clock and 9–15× the `.rete` in staged text on disk |
| past *that* | — | nothing today; it is engine work, not scripting |

**The failure mode when you exceed the ceiling is an OOM kill mid-rebuild**,
after however long the read took — not a diagnostic. Predict before you start:
statement count × ~500 B is the working estimate for `repyramid`, × ~300 B for
the staged path. `switzerland-fedlex` (1.04 GB, 56.3 M quads) predicted ≈36 GiB
for `repyramid` and was never attempted there; the staged path did it under
19.1 GiB.

The staged path is bounded on the export half and only the export half:
`rete export --format nq` streams, with a **measured peak of 2.9–3.0 MiB**
whether it is producing 679 MB or 6.88 GB of N-Quads. What it buys with that is
disk — the staged text runs **9–15× the `.rete`** — and the two paths land in
the same place: on `nkod` they produce the **same content hash**, differing only
in the four unhashed bytes that say `repyramid` rather than `build`.

**`rete build --memory-budget-mb` is still not the third option**, even though
it is the genuinely bounded builder — and even now that it builds named graphs
(#139), which used to rule out exactly the named-graph population this work
exists for. Where it runs it writes a **counts-only card**, roughly a hundred
bytes of `triple_count` / `quad_count` / `term_count` / `named_graph_count` with
no profile and **no starter queries**, because deriving the profile lists needs
unbounded RAM. Ten of the largest published datasets — `crossref`, `datacite`,
`dblp`, `deps-dev`, `epfl-graph`, `gharchive`, `gharchive-2026-06`,
`opencitations`, `orcid`, `wikiart` — carry exactly that, and report `0 starter
queries` in the survey. That is *why* they do. They are also the files no
rebuild path above can reach.

**Measurement RAM is in the queries, not in the file.** Attaching costs to an
existing file rewrites it through a bounded buffer, but running the starter
queries to get those costs does not: the engine evaluates eagerly, so a query
with a large result materializes it. `switzerland-fedlex` took 381 s and peaked
at **14.2 GiB** for `--measure --write-costs`, of which `ng-list` alone —
497,905 rows — accounts for 3.2 GiB. Budget for the widest starter query, not
for the file.

### What a re-card changes — and what it must not

It **re-derives the whole derived half** from the data the file already holds:
the profile, the signals, the class-link quotient, and the starter-query
library with every generator fix since the file was built. It **adds the build
record**. It **carries the curated half across verbatim** — and only if you
hand it back:

```sh
rete card old.rete --json > old-card.json
python3 scripts/recard/card_tools.py curated old-card.json -o curated.json
rete repyramid old.rete -o new.rete --card --card-file curated.json --pyramid-algo types
```

A bare `rete repyramid --card` **silently drops** `title`, `license`, `source`
and `description`: the card flags take the curated half from flags or
`--card-file` and nowhere else. `card_tools.py verify --old … --new …` exists to
fail the run when a curated field goes missing, and the two proofs a re-card
owes — identical N-Quads out of both files, and every new starter query
returning rows — are what `scripts/recard/recard.sh` runs before it moves
anything into place. (Note the `--pyramid-algo` in that command: `repyramid`
defaults to `louvain` regardless of what the file was originally built with,
and on an older file the build record that would have said is precisely what is
missing.)

What a re-card **must not** do is rewrite the publisher's sentences. A tool
that edits prose cannot be trusted with the prose it leaves alone, so
`description` travels through unchanged, stale figures included. When a
description needs correcting, a human corrects it — and proves the edit was
surgical by running `verify` twice: once against the original card, expecting
**exactly one** reported difference, and once against the corrected document,
expecting a clean pass. Two differences on the first run means something moved
that you did not intend.

### Carrying forward is a floor — filling a blank is not rewriting

"Carry the curated half across verbatim" answers the dangerous question (may a
tool alter what a publisher wrote?) but not the useful one: **may it fill in
what nobody ever wrote?** A file built before `keywords`, `theme`,
`canonical_url`, `publisher` and `derived_from` existed has none of them, and
carrying-only guarantees it never will. Those are different acts, and only the
first needs forbidding.

So `recard.sh --enrich FILE` takes a document with this same reserved top level
and lays it over the carried fields. The gate is handed the *same* document and
permits exactly the keys it names to differ — and requires every one of them to
be present afterwards. Enriching `keywords` while dropping `title` still fails.

Two rules make this safe rather than merely convenient:

1. **Only on files you publish.** For someone else's `.rete`, an invented ROR or
   DOI is fabricated metadata in another party's record, and nothing in the file
   distinguishes the two cases — so the tool's default stays carry-only and the
   caller opts in per file.
2. **Derive, or leave it empty.** Every value must trace to something checkable:
   the catalog's own published URL, a resolved concept IRI, an upstream that
   answered. Where nothing supports a field, the honest output is a blank —
   `creators` needs an ORCID nobody has recorded, and an upstream's DOI belongs
   in `derived_from` and `cite_as`, never in `doi`, which would claim this file
   *is* that deposit. `sparql_endpoint` is the sharpest case: it means an
   endpoint a client can send this dataset's query to, so a project home page in
   that field is a lie a client can act on. Probing the 22 files of the
   2026-08-06 batch, one upstream endpoint answered the protocol *and* answered
   about the file's own IRIs; a second answered the protocol and knew nothing
   about them, which is exactly the failure a home page would have hidden.

The prose rule is untouched: `description` and `title` still travel through
unchanged. The one repair that is not a rewrite is a **provable encoding
defect** — `memoria`'s card stored "MemÃ²ria", the UTF-8 bytes of "Memòria" read
once as latin-1, and re-decoding them returns the publisher's own string
exactly. That is recovering what was written, not editing it; it goes through
`--enrich` like any other change so the gate reports it, and it is worth doing
only because the bytes prove the intended text.

**A headline count can go down, and that is the fix, not data loss.** An older
card counted the raw pre-dedup input multiset; a re-card counts what the file
actually stores. On `lombardi` the published card says 70,719 statements, its
own header says 70,545, and the file exports exactly 70,545 N-Quads — the
re-carded card says 70,545, and the card and the header agree for the first
time. `switzerland-fedlex` moved 66,392,663 → 56,321,446 the same way. Read the
new number against the data proof (identical N-Quads), never against the old
one; and where the old number is real provenance — an input line count, as
fedlex's was — keep both, in words, by hand.

### Publishing the result

A re-card changes the content hash, by construction and correctly: the card is
part of what the file says about itself. So the rebuilt file is a **new object**
to upload, `scripts/check_dataset_catalog.py` has to re-probe it, and
`web/datasets.lock.json` has to be regenerated. That is a release action —
`scripts/recard/*` deliberately stop at `--out-dir` and let a human decide.

Attaching costs is the one exception worth knowing: the build-info section sits
**outside** the content hash, so `rete card-audit --measure --write-costs`
leaves the checksum, `rete verify` and the exported N-Quads untouched. The file
is still rewritten end to end (the section sits near the front, so making room
shifts everything behind it), but its **identity is preserved** — confirmed on
published files, which grew by 2,007 bytes (`tree-city-inventory`), 1,952
(`lombardi`) and 32 (`switzerland-fedlex`) while keeping their content hash,
their `rete verify`, and their exported N-Quads byte for byte.

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

### In the browser

The [playground](playground.html)'s **🏷 Card** button renders the same card,
and the build record with it. Both come from **one** engine call — the header
plus the single coalesced range that covers the card and the adjacent
build-info section — so showing the build conditions costs a reader nothing
over showing the card alone.

What the rendered view does with each field is chosen to keep the card's own
distinctions visible:

- `keywords` and `theme` sit with the description as tags, since they say what
  the dataset is *about*. A theme's IRI is **not resolved** — that would be a
  network read the CARD tier exists to avoid — so the viewer names the
  **concept scheme**, which it can read from the IRI's prefix, and shows the
  concept's own identifier. It never invents the label the scheme owns.
- `creators`, `publisher` and `doi` render as **links to the identifier**. That
  is the whole reason the card asks for an ORCID/ROR/DOI as an IRI rather than
  a string: it resolves, and it joins.
- `cite_as` gets a copy button — a citation exists to be pasted elsewhere.
- `extra` is rendered **last and fenced off**, labelled as the publisher's own
  fields, with the values shown in their JSON form and nothing linkified,
  rounded or thousands-separated. Formatting them would imply rete had
  understood them, and its contract is precisely that it has not.
- The **build record** is its own part of the modal, after the card, because it
  describes one *build of one file* rather than the data. Its per-query cost
  figures are shown **with the queries they describe** — that is where the
  question "what will this cost me?" gets asked — with `bytes`/`requests`
  leading and `debug_ms` labelled as one machine's reference.
- A file with no build record **says so**. It is the common case (every card
  written before the section existed, and every in-browser build) and it reads
  as absence, never as zeroed measurements.

The playground's **Build** mode can also *write* a card: paste the same
`--card-file` document into step 3 and the file it builds carries it. The
engine validates it with the rules on this page — the same code the CLI runs,
so the browser cannot compose a card `rete build --card-file` would refuse. It
writes the curated fields plus the four counts its own build measured, and the
built card carries no derived profile and no build record. See
[the playground guide](playground-guide.md#build).

That is the playground's own choice, not a limit of the engine: the wasm build
exports `build_with_derived_card`, which derives the full profile in the
browser. It is a separate export because derivation walks the graph twice more,
and `build_with_card`'s bytes are a shipped contract.

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
- **Off the remote-lazy query path.** `rete query-url`, `summary-url`, federation
  routing, browser/WASM queries, and larger or eager-disabled native
  `sparql-url` opens fetch sections by offset and **never read the card**, so it
  adds nothing to their query-time bytes-on-the-wire. An eligible small native
  `sparql-url` object is transferred once in full, so that transfer includes the
  card bytes; the bounded in-memory range opener still does not decode the card.
  To read the card explicitly without downloading the file, `rete card-url`
  fetches just the header + metadata range (two ranges, index untouched).
- **Opaque to the writer's *layout*, not to the card.** The metadata section is
  raw bytes as far as the file format is concerned — a general extension point,
  of which a card is just the first use. The card *itself*, both halves, lives
  in `rete_core::card`: the write-time rules for the curated half (the reserved
  top level, the `theme` IRI requirement, the `extra` bounds), and the
  derivation of the profile and the starter-query library. It used to live in
  `rete-cli`, which is a binary-only crate no client can link, so a card
  derived there could never be derived anywhere else ([#152]) — a correction to
  the derivation reached CLI-built files and nothing else. One implementation
  now answers for the CLI, the browser, and every language binding.

[#152]: https://github.com/caviri/rete/issues/152

See [the format specification](SPEC.md) for the header layout and
[the CLI reference](cli.md) for every flag.
