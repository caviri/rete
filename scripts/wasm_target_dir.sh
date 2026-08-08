#!/usr/bin/env bash
# The CARGO_TARGET_DIRs the wasm builds — and *only* the wasm builds — use.
# `source` this from the repository root; it exports
#
#   RETE_WASM_TARGET_DIR        the plain `wasm-pack build` builds (web/pkg*)
#   RETE_WASM_ASYNC_TARGET_DIR  the Asyncify build (different RUSTFLAGS, nightly,
#                               -Z build-std — a different configuration entirely)
#
# and leaves CARGO_TARGET_DIR itself alone, so the HOST builds that surround them
# (tests/gate/fixtures.sh compiles rete-cli) keep their own warm cache.
#
# WHY THE WASM BUILD GETS A DIR OF ITS OWN
#
# Commit 35adffeb reports a regeneration whose wasm came out byte-for-byte the
# same SIZE as CI's with 13 differing bytes out of 3,254,668 — data-symbol
# addresses off by exactly 4 — from a CARGO_TARGET_DIR that had also built the
# workspace for the HOST triple (`cargo test --workspace`, `cargo clippy
# --all-targets`). Rebuilding into a dedicated empty dir reproduced CI exactly.
#
# Be honest about the evidence: that mechanism did NOT reproduce when this file
# was written. On rustc 1.92.0 / wasm-pack 0.15.0, seven builds of
# web/pkg/rete_wasm_bg.wasm all came out af18d6ec…, equal to CI's uploaded
# artifact — two clean ones at different paths, one after `cargo build --release
# -p rete-cli`, one after `cargo clippy --workspace --all-targets` plus
# `cargo test --workspace`, one after the whole workspace `--all-targets`, one
# after the Asyncify build, and one full `build_wasm.sh` run in a dir carrying
# all of the above. So cargo looks robust to this today.
#
# It is still worth the isolation. Cargo promises nothing about sharing a target
# dir between triples; the report is specific, was expensive to diagnose, and
# arrives as `Binary files a/docs/engine/rete_wasm_bg.wasm and b/… differ` six
# minutes after you push, indistinguishable from genuinely stale artifacts; and
# the price is one 1m41s first build per target dir and ~510 MB. A private dir
# also spares the wasm build the full recompile a host build in the same dir
# triggers (measured: ~20 s of pointless work per interleave).
#
# The dirs hang off whatever CARGO_TARGET_DIR the caller arranged rather than a
# fixed path, so they stay on the same fast volume (compose mounts /target; CI
# uses /work/target) instead of landing in a slow bind mount. That is safe
# because the output is invariant to the target dir PATH: two clean builds at
# /work/dev/t1 and /work/dev/wg-a-much-longer-target-path/t2 are byte-identical
# to each other and to CI's artifact.

RETE_WASM_TARGET_BASE="${CARGO_TARGET_DIR:-$PWD/target}"
RETE_WASM_TARGET_DIR="${RETE_WASM_TARGET_DIR:-$RETE_WASM_TARGET_BASE/wasm32}"
RETE_WASM_ASYNC_TARGET_DIR="${RETE_WASM_ASYNC_TARGET_DIR:-$RETE_WASM_TARGET_BASE/wasm32-asyncify}"

# Belt and braces. Nothing but the two build scripts writes these dirs, so the
# usual route to contamination is closed by construction — but a caller can
# still point CARGO_TARGET_DIR straight at one and run the test suite in it, and
# that must fail loudly instead of shipping 13 wrong bytes. `release/deps` holds
# host build-script and proc-macro output even in a pure wasm build, so the
# discriminator is a host rlib/binary of a WORKSPACE crate, which a
# `--target wasm32-unknown-unknown` build never puts there.
rete_wasm_target_dir_guard() {
  local dir="$1" what="$2"
  local hit=""
  if [ -d "$dir/debug" ]; then
    hit="$dir/debug (a dev-profile host build)"
  elif compgen -G "$dir/release/deps/librete_core-*.rlib" >/dev/null; then
    hit="$dir/release/deps/librete_core-*.rlib (a release host build)"
  fi
  if [ -z "$hit" ]; then
    return 0
  fi
  {
    echo "✗ the $what target dir carries HOST-triple build output:"
    echo "    $hit"
    echo "  Something ran cargo for the host triple in here — cargo test, cargo clippy"
    echo "  --all-targets, or cargo build without --target. Nothing fails when it does;"
    echo "  the reported outcome is a wasm of the right SIZE with a handful of wrong"
    echo "  bytes (35adffeb), reaching you as an opaque binary diff in CI's parity job."
    echo "  Fix:"
    echo "    rm -rf '$dir'"
    echo "  and leave CARGO_TARGET_DIR pointing at the dir the HOST builds use — the"
    echo "  wasm build derives its own from it."
  } >&2
  return 1
}

rete_wasm_target_dir_guard "$RETE_WASM_TARGET_DIR" wasm || exit 1
rete_wasm_target_dir_guard "$RETE_WASM_ASYNC_TARGET_DIR" asyncify || exit 1

export RETE_WASM_TARGET_DIR RETE_WASM_ASYNC_TARGET_DIR
