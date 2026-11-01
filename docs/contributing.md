# Development {#contributing}

Start here to understand the code, make a change and check that it works.
The Rust core is shared by the CLI and language bindings; the documentation,
browser applications and release tooling live alongside it. You do not need
to learn the whole repository before making a useful contribution.

## Choose where to start {#find-the-right-part-of-the-project}

| I want to… | Read this |
|---|---|
| Find the right code and follow a query through the engine | [Repository and core](architecture.md) |
| Understand what the tests cover and what CI runs | [Tests, coverage and CI](testing.md) |
| Measure performance and interpret existing results | [Benchmarks](benchmarks.md) |
| Change a language binding | [Client development](clients-dev.md) |
| Prepare a release | [Release guide](release.md) |

The [format specification](SPEC.md) and [compatibility reference](compatibility.md)
describe the contracts that engine changes must preserve. To use rete without
changing it, follow the [Quickstart](getting-started.md).

## Build and test

From a checkout of the repository:

```sh
docker compose build dev
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev cargo test --workspace --exclude rete-bench
```

The development image contains the pinned Rust, Python and WebAssembly
tooling. The build produces the CLI; the tests check the Rust workspace,
excluding the benchmark crate. Client packages and browser behavior have
their own checks, explained in [Tests, coverage and CI](testing.md).

Use a distinct `COMPOSE_PROJECT_NAME` in each worktree so builds do not share
their target directory. The
[contributor guide](https://github.com/caviri/rete/blob/main/CONTRIBUTING.md)
has the full local workflow, including formatting and generated artifacts.

## Change the documentation

Edit Markdown in `docs/`, then regenerate the site:

```sh
docker compose run --rm dev cargo test -p docgen
docker compose run --rm dev cargo run -q -p docgen
docker compose run --rm dev python3 scripts/check_docs_links.py
```

Give each page a clear audience and task. Explain what an example accomplishes
and how to recognize its result. Link to canonical reference material rather
than repeating it, and preserve existing heading links when changing titles.
Keep authored documentation and generated HTML in separate commits.

These development pages describe the checked-in tooling, reviewed on
2026-11-01. For the status of a particular commit, use its
[CI run](https://github.com/caviri/rete/actions/workflows/ci.yml). Keep this guide
in step with changes to the workspace, check scripts and workflows; recorded
benchmark results retain their original measurement context.
