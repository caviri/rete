#!/usr/bin/env python3
"""Phase 1b -- multilingual labels for the vocabularies.

The /en/artists-by-<facet>?json=2 pages return a `Categories` array in which
each entry's title is given in ~24 languages:

    {"_id": {"_oid": "5beea37b..."},
     "Content": {"Title": {"Title": {"en": "Ancient Egyptian art",
                                     "de": "Altägyptische Kunst",
                                     "ja": "エジプト美術", ...}}}}

That is the natural source of `skos:prefLabel`s in many languages for the graph,
and it keys on the same Mongo ids as the v2 dictionaries, so the two join
directly.

These live on the site layer, not /en/api/2/, so they are UNMETERED -- verified
serving normally while the v2 quota was exhausted.
"""

import json
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import get, raw_dir     # noqa: E402

FACETS = [
    "art-movement", "nation", "century", "field",
    "painting-school", "art-institution",
]


def main():
    out_dir = os.path.join(raw_dir(), "categories")
    os.makedirs(out_dir, exist_ok=True)
    langs, total = Counter(), 0

    for facet in FACETS:
        try:
            doc = get(f"/en/artists-by-{facet}", {"json": 2})
        except Exception as e:
            print(f"  {facet:<18} FAILED: {e}")
            continue

        # `Categories` holds the GROUP headings, multilingual. `Dictionaries`
        # holds the actual vocabulary entries -- slug, English title, `Group`
        # (matching the v2 group numbers) and `Count`, the number of artists
        # carrying that term. Keep both; only together are they useful.
        cats = doc.get("Categories") or []
        entries = doc.get("Dictionaries") or []
        n_lang = set()
        for c in cats:
            t = (((c.get("Content") or {}).get("Title") or {}).get("Title")) or {}
            n_lang.update(t.keys())
        langs.update(n_lang)

        path = os.path.join(out_dir, f"{facet}.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump({"facet": facet, "group": doc.get("Group"),
                       "categories": cats, "entries": entries},
                      f, ensure_ascii=False, indent=1)
        total += len(entries)
        counted = sum(e.get("Count") or 0 for e in entries)
        print(f"  {facet:<18} {len(entries):>5,} entries  {len(cats):>3} groups"
              f"  {len(n_lang):>2} langs  ({counted:,} artist links)")

    print(f"  {total:,} vocabulary entries; group labels in {len(langs)} languages: "
          f"{', '.join(sorted(langs))}")


if __name__ == "__main__":
    main()
