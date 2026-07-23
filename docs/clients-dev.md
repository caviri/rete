# Client development & releases

Maintainer documentation for the language clients under `clients/` — how they
are laid out, built, tested, and released. **Users** should read the pages in
the *Clients* nav section instead (e.g. [Python — rete-graph](python.md)).

## Monorepo layout

One directory per language client, each owning its toolchain:

```
clients/
  python/        # PyO3 + maturin -> PyPI `rete-graph`
  r/             # extendr + rextendr -> CRAN/R-universe `rete`
  go/            # (future)
```

Clients consume `crates/rete-core` — never the other way around. The Python
crate is **excluded from the cargo workspace** (root `Cargo.toml`, like
`fuzz/`): it versions and locks independently (`clients/python/Cargo.lock`),
so the workspace release gates (`--locked` checks, feature matrix, coverage
floors, MSRV) never see binding-only changes.

## The Python client

```
clients/python/
  Cargo.toml               # cdylib `_rete`; pyo3 abi3-py39, rete-core, ureq
  pyproject.toml           # maturin backend; the PyPI version lives HERE
  src/lib.rs               # bindings, mirroring the wasm Graph/RemoteGraph
  src/readers.rs           # Local (pread) / Http (Range) / Py (callback) readers
  python/rete_graph/       # pure-Python layer: Term parsing, open(), Builder
  tests/                   # pytest; Range-capable local HTTP fixture
  examples/tutorial.ipynb  # executed notebook (re-run it when the API changes)
```

Design invariants worth keeping:

- The method surface **mirrors the wasm client** (`crates/rete-wasm`) — same
  JSON envelopes, same lazy-open pipeline (counting reader → block cache →
  `open_ranged_lazy`), same incomplete-fetch-is-an-error contract.
- Every engine call runs inside `Python::allow_threads`; the HTTP reader's
  16-way `read_many` matches the CLI and the browser fetch pool.
- The engine emits N-Triples tokens (`<iri>`); the pure-Python layer converts
  them to clean values everywhere (`Term`, `schema()`, `graph_names()`,
  searches). Keep new bindings consistent with that.
- Dataset Card entries written from Python must be **complete** — the CLI's
  card schema has required fields (counts, `format_version`, every field of a
  rich example query); a partial entry makes `rete card` reject the card.

### Build and test (Docker-only, nothing on the host)

```sh
# wheel
docker run --rm -v "$PWD":/io ghcr.io/pyo3/maturin build \
    --release -m clients/python/Cargo.toml --out clients/python/dist

# fmt + clippy (the image's entrypoint is maturin; override it)
docker run --rm --entrypoint sh -v "$PWD":/io -w /io/clients/python \
    ghcr.io/pyo3/maturin -c "cargo fmt --check && cargo clippy --all-targets -- -D warnings"

# tests, installing the wheel with uv in a clean container
docker run --rm -v "$PWD":/io -w /io/clients/python python:3.12-slim bash -c \
    "pip install -q uv; uv venv /tmp/v && uv pip install --python /tmp/v/bin/python \
     dist/*.whl pytest pandas rdflib && /tmp/v/bin/python -m pytest tests -q"
```

## CI: two workflows, deliberately separate

| Workflow | Trigger | Does |
|---|---|---|
| `python-test.yml` | PR / push touching `clients/python/**` or `crates/rete-core/**` | fmt + clippy, build wheel, pytest (incl. the live R2 remote smoke) — **never publishes** |
| `python-client-publish.yml` | pushing a `py-v*` tag only | full wheel matrix + sdist, then PyPI upload |

Merging to main can never publish; releasing is always the deliberate tag.

## Releasing to PyPI

Publishing uses **trusted publishing** (OIDC) — no tokens, no secrets. The
registered publisher on PyPI: project `rete-graph`, owner `caviri`, repo
`rete`, workflow `python-client-publish.yml`, environment `pypi`. Renaming
that workflow file breaks publishing until the PyPI form is updated.

Release procedure:

1. Bump `version` in `clients/python/pyproject.toml` (PyPI refuses to
   re-upload an existing version, so a forgotten bump just fails cleanly).
2. Commit, push, then `git tag py-vX.Y.Z && git push origin py-vX.Y.Z`.
3. Watch the *Python client publish* run: test → 4 wheels + sdist → publish.

Hard-won build-matrix facts (already encoded in the workflow — keep them):

- The **aarch64 wheel must build on `manylinux: 2_28`**: `ring`'s pregenerated
  ARM assembly rejects manylinux2014's old cross-assembler
  (`ARM assembler must define __ARM_ARCH`).
- `fail-fast: false` on the wheel matrix, so one platform's failure cannot
  cancel the other wheels.
- Wheels are abi3 (`abi3-py39`): one wheel per platform covers every
  CPython ≥ 3.9 — no per-Python-version matrix.

Optionally add required reviewers to the `pypi` GitHub environment to make
every release pause for a manual approval.

## Runtime compatibility

The main wheels are **native** CPython extensions: anything running real
CPython works out of the box — scripts, Jupyter, **marimo (desktop/server)**,
Colab, uv/pip/poetry/conda environments, Linux (x86_64/aarch64, glibc
2.17+/2.28+), macOS (Intel + Apple Silicon), Windows x64.

### The Pyodide (browser Python) build

From 0.2.0 the release also ships **PyEmscripten wheels** (PEP 783,
`pyemscripten_*_wasm32` tags — accepted by PyPI) for Pyodide runtimes:
JupyterLite, marimo WASM. How it works, all behind
`cfg(target_os = "emscripten")` so native builds are untouched:

- **No sockets in browsers** → `ureq`, `HttpRangeReader`, and the SERVICE
  client are compiled out (this also drops `ring`/rustls, the one dependency
  that genuinely hurts on emscripten). Remote opens route through the
  pure-Python `_XhrRangeReader` — synchronous `XMLHttpRequest` with binary
  responses, which browsers allow **only in web workers**; JupyterLite and
  marimo run their kernels there. The engine stays fully synchronous: no
  Asyncify anywhere.
- **No threads in wasm** → with the HTTP reader gone, nothing spawns threads;
  the Python-callback reader uses the default sequential `read_many`.
- **No C zstd encoder** → rete-core builds without the `compression` feature;
  reads of compressed files still work (pure-Rust decoder), in-browser
  `build()` writes codec NONE like the playground's Build tab.
- **Toolchain — three hard-won pins** (all encoded in the `wheel-pyodide`
  job; change them together or not at all):
  1. **A dated nightly** (`nightly-2025-06-01`): pyodide-build drives cargo
     with `-Z` emscripten flags — nightly-only, and *newer* nightlies dropped
     `-Z emscripten-wasm-eh` once it became the default. The window is
     ≥ 1.87 (the workspace MSRV) and pre-removal. Set it via
     `RUSTUP_TOOLCHAIN` — the repo's `rust-toolchain.toml` stable pin
     silently overrides `rustup default` otherwise.
  2. **`build-std`** (`CARGO_UNSTABLE_BUILD_STD=std,panic_abort,panic_unwind`
     + the `rust-src` component): Rust's *prebuilt* emscripten std is
     compiled **without** wasm-EH; linking it emits JS-EH `invoke_*` imports
     that Pyodide's runtime refuses at import time
     (`cannot resolve symbol invoke_vii`). Recompiling std with the same
     flags fixes the ABI mismatch.
  3. **cibuildwheel** (`--platform pyodide`) provisions the pinned emsdk +
     xbuildenv and emits one wheel per Pyodide ABI year.
  Revisit all three when pyodide-build supports stable Rust — upstream main
  already pins `1.93.0` + Emscripten 5, at which point the nightly and
  build-std steps disappear.

Local build (Docker, like everything else):

```sh
docker run --rm -v "$PWD":/io -w /io/clients/python python:3.13-bookworm bash -c '
  curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none --profile minimal
  export PATH=/root/.cargo/bin:$PATH
  rustup toolchain install nightly-2025-06-01 --profile minimal
  rustup target add --toolchain nightly-2025-06-01 wasm32-unknown-emscripten
  rustup component add --toolchain nightly-2025-06-01 rust-src
  export RUSTUP_TOOLCHAIN=nightly-2025-06-01
  export CARGO_UNSTABLE_BUILD_STD=std,panic_abort,panic_unwind
  pip install cibuildwheel && cibuildwheel --platform pyodide --output-dir dist-pyodide .'
```

Smoke-test the wheel in a node-backed Pyodide venv (`pip install
pyodide-build && pyodide venv /tmp/pyenv`, then install the wheel with its
pip and import) — it must print `sys.platform == "emscripten"` and answer a
query. Remote/XHR paths need a real browser (worker), so keep a manual
JupyterLite check for releases that touch the reader.

**wasm64 (tracked future work)**: wasm32 caps memory at 4 GiB. Browsers ship
memory64 now, but Pyodide/emscripten don't build for it yet — when Pyodide
gains a wasm64 ABI, add its wheel here; nothing in our code assumes 32-bit.

## The JavaScript client

`clients/js/` (npm `rete-graph`) wraps the **wasm engine built fresh from the
checked-out crates** — `build-wasm.sh` runs `wasm-pack build crates/rete-wasm
--target web` into `vendor/pkg` (gitignored). It deliberately does *not*
vendor the committed `web/pkg` playground artifacts: those follow their own
build pipeline and have been observed to lag the engine sources (a stale
artifact rejects newer-format files with `header: unsupported version`).

- `build.mjs` (esbuild) emits three shapes: `dist/index.js` (ESM, wasm as a
  lazy-loaded sibling file — bundlers, Node), and the p5.js-style script-tag
  singles `dist/rete-graph.js` / `.min.js` (wasm embedded via the `binary`
  loader, global `rete`). Node builtins stay `external: ["node:*"]` behind
  dynamic imports so browser bundles never resolve them.
- **Remote opens on Node** go through `src/node-sync-xhr.js`: a minimal
  `XMLHttpRequest` implementing exactly the subset `web_sys` calls, backed by
  fetch in a worker thread + `Atomics.wait`. Two gotchas encoded in the
  tests: anything the blocked main thread must *itself* serve deadlocks — the
  test Range server runs in its own worker; and the IIFE's `var rete` only
  becomes a global under classic-script semantics, so the bundle test loads
  it via `vm.runInThisContext`, not `import`.
- CI: `js-test.yml` (paths-filtered: clients/js + rete-wasm + rete-core) and
  `js-client-publish.yml` (`js-v*` tags → npm publish via OIDC trusted
  publishing; configure the publisher on npmjs.com → package → Settings →
  Trusted Publisher: repo `caviri/rete`, workflow `js-client-publish.yml`).
- Parity backlog vs Python: Dataset Card + embedded examples (needs a
  `card()` export in rete-wasm — an engine change, so the playground gate
  applies), custom headers, a Builder.

## The R client

`clients/r/` (package `rete`) binds `rete-core` with **extendr**, scaffolded
by `rextendr::use_extendr()` in its CRAN-ready shape (`configure` +
`tools/msrv.R` check for cargo, `src/Makevars.in` drives the cargo build
during `R CMD INSTALL`). The crate at `clients/r/src/rust/` is excluded from
the cargo workspace like the Python one.

- **extendr is pinned to the 0.8 line** (`extendr-api = '0.8'`): rextendr
  0.5's generated plumbing (entrypoint.c, wrapper conventions) targets it,
  and 0.9 changed the `#[extendr]` macro contract.
- Two extendr 0.8 shapes that cost a debugging session — keep them:
  the struct needs its **own** `#[extendr]` attribute (it generates the
  `Robj` conversions; the impl-level macro alone leaves you with opaque
  `ToVectorValue`/`TryFrom` errors), and fallible functions don't return
  `Result` — they diverge via `throw_r_error` (the `fail()` helper), which
  surfaces as a regular R condition.
- `R/extendr-wrappers.R` is **generated** by `rextendr::document()` (which
  recompiles the crate first — plain `devtools::document()` does not);
  regenerate and commit it whenever the Rust surface changes. CI diff-checks
  it. Note `rextendr::document()` prints a deprecation notice (removed-in-favour
  of `devtools::document()` since rextendr 0.4.0). It still works and still
  recompiles; before switching, verify the replacement actually rebuilds the
  crate, since a silent no-op would commit stale wrappers — the exact failure
  the parenthesis above warns about.
- The R layer follows the same rule as Python: the engine emits N-Triples
  tokens, and `R/query.R` coerces them (`parse_term`/`coerce_terms`) into
  clean data-frame columns — IRIs unbracket, numeric/boolean literals become
  R types, `rete_query_raw()` keeps full fidelity.

Build and test (Docker, nothing on the host):

```sh
docker compose run --rm r        # regenerate wrappers + docs
docker compose run --rm r-test   # regenerate, then run testthat
```

Both use `.devcontainer/Dockerfile.r` — the recipe below, baked into a cached
image on `rocker/r2u` (every CRAN package as an apt binary) with rustc pinned to
the workspace toolchain. It is a separate image from the main devcontainer on
purpose: that one is rebuilt by nearly every CI job, and the R package tree
would add ~1 GB to all of them. Cargo artifacts go to a named volume, since
`clients/r/src/rust/target` is not gitignored.

The equivalent from scratch, if you would rather not use compose:

```sh
docker run --rm -v "$PWD":/io -w /io/clients/r rocker/r2u:jammy bash -c '
  apt-get update -qq && apt-get install -y -qq curl build-essential >/dev/null
  curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile minimal
  export PATH=/root/.cargo/bin:$PATH
  Rscript -e "install.packages(c(\"rextendr\", \"devtools\", \"jsonlite\", \"testthat\"))"
  Rscript -e "rextendr::document()"
  Rscript -e "devtools::test(stop_on_failure = TRUE)"'
```

CI: `r-test.yml` (paths-filtered: clients/r + rete-core) regenerates the
wrappers, fails if the committed copy is stale, runs testthat, then
`R CMD check --no-manual` as a CRAN preflight. See the *Releasing to CRAN*
notes at the end of this section.

Two install-path facts, both verified in clean containers:

- Direct installs need `remotes::install_github(..., subdir = "clients/r",
  build = FALSE)` — the default first builds a tarball of the subdir alone,
  where the `../../../../crates/rete-core` path dependency cannot resolve;
  `build = FALSE` installs from the extracted repo tree. `pak`'s
  `user/repo/subdir` shorthand has no such switch and fails — pak support
  arrives with R-universe/CRAN hosting.
- The executable bits on `configure`/`cleanup` matter: committed from
  Windows they become mode 644, and `R CMD INSTALL` on Unix rejects a
  non-executable `configure` (`R CMD build` silently corrects it, hiding
  the problem from tarball-based checks). Fixed via
  `git update-index --chmod=+x`; keep it when regenerating the scaffold.

### Releasing to CRAN (and the pragmatic path first)

CRAN has a [Rust policy](https://cran.r-project.org/web/packages/using_rust.html):
builds must not download the network, so every crate must be **vendored**
into the source tarball:

1. `rextendr::vendor_pkgs()` — writes `src/rust/vendor.tar.xz` +
   `vendor-config.toml` and points the Makevars at the offline registry.
   Because `rete-core` is a *path* dependency it rides along automatically;
   re-vendor after any engine change.
2. `LICENSE.note` must list every vendored crate and its license
   (`rextendr::write_license_note()`).
3. The DESCRIPTION already carries the required
   `SystemRequirements: Cargo (Rust's package manager), rustc >= 1.87`; keep
   the version in sync with the workspace MSRV.
4. Stage the standalone package first — `scripts/r_cran_prep.sh <dir>
   [--vendor]` embeds a self-contained `rete-core` via `cargo package`
   (the path dependency climbs out of the package, so an unstaged
   `R CMD build` tarball cannot compile) — then `R CMD build` +
   `R CMD check --as-cran` on the result must be clean (no ERROR/WARNING;
   justify any NOTE in the submission comment). Known NOTE: extendr-api 0.8
   itself calls the non-API `R_NamespaceRegistry`; CRAN's API-compliance
   push may question it on a *new* submission — the fix lands with the
   extendr 0.9 line, so consider timing the CRAN submission to a future
   rextendr/extendr upgrade and using R-universe meanwhile.
   Check the tarball stays under CRAN's 5 MB preference — the vendor archive
   is the risk; mention it in the submission comment if exceeded.
5. Submit at <https://cran.r-project.org/submit.html>; confirm the
   maintainer-address email. First submissions get a human review measured
   in days-to-weeks.

**R-universe first**: before (or instead of) CRAN, register the repo at
<https://github.com/r-universe-org> (a `caviri.r-universe.dev` universe with
a `packages.json` pointing at `caviri/rete`, `subdir: clients/r`). It builds
binaries for all platforms on every push — users
`install.packages("rete", repos = "https://caviri.r-universe.dev")` with no
Rust toolchain — and it exercises the exact source layout CRAN will see.

## Adding a new language client (R, Go, …)

The checklist that made Python work:

1. `clients/<lang>/` with its own toolchain and lockfile; bind `rete-core`
   natively (R: extendr; Go: cgo over a small C ABI crate).
2. Mirror the wasm `Graph`/`RemoteGraph` surface and the reader contract:
   HTTP Range with a hard 206 requirement, short reads are errors, batched
   `read_many` fetched concurrently, block cache on top, and the
   incomplete-fetch guard before returning results.
3. Parse the shared JSON envelopes; present clean IRIs/values, not `<tokens>`.
4. Two workflows: `<lang>-test.yml` paths-filtered to the client + rete-core,
   and `<lang>-client-publish.yml` gated on `<lang>-v*` tags with the
   registry's trusted-publishing equivalent.
5. A user page in the *Clients* nav section, dev notes in this page, and a
   runnable example (notebook or equivalent) executed in CI or Docker.

## Docs maintenance

Client pages are Markdown under `docs/`; the nav lives in
`crates/docgen/src/main.rs` (`SECTIONS`). After editing either:

```sh
docker run --rm -v "$PWD":/work -w /work rust:1.92-bookworm cargo run -q -p docgen
python scripts/check_docs_links.py
```

CI re-renders and diff-checks `docs/`, so commit the regenerated HTML.
