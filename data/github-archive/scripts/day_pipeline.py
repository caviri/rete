#!/usr/bin/env python3
"""Full per-day pipeline for month-scale processing, stdlib downloads.

    python day_pipeline.py DAY [DAY...] [--emit-gz] [--rm-raw]

Per day: download the 24 hourly .json.gz (skip present, retry, atomic) →
to_parquet.py → optionally to_rdf.py with GZIP=1 → optionally delete the
day's raw files (reproducible via download.sh; Parquet is the keeper).
Designed to run inside python:3.12-slim in Docker (no curl needed).
"""
import os
import pathlib
import subprocess
import sys
import time
import urllib.request

BASE = pathlib.Path(__file__).resolve().parent.parent
RAW = BASE / "raw"
SCRIPTS = BASE / "scripts"

days = [a for a in sys.argv[1:] if not a.startswith("--")]
emit_gz = "--emit-gz" in sys.argv
rm_raw = "--rm-raw" in sys.argv


def fetch(day: str) -> None:
    for h in range(24):
        out = RAW / f"{day}-{h}.json.gz"
        if out.exists() and out.stat().st_size > 0:
            continue
        url = f"https://data.gharchive.org/{day}-{h}.json.gz"
        # data.gharchive.org 403s the default Python-urllib UA
        req = urllib.request.Request(url, headers={"User-Agent": "curl/8.5.0"})
        for attempt in range(4):
            try:
                tmp = out.with_suffix(".gz.part")
                with urllib.request.urlopen(req, timeout=120) as r, \
                        open(tmp, "wb") as f:
                    while chunk := r.read(1 << 20):
                        f.write(chunk)
                tmp.rename(out)
                break
            except Exception as e:
                if attempt == 3:
                    raise
                print(f"retry {day}-{h}: {e}", flush=True)
                time.sleep(5 * (attempt + 1))


def run(script: str, day: str, **env) -> None:
    e = dict(os.environ, DAY=day, **{k: str(v) for k, v in env.items()})
    subprocess.run([sys.executable, str(SCRIPTS / script)], env=e, check=True)


for day in days:
    t0 = time.time()
    print(f"=== {day}: download", flush=True)
    fetch(day)
    print(f"=== {day}: parquet", flush=True)
    run("to_parquet.py", day)
    if emit_gz:
        print(f"=== {day}: rdf (gz)", flush=True)
        run("to_rdf.py", day, GZIP=1)
    if rm_raw:
        for h in range(24):
            (RAW / f"{day}-{h}.json.gz").unlink(missing_ok=True)
        print(f"=== {day}: raw deleted", flush=True)
    print(f"=== {day}: done in {(time.time()-t0)/60:.1f} min", flush=True)
