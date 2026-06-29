#!/usr/bin/env bash
# Build the ASYNCIFY variant of rete-wasm: every remote range read goes through one
# async import (env.rete_fetch_ranges = a Promise.all of fetch), and Binaryen's
# Asyncify pass makes the synchronous engine SUSPEND/RESUME around it — concurrent
# reads with NO SharedArrayBuffer / cross-origin isolation (works on a static CDN).
#
# Output: web/pkg-nomodules-async/ — a SEPARATE artifact; the default web/pkg-nomodules
# (built by build_playground.py) is untouched. Load it only behind the Settings toggle.
#
# Hard-won toolchain facts (see dev/asyncify-wbg-probe + the async-asyncify-reads memo):
#   * Asyncify CANNOT instrument functions with reference types (externref). The
#     default wasm-bindgen ABI (Rust 1.82+) uses externref for JsValue, so the asyncify
#     pass dies with "Asyncify does not yet support non-number types, like references".
#   * Fix: disable the `reference-types` target feature. But the PRECOMPILED wasm std
#     still carries it, so you must recompile std too → nightly + `-Z build-std`.
#   * Keep `multivalue` ON (don't add `,-multivalue`): the multi-value String-return ABI
#     is what lets the worker call the wasm-bindgen glue wrapper repeatedly in the
#     suspend/rewind loop safely (the unwind pass reads+frees a harmless [0,0]).
#   * The async import must be declared `#[link(wasm_import_module = "env")] extern "C"`
#     in lib.rs (feature = "asyncify") so rust-lld emits it as an import, not a symbol.
#
# Run in the rete-asyncify image (FROM rete-dev + nodejs + binaryen):
#   docker run --rm --user root -v D:/pro/rete:/work -w /work rete-asyncify:latest \
#     bash scripts/build_playground_async.sh
set -euo pipefail
cd "$(dirname "$0")/.."

# nightly + rust-src for build-std (the image's default toolchain is stable)
if ! rustup run nightly true 2>/dev/null; then
  rustup toolchain install nightly --profile minimal -c rust-src
  rustup target add wasm32-unknown-unknown --toolchain nightly
fi

export RUSTFLAGS="-Ctarget-feature=-reference-types"   # NOT -multivalue (see above)
rustup run nightly wasm-pack build crates/rete-wasm \
  --target no-modules --out-dir ../../web/pkg-nomodules-async \
  -- --features asyncify -Z build-std=panic_abort,std

RAW=web/pkg-nomodules-async/rete_wasm_bg.wasm
node -e "const m=new WebAssembly.Module(require('fs').readFileSync('$RAW'));if(WebAssembly.Module.imports(m).some(i=>/externref/.test(i.name)))throw new Error('externref still present — asyncify will fail');console.log('externref gone, env.rete_fetch_ranges import present')"

wasm-opt --asyncify --pass-arg=asyncify-imports@env.rete_fetch_ranges "$RAW" -o "$RAW.async"
mv "$RAW.async" "$RAW"
echo ">> asyncified: $(stat -c%s "$RAW") bytes; asyncify control fns: $(node -e "const m=new WebAssembly.Module(require('fs').readFileSync('$RAW'));console.log(WebAssembly.Module.exports(m).filter(e=>/asyncify/.test(e.name)).map(e=>e.name).join(', '))")"
