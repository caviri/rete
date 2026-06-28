#!/usr/bin/env python3
"""Parse the harvested CCBAE listing pages (data/ccbae/pages/page_*.html) into one
records JSON: per cartographic record - title, image-group path id, and the
bibliographic block text (author / date / place / signatura where present).

Run: uv run --no-project --with beautifulsoup4 --with lxml python scripts/parse_ccbae.py
"""
import glob
import json
import os
import re

from bs4 import BeautifulSoup

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PAGES = os.path.join(ROOT, "data", "ccbae", "pages")
OUT = os.path.join(ROOT, "data", "ccbae", "ccbae_records.json")

records = []
for pg in sorted(glob.glob(os.path.join(PAGES, "page_*.html"))):
    soup = BeautifulSoup(open(pg, encoding="utf-8").read(), "lxml")
    for r in soup.select(".registro_datos"):
        rec = {}
        a = r.find("a", href=re.compile(r"grupo\.do"))
        if a:
            m = re.search(r"path=(\d+)", a.get("href", ""))
            rec["path"] = m.group(1) if m else None
            rec["title"] = (a.get("data-analytics-recordtitle") or "").strip()
        bib = r.select_one(".registro_bib")
        if bib:
            rec["bib"] = re.sub(r"\s+", " ", bib.get_text(" ", strip=True)).strip()
        if rec.get("path") or rec.get("title"):
            records.append(rec)

with open(OUT, "w", encoding="utf-8") as f:
    json.dump(records, f, ensure_ascii=False, indent=1)
print("records:", len(records))
for r in records[:4]:
    print(json.dumps(r, ensure_ascii=False)[:300])
