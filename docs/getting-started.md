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

`query-url` fetches only the byte ranges a query needs; `summary-url` reads just
the header, dictionary, and summary — **the index is never downloaded**. The host
**must** return `206 Partial Content` to a `Range` request; a host that ignores
`Range` (returns `200`) is rejected with a clear error rather than silently
returning wrong bytes.

## Testing

```sh
cargo test            # full suite (unit, round-trip, robustness, ranged, HTTP)
cargo clippy --workspace --all-targets -- -D warnings
bash scripts/smoke.sh # end-to-end acceptance test of every CLI subcommand
```

CI runs all of this — plus the feature matrix and the wasm build — in containers,
so nothing ever builds on the host.
