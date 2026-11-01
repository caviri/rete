# Contributing

Use the development container to build rete itself. It contains the Rust,
Python and WebAssembly tooling used by the project, so you do not need to
reconstruct the toolchain on your host.

If you only want to use rete, start with the [Quickstart](getting-started.md)
or a [language client](clients.md).

## Build and test

From a checkout of the repository:

```sh
docker compose build dev
docker compose run --rm dev cargo build --release -p rete-cli
docker compose run --rm dev cargo test --workspace --exclude rete-bench
```

The [contributor guide](https://github.com/caviri/rete/blob/main/CONTRIBUTING.md)
covers formatting, linting, generated artifacts and the regression gate.
Use a distinct `COMPOSE_PROJECT_NAME` when working in another worktree so
builds do not share their target directory.

## Find the right part of the project

[Architecture](architecture.md) maps the crates and execution paths.
The [format specification](SPEC.md) and [compatibility reference](compatibility.md)
describe the contracts an implementation change must preserve.

[Client development](clients-dev.md) covers the language bindings and their
packaging. The [release guide](release.md) owns the release procedure;
publishing a dataset is a separate workflow described in
[Build and publish](build-and-publish.md).

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
