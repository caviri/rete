#!/usr/bin/env python3
"""Fetch the IIIF Presentation v3 manifests (and optionally page images) for
every digitized object in the Peirce papers dataset (data/peirce/peirce.nt,
a:digitalContent -> nrs.harvard.edu URNs).

Manifest URL = the NRS URN + ":MANIFEST:3" (discovered from
viewer.lib.harvard.edu's manifestId; redirects to mps.lib.harvard.edu/iiif/3/).
The IIIF hosts publish no robots.txt: these are public IIIF APIs.

  python scripts/peirce_fetch_iiif.py manifests   # all manifests -> data/peirce/iiif/
  python scripts/peirce_fetch_iiif.py census      # canvas/page counts from local manifests
  python scripts/peirce_fetch_iiif.py images [--width 1200] [--limit N]
                                                  # every page image -> data/peirce/images/

Resumable: existing files are skipped. Polite: one request at a time + delay.
"""
import argparse
import json
import os
import re
import sys
import time
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DATA = os.path.join(ROOT, "data", "peirce")
NT = os.path.join(DATA, "peirce.nt")
MDIR = os.path.join(DATA, "iiif")
IDIR = os.path.join(DATA, "images")
UA = "rete-dataset-builder/1.0 (research; contact: carlosvivarrios@gmail.com)"
DELAY = 0.4


def urns():
    txt = open(NT, encoding="utf-8").read()
    return sorted(set(re.findall(r"ontology#digitalContent> <([^>]+)>", txt)))


def urn_base(urn):
    """Drop query suffixes some URNs carry (?buttons=y)."""
    return urn.split("?")[0]


def tail(urn):
    """URN-3:FHCL.HOUGH:105144787 -> FHCL.HOUGH_105144787 (fs-safe, case-normal)."""
    t = urn_base(urn).rsplit("/", 1)[-1]
    t = re.sub(r"^urn-3:", "", t, flags=re.I)
    return t.replace(":", "_")


def get(url, dest, binary=False):
    req = urllib.request.Request(url, headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=120) as r:
        data = r.read()
    if binary:
        open(dest, "wb").write(data)
    else:
        open(dest, "w", encoding="utf-8", newline="\n").write(
            data.decode("utf-8"))
    return len(data)


# --- 429-aware pacing (the image API rate-limits sustained pulls) ---
import threading
_cooldown_until = [0.0]
_cd_lock = threading.Lock()


def get_paced(url, dest, retries=12):
    """get() that honors 429 Retry-After with a GLOBAL cool-down all workers
    respect, exponential fallback, and a few retries before giving up."""
    for attempt in range(retries):
        wait = _cooldown_until[0] - time.time()
        if wait > 0:
            time.sleep(wait)
        try:
            return get(url, dest, binary=True)
        except urllib.error.HTTPError as e:
            if e.code == 429:
                ra = e.headers.get("Retry-After")
                pause = float(ra) if ra and ra.isdigit() else min(60 * 2 ** attempt, 900)
                with _cd_lock:
                    _cooldown_until[0] = max(_cooldown_until[0], time.time() + pause)
                continue
            raise
    raise RuntimeError(f"429 persisted after {retries} backoffs: {url}")


def fetch_manifests():
    os.makedirs(MDIR, exist_ok=True)
    todo = urns()
    done = err = 0
    for i, urn in enumerate(todo):
        dest = os.path.join(MDIR, tail(urn) + ".json")
        if os.path.exists(dest) and os.path.getsize(dest) > 0:
            continue
        try:
            n = get(urn_base(urn) + ":MANIFEST:3", dest)
            done += 1
            if done % 25 == 0:
                print(f"  {i + 1}/{len(todo)} fetched={done} last={tail(urn)} ({n}B)")
        except Exception as e:
            err += 1
            print(f"  ERR {urn}: {e}")
        time.sleep(DELAY)
    print(f"manifests: {len(todo)} urns, fetched {done} new, {err} errors -> {MDIR}")


def census(pr=True):
    rows = []
    for f in sorted(os.listdir(MDIR)):
        if not f.endswith(".json"):
            continue
        try:
            m = json.load(open(os.path.join(MDIR, f), encoding="utf-8"))
        except Exception:
            print("  BAD JSON:", f); continue
        rows.append((f[:-5], len(m.get("items", []))))
    total = sum(n for _, n in rows)
    if pr:
        print(f"manifests={len(rows)} total canvases (page images)={total}")
        rows.sort(key=lambda r: -r[1])
        for t, n in rows[:10]:
            print(f"  {n:6}  {t}")
    return rows


def fetch_images(width, limit, workers=4, delay=None):
    from concurrent.futures import ThreadPoolExecutor
    a_delay = [DELAY if delay is None else delay]
    os.makedirs(IDIR, exist_ok=True)
    size = "full" if not width else f"{width},"
    # build the full work list first (skipping already-downloaded pages)
    jobs, skip, bad = [], 0, 0
    for f in sorted(os.listdir(MDIR)):
        if not f.endswith(".json"):
            continue
        m = json.load(open(os.path.join(MDIR, f), encoding="utf-8"))
        odir = os.path.join(IDIR, f[:-5])
        os.makedirs(odir, exist_ok=True)
        for i, c in enumerate(m.get("items", []), 1):
            dest = os.path.join(odir, f"{i:04}.jpg")
            if os.path.exists(dest) and os.path.getsize(dest) > 0:
                skip += 1; continue
            try:
                body = c["items"][0]["items"][0]["body"]
                svc = body.get("service", [{}])[0]
                base = svc.get("id") or svc.get("@id") or body["id"].split("/full/")[0]
            except Exception:
                bad += 1; continue
            jobs.append((f"{base}/full/{size}/0/default.jpg", dest))
    if limit:
        jobs = jobs[:limit]
    print(f"images to fetch: {len(jobs)} (skipping {skip} existing, {bad} bad canvases)")
    done = [0]; err = [0]; nbytes = [0]
    t0 = time.time()

    def one(job):
        url, dest = job
        try:
            nbytes[0] += get_paced(url, dest)
            done[0] += 1
        except Exception as e:
            err[0] += 1
            print(f"  ERR {os.path.basename(os.path.dirname(dest))}/{os.path.basename(dest)}: {e}")
        n = done[0]
        if n and n % 500 == 0:
            dt = time.time() - t0
            eta = (len(jobs) - n) / (n / dt) / 60
            print(f"  {n}/{len(jobs)} ({nbytes[0] / 1e9:.2f} GB, {n / dt:.1f}/s, ~{eta:.0f} min left)")
        time.sleep(a_delay[0])

    with ThreadPoolExecutor(max_workers=workers) as ex:
        list(ex.map(one, jobs))
    print(f"images: fetched {done[0]} new ({nbytes[0] / 1e9:.2f} GB), skipped {skip} existing, "
          f"{err[0]} errors in {(time.time() - t0) / 60:.0f} min -> {IDIR}")


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("cmd", choices=["manifests", "census", "images"])
    ap.add_argument("--width", type=int, default=1200,
                    help="IIIF width for images (0 = full resolution)")
    ap.add_argument("--limit", type=int, default=0, help="max new images this run")
    ap.add_argument("--workers", type=int, default=4)
    ap.add_argument("--delay", type=float, default=None,
                    help="per-worker sleep between requests (default 0.4)")
    a = ap.parse_args()
    if a.cmd == "manifests":
        fetch_manifests()
    elif a.cmd == "census":
        census()
    else:
        fetch_images(a.width, a.limit, a.workers, a.delay)
