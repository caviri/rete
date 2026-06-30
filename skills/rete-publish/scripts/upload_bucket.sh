#!/usr/bin/env bash
# Upload a built .rete (or a companions directory) to the HF bucket the playground
# serves. Writes use your `hf` CLI auth — NOT the read token in catalog.js.
#
#   upload_bucket.sh web/foo.rete                 # → playground/foo.rete
#   upload_bucket.sh web/foo.rete bar.rete        # → playground/bar.rete (rename)
#   upload_bucket.sh data/foo/foo-tables/ foo-tables   # a directory (sync)
#
# Env:
#   RETE_BUCKET   bucket repo (default: katospiegel/knowledge-graphs)
#   RETE_PREFIX   key prefix  (default: playground)
#   HF            path to the hf CLI (default: hf on PATH)
set -euo pipefail
export MSYS_NO_PATHCONV=1   # keep hf://... intact on Windows Git-Bash

SRC="${1:?usage: upload_bucket.sh <file|dir> [dest-name]}"
BUCKET="${RETE_BUCKET:-katospiegel/knowledge-graphs}"
PREFIX="${RETE_PREFIX:-playground}"
HF="${HF:-hf}"

if ! command -v "$HF" >/dev/null 2>&1; then
  echo "hf CLI not found (set HF=/path/to/hf). Install: pip install huggingface_hub[cli]" >&2
  exit 127
fi

name="${2:-$(basename "$SRC")}"
dest="hf://buckets/${BUCKET}/${PREFIX}/${name}"

if [ -d "$SRC" ]; then
  echo "sync  $SRC  →  $dest/"
  "$HF" buckets sync "$SRC" "$dest"
else
  echo "cp    $SRC  →  $dest"
  "$HF" buckets cp "$SRC" "$dest"
fi
echo "done. URL: https://<space>/data/${PREFIX}/${name}?token=<read-token>"
