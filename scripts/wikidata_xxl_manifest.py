#!/usr/bin/env python3
"""Emit the shard list for the Wikidata-XXL federated dataset.

Scans data/wikidata-xxl/shard_*.rete (the --no-pyramid shards built by
build_wikidata_xxl.sh) and prints the bucket URLs as a JS `shards: [...]` array to
paste into web/playground-src/catalog.js — shards[0] is the dataset's primary `url`,
the rest become intrinsic federation partners (every query fans across them). Also
writes data/wikidata-xxl/manifest.json (upload alongside the shards if you want a
machine-readable list in the bucket).

  python scripts/wikidata_xxl_manifest.py
Env: RETE_BUCKET_URL (default the project Space), RETE_TOKEN (read token).
"""
import glob
import json
import os
import sys

BUCKET = os.environ.get("RETE_BUCKET_URL", "https://katospiegel-rete.hf.space/data/playground")
TOKEN = os.environ.get("RETE_TOKEN", "sfdbgf1094by21hd128ru39802")

shards = sorted(glob.glob("data/wikidata-xxl/shard_*.rete"))
if not shards:
    sys.exit("no data/wikidata-xxl/shard_*.rete found — build some first")
urls = [f"{BUCKET}/wikidata-xxl/{os.path.basename(s)}?token={TOKEN}" for s in shards]
total = sum(os.path.getsize(s) for s in shards)
sys.stderr.write(f"{len(urls)} shards, total {total / 1e9:.1f} GB\n")

print('"shards": [')
print(",\n".join('  "%s"' % u for u in urls))
print("]")

with open("data/wikidata-xxl/manifest.json", "w", encoding="utf-8") as f:
    json.dump({"shards": urls, "count": len(urls), "bytes": total}, f, indent=2)
sys.stderr.write("wrote data/wikidata-xxl/manifest.json\n")
