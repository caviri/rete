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
`node` bundle — one JS file plus one `.wasm` — that runs unchanged on macOS,
Windows and Linux with the Node runtime Claude Desktop ships.

```
build/
  manifest.json
  icon.png
  server/
    index.mjs              1.2 MB   server + MCP SDK + rete-graph client, bundled
    rete_wasm_bg.wasm      3.1 MB   the engine
    catalog.json           0.6 MB   published-dataset catalogue
```

No `node_modules` in the archive, no native modules, nothing to install.

## Tools

| Tool | What it does |
| --- | --- |
| `list_datasets` | Local files + published catalogue, optionally filtered |
| `dataset_card` | The file's embedded self-description (2 small reads, any size) |
| `dataset_schema` | Classes and relations with counts, from the baked pyramid |
| `example_queries` | Runnable SPARQL the dataset ships with |
| `sparql_query` | SELECT / ASK / CONSTRUCT / DESCRIBE, `reason=true` for OWL 2 QL |
| `find_entities` | Label-prefix + full-text search → entity IRIs |
| `describe_entity` | Every statement about one IRI, both directions |
| `validate_shacl` | SHACL Core report; only the shapes' targets are read |
| `build_rete` | RDF text → a new `.rete` in a granted folder, offline |

Every `dataset` argument accepts a local file name or path, a catalogue key, or
an `https://` URL to any published `.rete`. Local files win a name clash.

## Install

**⬇ <https://data.graphplaza.com/mcpb/rete.mcpb>** — the current build, always
at that URL (pinned versions sit beside it as `rete-<version>.mcpb`).

Every tagged release also attaches `rete-<version>.mcpb` to the
[releases page](https://github.com/caviri/rete/releases), where the workflow
builds it, runs the stdio suite against it, validates the manifest, and
publishes it with a SHA-256 checksum and build provenance — take that copy to
verify before installing.

Then double-click it, or drag it into Claude Desktop, or Settings → Extensions
→ Advanced settings → Install Extension…

At install time you pick the **Graph folders** to expose. Leaving it empty is
fine — the published catalogue still works, and the extension can read nothing
on disk.

## Build it yourself

Needs Docker only (no host node):

```sh
./build.sh          # assemble build/ and pack rete.mcpb
./build.sh --test   # ... running the stdio smoke test first
```

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

The download link above is an R2 object, and `publish.sh` is what writes it:

```sh
./publish.sh            # build, then upload
./publish.sh --no-build # upload the rete.mcpb already here
./publish.sh --dry-run  # print the object keys, upload nothing
```

It writes the same bytes to two keys — `mcpb/rete-<version>.mcpb` first, then
the floating `mcpb/rete.mcpb` — so a reader following the floating pointer
always finds the pinned build already beside it. The version comes from the
*built* `build/manifest.json`, which `build.mjs` stamps from the workspace
`Cargo.toml`, so a published pin cannot disagree with the file's contents.

Credentials are the repository's usual R2 set (`ACCESS_KEY_ID`,
`SECRET_ACCESS_KEY`, `S3_API_ENDPOINT`, optional `RETE_BUCKET`); the upload
itself goes through `skills/rete-publish/scripts/upload_bucket.sh`, in a
container like everything else. The tagged-release artifact on GitHub is
produced separately by `.github/workflows/release.yml` and is unaffected.

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
- **One query at a time.** The engine's range reads are synchronous (sync XHR
  semantics bridged over `worker_threads`), so a query blocks the server's
  event loop for its duration. Fine for stdio MCP; if it ever bites, move the
  graph into a worker thread.
- **Sandbox.** Local reads and writes are confined to the granted folders,
  compared on *real* paths so a symlink cannot escape.
- **`build_rete` is in-wasm**, therefore uncompressed — for very large datasets
  use the `rete build` CLI and drop the result into a granted folder.
