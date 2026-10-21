# Release guide

What a Rete release contains, how a `v*` tag turns into one, and the runbooks
behind it. The notes for any one version are its section of the
[changelog](https://github.com/caviri/rete/blob/main/CHANGELOG.md); the GitHub
release body is generated from that section, not from this page.

Rete is deliberately 0.x. The on-disk format is already stable generation 1,
but the 0.3 line proves the packaging, the docs.rs builds, `cargo install
rete-cli`, and the release automation against the real registries before any
version has to honour a compatibility promise.

## How a `v*` tag becomes a GitHub release

Two workflows build for every `v*` tag, and exactly one of them creates the
release:

| step | workflow / job | what it does |
|---|---|---|
| 1 | `release.yml` / `release identity` | checks the tag equals `v` + the `[workspace.package]` version |
| 2 | `release.yml` / `draft GitHub release` | extracts the `## [X.Y.Z]` section of `CHANGELOG.md` (the job **fails** if it is missing or empty) and creates a **draft** release named `Rete X.Y.Z`, or reuses the one the tag already has |
| 3 | `desktop-release.yml` / `resolve release` | never creates a `v*` release: it waits (up to 20 min) for step 2's draft and hands its id to the macOS and Windows builders, which upload the installers into it |
| 4 | `release.yml` / `checksums, SBOM, attestations, and GitHub release` | uploads the archives, WASM bundle, extensions, `SHA256SUMS` and SBOM with `--clobber`, then publishes the draft |
| 5 | `release.yml` / `protected crates.io publishing` | runs in the `crates-io` environment (`v*` tags only, no reviewer), re-runs the `cargo audit`/`cargo deny` gate, publishes through trusted publishing, and attaches the receipt |

On publish, only a semver prerelease (`-rc.1`, `-alpha`, `-beta.2`) is marked
prerelease. A final version, 0.x included, is a normal release and becomes
**Latest** unless a newer final `v*` release already exists (so a backport
cannot take the badge). App-only `explorer-v*` releases are always prereleases
and never Latest.

Re-running any job of either workflow is safe. Releases are looked up by
listing them, drafts included (GitHub's get-by-tag endpoint cannot see drafts,
which is how v0.3.4 ended up with two releases), so a re-run reuses the release
it finds; uploads overwrite same-named assets; publishing an already published
release only re-applies the flags. If a tag ever carries **more than one**
release, the jobs stop and name them rather than guess: delete the extras by
hand. `workflow_dispatch` runs of `release.yml` never touch a GitHub release.

All of that logic lives in `scripts/github_release.py`, tested against a fake
`gh` by `python3 -m unittest scripts/tests/test_github_release.py`.

Before tagging, therefore: move the `Unreleased` entries under a
`## [X.Y.Z] - date` heading in `CHANGELOG.md`.

## Install and artifacts

```sh
cargo install rete-cli --locked
```

The release also provides native archives for:

- Linux GNU: x86-64 and ARM64
- macOS: Intel x86-64 and Apple Silicon ARM64
- Windows MSVC: x86-64 and ARM64

Each archive includes `rete`, Bash/Zsh/Fish/PowerShell completions, `rete(1)`,
README, license, and changelog.

Three more artifacts ship alongside them:

- `rete-<version>-wasm.tar.gz` — the browser/WASM bundle.
- `rete-blender-<version>.zip` — the **Blender add-on**, which turns a SPARQL
  answer into scene content: 3D assets imported, geometry placed, RDF properties
  inherited as drivable custom properties, relations as hierarchy or rigid-body
  constraints, time on the timeline. Install it with *Preferences ▸ Add-ons ▸
  Install from Disk…*; the engine is bundled for every platform, so there is no
  further install step. See the [Blender guide](blender.md).
- `rete-<version>.mcpb` — the **Claude Desktop extension**
  ([MCP Bundle](https://github.com/modelcontextprotocol/mcpb)). Download it and
  double-click, or drag it into Claude Desktop. It carries the same engine
  compiled to wasm, so one file installs on macOS, Windows and Linux; it queries
  your own `.rete` files offline and the published catalogue over HTTP Range.
  See the [agentic interfaces guide](agents.md). The same bytes are attached a
  second time as `rete.mcpb`, so
  `https://github.com/caviri/rete/releases/latest/download/rete.mcpb` always
  serves the Latest release's bundle; that is the link the README and docs
  advertise. This is the bundle's only publication channel: the R2 copies
  under `data.graphplaza.com/mcpb/` were uploaded by hand through 0.3.5 and
  are frozen there.

`SHA256SUMS` covers every archive, the WASM bundle, and both extensions (the
`.mcpb` under both names). A
CycloneDX JSON SBOM and GitHub artifact attestations are published with the
release; the SBOM describes the Rust dependency graph and is bound to the
native and WASM archives, while the extension carries build provenance only.

## Crates.io publication runbook

crates.io can only attach a trusted publisher to a crate that already exists, so
this first publication is a one-time manual bootstrap; every later tag publishes
through GitHub OIDC with no stored registry secret. It runs only after the
signed tag is checked out in a clean worktree. Enter the ephemeral token inside
the running devcontainer; do not use `cargo login`:

```sh
read -r -s -p "crates.io bootstrap token: " CARGO_REGISTRY_TOKEN
printf '\n'
export CARGO_REGISTRY_TOKEN
bash scripts/publish_crates.sh --bootstrap 0.3.0
unset CARGO_REGISTRY_TOKEN
```

The script packages and publishes `rete-core`, waits for registry and sparse
index visibility, then repeats for `rete-cli` and `rete-wasm`. Reruns skip an
existing package only when its registry checksum matches the locally packaged
archive. It then downloads all three registry archives, tests fresh native and
WASM consumers, and writes the non-secret
`target/release/crates-io-receipt.json`. Revoke the bootstrap token immediately
after attaching that receipt to the release and verifying the crates and
docs.rs pages.

After bootstrap, configure trusted publishers for all three crates with owner
`caviri`, repository `rete`, workflow `release.yml`, and environment
`crates-io`. That GitHub environment admits deployments from `v*` tags only
and has no required reviewer, so a release tag publishes to crates.io without a
manual step (up to 0.3.6 it needed a one-time approval per run). What guards
publication is the tag-only deployment policy, crates.io trusted publishing
(a short-lived OIDC token; the environment holds no crates.io secret), and the
strict `cargo audit` / `cargo deny` gate in `scripts/publish_crates.sh`, which
refuses any advisory other than the two quick-xml exceptions listed under
Known limitations. Run the release
workflow manually against the `v0.3.0` tag with `verify_crates_io_auth=true`.
This requests and automatically revokes a short-lived OIDC token, validates the
tag and package preflight, and makes no publish call. Later release tags publish
in dependency order through the same protected job.

## Language clients

Client versions track the engine's `MAJOR.MINOR`: `rete-graph` 0.3.x on PyPI and
npm, the R package, and the Java client (`io.github.caviri:rete-*`), all embed
engine 0.3.x. The patch component moves independently, so a binding-only fix ships as 0.3.1 without an engine release.
`scripts/sync_versions.py --check` enforces this in CI; `--write` realigns.
Every client also exposes the engine version it embeds — `rete_graph
.__engine_version__` in Python — which is the reliable answer to "does my
install support feature X?", since the binding's own version tracks the binding.

Clients release from their own tags (`py-v0.3.0`, `js-v0.3.0`), not the `v*`
release tag.

### Cutting a version

`scripts/sync_versions.py --set X.Y.Z` stamps the workspace, the exact
`rete-core` pins the publishable crates carry, and every client manifest.
`--write` will not do this: it only repairs `MAJOR.MINOR` drift (it writes
`{minor}.0`), so it is a no-op for the patch bump that *is* the release.

Two groups deliberately lag the bump, because they name a version a registry
must already serve:

| what | when | why |
|---|---|---|
| the three PyPI floors — `clients/relay/requirements.txt`, `clients/blender/{build.sh,Dockerfile}` | **after** the wheel is on PyPI, via `--set-published X.Y.Z` | they `pip install rete-graph` at image-build time; bumping them early fails every image build with `Could not find a version that satisfies rete-graph>=X.Y.Z` |
| the Pyodide fallback URL in `docs/python.md` | **with** the bump (`--set` does it) | `publish_pyodide_wheel.sh` refuses to upload a wheel whose version disagrees with the docs, so the doc has to promise it first — the URL 404s until that upload |

`--check` keeps passing while the floors lag, because the lockstep contract
compares `MAJOR.MINOR` only.

Also note the engine version is compiled into the wasm (`rete_core::VERSION`),
so a bump changes the shipped browser binaries: rerun `scripts/build_wasm.sh`
and commit the artifacts, or the parity gate rejects the release PR.

### The Pyodide fallback wheel

`%pip install rete-graph` resolves from PyPI and needs no pin. But Pyodide
0.29's installer predates PEP 783, which renamed the wheel platform tag
`pyodide_*` to `pyemscripten_*`, so it cannot see the wheel PyPI now requires.
`docs/python.md` points those users at a retagged copy on our own bucket.

The publish workflow builds that copy for every `py-v*` tag and uploads it as
the `pyodide-legacy-wheel` artifact — kept out of `dist`, which goes to PyPI and
would reject the legacy tag. Actually publishing it is a separate step, because
the repository has no R2 credentials in Actions:

```sh
scripts/publish_pyodide_wheel.sh          # after the py-v<version> run finishes
```

It pulls the artifact from that tag's run, refuses anything whose version or
platform tag disagrees with what `docs/python.md` promises, uploads it to
`wheels/`, and verifies the documented URL returns 200 with a plausible body.
`sync_versions.py --check` keeps that URL's version honest; only this step makes
it resolve.

## Compatibility statement

The experimental generations `0x01`–`0x04` that predate this release are not
readable: rebuild those datasets from their source RDF with the 0.3 toolchain,
and do not rename an older file and assume it is compatible. Generation 1
(`0x05`) has not moved since it froze on 2026-07-14, but **no
backwards-compatibility promise is made before 1.0.0**: the format may still
change in ways that require rebuilding files produced by the 0.3 line. Keep the
RDF source you built from. The durable compatibility promise starts at 1.0.0.
See [compatibility](compatibility.md#stable-rete-file-compatibility).

The Rust crates declare Rust 1.87 as their minimum supported version. The
public Rust, CLI, and WASM surfaces carry no semantic-versioning promise while
the crates are 0.x; semver applies from 1.0.0 onward.

## Verify a download

```sh
v=0.3.6   # the version you downloaded
sha256sum --ignore-missing -c SHA256SUMS
gh attestation verify rete-$v-x86_64-unknown-linux-gnu.tar.gz \
  --repo caviri/rete
./rete-$v-x86_64-unknown-linux-gnu/rete --version
```

Then build and query a source graph:

```sh
rete build source.nq -o graph.rete --pyramid-algo types --card
rete verify graph.rete
rete sparql graph.rete 'SELECT * WHERE { ?s ?p ?o } LIMIT 1'
```

## Known limitations

- Browser execution is single-threaded by default. Threads are experimental and
  require cross-origin isolation.
- Range reads are lazy; query result materialization is currently eager.
- Federation unions results from independent files and does not resolve general
  joins spanning files.
- The RDF/XML parser resolves `quick-xml 0.37.5` through upstream Oxigraph:
  every published `oxrdfxml`, including the current 0.2.3, requires
  `quick-xml ^0.37`, so the patched 0.41 line is unreachable from here.
  RUSTSEC-2026-0194 and RUSTSEC-2026-0195 are consequently carried as documented
  exceptions in `deny.toml` and in the publish preflight. Both are
  availability-only denial-of-service findings whose exposure is a caller
  parsing untrusted RDF/XML; neither affects confidentiality or integrity. Both
  exceptions are removed as soon as Oxigraph ships a `quick-xml >= 0.41` bump.

Full changes are in the [changelog](https://github.com/caviri/rete/blob/main/CHANGELOG.md).
