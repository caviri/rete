# Release guide

What a Rete release contains, how a `v*` tag turns into one, and the runbooks
behind it. The notes for any one version are its section of the
[changelog](https://github.com/caviri/rete/blob/main/CHANGELOG.md); the GitHub
release body is generated from that section, not from this page.

Rete is deliberately 0.x. The on-disk format is already stable generation 1,
but the 0.3 line proves the packaging, the docs.rs builds, `cargo install
rete-cli`, and the release automation against the real registries before any
version has to honour a compatibility promise.

## Release procedure

One tag releases every channel: crates.io, PyPI, npm, the GitHub release with
all its assets, the container images, and the File Explorer installers.

1. **One release PR.** `python3 scripts/sync_versions.py --set X.Y.Z` stamps
   the version on the workspace, the `rete-core` pins, every client, the PyPI
   floors and the Binder pin (see [Cutting a version](#cutting-a-version)).
   The same PR carries what `--set` does not own: the lockfiles cargo and npm
   rewrite, `codemeta.json`, the File Explorer's `tauri.conf.json`, the
   browser artifacts (`scripts/build_wasm.sh`; the engine version is compiled
   in), the regenerated docs, and the `Unreleased` entries of `CHANGELOG.md`
   moved under `## [X.Y.Z] - date` (the 0.3.6 cut, #315, is a complete
   example). Run the publish gate before merging:
   `docker compose run --rm dev bash scripts/release_check.sh security`.
2. **Merge it** (squash) once CI is green.
3. **Push the tag** on the merge commit:
   `git tag vX.Y.Z <merge-sha> && git push origin vX.Y.Z`. Nothing after it
   is manual: the Pyodide 0.29 wheel, which used to be uploaded to our bucket
   by hand, is now a release asset ([why](#the-pyodide-fallback-wheel)).

Until 0.3.6 the clients released from their own `py-v*` and `js-v*` tags, and
the `v*` tag had to wait for PyPI and for a second "raise the PyPI floors" PR,
because the relay image and the Blender add-on installed `rete-graph` from
PyPI while they were built. Both are now built from wheels compiled in the
same run, so nothing in a release waits for a registry and no tag order
exists. The old `py-v*`/`js-v*` tags stay in the repository as history; they
trigger nothing.

## What the `v*` tag runs

Five workflows start on the tag, independently. Each one first runs
`scripts/sync_versions.py --check-tag vX.Y.Z` on the tagged commit: the tag
must be `v` + the workspace version **and** every channel must name that
version. They all check the same thing on the same commit, so a mis-stamped
tag stops every channel before any publishes, instead of shipping the ones
that happened to match.

| workflow / job | what it does |
|---|---|
| `release.yml` / `release identity` | checks the tag equals `v` + the `[workspace.package]` version, then `--check-tag` |
| `release.yml` / `draft GitHub release` | extracts the `## [X.Y.Z]` section of `CHANGELOG.md` (the job **fails** if it is missing or empty) and creates a **draft** release named `Rete X.Y.Z`, or reuses the one the tag already has |
| `release.yml` / native targets, `browser WASM bundle`, `Claude Desktop extension` | build the six native archives, the wasm bundle and the `.mcpb` (`rete-X.Y.Z.mcpb` and `rete.mcpb`) |
| `release.yml` / `rete-graph wheels` → `Blender add-on` | builds the four native `rete-graph` wheels from the tagged commit (`python-wheels.yml`), tests the add-on against them, and bundles exactly those wheels into `rete-blender-X.Y.Z.zip` |
| `desktop-release.yml` / `resolve release` | never creates a `v*` release: it waits (up to 20 min) for the draft and hands its id to the macOS and Windows builders, which upload the File Explorer installers into it |
| `release.yml` / `checksums, SBOM, attestations, and GitHub release` | uploads the archives, WASM bundle, extensions, `SHA256SUMS` and SBOM with `--clobber`, then publishes the draft |
| `release.yml` / `protected crates.io publishing` | runs in the `crates-io` environment (`v*` tags only, no reviewer), re-runs the `cargo audit`/`cargo deny` gate, publishes through trusted publishing, and attaches the receipt |
| `python-client-publish.yml` | tests the binding, builds the four native wheels (`python-wheels.yml`), the two Pyodide wheels and the sdist, then publishes them to PyPI in the `pypi` environment (`v*` tags only); its `Pyodide wheels into the GitHub release` job waits (up to 20 min) for the draft, like the File Explorer builders, and uploads the two Pyodide wheels plus the legacy-tagged copy for Pyodide 0.29 |
| `js-client-publish.yml` | builds the wasm engine, tests and packs `rete-graph`, runs `npm publish --dry-run`, then publishes that tarball with provenance in the `npm` environment (`v*` tags only) |
| `docker-publish.yml` | builds `rete-relay` (on the two Linux wheels it builds from the tagged commit; the build fails unless the image carries rete-graph X.Y.Z), `rete-cli` and `rete-dev` for amd64 and arm64, and pushes them to ghcr.io as `X.Y.Z`, `X.Y` and `latest` |

None of them waits for another, except the File Explorer builders and the
Pyodide wheel upload, which wait for the draft. PyPI, npm, crates.io and GHCR
are therefore published in whatever order the runners finish; a channel that
fails (a crates.io audit advisory, a flaky runner) is re-run on its own, and
the versions already on the other registries cannot be replaced, so fix
forward rather than re-tagging.

`workflow_dispatch` on any of them, from any branch, is a dry run: everything
is built and tested, the version check runs against the workspace version, and
nothing is published. `docker-publish.yml` is the exception: a manual run
pushes images unless its `dry-run` input is set, which builds both
architectures (the relay on wheels built in the run, as a tag does) and pushes
nothing.

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

More artifacts ship alongside them:

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
- `rete_graph-<version>-cp39-abi3-{pyemscripten_2025_0,pyemscripten_2026_0,pyodide_2025_0}_wasm32.whl`
  — the **browser Python (Pyodide) wheels**: the two on PyPI and the copy
  retagged for Pyodide 0.29, which is published nowhere else
  ([why](#the-pyodide-fallback-wheel)). From the release after 0.3.6.

`SHA256SUMS` covers every archive, the WASM bundle, and both extensions (the
`.mcpb` under both names). A
CycloneDX JSON SBOM and GitHub artifact attestations are published with the
release; the SBOM describes the Rust dependency graph and is bound to the
native and WASM archives, while the extension carries build provenance only.
The Pyodide wheels come from `python-client-publish.yml`, not from the
workflow that writes `SHA256SUMS`, so they are in neither the checksums nor the
attestations; GitHub shows each release asset's sha256 digest, and PyPI lists
the digests of the two it hosts.

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
strict audit gate in `scripts/publish_crates.sh` — the
[dependency audit](#dependency-audit) on the workspace lockfile, then
`cargo deny` — which refuses any advisory other than the two quick-xml
exceptions listed under Known limitations. Run the release
workflow manually against the `v0.3.0` tag with `verify_crates_io_auth=true`.
This requests and automatically revokes a short-lived OIDC token, validates the
tag and package preflight, and makes no publish call. Later release tags publish
in dependency order through the same protected job.

## Dependency audit

Every lockfile in the repository is audited, not only the workspace's:
`scripts/audit_lockfiles.py` runs `cargo audit` on the workspace, the Python
wheels (PyPI and Pyodide), the R package, the Java client's wasm (resolved
fresh, as its build does), the desktop File Explorer and the fuzz harness, and
`npm audit --omit=dev` on the npm packages that ship (`rete-graph`, the Claude
Desktop extension, the playground's CodeMirror bundle). The list, and the only
advisories it lets through, live in `lockfile-audit.toml`:

- every finding (vulnerability, unmaintained, unsound, yanked, notice) fails
  unless an exception names its id **and** the lockfile it was found in, with a
  reason and a `review_by` date at most a quarter away; there are no blanket
  ignores, and a yanked crate cannot be excepted;
- an exception that no longer matches anything fails (delete it), and one past
  its review date fails until it is re-checked and renewed or removed;
- a tracked lockfile the file does not list fails, and so does a committed
  `Cargo.lock` whose path crates disagree with their manifests (a lockfile
  cargo would rewrite is not the one being built);
- `deny.toml`'s advisory ignores must equal the exceptions for the workspace.

CI's `security` job runs it on any change to a lockfile, manifest or the audit
itself, and **nightly**, because advisories are published against lockfiles
nobody touched — 0.3.5's crates.io publish was blocked by two that appeared the
day after its tag. `scripts/publish_crates.sh` runs the same script on the
workspace lockfile (`--only .`), so the publish gate and CI cannot disagree; a
red nightly `security` is the warning that the next `v*` tag would be refused.
Run it before tagging:

```sh
docker compose run --rm dev bash scripts/release_check.sh security
```

## Language clients

Every client carries the engine's version exactly: `rete-graph` X.Y.Z on PyPI
and npm, the R package and the Java client (`io.github.caviri:rete-*`) all
embed engine X.Y.Z, because one `v*` tag releases them all. A binding-only fix
is therefore a full release (crates.io gets the unchanged crates under the new
version too); until 0.3.6 the clients had their own tags and only had to share
the engine's `MAJOR.MINOR`. `scripts/sync_versions.py --check` enforces the
single version in CI, `--check-tag` is what the release workflows run, and
`--write` realigns every channel on the workspace version. Every client also
exposes the engine version it embeds — `rete_graph.__engine_version__` in
Python.

### Registry credentials

No registry token is stored anywhere: the repository and its environments
hold no secrets, and every registry trusts a workflow identity instead
(GitHub OIDC, "trusted publishing"):

| registry | workflow / job | environment | how it authenticates |
|---|---|---|---|
| crates.io | `release.yml` / `protected crates.io publishing` | `crates-io` (`v*` tags only) | `rust-lang/crates-io-auth-action` trades OIDC for a short-lived token, revoked after the job |
| PyPI | `python-client-publish.yml` / `publish to PyPI` | `pypi` (`v*` tags only) | `uv publish --trusted-publishing always` (no attestations uploaded) |
| npm | `js-client-publish.yml` / `publish to npm` | `npm` (`v*` tags only) | the npm CLI (≥ 11.5.1) trades OIDC for a short-lived token; `--provenance` signs the build |

None of the three environments has a required reviewer: the tag is the
approval. What guards them is the tag-only deployment policy, the short-lived
OIDC token and, for crates.io, the audit gate.

Each registry's trusted-publisher entry names the workflow **file** (and the
environment), so renaming either breaks that channel until the entry on the
registry is updated. That is also why PyPI and npm publish from workflows of
their own, started by the same tag, rather than from jobs `release.yml` calls:
PyPI does not accept a reusable workflow as a trusted publisher, and npm checks
the *calling* workflow's filename, which would be `release.yml`. The npm job
publishes only the tarball the `build` job tested, and only for a pushed `v*`
tag; a `workflow_dispatch` run is a dry run
([details](clients-dev.md#releasing-to-npm)).

npm is phasing out granular tokens that bypass 2FA: they lost account and
package-management actions in July 2026, and from January 2027 they can only
stage a publish for a maintainer to approve. That is the notice
`npm publish` prints in the 0.3.6 log. It does not apply to trusted
publishing, which is what published 0.3.6. With no token in use, the package's
*Publishing access* on npmjs.com can be set to "Require two-factor
authentication and disallow tokens"; trusted publishing keeps working under
that setting.

### Cutting a version

`scripts/sync_versions.py --set X.Y.Z` stamps, in one commit, everything that
names the version:

| what | note |
|---|---|
| the workspace and the exact `rete-core` pins the publishable crates carry | |
| every client manifest: Python, npm, R, Java | |
| the PyPI floors: `clients/relay/requirements.txt`, `clients/blender/{build.sh,Dockerfile}` | a release builds the relay image and the Blender add-on from the wheels of its own run, so a floor may name a version PyPI does not serve yet; only a plain local `docker build` or the HF Space resolves it from PyPI, after the release |
| the Binder pin in `binder/requirements.txt` | mybinder.org resolves it from PyPI at launch, so a Binder launch from `main` fails between the merge and the tag's PyPI upload (about half an hour); nothing in CI reads it |

Up to 0.3.6 the floors and the Binder pin waited for a second PR
(`--set-published`, now removed), because the relay and Blender builds
installed `rete-graph` from PyPI and failed on a floor PyPI could not satisfy.
CI no longer depends on PyPI for them either: `relay-test.yml` and
`docker-publish.yml`'s PR check build the relay image on a wheel compiled from
the PR's own commit.

Also note the engine version is compiled into the wasm (`rete_core::VERSION`),
so a bump changes the shipped browser binaries: rerun `scripts/build_wasm.sh`
and commit the artifacts, or the parity gate rejects the release PR.

### The Pyodide fallback wheel

`%pip install rete-graph` resolves from PyPI and needs no pin. But Pyodide
0.29's installer predates PEP 783, which renamed the wheel platform tag
`pyodide_*` to `pyemscripten_*`, so it cannot see the wheel PyPI now requires.
Those users need the same build retagged with the old spelling,
`rete_graph-X.Y.Z-cp39-abi3-pyodide_2025_0_wasm32.whl`.

The PyPI workflow builds that copy for every `v*` tag as the
`pyodide-legacy-wheel` artifact, kept out of `dist` because PyPI would reject
the legacy tag. Its `Pyodide wheels into the GitHub release` job then attaches
it to the tag's release, together with the two `pyemscripten_*` wheels PyPI
gets. `docs/python.md` tells Pyodide 0.29 users to download it from there,
upload it into JupyterLite and `%pip install emfs:./<file>.whl`: GitHub serves
release assets without CORS headers, so a notebook cannot install one by its
URL.

Through 0.3.6 the copy was uploaded by hand to
`https://data.graphplaza.com/wheels/` (a CORS-enabled bucket, so it installs by
URL), the one manual step a release had. Those copies stay there, frozen at
0.3.6; nothing newer is uploaded.

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
v=0.4.2   # the version you downloaded
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
  every published `oxrdfxml` and `sparesults`, including oxrdfxml 0.2.5 and
  sparesults 0.3.5 (2026-10-18), requires `quick-xml ^0.37`, so the patched
  0.41 line is unreachable from here. RUSTSEC-2026-0194 and RUSTSEC-2026-0195
  are consequently carried as documented exceptions in `lockfile-audit.toml`
  (mirrored in `deny.toml`) for every lockfile that embeds the engine. Both are
  availability-only denial-of-service findings whose exposure is a caller
  parsing untrusted RDF/XML; neither affects confidentiality or integrity. Both
  exceptions are removed as soon as Oxigraph ships a `quick-xml >= 0.41` bump.
  The other accepted exceptions are informational notices with no shipped
  exposure: `paste` (unmaintained, a compile-time macro of the R client's
  extendr) and the File Explorer's GTK-backend crates (`glib`,
  `proc-macro-error`, Linux-only, while the app ships for macOS and Windows)
  plus the `unic-*` tables of `urlpattern` 0.3; `lockfile-audit.toml` gives the
  reason and the review date of each.

Full changes are in the [changelog](https://github.com/caviri/rete/blob/main/CHANGELOG.md).
