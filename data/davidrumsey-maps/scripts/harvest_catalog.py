#!/usr/bin/env python3
"""Harvest the COMPLETE David Rumsey catalog via the LUNA JSON API in large
batches — the primary metadata lane.

    python harvest_catalog.py [--out DIR] [--bs N] [--sleep S] [--limit N]

One call returns `bs` full records (id, displayName, all ~38 metadata field
labels, urlSize0-4 image tiers, IIIF manifest URL):

    /luna/servlet/as/search?q=&lc=RUMSEY~8~1&bs=500&os=<offset>&sort=Pub_List_No_InitialSort

At bs=500 the whole 150,017-item collection is ~301 calls (~45-60s each server
side, ~1.6GB raw -> ~300MB gzipped on disk). SEQUENTIAL by design: the server
caps per-IP connections and each call is heavy — parallelism here just breeds
connection-refused churn (measured). The IIIF page lane (enumerate_iiif.py)
remains as a robots-clean fallback; per-item IIIF manifests remain the archival
lane (harvest_manifests.py).

Outputs (in --out, default data/davidrumsey-maps/raw):
    catalog/os_<offset>.json.gz   one gzipped API response per batch (as-is)
    items_index.tsv               <id>\t<displayName> for every item
    manifest_urls.txt             one IIIF manifest URL per item

Resume-safe: existing non-empty batch files are skipped; re-run to continue.
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import gzip
import json
import sys
import time
import urllib.request
from pathlib import Path

URL = ("https://www.davidrumsey.com/luna/servlet/as/search"
       "?q=&lc=RUMSEY~8~1&bs={bs}&os={os}&sort=Pub_List_No_InitialSort")
UA = {"User-Agent": "rete-dataset-harvester/1.0 (research; contact: carlosvivarrios@gmail.com)"}


def parse_args(argv: list[str]):
    out, bs, sleep, limit = Path("data/davidrumsey-maps/raw"), 500, 2.0, 0
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--out":
            out = Path(argv[i + 1]); i += 2; continue
        if a == "--bs":
            bs = int(argv[i + 1]); i += 2; continue
        if a == "--sleep":
            sleep = float(argv[i + 1]); i += 2; continue
        if a == "--limit":
            limit = int(argv[i + 1]); i += 2; continue
        sys.exit(f"unknown arg: {a}")
    return out, bs, sleep, limit


def get_batch(bs: int, os_: int, retries: int = 6) -> dict:
    url = URL.format(bs=bs, os=os_)
    last = ""
    for attempt in range(retries):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=300) as r:
                body = r.read()
            if not body.lstrip().startswith(b"{"):
                raise ValueError("non-JSON response (captcha wall?) — long backoff")
            d = json.loads(body)
            if "results" not in d:
                raise ValueError(f"no results key (keys: {sorted(d)[:6]})")
            return d
        except Exception as e:  # noqa: BLE001 — network best-effort
            last = f"{type(e).__name__}: {e}"
            wait = min(15 * 2 ** attempt, 300)
            print(f"  retry os={os_} in {wait}s: {last}", flush=True)
            time.sleep(wait)
    raise RuntimeError(f"os={os_} failed after {retries} attempts: {last}")


def main() -> None:
    out, bs, sleep, limit = parse_args(sys.argv[1:])
    cat = out / "catalog"
    cat.mkdir(parents=True, exist_ok=True)

    first = get_batch(bs, 0)
    total = int(first.get("totalResults", "0"))
    offsets = list(range(0, total, bs))
    if limit:
        offsets = offsets[:limit]
    print(f"totalResults={total} bs={bs} batches={len(offsets)}", flush=True)

    t0 = time.time()
    fetched = 0
    for n, os_ in enumerate(offsets, 1):
        dest = cat / f"os_{os_:06d}.json.gz"
        if dest.exists() and dest.stat().st_size > 0:
            continue
        d = first if os_ == 0 else get_batch(bs, os_)
        nres = len(d.get("results", []))
        expect = min(bs, total - os_)
        if nres < expect:
            print(f"  WARNING os={os_}: {nres} results, expected {expect}", flush=True)
        tmp = dest.with_suffix(".gz.part")
        with gzip.open(tmp, "wt", encoding="utf-8", compresslevel=6) as f:
            json.dump(d, f, ensure_ascii=False)
        tmp.replace(dest)
        fetched += 1
        el = time.time() - t0
        eta = el / fetched * (len(offsets) - n)
        print(f"  batch {n}/{len(offsets)} os={os_} results={nres} "
              f"elapsed={el/60:.1f}m eta={eta/60:.0f}m", flush=True)
        time.sleep(sleep)

    # consolidate index + manifest URL list from all batch files
    ids: list[tuple[str, str]] = []
    seen: set[str] = set()
    for f in sorted(cat.glob("os_*.json.gz")):
        d = json.loads(gzip.open(f, "rt", encoding="utf-8").read())
        for r in d.get("results", []):
            iid = r.get("id", "")
            if iid and iid not in seen:
                seen.add(iid)
                ids.append((iid, " ".join(str(r.get("displayName", "")).split())))
    with (out / "items_index.tsv").open("w", encoding="utf-8") as f:
        for iid, label in ids:
            f.write(f"{iid}\t{label}\n")
    with (out / "manifest_urls.txt").open("w", encoding="utf-8") as f:
        for iid, _ in ids:
            f.write(f"https://www.davidrumsey.com/luna/servlet/iiif/m/{iid}/manifest\n")
    print(f"DONE items={len(ids)} (totalResults {total}) -> items_index.tsv, manifest_urls.txt", flush=True)
    if len(ids) != total:
        print("WARNING: unique ids != totalResults — re-run to fill gaps "
              "(or collection changed mid-harvest)", flush=True)


if __name__ == "__main__":
    main()
