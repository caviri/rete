#!/usr/bin/env bash
# The one-command regression gate. Run after EVERY playground/engine change:
#
#   bash tests/gate/gate.sh            # full: static + node harness + browser matrix (~4 min)
#   bash tests/gate/gate.sh fast       # static + node harness only (~15 s)
#   bash tests/gate/gate.sh --only=worldcup   # a single browser check
#   bash tests/gate/gate.sh --deployed # also probe the live GitHub Pages site (informational)
#
# Green gate = safe to commit. Red gate = fix before committing.
set -e
export MSYS_NO_PATHCONV=1 MSYS2_ARG_CONV_EXCL='*'
ROOT="$(git rev-parse --show-toplevel)"
mkdir -p "$ROOT/tests/gate/.cache"

# --- preflight 1: the gitignored engine artifacts ------------------------------
# web/pkg* is build output, so a fresh clone has none of it — and the two G0
# checks that read it then fail as "generated WASM API contract: ENOENT" and
# "async wasm present: missing", which read like engine defects. Three separate
# agents diagnosed that from scratch. Say it once, up front, with the command.
missing=0
need() { # need <file> <what reads it> <command that builds it>
  # `if`, not `[ … ] && …`: under `set -e` a failing test as the last command of
  # an && list aborts the script, so the second missing artifact would never be
  # reported — and reporting all of them at once is the entire point here.
  if [ ! -f "$ROOT/$1" ]; then
    if [ "$missing" -eq 0 ]; then
      echo "GATE PREFLIGHT FAILED — the compiled engine is missing (build output, gitignored):" >&2
    fi
    missing=1
    printf '  %s\n      read by : %s\n      build it: %s\n' "$1" "$2" "$3" >&2
  fi
}
need web/pkg-nomodules/rete_wasm.js \
  "G0 check_wasm_api (the documented WASM export surface)" \
  "docker compose run --rm wasm"
need web/pkg-nomodules/rete_wasm_bg.wasm \
  "G0 check_wasm_api (the documented WASM export surface)" \
  "docker compose run --rm wasm"
need web/pkg-nomodules-async/rete_wasm_bg.wasm \
  "G0 async-wasm freshness, G1 asyncify_e2e" \
  "docker compose run --rm wasm-async"
if [ "$missing" -ne 0 ]; then
  echo "Then re-run: bash tests/gate/gate.sh" >&2
  exit 2
fi

# --- preflight 2: the .rete fixtures ------------------------------------------
# ONE producer, shared with scripts/build_wasm.sh and CI: tests/gate/fixtures.sh
# builds every fixture from its tracked source and verifies it against
# tests/gate/fixtures/manifest.json before a single check runs.
#
# This used to be a curl of https://data.graphplaza.com/worldcup2026/worldcup2026.rete
# whenever tests/gate/.cache/worldcup2026.rete was missing. That published file
# is a DIFFERENT graph from the fixture of the same name — 16,184 triples and a
# full Dataset Card, against a 7-triple cardless build — and check_card_modal
# asserts cardless (checks/check_card_modal.mjs:422,441). So a fresh clone got
# "a cardless file did not say so" and could never go green from gate.sh alone.
# The fixture is built from tests/gate/fixtures/worldcup2026.nt now; nothing is
# downloaded. (The live-R2 G2 checks still read the published datasets — that is
# a deliberate integration test, and it is not this file.)
bash "$ROOT/tests/gate/fixtures.sh"

# First run: install the playwright npm package next to the checks (the image
# ships the BROWSERS but not a global npm package; ESM import resolves from here).
if [ ! -d "$ROOT/tests/gate/node_modules/playwright" ]; then
  echo "installing gate deps (first run)…"
  docker run --rm --network host -v "$ROOT:/work" -w /work/tests/gate \
    mcr.microsoft.com/playwright:v1.49.0-jammy \
    bash -c 'PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm i --no-audit --no-fund --loglevel=error'
fi

docker run --rm --network host -v "$ROOT:/work" -w /work/tests/gate \
  mcr.microsoft.com/playwright:v1.49.0-jammy \
  node run.mjs "$@"
