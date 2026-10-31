# `rete build` reference

`.rete` stable format generation **1** (header byte `0x05`) is immutable and
content-hashed, with a 1 KB typed section directory (dictionary, permutation index,
schema/community pyramid, optional text index,
embedded Dataset Card), all HTTP-range-readable. The index carries 6 permutations
by default and 3 with `--permutations 3`; the header records which.

## The Docker-only build

The project builds in the `rete-dev` image against the compiled binary at
`/work/target/release/rete` (it is NOT on PATH). The `scripts/rete` wrapper does
this for you; raw form:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build /work/data/foo/foo.nt -o /work/data/foo/foo.rete \
  --pyramid-algo types --label-index --card
```

(`MSYS_NO_PATHCONV=1` is only needed on Windows Git-Bash so `/work` isn't mangled.)
The image must have the repo compiled — `cargo build --release` inside it if the
binary is stale/missing.

## All `build` flags

| Flag | When |
|---|---|
| `-o, --output <FILE>` | required output `.rete` |
| `<INPUTS>...` | one or more RDF inputs (merged); `-` reads stdin |
| `--format nt\|nq\|ttl\|rdfxml` | force input format (else by extension) |
| `--pyramid-algo louvain\|types` | community pyramid algo. **`types`** = partition by `rdf:type`: deterministic, parallelizable, self-naming, falls back to louvain when untyped. `louvain` (default) = topological modularity, single-threaded, byte-identical. Prefer `types` for typed graphs and anything large. |
| `--no-pyramid` | skip the pyramid entirely — smaller file, still fully queryable (SPARQL/SHACL/triple/reach don't use it). Only community/summary/progressive queries need it. |
| `--text-index` | full-text word index over literals → `rete search --contains <word>`. Large on text-heavy graphs; off by default. |
| `--label-index` | the LABEL_INDEX section → `rete search <prefix>` / the playground's entity search: every labelled subject (people as "family, given"), no cap, a few small range reads per search. **Pass it for a publishable dataset** — labels are no longer inside the pyramid, so without it there is no label index. Works with `--no-pyramid` and `--memory-budget-mb`. |
| `--no-schema` | leave out the SCHEMA section (class hierarchy, level rollups, planner `query_stats`, characteristic sets), which every build writes by default — a few KB, no pyramid needed. |
| `--type-predicate <FULL-IRI>` | override the typing predicate for the schema pyramid. **Use the full IRI** (`http://www.wikidata.org/prop/direct/P31`), never a prefix — a prefix yields "0 typed classes". |
| `--card` | embed a Dataset Card (counts, top predicates/classes, vocabularies + curated fields). Always pass for a publishable dataset. |
| `--title / --license / --source / --description / --created` | curated card fields (each implies `--card`). |
| `--card-file <json>` | JSON of curated card fields (implies `--card`). Publisher-defined custom fields go inside its `extra` object (bounded: 8 KB serialized, ≤64 keys, nesting ≤2); unknown TOP-LEVEL keys are rejected loudly — see `docs/dataset-cards.md`. |
| `--memory-budget-mb <N>` | **Memory-bounded external build**: chunk the input to disk and merge, holding ~N MiB in RAM regardless of graph size; the budget decides the chunk count and sort-run sizes. Byte-identical to a standard `--no-pyramid` build (the SCHEMA section and `--label-index` included). PROVEN at 1.3B triples: ORCID → ONE 17.5 GB .rete @ 16 GiB budget (37 chunks, ~2.5 h). v1: .nt/.nq only (files or stdin `-` with explicit `--format` — the single input pass makes pipes valid), default graph only, no pyramid/reasoning. A card is the full one (profile + measured starter queries, derived from the finished file as `rete recard` does, written after the data). Since 0.4.0 it takes `--text-index` (any `--text-index-graphs/-include/-exclude` scope) and writes the same TEXT_INDEX bytes as the in-RAM build. Spill dir via `--tmp-dir`. |
| `--tmp-dir <dir>` | Where `--memory-budget-mb` puts its spill files (default: alongside the output). |
| `--permutations 3\|6` | How many index permutations to store. **Default 6 — leave it alone for anything published.** `3` writes SPO/POS/OSP only: same rows, same routing, same tiles for every query, but no sort-merge join (the planner hash-joins instead) and **older readers refuse the file outright** (`malformed container: expected 6 permutation sections`, exit 1). The three dropped orders are 36.8%–50.5% of a built file. Consider `3` only for a private/self-hosted, index-dominated dataset whose workload is lookup-and-follow rather than `?s <p1> ?o1 . ?s <p2> ?o2` subject stars — and measure with `rete cost` first. |
| `--materialize` | bake RDFS/OWL-RL entailments into the file at build time (aborts if incoherent). |
| `--reason` | run the reasoner and stamp the coherence verdict into the card (implies `--card`; does NOT abort on incoherence). Verify later with `rete reason --verify-card`. |

### A good default for a publishable typed graph

```bash
rete build foo.nt -o foo.rete \
  --pyramid-algo types --label-index --card \
  --title "Foo KG" --license "CC0-1.0" --source "https://example.org" \
  --description "What this graph is."
# add --text-index if content search matters; --type-predicate <IRI> if not rdf:type
```

## Large graphs / out-of-memory

A monolithic build holds the dictionary + index in RAM (the pyramid is the biggest
section). Escalating levers:

1. `--pyramid-algo types` (parallel) and/or `--no-pyramid`.
2. The streaming/two-pass ingest path (lower peak RAM than in-memory).
3. **`--memory-budget-mb` external build** — the default answer for a huge SINGLE
   file now that it's proven at 1.3B triples / 397M terms (ORCID → one 17.5 GB
   .rete inside a 16 GiB budget). One file beats shards for UX (one URL, one card,
   real cross-entity BGP joins); shards still win when you need per-shard
   parallel builds or per-part re-publishing.
4. **Shard** when one file won't fit or v1 limits bite (the pyramid): split the N-Triples by subject into ~1–2 GB shards, build each
   with `--no-pyramid` in parallel, and ship a folder + a JSON manifest. The
   dictionary law: cross-shard joins are term-level (string) joins, so shard by
   **subject/entity** to keep star-queries inside one shard. Model:
   `data/scripts/standalone/build_databnf_shards.sh`, `data/scripts/standalone/build_biblissima_shards.sh`.

### Many small named graphs (per-entity provenance)

An N-Quads dump with one graph per entity (OpenCitations' `<entity>/prov/`, a
graph per record) is the case `--named-graphs quad` (0.4.0) exists for: the
per-graph layout pays ~650 B of framing per graph and a remote `GRAPH <g>` walks
the section to reach `g`; the quad index stores the quads once in shared tiles
and finds a graph by binary search. Same rows from every query. It works on
every build path (`--memory-budget-mb` included). Readers before 0.4.0 refuse
the file, so use it where the consumers are 0.4.0 or later. Few, large graphs
(a TriG export of a handful of datasets) gain little; `--collapse-graphs` is
still the choice when the graph names carry no meaning.

### External-build playbook (billion-triple single file)

Hard-won operational rules — model `data/scripts/orcid/build_single_rete.sh`:

- **Two robust phases, never one host pipeline.** Emit the N-Triples to a file on
  the spill drive first (~10× faster than piping into an attached container),
  then run the build as a **DETACHED container** (`docker run -d --name x`): an
  attached `--rm` container dies with its host process tree — one 4 h build was
  lost to a bash bug in the pipeline that launched it. Add
  `--oom-score-adj -500` so a busy Docker VM OOM-kills something else first.
- **Spill sizing**: the spill (`--tmp-dir`) lives in a
  fresh `.rete-extbuild-<pid>-<seq>-<random>` SUBDIR of `--tmp-dir`, so
  builds can share one `--tmp-dir` (up to 0.4.2 two containers, both pid 1,
  shared a directory and deleted each other's spill); a directory a killed
  build left behind is never reused, so delete it once no build is running. Every spill file is
  compressed (delta-varint id runs, front-coded terms, zstd blocks) and the
  six permutations are chained, so the statements are on disk once at a
  time. The peak is now usually in the last permutations, and most of it is
  the finished index itself: plan for about **the output's size plus 2–4 B
  per statement** (plus the staged NT, if you put it there). Measured: the
  OpenCitations Meta sample peaked at 76 MB instead of 245 MB, eight renamed
  copies of it at 0.51 GB instead of 1.98 GB; up to 0.4.1 a build needed
  24 B per statement for `global.tri` and one permutation's runs alone (Meta
  v9.2.1: 230 GiB). The id remap tables live there too, not in RAM (0.4.0
  and earlier held them resident: 15 GB for 1.7 B terms). The final write
  deletes each section once it is in the output, so spill and output do not
  stack. `RETE_SPILL_ZSTD_LEVEL` (default 1; `0` = no zstd) trades CPU for
  disk.
- **Crash resume**: the spill survives a kill. A build killed in the
  permutation phase (after the first permutation) is resumable: the merged
  dictionary and every finished permutation's `<PERM>.tiles` are reusable,
  and the last finished one holds every statement, sorted — the
  `resume_from_spill` harness (`cargo test -p rete-core --release --lib
  extbuild::tests::resume_from_spill -- --ignored`, driven by
  `RETE_RESUME_SPILL/OUT/TERMS/QUADS/CARD[/PERMS/BUDGET_MB]`) re-reads it,
  re-runs the cut-short permutation and the ones after it, and writes the final
  file. Not resumable: a kill in the first permutation (its runs are consumed
  as they are merged; up to 0.4.1 a raw `global.tri` survived) or during the
  final write. Resume with a smaller `RETE_RESUME_BUDGET_MB` if the machine is
  busier now.
- **Verify/query the output LAZILY**: `rete verify` and plain `rete sparql` read
  the whole file into RAM (17.5 GB file OOM-killed a capped container) — use
  `rete sparql-url <local path or URL>` / `card-url` / `query-url`, which accept
  local paths and do the same lazy tile-faulting as HTTP (~30–60 MB per
  selective query). Give `verify` an uncapped container.
- **No pyramid in v1**: catalog examples and docs must steer to SELECTIVE
  queries (one subject, one bound object); a whole-graph aggregate scans the
  file. `rete repyramid` at this scale is untested — don't promise it.

The full memory write-up is in the repo's dev notes (low-mem large build).

## Post-build maintenance

- `rete repyramid <file>` — rebuild the pyramid in place (e.g. to add a schema
  pyramid to a file built before it existed), reading triples straight from the
  file (no `export | build` round-trip).
- `rete export <file> nq|ttl|jsonld` — serialize back out (nq is lossless;
  ttl/jsonld are default-graph only).
- `rete card <file>` / `rete card-url <url>` — read the embedded card (the second
  fetches ONLY the header + card range over HTTP — the index-free self-description).
