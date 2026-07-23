# Container images

Three images are published to GitHub Container Registry **when the version
moves** — a `v*` tag, the same trigger `release.yml` uses — and never on an
ordinary push to `main`. All three are multi-arch manifest lists covering
**linux/amd64 and linux/arm64**, so an Apple Silicon laptop, a Graviton box and
an ordinary x86 runner all pull the same tag and get a native image — no
emulation.

| Image | What it is | Size | Built from |
| --- | --- | --- | --- |
| `ghcr.io/caviri/rete-dev` | The full toolchain: Rust 1.92 + the pinned asyncify nightly, wasm-pack, the cargo release tools, node, python3 + uv, binaryen, chromium. Everything the repo builds datasets and playground artifacts with. | ~5 GB | `.devcontainer/Dockerfile` |
| `ghcr.io/caviri/rete-relay` | The gateway: HTTP Range + CORS over a data directory, the `/api` SPARQL plane, and the `/mcp` MCP server. The same image the live Hugging Face Space runs. | ~400 MB | `hf-space/Dockerfile` |
| `ghcr.io/caviri/rete-cli` | Just the `rete` binary on distroless. | ~30 MB | `docker/Dockerfile.cli` |

Tags: `latest` (newest stable release), `1.2.3` / `1.2` (release tags —
prereleases like `1.0.0-rc.1` get the exact version only and never move `latest`
or `1.0`), and `sha-<short>` for the exact commit the tag pointed at. A manual
run from `main` also produces a `main` tag. `rete-dev` additionally carries an
immutable `<hash>` tag, the first 16 hex of `sha256(.devcontainer/Dockerfile)`,
which is exactly the identity CI derives for its own build of that image.

## Building artifacts — `rete-dev`

Same commands as the [contributing workflow](../AGENTS.md), with the image
pulled instead of built:

```sh
docker run --rm -v "$PWD:/work" -w /work ghcr.io/caviri/rete-dev:latest \
  cargo build --release -p rete-cli

docker run --rm -v "$PWD:/work" -w /work ghcr.io/caviri/rete-dev:latest \
  uv run python scripts/build_playground.py
```

The image runs as `vscode` (uid 1000). On Linux, add
`--user "$(id -u):$(id -g)"` if your checkout is owned by a different uid, and
`-e CARGO_TARGET_DIR=/tmp/target` if you would rather not write `target/` into
the mount.

Cargo's registry lives inside the image, so give it a volume to keep downloads
across runs:

```sh
docker run --rm -v "$PWD:/work" -w /work \
  -v rete-cargo-registry:/usr/local/cargo/registry \
  ghcr.io/caviri/rete-dev:latest cargo test --workspace --exclude rete-bench
```

## Running the relay — `rete-relay`

Put one or more `.rete` files in a directory and mount it at `/data`:

```sh
docker run --rm -p 7860:7860 -v "$PWD/data:/data" ghcr.io/caviri/rete-relay:latest
```

| Endpoint | What it serves |
| --- | --- |
| `GET /` | Landing page (themed by the baked-in `branding.json`) |
| `GET /<file>.rete` | The file itself, with `Accept-Ranges`/`206` and CORS — this is what browser clients read |
| `POST /api/…` | SPARQL query plane over the catalog |
| `/sparql/<key or .rete URL>` | SPARQL 1.1 Protocol endpoint |
| `POST /mcp` | MCP server, streamable HTTP |
| `GET /docs` | OpenAPI browser |
| `GET /health` | Liveness + resolved data dir |

Point an MCP client at `http://localhost:7860/mcp`. For Claude Code:

```sh
claude mcp add --transport http rete http://localhost:7860/mcp
```

Tunables are environment variables — the defaults are tuned for a FUSE-mounted
bucket on a 2 vCPU Space, and a local disk can afford more:

```sh
docker run --rm -p 7860:7860 -v "$PWD/data:/data" \
  -e WEB_CONCURRENCY=4 -e FUSE_CONCURRENCY=64 -e CACHE_MB=1024 \
  ghcr.io/caviri/rete-relay:latest
```

`rete_ask.py` (the natural-language query tool) only activates when an
`OPENAI_API_KEY` or `ANTHROPIC_API_KEY` is present in the environment; every
other endpoint works without one.

The relay installs the engine from the published `rete-graph` wheel on PyPI, so
it tracks *released* clients, not the working tree. To exercise an unreleased
engine change, build the wheel from `clients/python` and install it over the top
in a derived image.

## One-shot CLI — `rete-cli`

```sh
docker run --rm -v "$PWD:/work" ghcr.io/caviri/rete-cli:latest \
  build input.nt -o graph.rete

docker run --rm -v "$PWD:/work" ghcr.io/caviri/rete-cli:latest \
  sparql graph.rete 'SELECT * WHERE { ?s ?p ?o } LIMIT 10' --json
```

Remote graphs work straight out of the image — the CLI links rustls with
compiled-in trust roots, so there is no ca-certificates package to be missing:

```sh
docker run --rm ghcr.io/caviri/rete-cli:latest \
  summary-url https://data.graphplaza.com/boe.rete
```

`rete serve` binds `127.0.0.1:7878` by default, which nothing outside the
container can reach. Bind the wildcard address explicitly, and set a token,
because a reachable endpoint accepts SPARQL Update:

```sh
docker run --rm -p 7878:7878 -v "$PWD:/work" ghcr.io/caviri/rete-cli:latest \
  serve graph.rete --bind 0.0.0.0:7878 --token "$RETE_TOKEN"
```

The image has no shell — it is distroless. `docker run --entrypoint sh` will not
work; use `rete-dev` when you need to poke around.

## Publishing

`.github/workflows/docker-publish.yml` decides which images an event needs and
calls the reusable `.github/workflows/docker-image.yml` once per image. That
workflow builds each architecture on a runner of that architecture
(`ubuntu-24.04` and `ubuntu-24.04-arm`, both free for this public repo), pushes
each result **by digest**, and merges the digests into one tagged manifest list
with `docker buildx imagetools create`.

| Event | What happens |
| --- | --- |
| Push a `v*` tag | All three images build and publish. The tag must match `[workspace.package] version` in `Cargo.toml` — the same check `scripts/package_release.sh` makes — or the run fails before building anything. |
| Pull request touching `.devcontainer/Dockerfile*`, `hf-space/**`, `docker/**` or the docker workflows | `rete-relay` and `rete-cli` are built for amd64 only and thrown away. Nothing is pushed. |
| Manual run | Publishes `all`, or just `dev` / `relay` / `cli`. |
| Ordinary push to `main` | Nothing. Images follow releases, not commits. |

Native runners are the point. `rete-dev` installs about eight cargo tools from
source; the same build under QEMU emulation takes hours and flirts with the
six-hour job ceiling.

To publish by hand: Actions → *Publish container images* → *Run workflow*, and
pick `all`, `dev`, `relay` or `cli`.

### Build context

There is deliberately no root `.dockerignore`, because `scripts/Dockerfile.gltf`,
`scripts/Dockerfile.blender` and friends legitimately build from a full context.
Instead each image that wants a narrow context ships its own
`<dockerfile>.dockerignore`, which BuildKit prefers over the context's root file:

- `.devcontainer/Dockerfile.dockerignore` is `**` — that Dockerfile has no
  `COPY` at all, so it needs no context. This also speeds up
  `docker compose build dev` on a working tree carrying `data/`.
- `docker/Dockerfile.cli.dockerignore` allows only `Cargo.toml`, `Cargo.lock`,
  `rust-toolchain.toml` and `crates/**`.
