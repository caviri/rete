#!/usr/bin/env bash
# Publish the legacy-tagged Pyodide wheel to the public bucket.
#
#   scripts/publish_pyodide_wheel.sh                 # fetch from the release run
#   scripts/publish_pyodide_wheel.sh path/to/*.whl   # use a local wheel
#
# Why this exists: `%pip install rete-graph` resolves from PyPI and needs no
# pin, but Pyodide 0.29's installer predates PEP 783 — which renamed the wheel
# platform tag `pyodide_*` to `pyemscripten_*` — so it cannot see the wheel PyPI
# requires. docs/python.md therefore points those users at a retagged copy at
#   https://data.graphplaza.com/wheels/rete_graph-<version>-cp39-abi3-pyodide_2025_0_wasm32.whl
# The publish workflow builds that copy (artifact `wheel-pyodide-legacy`) but
# cannot upload it: the repository has no R2 credentials in Actions. This script
# is that last step. `sync_versions.py --check` keeps the documented URL's
# version honest, but only this makes the URL resolve.
#
# Env: ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT (or repository .env),
#      as used by skills/rete-publish/scripts/upload_bucket.sh.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

BASE_URL="https://data.graphplaza.com/wheels"
EXPECTED_TAG="pyodide_2025_0_wasm32"

# The wheel must match the version the client is releasing, or the URL baked
# into docs/python.md points at something that does not exist.
version=$(sed -n 's/^version = "\(.*\)"/\1/p' clients/python/pyproject.toml | head -1)
test -n "$version" || { echo "could not read clients/python/pyproject.toml version" >&2; exit 1; }

wheel="${1:-}"
if [ -z "$wheel" ]; then
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT HUP INT TERM
  echo "== downloading the wheel-pyodide-legacy artifact from py-v$version =="
  run=$(gh run list --workflow=python-client-publish.yml \
        --branch "py-v$version" --limit 1 --json databaseId --jq '.[0].databaseId')
  test -n "$run" || { echo "no publish run found for tag py-v$version" >&2; exit 1; }
  gh run download "$run" --name wheel-pyodide-legacy --dir "$tmp"
  wheel=$(ls "$tmp"/*.whl | head -1)
fi

test -f "$wheel" || { echo "not a file: $wheel" >&2; exit 1; }
name=$(basename "$wheel")

case "$name" in
  *"$EXPECTED_TAG".whl) ;;
  *)
    echo "refusing to upload $name: expected the $EXPECTED_TAG platform tag." >&2
    echo "A pyemscripten_* wheel belongs on PyPI, not in this bucket path." >&2
    exit 1
    ;;
esac

case "$name" in
  "rete_graph-$version-"*) ;;
  *)
    echo "refusing to upload $name: it is not version $version," >&2
    echo "which is what docs/python.md and pyproject.toml say is being released." >&2
    exit 1
    ;;
esac

echo "== uploading $name =="
skills/rete-publish/scripts/upload_bucket.sh "$wheel" "wheels/$name"

echo "== verifying the documented URL resolves =="
url="$BASE_URL/$name"
status=$(curl -sS -o /dev/null -w '%{http_code}' -L "$url")
test "$status" = 200 || { echo "$url returned HTTP $status" >&2; exit 1; }

# docs/python.md tells users to %pip install this exact URL, so a wrong
# content-type or a zero-length object is a broken instruction, not a nit.
bytes=$(curl -sS -o /dev/null -w '%{size_download}' -L "$url")
test "$bytes" -gt 100000 || { echo "$url served only $bytes bytes" >&2; exit 1; }

echo "OK  $url  ($bytes bytes)"
echo
echo "docs/python.md references this URL; confirm it matches:"
grep -n "wheels/rete_graph-" docs/python.md || true
