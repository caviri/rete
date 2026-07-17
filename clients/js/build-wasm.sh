#!/usr/bin/env bash
# Build the wasm engine for the npm package FRESH from the checked-out crates
# (wasm-pack --target web into vendor/pkg). The repo's committed web/pkg
# artifacts belong to the playground's own build pipeline and can lag the
# engine sources; the npm package must always match the sources it ships from.
set -euo pipefail
cd "$(dirname "$0")"

command -v wasm-pack >/dev/null 2>&1 || {
    curl -sSf https://rustwasm.github.io/wasm-pack/installer/init.sh | sh >/dev/null
}

# --no-opt mirrors scripts/build_wasm.sh: the repo's binaryen pass is only for
# the Asyncify variant; regular builds rely on the Rust release profile.
wasm-pack build ../../crates/rete-wasm --target web --no-opt \
    --out-dir ../../clients/js/vendor/pkg

ls -la vendor/pkg/
