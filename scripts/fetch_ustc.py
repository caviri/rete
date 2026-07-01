#!/usr/bin/env python3
"""Harvest a BOUNDED subset of the Universal Short Title Catalogue (ustc.ac.uk):
only the editions cited by the Embassy/BPH collection (their ustc_id links), so
the two datasets FEDERATE. USTC is a copyrighted scholarly database with no open
licence and no bulk API - this is a small fair-use research extract (727 of ~1M
records), NOT a wholesale copy. Attribute USTC / University of St Andrews.

Each /editions/{sn} page is an Inertia.js app: the full record is embedded as
JSON in the root <div data-page="...">. We read props.edition + copies +
digitisations + references. Resumable, polite (4 workers).

  python scripts/fetch_ustc.py            # reads bph ustc_ids, fetches missing
"""
import concurrent.futures as cf
import glob
import html
import json
import os
import re
import time
import urllib.request
import urllib.error

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BPH = os.path.join(ROOT, "data", "bph", "books")
OUT = os.path.join(ROOT, "data", "ustc")
EDS = os.path.join(OUT, "editions")
BASE = "https://www.ustc.ac.uk/editions/"
UA = ("rete-dataset-harvester/1.0 (bounded bph<->USTC federation subset, ~727 records; "
      "+https://github.com/caviri/rete)")
WORKERS = 4
os.makedirs(EDS, exist_ok=True)

_DP = re.compile(r'data-page="([^"]+)"')


def bph_ustc_ids():
    ids = set()
    for f in glob.glob(os.path.join(BPH, "*.json")):
        try:
            o = json.load(open(f, encoding="utf-8"))
        except Exception:
            continue
        u = o.get("ustc_id")
        if u:
            ids.add(str(u).strip())
    return sorted(ids)


def fetch(sn, tries=4):
    dst = os.path.join(EDS, sn + ".json")
    if os.path.exists(dst) and os.path.getsize(dst) > 50:
        return "skip"
    for i in range(tries):
        try:
            req = urllib.request.Request(BASE + sn, headers={"User-Agent": UA})
            with urllib.request.urlopen(req, timeout=60) as r:
                t = r.read().decode("utf-8", "replace")
            m = _DP.search(t)
            if not m:
                return "nodata"
            page = json.loads(html.unescape(m.group(1)))
            props = page.get("props", {})
            rec = {k: props.get(k) for k in
                   ("edition", "copies", "digitisations", "references", "contemp", "notes")}
            json.dump(rec, open(dst, "w", encoding="utf-8"), ensure_ascii=False)
            time.sleep(0.15)          # be gentle to a scholarly server
            return "ok"
        except urllib.error.HTTPError as e:
            if e.code in (404, 410):
                return "missing"
            time.sleep(1.5 * (i + 1))
        except Exception:
            time.sleep(1.0 * (i + 1))
    return "fail"


def main():
    ids = bph_ustc_ids()
    print(f"bph cites {len(ids)} USTC editions; harvesting into {EDS}")
    from collections import Counter
    c = Counter()
    t0 = time.time()
    with cf.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        futs = {ex.submit(fetch, sn): sn for sn in ids}
        for n, f in enumerate(cf.as_completed(futs), 1):
            c[f.result()] += 1
            if n % 100 == 0 or n == len(ids):
                print(f"  {n}/{len(ids)} {dict(c)} {time.time()-t0:.0f}s", flush=True)
    print("done:", dict(c))


if __name__ == "__main__":
    main()
