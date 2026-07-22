#!/usr/bin/env bash
# Build rete.mcpb in Docker (no node needed on the host).
#
#   ./build.sh          assemble build/ and pack rete.mcpb
#   ./build.sh --test   ... and run the stdio smoke test first
#
# Assumes clients/js/dist is current; rebuild it with:
#   docker compose run --rm --user root dev bash -c \
#     'wasm-pack build crates/rete-wasm --target web --no-opt --out-dir /work/clients/js/vendor/pkg'
#   docker run --rm -v "$PWD":/w -w /w/clients/js node:22-slim node build.mjs
set -euo pipefail
cd "$(dirname "$0")"
repo="$(cd ../.. && pwd)"

run() { MSYS_NO_PATHCONV=1 docker run --rm -v "$repo":/w -w /w/clients/mcpb node:22-slim "$@"; }

run sh -c '
  set -e
  npm install --no-audit --no-fund --silent
  node build.mjs
  '"$([ "${1:-}" = "--test" ] && echo 'node --test --test-timeout=120000 ./test/*.test.mjs')"'
  npx --yes @anthropic-ai/mcpb@2 pack build rete.mcpb
'

ls -la rete.mcpb
