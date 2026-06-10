# Getting started

## Everything runs in Docker

`rete` is developed and built **entirely inside a container** — nothing runs on
the host. The dev container ([`.devcontainer/`](../.devcontainer)) carries the
Rust 1.92 toolchain, rustfmt, clippy, Python, the `wasm32-unknown-unknown`
target, and `wasm-pack`.

Open the folder in a dev container (VS Code: *Reopen in Container*), or run the
same image directly:

```sh
docker build -t rete-dev -f .devcontainer/Dockerfile .
docker run --rm -it -v "${PWD}:/work" -w /work rete-dev bash
# then, inside:
cargo build --release -p rete-cli
```

The compiled CLI is at `target/release/rete`. The examples below assume it is on
your `PATH` (or substitute `cargo run -p rete-cli --`).

## Building a `.rete` file

<figure class="fig-right">
  <img src="img/build-pipeline.svg" alt="A pipeline: .nt, .ttl and .nq inputs feed into 'rete build', which produces one social.rete file containing a dictionary, indexes and a pyramid, ready to put on an HTTP host or URL.">
  <figcaption><code>rete build</code> packs your triples into one immutable file — dictionary, permutation indexes, and a community pyramid — that you can drop on any URL.</figcaption>
</figure>

`rete build` accepts N-Triples (`.nt`), N-Quads (`.nq`), and Turtle (`.ttl`),
detected by extension. Multiple inputs are merged under one shared dictionary,
and `-` reads standard input.

```sh
rete build data.nt -o data.rete                  # single file
rete build part1.nt part2.nt -o merged.rete      # merge several inputs
curl -s https://host/data.nt | rete build - -o data.rete   # from a pipe
rete build dump.unknown --format nt -o out.rete  # force a format
```

N-Quads inputs build a **dataset**: one shared dictionary, a default-graph index,
and one index per named graph.

## Querying locally

```sh
# Triple pattern — any of subject/predicate/object may be omitted (a wildcard):
rete query data.rete --predicate '<http://ex/knows>'
rete query data.rete --object   '<http://ex/Alice>'

# Basic Graph Pattern (multi-pattern join); ?x is a variable, ' . ' separates:
rete bgp data.rete "?x <http://ex/knows> ?y . ?y <http://ex/knows> ?z"

# Full SPARQL (SELECT / ASK / CONSTRUCT / DESCRIBE):
rete sparql data.rete "PREFIX e: <http://ex/> SELECT ?x ?z WHERE { ?x e:knows ?y . ?y e:knows ?z }"

# Standard SPARQL Results JSON, for piping into other tools:
rete sparql data.rete "SELECT ?s ?p ?o WHERE { ?s ?p ?o } LIMIT 10" --json
```

See the [SPARQL support](sparql.md) page for the full feature list.

## Validating shapes

Use `rete shacl` when you need semantic data-quality checks, not just syntax or
file integrity. Shapes are Turtle files; the command exits non-zero if the graph
does not conform.

```sh
rete shacl data.rete --shapes shapes.ttl
rete shacl data.rete --shapes shapes.ttl --format json
```

See [SHACL validation](shacl.md) for the supported SHACL Core surface.

## Inspecting a file

```sh
rete info   data.rete   # raw header
rete stats  data.rete   # size, counts, named graphs, top predicates
rete verify data.rete   # check the blake3 content hash (detect corruption)
rete graphs data.rete   # list named-graph IRIs
rete export data.rete   # dump back to N-Quads (lossless)
```

Two **coarse-graph** views answer questions without reading the triple index:

```sh
rete summary data.rete   # structural: Louvain community quotient graph
rete schema  data.rete   # semantic: relations between rdf:type classes
rete predicates data.rete  # exact per-predicate totals, from the summary alone
```

## Deploying & querying over a URL

A `.rete` file is immutable and self-describing, so any static host that honors
HTTP `Range` requests works — S3, GCS, GitHub, a CDN, or the bundled dev server:

```sh
# Local range-capable server for testing:
python3 scripts/range_server.py 8000 .
rete query-url http://127.0.0.1:8000/data.rete --object '<http://ex/Dave>'

# Real https hosts (rustls; no http-only limitation):
rete query-url   https://my-bucket.s3.amazonaws.com/data.rete --predicate '<http://ex/knows>'
rete summary-url https://raw.githubusercontent.com/me/repo/main/data.rete
```

`query-url` resolves bound terms from the dictionary, then fetches only the
selected SPO/POS/OSP permutation payload for the triple pattern. `summary-url`
reads just the header, dictionary, and summary — **the index is never
downloaded**. The host **must** return `206 Partial Content` to a `Range`
request; a host that ignores `Range` (returns `200`) is rejected with a clear
error rather than silently returning wrong bytes.

## Generating synthetic test data

`scripts/synth_graph.py` generates a realistic scholarly knowledge graph —
papers, authors, venues, institutions, grants, fields — with the statistics
real graphs have (power-law citations via preferential attachment, field
communities, Zipfian venue popularity, log-normal team sizes, typed literals,
per-year temporal structure). Two orthogonal knobs control it on demand:

```sh
# 10k papers (~315k triples), clean:
uv run python scripts/synth_graph.py --papers 10000 -o clean.nt

# Same size, 20% deliberate mess (cross-field rewires, temporal violations,
# missing attributes, mangled literals) — for robustness/quality testing:
uv run python scripts/synth_graph.py --papers 10000 --noise 0.2 --seed 7 -o messy.nt

# N-Quads with one named graph per publication year:
uv run python scripts/synth_graph.py --papers 5000 --quads -o by-year.nq

rete build clean.nt -o clean.rete
```

Identical arguments + seed reproduce the byte-identical graph; different seeds
give natural variability at the same size/noise point. A per-kind breakdown of
every noise event goes to stderr, so a test knows exactly what mess it got.
(`scripts/gen_graph.py` is the older, simpler social-graph generator used by
`scripts/bench.sh`.)

### Scaling to ~1 GB

The generator and `rete build` scale linearly, so a big stress-test graph is
just a bigger `--papers`. Output is roughly **85 bytes/triple** as N-Triples
and **31 triples/paper**, so ~1 GB is about 400k papers / 12.5M triples:

```sh
uv run python scripts/synth_graph.py --papers 400000 --seed 1 -o big.nt  # ~1.1 GB, ~22 s
rete build big.nt -o big.rete                                            # ~56 s -> ~100 MB
```

Measured on the dev container (12.5M triples, 2.0M terms, ~30k communities):
build is ~56 s and the `.rete` is ~100 MB (zstd). The point of the size is what
querying it then *doesn't* read: a selective pattern answers in under a second,
`rete predicates` reads ~20 MB of summary rather than the 80 MB index, and a
lazy query open (`rete cost big.rete "<query>"`) touches ~7 MB in ~50 range
requests instead of the whole file — the range-query promise, at 1 GB.

The playground's `scholar` / `scholar-noisy` demo datasets are built with this
generator (250 papers, seed 42, noise 0 and 0.25 — the exact commands are in
the `scripts/build_playground.py` docstring).

## A real-world graph: a Wikidata biology slice

For a genuinely large, real dataset, `scripts/fetch_wikidata_bio.py` pulls a
life-sciences slice from the [Wikidata Query Service](https://query.wikidata.org):
genes, the proteins they encode, the diseases they associate with, drugs that
treat those diseases, and a disease subclass hierarchy — one connected graph,
every entity labelled in English. It runs a handful of bounded `CONSTRUCT`
queries (each well under the WDQS timeout) and merges them as N-Triples.

```sh
uv run python scripts/fetch_wikidata_bio.py --limit 4000 -o data/wikidata-bio.nt
rete build data/wikidata-bio.nt -o bio.rete
rete stats bio.rete        # ~40k triples, ~27k terms, hundreds of communities
```

A `--limit 4000` run is roughly 40,000 triples (≈2,800 genes, ≈4,000 proteins,
≈3,600 diseases) — the community pyramid finds hundreds of organism/disease
clusters, and it exercises every surface: typed-class queries, label joins, the
disease hierarchy via `wdt:P279`, and HTTP range queries over a real graph.
`--taxon Q83310` fetches mouse instead of human; `--limit` trades size against
WDQS time. Output lands in `data/` (git-ignored, like all fetched datasets —
the script is tracked, the bytes are regenerated on demand). Be a good WDQS
citizen: it is rate-limited, so fetch a slice, not a firehose.

## Testing

```sh
cargo test            # full suite (unit, round-trip, robustness, ranged, HTTP)
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/smoke.sh # end-to-end acceptance test of every CLI subcommand
```

CI runs all of this — plus the feature matrix and the wasm build — in containers,
so nothing ever builds on the host.
