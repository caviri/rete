#!/usr/bin/env bash
# EXPERIMENTAL: build the *multithreaded* wasm playground bundle into
# web/pkg-threads using wasm-bindgen-rayon (real browser threads via Web Workers
# + SharedArrayBuffer).
#
# This is NOT the normal build. The default offline playground (docs/playground.html
# from web/pkg-nomodules) and the normal web/pkg build are untouched by this.
#
# REQUIREMENTS (all satisfied inside the Docker container, never on host):
#   - nightly toolchain + rust-src component (for -Z build-std)
#   - RUSTFLAGS enabling wasm atomics / shared memory
#   - wasm-pack
# The output page (web/playground-threads.html) MUST be served cross-origin
# isolated (COOP/COEP) — use scripts/serve_coi.py. It cannot run from file://.
#
# Run via Docker from the repo root (see CLAUDE.md):
#   MSYS_NO_PATHCONV=1 docker run --rm -v "/d/pro/rete":/work -w /work \
#     -v rete-cargo-registry:/usr/local/cargo/registry \
#     -v rete-cargo-bin:/usr/local/cargo/bin \
#     rust:1.92-bookworm bash -c 'bash scripts/build_playground_threads.sh'
set -euo pipefail

echo ">> ensuring nightly toolchain + rust-src (for -Z build-std)…"
rustup toolchain install nightly -c rust-src >/dev/null 2>&1 || \
  rustup toolchain install nightly -c rust-src

echo ">> ensuring wasm-pack is installed…"
if ! command -v wasm-pack >/dev/null 2>&1; then
  cargo install wasm-pack --locked
fi

# Atomics + bulk-memory + mutable-globals are required for shared-memory wasm
# threading; wasm-bindgen-rayon relies on them.
export RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"

echo ">> building threaded wasm into web/pkg-threads (nightly, build-std)…"
# wasm-pack forwards everything after `--` to `cargo build`. We pass
# -Z build-std there (needs nightly) plus our `threads` feature.
rustup run nightly wasm-pack build crates/rete-wasm \
  --target web \
  --out-dir ../../web/pkg-threads \
  -- --features threads -Z build-std=panic_abort,std

echo
echo ">> done. Verify exports:"
grep -o 'export function [A-Za-z_]*' web/pkg-threads/rete_wasm.js | sort -u || true
echo
echo "Serve cross-origin-isolated and open the experimental page:"
echo "  python3 scripts/serve_coi.py 8080 web"
echo "  -> http://localhost:8080/playground-threads.html"
