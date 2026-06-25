#!/bin/sh
# Line/region coverage for the workspace — the SQLite "100% coverage" north star.
# Run inside the rete-dev container (self-bootstraps cargo-llvm-cov + llvm-tools
# the first time, ~3 min; bake them into the image to skip it):
#   docker run --rm --user root -v "$PWD":/work -w /work rete-dev:latest sh scripts/coverage.sh
#
#   sh scripts/coverage.sh            # per-crate summary table
#   sh scripts/coverage.sh --html     # browsable report -> target/llvm-cov/html
set -e
command -v cargo-llvm-cov >/dev/null 2>&1 || {
  rustup component add llvm-tools-preview
  cargo install cargo-llvm-cov
}
PKGS="-p rete-core -p rete-cli"
if [ "$1" = "--html" ]; then
  shift
  cargo llvm-cov $PKGS --html "$@"
  echo "HTML report: target/llvm-cov/html/index.html"
else
  cargo llvm-cov $PKGS --summary-only "$@"
fi
