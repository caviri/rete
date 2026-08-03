#!/usr/bin/env python3
"""Harvest one IIIF Presentation manifest per item — the complete metadata
record (27 LUNA fields, image dimensions, IIIF image service, JP2 master link).

    python harvest_manifests.py [--index PATH] [--out DIR] [--workers N] [--limit N]

Reads ids from items_index.tsv (produced by enumerate_iiif.py) and writes each
manifest gzipped to <out>/<shard>/<id>.json.gz where <shard> is the last two
characters of the id's numeric tail (256-ish dirs, ~600 files each — NTFS-kind).

~150k manifests x ~14KB raw -> ~2.1GB raw, ~500-700MB gzipped on disk.

Resume-safe (skips existing non-empty files), atomic (.part then rename),
retries with backoff, failures logged to <out>/download_failures.txt.
Stdlib only — runs in a plain python:3.12-slim container.
"""
from __future__ import annotations

import gzip
import sys
import time
import urllib.request
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

MANIFEST_URL = "https://www.davidrumsey.com/luna/servlet/iiif/m/{id}/manifest"
UA = {"User-Agent": "rete-dataset-harvester/1.0 (research; contact: carlosvivarrios@gmail.com)"}
POLITE_SLEEP = 0.05


def parse_args(argv: list[str]):
    index = Path("data/davidrumsey-maps/raw/items_index.tsv")
    out = Path("data/davidrumsey-maps/raw/manifests")
    workers, limit = 6, 0
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--index":
            index = Path(argv[i + 1]); i += 2; continue
        if a == "--out":
            out = Path(argv[i + 1]); i += 2; continue
        if a == "--workers":
            workers = int(argv[i + 1]); i += 2; continue
        if a == "--limit":
            limit = int(argv[i + 1]); i += 2; continue
        sys.exit(f"unknown arg: {a}")
    return index, out, workers, limit


def shard_of(iid: str) -> str:
    tail = iid.rsplit("~", 1)[-1]
    return tail[-2:].rjust(2, "0")


def fetch(iid: str, out_root: Path, retries: int = 5) -> tuple[str, str]:
    dest = out_root / shard_of(iid) / f"{iid}.json.gz"
    if dest.exists() and dest.stat().st_size > 0:
        return ("skip", iid)
    dest.parent.mkdir(parents=True, exist_ok=True)
    tmp = dest.with_suffix(".gz.part")
    url = MANIFEST_URL.format(id=iid)
    last = ""
    for attempt in range(retries):
        try:
            time.sleep(POLITE_SLEEP)
            req = urllib.request.Request(url, headers=UA)
            with urllib.request.urlopen(req, timeout=90) as resp:
                body = resp.read()
            if not body.lstrip().startswith(b"{"):
                raise ValueError("non-JSON response (captcha wall?)")
            with gzip.open(tmp, "wb", compresslevel=6) as fh:
                fh.write(body)
            tmp.replace(dest)
            return ("ok", iid)
        except Exception as e:  # noqa: BLE001 — network best-effort
            last = f"{type(e).__name__}: {e}"
            time.sleep(min(2 ** attempt, 30))
    tmp.unlink(missing_ok=True)
    return (f"FAIL {last}", iid)


def main() -> None:
    index, out, workers, limit = parse_args(sys.argv[1:])
    ids = [ln.split("\t", 1)[0] for ln in index.read_text(encoding="utf-8").splitlines() if ln.strip()]
    if limit:
        ids = ids[:limit]
    out.mkdir(parents=True, exist_ok=True)
    total = len(ids)
    print(f"index={index} out={out} manifests={total} workers={workers}", flush=True)

    done = ok = skip = fail = 0
    failures: list[str] = []
    with ThreadPoolExecutor(max_workers=workers) as ex:
        futs = [ex.submit(fetch, i, out) for i in ids]
        for fut in as_completed(futs):
            status, iid = fut.result()
            done += 1
            if status == "ok":
                ok += 1
            elif status == "skip":
                skip += 1
            else:
                fail += 1
                failures.append(f"{status}\t{iid}")
            if done % 1000 == 0 or done == total:
                print(f"  {done}/{total}  ok={ok} skip={skip} fail={fail}", flush=True)

    print(f"\nDONE manifests={total} ok={ok} skip={skip} fail={fail}", flush=True)
    fpath = out / "download_failures.txt"
    if failures:
        fpath.write_text("\n".join(failures) + "\n", encoding="utf-8")
        print(f"  {fail} failures -> {fpath} (re-run to retry only those)", flush=True)
    elif fpath.exists():
        fpath.unlink()


if __name__ == "__main__":
    main()
