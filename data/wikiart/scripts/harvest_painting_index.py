#!/usr/bin/env python3
"""Phase 3 -- the painting inventory (ids), from both id surfaces.

  raw/paintings_app.jsonl    PRIMARY, COMPLETE. One request per artist slug to
                             /en/App/Painting/PaintingsByArtist?artistUrl=<slug>,
                             which returns that artist's entire oeuvre in one
                             shot: numeric `contentId`, title, year, image URL,
                             pixel dimensions. Slug-driven, so it is immune to
                             the UpdatedArtists wall (see harvest_artists.py)
                             and should total the 221,558 the sitemap declares.
  raw/painting_index.jsonl   ENRICHMENT. /en/api/2/PaintingsByArtist?id=<mongo>
                             for every artist whose Mongo id is known, yielding
                             the painting **Mongo ids** phase 4 needs.

Both are resumable per artist: an artist already recorded as done is skipped.
"""

import json
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import (DEFAULT_WORKERS, JsonlSink, PagedWall, QuotaExceeded, get,   # noqa: E402
                 paged, progress, raw_dir)


def jsonl(path):
    if not os.path.exists(path):
        return []
    out = []
    for line in open(path, encoding="utf-8"):
        line = line.strip()
        if line:
            try:
                out.append(json.loads(line))
            except Exception:
                pass
    return out


def main():
    raw = raw_dir()
    apath = os.path.join(raw, "artists_alphabet.json")
    if not os.path.exists(apath):
        sys.exit("run harvest_artists.py first (raw/artists_alphabet.json missing)")
    alpha = json.load(open(apath, encoding="utf-8"))

    # ---- PRIMARY: slug-driven, complete -------------------------------------
    asink = JsonlSink(os.path.join(raw, "paintings_app.jsonl"), key="contentId")
    adone = JsonlSink(os.path.join(raw, ".artists_app_done.jsonl"), key="id")
    atodo = [a for a in alpha if a.get("url") and a["url"] not in adone.seen]
    print(f"  App layer: {len(alpha)-len(atodo):,} artists done, {len(atodo):,} to go")
    t0, n = time.time(), 0

    def walk_app(a):
        try:
            recs = get("/en/App/Painting/PaintingsByArtist",
                       {"artistUrl": a["url"], "json": 2})
        except Exception:
            return                      # left un-done; next run retries it
        for p in recs or []:
            p["_artistUrl"] = a["url"]
            asink.write(p)
        adone.write({"id": a["url"]})

    if atodo:
        with ThreadPoolExecutor(max_workers=DEFAULT_WORKERS) as ex:
            for _ in ex.map(walk_app, atodo):
                n += 1
                if n % 25 == 0:
                    progress(n, len(atodo), "artists", t0)
        sys.stderr.write("\n")
    asink.close(); adone.close()
    print(f"  paintings_app.jsonl    {asink.n:,} paintings (numeric contentIds)")

    # ---- ENRICHMENT: Mongo ids where the artist id is known -----------------
    artists = jsonl(os.path.join(raw, "artists.jsonl"))
    artists += jsonl(os.path.join(raw, "artists_recovered.jsonl"))
    seen_id, uniq = set(), []
    for a in artists:
        if a.get("id") and a["id"] not in seen_id:
            seen_id.add(a["id"])
            uniq.append(a)

    sink = JsonlSink(os.path.join(raw, "painting_index.jsonl"), key="id")
    done = JsonlSink(os.path.join(raw, ".artists_indexed.jsonl"), key="id")
    todo = [a for a in uniq if a["id"] not in done.seen]
    print(f"  v2 layer: {len(uniq):,} artists with a Mongo id, {len(todo):,} to walk")
    t1, m, walled, quota_spent = time.time(), 0, [], False

    def walk(a):
        nonlocal quota_spent
        if quota_spent:
            return
        try:
            for p in paged("/en/api/2/PaintingsByArtist", {"id": a["id"]}):
                p["_artistId"] = a["id"]
                sink.write(p)
        except QuotaExceeded:
            quota_spent = True
            return
        except PagedWall as w:
            if isinstance(w.cause, QuotaExceeded):
                quota_spent = True
                return
            # Deterministic server-side wall part-way through this artist. The
            # records before it are already written; retrying would only refetch
            # them, so mark the artist done rather than looping forever.
            walled.append(a["id"])
        except Exception:
            return                      # transient -- leave un-done, retry later
        done.write({"id": a["id"]})

    if todo:
        with ThreadPoolExecutor(max_workers=DEFAULT_WORKERS) as ex:
            for _ in ex.map(walk, todo):
                m += 1
                if m % 25 == 0:
                    progress(m, len(todo), "artists", t1)
        sys.stderr.write("\n")
    sink.close(); done.close()
    print(f"  painting_index.jsonl   {sink.n:,} paintings (Mongo ids)")
    if quota_spent:
        print("  !! keyless /en/api/2/ quota spent -- v2 enrichment is partial.")
        print("     The App layer above is complete and unaffected; re-run later.")
    if walled:
        print(f"  {len(walled):,} artists walled mid-chain (partial oeuvre captured)")
    if asink.n:
        print(f"  Mongo coverage of the inventory: {100.0*sink.n/asink.n:.1f}%")


if __name__ == "__main__":
    main()
