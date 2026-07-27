#!/usr/bin/env python3
"""FULL USTC crawl for INTERNAL RESEARCH (local only — do NOT publish).

The public playground carries only the 727-edition fair-use slice
(data/ustc/editions/, the bph federation subset). THIS crawler collects the
whole catalogue for a PhD research project run in collaboration with USTC, and
writes it to data/ustc/crawl/ — which is gitignored and must NOT be uploaded to
the public bucket. USTC data is © University of St Andrews & contributors.

Method: USTC has no sitemap and no bulk API; each /editions/{sn} page is an
Inertia.js app embedding the full record as JSON in <div data-page="...">. We
iterate the sn number space (1..MAX; ~60-80% dense, gaps return a permanent 500)
and store each hit. GENTLE by design (a collaborator's server): a small worker
pool + per-request delay, ~a handful of req/s. Fully resumable via a cursor.

  python scripts/crawl_ustc.py                       # start / resume
  python scripts/crawl_ustc.py --workers 2 --delay 0.6   # even gentler
  python scripts/crawl_ustc.py --max 1100000
Monitor:  tail -f data/ustc/crawl.log   |   status in data/ustc/progress.json
"""
import argparse
import concurrent.futures as cf
import html
import json
import os
import re
import sys
import time
import urllib.request
import urllib.error

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "ustc")
CRAWL = os.path.join(OUT, "crawl")
PROG = os.path.join(OUT, "progress.json")
LOG = os.path.join(OUT, "crawl.log")
BASE = "https://www.ustc.ac.uk/editions/"
UA = ("rete-dataset-harvester/1.0 (internal PhD research in collaboration with USTC; "
      "local, throttled; +https://github.com/caviri/rete)")
_DP = re.compile(r'data-page="([^"]+)"')
os.makedirs(CRAWL, exist_ok=True)


def shard_path(sn):
    d = os.path.join(CRAWL, f"{int(sn) % 100:02d}")
    return d, os.path.join(d, f"{sn}.json")


def log(msg):
    line = time.strftime("%H:%M:%S ") + msg
    print(line, flush=True)
    with open(LOG, "a", encoding="utf-8") as fh:
        fh.write(line + "\n")


def load_cursor(default_start):
    if os.path.exists(PROG):
        try:
            return json.load(open(PROG, encoding="utf-8")).get("cursor", default_start)
        except Exception:
            pass
    return default_start


def save_progress(cursor, stats):
    tmp = PROG + ".tmp"
    json.dump({"cursor": cursor, **stats}, open(tmp, "w", encoding="utf-8"))
    os.replace(tmp, PROG)


def fetch_one(sn, delay, tries=3):
    d, dst = shard_path(sn)
    if os.path.exists(dst) and os.path.getsize(dst) > 40:
        return "skip"
    for i in range(tries):
        try:
            req = urllib.request.Request(BASE + str(sn), headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=45) as r:
                t = r.read().decode("utf-8", "replace")
            m = _DP.search(t)
            if not m:
                return "nodata"
            props = json.loads(html.unescape(m.group(1))).get("props", {})
            rec = {k: props.get(k) for k in
                   ("edition", "copies", "digitisations", "references", "contemp", "notes")}
            os.makedirs(d, exist_ok=True)
            json.dump(rec, open(dst, "w", encoding="utf-8"), ensure_ascii=False)
            time.sleep(delay)
            return "ok"
        except urllib.error.HTTPError as e:
            if e.code in (404, 410, 500):
                return "gap"          # permanent: no such edition
            time.sleep(2.0 * (i + 1))  # 429/503: back off hard
        except Exception:
            time.sleep(1.5 * (i + 1))
    return "fail"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", type=int, default=1)
    ap.add_argument("--max", type=int, default=1_100_000)
    ap.add_argument("--workers", type=int, default=3)
    ap.add_argument("--delay", type=float, default=0.4, help="per-request sleep (politeness)")
    ap.add_argument("--batch", type=int, default=600, help="checkpoint every N ids")
    a = ap.parse_args()

    start = max(a.start, load_cursor(a.start))
    log(f"CRAWL start sn={start} max={a.max} workers={a.workers} delay={a.delay}s "
        f"(~{a.workers/max(a.delay,0.01):.0f} req/s cap)")
    from collections import Counter
    stats = Counter()
    t0 = time.time()
    sn = start
    with cf.ThreadPoolExecutor(max_workers=a.workers) as ex:
        while sn <= a.max:
            batch = list(range(sn, min(sn + a.batch, a.max + 1)))
            for res in ex.map(lambda x: fetch_one(x, a.delay), batch):
                stats[res] += 1
            sn = batch[-1] + 1
            done = sum(stats.values())
            rate = done / max(time.time() - t0, 1)
            eta_h = (a.max - sn) / max(rate, 0.1) / 3600
            save_progress(sn, dict(stats))
            log(f"sn={sn} ok={stats['ok']} gap={stats['gap']} skip={stats['skip']} "
                f"fail={stats['fail']} | {rate:.1f} req/s | ETA {eta_h:.1f}h")
    log(f"DONE at sn={sn}: {dict(stats)}")


if __name__ == "__main__":
    main()
