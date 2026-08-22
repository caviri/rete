# The regression gate

**Rule: no commit to playground or engine files without a green gate.**

One command runs the full matrix that has actually broken in the field
(iOS traps, settings overflow, clipboard, cache clearing, catalog syntax):

```sh
bash tests/gate/gate.sh              # full gate (~4 min)
bash tests/gate/gate.sh fast         # static + node engine harness (~15 s)
bash tests/gate/gate.sh --only=worldcup   # one browser check while iterating
bash tests/gate/gate.sh --deployed   # + probe the live GitHub Pages site
bash tests/gate/gate.sh --local      # no live R2 reads (the pull-request CI mode)
```

The optional Compose services add broader, slower coverage:

```sh
docker compose run --rm gate-catalog       # Chromium: 73 queries, 11 embedded datasets
docker compose run --rm gate-catalog-live  # Chromium: all 466 queries, 65 datasets + live R2
docker compose run --rm gate-firefox       # the regular browser matrix in Firefox

# The exhaustive live sweep can use Firefox too, or target one dataset while iterating.
docker compose run --rm -e RETE_BROWSER=firefox gate-catalog-live
docker compose run --rm gate node run.mjs --catalog=all --catalog-dataset=worldcup
```

Those services run `node run.mjs` directly, so they do **not** produce the
fixtures (the Playwright image has no Rust). Run `bash tests/gate/fixtures.sh`
once first — the G0 provenance check says so by name if you forget.

The catalog sweeps load the generated playground, select each example through
the rendered Query Library, run it through the editor/worker/WASM path, and wait
for the real result renderer. Successful zero-row results are valid; parse,
engine, transport, page-script, timeout, and browser-limit failures are not. The
live sweep uses the reliable reader and retries each remote example once; the
normal G2 matrix independently covers the concurrent Asyncify reader.
Progress streams case by case. The final machine-readable report is written to
`tests/gate/.cache/catalog-report-<browser>-<scope>.json`; final failures also
leave screenshots under `tests/gate/.cache/catalog-failures/`.

Requires Docker (uses the `mcr.microsoft.com/playwright:v1.49.0-jammy` image)
and network (the lazy checks read the live R2 datasets).
The gate's local Range servers bind OS-assigned ports, so gates from separate
worktrees can run concurrently without one suite reading another checkout.

### From a git worktree: name the Compose project

`fixtures.sh` shells out to `docker compose run --rm dev` whenever cargo is not
on PATH — which is the normal host case — and `compose.yaml` pins the project
name to `rete`. Every worktree therefore builds into the **same**
`rete_cargo-target` volume: two checkouts running the gate at once fight over
one `/target`, and each leaves build output the other picks up. Give the
worktree its own project, and its own volumes come with it:

```sh
export COMPOSE_PROJECT_NAME=my-worktree     # -> my-worktree_cargo-target
```

The ports are already handled (see above); this is the other half.

## From a fresh clone

`gate.sh` needs two things a clone does not carry, both build output:

* **the compiled engine** — `web/pkg*` is gitignored. `gate.sh` checks for it
  first and stops with the command to build it (`docker compose run --rm wasm`)
  rather than letting the two G0 checks that read it fail as if the engine were
  broken.
* **the `.rete` fixtures** — built for you, see below. Nothing is downloaded.

## Fixtures — one producer, a recipe, and a verified result

```sh
bash tests/gate/fixtures.sh            # build (if stale) + verify — gate.sh does this for you
bash tests/gate/fixtures.sh --force    # rebuild unconditionally
bash tests/gate/fixtures.sh --verify   # verify what is on disk, build nothing
```

`tests/gate/fixtures/manifest.json` is the whole contract: for each fixture, the
tracked source it is built from, the checks that read it, and the properties
those checks silently depend on — quad count, named-graph count, card
present/absent, build record, which curated card fields must be there and which
must not. `fixtures.sh` builds from the recipe and verifies against the
assertions, so **a wrong fixture fails naming itself**, with the command that
repairs it, instead of reddening whichever check happens to notice.

It is the *only* producer: `gate.sh`, `scripts/build_wasm.sh` and CI all call it.
Three separate copies of these build commands is how they drifted:

* `gate.sh` used to **download** `tests/gate/.cache/worldcup2026.rete` from R2
  when it was missing. The published dataset of that name is a different graph —
  16,184 triples **with** a Dataset Card — while the recipe builds a 7-triple
  **cardless** file, and `check_card_modal` asserts cardless. A fresh clone got
  `a cardless file did not say so` and could not go green from `gate.sh` alone.
  Nothing is downloaded now. (The live-R2 G2 checks still read the *published*
  datasets through the playground catalog — that is a deliberate integration
  test, and a different thing from a fixture.)
* A `rete-cli` older than PR #161 accepts a card file carrying `version`,
  `creators`, `publisher`, `doi`, `cite_as`, `keywords`, `theme`, `extra`,
  `canonical_url`, `sparql_endpoint`, `derived_from`, `source_date`, exits 0,
  prints `embedded dataset card (N bytes of metadata)` — and writes none of
  them, nor any build record. `fixtures.sh` **probes the binary first** on a
  throwaway file and refuses to run, naming the fields it dropped.

The producer writes `tests/gate/.cache/fixtures.stamp.json` (recipe hash, source
hashes, per-fixture sha256 + builder). `check_fixture_provenance.mjs` re-checks
it in G0, which is what protects the paths that call `node run.mjs` directly —
`docker compose run --rm gate`, `gate-firefox`, the catalog sweeps. The stamp is
not committed: a build record carries a timestamp and measured milliseconds, so
two builds of the same recipe are legitimately different bytes.

## Writing a check

The runner reads a check's **last JSON line**, not its exit code
(`lastJson(stdout).verdict === "PASS"`). A check that dies on a bare
`assert.equal` prints no verdict at all, so the log gets a 160-character slice of
a Node stack trace instead of the numbers. Collect the assertions with
`checks/_expect.mjs` and always print a verdict:

```js
import { expect } from "./_expect.mjs";
const t = expect("test_catalog_matrix");
t.equal("allQueries", all.length, 676, "every catalog query must be in the matrix");
t.finish({ allQueries: all.length });
```

PASS prints `{"verdict":"PASS", …payload}` as before. FAIL prints
`{"verdict":"FAIL","failures":[{"check":"allQueries","actual":676,"expected":669,…}], …}`
plus one compact stderr line, and still exits 1 — so a stale tripwire tells you
which number went stale and what it is now.

## Tiers

| Tier | What it verifies | Time |
|---|---|---|
| **G0 static** | the `.rete` fixtures are byte-for-byte the ones `fixtures.sh` built from `fixtures/manifest.json`, on the same sources (nothing substituted or left over from an older recipe); `app.js` / `catalog.js` parse; every inline `<script>` of the **built** `docs/playground.html` parses (one bad char blanks the whole playground); all catalog example queries declare every prefix they use; every example and dataset has a share page + card image and no page's `og:image` 404s or is relative; no dataset advertises a **full-text index** it has not declared with `textIndex: true` (and none hides one it has) | ~5 s |
| **G1 engine-in-node** | the **production async wasm + Asyncify driver** (`docs/rete_wasm_async.js`) answers a lazy query with 4 OPTIONALs + ORDER BY cast over a local range server — no browser, catches a broken/stale async build immediately | ~10 s |
| **G2 browser matrix** | see below | ~4 min |
| **G2 catalog** (optional) | every catalog query through the real playground: 73 embedded, or all 431 including live R2 | minutes to hours |
| **G3 engine** (manual) | `cargo test -p rete-core --release` in Docker; then run the canonical WASM build, which rebuilds and boots both regular targets, rebuilds Asyncify, and regenerates the playground | ~min |

## G2 matrix — the combinations that break independently

| Check | Load | Variant | Device |
|---|---|---|---|
| `check_diag` | embedded | sync | desktop |
| `check_worldcup` | remote-lazy (live R2) | **default (async)** | desktop |
| `check_lazy_async` | remote-lazy (live R2) | async forced · GROUP BY | desktop |
| `check_sync_read` | remote-lazy (live R2) | **sync forced** | desktop |
| `check_ios_default` | remote-lazy (live R2) | auto → must pick sync | **iPhone UA** |
| `check_settings_mobile` | embedded | — | **390px phone** |
| `check_copy` | embedded | — | clipboard read-back |
| `check_clear` | — | — | IndexedDB + Cache API |
| `check_worker_init` | remote-lazy | async (corrupted) | broken wasm → error, no hang |
| `check_refresh_session` | embedded | — | ↻ Refresh really reloads |
| `check_async_fallback` | remote-lazy | async 404 → sync | degrade, don't hard-fail |
| `check_query_shapes` | embedded | — | property paths + CONSTRUCT→graph + reasoning |
| `check_boe_reason` | remote-lazy (live R2) | reliable sync | OWL 2 QL: 0 rows off → rows on |
| `check_map_geo` | embedded + local PMTiles | local renderer boundary | GeoSPARQL → Tiles canvas, layer, bounds |
| `check_service_success` | embedded + local endpoint | sync | successful `SERVICE` local/remote join |
| `check_builder` | in-browser build | sync | N-Quads → `.rete` → named-graph Alice query |
| `check_cache_mode` | remote-cache (local host) | sync | one full download, then reload with zero reads |
| `check_optional_tabs` | embedded + local stubs | no WebGPU/model downloads | Ask AI + Semantic/RAG controls and copy |

Live-R2 checks retry any no-rows outcome (`_util.runWithRetry`) so a transient CDN
blip doesn't red the gate; a real regression fails every retry and still goes red.
Pull requests use `--local`, which still runs G0, the local ranged G1 fixture, and
all embedded/local G2 checks. Pushes to `main` and `release-*` run the complete
live-R2 matrix.

## After an engine (crates/) change — checklist

1. `cargo test --release -p rete-core` (Docker, `CARGO_TARGET_DIR=/work/target-star`).
2. Rebuild **everything browser-facing** with the one producer:

   ```sh
   docker compose run --rm -e RETE_SOURCE_REVISION=$(git rev-parse HEAD) wasm
   ```

   It runs both sync `wasm-pack` builds, the **async** one — which
   `build_playground.py` only *copies*, so an engine change silently leaves it
   stale if you skip it — `build_playground.py`, `docs/engine/`, the gate
   fixtures and `docs/wasm-build.json`, in that order. It also does two things
   the hand-run `wasm-pack` lines do not: build into wasm-only target dirs
   (`scripts/wasm_target_dir.sh`), boot-check both generated initializers, and
   stamp the page with the workspace version. CI reruns the same script and
   byte-diffs its tracked output.
3. `bash tests/gate/gate.sh` → green → commit.

If that byte diff goes red, `python3 -P scripts/wasm_parity_triage.py` reads the
bytes and says which of the three it is — genuinely stale artifacts, a wrong
build stamp, or a wasm that merely moved in a shared target dir. CI runs it for
you on failure and uploads its own byte-exact artifacts either way.

## After a playground (web/) change

1. `python scripts/build_playground.py`.
2. `bash tests/gate/gate.sh` (or `fast` + `--only=<check>` while iterating,
   full gate before the commit).
3. If you added or edited a catalog example or dataset, refresh its link
   preview — `scripts/preview/run.sh capture --dataset=<key>` then
   `scripts/preview/run.sh build`. G0 goes red on a missing share page or card.

## Known coverage gaps

The fast/default gate samples representative datasets; the optional catalog gate
executes every example. The remaining deliberate gap is heavyweight local AI:

The optional AI gate deliberately stops at initialization and deterministic local
worker stubs. Real WebGPU inference and multi-hundred-megabyte model downloads stay
outside the release gate; they require a separate manual/device smoke test.

## Device-specific bugs the gate can't catch

Real iOS Safari (JSC) differs from anything runnable here (headless WebKit
included) — e.g. the asyncify-variant wasm trap only reproduces on a real iPhone.
When a user reports a phone-only failure: get the **📋 Copy full log** report from
the error box — it carries the wasm variant, load mode, device and the full stack.
Add the failing shape to this gate once diagnosed.
