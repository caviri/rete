# rete.mcpb — the rete engine as a Claude Desktop extension

A one-click [MCP Bundle](https://github.com/modelcontextprotocol/mcpb) that puts
the whole `.rete` engine — compiled to WebAssembly — on the user's machine, so
Claude can query knowledge graphs **locally and lazily**:

- **Your own graphs.** Grant a folder; every `.rete` in it becomes queryable
  with SPARQL 1.1. Files are read **byte range by byte range**, so a
  multi-gigabyte graph answers a selective query in megabytes and never has to
  be loaded into memory. No network involved.
- **The published catalogue.** 78 public graphs read the same lazy way over
  HTTP Range, with no server in between.
- **Building.** Turn Turtle/N-Triples into a real `.rete` file offline, then
  query it immediately.
- **Quality.** SHACL Core validation and OWL 2 QL reasoning, both lazy.

Every tool result carries `stats` — the bytes and requests actually read.

## Why this format fits MCPB unusually well

MCPB's own guidance warns that Python bundles "cannot portably bundle compiled
dependencies" and that binaries need a per-platform build. The rete engine is
Rust **already compiled to architecture-neutral wasm**, so this is a plain
`node` bundle — two JS files plus one `.wasm` — that runs unchanged on macOS,
Windows and Linux with the Node runtime Claude Desktop ships.

```
build/
  manifest.json
  icon.png
  server/
    index.mjs              1.3 MB   server + MCP SDK + rete-graph client, bundled
    worker.mjs             0.1 MB   the query worker (rete-graph client), bundled
    rete_wasm_bg.wasm      3.8 MB   the engine, loaded by each thread
    catalog.json           0.8 MB   published-dataset catalogue
```

No `node_modules` in the archive, no native modules, nothing to install.

## Tools

| Tool | What it does |
| --- | --- |
| `list_datasets` | Local files + published catalogue, optionally filtered |
| `dataset_card` | The file's embedded self-description (2 small reads, any size) |
| `dataset_schema` | Classes and relations with counts, from the baked pyramid |
| `example_queries` | Runnable SPARQL the dataset ships with |
| `sparql_query` | SELECT / ASK / CONSTRUCT / DESCRIBE, `reason=true` for OWL 2 QL; waits up to 40 s, then answers with a `job_id` while the query keeps running. A FILTER type error (CONTAINS on an IRI, …) comes back as `warnings` plus a plain-language line |
| `start_query` | The same, without waiting: a `job_id` at once. Both take `partition_by`, which runs a long extraction in resumable chunks |
| `query_status` | Waits up to 45 s for a job; state, reads, bytes, elapsed; the first rows once done. No `job_id`: every job held |
| `query_result` | A finished job's rows by cursor (up to 1000 a page), or all of them to a `.csv` / `.tsv` / `.json` / `.jsonl` / `.nt` file in a granted folder (`save_as`; never overwrites) |
| `cancel_query` | Stops a job by terminating its worker |
| `find_entities` | Label-prefix + full-text search → entity IRIs |
| `describe_entity` | Every statement about one IRI, both directions |
| `validate_shacl` | SHACL Core report; only the shapes' targets are read |
| `build_rete` | RDF text (Turtle, TriG, N-Triples, N-Quads) → a new `.rete` in a granted folder, offline; `quoted_triple_syntax: "rdf12"` reads RDF 1.2 Turtle/TriG (default `rdf-star`, as `rete build`) |
| `load_rdf` | RDF shared in the chat → a graph held in memory, `chat:<name>`, no folder needed; `append` adds a part, `path` reads a file in a granted folder, `save_as` keeps it as a `.rete` |

Every `dataset` argument accepts a local file name or path, a catalogue key,
an `https://` URL to any published `.rete`, or `chat:<name>`. Local files win
a name clash.

`dataset_card` includes `signals.quoted_triples` — whether the graph holds quoted
triples and the surfaces `rete export` can write them in — measured from the
file's header, so it answers for every published file.

### Files shared in the chat

Claude Desktop does not hand chat attachments to an extension, and the path
the model sees for an upload (`/mnt/user-data/uploads/…`) is in Claude's own
sandbox, not on this machine. So the model passes an attached or pasted RDF
file's text to `load_rdf`:

- It is built in-wasm into a graph held in memory, shared with the query
  workers (one `SharedArrayBuffer`, no copy per job), until the extension
  restarts. Nothing is written to disk unless `save_as` asks.
- The syntax comes from the file's extension (`.ttl`, `.nt`, `.nq`, `.trig`;
  otherwise Turtle). A file too long for one call goes in parts with
  `append: true`; each part is added to the text already held and the whole
  graph rebuilt, so the first part's prefixes carry over. A part that does not
  parse is refused and the graph keeps the parts before it. Up to 32 MB of
  text per graph.
- Reloading a name without `append` replaces the graph; the workers key it by
  a generation, so none answers from the old bytes.
- CSV, JSON, RDF/XML and JSON-LD are the model's to convert to Turtle first; a
  `.rete` is binary and has to come through a granted folder or a URL.

### Long queries: jobs

Claude gives up on a tool call after 60 s, and the engine's range reads are
synchronous: a query run on the server's own thread blocked every other call
until it finished, and one the client had abandoned could not be cancelled.
So every query runs on a worker thread (`src/worker.mjs`, scheduled by
`src/jobs.mjs`) as a job:

- A call answers well inside the limit: with the result, or with the
  `job_id` of a query that is still running and keeps running.
- Up to three jobs run at once, each worker with its own engine and caches;
  more wait in a queue. An idle worker is let go after ten minutes, a finished
  job's rows after fifty newer jobs have finished.
- Progress (reads, bytes) is counted while the query runs, through the
  engine's `reteProgress` / `reteReadMany` hooks into counters shared with
  the server.
- `cancel_query`, or the client cancelling a `sparql_query` call, terminates
  the worker mid-read. A `query_status` call that is cancelled only stops
  waiting.
- Workers key their open graphs by a stamp of the file (rebuild count, size,
  mtime), so a local file rebuilt under the same name is never answered from
  its old bytes.

The server's instructions tell the model what a query costs (one round trip
per scattered entity, so COUNT first), how to follow a job, and when and how
to split a large extraction with `partition_by`.

### Long extractions: `partition_by`

A job that runs for many minutes is lost when Claude Desktop restarts or the
query is cancelled. `partition_by: "?work"` on `sparql_query` / `start_query`
runs it as many short queries instead (`src/partition.mjs`), driving the
engine's split planner ([SPARQL docs](../../docs/sparql.md#partition)):

1. **Plan** (a worker step): the engine refuses a split that would change the
   answer, saying why, and writes the keys query from the cheapest constant
   pattern on the variable. It also returns the graph's content hash.
2. **Keys**: the variable's values, listed once (refused past 1,000,000, or
   when they are blank nodes).
3. **Chunks**: the query with a chunk's keys seeded as `VALUES`, two at once
   (the third worker stays free for other queries). The first holds 25 keys;
   later ones are sized from the engine time of earlier ones to take about 15 s,
   up to 5,000. A chunk whose reads failed is sent again (twice); one that
   crashed its worker is halved.
4. **Merge**: chunks in key order, deduplicated when the plan says so, then
   the query's OFFSET/LIMIT. A LIMIT stops starting chunks once the finished
   ones hold enough rows.

Every finished chunk is written to disk, under
`<temp>/rete-mcpb/partitions/<hash of graph, query, variable, reason>/`
(`RETE_MCPB_RUNS` moves it): `state.json`, `keys.json` and one
`c<start>-<end>.json` per chunk, each written aside and renamed. Sending the
same call again — after a cancel, a crash or a restart — resumes from them;
a checkpoint made on other bytes (a rebuilt local file, a republished URL:
the content hash differs) is discarded instead. A call identical to one still
running joins it rather than starting a second copy. Checkpoints untouched for
a week are deleted at startup. `query_status` reports the phase, keys and
chunks done, rows so far, the chunk size and how many keys were resumed.

## Install

**⬇ <https://github.com/caviri/rete/releases/latest/download/rete.mcpb>** —
the current build, always at that URL: it is an asset of whichever GitHub
release is Latest.

Every tagged release attaches the bundle twice, same bytes: `rete.mcpb` (what
that link serves) and the pinned `rete-<version>.mcpb`, for example
<https://github.com/caviri/rete/releases/download/v0.3.6/rete-0.3.6.mcpb>.
The release workflow builds it, runs the stdio suite against it, validates the
manifest, and publishes both names in `SHA256SUMS` with build provenance, so
you can verify before installing:

```sh
sha256sum --ignore-missing -c SHA256SUMS
gh attestation verify rete.mcpb --repo caviri/rete
```

Then double-click it, or drag it into Claude Desktop, or Settings → Extensions
→ Advanced settings → Install Extension…

At install time you pick the **Graph folders** to expose. Leaving it empty is
fine — the published catalogue still works, and the extension can read nothing
on disk.

## Build it yourself

Needs Docker only (no host node):

```sh
./build.sh          # assemble build/ and pack rete.mcpb
./build.sh --test   # ... running the stdio suites first
```

CI runs the same suites (`test/smoke.test.mjs`, `test/jobs.test.mjs`,
`test/chat.test.mjs`, `test/partition.test.mjs`) on every
pull request that touches the extension, the JS client or the engine
(`.github/workflows/mcpb-test.yml`), on Node 22 and on the Node 18 floor, and
validates the manifest; the release job runs them again before packing.

The bundle's version is stamped from the workspace `Cargo.toml` at build time —
the extension *is* the engine, so it never carries a second version to keep in
sync (the `version` in `manifest.json` is an inert placeholder).

The bundle ships the JS client's `dist/`, so rebuild that first if the engine
changed:

```sh
docker compose run --rm --user root dev bash -c \
  'wasm-pack build crates/rete-wasm --target web --no-opt --out-dir /work/clients/js/vendor/pkg'
docker run --rm -v "$PWD":/w -w /w/clients/js node:22-slim node build.mjs
```

The catalogue is projected at build time from the playground's
`web/playground-src/catalog.js` — the source of truth — so a bundle can never
ship a listing that has drifted behind it. Datasets published as shards are
listed with their shard URLs and a note, since they are not one file.

## Publish it

There is no manual step: a `v*` tag publishes the bundle. The `Claude Desktop
extension` job of `.github/workflows/release.yml` packs
`rete-<version>.mcpb`, copies it to `rete.mcpb`, and the release job uploads
both, checksums them in `SHA256SUMS` and attests them. Once that release is
published as Latest, `releases/latest/download/rete.mcpb` serves it.

The bundle used to be uploaded by hand to R2 (`clients/mcpb/publish.sh`,
removed in 0.3.6). Those objects stay where they were, frozen at 0.3.5, so old
links keep working but no longer move:
`https://data.graphplaza.com/mcpb/rete.mcpb` (0.3.5) and the pinned
`https://data.graphplaza.com/mcpb/rete-<version>.mcpb` builds up to 0.3.5.

## Test

```sh
docker run --rm -v "$PWD/../..":/w -w /w/clients/mcpb node:22-slim \
  sh -c 'npm install --silent && node --test ./test/*.test.mjs'
```

The suite spawns the built server and speaks MCP over stdio exactly as Claude
Desktop does — covering a local graph, the sandbox refusal, an offline build,
SHACL, and a published graph over HTTP Range.

## Notes and limits

- **stdout is protocol.** Diagnostics go to stderr; nothing else may print.
- **Queries run off the server's thread.** The engine's range reads are
  synchronous (sync XHR semantics bridged over `worker_threads`), so every
  SPARQL query runs in a worker (see *Long queries: jobs*). The other tools —
  card, schema, examples, entity lookups, SHACL — are a few reads each and
  still run on the server's own thread.
- **Sandbox.** Local reads and writes are confined to the granted folders,
  compared on *real* paths so a symlink cannot escape.
- **`build_rete` is in-wasm**, therefore uncompressed — for very large datasets
  use the `rete build` CLI and drop the result into a granted folder.
