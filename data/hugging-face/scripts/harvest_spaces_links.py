#!/usr/bin/env python3
"""Sweep the spaces listing with expand[]=models/datasets — the Hub-computed
"space uses model/dataset" links (much richer than the spaces' cardData).

~1,430 requests for all ~1.43M spaces (limit=1000 pages + Link cursor).
Writes spaces with at least one link to raw/api/space_links/*.jsonl.
Resumable: the next-page URL is checkpointed in _cursor.txt after every page;
a finished sweep leaves "DONE" there (delete the file to re-sweep).

Usage (in Docker, token via -e HF_TOKEN):
  python data/hugging-face/scripts/harvest_spaces_links.py
"""
import json
import os
import re
import sys
import time
from datetime import datetime, timezone

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from harvest_profiles import BASE, API, RateLimiter, fetch  # noqa: E402

START = (f"{API}/spaces?expand%5B%5D=models&expand%5B%5D=datasets"
         f"&expand%5B%5D=sdk&limit=1000")


def main():
    token = os.environ.get("HF_TOKEN", "").strip()
    rps = 7.5 if token else 1.4
    out_dir = os.path.join(BASE, "raw", "api", "space_links")
    os.makedirs(out_dir, exist_ok=True)
    cursor_path = os.path.join(out_dir, "_cursor.txt")

    url = START
    if os.path.exists(cursor_path):
        saved = open(cursor_path, encoding="utf-8").read().strip()
        if saved == "DONE":
            print("space_links sweep already complete (_cursor.txt=DONE)", flush=True)
            return
        if saved:
            url = saved

    limiter = RateLimiter(rps)
    run_id = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S")
    shard = open(os.path.join(out_dir, f"spaces-expand-{run_id}.jsonl"), "a",
                 encoding="utf-8")
    pages = kept = 0
    t0 = time.monotonic()
    now = datetime.now(timezone.utc).isoformat()

    while url:
        status, body, headers = fetch(url, token, limiter)
        if status != 200:
            print(f"page failed (HTTP {status}) — cursor saved, re-run to resume",
                  flush=True)
            break
        for sp in json.loads(body):
            if sp.get("models") or sp.get("datasets"):
                shard.write(json.dumps(
                    {"id": sp.get("id"), "sdk": sp.get("sdk"),
                     "models": sp.get("models") or [],
                     "datasets": sp.get("datasets") or [],
                     "fetched_at": now}, ensure_ascii=False) + "\n")
                kept += 1
        pages += 1
        m = re.search(r'<([^>]+)>;\s*rel="next"', headers.get("Link", ""))
        url = m.group(1) if m else None
        with open(cursor_path, "w", encoding="utf-8") as f:
            f.write(url if url else "DONE")
        if pages % 50 == 0:
            shard.flush()
            print(f"  {pages} pages (~{pages*1000:,} spaces), {kept:,} with links, "
                  f"{pages/(time.monotonic()-t0):.1f} pages/s", flush=True)

    shard.close()
    print(f"sweep {'complete' if not url else 'INTERRUPTED'}: {pages} pages, "
          f"{kept:,} spaces with links in {(time.monotonic()-t0)/60:.1f} min", flush=True)


if __name__ == "__main__":
    sys.exit(main())
