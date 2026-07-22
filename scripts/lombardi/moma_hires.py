#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""Add the largest MoMA-signed image URL to each work in lombardi_moma.json.

MoMA's open-data ``ImageURL`` is a *signed* 1024px transform, and the signature
is an HMAC of the exact resize job — so you cannot fabricate a bigger one (the
server answers 400). But each work's public collection PAGE embeds signed URLs at
several sizes, up to 2000px. This fetches each page once and records the largest
signed URL for the work's file id as ``FullImageURL`` (and its longest edge as
``FullImageSize``). The 1024px ``ImageURL`` stays as the fast thumbnail.

Signatures are HMACs of the job, with no timestamp, so they do not expire.
Idempotent: re-running only refreshes.
"""
import base64
import json
import os
import re
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
JSON = os.path.join(ROOT, "data", "lombardi", "moma", "lombardi_moma.json")
UA = ("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
      "(KHTML, like Gecko) Chrome/126.0 Safari/537.36")
MEDIA = re.compile(r"https://www\.moma\.org/media/([A-Za-z0-9_-]+)\.jpg\?sha=[a-f0-9]+")


def file_id(image_url):
    """The MoMA file id encoded in a /media/<b64>.jpg URL."""
    m = MEDIA.search(image_url or "")
    if not m:
        return None
    b = m.group(1)
    try:
        job = json.loads(base64.urlsafe_b64decode(b + "=" * (-len(b) % 4)))
        return job[0][1]                      # [["f","530594"], ...]
    except Exception:
        return None


def largest_for(html, fid):
    """The signed URL on the page whose job is file `fid`, at the biggest resize."""
    best, best_px = None, -1
    for m in MEDIA.finditer(html):
        try:
            job = json.loads(base64.urlsafe_b64decode(m.group(1) + "=" * (-len(m.group(1)) % 4)))
        except Exception:
            continue
        if not (job and job[0][1] == fid):
            continue
        px = 0
        for step in job[1:]:
            r = re.search(r"-resize (\d+)x(\d+)", " ".join(map(str, step)))
            if r:
                px = max(int(r.group(1)), int(r.group(2)))
        if px > best_px:
            best, best_px = m.group(0), px
    return best, best_px


def fetch(url):
    # MoMA 403s a bare urllib even with a browser UA; curl gets through.
    out = subprocess.run(
        ["curl", "-sL", "--max-time", "45", "-A", UA,
         "-H", "Accept: text/html", "-H", "Accept-Language: en-US,en;q=0.9", url],
        capture_output=True, timeout=60)
    if out.returncode != 0 or not out.stdout:
        raise RuntimeError("curl failed (%d)" % out.returncode)
    return out.stdout.decode("utf-8", "replace")


def main():
    works = json.load(open(JSON, encoding="utf-8"))
    n = 0
    for w in works:
        fid = file_id(w.get("ImageURL"))
        if not fid or not w.get("URL"):
            continue
        try:
            html = fetch(w["URL"])
        except Exception as e:
            print("  ! %s: %s" % (w["AccessionNumber"], e), file=sys.stderr)
            continue
        url, px = largest_for(html, fid)
        if url and px > 1024:
            w["FullImageURL"], w["FullImageSize"] = url, px
            n += 1
            print("  %-14s %4dpx  %s" % (w["AccessionNumber"], px, w["Title"][:44]))
        time.sleep(1.1)
    json.dump(works, open(JSON, "w", encoding="utf-8"), indent=1, ensure_ascii=False)
    print("added a hi-res URL to %d of %d works -> %s" % (n, len(works), JSON))


if __name__ == "__main__":
    main()
