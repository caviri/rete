#!/usr/bin/env python3
"""Phase 4b -- COMPLETE per-painting detail, keyed by numeric contentId.

/en/App/Painting/ImageJson/<contentId> is the site's own painting endpoint. It
covers every painting in the inventory (no Mongo id needed, so no dependency on
the walled artist chain) and carries fields the v2 API does NOT expose:

    dictionaries[]   numeric vocabulary ids -- the MULTI-VALUED style/genre/
                     media linkage, where the flat `style`/`genre` fields hold
                     only the primary value
    auction, yearOfTrade, lastPrice        -- art-market provenance
    technique, material                    -- finer than v2's media[]
    period, serie, galleryName, location

Conversely v2's /Painting has the long `description` prose and galleries[]/
tags[] as arrays. The two are complementary; both are harvested and joined on
(artistUrl, url) when the graph is built.

One request per painting (~221.5k). Parallel, resumable -- re-run until it
reports 0 remaining.
"""

import json
import os
import sys
import time
import urllib.error
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import (DEFAULT_WORKERS, JsonlSink, NotJson, get, progress,   # noqa: E402
                 raw_dir)

# how many whole runs an id may fail before it is treated as permanently absent
GIVE_UP_AFTER = 3


def main():
    raw = raw_dir()
    src = os.path.join(raw, "paintings_app.jsonl")
    if not os.path.exists(src):
        sys.exit("run harvest_painting_index.py first (raw/paintings_app.jsonl missing)")

    ids, seen = [], set()
    for line in open(src, encoding="utf-8"):
        try:
            cid = json.loads(line)["contentId"]
        except Exception:
            continue
        if cid not in seen:
            seen.add(cid)
            ids.append(cid)
    print(f"  {len(ids):,} distinct paintings in the inventory")

    sink = JsonlSink(os.path.join(raw, "paintings_imagejson.jsonl"), key="contentId")
    misses = os.path.join(raw, "imagejson_misses.txt")
    known = set()
    if os.path.exists(misses):
        known = {l.strip() for l in open(misses, encoding="utf-8") if l.strip()}

    # Some ids are listed in an artist's oeuvre but have no detail document:
    # ImageJson 302s them to an error page that itself answers 500 (verified for
    # contentId 197943, "The Death of Marat"). get() cannot tell that apart from
    # a transient 500, so it retries and finally raises RuntimeError -- which
    # would make such an id retry forever, run after run. A persistent failure
    # counter retires them after GIVE_UP_AFTER whole runs.
    fpath = os.path.join(raw, "imagejson_failures.json")
    fails = {}
    if os.path.exists(fpath):
        try:
            fails = json.load(open(fpath, encoding="utf-8"))
        except Exception:
            fails = {}
    retired = {k for k, v in fails.items() if v >= GIVE_UP_AFTER}

    todo = [i for i in ids
            if i not in sink.seen and str(i) not in known and str(i) not in retired]
    print(f"  {sink.n:,} already fetched, {len(known):,} known misses, "
          f"{len(retired):,} retired after {GIVE_UP_AFTER} failed runs, {len(todo):,} to go")
    if not todo:
        print("  nothing to do -- phase 4b complete")
        return

    t0, n, gone = time.time(), 0, []
    failed_now = []

    def fetch(cid):
        try:
            return get(f"/en/App/Painting/ImageJson/{cid}")
        except NotJson:
            gone.append(str(cid))       # 302 -> HTML error page; no such record
            return None
        except urllib.error.HTTPError as e:
            if e.code in (404, 500):
                gone.append(str(cid))
                return None
            raise
        except Exception:
            failed_now.append(str(cid))  # retries exhausted; counted, retried next run
            return None

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
                f.write("".join(g + "\n" for g in gone))
        # bump the counter for ids that failed, clear it for ids that succeeded
        for cid in failed_now:
            fails[cid] = fails.get(cid, 0) + 1
        for cid in list(fails):
            if int(cid) in sink.seen:
                del fails[cid]
        with open(fpath, "w", encoding="utf-8") as f:
            json.dump(fails, f, indent=1, sort_keys=True)

    newly_retired = [k for k, v in fails.items() if v >= GIVE_UP_AFTER and k not in retired]
    print(f"  paintings_imagejson.jsonl  {sink.n:,} records ({len(gone):,} new misses)")
    if newly_retired:
        print(f"  retired {len(newly_retired):,} id(s) after {GIVE_UP_AFTER} failed runs: "
              f"{', '.join(sorted(newly_retired)[:5])}")
    remaining = len(ids) - sink.n - len(known) - len(gone) - len(retired) - len(newly_retired)
    if remaining > 0:
        print(f"  {remaining:,} still missing -- re-run this script to retry them")
    else:
        print("  phase 4b complete")


if __name__ == "__main__":
    main()
