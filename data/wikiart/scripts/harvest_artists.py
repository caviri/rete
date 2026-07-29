#!/usr/bin/env python3
"""Phase 2 -- every artist, from both id surfaces.

  raw/artists_alphabet.json  /en/App/Artist/AlphabetJson. THE COMPLETE LIST --
                             all artists in one request, with the numeric
                             `contentId` and numeric dictionary ids. This is the
                             spine: every later phase is driven off these slugs.
  raw/artists.jsonl          /en/api/2/UpdatedArtists, paginated. The rich
                             record: Mongo id, biography, gender, activeYears,
                             periods, series, relatedArtists.
  raw/artists_recovered.jsonl  slug -> Mongo id pairs recovered for artists the
                             paginated chain could not reach (see below).

GOTCHA -- the UpdatedArtists chain hits a WALL. Somewhere past ~4,800 artists a
page 500s with "Year, Month, and Day parameters describe an un-representable
Date": one artist record carries a birth/death date .NET cannot serialise, and
it poisons that whole page of 60. The failure is deterministic (verified over
repeated attempts), and there is no way around it:

  * `fromDate` is accepted in epoch-seconds/ms form but is INERT -- every value
    returns the identical first page, so the chain cannot be restarted past the
    bad page;
  * `fromDate` in any ISO/US/.NET date form 500s with "Failed to parse date";
  * there is no page-number pagination (`page=N` is ignored on these endpoints);
  * /en/api/2/Artist?id= and /en/App/Artist/ArtistJson/<contentId> both 302 to
    an error page.

So the rich layer is best-effort and the run stops cleanly at the wall. For the
artists it could not reach, `PaintingSearch` is used to recover the Mongo id
from the artist's own name -- it works whenever the artist has indexed works,
which is most of them. Coverage is reported at the end; the slug layer is
unaffected and remains 100% complete.
"""

import json
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import (DEFAULT_WORKERS, QuotaExceeded, get, paged_list, progress,   # noqa: E402
                 raw_dir)


def main():
    raw = raw_dir()

    # -- 1. the complete slug/contentId layer ---------------------------------
    alpha = get("/en/App/Artist/AlphabetJson", {"v": "new", "inPublicDomain": "false"})
    with open(os.path.join(raw, "artists_alphabet.json"), "w", encoding="utf-8") as f:
        json.dump(alpha, f, ensure_ascii=False)
    print(f"  artists_alphabet.json  {len(alpha):,} artists (COMPLETE, numeric contentIds)")

    # -- 2. the rich Mongo layer, up to the wall ------------------------------
    # QUOTA-GATED: /en/api/2/ is metered for keyless use. If the quota is spent
    # this yields nothing -- which must NOT clobber a good earlier harvest, so
    # the file is only replaced once we have more records than are already on
    # disk. (Learned the hard way: a naive "w" open destroyed 5,100 records.)
    path = os.path.join(raw, "artists.jsonl")
    have = 0
    if os.path.exists(path):
        have = sum(1 for l in open(path, encoding="utf-8") if l.strip())

    try:
        recs, walled_token, cause = paged_list("/en/api/2/UpdatedArtists", on_error="stop")
    except QuotaExceeded as e:
        recs, walled_token, cause = [], None, e

    if len(recs) > have:
        tmp = path + ".part"
        with open(tmp, "w", encoding="utf-8") as f:
            for r in recs:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
        os.replace(tmp, path)
        print(f"  artists.jsonl          {len(recs):,} artists (rich: biography, gender, relatedArtists)")
    else:
        recs = [json.loads(l) for l in open(path, encoding="utf-8") if l.strip()] if have else []
        print(f"  artists.jsonl          kept {have:,} existing records "
              f"(this run reached {0 if not recs else len(recs):,})")
    if cause:
        print(f"  !! v2 chain stopped: {cause}")
        if isinstance(cause, QuotaExceeded):
            print("     -> keyless /en/api/2/ quota is spent; the App layer is unaffected.")
            print("     -> re-run this phase later to fill in the rich artist fields.")

    # -- 3. recover the artists the chain never reached -----------------------
    have = {r.get("url") for r in recs}
    missing = [a for a in alpha if a.get("url") and a["url"] not in have]
    print(f"  {len(missing):,} artists unreachable via pagination -- recovering ids via PaintingSearch")

    out, t0, n = [], time.time(), 0
    quota_spent = isinstance(cause, QuotaExceeded)

    def recover(a):
        """Find this artist's Mongo id in search hits for their own name.

        Best-effort only: PaintingSearch 500s outright on many long-tail terms,
        and those failures are deterministic -- so retries=1, no backoff. The
        Mongo id is an enrichment key, not a requirement: everything downstream
        can be reached through the artist's slug instead.
        """
        terms = []
        for t in (a.get("artistName"), a.get("lastNameFirst")):
            if t and t.strip() and t.strip() not in terms:
                terms.append(t.strip())
        nonlocal quota_spent
        for term in terms:
            if quota_spent:
                return None             # every further v2 call would 500 too
            try:
                hits = (get("/en/api/2/PaintingSearch",
                            {"term": term, "json": 2}, retries=1) or {}).get("data") or []
            except QuotaExceeded:
                quota_spent = True
                return None
            except Exception:
                continue                # 500 / no index entry -- try the next form
            for h in hits:
                if h.get("artistUrl") == a["url"] and h.get("artistId"):
                    return {"url": a["url"], "id": h["artistId"],
                            "artistName": a.get("artistName"),
                            "contentId": a.get("contentId"), "_via": "PaintingSearch"}
        return None

    if missing and not quota_spent:
        with ThreadPoolExecutor(max_workers=DEFAULT_WORKERS) as ex:
            for r in ex.map(recover, missing):
                if r:
                    out.append(r)
                n += 1
                if n % 25 == 0:
                    progress(n, len(missing), "recovering", t0)
        sys.stderr.write("\n")
    elif quota_spent:
        print("  skipped recovery -- v2 quota spent")

    rpath = os.path.join(raw, "artists_recovered.jsonl")
    prev = 0
    if os.path.exists(rpath):
        prev = sum(1 for l in open(rpath, encoding="utf-8") if l.strip())
    if len(out) >= prev:                # same no-clobber rule as above
        with open(rpath, "w", encoding="utf-8") as f:
            for r in out:
                f.write(json.dumps(r, ensure_ascii=False) + "\n")
    else:
        out = [json.loads(l) for l in open(rpath, encoding="utf-8") if l.strip()]

    total_mongo = len(recs) + len(out)
    print(f"  artists_recovered.jsonl {len(out):,} of {len(missing):,} recovered")
    print(f"  Mongo-id coverage: {total_mongo:,}/{len(alpha):,} artists "
          f"({100.0*total_mongo/max(len(alpha),1):.1f}%)  |  slug coverage: 100%")


if __name__ == "__main__":
    main()
