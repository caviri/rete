#!/usr/bin/env python3
"""Re-normalize Renouvaud from the ALREADY-DOWNLOADED raw SRU leaves (no re-harvest).

Each raw/renouvaud/sru/mms_*.xml.gz holds that leaf's SRU pages concatenated with a
`<!-- PAGE -->` separator. We split, re-parse the MARCXML, and re-emit renouvaud.jsonl
using the current marc.normalize (which now extracts AVA/AVE holdings → where each
item physically lives). Cheap: local-only, a few minutes for 3.55M records.
"""
from __future__ import annotations

import glob
import gzip
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import marc  # noqa: E402

REPO = Path(__file__).resolve().parents[2]


def main():
    base = REPO / "data" / "bcul"
    raw_dir = base / "raw" / "renouvaud" / "sru"
    out_path = base / "normalized" / "renouvaud.jsonl"
    now = datetime.now(timezone.utc).isoformat(timespec="seconds")

    files = sorted(glob.glob(str(raw_dir / "mms_*.xml.gz")))
    print(f"re-parsing {len(files)} raw leaves -> {out_path}", flush=True)
    n = 0
    with open(out_path, "w", encoding="utf-8") as out:
        for i, fn in enumerate(files, 1):
            blob = gzip.open(fn).read()
            for page in blob.split(b"<!-- PAGE -->"):
                page = page.strip()
                if not page:
                    continue
                try:
                    for m in marc.iter_marc_records(page):
                        rec = marc.normalize(m, "renouvaud")
                        rec["harvested_at"] = now
                        out.write(json.dumps(rec, ensure_ascii=False) + "\n")
                        n += 1
                except Exception as e:  # a corrupt page shouldn't abort the whole reparse
                    print(f"  !! parse error in {Path(fn).name}: {type(e).__name__} {e}", flush=True)
            if i % 50 == 0:
                print(f"  {i}/{len(files)} leaves, {n:,} records", flush=True)
    print(f"done: {n:,} records -> {out_path}", flush=True)


if __name__ == "__main__":
    main()
