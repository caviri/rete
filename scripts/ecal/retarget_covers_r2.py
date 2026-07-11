#!/usr/bin/env python3
"""Point the ECAL graph's cover fields at the R2-hosted WebP copies.

Reads data/ecal/normalized/ecal.jsonl and, per record:
  - if data/ecal/covers_webp/<local_id>.webp exists → set thumbnail_url and the
    'cover' file entry to https://data.graphplaza.com/ecal/covers/<local_id>.webp
    (format image/webp);
  - if it had a cover but no webp was produced (the handful of 404s) → drop the
    thumbnail + cover file so nothing renders a broken bibliomaker link.
schema:url (the bibliomaker record page) is left untouched as provenance.

Writes data/ecal/normalized/ecal.r2.jsonl (original kept intact).
"""
from __future__ import annotations

import json
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
SRC = REPO / "data" / "ecal" / "normalized" / "ecal.jsonl"
OUT = REPO / "data" / "ecal" / "normalized" / "ecal.r2.jsonl"
WEBP = REPO / "data" / "ecal" / "covers_webp"
BASE_URL = "https://data.graphplaza.com/ecal/covers/"


def main():
    n = retargeted = dropped = untouched = 0
    with open(SRC, encoding="utf-8") as fin, open(OUT, "w", encoding="utf-8") as fout:
        for line in fin:
            r = json.loads(line)
            n += 1
            lid = r.get("local_id")
            had_cover = bool(r.get("thumbnail_url")) or any(
                f.get("label") == "cover" for f in r.get("files", []))
            webp = (WEBP / f"{lid}.webp") if lid is not None else None
            if webp is not None and webp.exists() and webp.stat().st_size > 0:
                url = f"{BASE_URL}{lid}.webp"
                r["thumbnail_url"] = url
                files = r.get("files", [])
                found = False
                for f in files:
                    if f.get("label") == "cover":
                        f["url"] = url
                        f["format"] = "image/webp"
                        found = True
                if not found:
                    files.append({"url": url, "label": "cover", "format": "image/webp"})
                    r["files"] = files
                retargeted += 1
            elif had_cover:
                # cover promised but no webp (a 404) — strip it so nothing 404s in-browser
                r.pop("thumbnail_url", None)
                r["files"] = [f for f in r.get("files", []) if f.get("label") != "cover"]
                dropped += 1
            else:
                untouched += 1
            fout.write(json.dumps(r, ensure_ascii=False) + "\n")
    print(f"records {n:,} | retargeted->R2 {retargeted:,} | dropped(no webp) {dropped:,} | "
          f"no-cover {untouched:,}\n-> {OUT}")


if __name__ == "__main__":
    raise SystemExit(main())
