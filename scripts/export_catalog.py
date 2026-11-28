"""Export the playground catalog as the catalog.json rete-server reads.

The playground's ``web/playground-src/catalog.js`` is the source of truth for
published datasets. rete-server (clients/server) consumes a plain JSON
projection of it: key, label, description, the direct range-readable R2 URL,
meta (triples/size/license/source), tags, example queries and SHACL shapes.

Two copies exist: the one bundled in the package
(``clients/server/rete_server/catalog.json``, the fallback) and the published
one at ``https://data.graphplaza.com/catalog.json``, which
``.github/workflows/catalog-publish.yml`` uploads from this script's output.

Needs node on PATH (the catalog is evaluated in a sandboxed VM — same
approach as check_dataset_catalog.py). Run after catalog changes:

    python scripts/export_catalog.py            # rewrite the bundled copy
    python scripts/export_catalog.py --check    # fail if it is stale
    python scripts/export_catalog.py --out x.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from check_dataset_catalog import load_catalog  # noqa: E402

OUT = ROOT / "clients/server" / "rete_server" / "catalog.json"


def render() -> str:
    catalog = load_catalog()
    base = catalog["remoteBase"].rstrip("/")
    meta = catalog.get("datasetMeta") or {}
    extra = catalog.get("datasetExtra") or {}
    examples = catalog.get("examples") or {}  # object keyed by dataset
    shacl = catalog.get("shacl") or {}  # object keyed by dataset

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
            "shacl_shapes": [
                {"title": s.get("label"), "tip": s.get("tip"), "shape": s.get("shape")}
                for s in (shacl.get(key) or [])
                if s.get("shape")
            ],
        }
        if d.get("shards"):
            entry["shards"] = d["shards"]
        else:
            # Every published dataset is mirrored on R2 at the derived key —
            # the same contract check_dataset_catalog.py probes.
            entry["url"] = d.get("url") or f"{base}/{key}/{key}.rete"
        datasets.append({k: v for k, v in entry.items() if v not in (None, [], "")})

    return json.dumps({"remoteBase": base, "datasets": datasets}, indent=1, ensure_ascii=False) + "\n"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--out", type=Path, default=OUT, help=f"where to write (default {OUT.relative_to(ROOT)})")
    ap.add_argument("--check", action="store_true", help="exit 1 when --out differs from the catalog")
    args = ap.parse_args()
    text = render()
    count = len(json.loads(text)["datasets"])
    if args.check:
        current = args.out.read_text(encoding="utf-8") if args.out.is_file() else ""
        if current != text:
            print(f"{args.out}: stale; run python scripts/export_catalog.py", file=sys.stderr)
            return 1
        print(f"{args.out}: current ({count} datasets)")
        return 0
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {args.out}: {count} datasets")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
