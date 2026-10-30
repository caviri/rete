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

### `rete build <inputs…> -o <out.rete> [--format nt|nq|ttl|trig|rdfxml] [--quoted-triple-syntax rdf12|rdf-star]`
Build a file from one or more RDF inputs, merged under one shared dictionary.
Format is detected by extension (`.nt` / `.nq` / `.ttl` / `.trig`, plus `.rdf` /
`.owl` / `.rdfxml` for RDF/XML — how most OWL ontologies ship); `-` reads stdin and
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
**build record** (the build-info section, kept *outside* the content hash so
identical data still hashes identically): every flag that shaped the file, by
its effective value; the engine (version, git commit, target, profile,
features) and the host's OS and architecture — nothing else about the machine;
each input by **file name only** with its size, format and a BLAKE3 hashed as
it streamed; the statement counts (read, unique, duplicates dropped, named
graphs); wall time per phase, peak RSS, the memory limit and CPUs seen; the
memory-bounded build's spill; and the measured starter-query costs. See
[the build record](dataset-cards.md#build-conditions-the-build-info-section).
`RETE_BUILD_RECORD=reproducible` leaves the volatile facts out, so two builds
are byte-identical with it. That same run is what proves each starter query answers:
one measured at zero rows (or returning a row that binds nothing), or one that
does not finish inside the 64 MiB starter byte budget (counted past the open),
is **dropped from the
card**, loudly and with the reason recorded. The starters themselves are
bounded before they are run: past 2,000,000 statements a template ships a
`LIMIT`ed form or, when the card's own profile answers it, nothing — see
[Bounded by construction](dataset-cards.md#bounded-by-construction).
`--no-card-costs` skips the run, and so skips that check as well. `--text-index` adds a
full-text (word/CONTAINS) index over the literals for `rete search --contains`
and SPARQL `CONTAINS` — its scope flags and the memory-bounded build are under
[Full-text index](#full-text-index---text-index-and-its-scope) below.
`--type-predicate <IRI>` overrides the predicate that types subjects
with classes for the schema (default `rdf:type`) —
e.g. Wikidata's `--type-predicate http://www.wikidata.org/prop/direct/P31`.
`--no-pyramid` skips the community pyramid entirely (no pyramid section): SPARQL /
SHACL / triple-pattern / reachability queries don't use it, so the file stays
fully queryable and is markedly smaller — only community / summary / progressive
queries need the pyramid. `--pyramid-algo louvain|types` picks the community
algorithm: `louvain` (default) detects topological communities; `types` partitions
by `rdf:type` instead — deterministic, self-naming communities that stay feasible
on graphs too large for the single-threaded Louvain build (it falls back to
Louvain when the graph is untyped). These are opt-in; without them the output is
a plain build.

**Labels and schema have sections of their own, outside the pyramid.**
`--label-index` writes a LABEL_INDEX section (kind 8): the display label of
**every** labelled subject — `rdfs:label`, `skos:prefLabel`/`altLabel`,
`foaf:name`, `dc`/`dcterms:title`, `schema:name`, and for people who carry only
given and family names, `"family, given"` — sorted for
[`rete search <prefix>`](#rete-search-file-prefix---contains-word---contains-prefix-p---limit-n---json).
It has no cap, and a remote prefix search reads a few small ranges of it and no
dictionary. Without the flag the file has **no label index at all**: the
pyramid-meta no longer carries the 8,192-entry block it used to. Every build also
writes a SCHEMA section (kind 9) — the class hierarchy and level rollups, the
coherence axioms, the planner's per-predicate `query_stats` and the
characteristic sets — computed without the pyramid, so `--no-pyramid` and
`--memory-budget-mb` files have them too; it is a few kilobytes, and `--no-schema`
leaves it out. Both flags work with `--no-pyramid`, any `--pyramid-algo` and
`--memory-budget-mb`, and both sections sit outside the content hash (an older
`rete verify` still passes; an older reader just doesn't see them — see
[compatibility.md](compatibility.md)). The publish recipe is
`--pyramid-algo types --label-index --card`.

```sh
rete build data.nt -o data.rete --no-pyramid --label-index   # labels + schema, no pyramid
rete build huge.nq.gz -o huge.rete --memory-budget-mb 3000 --label-index
```

**The memory-bounded build (`--memory-budget-mb N`).** It reads the input
once and cuts it into chunks of about half the budget. Each chunk's
dictionary and statements go to `--tmp-dir` (default: next to the output). A
k-way merge then builds the one global dictionary, the chunks are rewritten in
global ids one at a time, and external sorts build every index. At any moment
it holds one chunk being sealed, the merge's buffers, or one sort run, and
each of those is sized from the budget, not from the graph. On the
OpenCitations sample (18.9 M statements, `--named-graphs quad --text-index
--label-index`) the peak RSS is 850 MiB at a budget of 1,000 MiB and 2.4 GB at
3,000 MiB, both while the chunks are sealed; eight renamed copies of it (151 M
statements, 186 chunks at 400 MiB) peak at 391 MiB, where 0.4.0 needed
1,084 MiB. The spill peaks at the end of the dictionary merge, while every
chunk's statements are still on disk; `--label-index` adds nothing to it,
because the label pass works on ids and reads its strings back from the
finished file. Up to 0.4.0 the merge also kept every chunk's id remap table
until the last chunk was rewritten, 4 B per chunk-local term: 15.0 GB for half
of OpenCitations Meta, which is why that graph had to be split.

Its sections are byte-identical to the in-RAM `--no-pyramid` build's of the same
input. With a card (`--card`, `--card-file`, a curated flag) it writes the
**full** card — the derived profile and the starter queries, not counts alone:
once the index sections are on disk, it streams them into the profile and runs
every starter query against the file, exactly as
[`rete recard`](#rete-recard-file--o-outrete----in-place---card-file-f----keep-curated---title----no-card-costs---memory-budget-mb-n---verify)
does (one derivation, so the two write the same card for the same file), inside
the same `--memory-budget-mb` (one byte per subject for the class map, the rest
for the reader's caches). `--no-card-costs` skips the queries. The card and the
build record are then written **after the data** rather than copied in front of
it, which only means one more read of the file to fold the content hash (the card
is hashed first wherever it lies; on input with no duplicate statements and no
multi-typed subjects the card, and so the hash, is the in-RAM build's). The
derivation's time and peak RSS are the `card_derive` and
`starter_queries` phases of the build record, and the hash pass its
`content_hash`. On the OpenCitations sample: see
[Maintaining a published card](dataset-cards.md#which-re-card-path).

`--strict` refuses input carrying an IRI that the N-Triples/N-Quads `IRIREF`
grammar and RFC 3987 disallow. It is **off** by default and the build only
*counts* them — see [Invalid IRIs](#invalid-iris) for what that means and why the
default is a warning.

<a id="quoted-triple-syntax-on-input"></a>
**Quoted triples on input (`--quoted-triple-syntax`).** The same flag name and
the same two values `rete export` takes, pointed the other way: which surface a
`<< … >>` in a **Turtle or TriG** input *means*. `rete validate` takes it too.

| value | `<< s p o >>` reads as | `<<( s p o )>>` reads as |
| --- | --- | --- |
| `rdf-star` (default) | a **quoted triple** — one term, subject or object position | a syntax error |
| `rdf12` | a **reifier**: `_:r rdf:reifies <<( s p o )>>` plus a statement about `_:r` | a **triple term**, object position only |

The same file parses into **two different graphs** under the two values. That is
not a rete quirk — RDF-star and RDF 1.2 genuinely give `<< s p o >>` different
meanings, and nothing in the bytes says which one the author meant, so it cannot
be detected and has to be told.

`rdf12` also reads the rest of RDF 1.2 Turtle: `{| … |}` annotations (which
expand to a reifier carrying the annotation's properties) and `"…"@lang--dir`
directional literals.

**Why the input default is `rdf-star` while the export default is `rdf12`.**
They answer different questions. Export asks *who can read this?* — and current
parsers want RDF 1.2. Input asks *what did the author mean?*, where the two
possible mistakes are not equal:

- An RDF-star file read as `rdf12` **parses**, and gives you a different graph.
  Silently. That is the failure this flag exists to prevent, and it is the same
  one [#262](https://github.com/caviri/rete/pull/262) found on the write side.
- An RDF 1.2 file read as `rdf-star` is a **hard parse error**, because `<<(`
  belongs to RDF 1.2 alone. rete detects that case and names the flag in the
  error rather than complaining about a stray `(`.

Only one of those is recoverable, so the default is the one that makes the other
mistake loud. The consequence to know: `rete export --format trig` writes
`rdf12` by default, so re-ingesting rete's own dump takes
`--quoted-triple-syntax rdf12` — or export with `--quoted-triple-syntax
rdf-star` and it re-ingests with no flag at all.

**N-Triples, N-Quads and RDF/XML ignore the flag.** Their reader is rete's own,
it has accepted both `<< s p o >>` and `<<( s p o )>>` since 0.3.2, and RDF 1.2
N-Triples has no reifier syntax for `<< … >>` to be — so there is no ambiguity
to resolve and nothing to choose.

**Nothing is stored differently.** Either surface lands rete's one canonical
token `<<s p o>>`, and an RDF 1.2 reifier is ordinary RDF — a blank node, an
IRI and a term — which rete stored long before the flag existed. A file with no
quoted triples builds byte-for-byte identically under either value. See
[Triple-store interop](interop.md#quoted-triples-two-surfaces-one-graph),
which also covers using the two flags together to **translate** between the two
worlds.

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

`--named-graphs per-graph|quad` (default **per-graph**) chooses how the named
graphs of an N-Quads/TriG input are stored. `per-graph` writes one permutation
index per graph (the NAMED_GRAPHS section): the graph's IRI as text, then its
own tiles, the graphs chained with no directory. `quad` writes the **quad
index** ([SPEC.md §6.5](SPEC.md#65-quad-index-quad_index-section-optional)):
every named-graph quad stored once, the graph as a fourth column of
graph-leading permutations in tiles that many graphs share, behind a sorted
graph directory, with graph names as dictionary terms. The rows a query returns
are the same either way. What changes is the cost of many small graphs — on the
OpenCitations sample (1.5 M per-entity provenance graphs of ~9 quads) the
named-graph section drops from 974 MB to 102 MB, the build from 33 min to 77 s,
and a remote `GRAPH <g>` query from 414 MB in 81 requests to 3.7 MB in 26
([BENCHMARK.md](BENCHMARK.md#named-graphs-per-graph-layout-vs-the-quad-index)).
The layout applies to every build path, `--memory-budget-mb` included;
`repyramid` and `merge` keep the layout of their input.

**A `quad` file is refused by every Rete before 0.4.0** — loudly, with the same
container section-count error as above, on every query, export and remote open
(`info`, `verify` and `card-url` still work). Keep `per-graph` for a file older
engines must read.

The flag exists on `build` only. The two commands that re-assemble an existing
file do not re-decide how it was built: [`repyramid`](#rete-repyramid-file--o-outrete---type-predicate-iri---pyramid-algo-louvaintypes---text-index---label-index---card-)
**preserves its input's set** (and says so on stderr when that set is not the
default six), and [`merge`](#rete-merge-inputs--o-outrete---memory-budget-mb-n---tmp-dir-dir---card-)
writes the **union** of its inputs' sets — one full shard is enough to keep the
merge-join orders its queries relied on. To change a file's set, rebuild it from
its RDF source.

#### Full-text index: `--text-index` and its scope

`--text-index` adds a TEXT_INDEX section: every word of every literal object in
the index's **scope** maps to the subjects that carry it. `rete search
--contains` answers from it, and SPARQL uses it to prune a `FILTER(CONTAINS(…))`
before re-checking every row. Three flags set the scope; each needs
`--text-index`, and `repyramid` takes them too:

| flag | default | what it covers |
| --- | --- | --- |
| `--text-index-graphs default\|all` | `default` | `default`: the default graph's literals — the scope of every index built before 0.4.0. `all`: those and every named graph's. |
| `--text-index-include <IRI>` | every predicate | only the literals of this predicate; repeat the flag for several |
| `--text-index-exclude <IRI>` | none left out | every predicate's literals but this one's; repeat for several |

The IRI is a full one, bare or in `<…>`; a prefixed name is refused, because a
build has no prefix map to expand it with. Include and exclude do not combine: an
include list already says exactly what is indexed. A listed predicate that no
literal in scope carries is reported on stderr, since a misspelt exclude would
otherwise exclude nothing.

```sh
rete build data.nq -o data.rete --text-index
# OpenCitations-style: index the provenance graphs too, but not the generated
# change notes or the stored SPARQL update text
rete build oc.nq -o oc.rete --text-index --text-index-graphs all \
  --text-index-exclude http://purl.org/dc/terms/description \
  --text-index-exclude https://w3id.org/oc/ontology/hasUpdateQuery
```

The scope is written **into the section** (SPEC §6.3), so a reader acts on what
the file covers rather than on how it was built:

- `rete search --contains` finds only words of literals in scope.
- The SPARQL `CONTAINS` pushdown runs only where the index covers the query: the
  active graph must be the default graph (or, under `all`, any graph), and the
  pattern's predicate must be indexed — a variable predicate only when every
  predicate is. Anywhere else the filter scans and returns the same rows, just
  without the shortcut. (Before 0.4.0 the pushdown also ran inside `GRAPH`
  against a default-graph index, where it returned no rows at all.)
- **An include/exclude index is stored as section kind 11**, not 6. Readers
  older than 0.4.0 do not know kind 11 and see no text index: `rete search
  --contains` says the file has none, and `CONTAINS` scans. That is on purpose.
  Those readers prune `CONTAINS` with whatever text index they find, as if it
  covered every literal; with a predicate-restricted one they would silently
  drop the rows of the predicates it left out. A `--text-index-graphs all` index
  over every predicate stays kind 6, because it covers more than they assume,
  never less.

A carded build records the flags in its build-info (`text_index_graphs`,
`text_index_include`, `text_index_exclude`), next to `text_index`.

**What a scope saves**, measured on the OpenCitations Meta sample (M=1200,
18,869,242 statements) laid out with its provenance **in the default graph**,
memory-bounded at 3,000 MiB. The file without a text index is 166,562,617 B.

| `--text-index` scope | tokens | section | token table | file |
| --- | --: | --: | --: | --: |
| every predicate | 2,178,475 | 68,256,530 B | 9,996,927 B | 234,819,147 B |
| all but `oco:hasUpdateQuery` | 2,164,679 | 60,352,482 B (−12%) | 9,770,745 B | 226,915,099 B |
| all but `dcterms:description` | 831,343 | 36,789,772 B (−46%) | 4,667,222 B | 203,352,389 B |
| all but both | 534,449 | 23,825,664 B (−65%) | 3,177,349 B | 190,388,281 B |
| all but both and `prov:generatedAtTime`, `prov:invalidatedAtTime` | 533,705 | 10,199,845 B (−85%) | 3,156,034 B | 176,762,462 B |

The last row is the data alone: its postings are byte for byte those of the same
sample built with provenance in named graphs and the default scope, and it finds
the same subjects for every data word. The provenance prose ("The entity … has
been modified.") and the stored SPARQL update text are 65% of the full index,
the timestamps another 20%. The token table, which a first remote search
fetches whole, shrinks from 10.0 MB to 3.2 MB.

The other way round: with the provenance **in named graphs** (the layout
OpenCitations ships), the default scope indexes the data alone (10.2 MB), and
`--text-index-graphs all` indexes the same literals as the first row above —
the same 2,178,475 tokens and the same postings, 68.3 MB, +5.4% on the 1.07 GB
file — so the provenance becomes searchable without moving it out of its
graphs.

**In the memory-bounded build.** `--text-index`, with any scope, works with
`--memory-budget-mb`, and the TEXT_INDEX section is **byte-identical** to the
in-RAM build's for the same input. Each chunk writes its literal words with
chunk-local subject ids to `--tmp-dir` while it is sealed. The ids are remapped
with the tables the dictionary merge already produces, sorted externally by
`(word, subject)` in runs of at most a quarter of the budget, and merged into
the section. The token table is the one part a reader must hold whole, so the
build streams it to disk and compresses it from there; the in-RAM build
compresses it with the same streaming encoder, which keeps the two identical
past zstd's 4 MiB window (0.4.0 did not: its memory-bounded token table could
come out a few KB larger). `extbuild: text index …` reports the token and
record counts.

Measured on the same sample with provenance in the default graph (50.5 M word
records, 2.18 M tokens): at 1,500 MiB the build took 83 s instead of 59 s, peak
RSS 1.19 GB against 1.17 GB without the index, and the peak spill grew from
0.91 GB to 1.38 GB. The peak is set by the chunk phase, not by the text sort,
so it does not move with the index. With provenance in named graphs and the
default scope (3.3 M records) the text phase takes under a second of a build
whose time goes to the 1.49 M named graphs.

<a id="compression-knobs"></a>
**Compression knobs (`--tile-kib`, `--dict-chunk-kib`, `--zstd-level`,
`--frequent-objects`).** How the index and dictionary are cut and compressed.
None of them changes the file format: every combination opens in every reader of
format `0x05`, released ones included (see
[compatibility.md](compatibility.md#04-writer-changes-that-every-0x05-reader-reads)),
and every one changes the bytes but never an answer. The measurements behind the
defaults are in [BENCHMARK.md](BENCHMARK.md#compression-04).

| flag | default | what it trades |
| --- | --- | --- |
| `--tile-kib N` (4–4096) | 64 | Uncompressed size of an index tile, the unit a reader fetches and decompresses. 128 makes the file about 1% smaller and each touched tile larger; above 128 the file grows again (zstd's small-input parameters stop applying) and remote queries fetch more bytes (+9% at 256, +24% at 512). |
| `--dict-chunk-kib N` (4–4096) | 64 | The same for dictionary chunks. |
| `--zstd-level N` (1–22) | 9 | Level of every tile and chunk. `19` makes a file 4–9% smaller than `9` for about 2.7× the build CPU, and decodes 8–18% slower: a choice for a file built once and read for a long time. Levels between 9 and 19 gain little or nothing. |
| `--frequent-objects N` | 0 (off) | Moves the `N` most frequent object-only terms (classes, controlled vocabularies, status literals) into the shared dictionary section, which gives them small ids where they sort ahead of the shared entities — the size effect of declaring the vocabulary in a TBox, without one. `64` made the OpenCitations samples 4.8–6.5% smaller, against 2.7–5.5% for their 499-statement TBox, and two other datasets 2.3–3.2% smaller, at no measurable build cost. Not available with `--memory-budget-mb` (refused). |

Each flag falls back to an environment variable when left out
(`RETE_TILE_KIB`, `RETE_DICT_CHUNK_KIB`, `RETE_ZSTD_LEVEL`,
`RETE_FREQUENT_OBJECTS`), which is also how `merge` and `repyramid` take them
(`merge` runs the external builder, so it ignores `RETE_FREQUENT_OBJECTS`). A card
build records the settings in its build info (`params.write`).

Since 0.4 every tile and chunk is a **single-segment zstd frame that records its
size**, which lets zstd use its small-input parameters: at the same level the
four measured datasets came out 0.7–7.0% smaller, and the build used less CPU. `RETE_ZSTD_FRAME=stream` writes
the streaming frames of 0.3 instead, byte for byte.

### `rete recard <file> (-o <out.rete> | --in-place) [--card-file F | --keep-curated] [--title …] [--no-card-costs] [--memory-budget-mb N] [--verify]`
Give a finished `.rete` a new Dataset Card **without rebuilding it** — the
re-card path for a file of any size.

The card is derived by **streaming the file's own sorted sections**: the
`rdf:type` range of POS gives every typed subject its class, SPO gives each
subject's out-degree and namespace the moment the subject changes, and OSP each
object's in-degree and every literal object, decoded once with the multiplicity
of each predicate that uses it. Every tally is kept per *vocabulary* term
(predicates, classes, datatypes, languages, namespaces), never per statement;
the one per-term structure is a class code per subject id — **one byte per
subject** below 255 classes, two above. Memory is that plus the reader's caches
(`--memory-budget-mb`, default 1024), not the statement count. The
memory-bounded `rete build` runs this same derivation for its own card, so a
re-card of its output writes the card it already has.

Then the starter queries are generated, each is run against the file inside the
starter byte budget (64 MiB past the open, see [Dataset Cards](dataset-cards.md#bounded-by-construction)),
and the ones that do not answer within it are dropped and recorded. With
`-o`, the file is **rewritten with only the card (metadata section) and the
build record (build-info section) replaced**: every other section — dictionary,
index, pyramid, named graphs, text index, any section kind this binary does not
know — is copied byte for byte, in its place in the file order, the header's
offsets are fixed, and the content hash is recomputed while the bytes go past
(the card is inside it). One read of the file to derive, one to copy;
`--verify` adds a third to check the written hash before the file is moved into
place. The output must not exist: a re-card writes a new file and never over
its input.

**`--in-place`** skips the copy. The new card and build record are written
**after the file's last section**, followed by a new footer; the file is
synced; only then is its 1 KiB header rewritten to point at them, with the new
content hash, and synced again. That costs one read of the hashed sections (the
card is hashed first, so the hash can only be folded once it exists) and a few
KB of writes, instead of a second copy of the file. Nothing the old header
points at is written over, so a crash before the header write leaves the old
file exactly as it was, readable by every reader, plus bytes past its end that
the next run overwrites; a crash after it leaves the new one. The old card,
build record and footer stay in the file as **dead bytes** — a few KB, more on a
card with large curated fields — that no section points at; every reader
ignores them, and a re-card with `-o` drops them. `--verify` re-reads the file
after the header write. A file re-carded in place, or built memory-bounded with
a card, keeps its card after the data; [every reader](compatibility.md#a-card-after-the-data)
since the format froze finds it there.

The curated half comes from `--card-file` and the curated flags exactly as
`rete build --card-file` takes them, or with `--keep-curated` from the input's
card, **verbatim** — description, example queries and `extra` bag included. With
neither, an input card that carries curated fields is **refused** rather than
silently stripped (the defect of a bare `repyramid --card`).

The build record keeps the original build's timestamp, builder and parameters —
the sections they describe were copied, not rebuilt — and gains a `recarded`
entry (`at`, `by`, `from_content_hash`, and the re-card's own `engine`, `host`,
`input` by file name, `params` — with `in_place` — and `run`); its
`query_costs` and `dropped_queries` are the re-card's own measurement. In place,
the record is sealed before the final write, so its `run` covers everything but
those few KB.

Two differences from the card an in-RAM build derives, both because a finished
file is read rather than its input: counts are over the **deduplicated**
statements the file holds, and a subject typed more than once is classified by
its class with the highest dictionary id (index order). On input with neither,
the two cards are identical, byte for byte.

```sh
rete recard old.rete -o new.rete --keep-curated --verify
rete recard old.rete -o new.rete --card-file curated.json     # curated fields from a document
rete recard big.rete --in-place --keep-curated                # no copy: card appended, header last
bash scripts/recard/recard.sh --source /work/old.rete --out /work/new.rete   # with both proofs
```

Measured on an OpenCitations Meta part (26.5 GB, 2,904,375,666 statements,
978,932,751 terms): see [Maintaining a published card](dataset-cards.md#which-re-card-path).

### `rete repyramid <file> -o <out.rete> [--type-predicate <IRI>] [--pyramid-algo louvain|types] [--text-index] [--label-index] [--card …]`
Rebuild a file's pyramid **in place**, reading the triples straight from the
existing `.rete` — no `export | build` N-Quads round-trip. Use it to add a schema
(or a `--text-index`, a `--label-index` / a Dataset Card) to a file built before
those existed, or to re-derive the schema pyramid under a different `--type-predicate`
or `--pyramid-algo` (same semantics as `rete build`). The card flags match
`rete build` (`--card-file` / `--title` / `--license` / …) — and take the
curated half **only** from those flags, so a bare `--card` drops the
publisher's `title`/`license`/`source`/`description`. `--pyramid-algo` likewise
defaults to `louvain` rather than to whatever the file was built with.

It loads and materializes every quad, so its RAM tracks the **statement count**
(~350–700 B each), not the file size — roughly **80 M statements** on a 48 GB
machine. **To change only the card, use [`rete recard`](#rete-recard-file--o-outrete----in-place---card-file-f----keep-curated---title----no-card-costs---memory-budget-mb-n---verify)**,
which copies every other section instead of rebuilding it and has no such
ceiling; `repyramid` is for when the pyramid or the text index must change too.
See [Maintaining a published card](dataset-cards.md#maintaining-a-published-card).

```sh
rete repyramid old.rete -o new.rete --type-predicate http://www.wikidata.org/prop/direct/P31
rete repyramid old.rete -o new.rete --text-index    # add full-text search to an existing file

# re-card AND re-pyramid a published file, carrying its curated fields across
# (to change only the card, `rete recard old.rete -o new.rete --keep-curated`)
rete card old.rete --json > old-card.json
python3 scripts/recard/card_tools.py curated old-card.json -o curated.json
rete repyramid old.rete -o new.rete --card --card-file curated.json --pyramid-algo types
```

`repyramid` **preserves its input's permutation set**: a 3-permutation file stays
3-permutation, a 6 stays 6, and it prints the preserved set on stderr when it is
not the default six. It re-assembles an existing file rather than re-deciding how
it was built, so it has no `--permutations` of its own — rebuild from source to
change the set. It keeps a LABEL_INDEX the input carries (`--label-index` adds
one), and writes the SCHEMA section and the community-only pyramid-meta `rete build`
writes. The named-graph layout is kept the same way: a quad-index input
(`--named-graphs quad`) comes out as a quad-index file, and `merge` writes one
when any of its inputs is one.

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

### `rete validate <inputs…> [--format nt|nq|ttl] [--strict] [--quoted-triple-syntax rdf12|rdf-star]`
Parse RDF input(s) without building, to check they are well-formed
N-Triples/N-Quads/Turtle. Reports statement and named-graph counts, or exits
non-zero with a precise parse error (file, line, column).

```sh
rete validate data.ttl
curl -s https://host/data.nt | rete validate - --format nt
```

It also reports **invalid IRIs**, because "it parses" and "it is valid RDF" are
different claims and only the second one predicts whether a dump will load
anywhere else. `--strict` turns the report into a non-zero exit.

`--quoted-triple-syntax` means exactly what it means on
[`build`](#quoted-triple-syntax-on-input), which makes `validate` the cheap way
to ask *which surface is this file actually in?* — the counts differ, because
the graphs do:

```sh
rete validate claims.ttl                               # 2 statement(s)  — << >> is a quoted triple
rete validate claims.ttl --quoted-triple-syntax rdf12  # 4 statement(s)  — << >> is a reifier
```

### Invalid IRIs

rete's N-Triples/N-Quads reader is deliberately tolerant: it stores whatever
sits between `<` and the next `>`, so the file is a faithful container of what
it was given. That tolerance is why every published dataset built — and it is
also why `rete export --format nq` could emit something no strict parser
accepts. `build` and `validate` now **count** what they ingested:

```
warning: 5 statement(s) carry an invalid IRI (5 IRI occurrence(s)).
         They are stored verbatim, so `rete export --format nq` emits N-Quads that a
         strict parser (Oxigraph, Jena, rapper) rejects — and a bulk loader rejects the
         whole chunk, not the line.
               2  '[' or ']' outside an IP-literal host
                  e.g. <http://example.org/a[b]>
               1  more than one '#'
                  e.g. <http://example.org/c#d#e>
               1  '%' not followed by two hex digits
                  e.g. <http://example.org/%x-template-artifact>
               1  a character the IRIREF grammar excludes (space, control, or one of <>"{}|^`\)
                  e.g. <http://example.org/a b>
```

#### Validity is decided by a parser, not by a list

Two documents apply: the `IRIREF` production
(`'<' ([^#x00-#x20<>"{}|^`\] | UCHAR)* '>'`), which is a character set, and RFC
3987, which the same grammar requires the content to satisfy as an **absolute**
IRI. The first is settled by a scan. The second is settled by **`oxiri`** — the
crate Oxigraph's own N-Triples reader validates with — so rete's answer to "will
a strict loader take this" is the loader's answer, not an imitation of it.

The classes below do not decide validity. They answer the *other* question:
**can this be repaired, and how.**

| class | example | repairable by escaping |
|---|---|---|
| no scheme (relative IRI) | `<noscheme/path>` | **no** |
| an excluded character | `<http://ex/a b>` | yes |
| `[` / `]` outside an IP-literal host | `<http://ex/a[b]>` | yes |
| more than one `#` | `<http://ex/c#d#e>` | yes |
| `%` not followed by two hex digits | `<http://ex/%x>` | yes |
| rejected by RFC 3987, no class recognises it | `<https://::1>` | **no** |

That last row is the important one. An IRI the parser rejects that matches none
of the repair shapes is still **counted** and still **blocks** — it is never
silently passed. `<https://::1>` is an IPv6 literal in the authority without the
brackets RFC 3987 requires; it has a scheme, no excluded character, no bracket,
one `#` and no `%`, so the five repair classes are all blind to it. A dump
carrying one was published and Oxigraph refused to load it. Since the parser
decides, a future shape nobody has thought of fails the same way this one now
does: as an unrepairable defect that stops the export, not as a zero.

What is **not** judged: scheme semantics (`<nonsense://x>` is well-formed and
accepted), whether the IRI resolves to anything, and normalisation — rete never
case-folds a scheme or removes a dot segment, because that would change the
dictionary key. Non-ASCII **is** judged now, by the parser: RFC 3987 `ucschar`
is legal, so `<http://ex/café>` is valid and is never touched, while the narrow
sub-ranges 3987 excludes are caught because the loader catches them too.

`UCHAR` escapes are resolved before the parser sees the IRI — `<http://ex/café>`
denotes `http://ex/café`, and the backslash is N-Triples syntax, not IRI content.
Skipping that step would flag every escaped dump, and flagging something valid
means percent-encoding an IRI that was fine, which is the one failure mode a
sanitizer must not have.

Three flags act on this, and they are separate on purpose:

- `rete build --strict` / `rete validate --strict` — refuse the input, naming
  the IRI and the rule. **Not** the default: datasets that build today, including
  published ones, must keep building.
- `rete export --sanitize-iris` — percent-encode the repairable classes on the
  way out. Also not the default, because it **changes the data**.

An IRI with **no scheme** is reported and never rewritten. Escaping cannot
repair it: resolving it needs a base IRI the `.rete` never recorded. This is the
class that cost the most in the field — one such line made a bulk loader drop an
entire ~102,000-line chunk — and the only honest thing a tool can do with it is
say where it is. The same holds for an unrecognised defect: rete will not invent
a repair for something it cannot explain, because `<https://::1>` percent-encoded
to `<https://%3A%3A1>` would name a different host entirely. Fix those at the
source.

**Automating on the report.** `--sanitize-iris` prints one line meant to be
parsed:

```
--sanitize-iris: totals invalid=11 repairable=11 unrepairable=0 unclassified=0
```

A publication gate should key on **`unrepairable > 0`**, never on one named
class. `unrepairable` is the count of occurrences that are still invalid after
everything the sanitizer could do, and it includes the unclassified bucket by
construction — which is precisely why a gate written against it does not need
updating when a new defect shape turns up.

## Inspecting

### `rete info <file>`
Print the decoded 1 KB header (the section directory, codecs, counts, content hash) — plus
the [Dataset Card](dataset-cards.md) catalog when the file carries one. Reads only
the header and card byte ranges (the same two small reads `card-url` does over
HTTP), so a 50 GB file answers in about a second.

### `rete stats <file>`
Human-friendly overview: file size, statement counts, distinct terms, pyramid
levels, and top predicates. Notes the label-index and **text-index** sizes when
the file carries them.

The header counts every statement in the file, so `stats` prints that as
`statements` and splits it: `default-graph triples`, `named graphs` (the graph
count) and, when there are any, `named-graph quads`. The split costs no read of
the named graphs: with none, the header says it all; otherwise it comes from
the [Dataset Card](dataset-cards.md)'s build-time counts when the card agrees
with the header, else from counting the default graph's index tiles. The line
says which.

```text
opencitations-2026-06-small-full-labels.rete — 1073354201 bytes
  statements            : 18869242 (all graphs)
  default-graph triples : 5971936
  named graphs          : 1494934
  named-graph quads     : 12897306 (split from the dataset card)
```

Before 0.4.0 the first line read `default-graph triples : 18869242`, which
on this file is the whole dataset, 1.5 M provenance graphs included.

### `rete verify <file>`
Recompute the blake3 content hash and compare to the header, and check the
LABEL_INDEX / SCHEMA sections against their own digests (they are outside the
content hash). Exits non-zero with `FAILED — content hash mismatch, …` on
corruption/truncation. The hashed sections
are streamed in 8 MiB reads, so verifying a 49 GB file needs megabytes, not
49 GB (it used to read the whole file into memory).

### `rete search <file> [<prefix>] [--contains <word>…] [--contains-prefix P] [--limit N] [--json]`
Two modes.

**Label prefix** (default) — the subjects whose label starts with `prefix`
(case-insensitive), printed as `label<TAB><iri>` (or
`{"schemaVersion":1,"matches":[{"label":…,"subject":…}]}` with `--json`).
An empty prefix returns the first `--limit` labels (default 20).
Answered from the file's **LABEL_INDEX** section (`rete build --label-index`) —
**no literal scan**, every labelled subject, results in label order — so it is the
fast path for autocomplete. Labels come from `rdfs:label`, `skos:prefLabel`/`altLabel`,
`foaf:name`, `dc(terms):title` and `schema:name`, and for people with only
given/family names `"family, given"` (so `svobodová, j` finds the author). A file
built before the section existed answers from the bounded block its pyramid-meta
carried instead (the 8,192 most-connected labelled subjects, direct labels only).
A file with neither says so; rebuild with `--label-index`.

**Full-text** (`--contains <word>…`) — the subjects whose literals contain
**every** given word (whole-word, case-insensitive — AND), printed one IRI per
line (or the same versioned `matches` envelope with `{subject}` entries under
`--json`). `--contains-prefix einst` additionally
requires a literal word starting with `einst`. Answered from the opt-in
**TEXT_INDEX** section (`rete build --text-index`); on a remote file only the
queried words' posting lists are fetched, not the whole index. A file built
without `--text-index` reports that it has no text index. Only words of the
literals in the index's [scope](#full-text-index---text-index-and-its-scope) are
found — by default the default graph's, every predicate. To search a file you
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
file carries one) the **build record** as a `build:` block: when it was built,
by which `rete` (version, commit, target), on what OS, the command line that
reproduces it, its inputs (name, format, size, BLAKE3), the statement counts,
the wall time per phase and the peak RAM, and the starter queries' measured
costs. Costs two small range
reads (header + one coalesced card/build-info range), never the dictionary or
index — instant on any size. `--json` emits the card object plus
`schemaVersion: 1` and a `build` block (the whole record). `--format jsonld`
projects the card to JSON-LD (VoID + schema.org + PROV-O — already RDF when
lifted out; the build is a `prov:Activity` with `prov:startedAtTime`,
`prov:endedAtTime`, `prov:used` for each input and `prov:wasAssociatedWith`
the engine, the rest under `rete:` as JSON literals);
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

### `rete export <file> [--format nq|ttl|trig|jsonld|hdt] [--graph G] [--subject S] [--predicate P] [--object O] [--sanitize-iris] [--no-prefixes] [--quoted-triple-syntax rdf12|rdf-star] [--compress none|zstd|gzip] [--compress-level N] [--in-memory]`
Serialize the dataset, or a slice of it.

Two formats keep every graph and are lossless round-trips:

- `nq` (the default) — N-Quads, one statement per line.
- `trig` — TriG: Turtle syntax plus `GRAPH <g> { … }` blocks. The same data, in
  fewer bytes, because a subject and its namespaces are written once rather than
  once per statement.

Two carry no graph term and therefore serialize a **single graph**:

- `ttl` — Turtle.
- `jsonld` — expanded JSON-LD.

And one is binary:

- `hdt` — [HDT](https://www.rdfhdt.org/), a compact format that stays
  **queryable without being decompressed**. See below; it is triples-only and it
  has a size ceiling the text formats do not.

`nq`, `ttl` and `trig` stream: peak memory follows `--memory-budget-mb` and the
output is byte-identical at every budget. `jsonld` builds the whole document in
memory, so it is not the format for a file that does not fit in RAM.

```sh
rete export data.rete                   # N-Quads (default, lossless)
rete export data.rete --format trig     # TriG — lossless and smaller
rete export data.rete --format ttl      # Turtle, one graph
rete export data.rete --format jsonld   # expanded JSON-LD, one graph
```

**Which graph a single-graph format writes.** `--graph` if given, otherwise the
default graph. The full ladder, which `ttl` and `jsonld` share and which always
reports its choice on stderr:

| situation | what is exported |
| --- | --- |
| `--graph <iri>` | that graph; an error naming the real graphs if it does not exist |
| `--graph ''` | the default graph, explicitly |
| no `--graph`, default graph has content | the default graph, noting that named graphs were left out |
| no `--graph`, default graph empty, one named graph | that graph |
| no `--graph`, default graph empty, several named graphs | an error listing them, pointing at `--graph` and `--format trig` |

Graphs are never silently merged, and a non-empty selection is never silently
dropped.

**Quoted triples: which surface (`--quoted-triple-syntax`).** A quoted triple —
a statement standing inside another statement — has two spellings in the wild,
and `nq`, `ttl` and `trig` can write either. Both are supported; rete re-ingests
either losslessly. They differ in who *else* can read the dump.

| value | surface | read by |
| --- | --- | --- |
| `rdf12` (default) | `<<( s p o )>>`, the ratified RDF 1.2 **triple term** | oxttl 0.2 and everything on it, incl. the `oxigraph` CLI; Jena 5.x, GraphDB 11 |
| `rdf-star` | `<<s p o>>`, the RDF-star community-group surface — and the token rete stores | oxrdf 0.2 / oxttl 0.1 (the versions rete itself links), and RDF-star mode in older Jena/GraphDB |

The default is RDF 1.2 because the alternative is a dump current tooling will
not take: `oxigraph convert --from-format nq` **rejects** `<<s p o>>` outright,
and a load is atomic, so one such line costs the whole file. In Turtle and TriG
the failure is quieter and worse — an RDF 1.2 parser reads `<< s p o >>` as a
*reifier*, so one statement silently becomes two with a blank node where the
triple term was. Pick the surface your consumer speaks; if you do not know, the
default is the one that fails loudly rather than quietly.

A file with no quoted triples in it is unaffected. The header records in one bit
whether the file holds any, so the check is skipped entirely and the dump is
byte-for-byte the same under either value.

Two edges worth knowing:

- **RDF 1.2 puts a triple term in object position only** (`ttSubject ::= iri |
  BlankNode`). A graph with a quoted triple in *subject* position — legal
  RDF-star, and legal rete — has no RDF 1.2 spelling at all, so the export
  refuses it by name and points at `--quoted-triple-syntax rdf-star`, rather
  than writing a dump no parser will accept.
- **Reading a dump back takes the matching input surface.** `rete build` and
  `rete validate` accept the *same* `--quoted-triple-syntax` flag with the same
  two values (see [above](#quoted-triple-syntax-on-input)), but their default is
  `rdf-star`, not `rdf12` — so a default `--format trig` or `--format ttl` dump
  containing triple terms needs `rete build --quoted-triple-syntax rdf12`. The
  export says so on stderr when it writes one. `--format nq` round-trips with no
  flag in either surface: that reader is rete's own tokenizer and takes both
  spellings unconditionally.

`--format jsonld` and `--format hdt` have no term kind for a quoted triple at
all and **refuse** a file that contains one, naming `--format trig`. They used
to write it as though it were an IRI — a file that loads cleanly and means
something else.

The browser engine's CONSTRUCT writers (`format` `"ttl"` / `"jsonld"`, see
[browser](browser.md)), the desktop explorer's Turtle, and the file
explorer's N-Triples extract all follow the same rule. RDF 1.2 is the
default, `rdf-star` is opt-in (`"ttl;quotedTripleSyntax=rdf-star"`, or
`extract(…, { quotedTripleSyntax: "rdf-star" })`), and a subject-position
quoted triple or JSON-LD is refused rather than mis-written.

**HDT (`--format hdt`).** A binary serialization whose point is that a reader
memory-maps it and answers triple patterns against the mapped bytes. Opening the
1.39 GB reference HDT costs 50.7 MB of RSS and 0.31 s, whatever the file's size —
that is what a compressed text dump cannot do.

```sh
rete export data.rete --format hdt > data.hdt
```

Four limitations, all enforced rather than merely documented:

- **Triples only.** HDT has no graph term, so it takes one graph by the same
  ladder Turtle uses. The quad extension (HDTQ) is niche and poorly supported
  and is not implemented.
- **It cannot stream, and it has a size ceiling.** rete stores IRIs as
  `<http://…>` and HDT stores them bare, which changes the dictionary's sort
  order, so every term id has to be remapped and the triples re-sorted in the new
  order. That needs the graph in memory. `rete export` estimates the cost from
  the file header and **refuses before doing any work** if it exceeds
  `--memory-budget-mb`, naming both numbers. Measured, the estimate is about 512
  bytes per distinct term plus 16 per triple; an 88M-triple, 39.7M-term graph
  needs roughly 14 GB.
- **No quoted triples.** HDT predates RDF-star and RDF 1.2 and has no term kind
  for a triple term. It did not notice: a quoted triple was interned into HDT's
  dictionary as an *IRI*, brackets stripped by the same rule that strips a real
  one's, so a consumer read back an "IRI" with spaces and angle brackets in it.
  `rete export` now refuses, from the header flag, before any work.
- **A second, independent cap: 2^32 object ids.** The reference implementation
  truncates object dictionary ids to 32 bits while building its query index, so a
  larger file would be written successfully and then answer queries *incorrectly*
  on whoever's machine read it. Rather than document that trap, `rete export`
  refuses to produce such a file at all.

Which limit binds depends on the graph's shape: a sparse graph with a huge object
vocabulary hits the id cap, a dense one hits memory.

**HDT is not the format to archive or ship in, and the numbers say so.**
Measured on a 1.5M-triple graph: HDT 14,217,647 bytes against `trig.zst`'s
4,296,486 — HDT is **3.3x larger**, and larger than the source `.rete`
(11,810,101) too. On the 88M-triple DBLP graph the ratio is the same: HDT
1,378,298,174 against `trig.zst`'s 462,563,780, **3.0x larger**.

So pick by what happens to the file next:

| you want to… | use |
| --- | --- |
| store or ship it | `--format trig --compress zstd` — smallest, lossless, no ceiling |
| feed a line-oriented pipeline | `--format nq` |
| **query it in place** | `--format hdt` |

HDT earns its place on the last row only, and that advantage is real and
measured: a reader memory-maps it and answers patterns in ~50 MB of RSS without
decompressing anything, where `trig.zst` has to be decompressed and parsed in
full before it can answer at all.

Above either limit, use `--format trig --compress zstd` — lossless, compact, and
with no ceiling.

`--compress` is rejected with `--format hdt`: wrapping an in-place-queryable
format in a codec removes the only property it has over a compressed text dump.

The first object-bound query a reader runs will build and cache a `.index.v1-1`
next to the file. That is normal, and safe for any file rete produces, because
the object-id cap is enforced at export time.

**Prefix compression (`ttl` and `trig`).** IRIs are abbreviated to QNames —
`rdfs:label` rather than `<http://www.w3.org/2000/01/rdf-schema#label>` — which
is where most of the size advantage over N-Quads comes from. Well-known
vocabularies keep their conventional prefix names; on top of that the exporter
reads a bounded sample of the data (a hundred thousand statements, whatever the
file's size) and names the namespaces that are frequent in *this* file, from
their own last path segment. An IRI is abbreviated only when the part after the
namespace is a legal Turtle `PN_LOCAL` needing no backslash escapes — otherwise
it is written in full, because a smaller file that does not parse is not a
smaller file. `--no-prefixes` writes every IRI in full.

**Compression (`--compress`).** `zstd` or `gzip`, applied as the dump is
written. The codec sits *in* the writer chain, so `--memory-budget-mb` still
bounds peak memory and a dump far larger than RAM still works — nothing is
buffered up to be compressed afterwards.

```sh
rete export data.rete --format trig --compress zstd > dump.trig.zst
rete export data.rete --compress zstd --compress-level 19 > archive.nq.zst
```

Prefer zstd. Measured on a 609 MB N-Quads dump (see the table below), zstd at
its default level produces a file 21% smaller than gzip's default while
compressing 2.4x faster and decompressing 1.5x faster — which is the trade that
matters for a dump written once and read repeatedly.

| codec | bytes | of plain | compress | decompress |
| --- | --- | --- | --- | --- |
| plain | 609,574,054 | 100% | — | — |
| zstd -1 | 31,411,730 | 5.15% | 3.19s | 3.36s |
| zstd -3 | 30,417,765 | 4.99% | 3.12s | 3.27s |
| **zstd -6** (default) | **27,926,890** | **4.58%** | **3.27s** | **3.26s** |
| zstd -9 | 26,718,710 | 4.38% | 3.24s | 3.41s |
| zstd -19 | 23,760,373 | 3.90% | 195.30s | 3.33s |
| gzip -1 | 42,476,408 | 6.97% | 6.51s | 4.86s |
| gzip -6 | 35,249,561 | 5.78% | 7.92s | 4.86s |
| gzip -9 | 32,656,726 | 5.36% | 13.75s | 5.18s |

`--compress-level` defaults to **6** for both codecs. For zstd that is
deliberately not the library's own default of 3: the 3 -> 6 step is the largest
remaining gain on the ratio curve (-8.2%), and everything after it returns less
(-4.4% to level 9, -0.9% to level 12) until level 19, which costs 60x the time
for 10% more. Negative zstd levels are accepted and trade ratio for speed.

**stdout becomes binary** when a codec is selected. Every note and report `rete
export` prints already goes to stderr, so redirecting stdout to a file is safe —
but do not pipe a compressed dump into something expecting text.

Compression applies to the text formats. Decompress with the standard tools;
the output is ordinary zstd and gzip, verified against both CLIs.

**Export streams from disk by default.** The file is opened through the lazy
ranged reader — header, section directories and index up front, then dictionary
chunks and tiles faulted in as the scan reaches them — so the raw file image is
never held resident. The old behaviour read the whole file into memory and
decoded every dictionary chunk and all six index permutations first, so export
RSS scaled ~3 GB per GB of file (6 GB on a 1.53 GB graph; a 52 GB dump would
need ~150 GB and OOM). `--in-memory` forces that whole-file load back — faster
for small files that comfortably fit in RAM, but it needs RAM ≈ the file size.
The dump is **byte-identical** either way: the reader choice changes only how
the bytes are fetched, never the order or content.

**What `--memory-budget-mb N` bounds** (default 4096; `0` = unlimited). The
budget is split between three evicting caches — decoded dictionary chunks, the
scanned permutation's index tiles (N/16, at most 256 MiB) and raw file blocks
(N/8, at most 256 MiB) — and a reserve of N/32 (at most 128 MiB) for what no
cache owns: one window's term strings and in-flight reads. Before the
dictionary gets its share, the open's **fixed directories** are taken out of it:
one descriptor per dictionary chunk and per index tile, held for the whole
export (88 MB on the 14.75 GiB openaire-2021-relation, 224 MB on the 1.5 GB
epfl-infoscience, whose pre-#198 directory stores whole boundary terms). Caches
are charged what the allocator really hands out for each body, not its length,
and the CLI uses jemalloc, which returns freed memory to the OS; with the
system allocator a long export used to keep up to 75% more than its budget in
freed-but-resident memory. Set `RETE_MEM_TRACE=<seconds>` to print RSS, cache
residency and the fixed directories periodically on stderr.

Measured on a full nq export (peak RSS from `/usr/bin/time -v`; 0.3.4 is the
release before this change, on glibc malloc):

| file | budget | peak RSS | 0.3.4 | export time |
| --- | --- | --- | --- | --- |
| openaire-2021-relation (14.75 GiB, 1.8 G quads) | 1024 MB | 1422 MB (1.39×) | | 19 431 s |
| | 4096 MB | 4352 MB (1.06×) | 7256 MB (1.77×), 7 811 s | 10 345 s |
| | 8192 MB | 8452 MB (1.03×) | | 2 067 s |
| datacite (48.6 GiB, 9.8 G quads) | 8192 MB | 8248 MB (1.01×) | 16.7 GB (2.04×), 5 h 16 min¹ | 6 986 s |
| epfl-infoscience (1.53 GiB, 9.8 M quads) | 256 MB | 605 MB (floor) | 790 MB, 50 s | 90 s |
| | 1024 MB | 1234 MB (1.21×) | 1836 MB, 35 s | 46 s |
| | 4096 MB | 3877 MB (0.95×) | 4279 MB, 23 s | 22 s |

¹ The scholar audit's run (with `--sanitize-iris`), not measured side by side.

So peak RSS stays within **N plus ~0.4 GB**, whatever the file size: steady
state is N plus 0.05–0.26 GB (jemalloc's not-yet-purged pages and thread
caches, the decode threads, the binary), and the worst transient seen was
+0.4 GB, once, at 1024 MB on relation. The output is byte-identical at every
budget.

There is one floor: the dictionary share never drops below 64 MiB, so a budget
smaller than the fixed directories plus ~350 MB is not honoured. On
epfl-infoscience that is anything under ~600 MB; there the open itself briefly
holds the raw chunk directory beside the parsed one (about twice the
directory). Rebuilding a pre-#198 file shrinks its directory to a few bytes per
chunk.

A smaller budget costs time, not correctness: the dictionary cache re-decodes
chunks it had to evict, so a file whose decoded dictionary is far larger than
the budget exports more slowly the smaller the budget is — openaire-relation
takes 34 min at 8192 MB, 2.9 h at 4096 and 5.4 h at 1024. A budget also buys
less cache than it did in 0.3.4, which spent memory it did not count: at the
same 4096 MB this build is ~30% slower on that file, and at the same *peak RSS*
(8192 MB here vs 4096 MB in 0.3.4, both ~7–8 GB) it is 3.8× faster.

`--sanitize-iris` percent-encodes IRIs that are outside the N-Triples/N-Quads
grammar (see [Invalid IRIs](#invalid-iris)), so the dump loads into a strict
store instead of being rejected — and a bulk loader rejects the whole *chunk*,
not the line, so one bad IRI is worth a hundred thousand good statements.

It is **off by default because it changes the data.** `<http://ex/a[b]>` is
written as `<http://ex/a%5Bb%5D>`, which is a different IRI: the dump no longer
joins against the graph it came from, `rete → store → rete` stops being the
identity, and if the graph happened to contain both spellings they collapse into
one. Deciding to accept that is the exporter's call, which is why it is a flag.
What it did goes to **stderr** (stdout is the dump), per class:

```
--sanitize-iris: totals invalid=5 repairable=5 unrepairable=0 unclassified=0
--sanitize-iris: percent-encoded 5 IRI occurrence(s). The dump's IRIs are NOT the
                 file's IRIs: it no longer joins against the source graph, and
                 rete → store → rete is no longer the identity.
                       2  '[' or ']' outside an IP-literal host
                       1  more than one '#'
                       …
```

An IRI escaping cannot repair — no scheme, or a defect no class recognises — is
counted, named, and written **verbatim**. The dump is then still not valid
N-Quads and the summary says exactly that rather than implying a fix it did not
make; the `totals` line's `unrepairable` field is the machine-readable form of
that statement. The [triple-store interop](interop.md) page has the measured
Oxigraph round-trip for all three cases.

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

### `rete sparql <file> ("<query>" | --query-file <path>) [--json] [--entail] [--memory-budget-mb N]`
Run SPARQL: `SELECT` / `ASK` / `CONSTRUCT` / `DESCRIBE`. With `--json`, emit
standard SPARQL Results JSON (for `SELECT`/`ASK`). See [SPARQL support](sparql.md).

`--entail` turns on **OWL 2 QL reasoning**: the query is rewritten so the answer
includes ontology-entailed solutions (`rdfs:subClassOf` / `subPropertyOf` /
`domain` / `range` / `owl:inverseOf` / `someValuesFrom`), computed over the raw
data with no materialization — off by default, so a plain query is unchanged. Same
flag on `rete sparql-url` reasons over a remote file, fetching only what the
rewritten query touches. See [Reasoning by query rewriting](reasoning.md#reasoning-by-query-rewriting-owl-2-ql).

**Warnings on stderr.** When a function raises a SPARQL type error, a FILTER
treats it as false and a BIND leaves the variable unbound, so the query can
come back empty with no error. Examples: `CONTAINS(?s, "x")` on an IRI,
`STRSTARTS(?year, "15")` on an `xsd:integer`, and a language-tag mismatch.
`rete sparql` (and `rete sparql-url`) keep those results exactly as they are
and print one `warning:` line per kind of error to **stderr**. Each line gives
the function, the argument, a sample value and a fix such as "wrap it in
STR()". stdout stays the result. With `--json`, the result object also gets a
`warnings` array. An empty result can also get a `hint:` line saying that
string matching is case-sensitive. See
[Type errors and query warnings](sparql.md#warnings).

```text
$ rete sparql maps.rete 'SELECT ?s WHERE { ?s rdfs:label ?l FILTER(CONTAINS(?s, "geneva")) }'
0 solution(s)
warning: CONTAINS received an IRI as argument 1 in 1 row: a SPARQL type error, which makes a FILTER drop the row, even under ! (BIND leaves the variable unbound); e.g. <http://ex.org/map/geneva-1572>. Hint: wrap it in STR() to match the IRI's text.
```

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

**Long queries: `--query-file <path>`.** The operating system caps a command
line: one argument may be at most 128 KiB on Linux, and the whole line 32,767
characters on Windows. A query with a `VALUES` block of a few thousand IRIs
crosses either and fails before rete starts (`Argument list too long`, or a
command line too long). Put the query in a file instead, or pipe it in with
`-`:

```sh
rete sparql data.rete --query-file lookup.rq --json
generate_query | rete sparql data.rete --query-file - --json
```

Every command that takes a query takes `--query-file` too: `sparql`,
`sparql-url`, `federate` (in place of `--query`), `cost`, `progressive`,
`cypher` and `manifest query`. It is an alternative to the query argument,
never an addition: giving both, or neither, is a usage error (exit 2). The
file must be UTF-8 (a leading byte-order mark is ignored); a file that cannot
be read, or holds only whitespace, is an error (exit 1).

**Memory & I/O.** Every command that opens a local file (this one, `stats`,
`search`, `serve`, …) reads it whole only when that is cheap, and otherwise
opens it through the same lazy range reader the `-url` commands use (block
size: `RETE_BLOCK_KB`). Reading whole holds the file image, its decoded
dictionary and every index permutation, none of it evictable: 6.8× the file
on the OpenCitations sample. So a file is read whole only when it is **at most
a sixteenth of the memory available to the process**, never past 1 GiB, and
for `sparql` also at most a quarter of `--memory-budget-mb`. Available memory
is the smaller of the kernel's `MemAvailable` and the headroom under the
process's cgroup limit (v2 `memory.max` or v1 `memory.limit_in_bytes`, page
cache not counted as used), so inside a container the container's limit is
what counts. Where neither can be read (macOS, Windows) the threshold is
256 MiB. `RETE_LOCAL_LAZY_ABOVE_MB` overrides the threshold outright (`0` =
always lazy); `RETE_OPEN_DEBUG=1` prints which way each open went and why.

Measured in a 4 GiB container on the OpenCitations sample: under the fixed
1 GiB threshold of earlier releases, `stats`, `sparql`, `search` and `serve`
on the 1.07 GB file each read it whole, passed 4 GiB and were killed after
24–49 s. Now the same file opens lazily: a one-row `sparql` answers in
0.035 s at 15 MB peak, `search --contains` in 0.11 s at 59 MB, `stats` in
0.53 s at 421 MB (it reads the pyramid's statistics blocks). Its 0.24 GB
sibling is still a small enough fraction to read whole (3.1 s, 1.6 GB). Aggregation
streams through per-group accumulators. Preview a query's byte cost with
`rete cost`, and see the exact ranges a query read with `rete why`.

**`--memory-budget-mb N`** (default 4096; `0` = unlimited) bounds what the
lazy reader keeps resident: the decoded dictionary chunks, the decoded index
tiles with their group directories, and the raw file blocks, split as for
`rete export` (see "What `--memory-budget-mb N` bounds" below). Least recently
used entries are evicted and re-read on demand, so the budget changes memory
and speed, never the answer. Two per-query memos (decoded terms, parsed
numbers) are capped at 64 MiB each on top, and `COUNT(DISTINCT …)` keeps at
most an eighth of the budget in memory, spilling sorted runs to `TMPDIR` past
it (exact either way). What the budget does **not** cover is the rest of the
query's own working state: hash-join build sides, `GROUP BY` groups,
`SELECT DISTINCT` sets and an unbounded `ORDER BY`, which grow with the data
the query asks for (see [Memory budget](sparql.md#memory-budget)). Measured on the 49 GB
DataCite file (9.53 B triples): `GROUP BY ?p` over the whole graph went from an
out-of-memory kill at 16 GB to 876 s at 1.1 GB peak RSS, and a `CONTAINS` count
over every title from 15.1 GB to 4.0 GB (the default budget, filled by the
title dictionary chunks).

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
- A query whose expressions raised SPARQL type errors (a FILTER drops those
  rows silently; see [query warnings](sparql.md#warnings)) gets two response
  headers: `Rete-Warnings`, the warnings as a JSON array (ASCII-escaped), and
  `Rete-Warning-Count`. Headers, not a body member: the result document stays
  standard SPARQL JSON or N-Triples, which any client parses unchanged. A
  query with nothing to report gets neither header.
- Binds loopback by default. Expose deliberately (`--bind 0.0.0.0:7878`) and
  set `--token`: updates then require `Authorization: Bearer <t>` (reads stay
  open). CORS is enabled, so browsers can query it directly.
- Scale: until the first update the endpoint answers **straight from the
  file**, opened like any local command opens it (read whole only when it is a
  small fraction of the free memory, lazily otherwise; see `rete sparql`'s
  Memory & I/O), and `GET /snapshot.rete` streams the file itself. So serving
  a multi-GB catalog file read-only costs what its queries touch. Before
  0.4.0 the server loaded every quad at startup: in a 4 GiB container it was
  killed before answering on both the 1.07 GB and the 0.24 GB OpenCitations
  files (18.9 M quads each); now it answers its first query in 0.06 s at
  16 MB, and in 3.0 s at 1.6 GB on the smaller file, which it reads whole. The first update (or a
  journal with changes to replay at startup) loads every quad into memory and
  rebuilds after each write, which is built for living small/medium datasets
  (annotation, curation; up to a few million triples). An update whose load
  would not fit is refused with `503` and a message, and the endpoint keeps
  answering queries. The estimate is 1 KiB per quad against the available
  memory (measured: 646 MiB at peak for 1 M OpenCitations quads), so a graph
  of very long literals can still need more than it predicts.

```sh
rete serve notes.rete
curl -s --data-urlencode 'query=SELECT * WHERE { ?s ?p ?o } LIMIT 5' http://127.0.0.1:7878/sparql
curl -s --data-urlencode 'update=INSERT DATA { <http://ex/n1> <http://ex/note> "hello" }' http://127.0.0.1:7878/sparql
curl -sO http://127.0.0.1:7878/snapshot.rete   # the updated companion .rete
```

### `rete cost <file-or-url> ("<query>" | --query-file <path>) [--json] [--explain]`
Preview the byte/range-request cost of a SPARQL query without evaluating it.
The report parses the query, lists the concrete predicates that can drive
summary-based routing, and compares three access paths:

- **summary overview** — header + dictionary + pyramid summary, skipping the
  triple index.
- **routed pattern open** — for a single default-graph triple pattern, header +
  dictionary + the one selected permutation payload (the best of the six).
- **full query open** — the eager open, which reads the dictionary + index
  (+ pyramid, text-index and named-graph sections when present) before
  evaluation. Its figure is computed from the header, not by performing the
  open: previewing a large file no longer reads it whole (a 26 GB file used
  to be killed at 2 GB).

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

### `rete progressive <file-or-url> ("<query>" | --query-file <path>) [--json]`
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

### `rete cypher <file> ("<query>" | --query-file <path>) [--base <iri>] [--json]`
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

**How the ranges are fetched.** On a remote file the time goes to round trips,
not bytes, so the HTTP reader behind every `-url` command (and `cost`,
`progressive`, `card-audit`, `reason --url`, `shacl-url` and `federate` on a
URL):

- keeps its connections alive: one client with a connection pool, where each
  range used to open (and on `https://` TLS-handshake) a connection of its own;
- fetches the engine's **batches in parallel**, up to 16 requests in flight —
  the tiles a scan or a join step is about to read, the dictionary chunks a
  result decodes (all four dictionary sections in one batch), the subjects a
  full-text search returns — so a batch costs one round trip instead of one
  per range; before 0.4.0 the CLI passed every batch on one range at a time;
- walks the dictionary and the index containers **at the same time** while
  opening a file: each is a chain of small dependent reads (a container's
  sections are found through their length prefixes), and the header says
  where both start;
- merges ranges of one batch that lie at most 32 KiB apart into one request
  (never past 8 MiB), and slices them apart again;
- retries a transient failure (a dropped connection, a short body, `408`,
  `429`, a `5xx`) twice with backoff, and fails at once on a permanent one: a
  host that ignores `Range` (`200` for a range request), a `Content-Range` for
  the wrong offset, a `404`. One range failing fails the batch, and a query
  never returns a partial answer as if it were whole.

Knobs: `RETE_HTTP_CONCURRENCY` (requests in flight, 1–64, default 16; `1`
fetches one range at a time, the behaviour before 0.4.0),
`RETE_HTTP_COALESCE_KB` (default 32; `0` merges only touching ranges) and
`RETE_HTTP_RETRIES` (default 2). The `fetched … in N range request(s)` line
counts the ranges handed to the HTTP reader (after the block cache has merged
adjacent blocks); when the reader merges two nearby ones into one request the
server sees one fewer, and retries are not counted.

Measured through a latency-injecting proxy on the OpenCitations sample: see
the CHANGELOG entry for 0.4.0 for the per-query numbers at 20 and 80 ms RTT.

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

**The curated `example_queries` are audited too, and reported apart** — under
`example_findings` in the JSON and in a section of their own in the text, with
the ids `example-1`, `example-2`, … and `revision: "curated"`. A curated query
makes no claim, so the card decides only what it can show: `invalid` (it does
not parse), `empty` (it reads only the default graph of a file whose default
graph the card records as empty), `suspect` (it names a predicate the card's
complete predicate list does not contain — not `empty`, since the predicate may
sit in an `OPTIONAL` or `UNION` branch), and otherwise `undecidable`.
`--measure` runs them like the starters (and `--only example-2` picks one);
their costs are reported beside, never stored by `--write-costs`, whose cost
list is the starters'.

```sh
rete card-audit https://host/data.rete
rete card-audit card.json --json | jq '.findings[] | select(.verdict=="empty")'
rete card-audit data.rete --json | jq '.example_findings[] | select(.verdict!="undecidable")'
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

Where that is worth it: when the queries answer and only the record is
missing. When they do not, re-card instead — [`rete recard`](#rete-recard-file--o-outrete----in-place---card-file-f----keep-curated---title----no-card-costs---memory-budget-mb-n---verify)
is the same single rewrite with a new card in it, at any size.

**The RAM goes into the measurement, not the rewrite**: the engine evaluates
eagerly, so a starter query with a big result materializes it. A card written
by an older build can carry unbounded starters — `switzerland-fedlex` (1.04 GB,
56.3 M quads) peaked at **14.2 GiB** for `--measure --write-costs`, 3.2 GiB of
it `ng-list`'s 497,905 rows — so budget for those queries (or `--max-mb` them),
not for the file. A card generated since 0.4.0 carries only bounded starters
([Dataset Cards](dataset-cards.md#bounded-by-construction)).

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
per posting list. The bare prefix mode reads the LABEL_INDEX section — its head
(preamble and root, one range of at most 64 KiB), one directory node per level and
the data blocks it needs — and on such a file the dictionary is **not opened at
all**, because the section stores each subject's term. On an older file it faults
the pyramid instead, where the legacy label block lives.

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

### `rete sparql-url <url> ("<query>" | --query-file <path>) [--json] [--entail] [--memory-budget-mb N]`
Run a full SPARQL query over HTTP with **lazy tile faulting** (tiled
files): the open fetches the header, dictionary, pyramid, and the index's
small tile directories; index tiles are then range-fetched only when the
query's scans and probes touch them, so a selective query reads O(touched
tiles) rather than the whole index. A range failure mid-query is reported as
an error, never as silently fewer rows. `--memory-budget-mb` bounds the
block, dictionary and tile caches exactly as for `rete sparql`.

```sh
rete sparql-url https://host/data.rete \
  "PREFIX e: <http://ex/> SELECT ?d WHERE { ?d e:dependsOn+ e:log4x }"
rete sparql-url https://host/data.rete --query-file lookup.rq   # a long query
```

## Federation

### `rete federate <sources…> (--query "<SPARQL>" | --query-file <path>) [--json] [--no-route]`
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
rete federate shard-*.rete --query-file lookup.rq --json    # the query from a file
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
rete manifest query  g.rete-manifest.json "SELECT … WHERE { … }"   # or --query-file q.rq
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
