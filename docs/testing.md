# Tests, coverage and CI

A useful test answers a specific question: does this query return the right
rows, can an older reader open this file, or does the browser fetch the right
bytes? Rete checks these at several levels. A green Rust test run is one part
of the evidence; browser behavior and language packages need their own checks.

## Choose checks for your change

Start with the tests nearest the behavior you changed. For example, a core
query change belongs with the Rust tests; a Python conversion belongs with
the Python client tests. Before handing off an engine change, also run the
broader checks required by the
[contributor guide](https://github.com/caviri/rete/blob/main/CONTRIBUTING.md).

From a checkout with the development image built:

```sh
docker compose run --rm dev cargo test -p rete-core
docker compose run --rm dev bash scripts/release_check.sh quality
```

The first command runs the core package's default tests. The second adds
formatting, linting, workspace tests, feature checks, a differential query
oracle, CLI smoke tests and the small query benchmark's row-count and timing
checks. Its generous time ceilings catch major slowdowns; they are not a
fine-grained performance comparison.

```sh
docker compose run --rm check
```

This runs the `quality`, `security` and `docs` stages of
[`release_check.sh`](https://github.com/caviri/rete/blob/main/scripts/release_check.sh).
It does **not** run every CI job: coverage, browser tests, W3C conformance,
interop and package checks are separate. Engine and playground changes also
need the canonical browser build and
[browser regression gate](https://github.com/caviri/rete/blob/main/tests/gate/README.md).

## Read coverage as a measurement

Line coverage tells you which instrumented source lines were exercised.
It does not establish that every assertion is useful, every input is safe,
or every supported query is correct.

| Package | Required line coverage | Where the rule lives |
|---|---:|---|
| `rete-core` | at least 90% | `scripts/coverage.sh` |
| `rete-cli` | at least 75% | `scripts/coverage.sh` |

These are the **minimums**, not the latest measured percentages. For the
measurement, open the [CI runs](https://github.com/caviri/rete/actions/workflows/ci.yml),
select the run for your commit, then read **coverage → Enforce core and CLI
coverage floors**. Check that the job actually ran. CI currently prints its
summaries in the log; it does not publish a coverage HTML artifact or a
repository-wide coverage percentage.

To measure your checkout and inspect uncovered lines locally:

```sh
docker compose run --rm dev sh scripts/coverage.sh --html
```

The script enforces the two package floors, then writes the detailed report
to `target/llvm-cov/html/index.html` in your checkout. Without `--html`, it
prints the summaries only. These runs measure native Rust execution; they
do not measure browser JavaScript, WASM execution or all language clients.
The [coverage script](https://github.com/caviri/rete/blob/main/scripts/coverage.sh)
is the source of truth if these thresholds change.

[SPARQL conformance](conformance.md) asks a different question: how many cases
from a pinned standards suite pass? Keep that score separate from line
coverage and from [performance measurements](benchmarks.md).

## Understand a CI run

The [main workflow](https://github.com/caviri/rete/blob/main/.github/workflows/ci.yml)
first detects which parts of the repository changed. It then runs the
relevant jobs using the pinned development image where applicable.

| Checks | What they help establish |
|---|---|
| Quality and coverage | Rust behavior, supported feature builds and exercised lines |
| WASM and browser | Browser artifacts match their source; browser checks use that exact build |
| Interop and conformance | Behavior against real external tools and the pinned W3C suite |
| Security | Dependency advisories, licences and allowed dependency sources |
| MSRV and package | The declared minimum Rust version and distributable package contents |
| Docs | Warning-free Rust documentation, regenerated HTML and valid local links |
| Shell and export-driver checks | Script correctness and the dataset export workflow |

A skipped job means it was not selected, not that its tests passed. Ordinary
`docs/` prose changes select the docs checks. Changes under `crates/`, including
docgen, currently match both the Rust and WASM filters. Manual dispatch runs
the main job set; the nightly run selects security, WASM and browser checks,
not the entire matrix or coverage.

On a PR, browser checks use local fixtures unless the change touches the live
dataset inputs identified by the workflow. The full run also checks published
datasets, whose availability can change independently of the code. Logs and
the job summary identify the local-only skips.

## Follow a change through delivery

Python, JavaScript, R, Java, MCP bundle, relay and explorer checks have
[separate workflows](https://github.com/caviri/rete/tree/main/.github/workflows).
They are not all included in `cargo test --workspace`. Scheduled fuzzing
looks for reader failures, while scheduled catalog checks verify hosted data.
These checks complement one another; none is an exhaustive proof of behavior.

After a relevant merge to `main`, the
[Pages workflow](https://github.com/caviri/rete/blob/main/.github/workflows/pages.yml)
publishes the tracked site and checks the deployed revision. It does not
deploy a full documentation preview for each PR. Package releases are a
separate, version-tagged process described in the [release guide](release.md).

When a check fails, read the first failing step for the exact commit and
reproduce that check locally. Keep its dataset, execution mode and toolchain
the same before deciding whether the failure is in the code or environment.
