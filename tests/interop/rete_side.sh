#!/usr/bin/env bash
# The rete half of tests/interop/oxigraph.sh — everything that runs INSIDE the
# repo's dev container. Kept as a file rather than a heredoc so the caller can
# launch it with either `docker compose run --rm dev` (local) or a plain
# `docker run <pinned image>` (CI), and so it is reviewable on its own.
#
#   bash tests/interop/rete_side.sh export     # build + export every fixture
#   bash tests/interop/rete_side.sh rebuild    # rebuild from Oxigraph's dump
#
# Every command's stdout, stderr and exit code are written next to each other in
# dev/interop/, so the assertions in oxigraph.sh read evidence instead of
# re-running anything — and a failure can be inspected after the fact.
set -u

STAGE="${1:-export}"
cd "$(dirname "$0")/../.." || exit 2
ROOT="$PWD"
BIN="${CARGO_TARGET_DIR:-$ROOT/target}/release/rete"
OUT="$ROOT/dev/interop"
mkdir -p "$OUT"

cargo build --release -q -p rete-cli || exit 90
cd "$OUT" || exit 2

# run NAME -- CMD...  →  NAME.out, NAME.err, NAME.code
run() {
  local name="$1"; shift; [ "$1" = "--" ] && shift
  "$@" >"$name.out" 2>"$name.err"
  echo $? >"$name.code"
}

case "$STAGE" in
  export)
    run build_repairable    -- "$BIN" build repairable.nt   -o repairable.rete
    run build_strict        -- "$BIN" build repairable.nt   -o strict.rete --strict
    run build_unrepairable  -- "$BIN" build unrepairable.nt -o unrepairable.rete
    run build_named         -- "$BIN" build named.nq        -o named.rete
    run validate_repairable -- "$BIN" validate repairable.nt

    run export_raw   -- "$BIN" export repairable.rete   --format nq
    run export_clean -- "$BIN" export repairable.rete   --format nq --sanitize-iris
    run export_unrep -- "$BIN" export unrepairable.rete --format nq --sanitize-iris
    run export_named -- "$BIN" export named.rete        --format nq

    cp export_raw.out   raw.nq
    cp export_clean.out clean.nq
    cp export_unrep.out unrepairable-sanitized.nq
    cp export_named.out named-export.nq
    ;;
  rebuild)
    # The other direction of the cycle docs/interop.md documents: take what
    # Oxigraph dumped and build a .rete from it.
    run build_back  -- "$BIN" build named-back.nq -o named-back.rete
    run export_back -- "$BIN" export named-back.rete --format nq
    cp export_back.out named-back-export.nq

    run build_repaired_back  -- "$BIN" build clean-back.nq -o clean-back.rete
    run export_repaired_back -- "$BIN" export clean-back.rete --format nq
    cp export_repaired_back.out clean-back-export.nq
    ;;
  *)
    echo "unknown stage: $STAGE (expected 'export' or 'rebuild')" >&2
    exit 2
    ;;
esac
