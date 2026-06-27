#!/usr/bin/env python3
"""Harvest the full Biblissima+ Wikibase RDF, entity by entity, into gzipped
N-Triples shards (resumable). Biblissima publishes no bulk dump and its query
service is auth-gated, but per-entity RDF export (Special:EntityData) is open.

~867k entities, ~340 triples each -> ~290M triples (~4 GB .rete). Run:
    uv run --with requests python scripts/harvest_biblissima.py
Resumable: re-run to continue (per-shard .done markers). Then build with:
    zcat data/biblissima/shards/*.nt.gz | rete build - -o data/biblissima/biblissima.rete --pyramid-algo types --card
"""
import gzip
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor

import requests

BASE = "https://data.biblissima.fr"
API = BASE + "/w/api.php"
# Paths relative to the repo root (this file is scripts/harvest_biblissima.py),
# so the harvest works regardless of the launching cwd.
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "data", "biblissima")
IDS_FILE = os.path.join(OUT, "ids.txt")
SHARDS = os.path.join(OUT, "shards")
NS = 120              # Wikibase item namespace
SHARD_SIZE = 5000
WORKERS = 8
MAX_SHARDS = int(os.environ.get("BIBLISSIMA_MAX_SHARDS") or 0) or None  # for a test run
UA = "rete-research/1.0 (carlosvivarrios@gmail.com) Biblissima RDF -> knowledge graph"

os.makedirs(SHARDS, exist_ok=True)
sess = requests.Session()
sess.headers["User-Agent"] = UA


def enumerate_ids():
    if os.path.exists(IDS_FILE):
        return [x.strip() for x in open(IDS_FILE) if x.strip()]
    ids, cont = [], None
    while True:
        p = {"action": "query", "list": "allpages", "apnamespace": NS,
             "aplimit": 500, "format": "json"}
        if cont:
            p["apcontinue"] = cont
        r = sess.get(API, params=p, timeout=60).json()
        for it in r["query"]["allpages"]:
            ids.append(it["title"].split(":")[-1])
        if "continue" in r:
            cont = r["continue"]["apcontinue"]
        else:
            break
        if len(ids) % 50000 < 500:
            print("enumerated", len(ids)); sys.stdout.flush()
    with open(IDS_FILE, "w") as f:
        f.write("\n".join(ids))
    return ids


def fetch(q):
    url = f"{BASE}/w/Special:EntityData/{q}.nt"
    for attempt in range(4):
        try:
            r = sess.get(url, timeout=60)
            if r.status_code == 200:
                return r.content
            if r.status_code in (429, 503):
                time.sleep(5 * (attempt + 1)); continue
            return b""
        except Exception:
            time.sleep(3 * (attempt + 1))
    return b""


def main():
    ids = enumerate_ids()
    print("total entities:", len(ids)); sys.stdout.flush()
    shards = [ids[i:i + SHARD_SIZE] for i in range(0, len(ids), SHARD_SIZE)]
    t0, done_e = time.time(), 0
    for k, sh in enumerate(shards):
        if MAX_SHARDS and k >= MAX_SHARDS:
            print("stopping at MAX_SHARDS"); break
        marker = f"{SHARDS}/shard_{k:05d}.done"
        if os.path.exists(marker):
            done_e += len(sh); continue
        ok = 0
        with gzip.open(f"{SHARDS}/shard_{k:05d}.nt.gz", "wb") as out:
            with ThreadPoolExecutor(max_workers=WORKERS) as ex:
                for data in ex.map(fetch, sh):
                    if data:
                        out.write(data); ok += 1
        open(marker, "w").write(str(ok))
        done_e += len(sh)
        el = time.time() - t0
        rate = done_e / el if el > 0 else 0
        eta = (len(ids) - done_e) / rate / 3600 if rate > 0 else 0
        print(f"shard {k+1}/{len(shards)}  {done_e}/{len(ids)} entities  "
              f"{ok}/{len(sh)} ok  {rate:.1f} e/s  ETA {eta:.1f}h"); sys.stdout.flush()
    print("HARVEST DONE (this pass)")


main()
