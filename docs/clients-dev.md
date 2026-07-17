# Client development & releases

Maintainer documentation for the language clients under `clients/` — how they
are laid out, built, tested, and released. **Users** should read the pages in
the *Clients* nav section instead (e.g. [Python — rete-graph](python.md)).

## Monorepo layout

One directory per language client, each owning its toolchain:

```
clients/
  python/        # PyO3 + maturin -> PyPI `rete-graph`
  r/             # (future)
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

The wheels are **native** CPython extensions: anything running real CPython
works out of the box — scripts, Jupyter, **marimo (desktop/server)**, Colab,
uv/pip/poetry/conda environments, Linux (x86_64/aarch64, glibc 2.17+/2.28+),
macOS (Intel + Apple Silicon), Windows x64.

**Browser Pythons are not covered yet**: JupyterLite and marimo's WASM
playground run on Pyodide, which only loads pure-Python or
`emscripten-wasm32` wheels. Supporting them needs (a) a maturin build against
the exact Pyodide/emscripten toolchain, (b) a no-threads `read_many` path
(wasm has no `std::thread`), and (c) remote reads routed through the existing
`open(reader=...)` callback backed by synchronous XHR (available there, since
Pyodide runs in a worker). Until then, the in-browser story is the
[playground](playground-guide.md) / [WASM API](browser.md), which is purpose-built
for it.

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
