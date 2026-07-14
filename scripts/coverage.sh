#!/bin/sh
# Enforce the 1.0 release coverage floors inside the pinned rete-dev image.
#
#   docker compose run --rm dev sh scripts/coverage.sh
#   docker compose run --rm dev sh scripts/coverage.sh --html
set -eu

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov is missing; run this script in the rete-dev Compose service" >&2
  exit 2
fi

html=false
if [ "${1:-}" = "--html" ]; then
  html=true
  shift
fi

cargo llvm-cov clean --workspace
cargo llvm-cov -p rete-core --all-targets --fail-under-lines 90 --summary-only "$@"
cargo llvm-cov -p rete-cli --all-targets --fail-under-lines 75 --summary-only "$@"

if [ "$html" = true ]; then
  cargo llvm-cov \
    -p rete-core \
    -p rete-cli \
    --all-targets \
    --html \
    --output-dir target/llvm-cov/html \
    "$@"
  echo "HTML report: target/llvm-cov/html/index.html"
fi
