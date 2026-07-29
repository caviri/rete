#!/usr/bin/env python3
"""Phase 5 -- build the image asset manifest (does NOT download the images).

Every painting record carries an `image` URL on uploads{0-9}.wikiart.org, always
suffixed with a WikiArt size token:

    .../vincent-van-gogh/the-starry-night-1889.jpg!Large.jpg
                                                  ^^^^^^

Stripping the "!Large.jpg" tail gives the original upload; other tokens
(!Portrait.jpg, !PinterestSmall.jpg, ...) are server-side derivatives. The
manifest records the Large form -- the largest WikiArt serves for the fair-use
copyrighted works, and plenty for a graph reproduction.

Writes raw/assets/images.urls.txt (feed to skills/dataset-download/scripts/
fetch_urls.py if the bytes are actually wanted) plus a TSV keyed by painting id
so a downloaded file can be traced back to its record.
"""

import json
import os
import re
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from _wa import raw_dir     # noqa: E402

SIZE_TOKEN = re.compile(r"!([A-Za-z0-9]+)\.(jpg|jpeg|png|gif)$", re.I)


def main():
    raw = raw_dir()
    # Prefer the App layer: it is the complete one. paintings.jsonl (v2) only
    # covers whatever the metered API allowed and may not exist at all.
    for cand in ("paintings_imagejson.jsonl", "paintings_app.jsonl", "paintings.jsonl"):
        src = os.path.join(raw, cand)
        if os.path.exists(src) and os.path.getsize(src) > 0:
            break
    else:
        sys.exit("no painting records found -- run harvest_painting_index.py "
                 "and harvest_imagejson.py first")
    print(f"  source: {os.path.basename(src)}")

    out_dir = os.path.join(raw, "assets")
    os.makedirs(out_dir, exist_ok=True)

    tokens, hosts, n, missing = Counter(), Counter(), 0, 0
    urls_path = os.path.join(out_dir, "images.urls.txt")
    tsv_path = os.path.join(out_dir, "all_assets.tsv")

    with open(urls_path, "w", encoding="utf-8") as uf, \
         open(tsv_path, "w", encoding="utf-8") as tf:
        tf.write("painting_id\tcontent_id\tartist_url\tpainting_url\twidth\theight\timage_url\n")
        for line in open(src, encoding="utf-8"):
            try:
                p = json.loads(line)
            except Exception:
                continue
            n += 1
            img = p.get("image")
            if not img:
                missing += 1
                continue
            m = SIZE_TOKEN.search(img)
            tokens[m.group(1) if m else "(none)"] += 1
            host = img.split("/")[2] if "//" in img else "?"
            hosts[host] += 1
            uf.write(img + "\n")
            tf.write("\t".join(str(x) for x in (
                p.get("id", ""), p.get("contentId", ""),
                p.get("artistUrl") or p.get("_artistUrl", ""), p.get("url", ""),
                p.get("width", ""), p.get("height", ""), img)) + "\n")

    print(f"  {n:,} painting records, {n-missing:,} with an image ({missing:,} without)")
    print(f"  size tokens: {dict(tokens.most_common(6))}")
    print(f"  upload hosts: {len(hosts)} ({', '.join(sorted(hosts)[:5])}...)")
    print(f"  wrote {os.path.relpath(urls_path, raw)} and {os.path.relpath(tsv_path, raw)}")
    print("\n  Images are NOT downloaded by default -- the manifest is enough to")
    print("  reference them from the graph. To fetch the bytes (~tens of GB):")
    print("    docker run --rm -v \"$PWD:/w\" -w //w python:3.12-slim python \\")
    print("      skills/dataset-download/scripts/fetch_urls.py \\")
    print("      data/wikiart/raw/assets/images.urls.txt data/wikiart/raw/assets/images --workers 8")


if __name__ == "__main__":
    main()
