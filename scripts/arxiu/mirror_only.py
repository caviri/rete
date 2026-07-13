#!/usr/bin/env python3
"""Mirror-only pass over an already-harvested records_*.jsonl: WebP-mirror every
non-reserved .jpg to R2 (skipping ones already there) and keep each record's `webp`
field current, rewriting the jsonl durably every 2000 records so a stopped/resumed
crawl never loses associations. No metadata re-harvest. Reuses harvest_and_mirror's
mirror()/list_existing(). Meant to run detached for a long slow crawl.

Usage (in a container with boto3+pillow and R2 .env):
    python scripts/arxiu/mirror_only.py --jsonl data/arxiu/records_1.jsonl --workers 10
"""
import json, sys, argparse
from pathlib import Path
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, str(Path(__file__).resolve().parent))
from harvest_and_mirror import mirror, list_existing  # noqa

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--jsonl", default="data/arxiu/records_1.jsonl")
    ap.add_argument("--workers", type=int, default=10)
    a = ap.parse_args()
    p = Path(a.jsonl)
    recs = [json.loads(l) for l in p.open(encoding="utf-8")]
    print(f"{len(recs):,} records; listing R2…", flush=True)
    existing = list_existing()
    print(f"  {len(existing):,} WebP already on R2; mirroring the rest with {a.workers} workers…", flush=True)

    def flush():
        tmp = p.with_suffix(".jsonl.tmp")
        with tmp.open("w", encoding="utf-8") as f:
            for rr in recs:
                f.write(json.dumps(rr, ensure_ascii=False) + "\n")
        tmp.replace(p)

    done = got = 0
    with ThreadPoolExecutor(max_workers=a.workers) as ex:
        for r, wp in zip(recs, ex.map(lambda r: mirror(r, existing), recs)):
            r["webp"] = wp
            done += 1
            if wp:
                got += 1
            if done % 2000 == 0:
                flush()
                print(f"  {done:,}/{len(recs):,}  webp={got:,}", flush=True)
    flush()
    print(f"DONE: {got:,} webp associations -> {p}", flush=True)

if __name__ == "__main__":
    main()
