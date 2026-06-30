# `rete build` reference

`.rete` is format **v0.4**: immutable, content-hashed, a 1 KB typed section directory
(dictionary, 6-permutation index, schema/community pyramid, optional text index,
embedded Dataset Card), all HTTP-range-readable.

## The Docker-only build

The project builds in the `rete-dev` image against the compiled binary at
`/work/target/release/rete` (it is NOT on PATH). The `scripts/rete` wrapper does
this for you; raw form:

```bash
MSYS_NO_PATHCONV=1 docker run --rm -v "$PWD:/work" -w /work rete-dev:latest \
  /work/target/release/rete build /work/data/foo/foo.nt -o /work/web/foo.rete \
  --pyramid-algo types --card
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
| `--pyramid-algo louvain\|types` | community pyramid algo. **`types`** = partition by `rdf:type`: deterministic, parallelizable, self-naming, still emits planner `query_stats`; falls back to louvain when untyped. `louvain` (default) = topological modularity, single-threaded, byte-identical. Prefer `types` for typed graphs and anything large. |
| `--no-pyramid` | skip the pyramid entirely — smaller file, still fully queryable (SPARQL/SHACL/triple/reach don't use it). Only community/summary/progressive queries need it. |
| `--text-index` | full-text word index over literals → `rete search --contains <word>`. Large on text-heavy graphs; off by default. |
| `--type-predicate <FULL-IRI>` | override the typing predicate for the schema pyramid. **Use the full IRI** (`http://www.wikidata.org/prop/direct/P31`), never a prefix — a prefix yields "0 typed classes". |
| `--card` | embed a Dataset Card (counts, top predicates/classes, vocabularies + curated fields). Always pass for a publishable dataset. |
| `--title / --license / --source / --description / --created` | curated card fields (each implies `--card`). |
| `--card-file <json>` | JSON of curated card fields (implies `--card`). |
| `--materialize` | bake RDFS/OWL-RL entailments into the file at build time (aborts if incoherent). |
| `--reason` | run the reasoner and stamp the coherence verdict into the card (implies `--card`; does NOT abort on incoherence). Verify later with `rete reason --verify-card`. |

### A good default for a publishable typed graph

```bash
rete build foo.nt -o foo.rete \
  --pyramid-algo types --card \
  --title "Foo KG" --license "CC0-1.0" --source "https://example.org" \
  --description "What this graph is."
# add --text-index if content search matters; --type-predicate <IRI> if not rdf:type
```

## Large graphs / out-of-memory

A monolithic build holds the dictionary + index in RAM (the pyramid is the biggest
section). Escalating levers:

1. `--pyramid-algo types` (parallel) and/or `--no-pyramid`.
2. The streaming/two-pass ingest path (lower peak RAM than in-memory).
3. **Shard** when one file won't fit: split the N-Triples by subject into ~1–2 GB
   shards, build each with `--no-pyramid` in parallel, and ship a folder + a JSON
   manifest. Each shard builds with today's streaming ingest; the playground/CLI
   federate across them. The dictionary law: cross-shard joins are term-level
   (string) joins, so shard by **subject/entity** to keep star-queries inside one
   shard. Model: `scripts/build_databnf_shards.sh`, `scripts/build_biblissima_shards.sh`.

The full memory write-up is in the repo's dev notes (low-mem large build).

## Post-build maintenance

- `rete repyramid <file>` — rebuild the pyramid in place (e.g. to add a schema
  pyramid to a file built before it existed), reading triples straight from the
  file (no `export | build` round-trip).
- `rete export <file> nq|ttl|jsonld` — serialize back out (nq is lossless;
  ttl/jsonld are default-graph only).
- `rete card <file>` / `rete card-url <url>` — read the embedded card (the second
  fetches ONLY the header + card range over HTTP — the index-free self-description).
