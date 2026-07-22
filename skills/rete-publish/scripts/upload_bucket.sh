#!/usr/bin/env bash
# Upload a built .rete file or companion directory to the public R2 bucket.
#
#   upload_bucket.sh web/foo.rete                 # -> foo/foo.rete
#   upload_bucket.sh web/foo.rete foo/foo.rete    # explicit object key
#   upload_bucket.sh data/foo/ foo                # recursive prefix upload
#
# Runs in a container like everything else in this repo — the host keeps no
# Python toolchain. Set RETE_UPLOAD_LOCAL=1 to run the uploader with the host
# interpreter instead (needs boto3; `uv run` wants --no-project on Windows,
# where the project .venv symlink is not writable).
#
# Env:
#   ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT (or repository .env)
#   RETE_BUCKET   bucket name (default: rete)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SRC="${1:?usage: upload_bucket.sh <file|dir> [object-key|prefix]}"
KEY="${2:-}"

if [ "${RETE_UPLOAD_LOCAL:-0}" = "1" ]; then
  args=("$ROOT/skills/rete-publish/scripts/upload_r2.py" "$SRC")
  [ -n "$KEY" ] && args+=("$KEY")
  exec uv run --no-project --with boto3 python "${args[@]}"
fi

# The source path is passed through relative to the repo root inside the
# container, so an absolute host path is rewritten to its /work equivalent.
case "$SRC" in
  "$ROOT"/*) SRC="${SRC#"$ROOT"/}" ;;
esac

MSYS_NO_PATHCONV=1 exec docker run --rm \
  -e ACCESS_KEY_ID -e SECRET_ACCESS_KEY -e S3_API_ENDPOINT -e RETE_BUCKET \
  -v "$ROOT:/work" -w /work python:3.12-slim \
  sh -c 'pip install --quiet --disable-pip-version-check --root-user-action=ignore boto3 &&
         exec python skills/rete-publish/scripts/upload_r2.py "$@"' \
  _ "$SRC" ${KEY:+"$KEY"}
