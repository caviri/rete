#!/usr/bin/env bash
# Publish rete.mcpb to the public R2 bucket — the scripted counterpart of the
# hand-run upload the release ladder used to do (issue #104).
#
#   ./publish.sh                 pack (via build.sh) and publish
#   ./publish.sh --no-build      publish the rete.mcpb already sitting here
#   ./publish.sh --dry-run       print the two object keys, upload nothing
#
# Two keys are written from the same bytes:
#
#   mcpb/rete-<version>.mcpb   the immutable, pinned build
#   mcpb/rete.mcpb             the "current build" pointer clients/mcpb/README.md
#                              and docs/agents.md advertise
#
# Order matters: the pinned key goes up first, so a reader who follows the
# floating key always finds the pinned one already beside it.
#
# The version is read from build/manifest.json — the manifest the bundle was
# actually packed with — not from a flag, so a published pin can never disagree
# with what is inside the file. That is the *built* manifest, which build.mjs
# stamps from the workspace Cargo.toml (build.mjs:111); the source
# manifest.json's version is the inert placeholder the README mentions.
#
# Content type: upload_r2.py falls back to application/octet-stream for an
# unknown extension, which is what .mcpb wants and what the live object already
# serves; nothing needs to be passed.
#
# Env (same contract as skills/rete-publish/scripts/upload_bucket.sh):
#   ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT (or repository .env)
#   RETE_BUCKET   bucket name (default: rete)
set -euo pipefail
cd "$(dirname "$0")"
repo="$(cd ../.. && pwd)"

build=1
dry=0
for arg in "$@"; do
  case "$arg" in
    --no-build) build=0 ;;
    --dry-run) dry=1 ;;
    *) echo "publish.sh: unknown argument: $arg" >&2; exit 2 ;;
  esac
done

[ "$build" = "1" ] && ./build.sh

[ -f rete.mcpb ] || {
  echo "publish.sh: rete.mcpb not found — run ./build.sh first (or drop --no-build)" >&2
  exit 1
}
# build/manifest.json is written by build.mjs and is what `mcpb pack` consumed.
[ -f build/manifest.json ] || {
  echo "publish.sh: build/manifest.json not found — the bundle was not assembled here" >&2
  exit 1
}

version="$(MSYS_NO_PATHCONV=1 docker run --rm -v "$repo":/w -w /w/clients/mcpb node:22-slim \
  node -e 'process.stdout.write(require("./build/manifest.json").version)')"
[ -n "$version" ] || { echo "publish.sh: no version in build/manifest.json" >&2; exit 1; }

echo "publish.sh: rete.mcpb v$version ($(wc -c < rete.mcpb) bytes)"
echo "  -> mcpb/rete-$version.mcpb"
echo "  -> mcpb/rete.mcpb"

if [ "$dry" = "1" ]; then
  echo "publish.sh: --dry-run, nothing uploaded"
  exit 0
fi

# Pinned key first, floating pointer second.
for key in "mcpb/rete-$version.mcpb" "mcpb/rete.mcpb"; do
  "$repo/skills/rete-publish/scripts/upload_bucket.sh" \
    "$repo/clients/mcpb/rete.mcpb" "$key"
done

echo "publish.sh: done — https://data.graphplaza.com/mcpb/rete.mcpb"
