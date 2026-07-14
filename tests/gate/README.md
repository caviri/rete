# The regression gate

**Rule: no commit to playground or engine files without a green gate.**

One command runs the full matrix that has actually broken in the field
(iOS traps, settings overflow, clipboard, cache clearing, catalog syntax):

```sh
bash tests/gate/gate.sh              # full gate (~4 min)
bash tests/gate/gate.sh fast         # static + node engine harness (~15 s)
bash tests/gate/gate.sh --only=worldcup   # one browser check while iterating
bash tests/gate/gate.sh --deployed   # + probe the live GitHub Pages site
```

Requires Docker (uses the `mcr.microsoft.com/playwright:v1.49.0-jammy` image)
and network (the lazy checks read the live R2 datasets).
The gate's local Range servers bind OS-assigned ports, so gates from separate
worktrees can run concurrently without one suite reading another checkout.

## Tiers

| Tier | What it verifies | Time |
|---|---|---|
| **G0 static** | `app.js` / `catalog.js` parse; every inline `<script>` of the **built** `docs/playground.html` parses (one bad char blanks the whole playground); all catalog example queries declare every prefix they use | ~5 s |
| **G1 engine-in-node** | the **production async wasm + Asyncify driver** (`docs/rete_wasm_async.js`) answers a lazy query with 4 OPTIONALs + ORDER BY cast over a local range server — no browser, catches a broken/stale async build immediately | ~10 s |
| **G2 browser matrix** | see below | ~4 min |
| **G3 engine** (manual) | `cargo test -p rete-core --release` in Docker; then rebuild **both** wasm variants + `build_playground.py` (the async variant is NOT rebuilt automatically — see below) | ~min |

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

## After an engine (crates/) change — checklist

1. `cargo test --release -p rete-core` (Docker, `CARGO_TARGET_DIR=/work/target-star`).
2. Rebuild the sync wasm (`wasm-pack`, web/pkg + web/pkg-nomodules).
3. Rebuild the **async** wasm: `scripts/build_playground_async.sh`
   (`build_playground.py` only *copies* `web/pkg-nomodules-async/` — an engine
   change silently leaves the async variant stale otherwise).
4. `python scripts/build_playground.py`.
5. `bash tests/gate/gate.sh` → green → commit.

## After a playground (web/) change

1. `python scripts/build_playground.py`.
2. `bash tests/gate/gate.sh` (or `fast` + `--only=<check>` while iterating,
   full gate before the commit).

## Known coverage gaps (not yet gated)

Honest list of playground surfaces with no runtime check — add one when you touch
them or when a bug lands there:

- **Coverage breadth**: only ~2 datasets are exercised live (worldcup2026, mtg) and
  one embedded (scholar); the other datasets' example queries are *statically*
  prefix-scanned (G0) but not executed. A broken example on another dataset ships
  green until someone runs it.

The optional AI gate deliberately stops at initialization and deterministic local
worker stubs. Real WebGPU inference and multi-hundred-megabyte model downloads stay
outside the release gate; they require a separate manual/device smoke test.

## Device-specific bugs the gate can't catch

Real iOS Safari (JSC) differs from anything runnable here (headless WebKit
included) — e.g. the asyncify-variant wasm trap only reproduces on a real iPhone.
When a user reports a phone-only failure: get the **📋 Copy full log** report from
the error box — it carries the wasm variant, load mode, device and the full stack.
Add the failing shape to this gate once diagnosed.
