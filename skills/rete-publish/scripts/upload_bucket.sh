#!/usr/bin/env bash
# Upload a built .rete file or companion directory to the public R2 bucket.
#
#   upload_bucket.sh web/foo.rete                 # -> foo/foo.rete
#   upload_bucket.sh web/foo.rete foo/foo.rete    # explicit object key
#   upload_bucket.sh data/foo/ foo                # recursive prefix upload
#
# Env:
#   ACCESS_KEY_ID / SECRET_ACCESS_KEY / S3_API_ENDPOINT (or repository .env)
#   RETE_BUCKET   bucket name (default: rete)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SRC="${1:?usage: upload_bucket.sh <file|dir> [object-key|prefix]}"

args=("$ROOT/skills/rete-publish/scripts/upload_r2.py" "$SRC")
if [ -n "${2:-}" ]; then
  args+=("$2")
fi

uv run --with boto3 python "${args[@]}"
