"""Export the playground catalog to hf-space/catalog.json.

The playground's ``web/playground-src/catalog.js`` is the source of truth for
published datasets. The HF Space (REST + MCP query planes) consumes a plain
JSON projection of it: key, label, description, the direct range-readable R2
URL, meta (triples/size/license/source), tags, and the example queries.

Needs node on PATH (the catalog is evaluated in a sandboxed VM — same
approach as check_dataset_catalog.py). Run after catalog changes:

    python scripts/export_space_catalog.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from check_dataset_catalog import load_catalog  # noqa: E402

OUT = ROOT / "hf-space" / "catalog.json"


def main() -> int:
    catalog = load_catalog()
    base = catalog["remoteBase"].rstrip("/")
    meta = catalog.get("datasetMeta") or {}
    extra = catalog.get("datasetExtra") or {}
    examples = catalog.get("examples") or {}  # object keyed by dataset

    datasets = []
    for d in catalog.get("datasets", []):
        key = d["key"]
        m, x = meta.get(key) or {}, extra.get(key) or {}
        entry = {
            "key": key,
            "label": d.get("label"),
            "description": d.get("description"),
            "kind": d.get("kind") or "embedded",
            "triples": m.get("triples"),
            "size": m.get("size"),
            "license": m.get("license"),
            "source": m.get("source"),
            "tags": x.get("tags"),
            "examples": [
                {"title": e.get("label"), "tip": e.get("tip"),
                 "reason": e.get("reason"), "sparql": e.get("q")}
                for e in (examples.get(key) or [])
                if e.get("q")
            ],
        }
        if d.get("shards"):
            entry["shards"] = d["shards"]
        else:
            # Every published dataset is mirrored on R2 at the derived key —
            # the same contract check_dataset_catalog.py probes.
            entry["url"] = d.get("url") or f"{base}/{key}/{key}.rete"
        datasets.append({k: v for k, v in entry.items() if v not in (None, [], "")})

    OUT.write_text(json.dumps({"remoteBase": base, "datasets": datasets},
                              indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(datasets)} datasets")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
