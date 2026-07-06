#!/bin/bash
# Upload every <key>_emb.f32 + <key>_emb_index.json to R2 (bucket `rete`) under
# <key>-rag/, served token-free at data.graphplaza.com. Run from repo root with
# .env sourced. Resumable-ish (re-uploads are idempotent overwrites).
cd "$(dirname "$0")/../.." || exit 1
set -a; . ./.env 2>/dev/null; set +a
for f in data/rag/*_emb.f32; do
  key=$(basename "$f" _emb.f32)
  PYTHONSAFEPATH=1 python dev/r2_s3.py put rete "data/rag/${key}_emb.f32"        "${key}-rag/${key}_emb.f32"        >/dev/null 2>&1 && \
  PYTHONSAFEPATH=1 python dev/r2_s3.py put rete "data/rag/${key}_emb_index.json" "${key}-rag/${key}_emb_index.json" >/dev/null 2>&1 && \
  echo "uploaded $key ($(du -h "$f" | cut -f1))" || echo "FAILED $key"
done
