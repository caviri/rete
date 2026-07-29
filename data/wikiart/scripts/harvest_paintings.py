#!/usr/bin/env python3
"""Phase 4 -- the payload: full detail for every painting.

/en/api/2/Painting?id=<mongoId> is the richest record WikiArt serves:

    genres[] styles[] media[] galleries[] tags[]  -- the vocabulary edges
    sizeX sizeY diameter                          -- physical dimensions (cm)
    location period serie                         -- provenance
    description                                   -- curatorial prose, with
                                                     [url href=...] links to
                                                     other WikiArt entities
    image width height                            -- the reproduction

One request per painting (~221.5k). Parallel, resumable, and the only phase that
takes real wall-clock time. Re-run it until it reports 0 remaining.

Note: this endpoint accepts ONLY the 24-hex Mongo id. Passing a numeric
contentId returns 404 "Painting not found".
"""

import json
import os
import sys
import time
import urllib.error
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import (DEFAULT_WORKERS, JsonlSink, QuotaExceeded, get, progress,   # noqa: E402
                 raw_dir)


def main():
    raw = raw_dir()
    idx = os.path.join(raw, "painting_index.jsonl")
    if not os.path.exists(idx):
        sys.exit("run harvest_painting_index.py first (raw/painting_index.jsonl missing)")

    ids = []
    seen = set()
    for line in open(idx, encoding="utf-8"):
        try:
            pid = json.loads(line)["id"]
        except Exception:
            continue
        if pid not in seen:
            seen.add(pid)
            ids.append(pid)
    print(f"  {len(ids):,} distinct paintings in the index")

    sink = JsonlSink(os.path.join(raw, "paintings.jsonl"), key="id")
    misses = os.path.join(raw, "painting_misses.txt")
    known_miss = set()
    if os.path.exists(misses):
        known_miss = {l.strip() for l in open(misses, encoding="utf-8") if l.strip()}

    todo = [i for i in ids if i not in sink.seen and i not in known_miss]
    print(f"  {sink.n:,} already fetched, {len(known_miss):,} known 404s, {len(todo):,} to go")
    if not todo:
        print("  nothing to do -- phase 4 complete")
        return

    t0, n = time.time(), 0
    gone, quota_spent = [], False

    def fetch(pid):
        nonlocal quota_spent
        if quota_spent:
            return None
        try:
            return get("/en/api/2/Painting", {"id": pid})
        except QuotaExceeded:
            quota_spent = True          # every further v2 call would 500 too
            return None
        except urllib.error.HTTPError as e:
            if e.code == 404:
                gone.append(pid)        # withdrawn / merged record
                return None
            raise
        except Exception:
            return None                 # exhausted retries; next run picks it up

    try:
        with ThreadPoolExecutor(max_workers=DEFAULT_WORKERS) as ex:
            for rec in ex.map(fetch, todo):
                if rec:
                    sink.write(rec)
                n += 1
                if n % 100 == 0:
                    progress(n, len(todo), "paintings", t0)
    finally:
        sys.stderr.write("\n")
        sink.close()
        if gone:
            with open(misses, "a", encoding="utf-8") as f:
                f.write("".join(p + "\n" for p in gone))

    print(f"  paintings.jsonl        {sink.n:,} full records"
          f"  ({len(gone):,} new 404s this run)")
    if quota_spent:
        print("  !! keyless /en/api/2/ quota spent -- stopped early.")
        print("     This phase is OPTIONAL enrichment; phase 4b (App layer) is")
        print("     unmetered and complete. Re-run this later to top it up.")
    remaining = len(ids) - sink.n - len(known_miss) - len(gone)
    if remaining > 0:
        print(f"  {remaining:,} still missing -- re-run this script to retry them")


if __name__ == "__main__":
    main()
