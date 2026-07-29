#!/usr/bin/env python3
"""Phase 1 -- the controlled vocabularies (WikiArt "dictionaries").

/en/api/2/DictionariesByGroup?group=N returns one vocabulary per group id.
Groups 0, 4, 5, 6 and 17+ are empty; 1..3 and 7..16 are the real ones. The group
number is the only thing identifying what a vocabulary *is* -- WikiArt does not
name them -- so the mapping below was established by inspecting the members.

These become the SKOS-ish backbone of the graph: every artist and painting
record references these entries by Mongo id.
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import QuotaExceeded, paged, raw_dir     # noqa: E402

# group id -> (slug, what the members are)
GROUPS = {
    1:  ("periods",          "historical periods, incl. Egyptian/Islamic dynastic periods"),
    2:  ("styles",           "art styles (Impressionism, Cubism, ...)"),
    3:  ("genres",           "genres (portrait, landscape, still life, ...)"),
    7:  ("art-movements",    "art movements, schools and groups"),
    8:  ("galleries",        "museums / galleries holding the works"),
    9:  ("auctions",         "auction sales"),
    10: ("nationalities",    "artist nationalities"),
    11: ("fields",           "artist fields (painting, sculpture, architecture, ...)"),
    12: ("media",            "materials and techniques"),
    13: ("art-institutions", "academies and art schools the artists attended"),
    14: ("series",           "work series / groupings"),
    15: ("countries",        "countries"),
    16: ("misc",             "uncategorised"),
}


def main():
    out_dir = os.path.join(raw_dir(), "dictionaries")
    os.makedirs(out_dir, exist_ok=True)
    summary = {}

    for group, (slug, desc) in sorted(GROUPS.items()):
        path = os.path.join(out_dir, f"group-{group:02d}-{slug}.json")
        have = []
        if os.path.exists(path):
            try:
                have = json.load(open(path, encoding="utf-8"))
            except Exception:
                have = []
        try:
            recs = list(paged("/en/api/2/DictionariesByGroup", {"group": group}))
        except QuotaExceeded:
            # Metered endpoint. Keep whatever a previous run captured rather than
            # overwriting good vocabularies with nothing.
            print(f"  group {group:2d}  {slug:<17} quota spent -- kept {len(have):,} on disk")
            summary[slug] = {"group": group, "count": len(have), "description": desc}
            continue
        if len(recs) < len(have):
            print(f"  group {group:2d}  {slug:<17} kept {len(have):,} (this run got {len(recs):,})")
            summary[slug] = {"group": group, "count": len(have), "description": desc}
            continue
        with open(path, "w", encoding="utf-8") as f:
            json.dump(recs, f, ensure_ascii=False, indent=1)
        summary[slug] = {"group": group, "count": len(recs), "description": desc}
        print(f"  group {group:2d}  {slug:<17} {len(recs):>6,} entries  -> {os.path.basename(path)}")

    with open(os.path.join(out_dir, "_summary.json"), "w", encoding="utf-8") as f:
        json.dump(summary, f, ensure_ascii=False, indent=1)
    total = sum(v["count"] for v in summary.values())
    print(f"  {total:,} vocabulary entries across {len(summary)} dictionaries")


if __name__ == "__main__":
    main()
